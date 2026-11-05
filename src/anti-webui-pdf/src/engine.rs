//! The threads that draw. The UI thread says what it wants, most urgent first, and gets pictures
//! back over a channel. Saying it again replaces what hasn't started, which is how old requests
//! are dropped: nothing can stop a page that is being drawn. See `docs/design/26-pdf.md`.

use crate::PdfError;
use crate::plan::Job;
use anti_webui_core::channel::{UiSender, UiWork};
use anti_webui_core::images::Bitmap;
use std::panic::{AssertUnwindSafe, catch_unwind};
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
}

pub(crate) trait Session {
    fn draw(&mut self, job: Job) -> Bitmap;
}

/// An open document and the size of each page, in points.
pub(crate) type Opened = (Arc<dyn Document>, Vec<(f32, f32)>);

/// What the engine has to say to the UI thread.
pub(crate) enum Event {
    Opened(Vec<(f32, f32)>),
    Failed(PdfError),
    Drawn(Job, Arc<Bitmap>),
    /// It panicked. The page won't be asked for again.
    NotDrawn(Job),
}

/// A job a worker is on.
struct Running {
    job: Job,
    /// Since when it hasn't been wanted.
    unwanted: Option<Instant>,
}

struct State {
    doc: Option<Arc<dyn Document>>,
    wanted: Vec<Job>,
    running: Vec<Running>,
    workers: usize,
    idle: usize,
    /// One is waiting to see if the others stay stuck.
    standing_by: bool,
    stopped: bool,
    /// Tells those who wait for pictures that some are coming.
    work: Option<UiWork>,
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
        let Some(doc) = state.doc.clone() else {
            return;
        };
        let needed = state.wanted.len().min(self.max_workers);
        let hires = needed
            .saturating_sub(state.idle)
            .min(self.max_workers.saturating_sub(state.workers));
        for _ in 0..hires {
            state.workers += 1;
            let shared = self.clone();
            let doc = doc.clone();
            let started = std::thread::Builder::new()
                .name("pdf".into())
                .spawn(move || work(&shared, &*doc));
            if let Err(e) = started {
                log::warn!("can't start a thread to draw PDF pages: {e}");
                state.workers -= 1;
            }
        }
    }

    /// Since when every worker has been on something nobody wants, with something else wanted.
    fn stuck_since(state: &State) -> Option<Instant> {
        if state.wanted.is_empty() || state.idle > 0 || state.running.len() < state.workers {
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
        let Some(doc) = state.doc.clone() else {
            return;
        };
        state.standing_by = true;
        let shared = self.clone();
        let started = std::thread::Builder::new()
            .name("pdf".into())
            .spawn(move || {
                if shared.wait_until_stuck() {
                    work(&shared, &*doc);
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
        if state.wanted.is_empty() && state.running.is_empty() {
            state.work = None;
        } else if state.work.is_none() {
            state.work = Some(self.out.work());
        }
    }
}

/// A worker: sessions one after the other, of which there is only more than one after a panic.
fn work(shared: &Shared, doc: &dyn Document) {
    loop {
        let mut current = None;
        let done = catch_unwind(AssertUnwindSafe(|| session(shared, doc, &mut current)));
        if done.is_ok() {
            break;
        }
        let Some(job) = current else {
            continue;
        };
        shared.out.send(Event::NotDrawn(job));
        let mut state = shared.lock();
        state.running.retain(|r| r.job != job);
        shared.settle(&mut state);
    }
}

/// Draws until there has been nothing to do for a while.
fn session(shared: &Shared, doc: &dyn Document, current: &mut Option<Job>) {
    let mut session = doc.session();
    let mut first = true;
    loop {
        let job = {
            let mut state = shared.lock();
            loop {
                // One more than asked for is only there for one job. Leaving counts here, under the
                // lock, so that nobody counts on a worker that is on its way out.
                let extra = state.workers > shared.max_workers;
                if state.stopped || (extra && (!first || state.wanted.is_empty())) {
                    state.workers -= 1;
                    return;
                }
                if !state.wanted.is_empty() {
                    let job = state.wanted.remove(0);
                    state.running.push(Running {
                        job,
                        unwanted: None,
                    });
                    first = false;
                    break job;
                }
                state.idle += 1;
                let (next, timeout) = shared
                    .changed
                    .wait_timeout(state, IDLE)
                    .expect("no panics with the lock held");
                state = next;
                state.idle -= 1;
                if timeout.timed_out() && state.wanted.is_empty() {
                    state.workers -= 1;
                    return;
                }
            }
        };
        *current = Some(job);
        let bitmap = session.draw(job);
        *current = None;
        shared.out.send(Event::Drawn(job, Arc::new(bitmap)));
        let mut state = shared.lock();
        state.running.retain(|r| r.job != job);
        shared.settle(&mut state);
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
                wanted: Vec::new(),
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
                            state.doc = Some(doc);
                            thread.hire(&mut state);
                        }
                        thread.out.send(Event::Opened(pages));
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
        for running in &mut state.running {
            if jobs.contains(&running.job) {
                running.unwanted = None;
            } else {
                running.unwanted.get_or_insert(now);
            }
        }
        let mut wanted = jobs;
        wanted.retain(|job| !state.running.iter().any(|r| r.job == *job));
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

    /// Lets the workers go. Those drawing finish that first.
    pub fn stop(&self) {
        let mut state = self.shared.lock();
        state.stopped = true;
        state.wanted.clear();
        state.work = None;
        drop(state);
        self.shared.changed.notify_all();
    }
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
                        Event::Drawn(job, _) | Event::NotDrawn(job) => pages.push(job.page),
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
