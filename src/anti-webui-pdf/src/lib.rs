//! A fast PDF viewer: `pdf_view` shows a document in a box that scrolls and zooms. Pages are drawn
//! on other threads with `hayro`, and only those that show and come next. See
//! `docs/design/26-pdf.md`.

mod appearance;
mod document;
mod editor;
mod edits;
mod embed;
mod engine;
mod fonts;
mod links;
mod marks;
mod plan;
mod reading;
mod slides;
mod text;
mod thumbnails;
mod writer;

use anti_webui_core::channel::ui_channel;
use anti_webui_core::event::{Key, KeyboardEvent, NamedKey, PinchPhase, ScrollEvent};
use anti_webui_core::geometry::{PointF, RectI};
use anti_webui_core::images::Bitmap;
use anti_webui_core::prelude::*;
use editor::Editor;
pub use editor::{PdfFieldChange, PdfTool};
use edits::Edits;
use engine::{Engine, Event};
pub use links::{PdfOutlineItem, PdfTarget};
use marks::{Annot, Marks, Shape};
pub use marks::{
    PdfAnnotationId, PdfAnnotationKind, PdfEditing, PdfField, PdfFieldKind, PdfFieldValue,
};
use plan::{Direction, Have, Held, Job, PageTable, Turn};
use reading::{BoxKind, HitPlace, Hits, Place, Reader, Selection};
use slides::Slides;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
pub use thumbnails::{PdfThumbnails, pdf_thumbnails};

/// How many physical pixels a point takes at 100% and a scale of 1, as in CSS.
const PX_PER_PT: f64 = 96.0 / 72.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 16.0;
/// How many times off a picture can be while pinching before it gets drawn again:
/// `kMaxScaleRatioDuringPinch` in Chromium.
const PINCH_RATIO: f64 = 2.0;
/// What shows on top of text that is selected, was found, and was found and is being looked at.
const SELECTED: u32 = 0x0054b466;
const FOUND: u32 = 0xffd60066;
const CURRENT: u32 = 0xff8a0088;
/// What shows on top of a field that can be filled in, as in PDF.js.
const FIELD_TINT: u32 = 0x0036ff21;
/// The colors of what gets added unless it is told otherwise.
const HIGHLIGHT: u32 = 0xffd900;
const LINE: u32 = 0xe02020;
const TEXT: u32 = 0x000000;
/// How long a page that isn't drawn yet leaves the one before it showing when presenting.
const SLIDE_WAITS: Duration = Duration::from_millis(300);

/// Why a document can't be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// It takes a password: see [`PdfViewer::unlock`].
    NeedsPassword,
    /// That wasn't the password.
    WrongPassword,
    /// It can't be read.
    Source(String),
    /// It isn't a PDF, or it is too broken to read.
    Invalid,
    /// It is a kind of PDF this can't read, like an unknown kind of encryption.
    Unsupported(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::NeedsPassword => f.write_str("this document needs a password"),
            PdfError::WrongPassword => f.write_str("that isn't the password"),
            PdfError::Source(e) => write!(f, "can't read the document: {e}"),
            PdfError::Invalid => f.write_str("this isn't a PDF, or it is too broken to read"),
            PdfError::Unsupported(e) => write!(f, "can't read this kind of PDF: {e}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// How big a document shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PdfZoom {
    /// 1.0 is 100%: a point is `96 / 72` logical pixels, as in CSS.
    Percent(f32),
    /// The widest page takes the width of the box.
    FitWidth,
    /// The first page fits in the box.
    FitPage,
}

/// How far along a document is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfState {
    Loading,
    Ready,
    Failed(PdfError),
}

/// What a [`pdf_view`] shows.
#[derive(Clone, Debug)]
pub enum PdfSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
    #[cfg(test)]
    Fake(tests::FakeSource),
}

impl From<&str> for PdfSource {
    fn from(path: &str) -> Self {
        Self::Path(path.into())
    }
}

impl From<String> for PdfSource {
    fn from(path: String) -> Self {
        Self::Path(path.into())
    }
}

impl From<&Path> for PdfSource {
    fn from(path: &Path) -> Self {
        Self::Path(path.into())
    }
}

impl From<PathBuf> for PdfSource {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<Vec<u8>> for PdfSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes.into())
    }
}

impl From<Arc<[u8]>> for PdfSource {
    fn from(bytes: Arc<[u8]>) -> Self {
        Self::Bytes(bytes)
    }
}

/// How saving went: where it went, or why not.
pub type SaveResult = Result<PathBuf, String>;

/// The thumbnails there are, by page.
type Thumbnails = HashMap<usize, (Job, Option<Arc<Bitmap>>)>;

/// Controls a [`pdf_view`] and says how far it has come. It can be copied into closures like a
/// signal.
#[derive(Clone, Copy)]
pub struct PdfViewer {
    node: NodeRef,
    /// How big each page is, in points, as the document has it.
    pages: Signal<Rc<Vec<(f32, f32)>>>,
    turn: Signal<Turn>,
    /// What [`pdf_thumbnails`] has, and what it wants next. A thumbnail that couldn't be drawn has
    /// no picture.
    thumbnails: Signal<Thumbnails>,
    thumbnails_wanted: Signal<Vec<Job>>,
    /// How many are showing, as there is nothing to keep thumbnails for without one.
    thumbnail_lists: Signal<usize>,
    zoom: Signal<PdfZoom>,
    percent: Signal<f32>,
    page: Signal<usize>,
    count: Signal<usize>,
    state: Signal<PdfState>,
    go_to: Signal<Option<PdfTarget>>,
    password: Signal<Option<String>>,
    outline: Signal<Rc<Vec<PdfOutlineItem>>>,
    selection: Signal<Option<Selection>>,
    /// How many times it was asked to copy.
    copies: Signal<u64>,
    query: Signal<String>,
    hits: Signal<Hits>,
    current_hit: Signal<Option<HitPlace>>,
    /// How many times it was asked to go to one, which shows it again even if it is the same one.
    hit_asked: Signal<u64>,
    search_done: Signal<bool>,
    /// What the document has for filling in and marking up, once that has been read.
    marks: Signal<Arc<Marks>>,
    /// All that was changed, and what it was before and after each change.
    edits: Signal<Arc<Edits>>,
    undone: Signal<Vec<Arc<Edits>>>,
    redone: Signal<Vec<Arc<Edits>>>,
    /// What is in the file that was opened or saved last.
    saved: Signal<Arc<Edits>>,
    /// Several changes that count as one, with what it was before them.
    next_annot: Signal<u64>,
    tool: Signal<PdfTool>,
    tool_color: Signal<Option<u32>>,
    ink_width: Signal<f32>,
    text_size: Signal<f32>,
    selected: Signal<Option<PdfAnnotationId>>,
    /// How many times it was asked to mark what is selected, and with what.
    marking: Signal<(u64, PdfAnnotationKind)>,
    save_to: Signal<Option<PathBuf>>,
    save_result: Signal<Option<SaveResult>>,
    edit_error: Signal<Option<String>>,
    /// Asked to present, which starts when it is on all of the screen.
    present_asked: Signal<bool>,
    presenting: Signal<bool>,
    /// The page that is wanted when presenting.
    slide: Signal<usize>,
}

/// Makes what controls a [`pdf_view`], to hand it with [`PdfView::viewer`].
pub fn pdf_viewer() -> PdfViewer {
    PdfViewer {
        node: node_ref(),
        pages: signal(Rc::new(Vec::new())),
        turn: signal(Turn::default()),
        thumbnails: signal(HashMap::new()),
        thumbnails_wanted: signal(Vec::new()),
        thumbnail_lists: signal(0),
        zoom: signal(PdfZoom::FitWidth),
        percent: signal(1.0),
        page: signal(0),
        count: signal(0),
        state: signal(PdfState::Loading),
        go_to: signal(None),
        password: signal(None),
        outline: signal(Rc::new(Vec::new())),
        selection: signal(None),
        copies: signal(0),
        query: signal(String::new()),
        hits: signal(Hits::default()),
        current_hit: signal(None),
        hit_asked: signal(0),
        search_done: signal(true),
        marks: signal(Arc::default()),
        edits: signal(Arc::default()),
        undone: signal(Vec::new()),
        redone: signal(Vec::new()),
        saved: signal(Arc::default()),
        next_annot: signal(0),
        tool: signal(PdfTool::Select),
        tool_color: signal(None),
        ink_width: signal(2.0),
        text_size: signal(14.0),
        selected: signal(None),
        marking: signal((0, PdfAnnotationKind::Highlight)),
        save_to: signal(None),
        save_result: signal(None),
        edit_error: signal(None),
        present_asked: signal(false),
        presenting: signal(false),
        slide: signal(0),
    }
}

