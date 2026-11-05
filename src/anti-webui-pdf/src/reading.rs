//! Reading a document rather than looking at it: selecting and copying text, searching, and
//! following links. All of it runs on the UI thread, with the text of the pages coming from the
//! workers when it is asked for. See `docs/design/26-pdf.md`.

use crate::engine::Engine;
use crate::links::{LinkTarget, PdfTarget};
use crate::plan::PageTable;
use crate::text::{self, NEAR_TEXT, PageText};
use crate::{PdfViewer, UrlListener};
use anti_webui_core::animation::request_frame;
use anti_webui_core::event::{ClipboardKind, PointerButton, PointerEvent};
use anti_webui_core::geometry::{PointF, RectI};
use anti_webui_core::prelude::*;
use anti_webui_core::view::{clear_text_selection, set_clipboard};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many pages keep where their glyphs are. Pages that show always do. It only takes a moment to
/// read a page again, and all of a long document would take tens of megabytes.
const KEPT_PAGES: usize = 32;
/// How many pages searching and copying ask for at a time, so that they stop soon when something
/// else is wanted.
const READ_AHEAD: usize = 64;
/// How long searching can go on in a frame through text that is there already.
const SEARCH_SLICE: Duration = Duration::from_millis(3);
/// How far, in logical pixels, a press can move and still be a click.
const CLICK_SLOP: f32 = 4.0;
/// How many pixels a frame scrolling goes for each pixel the pointer is outside of the box when
/// selecting.
const AUTOSCROLL: f32 = 0.25;

/// A place between two glyphs of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Place {
    pub page: usize,
    /// Past the last one is the end of the page.
    pub glyph: usize,
}

/// What was selected, the start before the end.
pub(crate) type Selection = (Place, Place);

/// The part of a page that is selected, as a range of its glyphs.
fn selected_on(selection: Selection, page: usize, len: usize) -> (usize, usize) {
    let (a, b) = selection;
    if page < a.page || page > b.page {
        return (0, 0);
    }
    (
        if page == a.page { a.glyph.min(len) } else { 0 },
        if page == b.page {
            b.glyph.min(len)
        } else {
            len
        },
    )
}

/// What selecting goes by, from how many times it was clicked.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Glyph,
    Word,
    Line,
}

/// Something that was found, by its page and its place among what is on that page.
pub(crate) type HitPlace = (usize, usize);

/// What a search has found so far, by page.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Hits {
    pages: Vec<(usize, Vec<(u32, u32)>)>,
    pub count: usize,
}

impl Hits {
    fn insert(&mut self, page: usize, found: Vec<(u32, u32)>) {
        if found.is_empty() {
            return;
        }
        self.count += found.len();
        let at = self.pages.partition_point(|p| p.0 < page);
        self.pages.insert(at, (page, found));
    }

    pub fn on(&self, page: usize) -> &[(u32, u32)] {
        match self.pages.binary_search_by_key(&page, |p| p.0) {
            Ok(at) => &self.pages[at].1,
            Err(_) => &[],
        }
    }

    /// How many come before a hit.
    pub fn index_of(&self, hit: HitPlace) -> usize {
        let at = self.pages.partition_point(|p| p.0 < hit.0);
        self.pages[..at].iter().map(|p| p.1.len()).sum::<usize>() + hit.1
    }

    /// The first one on a page or after it, or else the first one of all.
    pub fn first_from(&self, page: usize) -> Option<HitPlace> {
        let at = self.pages.partition_point(|p| p.0 < page);
        self.pages.get(at).or(self.pages.first()).map(|p| (p.0, 0))
    }

    /// The one after a hit, going around.
    pub fn after(&self, hit: HitPlace) -> Option<HitPlace> {
        if hit.1 + 1 < self.on(hit.0).len() {
            return Some((hit.0, hit.1 + 1));
        }
        self.first_from(hit.0 + 1)
    }

