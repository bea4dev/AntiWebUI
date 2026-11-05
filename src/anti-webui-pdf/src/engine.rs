//! The threads that draw and read the text of pages. The UI thread says what it wants, most urgent
//! first, and gets it back over a channel. Saying it again replaces what hasn't started, which is how old requests
//! are dropped: nothing can stop a page that is being drawn. See `docs/design/26-pdf.md`.

use crate::PdfError;
use crate::edits::Edits;
use crate::links::PdfOutlineItem;
use crate::marks::Marks;
use crate::plan::Job;
use crate::text::PageText;
use anti_webui_core::channel::{UiSender, UiWork};
use anti_webui_core::geometry::RectI;
use anti_webui_core::images::Bitmap;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a worker with nothing to do stays, with what it has cached.
const IDLE: Duration = Duration::from_secs(10);
/// How long all of the workers can be stuck drawing what nobody wants any more before another one
/// starts on what is wanted.
const STUCK: Duration = Duration::from_millis(200);
/// How many workers there can be with those that started because the others were stuck.
const MAX_STUCK_WORKERS: usize = 8;

/// A document that has been opened.
pub(crate) trait Document: Send + Sync + 'static {
    /// What a worker draws with, which holds its caches. It is thrown away after a panic.
    fn session(&self) -> Box<dyn Session + '_>;

    /// What it has for a table of contents. It is read once, after opening.
    fn outline(&self) -> Vec<PdfOutlineItem> {
        Vec::new()
    }

    /// The fields and the annotations it has. It is read once, after opening.
    fn marks(&self) -> Arc<Marks> {
        Arc::default()
    }

    /// The same document as it was opened with edits made to it.
    fn edited(self: Arc<Self>, _: &Edits) -> Result<Arc<dyn Document>, String> {
        Err("this document can't be edited".to_owned())
    }

    /// What a file of the document with edits made to it has in it.
    fn saved(&self, _: &Edits) -> Result<Vec<u8>, String> {
        Err("this document can't be saved".to_owned())
    }
}

pub(crate) trait Session {
    fn draw(&mut self, job: Job) -> Bitmap;

    /// A picture of only `boxes` of a job, without the white of the page.
    fn cut_out(&mut self, job: Job, boxes: &[RectI]) -> Bitmap;

    fn text(&mut self, page: usize) -> PageText;
}

/// An open document and the size of each page, in points.
pub(crate) type Opened = (Arc<dyn Document>, Vec<(f32, f32)>);

/// A picture of part of a page that shows with the pointer when dragging what is selected.
#[derive(Debug, PartialEq)]
pub(crate) struct CutOut {
    /// Which press it is for.
    pub press: u64,
    pub job: Job,
    /// What of the picture to keep.
    pub boxes: Vec<RectI>,
    /// Where it goes in the box of what is dragged.
    pub at: RectI,
}

/// What the engine has to say to the UI thread.
pub(crate) enum Event {
    Opened(Vec<(f32, f32)>),
    Failed(PdfError),
    Drawn(Job, Arc<Bitmap>),
    /// It panicked. The page won't be asked for again.
    NotDrawn(Job),
    /// A thumbnail, which there is none of if drawing it panicked.
    Thumbnail(Job, Option<Arc<Bitmap>>),
    CutOut(Arc<CutOut>, Arc<Bitmap>),
    /// The text of a page, which there is none of if reading it panicked.
    Text(usize, Arc<PageText>),
    Outline(Vec<PdfOutlineItem>),
    Marks(Arc<Marks>),
    /// From here on everything is of the document with these edits.
    Edited(Arc<Edits>),
    /// The edits couldn't be made, with why.
    NotEdited(String),
    /// A file was written, or why not.
    Saved(PathBuf, Result<(), String>),
}

/// What a worker does.
#[derive(Clone, PartialEq)]
enum Work {
    Draw(Job),
    Thumbnail(Job),
    Text(usize),
    CutOut(Arc<CutOut>),
}

/// What a worker is on.
struct Running {
    work: Work,
    /// Which version of the document it is for.
    version: u64,
    /// Since when it hasn't been wanted, or something else has been more. Text doesn't take long
    /// enough for this to matter.
    unwanted: Option<Instant>,
}