impl PdfViewer {
    pub fn zoom(&self) -> PdfZoom {
        self.zoom.get()
    }

    pub fn set_zoom(&self, zoom: PdfZoom) {
        self.zoom.set(zoom);
    }

    /// Makes it this many times bigger than it is now, keeping what is in the middle there.
    pub fn zoom_by(&self, factor: f32) {
        let now = self.percent.get_untracked();
        self.zoom
            .set(PdfZoom::Percent((now * factor).clamp(MIN_ZOOM, MAX_ZOOM)));
    }

    /// How far the pages are turned clockwise, in degrees: 0, 90, 180 or 270. Only the view turns,
    /// and the document stays as it is.
    pub fn rotation(&self) -> i32 {
        self.turn.get().0 as i32 * 90
    }

    /// Turns the pages to this many degrees clockwise, down to a quarter turn.
    pub fn set_rotation(&self, degrees: i32) {
        self.turn.set_if_changed(Turn::degrees(degrees));
    }

    /// Turns the pages a quarter turn clockwise, keeping what is in the middle there.
    pub fn rotate_clockwise(&self) {
        self.turn.update(|turn| *turn = turn.by(1));
    }

    /// Turns the pages a quarter turn counterclockwise, keeping what is in the middle there.
    pub fn rotate_counterclockwise(&self) {
        self.turn.update(|turn| *turn = turn.by(-1));
    }

    /// How big it shows now, whatever the zoom was: 1.0 is 100%.
    pub fn percent(&self) -> f32 {
        self.percent.get()
    }

    /// The page in the middle of the box, from 0.
    pub fn page(&self) -> usize {
        self.page.get()
    }

    /// How many pages there are, once it has been opened.
    pub fn page_count(&self) -> usize {
        self.count.get()
    }

    /// Scrolls to the top of a page, from 0, as soon as there is one.
    pub fn go_to_page(&self, page: usize) {
        self.go_to(PdfTarget { page, top: None });
    }

    /// Scrolls to a place in the document, as soon as there is one.
    pub fn go_to(&self, target: PdfTarget) {
        self.go_to.set(Some(target));
    }

    /// The outline of the document, which is what it has for a table of contents. Most documents
    /// don't have one, and it comes a little after the pages.
    pub fn outline(&self) -> Rc<Vec<PdfOutlineItem>> {
        self.outline.get()
    }

    /// Whether any text is selected.
    pub fn has_selection(&self) -> bool {
        self.selection.with(Option::is_some)
    }

    /// Selects all of the text of the document.
    pub fn select_all(&self) {
        let count = self.count.get_untracked();
        if count > 0 {
            self.selection.set(Some((
                Place { page: 0, glyph: 0 },
                Place {
                    page: count - 1,
                    glyph: usize::MAX,
                },
            )));
        }
    }

    pub fn clear_selection(&self) {
        self.selection.set_if_changed(None);
    }

    /// Puts what is selected on the clipboard, which takes a moment if not all of it has been read.
    pub fn copy(&self) {
        self.copies.update(|n| *n += 1);
    }

    /// Looks for text in all of the document, starting at the page that shows, and scrolls to the
    /// first it finds. Case, spaces, hyphens and the wide forms of letters don't matter, so that it
    /// also finds what a line breaks up. Nothing to look for stops it.
    pub fn search(&self, query: impl Into<String>) {
        self.query.set_if_changed(query.into());
    }

    /// How many were found so far. It goes up while [`Self::search_done`] is false.
    pub fn search_count(&self) -> usize {
        self.hits.with(|hits| hits.count)
    }

    /// Which of those found is being looked at, from 0.
    pub fn search_index(&self) -> Option<usize> {
        let current = self.current_hit.get()?;
        Some(self.hits.with(|hits| hits.index_of(current)))
    }

    /// Whether it has been through all of the document.
    pub fn search_done(&self) -> bool {
        self.search_done.get()
    }

    /// Goes to the next one found, starting over after the last one.
    pub fn find_next(&self) {
        let current = self.current_hit.get_untracked();
        let page = self.page.get_untracked();
        let next = self.hits.with_untracked(|hits| match current {
            Some(hit) => hits.after(hit),
            None => hits.first_from(page),
        });
        self.current_hit.set_if_changed(next);
        self.hit_asked.update(|n| *n += 1);
    }

    /// Goes to the one found before this one, starting over before the first one.
    pub fn find_previous(&self) {
        let current = self.current_hit.get_untracked();
        let page = self.page.get_untracked();
        let next = self.hits.with_untracked(|hits| match current {
            Some(hit) => hits.before(hit),
            None => hits.first_from(page),
        });
        self.current_hit.set_if_changed(next);
        self.hit_asked.update(|n| *n += 1);
    }

    pub fn state(&self) -> PdfState {
        self.state.get()
    }

    /// Gives it the keyboard: PageUp, PageDown, Home, End and the arrows move, Ctrl with `+`, `-` and
    /// `0` zooms, Ctrl with `]` and `[` turns, Ctrl with `A` and `C` selects and copies, and Escape
    /// stops selecting.
    pub fn focus(&self) {
        self.node.focus();
    }

    /// Shows the document one page at a time on all of the screen, starting with the page that shows,
    /// wherever the box is. The keys, a click and the wheel go through the pages, and Escape or
    /// [`Self::stop_presenting`] goes back to how it was. It takes a moment:
    /// [`Self::is_presenting`] says when it has started.
    pub fn present(&self) {
        self.present_asked.set(true);
        self.node.request_fullscreen();
    }

    pub fn stop_presenting(&self) {
        // Asked for and not there yet counts too, as it would start later otherwise.
        let asked = self.present_asked.get_untracked();
        self.present_asked.set(false);
        if asked || self.node.is_fullscreen() {
            exit_fullscreen();
        }
    }

    pub fn is_presenting(&self) -> bool {
        self.presenting.get()
    }

    /// Opens it again with a password, after [`PdfError::NeedsPassword`].
    pub fn unlock(&self, password: impl Into<String>) {
        self.password.set(Some(password.into()));
    }

    /// Whether the document can be edited, and otherwise why not. It shows either way.
    pub fn editing(&self) -> PdfEditing {
        self.marks.with(|m| m.editing)
    }

    /// Whether the document has signatures, which Acrobat says were followed by changes once it has
    /// been edited.
    pub fn is_signed(&self) -> bool {
        self.marks.with(|m| m.signed)
    }

    /// The fields of its forms, in the order of the pages.
    pub fn fields(&self) -> Vec<PdfField> {
        self.marks
            .with(|m| m.fields.iter().map(|f| f.info.clone()).collect())
    }

    /// What is in a field, by its whole name.
    pub fn field_value(&self, name: &str) -> Option<PdfFieldValue> {
        let _ = self.edits.get();
        let at = self.field_named(name)?;
        Some(self.field_value_at(at))
    }

    /// Fills in a field by its whole name. It says whether there is such a field. Being read only
    /// doesn't stop this, as it is for what a person does.
    pub fn set_field(&self, name: &str, value: PdfFieldValue) -> bool {
        let Some(at) = self.field_named(name) else {
            return false;
        };
        self.set_field_at(at, value);
        true
    }

    /// Puts all of the fields back to what the document says they start with.
    pub fn reset_fields(&self) {
        let marks = self.marks.get_untracked();
        self.batch_edits(|viewer| {
            for (at, field) in marks.fields.iter().enumerate() {
                viewer.set_field_at(at, field.default.clone());
            }
        });
    }

    fn field_named(&self, name: &str) -> Option<usize> {
        self.marks
            .with_untracked(|m| m.fields.iter().position(|f| f.info.name == name))
    }

