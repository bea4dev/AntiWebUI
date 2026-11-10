//! Filling in forms and marking up a document: what happens on the UI thread. What it comes down to
//! is changing the list of edits, and the document comes back with them from [`crate::edits`]. See
//! "フォームと注釈" in `docs/design/26-pdf.md`.

mod scripts;

use crate::appearance;
use crate::edits::Edits;
use crate::marks::{
    Annot, Field, Marks, PdfAnnotationId, PdfAnnotationKind, PdfEditing, PdfFieldKind,
    PdfFieldValue, Shape, Widget, WidgetAction,
};
use crate::plan::PageTable;
use crate::reading::{BoxKind, PageBox, Reader};
use crate::script::{Mouse, ScriptHost};
use crate::{FieldListener, PdfViewer};
use anti_webui_core::event::{Key, KeyboardEvent, NamedKey, PointerButton, PointerEvent};
use anti_webui_core::geometry::{PointF, RectI};
use anti_webui_core::platform::ImePurpose;
use anti_webui_core::prelude::*;
use anti_webui_core::svg;
use kurbo::BezPath;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

/// How far a press can move and still be a click, in logical pixels.
const CLICK_SLOP: f32 = 4.0;
/// How far from the corner of what is selected counts as its handle, in logical pixels.
const HANDLE: f32 = 7.0;
/// How wide a box of text gets at most before its lines break, in points.
const WIDEST_TEXT: f64 = 320.0;
/// The room a comment takes next to what it is about, in logical pixels.
const COMMENT_WIDTH: f32 = 240.0;
/// How close to each other two points of a line have to be to be one, in points.
const INK_STEP: f64 = 0.5;
/// How much the corners and the frame of what is selected show.
const FRAME: u32 = 0x1a73e8ff;
/// What is in a list that opens, when picked.
const PICKED: u32 = 0x1a73e833;

/// What pressing and dragging on a page does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PdfTool {
    /// Selects text, fills in fields, and picks annotations.
    #[default]
    Select,
    /// Text that gets selected gets marked.
    Highlight,
    Underline,
    StrikeOut,
    /// Dragging draws a line.
    Ink,
    /// A click starts typing there.
    FreeText,
    /// A click puts a note there.
    Note,
}

/// A value of a field that someone changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfFieldChange {
    pub name: String,
    pub value: PdfFieldValue,
}

/// What is taking text on top of a page.
#[derive(Clone)]
struct Active {
    /// To tell this one from the next.
    number: u64,
    kind: ActiveKind,
    /// What is typed so far.
    value: Signal<String>,
    /// Something happened to it, so that it doesn't happen again when losing focus.
    done: Rc<Cell<bool>>,
    /// The text as the scripts of the document last saw it.
    typed: Rc<RefCell<String>>,
}

#[derive(Clone, Copy, PartialEq)]
enum ActiveKind {
    Field {
        field: usize,
        page: usize,
        widget: usize,
    },
    /// A list of what can be picked for a field.
    Options {
        field: usize,
        page: usize,
        widget: usize,
    },
    FreeText {
        page: usize,
        /// Where it goes, in the points of the page, with what it was if there was something.
        area: [f32; 4],
        id: Option<PdfAnnotationId>,
    },
    Comment {
        id: PdfAnnotationId,
    },
}

/// What a press on a page is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pressed {
    Widget {
        page: usize,
        widget: usize,
    },
    /// Moving what is selected, or its size with a corner: 0 to 3, clockwise from the top left.
    Annot {
        id: PdfAnnotationId,
        from: (f32, f32),
        rect: [f32; 4],
        corner: Option<usize>,
        moved: bool,
    },
    Ink,
    Place {
        page: usize,
        x: f32,
        y: f32,
    },
}

/// A line being drawn: its page, and where it went in the points of that page.
type InkLine = (usize, Vec<(f64, f64)>);

/// See the top of this file. A [`crate::PdfView`] has one.
pub(crate) struct Editor {
    viewer: PdfViewer,
    reader: Rc<Reader>,
    table: Memo<Rc<PageTable>>,
    on_change: Option<FieldListener>,
    tint: Option<Color>,

    active: Signal<Option<Active>>,
    next_number: Cell<u64>,
    /// What has been committed and stays up until the page has it too.
    lingering: Signal<Option<usize>>,
    pressed: Cell<Option<Pressed>>,
    /// Something that marks text was pressed, which picks it if the press stays where it is.
    on_text: Cell<Option<(PdfAnnotationId, PointF)>>,
    /// Where what is selected is while it is being moved, in the points of the page.
    moving: Signal<Option<[f32; 4]>>,
    /// The line being drawn, on its page, in the points of the page.
    ink: Signal<Option<InkLine>>,

    /// The scripts of the document, once it turns out to have some.
    scripts: RefCell<Option<ScriptHost>>,
    /// What the scripts were last told the edits are.
    told: RefCell<Arc<Edits>>,
    /// The widget the pointer is over: its page and which of the page's widgets.
    hovered: Cell<Option<(usize, usize)>>,
    /// What the scripts get for the name of the file.
    file_name: RefCell<String>,
    /// The page that was being read when the scripts started.
    start_page: Cell<usize>,
    /// A field a script wants to type in, which waits for what is being committed to be done.
    focus_after: Cell<Option<Option<usize>>>,
    /// The way back from the scripts, made with the view.
    script_out: RefCell<Option<anti_webui_core::channel::UiSender<crate::script::Message>>>,
}