    /// The one before a hit, going around.
    pub fn before(&self, hit: HitPlace) -> Option<HitPlace> {
        if hit.1 > 0 {
            return Some((hit.0, hit.1 - 1));
        }
        let at = self.pages.partition_point(|p| p.0 < hit.0);
        let page = if at > 0 {
            self.pages.get(at - 1)
        } else {
            self.pages.last()
        }?;
        Some((page.0, page.1.len() - 1))
    }
}

/// A search that is going on.
struct Search {
    query: String,
    start: usize,
    /// How many pages from `start` it has been through.
    done: usize,
    /// It has shown what it found first.
    shown_first: bool,
    pump_is_due: bool,
}

/// Text that is on its way to a clipboard, as all of a selection may not have been read.
struct Copying {
    kind: ClipboardKind,
    selection: Selection,
    /// What each page of the selection gets.
    parts: Vec<Option<String>>,
    /// All of the parts before this one are there.
    next: usize,
}

/// A press of the button, and what came of it so far.
#[derive(Clone, Copy, Debug)]
struct Press {
    position: PointF,
    local: PointF,
    clicks: u32,
    shift: bool,
    moved: bool,
    /// It is known what it pressed, which takes the text of the page.
    resolved: bool,
}

/// A box on a page, in its pixels, and what it is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PageBox {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub kind: BoxKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BoxKind {
    Found,
    Current,
    Selected,
}

/// See the top of this file. A [`crate::PdfView`] has one.
pub(crate) struct Reader {
    pub viewer: PdfViewer,
    pub table: Memo<Rc<PageTable>>,
    pub pages: Signal<Rc<Vec<(f32, f32)>>>,
    pub view: Rc<dyn Fn() -> RectI>,
    pub scroller: NodeRef,
    pub on_link: Option<UrlListener>,
    pub engine: Rc<RefCell<Option<Engine>>>,

    texts: Signal<HashMap<usize, Arc<PageText>>>,
    /// The pages of `texts`, oldest first.
    arrived: RefCell<VecDeque<usize>>,
    /// The pages that should have their text in `texts`.
    needed: RefCell<Vec<usize>>,
    pointer_page: Signal<Option<usize>>,
    /// Where each page counts as text, once it has been read, and how it looks there.
    pub cursor: Signal<CursorIcon>,

    press: Cell<Option<Press>>,
    /// What selecting started from and goes by.
    selecting: Cell<Option<(Selection, Unit)>>,
    /// Where it stays when extending with Shift.
    anchor: Cell<Option<Place>>,
    scrolling: Cell<bool>,

    /// What is searched of each page, which stays when the rest of its text goes. All of a long
    /// document only takes as much as its text.
    folded: RefCell<HashMap<usize, Arc<str>>>,
    search: RefCell<Option<Search>>,
    /// The next pages a search wants to read.
    search_wants: Signal<Vec<usize>>,

    copying: RefCell<Option<Copying>>,
    /// The pages it takes to copy what is being copied.
    copy_wants: Signal<Vec<usize>>,
}