    fn field_value_at(&self, at: usize) -> PdfFieldValue {
        let edited = self.edits.with_untracked(|e| e.fields.get(&at).cloned());
        edited.unwrap_or_else(|| self.marks.with_untracked(|m| m.fields[at].value.clone()))
    }

    /// Says what is in a field. It says whether that changed anything.
    fn set_field_at(&self, at: usize, value: PdfFieldValue) -> bool {
        if self.field_value_at(at) == value {
            return false;
        }
        let same = self.marks.with_untracked(|m| m.fields[at].value == value);
        self.change_edits(|edits| {
            if same {
                edits.fields.remove(&at);
            } else {
                edits.fields.insert(at, value);
            }
        });
        true
    }

    /// Changes the list of edits as one thing to undo.
    fn change_edits(&self, f: impl FnOnce(&mut Edits)) {
        let now = self.edits.get_untracked();
        let mut next = (*now).clone();
        f(&mut next);
        if next == *now {
            return;
        }
        self.undone.update(|all| all.push(now));
        self.redone.update(Vec::clear);
        self.edits.set(Arc::new(next));
    }

    /// Makes all that a function changes one thing to undo.
    fn batch_edits(&self, f: impl FnOnce(&Self)) {
        let before = self.edits.get_untracked();
        let depth = self.undone.with_untracked(Vec::len);
        f(self);
        if !Arc::ptr_eq(&before, &self.edits.get_untracked()) {
            self.undone.update(|all| {
                all.truncate(depth);
                all.push(before);
            });
        }
    }

    /// Goes back on what was changed last.
    pub fn undo(&self) {
        let Some(last) = self.undone.with_untracked(|all| all.last().cloned()) else {
            return;
        };
        self.undone.update(|all| {
            all.pop();
        });
        let now = self.edits.get_untracked();
        self.redone.update(|all| all.push(now));
        self.selected.set_if_changed(None);
        self.edits.set(last);
    }

    /// Goes forward again after [`Self::undo`].
    pub fn redo(&self) {
        let Some(last) = self.redone.with_untracked(|all| all.last().cloned()) else {
            return;
        };
        self.redone.update(|all| {
            all.pop();
        });
        let now = self.edits.get_untracked();
        self.undone.update(|all| all.push(now));
        self.selected.set_if_changed(None);
        self.edits.set(last);
    }

    pub fn can_undo(&self) -> bool {
        self.undone.with(|all| !all.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.redone.with(|all| !all.is_empty())
    }

    /// Whether anything was changed since it was opened or saved.
    pub fn is_modified(&self) -> bool {
        self.edits.get() != self.saved.get()
    }

    /// Writes the document as it is now to a file, on another thread: all of what it had and what
    /// changed after that. Nothing happens to what is there if it doesn't work out.
    /// [`Self::save_result`] says how it went.
    pub fn save(&self, path: impl Into<PathBuf>) {
        self.save_result.set(None);
        self.save_to.set(Some(path.into()));
    }

    /// How the last [`Self::save`] went, once it is done.
    pub fn save_result(&self) -> Option<SaveResult> {
        self.save_result.get()
    }

    /// Why the last change couldn't be made, if it couldn't.
    pub fn edit_error(&self) -> Option<String> {
        self.edit_error.get()
    }

    pub fn tool(&self) -> PdfTool {
        self.tool.get()
    }

    /// Says what pressing and dragging on a page does.
    pub fn set_tool(&self, tool: PdfTool) {
        self.tool.set_if_changed(tool);
    }

    /// The color of what gets added from here on, as `0xrrggbb`. What each kind has otherwise is
    /// yellow for what marks text and notes, red for lines and black for text.
    pub fn set_tool_color(&self, color: u32) {
        self.tool_color.set(Some(color));
    }

    /// How wide the lines of [`PdfTool::Ink`] are, in points.
    pub fn set_ink_width(&self, width: f32) {
        self.ink_width.set(width);
    }

    /// How big the text of [`PdfTool::FreeText`] is, in points.
    pub fn set_text_size(&self, size: f32) {
        self.text_size.set(size);
    }

    /// Marks what is selected in yellow, or the color it was told. It takes a moment if not all of
    /// it has been read.
    pub fn highlight_selection(&self) {
        self.mark_selection(PdfAnnotationKind::Highlight);
    }

    pub fn underline_selection(&self) {
        self.mark_selection(PdfAnnotationKind::Underline);
    }

    pub fn strike_out_selection(&self) {
        self.mark_selection(PdfAnnotationKind::StrikeOut);
    }

    fn mark_selection(&self, kind: PdfAnnotationKind) {
        self.marking.update(|m| *m = (m.0 + 1, kind));
    }

    /// The annotation that was picked by clicking on it.
    pub fn selected_annotation(&self) -> Option<PdfAnnotationId> {
        self.selected.get()
    }

    /// Takes away what was picked.
    pub fn delete_annotation(&self) {
        let Some(id) = self.selected.get_untracked() else {
            return;
        };
        self.selected.set(None);
        self.remove_annotation(id);
    }

    /// Takes away an annotation.
    pub fn remove_annotation(&self, id: PdfAnnotationId) {
        let Some(mut annot) = self.annot(id) else {
            return;
        };
        self.change_edits(|edits| match id {
            PdfAnnotationId::New(_) => {
                edits.annots.remove(&id);
            }
            PdfAnnotationId::Old(..) => {
                annot.removed = true;
                edits.annots.insert(id, annot);
            }
        });
    }

    /// The annotations of a page that can be picked: what it had, and what was added since.
    pub fn annotations(&self, page: usize) -> Vec<(PdfAnnotationId, PdfAnnotationKind, String)> {
        let _ = (self.edits.get(), self.marks.get());
        self.annots_on(page)
            .into_iter()
            .map(|a| (a.id, a.kind, a.contents))
            .collect()
    }

    fn annots_on(&self, page: usize) -> Vec<Annot> {
        let marks = self.marks.get_untracked();
        let edits = self.edits.get_untracked();
        let old = marks.pages.get(page).map_or(&[][..], |p| &p.annots[..]);
        let mut all: Vec<Annot> = old
            .iter()
            .map(|a| edits.annots.get(&a.id).unwrap_or(a).clone())
            .collect();
        all.extend(
            edits
                .annots
                .values()
                .filter(|a| a.page == page && matches!(a.id, PdfAnnotationId::New(_)))
                .cloned(),
        );
        all.retain(|a| !a.removed);
        all
    }

    fn annot(&self, id: PdfAnnotationId) -> Option<Annot> {
        if let Some(annot) = self.edits.with_untracked(|e| e.annots.get(&id).cloned()) {
            return (!annot.removed).then_some(annot);
        }
        let PdfAnnotationId::Old(page, _) = id else {
            return None;
        };
        self.marks.with_untracked(|m| {
            m.pages
                .get(page)?
                .annots
                .iter()
                .find(|a| a.id == id)
                .cloned()
        })
    }

    fn change_annot(&self, annot: Annot) {
        let original = self.marks.with_untracked(|m| {
            m.pages
                .get(annot.page)
                .and_then(|p| p.annots.iter().find(|a| a.id == annot.id).cloned())
        });
        self.change_edits(|edits| {
            if original.as_ref() == Some(&annot) {
                edits.annots.remove(&annot.id);
            } else {
                edits.annots.insert(annot.id, annot);
            }
        });
    }

    fn add_annot(
        &self,
        page: usize,
        kind: PdfAnnotationKind,
        rect: [f64; 4],
        color: [f32; 3],
        shape: Shape,
        contents: String,
    ) -> PdfAnnotationId {
        let number = self.next_annot.get_untracked() + 1;
        self.next_annot.set(number);
        let id = PdfAnnotationId::New(number);
        let annot = Annot {
            id,
            page,
            kind,
            rect,
            contents,
            color,
            shape: Arc::new(shape),
            removed: false,
            date: pdf_date(std::time::SystemTime::now()),
        };
        self.change_edits(|edits| {
            edits.annots.insert(id, annot);
        });
        id
    }

    /// The color something gets with a tool, as parts from 0 to 1.
    fn color_for(&self, tool: PdfTool) -> [f32; 3] {
        let color = self.tool_color.get_untracked().unwrap_or(match tool {
            PdfTool::Highlight | PdfTool::Note | PdfTool::Select => HIGHLIGHT,
            PdfTool::Underline | PdfTool::StrikeOut | PdfTool::Ink => LINE,
            PdfTool::FreeText => TEXT,
        });
        [
            ((color >> 16) & 0xff) as f32 / 255.0,
            ((color >> 8) & 0xff) as f32 / 255.0,
            (color & 0xff) as f32 / 255.0,
        ]
    }
}

/// A moment as a PDF writes it, in UTC.
fn pdf_date(at: std::time::SystemTime) -> String {
    let seconds = at
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let days = seconds.div_euclid(86400);
    let left = seconds.rem_euclid(86400);
    // Days to a date in the calendar, as Howard Hinnant's `civil_from_days` has it.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        left / 3600,
        left / 60 % 60,
        left % 60
    )
}