impl Editor {
    /// `stale` has the pages whose pictures are of an older version, and `shown` the edits that the
    /// document has as it shows now.
    pub fn new(
        viewer: PdfViewer,
        reader: Rc<Reader>,
        table: Memo<Rc<PageTable>>,
        on_change: Option<FieldListener>,
        tint: Option<Color>,
        stale: Signal<HashSet<usize>>,
        shown: Signal<Arc<Edits>>,
    ) -> Rc<Self> {
        let this = Rc::new(Self {
            viewer,
            reader,
            table,
            on_change,
            tint,
            active: signal(None),
            next_number: Cell::new(0),
            lingering: signal(None),
            pressed: Cell::new(None),
            on_text: Cell::new(None),
            moving: signal(None),
            ink: signal(None),
            scripts: RefCell::new(None),
            told: RefCell::new(Arc::default()),
            hovered: Cell::new(None),
            file_name: RefCell::new(String::new()),
            start_page: Cell::new(0),
            focus_after: Cell::new(None),
            script_out: RefCell::new(None),
        });
        // What was committed goes when the page shows it too.
        let (active, lingering) = (this.active, this.lingering);
        effect(move || {
            let Some(page) = lingering.get() else {
                return;
            };
            let there = Arc::ptr_eq(&viewer.edits.get(), &shown.get());
            if there && !stale.with(|s| s.contains(&page)) {
                lingering.set(None);
                active.set(None);
            }
        });
        // Going to another tool leaves whatever was being done.
        let weak = Rc::downgrade(&this);
        effect(move || {
            if viewer.tool.get() == PdfTool::Select {
                return;
            }
            viewer.selected.set_if_changed(None);
            if let Some(this) = weak.upgrade() {
                untrack(|| this.commit());
            }
        });
        this
    }

    fn marks(&self) -> Arc<Marks> {
        self.viewer.marks.get_untracked()
    }

    fn can_edit(&self) -> bool {
        self.marks().editing == PdfEditing::Yes
    }

    fn start(&self, kind: ActiveKind, value: String) {
        self.commit();
        let number = self.next_number.get() + 1;
        self.next_number.set(number);
        self.lingering.set(None);
        self.active.set(Some(Active {
            number,
            kind,
            typed: Rc::new(RefCell::new(value.clone())),
            value: signal(value),
            done: Rc::new(Cell::new(false)),
        }));
    }

    /// Makes what was typed part of the document.
    pub fn commit(&self) {
        let Some(active) = self.active.get_untracked() else {
            return;
        };
        if active.done.replace(true) {
            return;
        }
        self.focus_after.set(Some(None));
        let text = active.value.get_untracked();
        let mut linger = None;
        match active.kind {
            ActiveKind::Field {
                field,
                page,
                widget,
            } => {
                let value = match self.marks().fields.get(field).map(|f| f.info.kind) {
                    Some(PdfFieldKind::Combo { .. }) => PdfFieldValue::Choice(
                        Some(text).filter(|t| !t.is_empty()).into_iter().collect(),
                    ),
                    _ => PdfFieldValue::Text(text),
                };
                if self.set_field_by_user(field, value) {
                    linger = Some(page);
                }
                self.mouse(page, widget, Mouse::Blur, None);
            }
            ActiveKind::Options { .. } => {}
            ActiveKind::FreeText { page, area, id } => {
                if self.place_text(page, area, id, text) {
                    linger = Some(page);
                }
            }
            ActiveKind::Comment { id } => self.set_comment(id, text),
        }
        match linger {
            Some(page) => self.lingering.set(Some(page)),
            None => self.active.set(None),
        }
        if let Some(Some(field)) = self.focus_after.take() {
            self.focus_field(field);
        }
    }

    /// Goes back to how it was, without what was typed.
    fn cancel(&self) {
        if let Some(active) = self.active.get_untracked() {
            active.done.set(true);
            self.active.set(None);
            self.viewer.focus();
            if let ActiveKind::Field { page, widget, .. } = active.kind {
                self.mouse(page, widget, Mouse::Blur, None);
            }
        }
    }

    /// Sets what is in a field because someone said so. It says whether anything changed.
    fn set_field_by_user(&self, field: usize, value: PdfFieldValue) -> bool {
        let marks = self.marks();
        let Some(info) = marks.fields.get(field) else {
            return false;
        };
        if self.viewer.field_read_only_at(field) {
            return false;
        }
        if let Some(changed) = self.commit_with_scripts(field, value.clone()) {
            return changed;
        }
        if !self.viewer.set_field_at(field, value.clone()) {
            return false;
        }
        if let Some(f) = &self.on_change {
            f(&PdfFieldChange {
                name: info.info.name.clone(),
                value,
            });
        }
        true
    }

    fn set_comment(&self, id: PdfAnnotationId, text: String) {
        let Some(mut annot) = self.viewer.annot(id) else {
            return;
        };
        if annot.contents != text {
            annot.contents = text;
            self.viewer.change_annot(annot);
        }
    }

    /// Puts typed text on a page. It says whether anything changed.
    fn place_text(
        &self,
        page: usize,
        area: [f32; 4],
        id: Option<PdfAnnotationId>,
        text: String,
    ) -> bool {
        let marks = self.marks();
        let Some(shown) = marks.pages.get(page) else {
            return false;
        };
        if let Some(id) = id {
            if let Some(mut annot) = self.viewer.annot(id)
                && annot.contents != text
            {
                annot.contents = text;
                self.viewer.change_annot(annot);
                return true;
            }
            return false;
        }
        if text.trim().is_empty() {
            return false;
        }
        let size = self.viewer.text_size.get_untracked() as f64;
        let (w, h) = appearance::rough_size(&text, size, WIDEST_TEXT);
        let (x, y) = (area[0] as f64, area[1] as f64);
        let rect = shown.box_to_document([x as f32, y as f32, (x + w) as f32, (y + h) as f32]);
        self.viewer.add_annot(
            page,
            PdfAnnotationKind::FreeText,
            rect,
            self.viewer.color_for(PdfTool::FreeText),
            Shape::FreeText(size),
            text,
        );
        true
    }

    /// Marks text, with the boxes on each page that it takes.
    pub fn mark(&self, kind: PdfAnnotationKind, pages: Vec<(usize, Vec<[f32; 4]>)>) {
        let marks = self.marks();
        let tool = match kind {
            PdfAnnotationKind::Underline => PdfTool::Underline,
            PdfAnnotationKind::StrikeOut => PdfTool::StrikeOut,
            _ => PdfTool::Highlight,
        };
        let color = self.viewer.color_for(tool);
        self.viewer.batch_edits(|viewer| {
            for (page, boxes) in pages {
                let Some(shown) = marks.pages.get(page).filter(|_| !boxes.is_empty()) else {
                    continue;
                };
                let quads: Vec<[f64; 8]> = boxes
                    .iter()
                    .map(|b| {
                        let at = |x: f32, y: f32| shown.to_document((x as f64, y as f64));
                        let (tl, tr, bl, br) = (
                            at(b[0], b[1]),
                            at(b[2], b[1]),
                            at(b[0], b[3]),
                            at(b[2], b[3]),
                        );
                        [tl.0, tl.1, tr.0, tr.1, bl.0, bl.1, br.0, br.1]
                    })
                    .collect();
                let mut rect = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                for quad in &quads {
                    for point in quad.chunks_exact(2) {
                        rect[0] = rect[0].min(point[0]);
                        rect[1] = rect[1].min(point[1]);
                        rect[2] = rect[2].max(point[0]);
                        rect[3] = rect[3].max(point[1]);
                    }
                }
                viewer.add_annot(page, kind, rect, color, Shape::Quads(quads), String::new());
            }
        });
        self.viewer.clear_selection();
    }