struct State {
    doc: Option<Arc<dyn Document>>,
    /// How many times `doc` was replaced.
    version: u64,
    /// The edits to make next, of which only the last matters as each says all of them.
    edits: Option<Arc<Edits>>,
    /// A thread is making edits.
    editing: bool,
    wanted: Vec<Job>,
    /// What shows with the pointer, which comes before everything else.
    cut_outs: Vec<Arc<CutOut>>,
    /// The thumbnails, which come after all of the pictures of the pages.
    thumbnails: Vec<Job>,
    /// The text someone is waiting for, which comes before all pictures.
    text_now: Vec<usize>,
    /// The text searching goes through, which comes after them.
    text_later: Vec<usize>,
    /// The text that is on its way to the UI thread, which doesn't know yet and may ask again.
    text_sent: HashSet<usize>,
    running: Vec<Running>,
    workers: usize,
    idle: usize,
    /// One is waiting to see if the others stay stuck.
    standing_by: bool,
    stopped: bool,
    /// Tells those who wait for pictures that some are coming.
    work: Option<UiWork>,
}

impl State {
    fn nothing_wanted(&self) -> bool {
        self.wanted.is_empty()
            && self.cut_outs.is_empty()
            && self.thumbnails.is_empty()
            && self.text_now.is_empty()
            && self.text_later.is_empty()
    }

    fn next_work(&mut self) -> Option<Work> {
        if !self.cut_outs.is_empty() {
            return Some(Work::CutOut(self.cut_outs.remove(0)));
        }
        if !self.text_now.is_empty() {
            return Some(Work::Text(self.text_now.remove(0)));
        }
        if !self.wanted.is_empty() {
            return Some(Work::Draw(self.wanted.remove(0)));
        }
        if !self.thumbnails.is_empty() {
            return Some(Work::Thumbnail(self.thumbnails.remove(0)));
        }
        if !self.text_later.is_empty() {
            return Some(Work::Text(self.text_later.remove(0)));
        }
        None
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    out: UiSender<Event>,
    max_workers: usize,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("no panics with the lock held")
    }

    /// Starts as many workers as what is wanted can use.
    fn hire(self: &Arc<Self>, state: &mut State) {
        if state.doc.is_none() {
            return;
        }
        let wanted = state.wanted.len()
            + state.cut_outs.len()
            + state.thumbnails.len()
            + state.text_now.len()
            + state.text_later.len();
        let needed = wanted.min(self.max_workers);
        let hires = needed
            .saturating_sub(state.idle)
            .min(self.max_workers.saturating_sub(state.workers));
        for _ in 0..hires {
            state.workers += 1;
            let shared = self.clone();
            let started = std::thread::Builder::new()
                .name("pdf".into())
                .spawn(move || work(&shared));
            if let Err(e) = started {
                log::warn!("can't start a thread to draw PDF pages: {e}");
                state.workers -= 1;
            }
        }
    }

    /// Since when every worker has been on something nobody wants, with something else wanted.
    fn stuck_since(state: &State) -> Option<Instant> {
        if state.nothing_wanted() || state.idle > 0 || state.running.len() < state.workers {
            return None;
        }
        let mut latest = None;
        for running in &state.running {
            latest = latest.max(Some(running.unwanted?));
        }
        latest
    }

    /// Gets ready to start another worker if the others stay stuck. It is gone again after a job.
    fn stand_by(self: &Arc<Self>, state: &mut State) {
        let most = self.max_workers.max(MAX_STUCK_WORKERS);
        if state.standing_by || state.workers >= most || Self::stuck_since(state).is_none() {
            return;
        }
        if state.doc.is_none() {
            return;
        }
        state.standing_by = true;
        let shared = self.clone();
        let started = std::thread::Builder::new()
            .name("pdf".into())
            .spawn(move || {
                if shared.wait_until_stuck() {
                    work(&shared);
                }
            });
        if let Err(e) = started {
            log::warn!("can't start a thread to draw PDF pages: {e}");
            state.standing_by = false;
        }
    }