type ErrorListener = Rc<dyn Fn(&PdfError)>;
type PageErrorListener = Rc<dyn Fn(usize)>;
pub(crate) type UrlListener = Rc<dyn Fn(&str)>;
pub(crate) type FieldListener = Rc<dyn Fn(&PdfFieldChange)>;

/// A document in a box that scrolls. See [`pdf_view`].
pub struct PdfView {
    root: Div,
    source: PdfSource,
    viewer: Option<PdfViewer>,
    on_error: Option<ErrorListener>,
    on_page_error: Option<PageErrorListener>,
    on_link: Option<UrlListener>,
    on_field_change: Option<FieldListener>,
    field_tint: Option<Color>,
    page_style: Option<Rc<dyn Fn(Style) -> Style>>,
    gap: f32,
    workers: usize,
    cache_bytes: usize,
}

/// Shows a PDF. It takes the box the style gives it, and opens without holding up the UI.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use anti_webui_pdf::{PdfZoom, pdf_view, pdf_viewer};
///
/// # fn app() -> impl IntoView {
/// let viewer = pdf_viewer();
/// viewer.set_zoom(PdfZoom::Percent(1.25));
/// pdf_view("paper.pdf").viewer(viewer).w_full().h_full()
/// # }
/// ```
pub fn pdf_view(source: impl Into<PdfSource>) -> PdfView {
    PdfView {
        root: div(),
        source: source.into(),
        viewer: None,
        on_error: None,
        on_page_error: None,
        on_link: None,
        on_field_change: None,
        field_tint: Some(Color::from_rgba(FIELD_TINT)),
        page_style: None,
        gap: 8.0,
        workers: 2,
        cache_bytes: 256 << 20,
    }
}

impl Styled for PdfView {
    fn style_mut(&mut self) -> &mut Style {
        self.root.style_mut()
    }
}

impl PdfView {
    pub fn viewer(mut self, viewer: PdfViewer) -> Self {
        self.viewer = Some(viewer);
        self
    }

    /// Called when it can't be shown, and with why.
    pub fn on_error(mut self, f: impl Fn(&PdfError) + 'static) -> Self {
        self.on_error = Some(Rc::new(f));
        self
    }

    /// Called with a page, from 0, that can't be drawn, which then says so where it would show.
    pub fn on_page_error(mut self, f: impl Fn(usize) + 'static) -> Self {
        self.on_page_error = Some(Rc::new(f));
        self
    }

    /// Called with the address of a link to outside of the document that was clicked. Nothing opens
    /// it unless this does: documents can link to anything. Links within the document just go there.
    pub fn on_link(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_link = Some(Rc::new(f));
        self
    }

    /// Called when someone changes what is in a field. What is changed by [`PdfViewer::set_field`]
    /// and by going back doesn't count.
    pub fn on_field_change(mut self, f: impl Fn(&PdfFieldChange) + 'static) -> Self {
        self.on_field_change = Some(Rc::new(f));
        self
    }

    /// What shows on top of the fields that can be filled in, if anything: a light blue unless this
    /// says otherwise.
    pub fn field_tint(mut self, tint: Option<Color>) -> Self {
        self.field_tint = tint;
        self
    }

    /// Styles each page, like with a shadow. A page is white unless this says otherwise.
    pub fn page_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.page_style = Some(Rc::new(f));
        self
    }

    /// The room between the pages and around them, in logical pixels. 8 unless this says otherwise.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// How many threads draw at most. Two unless this says otherwise: more draws a little faster
    /// and takes a lot more memory, since each has its own caches.
    pub fn workers(mut self, workers: usize) -> Self {
        self.workers = workers;
        self
    }

    /// How many bytes the pictures of the pages may take, 256 MiB unless this says otherwise. What
    /// shows stays even if that is more, and pages that don't show aren't drawn ahead then.
    pub fn cache_bytes(mut self, bytes: usize) -> Self {
        self.cache_bytes = bytes;
        self
    }
}