impl Reader {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        viewer: PdfViewer,
        table: Memo<Rc<PageTable>>,
        pages: Signal<Rc<Vec<(f32, f32)>>>,
        view: Rc<dyn Fn() -> RectI>,
        scroller: NodeRef,
        on_link: Option<UrlListener>,
        engine: Rc<RefCell<Option<Engine>>>,
    ) -> Rc<Self> {
        Rc::new(Self {
            viewer,
            table,
            pages,
            view,
            scroller,
            on_link,
            engine,
            texts: signal(HashMap::new()),
            arrived: RefCell::default(),
            needed: RefCell::default(),
            pointer_page: signal(None),
            cursor: signal(CursorIcon::Default),
            press: Cell::new(None),
            selecting: Cell::new(None),
            anchor: Cell::new(None),
            scrolling: Cell::new(false),
            folded: RefCell::default(),
            search: RefCell::new(None),
            search_wants: signal(Vec::new()),
            copying: RefCell::new(None),
            copy_wants: signal(Vec::new()),
        })
    }

    fn text_of(&self, page: usize) -> Option<Arc<PageText>> {
        self.texts.with_untracked(|t| t.get(&page).cloned())
    }

    /// Asks for what text is wanted whenever that may have changed.
    pub fn watch_needs(self: &Rc<Self>) {
        let this = self.clone();
        effect(move || {
            let mut needed = Vec::new();
            needed.extend(this.pointer_page.get());
            if !this.table.with(|t| t.is_empty()) {
                let v = (this.view)();
                let visible = this.table.with(|t| t.visible(v.y, v.height));
                let selection = this.viewer.selection.get();
                let hits_in = this.viewer.hits.with(|hits| {
                    visible
                        .clone()
                        .filter(|&page| !hits.on(page).is_empty())
                        .collect::<Vec<_>>()
                });
                needed.extend(hits_in);
                if let Some((a, b)) = selection {
                    needed.extend(visible.clone().filter(|&p| p >= a.page && p <= b.page));
                }
            }
            needed.extend(this.viewer.current_hit.get().map(|hit| hit.0));
            let seen: HashSet<usize> = this.texts.with(|t| t.keys().copied().collect());
            let mut now: Vec<usize> = needed
                .iter()
                .copied()
                .filter(|p| !seen.contains(p))
                .collect();
            now.extend(this.copy_wants.get());
            *this.needed.borrow_mut() = needed;
            let later = this.search_wants.get();
            if let Some(engine) = &*this.engine.borrow() {
                engine.want_text(now, later);
            }
        });
    }

    /// The text of a page came from a worker.
    pub fn text_arrived(self: &Rc<Self>, page: usize, text: Arc<PageText>) {
        if let Some(engine) = &*self.engine.borrow() {
            engine.text_received(page);
        }
        self.folded.borrow_mut().insert(page, text.folded.clone());
        self.copy_arrived(page, &text);
        if self.needed.borrow().contains(&page) {
            self.keep_text(page, text);
            self.resolve_press();
            self.extend_selection();
        }
        self.pump_search();
    }

    fn keep_text(&self, page: usize, text: Arc<PageText>) {
        let needed = self.needed.borrow();
        let mut arrived = self.arrived.borrow_mut();
        self.texts.update(|texts| {
            if texts.insert(page, text).is_none() {
                arrived.push_back(page);
            }
            let mut left = arrived.len();
            while texts.len() > KEPT_PAGES && left > 0 {
                left -= 1;
                let Some(old) = arrived.pop_front() else {
                    break;
                };
                if needed.contains(&old) || old == page {
                    arrived.push_back(old);
                } else {
                    texts.remove(&old);
                }
            }
        });
    }

    /// A point of the box as a page and a point on it, for the page nearest to it.
    fn on_page(&self, local: PointF) -> Option<(usize, f32, f32)> {
        let (v, table) = untrack(|| ((self.view)(), self.table.get()));
        let (x, y) = (v.x as f32 + local.x, v.y as f32 + local.y);
        if table.is_empty() {
            return None;
        }
        let page = table.page_near(y as i32);
        let (width, height) = table.size(page);
        let (pw, ph) = self
            .pages
            .with_untracked(|p| *p.get(page).unwrap_or(&(1.0, 1.0)));
        let left = table.left(page) as f32;
        let top = table.top(page) as f32;
        Some((
            page,
            (x - left) * pw / width.max(1) as f32,
            (y - top) * ph / height.max(1) as f32,
        ))
    }

    pub fn pointer_down(self: &Rc<Self>, e: &PointerEvent) {
        if e.button != Some(PointerButton::Primary) || self.scroller.is_over_scrollbar(e.position) {
            return;
        }
        clear_text_selection();
        self.press.set(Some(Press {
            position: e.position,
            local: e.local,
            clicks: e.click_count,
            shift: e.modifiers.shift,
            moved: false,
            resolved: false,
        }));
        self.selecting.set(None);
        if let Some((page, ..)) = self.on_page(e.local) {
            self.pointer_page.set_if_changed(Some(page));
        }
        self.resolve_press();
    }

    /// Starts selecting from what was pressed, once the page has been read.
    fn resolve_press(&self) {
        let Some(mut press) = self.press.get().filter(|p| !p.resolved) else {
            return;
        };
        let Some((page, x, y)) = self.on_page(press.local) else {
            return;
        };
        let Some(text) = self.text_of(page) else {
            return;
        };
        press.resolved = true;
        self.press.set(Some(press));
        let caret = text.caret_at(x, y);
        let caret = caret.filter(|c| press.clicks > 1 || press.shift || c.distance <= NEAR_TEXT);
        let Some(caret) = caret else {
            self.viewer.selection.set_if_changed(None);
            self.anchor.set(None);
            return;
        };
        let unit = match press.clicks {
            1 => Unit::Glyph,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let (from, to) = range_at(&text, unit, caret);
        let (from, to) = (Place { page, glyph: from }, Place { page, glyph: to });
        let base = match self.anchor.get().filter(|_| press.shift) {
            Some(anchor) => (anchor, anchor),
            None => {
                self.anchor.set(Some(from));
                (from, to)
            }
        };
        self.selecting.set(Some((base, unit)));
        self.extend_selection();
    }

    /// Makes the selection reach to where the pointer is.
    pub fn extend_selection(&self) {
        let (Some((base, unit)), Some(press)) = (self.selecting.get(), self.press.get()) else {
            return;
        };
        let Some((page, x, y)) = self.on_page(press.local) else {
            return;
        };
        self.pointer_page.set_if_changed(Some(page));
        let Some(text) = self.text_of(page) else {
            return;
        };
        let Some(caret) = text.caret_at(x, y) else {
            return;
        };
        let (from, to) = range_at(&text, unit, caret);
        let from = base.0.min(Place { page, glyph: from });
        let to = base.1.max(Place { page, glyph: to });
        self.viewer
            .selection
            .set_if_changed((from != to).then_some((from, to)));
    }

    pub fn pointer_moved(self: &Rc<Self>, e: &PointerEvent) {
        if let Some(mut press) = self.press.get() {
            let slop = window_scale().pxf(CLICK_SLOP);
            press.moved |= press.position.distance(e.position) > slop;
            press.local = e.local;
            self.press.set(Some(press));
            self.extend_selection();
            self.keep_scrolling();
            return;
        }
        let Some((page, x, y)) = self.on_page(e.local) else {
            return;
        };
        self.pointer_page.set_if_changed(Some(page));
        let cursor = match self.text_of(page) {
            Some(text) if text.links.iter().any(|l| l.contains(x, y)) => CursorIcon::Pointer,
            Some(text) if text.caret_at(x, y).is_some_and(|c| c.distance <= NEAR_TEXT) => {
                CursorIcon::Text
            }
            _ => CursorIcon::Default,
        };
        self.cursor.set_if_changed(cursor);
    }

    pub fn pointer_left(&self) {
        if self.press.get().is_none() {
            self.pointer_page.set_if_changed(None);
        }
    }

    pub fn pointer_up(self: &Rc<Self>, e: &PointerEvent) {
        let Some(press) = self.press.take() else {
            return;
        };
        self.selecting.set(None);
        if !press.moved && press.clicks == 1 && !press.shift {
            // A click selects nothing, even if it was on text.
            self.viewer.selection.set_if_changed(None);
            self.follow_link(e.local);
            return;
        }
        if self.viewer.selection.get_untracked().is_some() {
            self.copy(ClipboardKind::Primary);
        }
    }

    fn follow_link(&self, local: PointF) {
        let Some((page, x, y)) = self.on_page(local) else {
            return;
        };
        let Some(text) = self.text_of(page) else {
            return;
        };
        match text
            .links
            .iter()
            .find(|l| l.contains(x, y))
            .map(|l| &l.target)
        {
            Some(LinkTarget::Place(target)) => self.viewer.go_to(*target),
            Some(LinkTarget::Url(url)) => {
                if let Some(f) = &self.on_link {
                    f(url);
                }
            }
            None => {}
        }
    }

    /// Scrolls while selecting with the pointer outside of the box.
    fn keep_scrolling(self: &Rc<Self>) {
        if self.scrolling.get() || self.scroll_by_pointer().is_none() {
            return;
        }
        self.scrolling.set(true);
        let this = self.clone();
        request_frame(move |_| this.scroll_tick());
    }

    fn scroll_tick(self: &Rc<Self>) {
        self.scrolling.set(false);
        let Some((dx, dy)) = self.scroll_by_pointer() else {
            return;
        };
        let scale = window_scale();
        let at = self.scroller.scroll_offset();
        self.scroller.scroll_to(
            at.x + scale.to_logical(dx * AUTOSCROLL),
            at.y + scale.to_logical(dy * AUTOSCROLL),
        );
        self.extend_selection();
        self.keep_scrolling();
    }

    /// How far outside of the box the pointer is when selecting.
    fn scroll_by_pointer(&self) -> Option<(f32, f32)> {
        self.selecting.get()?;
        let press = self.press.get()?;
        let v = untrack(|| (self.view)());
        let outside = |at: f32, size: i32| {
            if at < 0.0 {
                at
            } else {
                (at - size as f32).max(0.0)
            }
        };
        let (dx, dy) = (
            outside(press.local.x, v.width),
            outside(press.local.y, v.height),
        );
        (dx != 0.0 || dy != 0.0).then_some((dx, dy))
    }

    /// Puts what is selected on a clipboard, as soon as all of it has been read.
    pub fn copy(&self, kind: ClipboardKind) {
        let Some(selection) = self.viewer.selection.get_untracked() else {
            return;
        };
        let (a, b) = selection;
        let mut copying = Copying {
            kind,
            selection,
            parts: vec![None; b.page - a.page + 1],
            next: 0,
        };
        for (i, part) in copying.parts.iter_mut().enumerate() {
            if let Some(text) = self.text_of(a.page + i) {
                let (from, to) = selected_on(selection, a.page + i, text.len());
                *part = Some(text.copied(from, to).to_owned());
            }
        }
        self.copy_or_wait(copying);
    }

    fn copy_or_wait(&self, mut copying: Copying) {
        while copying.parts.get(copying.next).is_some_and(Option::is_some) {
            copying.next += 1;
        }
        if copying.next == copying.parts.len() {
            let parts: Vec<String> = copying.parts.into_iter().flatten().collect();
            let text = parts
                .iter()
                .filter(|p| !p.is_empty())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            set_clipboard(copying.kind, text);
            *self.copying.borrow_mut() = None;
            self.copy_wants.set_if_changed(Vec::new());
            return;
        }
        let start = copying.selection.0.page;
        let wants: Vec<usize> = (copying.next..copying.parts.len())
            .filter(|&i| copying.parts[i].is_none())
            .take(READ_AHEAD)
            .map(|i| start + i)
            .collect();
        *self.copying.borrow_mut() = Some(copying);
        self.copy_wants.set_if_changed(wants);
    }

    fn copy_arrived(&self, page: usize, text: &PageText) {
        let Some(mut copying) = self.copying.borrow_mut().take() else {
            return;
        };
        let (a, b) = copying.selection;
        if page >= a.page && page <= b.page {
            let (from, to) = selected_on(copying.selection, page, text.len());
            copying.parts[page - a.page] = Some(text.copied(from, to).to_owned());
        }
        self.copy_or_wait(copying);
    }

    /// Starts searching for what the viewer says, from the page that shows.
    pub fn start_search(self: &Rc<Self>, query: &str) {
        let mut folded = String::new();
        text::fold(query, &mut folded);
        self.viewer.hits.set(Hits::default());
        self.viewer.current_hit.set_if_changed(None);
        if folded.is_empty() {
            *self.search.borrow_mut() = None;
            self.search_wants.set_if_changed(Vec::new());
            self.viewer.search_done.set_if_changed(true);
            return;
        }
        self.viewer.search_done.set_if_changed(false);
        *self.search.borrow_mut() = Some(Search {
            query: folded,
            start: self.viewer.page.get_untracked(),
            done: 0,
            shown_first: false,
            pump_is_due: false,
        });
        self.pump_search();
    }

    /// Goes through the text that is there, and asks for the next pages.
    fn pump_search(self: &Rc<Self>) {
        let count = self.viewer.count.get_untracked();
        let mut search = self.search.borrow_mut();
        let Some(s) = search.as_mut() else {
            return;
        };
        s.pump_is_due = false;
        let page_of = |s: &Search, done: usize| (s.start + done) % count.max(1);
        let started = Instant::now();
        let mut found = Vec::new();
        {
            let folded = self.folded.borrow();
            while s.done < count {
                let page = page_of(s, s.done);
                let Some(text) = folded.get(&page) else {
                    break;
                };
                found.push((page, text::find_all(text, &s.query)));
                s.done += 1;
                if started.elapsed() >= SEARCH_SLICE {
                    s.pump_is_due = true;
                    break;
                }
            }
        }
        if found.iter().any(|f| !f.1.is_empty()) {
            self.viewer.hits.update(|hits| {
                for (page, found) in found.drain(..) {
                    hits.insert(page, found);
                }
            });
            if !s.shown_first {
                s.shown_first = true;
                let first = self.viewer.hits.with_untracked(|h| h.first_from(s.start));
                self.viewer.current_hit.set_if_changed(first);
            }
        }
        if s.done >= count {
            self.search_wants.set_if_changed(Vec::new());
            self.viewer.search_done.set_if_changed(true);
            return;
        }
        let folded = self.folded.borrow();
        let wants: Vec<usize> = (s.done..count)
            .map(|done| page_of(s, done))
            .filter(|page| !folded.contains_key(page))
            .take(READ_AHEAD)
            .collect();
        self.search_wants.set_if_changed(wants);
        if s.pump_is_due {
            let this = self.clone();
            request_frame(move |_| this.pump_search());
        }
    }

    /// Scrolls to what was found whenever that changes, once it is known where it is.
    pub fn watch_current_hit(self: &Rc<Self>) {
        let this = self.clone();
        let shown = Cell::new((None::<HitPlace>, 0));
        effect(move || {
            let current = (this.viewer.current_hit.get(), this.viewer.hit_asked.get());
            if current == shown.get() {
                return;
            }
            let Some(hit) = current.0 else {
                shown.set(current);
                return;
            };
            let Some(text) = this.texts.with(|t| t.get(&hit.0).cloned()) else {
                return;
            };
            let table = this.table.get();
            let found = this
                .viewer
                .hits
                .with_untracked(|h| h.on(hit.0).get(hit.1).copied());
            let Some(found) = found else {
                return;
            };
            shown.set(current);
            let (from, to) = text.found_glyphs(found);
            let boxes = text.boxes(from, to);
            let Some(area) = boxes.first() else {
                return;
            };
            let at = this.to_pixels(&table, hit.0, area);
            this.bring_into_view(&table, hit.0, at);
        });
    }

    /// A box in the points of a page in its pixels.
    fn to_pixels(&self, table: &PageTable, page: usize, area: &[f32; 4]) -> RectI {
        let (width, height) = table.size(page);
        let (pw, ph) = self
            .pages
            .with_untracked(|p| *p.get(page).unwrap_or(&(1.0, 1.0)));
        let (sx, sy) = (width as f32 / pw.max(1.0), height as f32 / ph.max(1.0));
        let (x0, y0) = ((area[0] * sx).floor() as i32, (area[1] * sy).floor() as i32);
        let (x1, y1) = ((area[2] * sx).ceil() as i32, (area[3] * sy).ceil() as i32);
        RectI::new(x0, y0, x1 - x0, y1 - y0)
    }

    fn bring_into_view(&self, table: &PageTable, page: usize, area: RectI) {
        let v = untrack(|| (self.view)());
        let (x, y) = (area.x + table.left(page), area.y + table.top(page));
        let inside_y = y >= v.y && y + area.height <= v.y + v.height;
        let inside_x = x >= v.x && x + area.width <= v.x + v.width;
        if inside_x && inside_y {
            return;
        }
        let to_x = if inside_x {
            v.x
        } else {
            (x + area.width / 2 - v.width / 2).max(0)
        };
        let to_y = if inside_y {
            v.y
        } else {
            (y + area.height / 2 - v.height / 2).max(0)
        };
        let scale = untrack(window_scale);
        self.scroller
            .scroll_to(scale.to_logical(to_x as f32), scale.to_logical(to_y as f32));
    }

    /// What shows on top of a page: what was found and what is selected.
    pub fn boxes(&self, page: usize) -> Vec<PageBox> {
        let table = self.table.get();
        if page >= table.len() {
            return Vec::new();
        }
        let Some(text) = self.texts.with(|t| t.get(&page).cloned()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut add = |areas: Vec<[f32; 4]>, kind: BoxKind| {
            for area in &areas {
                let r = self.to_pixels(&table, page, area);
                let boxed = PageBox {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                    kind,
                };
                if !out.contains(&boxed) {
                    out.push(boxed);
                }
            }
        };
        let current = self.viewer.current_hit.get();
        self.viewer.hits.with(|hits| {
            for (i, &found) in hits.on(page).iter().enumerate() {
                let (from, to) = text.found_glyphs(found);
                let kind = if current == Some((page, i)) {
                    BoxKind::Current
                } else {
                    BoxKind::Found
                };
                add(text.boxes(from, to), kind);
            }
        });
        if let Some(selection) = self.viewer.selection.get() {
            let (from, to) = selected_on(selection, page, text.len());
            add(text.boxes(from, to), BoxKind::Selected);
        }
        out
    }
}

/// What selecting takes at a caret, as a range of glyphs.
fn range_at(text: &PageText, unit: Unit, caret: text::Caret) -> (usize, usize) {
    match unit {
        Unit::Glyph => (caret.at, caret.at),
        Unit::Word => text.word_at(caret.glyph),
        Unit::Line => text.line_at(caret.glyph),
    }
}

/// Where a place in a document is, for scrolling to it.
pub(crate) fn target_position(
    table: &PageTable,
    pages: &[(f32, f32)],
    target: PdfTarget,
) -> Option<i32> {
    if table.is_empty() {
        return None;
    }
    let page = target.page.min(table.len() - 1);
    let down = target.top.map_or(0.0, |top| {
        let tall = pages.get(page).map_or(1.0, |p| p.1).max(1.0);
        top.clamp(0.0, tall) * table.size(page).1 as f32 / tall
    });
    Some(table.top(page) + down as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_go_around() {
        let mut hits = Hits::default();
        hits.insert(5, vec![(0, 1), (2, 3)]);
        hits.insert(2, vec![(0, 1)]);
        hits.insert(9, Vec::new());
        assert_eq!(hits.count, 3);
        assert_eq!(hits.first_from(0), Some((2, 0)));
        assert_eq!(hits.first_from(3), Some((5, 0)));
        assert_eq!(hits.first_from(6), Some((2, 0)));
        assert_eq!(hits.after((2, 0)), Some((5, 0)));
        assert_eq!(hits.after((5, 0)), Some((5, 1)));
        assert_eq!(hits.after((5, 1)), Some((2, 0)));
        assert_eq!(hits.before((5, 0)), Some((2, 0)));
        assert_eq!(hits.before((2, 0)), Some((5, 1)));
        assert_eq!(hits.index_of((5, 1)), 2);
        assert!(Hits::default().first_from(0).is_none());
        assert!(Hits::default().before((0, 0)).is_none());
    }

    #[test]
    fn a_selection_takes_all_of_the_pages_in_between() {
        let selection = (Place { page: 1, glyph: 5 }, Place { page: 3, glyph: 2 });
        assert_eq!(selected_on(selection, 0, 10), (0, 0));
        assert_eq!(selected_on(selection, 1, 10), (5, 10));
        assert_eq!(selected_on(selection, 2, 10), (0, 10));
        assert_eq!(selected_on(selection, 3, 10), (0, 2));
        assert_eq!(selected_on(selection, 4, 10), (0, 0));
    }
}