    /// Whether the workers have stayed stuck for long enough, which makes this one of them.
    fn wait_until_stuck(&self) -> bool {
        let mut state = self.lock();
        loop {
            let since = (!state.stopped)
                .then(|| Self::stuck_since(&state))
                .flatten();
            let Some(since) = since else {
                state.standing_by = false;
                return false;
            };
            let waited = since.elapsed();
            if waited >= STUCK {
                state.standing_by = false;
                state.workers += 1;
                return true;
            }
            state = self
                .changed
                .wait_timeout(state, STUCK - waited)
                .expect("no panics with the lock held")
                .0;
        }
    }

    fn settle(&self, state: &mut State) {
        if state.nothing_wanted() && state.running.is_empty() && !state.editing {
            state.work = None;
        } else if state.work.is_none() {
            state.work = Some(self.out.work());
        }
    }
}

/// How a session ended.
enum Ended {
    /// There is nothing more for this worker to do.
    Left,
    /// There is another version of the document to do it with.
    NewVersion,
}

/// A worker: sessions one after the other, of which there is only more than one after a panic or
/// when the document changes.
fn work(shared: &Shared) {
    loop {
        let Some((doc, version)) = ({
            let state = shared.lock();
            state.doc.clone().map(|doc| (doc, state.version))
        }) else {
            shared.lock().workers -= 1;
            return;
        };
        let mut current = None;
        let ended = catch_unwind(AssertUnwindSafe(|| {
            session(shared, &*doc, version, &mut current)
        }));
        match ended {
            Ok(Ended::Left) => {
                // What it kept is small pieces, which `malloc` wouldn't give back by itself.
                #[cfg(all(target_os = "linux", target_env = "gnu"))]
                // SAFETY: this takes no pointers and only touches what `malloc` has free.
                unsafe {
                    libc::malloc_trim(0);
                }
                return;
            }
            Ok(Ended::NewVersion) => continue,
            Err(_) => {}
        }
        let Some(work) = current else {
            continue;
        };
        let mut state = shared.lock();
        if version == state.version {
            if let Work::Text(page) = work {
                state.text_sent.insert(page);
            }
            match &work {
                Work::Draw(job) => shared.out.send(Event::NotDrawn(*job)),
                Work::Thumbnail(job) => shared.out.send(Event::Thumbnail(*job, None)),
                Work::Text(page) => shared.out.send(Event::Text(*page, Arc::default())),
                // It only shows for as long as it is dragged.
                Work::CutOut(_) => {}
            }
        }
        state
            .running
            .retain(|r| r.work != work || r.version != version);
        shared.settle(&mut state);
    }
}

/// Draws until there has been nothing to do for a while.
fn session(shared: &Shared, doc: &dyn Document, version: u64, current: &mut Option<Work>) -> Ended {
    let mut session = doc.session();
    let mut first = true;
    loop {
        let work = {
            let mut state = shared.lock();
            loop {
                // One more than asked for is only there for one job. Leaving counts here, under the
                // lock, so that nobody counts on a worker that is on its way out.
                let extra = state.workers > shared.max_workers;
                if state.stopped || (extra && (!first || state.nothing_wanted())) {
                    state.workers -= 1;
                    return Ended::Left;
                }
                if state.version != version {
                    return Ended::NewVersion;
                }
                if let Some(work) = state.next_work() {
                    state.running.push(Running {
                        work: work.clone(),
                        version,
                        unwanted: None,
                    });
                    first = false;
                    break work;
                }
                state.idle += 1;
                let (next, timeout) = shared
                    .changed
                    .wait_timeout(state, IDLE)
                    .expect("no panics with the lock held");
                state = next;
                state.idle -= 1;
                if timeout.timed_out() && state.nothing_wanted() {
                    state.workers -= 1;
                    return Ended::Left;
                }
            }
        };
        *current = Some(work.clone());
        let event = match &work {
            Work::Draw(job) => Event::Drawn(*job, Arc::new(session.draw(*job))),
            Work::Thumbnail(job) => Event::Thumbnail(*job, Some(Arc::new(session.draw(*job)))),
            Work::Text(page) => Event::Text(*page, Arc::new(session.text(*page))),
            Work::CutOut(cut) => {
                let picture = session.cut_out(cut.job, &cut.boxes);
                Event::CutOut(cut.clone(), Arc::new(picture))
            }
        };
        *current = None;
        let mut state = shared.lock();
        // Under the lock, so that nothing of an old version comes after saying there is a new one.
        if state.version == version {
            if let Work::Text(page) = work {
                state.text_sent.insert(page);
            }
            shared.out.send(event);
        }
        state
            .running
            .retain(|r| r.work != work || r.version != version);
        shared.settle(&mut state);
    }
}