impl IntoView for PdfView {
    fn into_view(self) -> View {
        let viewer = self.viewer.unwrap_or_else(pdf_viewer);
        let (source, workers, budget) = (self.source, self.workers, self.cache_bytes);
        let page_style = self.page_style;
        let gap = self.gap;
        let on_error = self.on_error;
        let on_page_error = self.on_page_error;
        let on_link = self.on_link;
        let on_field_change = self.on_field_change;
        let field_tint = self.field_tint;

        let pages = viewer.pages;
        let pinching = signal(false);
        let pictures = signal(HashMap::<usize, Arc<Bitmap>>::new());
        // A sharp picture of what shows, of pages too big for one of all of it.
        let parts = signal(HashMap::<usize, (Job, Arc<Bitmap>)>::new());
        let failed = signal(HashSet::<usize>::new());
        let failed_parts = signal(HashSet::<usize>::new());
        // The pictures that show what a page was before it was changed, until the new ones come.
        let stale = signal(HashSet::<usize>::new());
        let stale_parts = signal(HashSet::<usize>::new());
        let shown_edits = signal(Arc::<Edits>::default());
        // The pages the last plan wants to have a picture.
        let keep = Rc::new(RefCell::new(Vec::<usize>::new()));
        let viewport = signal((0.0f32, 0.0f32));
        let offset = signal(PointF::new(0.0, 0.0));
        let direction = Rc::new(Cell::new(Direction::Down));
        let scroller = viewer.node;
        // Where the pointer is in the box, in physical pixels: what zooming with it keeps in place.
        let pointer = Rc::new(Cell::new(None::<(i32, i32)>));

        let px_per_pt = memo(move || {
            let scale = window_scale();
            let css = PX_PER_PT * scale.as_f32() as f64;
            let (width, height) = viewport.get();
            let (width, height) = (scale.px(width) as f64, scale.px(height) as f64);
            let gap = scale.px(gap) as f64;
            let turn = viewer.turn.get();
            let fit_width = move |pages: &[(f32, f32)]| {
                let widest = pages.iter().map(|p| turn.size(*p).0).fold(0.0, f32::max) as f64;
                ((width - 2.0 * gap) / widest.max(1.0)).max(0.0)
            };
            let wanted = pages.with(|pages| match viewer.zoom.get() {
                PdfZoom::Percent(p) => p as f64 * css,
                PdfZoom::FitWidth => fit_width(pages),
                PdfZoom::FitPage => {
                    let tall = pages.first().map_or(1.0, |p| turn.size(*p).1) as f64;
                    fit_width(pages).min(((height - 2.0 * gap) / tall.max(1.0)).max(0.0))
                }
            });
            wanted.clamp(MIN_ZOOM as f64 * css, MAX_ZOOM as f64 * css)
        });
        effect(move || {
            let css = PX_PER_PT * window_scale().as_f32() as f64;
            viewer
                .percent
                .set_if_changed((px_per_pt.get() / css) as f32);
        });

        let table = memo(move || {
            let scale = window_scale();
            let (width, height) = viewport.get();
            let (width, height) = (scale.px(width), scale.px(height));
            let turn = viewer.turn.get();
            if viewer.presenting.get() {
                return Rc::new(
                    pages.with(|pages| PageTable::slides(pages, turn, (width, height))),
                );
            }
            Rc::new(pages.with(|pages| {
                PageTable::turned(pages, turn, px_per_pt.get(), scale.px(gap), width)
            }))
        });

        // What the box shows, in the physical pixels of what scrolls. When presenting that goes by
        // the page and `offset` stays for later.
        let view = move || {
            let scale = window_scale();
            let (width, height) = viewport.get();
            let (x, y) = table.with(|t| {
                if t.slot > 0 {
                    (0, t.slot_top(viewer.slide.get()))
                } else {
                    let at = offset.get();
                    (scale.px(at.x), scale.px(at.y))
                }
            });
            RectI::new(x, y, scale.px(width), scale.px(height))
        };

        // Whatever makes the pages move keeps the same place on the same page in the same place of
        // the box: what is under the pointer or in the middle when zooming, and what is at the top
        // when the box changes. It runs before the next layout, which then scrolls with the new
        // sizes.
        {
            let last = RefCell::new(Rc::new(PageTable::default()));
            let last_size = Cell::new((0.0f32, 0.0f32));
            let pointer = pointer.clone();
            effect(move || {
                let new = table.get();
                let old = std::mem::replace(&mut *last.borrow_mut(), new.clone());
                let (width, height) = untrack(|| viewport.get());
                let was = last_size.replace((width, height));
                // Nothing showed before the box had a size, so there is no place to keep. Presenting
                // takes care of itself.
                if old.is_empty() || *old == *new || was.1 <= 0.0 || old.slot > 0 || new.slot > 0 {
                    return;
                }
                let scale = untrack(window_scale);
                let at = untrack(|| offset.get());
                let (ax, ay) = if was != (width, height) {
                    (0, 0)
                } else {
                    pointer
                        .get()
                        .unwrap_or((scale.px(width) / 2, scale.px(height) / 2))
                };
                let (x, y) = old.move_point(&new, scale.px(at.x) + ax, scale.px(at.y) + ay);
                let (x, y) = ((x - ax).max(0), (y - ay).max(0));
                scroller.scroll_to_after_layout(x, y);
                offset.set(PointF::new(
                    scale.to_logical(x as f32),
                    scale.to_logical(y as f32),
                ));
            });
        }

        // The page that is being read.
        effect(move || {
            let v = view();
            let t = table.get();
            if !t.is_empty() {
                viewer.page.set_if_changed(t.page_at_middle(v.y, v.height));
            }
        });

        effect(move || {
            let Some(target) = viewer.go_to.get() else {
                return;
            };
            let t = table.get();
            if t.is_empty() || viewport.get().1 <= 0.0 {
                return;
            }
            if t.slot > 0 {
                viewer.slide.set(target.page.min(t.len() - 1));
                viewer.go_to.set(None);
                return;
            }
            let scale = untrack(window_scale);
            let y = pages.with_untracked(|pages| reading::target_position(&t, pages, target));
            let Some(y) = y else {
                return;
            };
            let x = scale.px(untrack(|| offset.get()).x);
            scroller.scroll_to_after_layout(x, y);
            offset.set(PointF::new(
                scale.to_logical(x as f32),
                scale.to_logical(y as f32),
            ));
            viewer.go_to.set(None);
        });

        let engine = Rc::new(RefCell::new(None::<Engine>));
        let reader = Reader::new(
            viewer,
            table,
            pages,
            Rc::new(view),
            scroller,
            on_link,
            engine.clone(),
        );
        let editor = Editor::new(
            viewer,
            reader.clone(),
            table,
            on_field_change,
            field_tint,
            stale,
            shown_edits,
        );
        {
            // Not the editor itself, which already has the reader.
            let editor = Rc::downgrade(&editor);
            reader.set_marker(Rc::new(move |kind, pages| {
                if let Some(editor) = editor.upgrade() {
                    editor.mark(kind, pages);
                }
            }));
        }

        let slides = Slides::new(viewer, reader.clone(), direction.clone());
        // The page that shows when presenting, which stays until the next has been drawn.
        let displayed = signal(0usize);

        // When it is on all of the screen, for whatever reason.
        {
            let slides = slides.clone();
            let editor = editor.clone();
            // Where it was, and on which page it was reading.
            let left = RefCell::new(None::<(Rc<PageTable>, (i32, i32), usize)>);
            effect(move || {
                let full = scroller.is_fullscreen();
                untrack(|| {
                    let presenting = viewer.presenting.get();
                    if full && !presenting && viewer.present_asked.get() {
                        let page = viewer.page.get();
                        let v = view();
                        *left.borrow_mut() = Some((table.get(), (v.x, v.y), page));
                        editor.commit();
                        viewer.clear_selection();
                        viewer.selected.set_if_changed(None);
                        viewer.slide.set(page);
                        displayed.set(page);
                        viewer.presenting.set(true);
                        viewer.node.focus();
                        slides.started();
                    } else if !full && presenting {
                        viewer.presenting.set(false);
                        viewer.present_asked.set(false);
                        slides.stopped();
                        let Some((before, (x, y), page)) = left.borrow_mut().take() else {
                            return;
                        };
                        let slide = viewer.slide.get();
                        if slide != page {
                            viewer.go_to_page(slide);
                            return;
                        }
                        // The same page goes back to the same place on it.
                        let scale = window_scale();
                        let (x, y) = before.move_point(&table.get(), x, y);
                        let (x, y) = (x.max(0), y.max(0));
                        scroller.scroll_to_after_layout(x, y);
                        offset.set(PointF::new(
                            scale.to_logical(x as f32),
                            scale.to_logical(y as f32),
                        ));
                    }
                });
            });
        }

        // What comes back from the workers. A picture at another size than is wanted now only helps
        // a page that has nothing to show.
        // What the file gets when it is saved.
        let saving = signal(Arc::<Edits>::default());
        let out = {
            let on_error = on_error.clone();
            let keep = keep.clone();
            let reader = reader.clone();
            let engine = engine.clone();
            ui_channel(move |event: Event| match event {
                Event::Text(page, text) => reader.text_arrived(page, text),
                Event::Outline(outline) => viewer.outline.set(Rc::new(outline)),
                Event::Marks(marks) => {
                    let needs_looks = marks.needs_looks() && marks.editing == PdfEditing::Yes;
                    viewer.marks.set(marks);
                    if needs_looks && let Some(engine) = &*engine.borrow() {
                        engine.edit(viewer.edits.get_untracked());
                    }
                }
                Event::Edited(edits) => {
                    let marks = viewer.marks.get_untracked();
                    let before = shown_edits.get_untracked();
                    // What it made without being asked to shows on the pages that have it.
                    let mut changed = edits.pages_changed_from(&before, &marks);
                    if before.is_empty() && edits.is_empty() {
                        changed.extend(
                            marks
                                .fields
                                .iter()
                                .filter(|f| f.needs_look)
                                .flat_map(|f| f.widgets.iter().map(|w| w.0)),
                        );
                    }
                    pictures.with_untracked(|all| {
                        stale.update(|s| {
                            s.extend(changed.iter().filter(|p| all.contains_key(p)).copied());
                        });
                    });
                    parts.with_untracked(|all| {
                        stale_parts.update(|s| {
                            s.extend(changed.iter().filter(|p| all.contains_key(p)).copied());
                        });
                    });
                    viewer.thumbnails.update(|all| {
                        for page in &changed {
                            all.remove(page);
                        }
                    });
                    failed.update(|f| f.retain(|p| !changed.contains(p)));
                    failed_parts.update(|f| f.retain(|p| !changed.contains(p)));
                    reader.forget_text(&changed);
                    shown_edits.set(edits);
                }
                Event::NotEdited(e) => {
                    log::warn!("couldn't edit a PDF: {e}");
                    viewer.edit_error.set(Some(e));
                    // What was typed doesn't stay up for what won't come.
                    shown_edits.set(viewer.edits.get_untracked());
                }
                Event::Saved(path, result) => {
                    viewer.save_result.set(Some(result.map(|()| path)));
                    if matches!(viewer.save_result.get_untracked(), Some(Ok(_))) {
                        viewer.saved.set(saving.with_untracked(Clone::clone));
                    }
                }
                Event::Opened(sizes) => {
                    viewer.count.set(sizes.len());
                    pages.set(Rc::new(sizes));
                    viewer.state.set(PdfState::Ready);
                }
                Event::Failed(e) => {
                    if let Some(f) = &on_error {
                        f(&e);
                    }
                    viewer.state.set(PdfState::Failed(e));
                }
                Event::NotDrawn(job) => {
                    log::warn!("couldn't draw page {} of a PDF", job.page + 1);
                    if job.area.is_some() {
                        stale_parts.update(|s| {
                            s.remove(&job.page);
                        });
                        failed_parts.update(|f| {
                            f.insert(job.page);
                        });
                        return;
                    }
                    stale.update(|s| {
                        s.remove(&job.page);
                    });
                    failed.update(|f| {
                        f.insert(job.page);
                    });
                    if let Some(f) = &on_page_error {
                        f(job.page);
                    }
                }
                Event::Thumbnail(job, bitmap) => {
                    let turn = viewer.turn.get_untracked();
                    if viewer.thumbnail_lists.get_untracked() > 0 && job.turn == turn {
                        viewer.thumbnails.update(|all| {
                            all.insert(job.page, (job, bitmap));
                        });
                    }
                }
                Event::CutOut(cut, picture) => reader.cut_out_arrived(&cut, picture),
                Event::Drawn(job, bitmap) => {
                    let t = untrack(|| table.get());
                    if job.page >= t.len() || job.turn != t.turn {
                        return;
                    }
                    let v = untrack(view);
                    let size = t.size(job.page);
                    if job.area.is_some() {
                        let shown = t.shown(job.page, v);
                        // A picture of what the page was has nothing on one of what it is.
                        let current = parts
                            .with_untracked(|all| all.get(&job.page).map(|p| p.0))
                            .filter(|_| !stale_parts.with_untracked(|s| s.contains(&job.page)));
                        if !plan::part_is_of_use(&t, &job, v)
                            || !plan::takes_over(&job, current.as_ref(), size, shown)
                        {
                            return;
                        }
                        parts.update(|all| {
                            all.insert(job.page, (job, bitmap));
                        });
                        stale_parts.update(|s| {
                            s.remove(&job.page);
                        });
                    } else {
                        let right = plan::is_right_size(size, job.size());
                        let wanted = keep.borrow().contains(&job.page);
                        let there = pictures.with_untracked(|all| {
                            all.get(&job.page).map(|b| (b.width(), b.height()))
                        });
                        // What was drawn while pinching stays closer than what is there, even if
                        // the size has moved on since.
                        let want = plan::picture_size(size);
                        let closer = pinching.get_untracked()
                            && there.is_some_and(|there| {
                                plan::size_is_off(job.size(), want) < plan::size_is_off(there, want)
                            });
                        if !right && !closer && (there.is_some() || !wanted) {
                            return;
                        }
                        pictures.update(|all| {
                            all.insert(job.page, bitmap);
                        });
                        stale.update(|s| {
                            s.remove(&job.page);
                        });
                    }
                    // The parts show, so only the rest of the budget is for the whole pictures.
                    let taken: usize =
                        parts.with_untracked(|all| all.values().map(|p| p.0.bytes()).sum());
                    let visible = t.visible(v.y, v.height);
                    let center = t.page_at_middle(v.y, v.height);
                    let dropped = pictures.with_untracked(|all| {
                        let held = all.iter().map(|(&page, b)| {
                            let size = (b.width(), b.height());
                            Held {
                                page,
                                bytes: size.0 as usize * size.1 as usize * 4,
                                right_size: plan::is_right_size(t.size(page), size),
                            }
                        });
                        plan::evict(
                            held,
                            &visible,
                            &keep.borrow(),
                            center,
                            budget.saturating_sub(taken),
                        )
                    });
                    if !dropped.is_empty() {
                        pictures.update(|all| {
                            for page in dropped {
                                all.remove(&page);
                            }
                        });
                    }
                }
            })
        };

        {
            let engine = engine.clone();
            effect(move || {
                let password = viewer.password.get();
                let source = source.clone();
                let out = out.clone();
                let opening = Engine::open(
                    move || document::open(&source, password.as_deref()),
                    out,
                    workers,
                );
                // Opening again is only for a password, when there are no pages yet.
                viewer.state.set(PdfState::Loading);
                *engine.borrow_mut() = Some(opening);
            });
        }
        {
            let engine = engine.clone();
            on_cleanup(move || *engine.borrow_mut() = None);
        }

        // Whatever changes, the document gets made again with all of it.
        {
            let engine = engine.clone();
            let asked = RefCell::new(viewer.edits.get_untracked());
            effect(move || {
                let edits = viewer.edits.get();
                if Arc::ptr_eq(&edits, &asked.borrow()) {
                    return;
                }
                *asked.borrow_mut() = edits.clone();
                viewer.edit_error.set_if_changed(None);
                if let Some(engine) = &*engine.borrow() {
                    engine.edit(edits);
                }
            });
        }
        {
            let engine = engine.clone();
            effect(move || {
                let Some(path) = viewer.save_to.get() else {
                    return;
                };
                let edits = untrack(|| viewer.edits.get());
                saving.set(edits.clone());
                if let Some(engine) = &*engine.borrow() {
                    engine.save(edits, path);
                }
                viewer.save_to.set(None);
            });
        }
        {
            let reader = reader.clone();
            let asked = Cell::new(0);
            effect(move || {
                let (count, kind) = viewer.marking.get();
                if asked.replace(count) != count {
                    untrack(|| reader.mark_selection(kind));
                }
            });
        }

        // Pictures can't be used turned. As in Chromium and PDF.js, the pages are white until they
        // have been drawn again.
        {
            let last = Cell::new(Turn::default());
            effect(move || {
                let turn = viewer.turn.get();
                if last.replace(turn) != turn {
                    pictures.update(HashMap::clear);
                    parts.update(HashMap::clear);
                    viewer.thumbnails.update(HashMap::clear);
                }
            });
        }

        {
            let engine = engine.clone();
            effect(move || {
                let wanted = viewer.thumbnails_wanted.get();
                if let Some(engine) = &*engine.borrow() {
                    engine.want_thumbnails(wanted);
                }
            });
        }

        // What no longer shows goes right away.
        effect(move || {
            let (t, v) = (table.get(), view());
            let of_use = |job: &Job| job.page < t.len() && plan::part_is_of_use(&t, job, v);
            if parts.with(|all| all.values().any(|p| !of_use(&p.0))) {
                parts.update(|all| all.retain(|_, p| of_use(&p.0)));
            }
        });

        reader.watch_needs();
        reader.watch_current_hit();
        {
            let reader = reader.clone();
            effect(move || {
                let query = viewer.query.get();
                if viewer.count.get() > 0 {
                    untrack(|| reader.start_search(&query));
                }
            });
        }
        {
            let reader = reader.clone();
            let asked = Cell::new(0);
            effect(move || {
                let copies = viewer.copies.get();
                if asked.replace(copies) != copies {
                    untrack(|| reader.copy(ClipboardKind::Clipboard));
                }
            });
        }

        // Says what to draw whenever that may have changed. While pinching what there is gets
        // stretched, as nearly all that gets drawn would be for a size that has gone by the time it
        // comes. Only what is too far off gets drawn again, as in Chromium.
        {
            let direction = direction.clone();
            let keep = keep.clone();
            let goals = RefCell::new(HashMap::new());
            effect(move || {
                let t = table.get();
                let v = view();
                let pinching = pinching.get();
                let (stale, stale_parts) = (stale.get(), stale_parts.get());
                let plan = failed.with(|failed| {
                    failed_parts.with(|failed_parts| {
                        pictures.with(|have| {
                            parts.with(|parts| {
                                plan::plan(
                                    &t,
                                    v,
                                    direction.get(),
                                    budget,
                                    Have {
                                        whole: |page| {
                                            if failed.contains(&page) {
                                                return Some(plan::picture_size(t.size(page)));
                                            }
                                            if stale.contains(&page) {
                                                return None;
                                            }
                                            let size =
                                                have.get(&page).map(|b| (b.width(), b.height()));
                                            let wanted = plan::picture_size(t.size(page));
                                            match size {
                                                Some(size)
                                                    if pinching
                                                        && plan::size_is_off(size, wanted)
                                                            < PINCH_RATIO =>
                                                {
                                                    Some(wanted)
                                                }
                                                size => size,
                                            }
                                        },
                                        part: |page| {
                                            parts
                                                .get(&page)
                                                .map(|p| p.0)
                                                .filter(|_| !stale_parts.contains(&page))
                                        },
                                        gave_up: |page| pinching || failed_parts.contains(&page),
                                    },
                                    &mut goals.borrow_mut(),
                                )
                            })
                        })
                    })
                });
                let mut jobs = plan.jobs;
                if pinching {
                    let visible = t.visible(v.y, v.height);
                    jobs.retain(|job| visible.contains(&job.page));
                }
                *keep.borrow_mut() = plan.keep;
                if let Some(engine) = &*engine.borrow() {
                    engine.want(jobs);
                }
            });
        }

        // Goes on to the next page when it has been drawn, or when that takes too long. Whatever else
        // changes in the meantime starts this over, and that doesn't start the wait over.
        {
            let waiting = Cell::new(None::<(usize, Instant)>);
            let gave_up = signal(None::<usize>);
            effect(move || {
                if !viewer.presenting.get() {
                    waiting.set(None);
                    return;
                }
                let page = viewer.slide.get();
                let t = table.get();
                if page >= t.len() {
                    return;
                }
                let drawn = pictures.with(|all| {
                    all.get(&page)
                        .is_some_and(|b| plan::is_right_size(t.size(page), (b.width(), b.height())))
                });
                if drawn || failed.with(|f| f.contains(&page)) || gave_up.get() == Some(page) {
                    waiting.set(None);
                    displayed.set_if_changed(page);
                    return;
                }
                if displayed.get_untracked() == page {
                    return;
                }
                let since = match waiting.get() {
                    Some((p, since)) if p == page => since,
                    _ => Instant::now(),
                };
                waiting.set(Some((page, since)));
                set_timeout(SLIDE_WAITS.saturating_sub(since.elapsed()), move || {
                    gave_up.set(Some(page));
                });
            });
        }

        let on_scrolled = {
            let direction = direction.clone();
            let reader = reader.clone();
            move |at: PointF| {
                // The box doesn't scroll then, and what it did goes back to `offset` afterwards.
                if viewer.presenting.get_untracked() {
                    return;
                }
                let old = offset.get_untracked();
                if at.y > old.y {
                    direction.set(Direction::Down);
                } else if at.y < old.y {
                    direction.set(Direction::Up);
                }
                offset.set_if_changed(at);
                // What is under a pointer that is selecting has changed.
                reader.extend_selection();
            }
        };

        let on_pinch = {
            let pointer = pointer.clone();
            move |e: &PinchEvent| {
                if viewer.presenting.get_untracked() {
                    e.stop_propagation();
                    return;
                }
                pinching.set_if_changed(matches!(e.phase, PinchPhase::Began | PinchPhase::Moved));
                if e.phase == PinchPhase::Moved {
                    pointer.set(Some((e.local.x as i32, e.local.y as i32)));
                    viewer.zoom_by(e.scale);
                }
                e.stop_propagation();
            }
        };

        let on_scroll = {
            let slides = slides.clone();
            move |e: &ScrollEvent| {
                if viewer.presenting.get_untracked() {
                    slides.scroll(e);
                    return;
                }
                if e.modifiers.ctrl {
                    // A notch of a wheel is about a tenth, and a touchpad gets there in small steps.
                    viewer.zoom_by((-e.delta_y / 1000.0).exp());
                    e.stop_propagation();
                }
            }
        };

        let on_key = {
            let editor = editor.clone();
            let slides = slides.clone();
            move |e: &KeyboardEvent| {
                if slides.key(e) || editor.key(e) {
                    return;
                }
                let scale = untrack(window_scale);
                let at = scroller.scroll_offset();
                let height = viewport.get_untracked().1;
                let total = scale.to_logical(table.with(|t| t.height) as f32);
                if e.modifiers.ctrl {
                    match &e.key {
                        Key::Character(c) if c == "=" || c == "+" => viewer.zoom_by(1.25),
                        Key::Character(c) if c == "-" => viewer.zoom_by(0.8),
                        Key::Character(c) if c == "0" => viewer.set_zoom(PdfZoom::FitWidth),
                        // As in Chromium's viewer.
                        Key::Character(c) if c == "]" => viewer.rotate_clockwise(),
                        Key::Character(c) if c == "[" => viewer.rotate_counterclockwise(),
                        // Otherwise the text of the application would get these too.
                        Key::Character(c)
                            if c.eq_ignore_ascii_case("c")
                                && viewer.selection.get_untracked().is_some() =>
                        {
                            viewer.copy();
                            e.prevent_default();
                        }
                        Key::Character(c) if c.eq_ignore_ascii_case("a") => {
                            viewer.select_all();
                            e.prevent_default();
                        }
                        _ => return,
                    }
                    e.stop_propagation();
                    return;
                }
                let y = match &e.key {
                    Key::Named(NamedKey::PageDown) => at.y + height * 0.9,
                    Key::Named(NamedKey::PageUp) => at.y - height * 0.9,
                    Key::Named(NamedKey::ArrowDown) => at.y + 40.0,
                    Key::Named(NamedKey::ArrowUp) => at.y - 40.0,
                    Key::Named(NamedKey::Home) => 0.0,
                    Key::Named(NamedKey::End) => total,
                    Key::Named(NamedKey::Escape) if viewer.selection.get_untracked().is_some() => {
                        viewer.clear_selection();
                        e.stop_propagation();
                        return;
                    }
                    _ => return,
                };
                scroller.scroll_to(at.x, y);
                e.stop_propagation();
            }
        };

        let page_view = {
            let reader = reader.clone();
            let editor = editor.clone();
            move |page: usize| {
                let page_style = page_style.clone();
                let reader = reader.clone();
                let editor = editor.clone();
                let tint = editor.tint();
                div()
                    .absolute()
                    // Pages go while the button is held when selecting past the edge of the box, and
                    // what was pressed has to stay for the rest of the drag to come.
                    .pointer_events_none()
                    .dyn_style(move |s| {
                        let t = table.get();
                        if page >= t.len() {
                            return s;
                        }
                        let (w, h) = t.size(page);
                        let s = s
                            .left(Length::Device(t.left(page)))
                            .top(Length::Device(t.top(page) - t.slot_top(displayed.get())))
                            .w(Length::Device(w))
                            .h(Length::Device(h))
                            .bg(Color::WHITE);
                        match &page_style {
                            Some(f) if t.slot == 0 => f(s),
                            _ => s,
                        }
                    })
                    .child(
                        bitmap_view(move || pictures.with(|all| all.get(&page).cloned()))
                            .w_full()
                            .h_full(),
                    )
                    .child(
                        bitmap_view(move || parts.with(|all| all.get(&page).map(|p| p.1.clone())))
                            .absolute()
                            .dyn_style(move |s| {
                                let t = table.get();
                                let job = parts.with(|all| all.get(&page).map(|p| p.0));
                                let (Some(job), true) = (job, page < t.len()) else {
                                    return s;
                                };
                                let area = job.area_at(t.size(page));
                                s.left(Length::Device(area.x))
                                    .top(Length::Device(area.y))
                                    .w(Length::Device(area.width))
                                    .h(Length::Device(area.height))
                            }),
                    )
                    .child(show(
                        move || failed.with(|f| f.contains(&page)),
                        || {
                            div()
                                .absolute()
                                .inset(Length::Px(0.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    text("This page can't be drawn")
                                        .text_color(Color::from_rgb(0x666666)),
                                )
                        },
                    ))
                    .child(for_each(
                        move || {
                            if viewer.presenting.get() {
                                return Vec::new();
                            }
                            let mut boxes = editor.field_boxes(page);
                            boxes.extend(reader.boxes(page));
                            boxes
                        },
                        |b| *b,
                        move |b| {
                            let color = match b.kind {
                                BoxKind::Selected => Color::from_rgba(SELECTED),
                                BoxKind::Found => Color::from_rgba(FOUND),
                                BoxKind::Current => Color::from_rgba(CURRENT),
                                BoxKind::Field => tint,
                            };
                            div()
                                .absolute()
                                .left(Length::Device(b.x))
                                .top(Length::Device(b.y))
                                .w(Length::Device(b.width))
                                .h(Length::Device(b.height))
                                .bg(color)
                        },
                    ))
            }
        };

        self.root
            .overflow_scroll()
            .focusable()
            // Selecting here is up to the reader, and what is pressed isn't text of the application.
            .select_none()
            .node_ref(scroller)
            .on_scrolled(on_scrolled)
            .on_resized(move |w, h| viewport.set_if_changed((w, h)))
            .on_scroll(on_scroll)
            .on_pinch(on_pinch)
            .on_key_down(on_key)
            .dyn_style({
                let cursor = reader.cursor;
                let hidden = slides.pointer_hidden;
                move |s| {
                    if hidden.get() {
                        return s.cursor(CursorIcon::None);
                    }
                    let s = s.cursor(cursor.get());
                    if viewer.presenting.get() {
                        s.bg(Color::BLACK)
                    } else {
                        s
                    }
                }
            })
            .on_pointer_down({
                let reader = reader.clone();
                let editor = editor.clone();
                let slides = slides.clone();
                move |e| {
                    if viewer.presenting.get_untracked() {
                        slides.pointer_down(e);
                    } else if !editor.pointer_down(e) {
                        reader.pointer_down(e);
                    }
                }
            })
            .on_pointer_up({
                let reader = reader.clone();
                let editor = editor.clone();
                move |e| {
                    if viewer.presenting.get_untracked() {
                        return;
                    }
                    let taken = editor.is_pressed();
                    if !taken {
                        reader.pointer_up(e);
                    }
                    editor.pointer_up(e);
                }
            })
            .on_pointer_move({
                let pointer = pointer.clone();
                let reader = reader.clone();
                let editor = editor.clone();
                let slides = slides.clone();
                move |e| {
                    if viewer.presenting.get_untracked() {
                        slides.pointer_moved(e);
                        return;
                    }
                    pointer.set(Some((e.local.x as i32, e.local.y as i32)));
                    if editor.pointer_moved(e) {
                        return;
                    }
                    reader.pointer_moved(e);
                    if let Some(cursor) = editor.cursor_at(e) {
                        reader.cursor.set_if_changed(cursor);
                    }
                }
            })
            .on_hover(move |inside| {
                if !inside {
                    pointer.set(None);
                    reader.pointer_left();
                }
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .dyn_style(move |s| {
                        let t = table.get();
                        // One page at a time doesn't scroll.
                        let height = if t.slot > 0 { t.slot } else { t.height };
                        s.w(Length::Device(t.width)).h(Length::Device(height))
                    })
                    .child(for_each(
                        move || {
                            if table.with(|t| t.slot > 0) {
                                return vec![displayed.get()];
                            }
                            let scale = window_scale();
                            let top = scale.px(offset.get().y);
                            let height = scale.px(viewport.get().1);
                            table.with(|t| {
                                let visible = t.visible(top, height);
                                let first = visible.start.saturating_sub(1);
                                let last = (visible.end + 1).min(t.len());
                                (first..last).collect::<Vec<_>>()
                            })
                        },
                        |page| *page,
                        page_view,
                    ))
                    .child(show(
                        move || !viewer.presenting.get(),
                        move || editor.layer(),
                    )),
            )
            .child(show(
                {
                    let blank = slides.blank;
                    move || blank.get().is_some()
                },
                {
                    let blank = slides.blank;
                    move || {
                        div()
                            .absolute()
                            .inset(Length::Px(0.0))
                            .pointer_events_none()
                            .dyn_style(move |s| match blank.get() {
                                Some(color) => s.bg(color),
                                None => s,
                            })
                    }
                },
            ))
            .into_view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anti_webui_core::Ui;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::geometry::{Scale, SizeI};
    use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
    use engine::{Document, Session};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Pages that can't be drawn, as there is no real document that makes `hayro` panic.
    #[derive(Clone)]
    pub struct FakeSource {
        pub doc: Arc<Fake>,
        pub pages: Vec<(f32, f32)>,
    }

    impl std::fmt::Debug for FakeSource {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("FakeSource")
        }
    }

    #[derive(Default)]
    pub struct Fake {
        panics_on: Option<usize>,
        panics_on_parts: bool,
        draws: AtomicUsize,
    }

    struct FakeSession<'a>(&'a Fake);

    impl Document for Fake {
        fn session(&self) -> Box<dyn Session + '_> {
            Box::new(FakeSession(self))
        }
    }