    fn widget_at(&self, page: usize, x: f32, y: f32) -> Option<usize> {
        let marks = self.marks();
        let shown = marks.pages.get(page)?;
        (0..shown.widgets.len()).rev().find(|&i| {
            let b = shown.box_to_points(shown.widgets[i].rect);
            !self.viewer.widget_hidden(&marks, page, i)
                && x >= b[0]
                && x <= b[2]
                && y >= b[1]
                && y <= b[3]
        })
    }

    fn annot_at(&self, page: usize, x: f32, y: f32) -> Option<Annot> {
        let marks = self.marks();
        let shown = marks.pages.get(page)?;
        self.viewer.annots_on(page).into_iter().rev().find(|a| {
            let b = shown.box_to_points(a.rect);
            x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]
        })
    }

    /// The corner of what is selected at a point, if the point is that close to one.
    fn corner_at(&self, annot: &Annot, x: f32, y: f32, scale: f32) -> Option<usize> {
        if annot.kind.is_on_text() || annot.kind == PdfAnnotationKind::Note {
            return None;
        }
        let marks = self.marks();
        let b = marks.pages.get(annot.page)?.box_to_points(annot.rect);
        let reach = HANDLE * scale;
        [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])]
            .iter()
            .position(|c| (c.0 - x).abs() <= reach && (c.1 - y).abs() <= reach)
    }

    /// Takes a press, and says whether it did: otherwise it is for selecting text.
    pub fn pointer_down(self: &Rc<Self>, e: &PointerEvent) -> bool {
        self.on_text.set(None);
        if e.button != Some(PointerButton::Primary)
            || self.reader.scroller.is_over_scrollbar(e.position)
            || !self.can_edit()
        {
            return false;
        }
        // Losing focus would do it, but what comes next has to see it done.
        self.commit();
        let Some((page, x, y)) = self.reader.on_page(e.local) else {
            return false;
        };
        match self.viewer.tool.get_untracked() {
            PdfTool::Ink => {
                let at = (x as f64, y as f64);
                self.ink.set(Some((page, vec![at])));
                self.pressed.set(Some(Pressed::Ink));
                return true;
            }
            PdfTool::FreeText | PdfTool::Note => {
                self.pressed.set(Some(Pressed::Place { page, x, y }));
                return true;
            }
            PdfTool::Highlight | PdfTool::Underline | PdfTool::StrikeOut => return false,
            PdfTool::Select => {}
        }
        if let Some(annot) = self
            .viewer
            .selected
            .get_untracked()
            .and_then(|id| self.viewer.annot(id))
            .filter(|a| a.page == page)
        {
            let units = self.points_per_logical(page);
            if let Some(corner) = self.corner_at(&annot, x, y, units) {
                self.press_annot(&annot, (x, y), Some(corner));
                return true;
            }
        }
        if let Some(widget) = self.widget_at(page, x, y) {
            let marks = self.marks();
            let w = &marks.pages[page].widgets[widget];
            let field = &marks.fields[w.field];
            let scripted =
                self.has_scripts() && (w.scripts.down.is_some() || w.scripts.up.is_some());
            let does_something = scripted
                || (!self.viewer.field_read_only_at(w.field)
                    && (!matches!(
                        field.info.kind,
                        PdfFieldKind::Push | PdfFieldKind::Signature
                    ) || w.action != WidgetAction::None));
            if does_something {
                self.viewer.selected.set_if_changed(None);
                self.pressed.set(Some(Pressed::Widget { page, widget }));
                self.mouse(page, widget, Mouse::Down, Some(e));
                return true;
            }
        }
        if let Some(annot) = self.annot_at(page, x, y) {
            if annot.kind.is_on_text() {
                if e.click_count == 2 {
                    self.viewer.selected.set_if_changed(Some(annot.id));
                    self.start(ActiveKind::Comment { id: annot.id }, annot.contents.clone());
                    return true;
                }
                self.on_text.set(Some((annot.id, e.position)));
                return false;
            }
            self.viewer.selected.set_if_changed(Some(annot.id));
            if e.click_count == 2 || annot.kind == PdfAnnotationKind::Note {
                let kind = if annot.kind == PdfAnnotationKind::FreeText {
                    ActiveKind::FreeText {
                        page,
                        area: self.marks().pages[page].box_to_points(annot.rect),
                        id: Some(annot.id),
                    }
                } else {
                    ActiveKind::Comment { id: annot.id }
                };
                self.start(kind, annot.contents.clone());
            }
            self.press_annot(&annot, (x, y), None);
            return true;
        }
        self.viewer.selected.set_if_changed(None);
        false
    }

    fn press_annot(&self, annot: &Annot, from: (f32, f32), corner: Option<usize>) {
        let marks = self.marks();
        let Some(shown) = marks.pages.get(annot.page) else {
            return;
        };
        self.pressed.set(Some(Pressed::Annot {
            id: annot.id,
            from,
            rect: shown.box_to_points(annot.rect),
            corner,
            moved: false,
        }));
    }

    /// How many points of a page a logical pixel is.
    fn points_per_logical(&self, page: usize) -> f32 {
        let table = untrack(|| self.table.get());
        let (width, _) = table.size(page);
        let points = self.reader.shown_size(page).0;
        (points as f32 * window_scale().as_f32()) / width.max(1) as f32
    }

    /// Follows the pointer, and says whether a press here is what it is for.
    pub fn pointer_moved(&self, e: &PointerEvent) -> bool {
        let Some(pressed) = self.pressed.get() else {
            return false;
        };
        let Some((page, x, y)) = self.reader.on_page(e.local) else {
            return true;
        };
        match pressed {
            Pressed::Ink => {
                let at = (x as f64, y as f64);
                self.ink.update(|ink| {
                    if let Some((on, line)) = ink
                        && *on == page
                        && line.last().is_none_or(|last| {
                            (last.0 - at.0).abs() >= INK_STEP || (last.1 - at.1).abs() >= INK_STEP
                        })
                    {
                        line.push(at);
                    }
                });
            }
            Pressed::Annot {
                id,
                from,
                rect,
                corner,
                moved,
            } => {
                let (dx, dy) = (x - from.0, y - from.1);
                let slop = CLICK_SLOP * self.points_per_logical(page);
                if !moved && dx.abs() < slop && dy.abs() < slop {
                    return true;
                }
                self.pressed.set(Some(Pressed::Annot {
                    id,
                    from,
                    rect,
                    corner,
                    moved: true,
                }));
                let Some(annot) = self.viewer.annot(id).filter(|a| a.page == page) else {
                    return true;
                };
                if annot.kind.is_on_text() {
                    return true;
                }
                let size = self.reader.size_in_points(page);
                self.moving.set(Some(dragged(
                    rect,
                    corner,
                    dx,
                    dy,
                    (size.0 as f32, size.1 as f32),
                )));
            }
            Pressed::Widget { .. } | Pressed::Place { .. } => {}
        }
        true
    }

    /// Whether a press is doing something here, rather than selecting text.
    pub fn is_pressed(&self) -> bool {
        self.pressed.get().is_some()
    }

    /// Ends a press. It says whether the press was taken.
    pub fn pointer_up(self: &Rc<Self>, e: &PointerEvent) -> bool {
        let Some(pressed) = self.pressed.take() else {
            if let Some((id, at)) = self.on_text.take()
                && at.distance(e.position) <= window_scale().pxf(CLICK_SLOP)
            {
                self.viewer.selected.set_if_changed(Some(id));
            }
            self.mark_with_tool();
            return false;
        };
        let at = self.reader.on_page(e.local);
        match pressed {
            Pressed::Widget { page, widget } => {
                if let Some((on, x, y)) = at
                    && on == page
                    && self.widget_at(page, x, y) == Some(widget)
                {
                    self.use_widget(page, widget);
                    self.mouse(page, widget, Mouse::Up, Some(e));
                }
            }
            Pressed::Annot { id, .. } => {
                let moved_to = self.moving.get_untracked();
                self.moving.set(None);
                if let (Some(to), Some(annot)) = (moved_to, self.viewer.annot(id)) {
                    let marks = self.marks();
                    if let Some(shown) = marks.pages.get(annot.page) {
                        let rect = shown.box_to_document(to);
                        self.viewer.change_annot(annot.moved_to(rect));
                    }
                }
            }
            Pressed::Ink => {
                if let Some((page, line)) = self.ink.get_untracked() {
                    self.ink.set(None);
                    self.add_ink(page, &line);
                }
            }
            Pressed::Place { page, x, y } => self.place(page, x, y),
        }
        true
    }

    /// Marks what was just selected, when that is what the tool is for.
    fn mark_with_tool(&self) {
        let kind = match self.viewer.tool.get_untracked() {
            PdfTool::Highlight => PdfAnnotationKind::Highlight,
            PdfTool::Underline => PdfAnnotationKind::Underline,
            PdfTool::StrikeOut => PdfAnnotationKind::StrikeOut,
            _ => return,
        };
        if self.viewer.selection.get_untracked().is_some() {
            self.viewer.mark_selection(kind);
        }
    }

    fn add_ink(&self, page: usize, line: &[(f64, f64)]) {
        let marks = self.marks();
        let Some(shown) = marks.pages.get(page) else {
            return;
        };
        let width = self.viewer.ink_width.get_untracked() as f64;
        let line: Vec<(f64, f64)> = line.iter().map(|p| shown.to_document(*p)).collect();
        let mut rect = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for p in &line {
            rect[0] = rect[0].min(p.0 - width);
            rect[1] = rect[1].min(p.1 - width);
            rect[2] = rect[2].max(p.0 + width);
            rect[3] = rect[3].max(p.1 + width);
        }
        self.viewer.add_annot(
            page,
            PdfAnnotationKind::Ink,
            rect,
            self.viewer.color_for(PdfTool::Ink),
            Shape::Ink(vec![line], width),
            String::new(),
        );
    }

    /// A click with a tool that puts something somewhere.
    fn place(&self, page: usize, x: f32, y: f32) {
        let marks = self.marks();
        let Some(shown) = marks.pages.get(page) else {
            return;
        };
        match self.viewer.tool.get_untracked() {
            PdfTool::FreeText => {
                let size = self.viewer.text_size.get_untracked();
                let area = [x, y, x + WIDEST_TEXT as f32, y + size * 4.0];
                self.viewer.tool.set(PdfTool::Select);
                self.start(
                    ActiveKind::FreeText {
                        page,
                        area,
                        id: None,
                    },
                    String::new(),
                );
            }
            PdfTool::Note => {
                let half = (appearance::NOTE_SIZE / 2.0) as f32;
                let rect = shown.box_to_document([x - half, y - half, x + half, y + half]);
                let id = self.viewer.add_annot(
                    page,
                    PdfAnnotationKind::Note,
                    rect,
                    self.viewer.color_for(PdfTool::Note),
                    Shape::Note,
                    String::new(),
                );
                self.viewer.tool.set(PdfTool::Select);
                self.viewer.selected.set(Some(id));
                self.start(ActiveKind::Comment { id }, String::new());
            }
            _ => {}
        }
    }

    /// What a click on a widget does.
    fn use_widget(&self, page: usize, widget: usize) {
        let marks = self.marks();
        let w = &marks.pages[page].widgets[widget];
        let field = &marks.fields[w.field];
        if w.action == WidgetAction::Reset {
            self.viewer.reset_fields();
            return;
        }
        if self.viewer.field_read_only_at(w.field) {
            return;
        }
        let current = self.viewer.field_value_at(w.field);
        match field.info.kind {
            PdfFieldKind::Check | PdfFieldKind::Radio => {
                let Some(state) = w.on_state.clone() else {
                    return;
                };
                let is_on = current == PdfFieldValue::State(Some(state.clone()));
                let next = if is_on {
                    if field.info.kind == PdfFieldKind::Radio && !field.can_turn_off() {
                        return;
                    }
                    PdfFieldValue::State(None)
                } else {
                    PdfFieldValue::State(Some(state))
                };
                self.set_field_by_user(w.field, next);
            }
            PdfFieldKind::Text => {
                let PdfFieldValue::Text(text) = current else {
                    return;
                };
                self.start(
                    ActiveKind::Field {
                        field: w.field,
                        page,
                        widget,
                    },
                    text,
                );
                self.mouse(page, widget, Mouse::Focus, None);
            }
            PdfFieldKind::Combo { editable } => {
                let PdfFieldValue::Choice(picked) = current else {
                    return;
                };
                let text = picked.first().cloned().unwrap_or_default();
                if editable {
                    self.start(
                        ActiveKind::Field {
                            field: w.field,
                            page,
                            widget,
                        },
                        text,
                    );
                    self.mouse(page, widget, Mouse::Focus, None);
                } else {
                    self.start(
                        ActiveKind::Options {
                            field: w.field,
                            page,
                            widget,
                        },
                        text,
                    );
                }
            }
            PdfFieldKind::List { .. } => {
                self.start(
                    ActiveKind::Options {
                        field: w.field,
                        page,
                        widget,
                    },
                    String::new(),
                );
            }
            PdfFieldKind::Push | PdfFieldKind::Signature => {}
        }
    }

    /// How the pointer looks at a place, if something here says.
    pub fn cursor_at(&self, e: &PointerEvent) -> Option<CursorIcon> {
        if !self.can_edit() {
            return None;
        }
        let (page, x, y) = self.reader.on_page(e.local)?;
        match self.viewer.tool.get_untracked() {
            PdfTool::Ink | PdfTool::FreeText | PdfTool::Note => return Some(CursorIcon::Crosshair),
            PdfTool::Highlight | PdfTool::Underline | PdfTool::StrikeOut => {
                return Some(CursorIcon::Text);
            }
            PdfTool::Select => {}
        }
        if let Some(annot) = self
            .viewer
            .selected
            .get_untracked()
            .and_then(|id| self.viewer.annot(id))
            .filter(|a| a.page == page)
            && self
                .corner_at(&annot, x, y, self.points_per_logical(page))
                .is_some()
        {
            return Some(CursorIcon::Move);
        }
        if let Some(widget) = self.widget_at(page, x, y) {
            let marks = self.marks();
            let w = &marks.pages[page].widgets[widget];
            let field = &marks.fields[w.field];
            if self.has_scripts() && w.scripts.up.is_some() {
                return Some(CursorIcon::Pointer);
            }
            if self.viewer.field_read_only_at(w.field) {
                return None;
            }
            return match field.info.kind {
                PdfFieldKind::Text => Some(CursorIcon::Text),
                PdfFieldKind::Signature => None,
                PdfFieldKind::Push if w.action == WidgetAction::None => None,
                _ => Some(CursorIcon::Pointer),
            };
        }
        let annot = self.annot_at(page, x, y)?;
        if annot.kind.is_on_text() {
            return None;
        }
        Some(if Some(annot.id) == self.viewer.selected.get_untracked() {
            CursorIcon::Move
        } else {
            CursorIcon::Pointer
        })
    }

    /// The keys of editing. It says whether one was taken.
    pub fn key(&self, e: &KeyboardEvent) -> bool {
        if self.active.with_untracked(Option::is_some) {
            // Otherwise the keys for typing would also scroll and select the whole document.
            return true;
        }
        if e.modifiers.primary() {
            match &e.key {
                Key::Character(c) if c.eq_ignore_ascii_case("z") && e.modifiers.shift => {
                    self.viewer.redo();
                }
                Key::Character(c) if c.eq_ignore_ascii_case("z") => self.viewer.undo(),
                Key::Character(c) if c.eq_ignore_ascii_case("y") => self.viewer.redo(),
                _ => return false,
            }
            e.stop_propagation();
            e.prevent_default();
            return true;
        }
        match &e.key {
            Key::Named(NamedKey::Delete | NamedKey::Backspace)
                if self.viewer.selected.get_untracked().is_some() =>
            {
                self.viewer.delete_annotation();
            }
            Key::Named(NamedKey::Escape) if self.viewer.selected.get_untracked().is_some() => {
                self.viewer.selected.set(None);
            }
            Key::Named(NamedKey::Escape) if self.viewer.tool.get_untracked() != PdfTool::Select => {
                self.viewer.tool.set(PdfTool::Select);
            }
            _ => return false,
        }
        e.stop_propagation();
        true
    }

    /// What shows on top of a page to say that there are fields.
    pub fn field_boxes(&self, page: usize) -> Vec<PageBox> {
        if self.tint.is_none() {
            return Vec::new();
        }
        let marks = self.viewer.marks.get();
        let table = self.table.get();
        let _ = self.viewer.edits.get();
        if marks.editing != PdfEditing::Yes || page >= table.len() {
            return Vec::new();
        }
        let Some(shown) = marks.pages.get(page) else {
            return Vec::new();
        };
        shown
            .widgets
            .iter()
            .enumerate()
            .filter(|(i, w)| {
                let field = &marks.fields[w.field];
                !self.viewer.widget_hidden(&marks, page, *i)
                    && !self.viewer.field_read_only_at(w.field)
                    && !matches!(
                        field.info.kind,
                        PdfFieldKind::Push | PdfFieldKind::Signature
                    )
            })
            .map(|(_, w)| {
                let r = self
                    .reader
                    .to_pixels(&table, page, &shown.box_to_points(w.rect));
                PageBox {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                    kind: BoxKind::Field,
                }
            })
            .collect()
    }

    pub fn tint(&self) -> Color {
        self.tint.unwrap_or(Color::TRANSPARENT)
    }

    /// What goes over all of the pages: what takes text, and what is selected.
    pub fn layer(self: &Rc<Self>) -> Div {
        let this = self.clone();
        let inputs = for_each(
            {
                let this = this.clone();
                move || this.active.get().into_iter().collect::<Vec<_>>()
            },
            |active: &Active| active.number,
            {
                let this = this.clone();
                move |active: Active| this.active_view(active)
            },
        );
        let frames = for_each(
            {
                let viewer = self.viewer;
                move || viewer.selected.get().into_iter().collect::<Vec<_>>()
            },
            |id: &PdfAnnotationId| *id,
            {
                let this = this.clone();
                move |id: PdfAnnotationId| this.frame_view(id)
            },
        );
        div()
            .absolute()
            .left(Length::Px(0.0))
            .top(Length::Px(0.0))
            .child(frames)
            .child(self.ink_view())
            .child(inputs)
    }

    fn ink_view(self: &Rc<Self>) -> View {
        let this = self.clone();
        show(
            {
                let ink = self.ink;
                move || ink.with(Option::is_some)
            },
            move || {
                let this = this.clone();
                let box_of = {
                    let this = this.clone();
                    move |s: Style| {
                        let table = this.table.get();
                        let Some(page) = this.ink.with(|i| i.as_ref().map(|i| i.0)) else {
                            return s;
                        };
                        if page >= table.len() {
                            return s;
                        }
                        let (w, h) = table.size(page);
                        s.left(Length::Device(table.left(page)))
                            .top(Length::Device(table.top(page)))
                            .w(Length::Device(w))
                            .h(Length::Device(h))
                    }
                };
                let line = {
                    let this = this.clone();
                    move || {
                        let mut path = BezPath::new();
                        let table = this.table.get();
                        this.ink.with(|ink| {
                            let Some((page, line)) = ink else {
                                return;
                            };
                            let size = this.reader.size_in_points(*page);
                            for (i, p) in line.iter().enumerate() {
                                let at = table.turn.point(*p, size);
                                if i == 0 {
                                    path.move_to(at);
                                } else {
                                    path.line_to(at);
                                }
                            }
                        });
                        path
                    }
                };
                let color = this.viewer.color_for(PdfTool::Ink);
                let width = this.viewer.ink_width.get_untracked();
                let view_box = this
                    .reader
                    .shown_size(this.ink.with_untracked(|i| i.as_ref().map_or(0, |i| i.0)));
                svg::svg()
                    .absolute()
                    .pointer_events_none()
                    .dyn_style(box_of)
                    .view_box(0.0, 0.0, view_box.0 as f32, view_box.1 as f32)
                    .child(
                        svg::path(line)
                            .fill_none()
                            .stroke(color_of(color))
                            .stroke_width(width)
                            .line_cap(LineCap::Round)
                            .line_join(LineJoin::Round),
                    )
            },
        )
        .into_view()
    }

    /// Where a box of points of a page is over all of the pages, in pixels.
    fn over_pages(&self, table: &PageTable, page: usize, area: &[f32; 4]) -> RectI {
        self.reader
            .to_pixels(table, page, area)
            .translate(table.left(page), table.top(page))
    }

    fn frame_view(self: &Rc<Self>, id: PdfAnnotationId) -> impl IntoView + use<> {
        let this = self.clone();
        let viewer = self.viewer;
        let moving = self.moving;
        let table = self.table;
        let area = {
            let this = this.clone();
            move || {
                let annot = viewer.edits.with(|_| viewer.annot(id))?;
                let marks = viewer.marks.get();
                let shown = marks.pages.get(annot.page)?;
                let points = moving
                    .get()
                    .unwrap_or_else(|| shown.box_to_points(annot.rect));
                let table = table.get();
                (annot.page < table.len()).then(|| this.over_pages(&table, annot.page, &points))
            }
        };
        let area = Rc::new(area);
        let resizable = viewer
            .annot(id)
            .is_some_and(|a| !a.kind.is_on_text() && a.kind != PdfAnnotationKind::Note);
        let corner = {
            let area = area.clone();
            move |index: usize| {
                let area = area.clone();
                div()
                    .absolute()
                    .pointer_events_none()
                    .bg(Color::WHITE)
                    .border(1.0)
                    .border_color(Color::from_rgba(FRAME))
                    .dyn_style(move |s| {
                        let Some(a) = area() else {
                            return s.hidden();
                        };
                        let size = window_scale().px(8.0);
                        let (x, y) = match index {
                            0 => (0, 0),
                            1 => (a.width, 0),
                            2 => (a.width, a.height),
                            _ => (0, a.height),
                        };
                        s.left(Length::Device(x - size / 2))
                            .top(Length::Device(y - size / 2))
                            .w(Length::Device(size))
                            .h(Length::Device(size))
                    })
            }
        };
        let mut frame = div()
            .absolute()
            .pointer_events_none()
            .border(2.0)
            .border_color(Color::from_rgba(FRAME))
            .dyn_style({
                let area = area.clone();
                move |s| {
                    let Some(a) = area() else {
                        return s.hidden();
                    };
                    s.left(Length::Device(a.x))
                        .top(Length::Device(a.y))
                        .w(Length::Device(a.width))
                        .h(Length::Device(a.height))
                }
            });
        if resizable {
            for index in 0..4 {
                frame = frame.child(corner(index));
            }
        }
        frame
    }

    fn active_view(self: &Rc<Self>, active: Active) -> View {
        match active.kind {
            ActiveKind::Field {
                field,
                page,
                widget,
            } => self.field_view(active, field, page, widget),
            ActiveKind::Options {
                field,
                page,
                widget,
            } => self.options_view(field, page, widget),
            ActiveKind::FreeText { page, area, id } => self.free_text_view(active, page, area, id),
            ActiveKind::Comment { id } => self.comment_view(active, id),
        }
    }

    /// What every input has: losing focus commits, and Escape doesn't.
    fn input_events(self: &Rc<Self>, input: TextInput, active: &Active) -> (TextInput, NodeRef) {
        let node = node_ref();
        let input = input
            .node_ref(node)
            .value(active.value)
            .on_pointer_down(|e| e.stop_propagation())
            .on_pointer_up(|e| e.stop_propagation())
            .on_focus_change({
                let this = self.clone();
                let number = active.number;
                move |focused| {
                    let current = this.active.with_untracked(|a| a.as_ref().map(|a| a.number));
                    if !focused && current == Some(number) {
                        this.commit();
                    }
                }
            });
        (input, node)
    }

    fn field_view(
        self: &Rc<Self>,
        active: Active,
        field: usize,
        page: usize,
        widget: usize,
    ) -> View {
        let marks = self.marks();
        let info = marks.fields[field].clone();
        let w = marks.pages[page].widgets[widget].clone();
        let look = appearance::parse_default_look(&info.da);
        let input = if info.multiline() {
            text_area()
        } else {
            text_input()
        };
        let input = if info.password() {
            input.purpose(ImePurpose::Password)
        } else {
            input
        };
        let (input, node) = self.input_events(input, &active);
        let this = self.clone();
        let (table, value) = (self.table, active.value);
        let typed = active.typed.clone();
        let max_len = info.max_len;
        let points = marks.pages[page].box_to_points(w.rect);
        let view = input
            .absolute()
            .bg(Color::WHITE)
            .text_color(Color::BLACK)
            .border(1.0)
            .border_color(Color::from_rgba(FRAME))
            .dyn_style({
                let this = this.clone();
                let look = look.clone();
                move |s| {
                    let table = table.get();
                    if page >= table.len() {
                        return s;
                    }
                    let area = this.over_pages(&table, page, &points);
                    let per_point =
                        table.size(page).1 as f32 / this.reader.shown_size(page).1 as f32;
                    let size = if look.size > 0.0 {
                        look.size as f32
                    } else if info.multiline() {
                        12.0
                    } else {
                        ((points[3] - points[1]) / 1.35).max(4.0)
                    };
                    let logical = window_scale().to_logical(size * per_point);
                    s.left(Length::Device(area.x))
                        .top(Length::Device(area.y))
                        .w(Length::Device(area.width))
                        .h(Length::Device(area.height))
                        .text_size(logical)
                }
            })
            .on_input({
                let this = this.clone();
                move |text| {
                    if let Some(most) = max_len
                        && text.chars().count() > most as usize
                    {
                        value.set(text.chars().take(most as usize).collect());
                    }
                    this.keystroke(field, &typed, value, node);
                }
            })
            .on_submit({
                let this = this.clone();
                move |_| {
                    this.commit();
                    this.viewer.focus();
                }
            })
            .on_key_down({
                let this = this.clone();
                move |e| match &e.key {
                    Key::Named(NamedKey::Escape) => {
                        this.cancel();
                        e.stop_propagation();
                    }
                    Key::Named(NamedKey::Tab) => {
                        this.commit();
                        this.go_to_next_field(field, page, widget, e.modifiers.shift);
                        e.prevent_default();
                        e.stop_propagation();
                    }
                    _ => {}
                }
            })
            .into_view();
        node.focus();
        node.select_end();
        view
    }

    /// Goes on to the next field that takes text, in the order of the pages.
    fn go_to_next_field(self: &Rc<Self>, field: usize, page: usize, widget: usize, back: bool) {
        let marks = self.marks();
        let all: Vec<(usize, usize)> = marks
            .pages
            .iter()
            .enumerate()
            .flat_map(|(p, shown)| (0..shown.widgets.len()).map(move |w| (p, w)))
            .collect();
        let Some(at) = all.iter().position(|w| *w == (page, widget)) else {
            return;
        };
        let count = all.len();
        let offsets = 1..count;
        let takes_text = |p: usize, i: usize| {
            let w: &Widget = &marks.pages[p].widgets[i];
            let f: &Field = &marks.fields[w.field];
            !self.viewer.widget_hidden(&marks, p, i)
                && !self.viewer.field_read_only_at(w.field)
                && w.field != field
                && matches!(f.info.kind, PdfFieldKind::Text | PdfFieldKind::Combo { .. })
        };
        let next = offsets
            .map(|step| {
                if back {
                    all[(at + count - step) % count]
                } else {
                    all[(at + step) % count]
                }
            })
            .find(|(p, w)| takes_text(*p, *w));
        let Some((p, w)) = next else {
            self.viewer.focus();
            return;
        };
        let table = untrack(|| self.table.get());
        let area = self.reader.to_pixels(
            &table,
            p,
            &marks.pages[p].box_to_points(marks.pages[p].widgets[w].rect),
        );
        self.reader.bring_into_view(&table, p, area);
        self.use_widget(p, w);
    }

    fn options_view(self: &Rc<Self>, field: usize, page: usize, widget: usize) -> View {
        let marks = self.marks();
        let info = marks.fields[field].clone();
        let w = marks.pages[page].widgets[widget].clone();
        let points = marks.pages[page].box_to_points(w.rect);
        let this = self.clone();
        let table = self.table;
        let multiple = matches!(info.info.kind, PdfFieldKind::List { multiple: true });
        let picked = move |this: &Rc<Self>| match this.viewer.field_value_at(field) {
            PdfFieldValue::Choice(picked) => picked,
            _ => Vec::new(),
        };
        let options: Vec<(usize, (String, String))> =
            info.info.options.iter().cloned().enumerate().collect();
        let list = div()
            .absolute()
            .bg(Color::WHITE)
            .text_color(Color::BLACK)
            .border(1.0)
            .border_color(Color::from_rgba(FRAME))
            .shadow_md()
            .overflow_y_scroll()
            .max_h(240.0)
            .select_none()
            .on_pointer_down(|e| e.stop_propagation())
            .on_pointer_up(|e| e.stop_propagation())
            .dyn_style({
                let this = this.clone();
                move |s| {
                    let table = table.get();
                    if page >= table.len() {
                        return s;
                    }
                    let area = this.over_pages(&table, page, &points);
                    s.left(Length::Device(area.x))
                        .top(Length::Device(area.y + area.height))
                        .min_w(Length::Device(area.width))
                }
            })
            .children(options.into_iter().map(|(_, (value, shown))| {
                let this = this.clone();
                let value_for_style = value.clone();
                div()
                    .px(8.0)
                    .py(3.0)
                    .cursor(CursorIcon::Pointer)
                    .dyn_style({
                        let this = this.clone();
                        move |s| {
                            this.viewer.edits.with(|_| {});
                            if picked(&this).contains(&value_for_style) {
                                s.bg(Color::from_rgba(PICKED))
                            } else {
                                s
                            }
                        }
                    })
                    .on_click(move |e| {
                        let mut now = picked(&this);
                        if multiple && e.modifiers.primary() {
                            match now.iter().position(|p| *p == value) {
                                Some(at) => {
                                    now.remove(at);
                                }
                                None => now.push(value.clone()),
                            }
                            this.set_field_by_user(field, PdfFieldValue::Choice(now));
                        } else {
                            this.set_field_by_user(
                                field,
                                PdfFieldValue::Choice(vec![value.clone()]),
                            );
                            this.active.set(None);
                            this.viewer.focus();
                        }
                    })
                    .child(text(shown.clone()))
            }));
        list.into_view()
    }

    fn free_text_view(
        self: &Rc<Self>,
        active: Active,
        page: usize,
        area: [f32; 4],
        id: Option<PdfAnnotationId>,
    ) -> View {
        let size = match id.and_then(|id| self.viewer.annot(id)).map(|a| a.shape) {
            Some(shape) => match *shape {
                Shape::FreeText(size) => size as f32,
                _ => 12.0,
            },
            None => self.viewer.text_size.get_untracked(),
        };
        let color = self.viewer.color_for(PdfTool::FreeText);
        let (input, node) = self.input_events(text_area(), &active);
        let this = self.clone();
        let table = self.table;
        let view = input
            .absolute()
            .text_color(color_of(color))
            .bg(Color::from_rgba(0xffffffcc))
            .border(1.0)
            .border_color(Color::from_rgba(FRAME))
            .dyn_style({
                let this = this.clone();
                move |s| {
                    let table = table.get();
                    if page >= table.len() {
                        return s;
                    }
                    let box_ = this.over_pages(&table, page, &area);
                    let per_point =
                        table.size(page).1 as f32 / this.reader.shown_size(page).1 as f32;
                    s.left(Length::Device(box_.x))
                        .top(Length::Device(box_.y))
                        .w(Length::Device(box_.width))
                        .min_h(Length::Device(box_.height))
                        .text_size(window_scale().to_logical(size * per_point))
                }
            })
            .on_key_down(move |e| {
                if matches!(e.key, Key::Named(NamedKey::Escape)) {
                    this.cancel();
                    e.stop_propagation();
                }
            })
            .into_view();
        node.focus();
        node.select_end();
        view
    }

    fn comment_view(self: &Rc<Self>, active: Active, id: PdfAnnotationId) -> View {
        let (input, node) = self.input_events(text_area().rows(4), &active);
        let this = self.clone();
        let (table, viewer) = (self.table, self.viewer);
        let view = input
            .absolute()
            .w(COMMENT_WIDTH)
            .bg(Color::from_rgb(0xfff9c4))
            .text_color(Color::BLACK)
            .border(1.0)
            .border_color(Color::from_rgb(0xc0a000))
            .shadow_md()
            .dyn_style({
                let this = this.clone();
                move |s| {
                    let table = table.get();
                    let Some(annot) = viewer.annot(id).filter(|a| a.page < table.len()) else {
                        return s;
                    };
                    let marks = viewer.marks.get_untracked();
                    let points = marks.pages[annot.page].box_to_points(annot.rect);
                    let area = this.over_pages(&table, annot.page, &points);
                    s.left(Length::Device(area.x + area.width + window_scale().px(6.0)))
                        .top(Length::Device(area.y))
                }
            })
            .on_key_down(move |e| {
                if matches!(e.key, Key::Named(NamedKey::Escape)) {
                    this.cancel();
                    e.stop_propagation();
                }
            })
            .into_view();
        node.focus();
        node.select_end();
        view
    }
}