/// Makes the edits that were asked for until there are none left.
fn make_edits(shared: &Shared) {
    loop {
        let (doc, edits) = {
            let mut state = shared.lock();
            let doc = state.doc.clone();
            match (doc, state.edits.take()) {
                (Some(doc), Some(edits)) if !state.stopped => (doc, edits),
                _ => {
                    state.editing = false;
                    shared.settle(&mut state);
                    return;
                }
            }
        };
        let edited = catch_unwind(AssertUnwindSafe(|| doc.edited(&edits)))
            .unwrap_or_else(|_| Err("making the edits panicked".to_owned()));
        let mut state = shared.lock();
        match edited {
            Ok(doc) => {
                state.doc = Some(doc);
                state.version += 1;
                // What these were for won't come.
                state.text_sent.clear();
                shared.out.send(Event::Edited(edits));
            }
            Err(e) => shared.out.send(Event::NotEdited(e)),
        }
        drop(state);
        shared.changed.notify_all();
    }
}

/// See the top of this file.
pub(crate) struct Engine {
    shared: Arc<Shared>,
}

impl Engine {
    /// Opens a document on another thread, with `open` saying how, and tells `out` how it went.
    /// The pages come with it.
    pub fn open(
        open: impl FnOnce() -> Result<Opened, PdfError> + Send + 'static,
        out: UiSender<Event>,
        max_workers: usize,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                doc: None,
                version: 0,
                edits: None,
                editing: false,
                wanted: Vec::new(),
                cut_outs: Vec::new(),
                thumbnails: Vec::new(),
                text_now: Vec::new(),
                text_later: Vec::new(),
                text_sent: HashSet::new(),
                running: Vec::new(),
                workers: 0,
                idle: 0,
                standing_by: false,
                stopped: false,
                work: None,
            }),
            changed: Condvar::new(),
            out,
            max_workers: max_workers.max(1),
        });
        let opening = shared.out.work();
        let thread = shared.clone();
        let started = std::thread::Builder::new()
            .name("pdf".into())
            .spawn(move || {
                let opened = catch_unwind(AssertUnwindSafe(open)).unwrap_or(Err(PdfError::Invalid));
                match opened {
                    Ok((doc, pages)) => {
                        // In this order, so that wanting pages can only start once they can be drawn.
                        {
                            let mut state = thread.lock();
                            state.doc = Some(doc.clone());
                            thread.hire(&mut state);
                        }
                        thread.out.send(Event::Opened(pages));
                        // After the first pictures are on their way, as it can take a while.
                        let outline = catch_unwind(AssertUnwindSafe(|| doc.outline()));
                        thread.out.send(Event::Outline(outline.unwrap_or_default()));
                        let marks = catch_unwind(AssertUnwindSafe(|| doc.marks()));
                        thread.out.send(Event::Marks(marks.unwrap_or_default()));
                    }
                    Err(e) => thread.out.send(Event::Failed(e)),
                }
                drop(opening);
            });
        if let Err(e) = started {
            shared
                .out
                .send(Event::Failed(PdfError::Source(e.to_string())));
        }
        Self { shared }
    }

    /// Says what is wanted, most urgent first, instead of what was wanted before.
    pub fn want(&self, jobs: Vec<Job>) {
        let mut state = self.shared.lock();
        let now = Instant::now();
        let version = state.version;
        for running in &mut state.running {
            let wanted = match &running.work {
                Work::Draw(job) => running.version == version && jobs.contains(job),
                // There is something better to do than this, however much it is wanted.
                Work::Thumbnail(_) => jobs.is_empty(),
                Work::Text(_) | Work::CutOut(_) => continue,
            };
            if wanted {
                running.unwanted = None;
            } else {
                running.unwanted.get_or_insert(now);
            }
        }
        let mut wanted = jobs;
        wanted.retain(|job| {
            !state
                .running
                .iter()
                .any(|r| r.version == version && r.work == Work::Draw(*job))
        });
        if state.wanted == wanted {
            self.shared.stand_by(&mut state);
            return;
        }
        state.wanted = wanted;
        self.shared.hire(&mut state);
        self.shared.stand_by(&mut state);
        self.shared.settle(&mut state);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Says what thumbnails are wanted, most urgent first, instead of what was wanted before.
    pub fn want_thumbnails(&self, jobs: Vec<Job>) {
        let mut state = self.shared.lock();
        let version = state.version;
        let mut wanted = jobs;
        wanted.retain(|job| {
            !state
                .running
                .iter()
                .any(|r| r.version == version && r.work == Work::Thumbnail(*job))
        });
        if state.thumbnails == wanted {
            return;
        }
        state.thumbnails = wanted;
        self.shared.hire(&mut state);
        self.shared.settle(&mut state);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Says what should show with the pointer if what is selected gets dragged.
    pub fn want_cut_outs(&self, cuts: Vec<CutOut>) {
        let mut state = self.shared.lock();
        state.cut_outs = cuts.into_iter().map(Arc::new).collect();
        self.shared.hire(&mut state);
        self.shared.stand_by(&mut state);
        self.shared.settle(&mut state);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Says what text is wanted instead of what was wanted before: what someone is waiting for, and
    /// what can wait until the pictures are done.
    pub fn want_text(&self, now: Vec<usize>, later: Vec<usize>) {
        let mut state = self.shared.lock();
        let not_running = |state: &State, pages: Vec<usize>| {
            let mut pages = pages;
            pages.retain(|&page| {
                !state.text_sent.contains(&page)
                    && !state
                        .running
                        .iter()
                        .any(|r| r.version == state.version && r.work == Work::Text(page))
            });
            pages
        };
        let now = not_running(&state, now);
        let later = not_running(&state, later);
        if state.text_now == now && state.text_later == later {
            return;
        }
        state.text_now = now;
        state.text_later = later;
        self.shared.hire(&mut state);
        self.shared.stand_by(&mut state);
        self.shared.settle(&mut state);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Says that the text of a page has arrived, so that asking for it again means it.
    pub fn text_received(&self, page: usize) {
        self.shared.lock().text_sent.remove(&page);
    }

    /// Replaces the document with the one that was opened with these edits, on another thread, and
    /// says so with [`Event::Edited`].
    pub fn edit(&self, edits: Arc<Edits>) {
        let mut state = self.shared.lock();
        state.edits = Some(edits);
        if !state.editing {
            state.editing = true;
            let shared = self.shared.clone();
            let started = std::thread::Builder::new()
                .name("pdf edits".into())
                .spawn(move || make_edits(&shared));
            if let Err(e) = started {
                state.editing = false;
                state.edits = None;
                self.shared.out.send(Event::NotEdited(e.to_string()));
            }
        }
        self.shared.settle(&mut state);
    }

    /// Writes the document with these edits to a file, on another thread. It first goes to another
    /// file next to it, so that nothing happens to what is there when it doesn't work out.
    pub fn save(&self, edits: Arc<Edits>, path: PathBuf) {
        let state = self.shared.lock();
        let Some(doc) = state.doc.clone() else {
            self.shared
                .out
                .send(Event::Saved(path, Err("it isn't open yet".to_owned())));
            return;
        };
        let out = self.shared.out.clone();
        let work = self.shared.out.work();
        let target = path.clone();
        let started = std::thread::Builder::new()
            .name("pdf save".into())
            .spawn(move || {
                let saved = catch_unwind(AssertUnwindSafe(|| doc.saved(&edits)))
                    .unwrap_or_else(|_| Err("saving panicked".to_owned()))
                    .and_then(|bytes| write_file(&target, &bytes));
                out.send(Event::Saved(target, saved));
                drop(work);
            });
        if let Err(e) = started {
            self.shared.out.send(Event::Saved(path, Err(e.to_string())));
        }
    }

    /// Lets the workers go. Those drawing finish that first.
    pub fn stop(&self) {
        let mut state = self.shared.lock();
        state.stopped = true;
        state.wanted.clear();
        state.cut_outs.clear();
        state.thumbnails.clear();
        state.text_now.clear();
        state.text_later.clear();
        state.work = None;
        drop(state);
        self.shared.changed.notify_all();
    }
}

/// Writes a file so that it is there as a whole or not at all.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.part", std::process::id()));
    let part = path.with_file_name(name);
    let written = std::fs::write(&part, bytes).and_then(|()| {
        // A file keeps what others may do with it.
        if let Ok(old) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&part, old.permissions());
        }
        std::fs::rename(&part, path)
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    Ok(())
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anti_webui_core::channel::ui_channel;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Draws one pixel, and waits for a turn when told to.
    struct Fake {
        gate: Option<Arc<(Mutex<usize>, Condvar)>>,
        /// The only page that waits for a turn, if any is given here.
        stuck_on: Option<usize>,
        sessions: AtomicUsize,
        panics_on: Option<usize>,
    }

    struct FakeSession<'a>(&'a Fake);

    impl Document for Fake {
        fn session(&self) -> Box<dyn Session + '_> {
            self.sessions.fetch_add(1, Ordering::Relaxed);
            Box::new(FakeSession(self))
        }
    }

    impl Session for FakeSession<'_> {
        fn text(&mut self, _: usize) -> PageText {
            PageText::default()
        }

        fn cut_out(&mut self, job: Job, _: &[RectI]) -> Bitmap {
            self.draw(job)
        }

        fn draw(&mut self, job: Job) -> Bitmap {
            let waits = self.0.stuck_on.is_none_or(|page| page == job.page);
            if let Some(gate) = self.0.gate.as_ref().filter(|_| waits) {
                let (turns, moved) = &**gate;
                let mut turns = turns.lock().unwrap();
                while *turns == 0 {
                    turns = moved.wait(turns).unwrap();
                }
                *turns -= 1;
            }
            assert_ne!(self.0.panics_on, Some(job.page));
            Bitmap::from_premultiplied(1, 1, vec![job.page as u8, 0, 0, 255], true)
        }
    }

    fn job(page: usize) -> Job {
        Job {
            page,
            width: 1,
            height: 1,
            area: None,
            turn: Default::default(),
        }
    }

    struct Rig {
        engine: Engine,
        seen: std::rc::Rc<std::cell::RefCell<VecDeque<Event>>>,
    }

    impl Rig {
        fn new(doc: Fake, workers: usize) -> Self {
            let seen = std::rc::Rc::new(std::cell::RefCell::new(VecDeque::new()));
            let out = {
                let seen = seen.clone();
                ui_channel(move |e: Event| seen.borrow_mut().push_back(e))
            };
            let engine = Engine::open(
                move || Ok((Arc::new(doc) as Arc<dyn Document>, vec![(1.0, 1.0)])),
                out,
                workers,
            );
            Self { engine, seen }
        }

        /// Pumps the channel until `count` pages came, which is all a window would do.
        fn drawn(&self, count: usize) -> Vec<usize> {
            let end = std::time::Instant::now() + Duration::from_secs(20);
            let mut pages = Vec::new();
            while pages.len() < count {
                assert!(std::time::Instant::now() < end, "only {pages:?}");
                anti_webui_core::channel::pump();
                while let Some(e) = self.seen.borrow_mut().pop_front() {
                    match e {
                        Event::Drawn(job, _) | Event::NotDrawn(job) | Event::Thumbnail(job, _) => {
                            pages.push(job.page)
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            pages
        }
    }

    fn fake() -> Fake {
        Fake {
            gate: None,
            stuck_on: None,
            sessions: AtomicUsize::new(0),
            panics_on: None,
        }
    }

    #[test]
    fn what_is_wanted_comes_most_urgent_first() {
        let rig = Rig::new(fake(), 1);
        rig.engine.want(vec![job(3), job(1), job(2)]);
        assert_eq!(rig.drawn(3), [3, 1, 2]);
    }

    #[test]
    fn saying_it_again_drops_what_has_not_started() {
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let rig = Rig::new(
            Fake {
                gate: Some(gate.clone()),
                ..fake()
            },
            1,
        );
        rig.engine.want(vec![job(1), job(2), job(3)]);
        // Page 1 is being drawn, or about to be, so it stays. The others go.
        while rig.engine.shared.lock().running.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        rig.engine.want(vec![job(7)]);
        *gate.0.lock().unwrap() = 2;
        gate.1.notify_all();
        assert_eq!(rig.drawn(2), [1, 7]);
        assert!(rig.engine.shared.lock().wanted.is_empty());
    }

    #[test]
    fn what_is_being_drawn_is_not_drawn_twice() {
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let rig = Rig::new(
            Fake {
                gate: Some(gate.clone()),
                ..fake()
            },
            1,
        );
        rig.engine.want(vec![job(1)]);
        while rig.engine.shared.lock().running.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        rig.engine.want(vec![job(1), job(2)]);
        assert_eq!(rig.engine.shared.lock().wanted, [job(2)]);
        *gate.0.lock().unwrap() = 2;
        gate.1.notify_all();
        assert_eq!(rig.drawn(2), [1, 2]);
    }

    #[test]
    fn thumbnails_wait_for_the_pictures_of_the_pages() {
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let rig = Rig::new(
            Fake {
                gate: Some(gate.clone()),
                stuck_on: Some(0),
                ..fake()
            },
            1,
        );
        rig.engine.want(vec![job(0)]);
        while rig.engine.shared.lock().running.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        rig.engine.want_thumbnails(vec![job(5), job(6)]);
        rig.engine.want(vec![job(0), job(1)]);
        *gate.0.lock().unwrap() = 1;
        gate.1.notify_all();
        assert_eq!(rig.drawn(4), [0, 1, 5, 6]);
    }

    #[test]
    fn a_thumbnail_that_panics_says_so_and_loses_nothing_else() {
        let rig = Rig::new(
            Fake {
                panics_on: Some(2),
                ..fake()
            },
            1,
        );
        rig.engine.want_thumbnails(vec![job(1), job(2), job(3)]);
        assert_eq!(rig.drawn(3), [1, 2, 3]);
    }

    #[test]
    fn a_panic_loses_one_page_and_the_caches_of_that_worker() {
        let rig = Rig::new(
            Fake {
                panics_on: Some(2),
                ..fake()
            },
            1,
        );
        rig.engine.want(vec![job(1), job(2), job(3)]);
        assert_eq!(rig.drawn(3), [1, 2, 3]);
    }

    #[test]
    fn another_worker_takes_over_when_all_are_stuck_on_what_is_not_wanted() {
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let rig = Rig::new(
            Fake {
                gate: Some(gate.clone()),
                stuck_on: Some(1),
                ..fake()
            },
            1,
        );
        rig.engine.want(vec![job(1)]);
        while rig.engine.shared.lock().running.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        rig.engine.want(vec![job(2)]);
        // Page 1 doesn't finish until told to, and page 2 gets drawn anyway.
        assert_eq!(rig.drawn(1), [2]);
        *gate.0.lock().unwrap() = 1;
        gate.1.notify_all();
        assert_eq!(rig.drawn(1), [1]);
        // Back to as many as it was told to have, when it has nothing to do.
        let end = std::time::Instant::now() + Duration::from_secs(20);
        while rig.engine.shared.lock().workers > 1 {
            assert!(std::time::Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn nobody_is_stuck_on_what_is_still_wanted() {
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let rig = Rig::new(
            Fake {
                gate: Some(gate.clone()),
                stuck_on: Some(1),
                ..fake()
            },
            1,
        );
        rig.engine.want(vec![job(1)]);
        while rig.engine.shared.lock().running.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        rig.engine.want(vec![job(1), job(2)]);
        std::thread::sleep(STUCK * 2);
        assert_eq!(rig.engine.shared.lock().workers, 1);
        *gate.0.lock().unwrap() = 1;
        gate.1.notify_all();
        assert_eq!(rig.drawn(2), [1, 2]);
    }

    #[test]
    fn a_document_that_cannot_be_opened_says_so() {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let out = {
            let seen = seen.clone();
            ui_channel(move |e: Event| seen.borrow_mut().push(e))
        };
        let _engine = Engine::open(|| Err(PdfError::NeedsPassword), out, 1);
        let end = std::time::Instant::now() + Duration::from_secs(20);
        while seen.borrow().is_empty() {
            assert!(std::time::Instant::now() < end);
            anti_webui_core::channel::pump();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(
            seen.borrow()[0],
            Event::Failed(PdfError::NeedsPassword)
        ));
    }
}