    impl Session for FakeSession<'_> {
        fn text(&mut self, _: usize) -> text::PageText {
            text::PageText::default()
        }

        fn cut_out(&mut self, job: Job, _: &[RectI]) -> Bitmap {
            self.draw(job)
        }

        fn draw(&mut self, job: Job) -> Bitmap {
            self.0.draws.fetch_add(1, Ordering::Relaxed);
            assert_ne!(self.0.panics_on, Some(job.page));
            assert!(!(self.0.panics_on_parts && job.area.is_some()));
            let (width, height) = job.size();
            let pixels = vec![255; width as usize * height as usize * 4];
            Bitmap::from_premultiplied(width, height, pixels, true)
        }
    }

    fn pictures(scene: &Scene, out: &mut Vec<ImageDraw>) {
        for p in &scene.primitives {
            match p {
                Primitive::Image(d) => out.push(d.clone()),
                Primitive::Layer(l) => pictures(&l.content, out),
                _ => {}
            }
        }
    }

    fn mount(
        doc: Fake,
        pages: Vec<(f32, f32)>,
        zoom: f32,
    ) -> (Ui, Arc<Fake>, Rc<RefCell<Vec<usize>>>) {
        let doc = Arc::new(doc);
        let errors = Rc::new(RefCell::new(Vec::new()));
        let source = PdfSource::Fake(FakeSource {
            doc: doc.clone(),
            pages,
        });
        let told = errors.clone();
        let mut ui = Ui::new(move || {
            let viewer = pdf_viewer();
            viewer.set_zoom(PdfZoom::Percent(zoom));
            let told = told.clone();
            pdf_view(source)
                .viewer(viewer)
                .w_full()
                .h_full()
                .on_page_error(move |page| told.borrow_mut().push(page))
        });
        ui.handle_event(WindowEvent::Resized {
            size: SizeI::new(420, 300),
            scale: Scale(120),
        });
        for _ in 0..5 {
            ui.update();
            ui.wait_for_images(Duration::from_secs(20));
        }
        (ui, doc, errors)
    }

    fn has_glyphs(scene: &Scene) -> bool {
        scene.primitives.iter().any(|p| match p {
            Primitive::Glyphs(_) => true,
            Primitive::Layer(l) => has_glyphs(&l.content),
            _ => false,
        })
    }

    /// How many more it draws in a while, which should be none when it has come to rest.
    fn draws_after_a_while(ui: &mut Ui, doc: &Fake) -> usize {
        let before = doc.draws.load(Ordering::Relaxed);
        let end = Instant::now() + Duration::from_millis(100);
        while Instant::now() < end {
            ui.update();
            let _ = ui.paint(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        doc.draws.load(Ordering::Relaxed) - before
    }

    #[test]
    fn a_page_that_cannot_be_drawn_says_so_and_is_not_tried_again() {
        let doc = Fake {
            panics_on: Some(0),
            ..Default::default()
        };
        let (mut ui, doc, errors) = mount(doc, vec![(100.0, 100.0); 3], 1.0);
        assert_eq!(*errors.borrow(), [0]);
        assert_eq!(draws_after_a_while(&mut ui, &doc), 0);
        let scene = ui.paint(Instant::now());
        let mut drawn = Vec::new();
        pictures(&scene, &mut drawn);
        // The next page still comes, below where the broken one says so.
        assert!(drawn.iter().all(|d| d.rect.y >= 133), "{drawn:?}");
        assert!(!drawn.is_empty());
        assert!(has_glyphs(&scene));
    }

    #[test]
    fn a_part_that_cannot_be_drawn_leaves_the_whole_picture() {
        let doc = Fake {
            panics_on_parts: true,
            ..Default::default()
        };
        // 1000 points at 400% are too many pixels for a picture of all of it.
        let (mut ui, doc, errors) = mount(doc, vec![(1000.0, 1000.0)], 4.0);
        assert!(errors.borrow().is_empty());
        assert_eq!(draws_after_a_while(&mut ui, &doc), 0);
        let mut drawn = Vec::new();
        pictures(&ui.paint(Instant::now()), &mut drawn);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].bitmap.width(), 4000);
    }
}