fn color_of([r, g, b]: [f32; 3]) -> Color {
    let part = |c: f32| (c * 255.0).round() as u32;
    Color::from_rgb((part(r) << 16) | (part(g) << 8) | part(b))
}

/// Where a box goes when it is moved, or a corner of it is. It stays on a page of a size, as it
/// couldn't be picked again otherwise, and keeps a size that something can show in.
fn dragged(rect: [f32; 4], corner: Option<usize>, dx: f32, dy: f32, page: (f32, f32)) -> [f32; 4] {
    const SMALLEST: f32 = 8.0;
    let [x0, y0, x1, y1] = rect;
    match corner {
        None => {
            let dx = dx.clamp(-x0.max(0.0), (page.0 - x1).max(0.0));
            let dy = dy.clamp(-y0.max(0.0), (page.1 - y1).max(0.0));
            [x0 + dx, y0 + dy, x1 + dx, y1 + dy]
        }
        Some(0) => [
            (x0 + dx).clamp(0.0, x1 - SMALLEST),
            (y0 + dy).clamp(0.0, y1 - SMALLEST),
            x1,
            y1,
        ],
        Some(1) => [
            x0,
            (y0 + dy).clamp(0.0, y1 - SMALLEST),
            (x1 + dx).clamp(x0 + SMALLEST, page.0),
            y1,
        ],
        Some(2) => [
            x0,
            y0,
            (x1 + dx).clamp(x0 + SMALLEST, page.0),
            (y1 + dy).clamp(y0 + SMALLEST, page.1),
        ],
        Some(_) => [
            (x0 + dx).clamp(0.0, x1 - SMALLEST),
            y0,
            x1,
            (y1 + dy).clamp(y0 + SMALLEST, page.1),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_moves_with_its_corner_and_stays_on_the_page() {
        let r = [10.0, 10.0, 50.0, 50.0];
        let page = (100.0, 100.0);
        assert_eq!(dragged(r, None, 5.0, 6.0, page), [15.0, 16.0, 55.0, 56.0]);
        assert_eq!(
            dragged(r, Some(2), 5.0, 6.0, page),
            [10.0, 10.0, 55.0, 56.0]
        );
        // It doesn't get smaller than can be seen, nor go off the page.
        assert_eq!(
            dragged(r, Some(0), 100.0, 0.0, page),
            [42.0, 10.0, 50.0, 50.0]
        );
        assert_eq!(
            dragged(r, None, -30.0, 300.0, page),
            [0.0, 60.0, 40.0, 100.0]
        );
    }
}
