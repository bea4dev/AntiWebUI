//! Text edited line by line: `text_editor`. Each line is a node of its own, shaped only near the
//! screen, and how the lines look (styles, parts hidden or shown as nodes, lines shown as one node)
//! comes from outside, as CodeMirror's decorations do. See "M3" in `docs/design/28-markdown.md`.

use super::edit::{
    Motion, Preedit, TextCallback, floor_char_boundary, next_grapheme, prev_grapheme,
};
use super::rich::{BoxData, RichData, SpanData, VerticalAlign};
use super::selection::Granularity;
use super::{Affinity, TextLayout};
use crate::event::ImeUpdate;
use crate::geometry::{PointF, RectI};
use crate::platform::{ImePurpose, ImeState};
use crate::reactive::{Owner, Signal, batch, create_child_owner, untrack};
use crate::tree::{Document, Node, NodeId, NodeKind, TextData, with_doc};
use crate::view::{IntoView, View, create_node};
use anti_webui_style::{Length, Style, Styled, TextAlign};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;

/// A node that shows part of the text, or some lines of it: a picture, a check box, a table.
#[derive(Clone)]
pub struct Widget {
    /// Says it is the same as one with the same key, which is then not made again.
    pub key: u64,
    pub build: Rc<dyn Fn() -> View>,
}

impl Widget {
    pub fn new<V: IntoView>(key: u64, build: impl Fn() -> V + 'static) -> Self {
        Self {
            key,
            build: Rc::new(move || build().into_view()),
        }
    }
}

impl PartialEq for Widget {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl std::fmt::Debug for Widget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Widget({})", self.key)
    }
}

/// How one line looks. Ranges are bytes of the line, without its break.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineLook {
    /// The style of the line's box: room on the left, a background, a line on the left, the size
    /// of the letters.
    pub style: Option<Rc<Style>>,
    /// Parts with a style of their own, in order and not overlapping.
    pub marks: Vec<(Range<usize>, Rc<Style>)>,
    /// Parts not shown (`None`) or shown as a node, in order and not overlapping.
    pub replace: Vec<(Range<usize>, Option<Widget>)>,
    /// Parts raised or lowered in their line, as CSS `vertical-align` does: a mark that is in one
    /// of these is.
    pub raised: Vec<(Range<usize>, VerticalAlign)>,
}

impl LineLook {
    fn has_widgets(&self) -> bool {
        self.replace.iter().any(|(_, w)| w.is_some())
    }
}

/// What a row of a `text_editor` is.
#[derive(Clone, Debug)]
pub enum Row {
    /// One line.
    Line(Rc<LineLook>),
    /// Some lines shown as one node instead.
    Block {
        lines: usize,
        style: Option<Rc<Style>>,
        widget: Widget,
    },
}

impl Row {
    fn lines(&self) -> usize {
        match self {
            Row::Line(_) => 1,
            Row::Block { lines, .. } => *lines,
        }
    }

    fn same_look(&self, other: &Row) -> bool {
        match (self, other) {
            (Row::Line(a), Row::Line(b)) => Rc::ptr_eq(a, b) || a == b,
            (
                Row::Block {
                    lines: la,
                    style: sa,
                    widget: wa,
                },
                Row::Block {
                    lines: lb,
                    style: sb,
                    widget: wb,
                },
            ) => la == lb && sa == sb && wa == wb,
            _ => false,
        }
    }

    fn has_widgets(&self) -> bool {
        match self {
            Row::Line(l) => l.has_widgets(),
            Row::Block { .. } => true,
        }
    }
}

/// What `decorate` is given: the text, its lines and the selection.
pub struct DecorateContext<'a> {
    pub text: &'a str,
    /// Changes whenever the text does.
    pub version: u64,
    starts: &'a [usize],
    pub anchor: usize,
    pub focus: usize,
    pub focused: bool,
}

impl DecorateContext<'_> {
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The bytes of line `i`, without its break.
    pub fn line(&self, i: usize) -> Range<usize> {
        line_range(self.text, self.starts, i)
    }

    /// The line that holds the byte at `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1)
    }

    /// The selected bytes, from the first to the last.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }
}

/// Gives the rows of all of the text, from the first line to the last.
pub type Decorate = dyn Fn(&DecorateContext) -> Vec<Row>;

/// Gives how some lines look, for the lines that get rows: one for each line of the range.
pub type DecorateLines = dyn Fn(&DecorateContext, Range<usize>) -> Vec<Rc<LineLook>>;

/// Gives the rows of several lines (folded code), each by its first line, in order. The other
/// lines are rows of their own, which `DecorateLines` decorates.
pub type Blocks = dyn Fn(&DecorateContext) -> Vec<(usize, Row)>;

/// Told of each change to the text.
pub type OnEdit = dyn Fn(&TextEdit);

/// Told of the selection, as its anchor and focus, whenever it moves or the text changes.
pub type OnSelect = dyn Fn(usize, usize);

/// Where the text changed, in bytes and in (line, byte in the line), as tree-sitter's `InputEdit`
/// wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

/// The line numbers on the left of the rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gutter {
    /// The box of a line's number: colors, the room around it, the font.
    pub style: Rc<Style>,
    /// What changes for the line with the caret.
    pub current: Option<Rc<Style>>,
    /// The fewest digits there is room for.
    pub digits: usize,
    /// Room on the left and on the right of the numbers, in logical pixels.
    pub padding: (f32, f32),
}

fn line_range(text: &str, starts: &[usize], i: usize) -> Range<usize> {
    let a = starts[i];
    let b = starts.get(i + 1).map_or(text.len(), |&s| s - 1);
    a..b
}

#[cfg(test)]
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

/// A change to the text: `removed` at `at` became `inserted`.
#[derive(Clone, Debug)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

/// One step of undo, with the selections before and after it.
struct Step {
    changes: Vec<Change>,
    before: (usize, usize),
    after: (usize, usize),
}

const UNDO_STEPS: usize = 1000;

/// What a row of the editor holds, as the editor last made it.
pub(crate) struct RowSlot {
    /// Its first line.
    first: usize,
    /// Where the text of its lines is, with their breaks.
    src: Range<usize>,
    look: Row,
    div: NodeId,
    text: Option<NodeId>,
    gutter: Option<GutterSlot>,
    owner: Option<Owner>,
}

/// The box of a row's line number.
struct GutterSlot {
    div: NodeId,
    text: NodeId,
    number: usize,
    current: bool,
}

/// What the rows were made from, to know when they are out of date.
#[derive(Clone, PartialEq)]
struct ShownKey {
    version: u64,
    anchor: usize,
    focus: usize,
    focused: bool,
    preedit: Option<Preedit>,
    looks: u64,
}

/// Where the pre-edit is in a line's shown text, and the range the IME points at in it.
pub type ShownPreedit = (Range<usize>, Option<(usize, usize)>);

/// How a line's text node is tied to its editor: where its letters come from in the line, and
/// what of the selection, the pre-edit and the caret is on it.
pub struct RowLink {
    pub host: NodeId,
    /// The parts that are shown: where each starts in the shown text and in the line, and how long
    /// it is, in order.
    pieces: Vec<(usize, usize, usize)>,
    /// The length of the line.
    len: usize,
    /// The length of the shown text before the space at its end.
    shown: usize,
    pub selected: Option<Range<usize>>,
    /// The pre-edit in the shown text, and the IME's range in it.
    pub preedit: Option<ShownPreedit>,
    pub caret: Option<usize>,
    /// Where what is dragged over the editor would go, when it is in this line.
    pub drop_caret: Option<usize>,
}

impl RowLink {
    /// Where a byte of the line is in the shown text. A byte that is hidden is where what follows
    /// it is shown.
    pub fn to_display(&self, at: usize) -> usize {
        for &(disp, src, len) in &self.pieces {
            if at < src {
                return disp;
            }
            if at <= src + len {
                return disp + (at - src);
            }
        }
        self.shown
    }

    /// Which byte of the line a place in the shown text is. A place in a node or the pre-edit is
    /// after what is shown before it.
    pub fn to_source(&self, at: usize) -> usize {
        if at >= self.shown {
            return self.len;
        }
        let mut before = 0;
        for &(disp, src, len) in &self.pieces {
            if at < disp {
                return before;
            }
            if at <= disp + len {
                return src + (at - disp);
            }
            before = src + len;
        }
        before
    }
}

/// What a line shows.
struct Display {
    text: String,
    pieces: Vec<(usize, usize, usize)>,
    boxes: Vec<(usize, Widget)>,
    spans: Vec<SpanData>,
    shown: usize,
    preedit: Option<Range<usize>>,
}

/// The ranges of `ranges` that are in `line`, in order, not overlapping and on its letters.
fn tidy<T: Clone>(ranges: &[(Range<usize>, T)], line: &str) -> Vec<(Range<usize>, T)> {
    let mut out: Vec<(Range<usize>, T)> = Vec::with_capacity(ranges.len());
    let mut end = 0;
    for (r, t) in ranges {
        let a = floor_char_boundary(line, r.start.max(end));
        let b = floor_char_boundary(line, r.end).max(a);
        if a < b || (a == b && r.is_empty()) {
            out.push((a..b, t.clone()));
            end = b;
        }
    }
    out
}

fn display(line: &str, look: &LineLook, preedit: Option<(usize, &str)>) -> Display {
    let cuts = tidy(&look.replace, line);
    let mut text = String::with_capacity(line.len() + 1);
    let mut pieces = Vec::new();
    let mut boxes = Vec::new();
    let mut pre_range = None;
    let mut pending = preedit;
    let show = |from: usize, to: usize, text: &mut String, pieces: &mut Vec<_>| {
        if from < to {
            pieces.push((text.len(), from, to - from));
            text.push_str(&line[from..to]);
        }
    };
    let mut at = 0;
    let visible = cuts
        .iter()
        .map(|(r, w)| (r.clone(), w.as_ref()))
        .chain(std::iter::once((line.len()..line.len(), None)));
    for (cut, widget) in visible {
        // What is shown before the cut, with the pre-edit where it goes.
        match pending {
            Some((p, s)) if at <= p && p <= cut.start => {
                show(at, p, &mut text, &mut pieces);
                let start = text.len();
                text.push_str(s);
                pre_range = Some(start..text.len());
                show(p, cut.start, &mut text, &mut pieces);
                pending = None;
            }
            _ => show(at, cut.start, &mut text, &mut pieces),
        }
        if let Some((p, s)) = pending
            && cut.contains(&p)
        {
            let start = text.len();
            text.push_str(s);
            pre_range = Some(start..text.len());
            pending = None;
        }
        if let Some(w) = widget {
            boxes.push((text.len(), w.clone()));
        }
        at = cut.end;
    }
    if let Some((_, s)) = pending {
        let start = text.len();
        text.push_str(s);
        pre_range = Some(start..text.len());
    }
    let shown = text.len();
    text.push(' ');

    let mut spans: Vec<SpanData> = Vec::new();
    for (m, style) in tidy(&look.marks, line) {
        let align = look
            .raised
            .iter()
            .find(|(r, _)| r.start <= m.start && m.end <= r.end)
            .map_or(VerticalAlign::Baseline, |(_, a)| *a);
        let span = |range: Range<usize>, style: &Rc<Style>| SpanData {
            range,
            style: style.clone(),
            hover: None,
            align,
            on_click: None,
        };
        for &(disp, src, len) in &pieces {
            let a = m.start.max(src);
            let b = m.end.min(src + len);
            if a < b {
                spans.push(span(disp + a - src..disp + b - src, &style));
            }
        }
        // What is being written takes the style of what it is written in.
        if let (Some((p, _)), Some(r)) = (preedit, &pre_range)
            && m.start < p
            && p <= m.end
        {
            spans.push(span(r.clone(), &style));
        }
    }
    spans.sort_by_key(|s| s.range.start);
    let mut end = 0;
    spans.retain_mut(|s| {
        s.range.start = s.range.start.max(end);
        end = end.max(s.range.end);
        !s.range.is_empty()
    });
    Display {
        text,
        pieces,
        boxes,
        spans,
        shown,
        preedit: pre_range,
    }
}

/// What a `text_editor` holds.
pub struct EditorData {
    /// Shared with what reads it while the rows are made, so that it isn't copied.
    pub text: Rc<String>,
    /// Where each line starts.
    starts: Rc<Vec<usize>>,
    pub anchor: usize,
    pub focus: usize,
    pub preedit: Option<Preedit>,
    pub read_only: bool,
    pub purpose: ImePurpose,
    /// Changes whenever the text does.
    pub version: u64,
    /// Where up and down go across, in the rows' coordinates.
    goal_x: Option<f32>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_edit: EditKind,
    pub caret_epoch: Instant,
    pub scroll_to_caret: bool,
    pub drag: Option<(Granularity, usize, usize)>,

    pub value: Option<Signal<String>>,
    pub on_change: Option<TextCallback>,
    pub(crate) decorate: Option<Rc<Decorate>>,
    pub(crate) decorate_lines: Option<Rc<DecorateLines>>,
    pub(crate) blocks: Option<Rc<Blocks>>,
    pub(crate) on_edit: Option<Rc<OnEdit>>,
    pub(crate) on_select: Option<Rc<OnSelect>>,
    pub(crate) gutter: Option<Gutter>,
    /// Edits not yet told to `on_edit`.
    edits: Vec<TextEdit>,
    /// The lines that changed since the rows were last made: how many at the start and how many at
    /// the end stayed as they were.
    touched: Option<(usize, usize)>,
    /// Changes when the decorations have to be asked for again.
    pub(crate) looks_version: u64,
    /// What the rows are made under.
    pub(crate) owner: Option<Owner>,
    plain: Rc<LineLook>,

    /// The rows that have nodes, in order. The others are in the gaps between them.
    pub(crate) rows: Vec<RowSlot>,
    /// The boxes that stand for the rows without nodes.
    spacers: Vec<NodeId>,
    /// How the lines are grouped into rows, as of when the rows were last made.
    structure: Structure,
    heights: Heights,
    /// The lines to make rows for, around the view.
    window: Range<usize>,
    /// How far the rows are scrolled across, which the line numbers make up for.
    gutter_x: i32,
    /// How wide the line numbers are.
    gutter_w: i32,
    /// How wide a digit of the line numbers is, once one has been laid out.
    digit_w: f32,
    /// The line the pre-edit was shown in.
    preedit_line: Option<usize>,
    shown_key: Option<ShownKey>,
    /// The text nodes that show part of the selection.
    pub(crate) selected_rows: Vec<NodeId>,
    /// The boxes of the rows shown as one node that are in the selection.
    pub(crate) selected_blocks: Vec<NodeId>,
    /// The text node with the drop caret.
    drop_row: Option<NodeId>,
    /// The text node with the caret, and the box of its row.
    pub(crate) caret_row: Option<(NodeId, NodeId)>,
}

/// What the caller has to do after an edit.
#[derive(Clone, Copy, Debug, Default)]
pub struct EditorOutcome {
    pub changed: bool,
    pub moved: bool,
}

impl EditorOutcome {
    pub(crate) const CHANGED: Self = Self {
        changed: true,
        moved: true,
    };
    pub(crate) const MOVED: Self = Self {
        changed: false,
        moved: true,
    };
}

/// Text put in becomes this: line breaks are `\n`.
fn cleaned(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('\r') {
        s.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        s.into()
    }
}

impl EditorData {
    pub fn new() -> Self {
        Self {
            text: Rc::default(),
            starts: Rc::new(vec![0]),
            anchor: 0,
            focus: 0,
            preedit: None,
            read_only: false,
            purpose: ImePurpose::Normal,
            version: 0,
            goal_x: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: EditKind::Other,
            caret_epoch: Instant::now(),
            scroll_to_caret: false,
            drag: None,
            value: None,
            on_change: None,
            decorate: None,
            decorate_lines: None,
            blocks: None,
            on_edit: None,
            on_select: None,
            gutter: None,
            edits: Vec::new(),
            touched: None,
            looks_version: 0,
            owner: None,
            plain: Rc::default(),
            rows: Vec::new(),
            spacers: Vec::new(),
            structure: Structure::default(),
            heights: Heights::default(),
            window: 0..0,
            gutter_x: 0,
            gutter_w: 0,
            digit_w: 0.0,
            preedit_line: None,
            shown_key: None,
            selected_rows: Vec::new(),
            selected_blocks: Vec::new(),
            drop_row: None,
            caret_row: None,
        }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The line that holds the byte at `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1)
    }

    /// The bytes of line `i`, without its break.
    pub fn line(&self, i: usize) -> Range<usize> {
        line_range(&self.text, &self.starts, i)
    }

    /// The boxes of the rows shown as one node (tables, diagrams) that are in the selection.
    pub fn selected_blocks(&self) -> &[NodeId] {
        &self.selected_blocks
    }

    /// The selection as it is now is where the last step of undo leaves it when it is redone.
    pub(crate) fn keep_selection_after(&mut self) {
        let after = (self.anchor, self.focus);
        if let Some(step) = self.undo.last_mut() {
            step.after = after;
        }
    }

    /// Forgets what can be undone and redone.
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last_edit = EditKind::Other;
    }

    pub fn selection_range(&self) -> (usize, usize) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.focus
    }

    pub fn selected_text(&self) -> &str {
        let (a, b) = self.selection_range();
        &self.text[a..b]
    }

    fn moved(&mut self) {
        self.scroll_to_caret = true;
        self.caret_epoch = Instant::now();
        self.last_edit = EditKind::Other;
    }

    fn clamp(&self, i: usize) -> usize {
        floor_char_boundary(&self.text, i.min(self.text.len()))
    }

    /// Selects from one byte to another, kept inside of the text and on its characters.
    pub fn select(&mut self, anchor: usize, focus: usize) {
        self.anchor = self.clamp(anchor);
        self.focus = self.clamp(focus);
        self.goal_x = None;
        self.moved();
    }

    pub fn select_all(&mut self) {
        self.select(0, self.text.len());
    }

    /// Keeps the table of line starts in step with a change at `at`.
    fn shift_starts(&mut self, at: usize, removed: usize, inserted: &str) {
        let starts = Rc::make_mut(&mut self.starts);
        let lo = starts.partition_point(|&s| s <= at);
        let hi = starts.partition_point(|&s| s <= at + removed);
        let new: Vec<usize> = inserted
            .match_indices('\n')
            .map(|(i, _)| at + i + 1)
            .collect();
        let n = new.len();
        starts.splice(lo..hi, new);
        let delta = inserted.len() as isize - removed as isize;
        for s in &mut starts[lo + n..] {
            *s = (*s as isize + delta) as usize;
        }
    }

    /// (line, byte in the line) of a byte.
    fn point(&self, at: usize) -> (usize, usize) {
        let line = self.line_of(at);
        (line, at - self.starts[line])
    }

    fn apply(&mut self, at: usize, removed: usize, inserted: &str) {
        let (first, last) = (self.line_of(at), self.line_of(at + removed));
        let lines = self.starts.len();
        let (same_start, same_end) = self.touched.unwrap_or((usize::MAX, usize::MAX));
        self.touched = Some((same_start.min(first), same_end.min(lines - 1 - last)));
        let told = self
            .on_edit
            .is_some()
            .then(|| (self.point(at), self.point(at + removed)));
        Rc::make_mut(&mut self.text).replace_range(at..at + removed, inserted);
        self.shift_starts(at, removed, inserted);
        if let Some((start_point, old_end_point)) = told {
            self.edits.push(TextEdit {
                start: at,
                old_end: at + removed,
                new_end: at + inserted.len(),
                start_point,
                old_end_point,
                new_end_point: self.point(at + inserted.len()),
            });
        }
        self.version += 1;
        self.goal_x = None;
        self.scroll_to_caret = true;
        self.caret_epoch = Instant::now();
    }

    fn record(&mut self, kind: EditKind, change: Change, before: (usize, usize)) {
        let after = (self.anchor, self.focus);
        self.redo.clear();
        if kind != EditKind::Other
            && kind == self.last_edit
            && let Some(step) = self.undo.last_mut()
            && let [c] = &mut step.changes[..]
        {
            let joined = match kind {
                EditKind::Insert => {
                    change.removed.is_empty() && change.at == c.at + c.inserted.len() && {
                        c.inserted.push_str(&change.inserted);
                        true
                    }
                }
                EditKind::Delete if change.inserted.is_empty() && c.inserted.is_empty() => {
                    if change.at + change.removed.len() == c.at {
                        c.removed.insert_str(0, &change.removed);
                        c.at = change.at;
                        true
                    } else if change.at == c.at {
                        c.removed.push_str(&change.removed);
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if joined {
                step.after = after;
                return;
            }
        }
        self.undo.push(Step {
            changes: vec![change],
            before,
            after,
        });
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
        }
        self.last_edit = kind;
    }

    /// Replaces `range` with `s` as one step of undo, and puts the caret after it.
    fn edit(&mut self, range: Range<usize>, s: &str, kind: EditKind) {
        let before = (self.anchor, self.focus);
        let removed = self.text[range.clone()].to_owned();
        self.apply(range.start, range.len(), s);
        let p = range.start + s.len();
        self.anchor = p;
        self.focus = p;
        self.record(
            kind,
            Change {
                at: range.start,
                removed,
                inserted: s.to_owned(),
            },
            before,
        );
    }

    /// Replaces a range of the text, as one step of undo. The selection moves with the text.
    pub fn replace(&mut self, range: Range<usize>, s: &str) -> EditorOutcome {
        let s = cleaned(s);
        let a = self.clamp(range.start);
        let b = self.clamp(range.end.max(range.start));
        if self.read_only || (a == b && s.is_empty()) || self.text[a..b] == *s {
            return EditorOutcome::default();
        }
        let before = (self.anchor, self.focus);
        let removed = self.text[a..b].to_owned();
        self.apply(a, b - a, &s);
        let follow = |i: usize| {
            if i >= b {
                i - (b - a) + s.len()
            } else if i > a {
                a + s.len()
            } else {
                i
            }
        };
        self.anchor = follow(before.0);
        self.focus = follow(before.1);
        self.last_edit = EditKind::Other;
        self.record(
            EditKind::Other,
            Change {
                at: a,
                removed,
                inserted: s.into_owned(),
            },
            before,
        );
        EditorOutcome::CHANGED
    }

    /// Takes the whole text from outside (a bound signal), as one step of undo.
    pub fn set_text(&mut self, text: &str) -> EditorOutcome {
        let text = cleaned(text);
        if *self.text == *text {
            return EditorOutcome::default();
        }
        let old = self.text.as_bytes();
        let new = text.as_bytes();
        let mut a = old.iter().zip(new).take_while(|(x, y)| x == y).count();
        let max_b = old.len().min(new.len()) - a;
        let mut b = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(max_b)
            .take_while(|(x, y)| x == y)
            .count();
        while !self.text.is_char_boundary(a) || !text.is_char_boundary(a) {
            a -= 1;
        }
        while !self.text.is_char_boundary(old.len() - b) || !text.is_char_boundary(new.len() - b) {
            b -= 1;
        }
        let read_only = std::mem::replace(&mut self.read_only, false);
        let out = self.replace(a..old.len() - b, &text[a..new.len() - b]);
        self.read_only = read_only;
        out
    }

    /// Replaces the selection with `s`.
    pub fn insert(&mut self, s: &str) -> EditorOutcome {
        if self.read_only {
            return EditorOutcome::default();
        }
        let s = cleaned(s);
        if s.is_empty() && !self.has_selection() {
            return EditorOutcome::default();
        }
        let kind = if s.contains([' ', '\n']) || self.has_selection() {
            EditKind::Other
        } else {
            EditKind::Insert
        };
        let (a, b) = self.selection_range();
        self.edit(a..b, &s, kind);
        EditorOutcome::CHANGED
    }

    pub fn delete(&mut self, forward: bool, word: bool) -> EditorOutcome {
        if self.read_only {
            return EditorOutcome::default();
        }
        let (mut a, mut b) = self.selection_range();
        if a == b {
            match (forward, word) {
                (true, true) => b = self.word_right(b),
                (false, true) => a = self.word_left(a),
                (true, false) => b = next_grapheme(&self.text, b),
                (false, false) => a = prev_grapheme(&self.text, a),
            }
        }
        if a == b {
            return EditorOutcome::default();
        }
        let kind = if word {
            EditKind::Other
        } else {
            EditKind::Delete
        };
        self.edit(a..b, "", kind);
        EditorOutcome::CHANGED
    }

    /// Puts what was dropped at `at`, as one step of undo, and selects it.
    pub fn drop_text(&mut self, at: usize, s: &str) -> EditorOutcome {
        let s = cleaned(s);
        if self.read_only || s.is_empty() {
            return EditorOutcome::default();
        }
        let at = self.clamp(at);
        self.edit(at..at, &s, EditKind::Other);
        self.anchor = at;
        self.focus = at + s.len();
        if let Some(step) = self.undo.last_mut() {
            step.after = (self.anchor, self.focus);
        }
        EditorOutcome::CHANGED
    }

    /// Moves the selection to `at`, as one step of undo, and keeps it selected.
    pub fn move_selection(&mut self, at: usize) -> EditorOutcome {
        let (a, b) = self.selection_range();
        if self.read_only || a == b || (a..=b).contains(&at) {
            return EditorOutcome::default();
        }
        let before = (self.anchor, self.focus);
        let moved = self.text[a..b].to_owned();
        let at = self.clamp(at);
        self.apply(a, b - a, "");
        let to = if at > b { at - moved.len() } else { at };
        self.apply(to, 0, &moved);
        self.anchor = to;
        self.focus = to + moved.len();
        self.redo.clear();
        self.undo.push(Step {
            changes: vec![
                Change {
                    at: a,
                    removed: moved.clone(),
                    inserted: String::new(),
                },
                Change {
                    at: to,
                    removed: String::new(),
                    inserted: moved,
                },
            ],
            before,
            after: (self.anchor, self.focus),
        });
        self.last_edit = EditKind::Other;
        EditorOutcome::CHANGED
    }

    /// Puts room at the start of the selected lines, or takes it away. With nothing selected it
    /// puts the room at the caret.
    pub fn indent(&mut self, back: bool) -> EditorOutcome {
        const ROOM: &str = "    ";
        if self.read_only {
            return EditorOutcome::default();
        }
        let (a, b) = self.selection_range();
        if !back && a == b {
            return self.insert(ROOM);
        }
        let first = self.line_of(a);
        let mut last = self.line_of(b);
        if last > first && self.starts[last] == b {
            last -= 1;
        }
        let before = (self.anchor, self.focus);
        let (mut anchor, mut focus) = before;
        let mut changes = Vec::new();
        for l in (first..=last).rev() {
            let s = self.starts[l];
            let change = if back {
                let rest = &self.text.as_bytes()[s..];
                let n = match rest.first() {
                    Some(b'\t') => 1,
                    _ => rest
                        .iter()
                        .take(ROOM.len())
                        .take_while(|&&c| c == b' ')
                        .count(),
                };
                if n == 0 {
                    continue;
                }
                Change {
                    at: s,
                    removed: self.text[s..s + n].to_owned(),
                    inserted: String::new(),
                }
            } else {
                Change {
                    at: s,
                    removed: String::new(),
                    inserted: ROOM.to_owned(),
                }
            };
            self.apply(s, change.removed.len(), &change.inserted);
            let follow = |i: usize| {
                if i < s {
                    i
                } else if change.removed.is_empty() {
                    i + change.inserted.len()
                } else {
                    i - (i - s).min(change.removed.len())
                }
            };
            anchor = follow(anchor);
            focus = follow(focus);
            changes.push(change);
        }
        if changes.is_empty() {
            return EditorOutcome::default();
        }
        self.anchor = anchor;
        self.focus = focus;
        self.redo.clear();
        self.undo.push(Step {
            changes,
            before,
            after: (anchor, focus),
        });
        self.last_edit = EditKind::Other;
        EditorOutcome::CHANGED
    }

    fn step(&mut self, back: bool) -> EditorOutcome {
        let from = if back { &mut self.undo } else { &mut self.redo };
        let Some(step) = from.pop() else {
            return EditorOutcome::default();
        };
        if back {
            for c in step.changes.iter().rev() {
                self.apply(c.at, c.inserted.len(), &c.removed);
            }
            (self.anchor, self.focus) = step.before;
            self.redo.push(step);
        } else {
            for c in &step.changes {
                self.apply(c.at, c.removed.len(), &c.inserted);
            }
            (self.anchor, self.focus) = step.after;
            self.undo.push(step);
        }
        self.last_edit = EditKind::Other;
        EditorOutcome::CHANGED
    }

    pub fn undo(&mut self) -> EditorOutcome {
        self.step(true)
    }

    pub fn redo(&mut self) -> EditorOutcome {
        self.step(false)
    }

    /// The start of the word before `i`, in its line; at the start of a line, the end of the one
    /// before.
    pub fn word_left(&self, i: usize) -> usize {
        let start = self.starts[self.line_of(i)];
        if i == start {
            return i.saturating_sub(1);
        }
        self.text[start..i]
            .split_word_bound_indices()
            .rev()
            .find(|(_, w)| w.chars().any(char::is_alphanumeric))
            .map_or(start, |(j, _)| start + j)
    }

    /// The end of the word after `i`, in its line; at the end of a line, the start of the next.
    pub fn word_right(&self, i: usize) -> usize {
        let end = self.line(self.line_of(i)).end;
        if i == end {
            return (i + 1).min(self.text.len());
        }
        self.text[i..end]
            .split_word_bound_indices()
            .find(|(_, w)| w.chars().any(char::is_alphanumeric))
            .map_or(end, |(j, w)| i + j + w.len())
    }

    /// The word at `i`, or what else is there between word boundaries.
    pub fn word_at(&self, i: usize) -> (usize, usize) {
        let line = self.line(self.line_of(i));
        let mut found = (i, i);
        for (j, w) in self.text[line.clone()].split_word_bound_indices() {
            let (a, b) = (line.start + j, line.start + j + w.len());
            if a <= i && i < b {
                return (a, b);
            }
            if b == i {
                found = (a, b);
            }
        }
        found
    }

    /// Moves along the text, without looking at how it is laid out.
    fn move_in_text(&mut self, motion: Motion, extend: bool) -> bool {
        let f = self.focus;
        let to = match motion {
            Motion::Left if !extend && self.has_selection() => self.selection_range().0,
            Motion::Right if !extend && self.has_selection() => self.selection_range().1,
            Motion::Left => prev_grapheme(&self.text, f),
            Motion::Right => next_grapheme(&self.text, f),
            Motion::WordLeft => self.word_left(f),
            Motion::WordRight => self.word_right(f),
            Motion::DocStart => 0,
            Motion::DocEnd => self.text.len(),
            _ => return false,
        };
        self.put_caret(to, extend);
        true
    }

    fn put_caret(&mut self, to: usize, extend: bool) {
        self.focus = self.clamp(to);
        if !extend {
            self.anchor = self.focus;
        }
        self.moved();
    }

    /// Applies an input-method update in the order text-input-v3 asks for.
    pub fn apply_ime(&mut self, update: &ImeUpdate) -> EditorOutcome {
        let mut out = EditorOutcome::default();
        if self.preedit.take().is_some() {
            out.moved = true;
        }
        if let Some((before, after)) = update.delete_surrounding {
            let (a, b) = self.selection_range();
            let start = self.clamp(a.saturating_sub(before));
            let end = self.clamp(b + after);
            if start < a || end > b {
                let sel = (self.anchor, self.focus);
                self.edit(b..end, "", EditKind::Other);
                self.edit(start..a, "", EditKind::Other);
                self.anchor = start;
                self.focus = start + (sel.1.max(sel.0) - sel.0.min(sel.1));
                out = EditorOutcome::CHANGED;
            }
        }
        if let Some(commit) = &update.commit {
            out.changed |= self.insert(commit).changed;
            out.moved = true;
        }
        if let Some((text, cursor)) = &update.preedit
            && !text.is_empty()
        {
            if self.has_selection() && !self.read_only {
                let (a, b) = self.selection_range();
                self.edit(a..b, "", EditKind::Other);
                out.changed = true;
            }
            self.preedit = Some(Preedit {
                text: text.clone(),
                cursor: *cursor,
            });
            out.moved = true;
        }
        if out.moved {
            self.moved();
        }
        out
    }

    /// The state told to the input method.
    pub fn ime_state(&self, caret: RectI) -> ImeState {
        const MAX: usize = 4000;
        let (lo, hi) = self.selection_range();
        let mut start = lo.saturating_sub(MAX / 2);
        let mut end = (hi + MAX / 2).min(self.text.len());
        if end - start > MAX {
            end = start + MAX;
        }
        start = floor_char_boundary(&self.text, start);
        end = floor_char_boundary(&self.text, end);
        let clamp = |i: usize| i.clamp(start, end) - start;
        ImeState {
            surrounding: self.text[start..end].to_owned(),
            cursor: clamp(self.focus),
            anchor: clamp(self.anchor),
            cursor_rect: caret,
            multiline: true,
            purpose: self.purpose,
        }
    }
}

impl Default for EditorData {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------------------
// Rows

/// At most this many rows get nodes, however small they are.
const MOST_ROWS: usize = 4000;

/// How the lines are grouped into rows.
#[derive(Default)]
struct Structure {
    lines: usize,
    /// Every row, when `decorate` gives them all.
    all: Option<Vec<Row>>,
    /// The rows of several lines (or of one line shown as a node): their first line, the row, and
    /// how many lines before it are in such rows without being their first.
    blocks: Vec<(usize, Row, usize)>,
}

impl Structure {
    fn new(lines: usize, all: Option<Vec<Row>>, blocks: Vec<(usize, Row)>) -> Self {
        let mut kept = Vec::with_capacity(blocks.len());
        let (mut lost, mut end) = (0, 0);
        for (first, mut row) in blocks {
            let Row::Block { lines: n, .. } = &mut row else {
                continue;
            };
            // In order, not overlapping, and in the text.
            if first < end || first >= lines {
                continue;
            }
            *n = (*n).clamp(1, lines - first);
            let n = *n;
            end = first + n;
            kept.push((first, row, lost));
            lost += n - 1;
        }
        Self {
            lines,
            all,
            blocks: kept,
        }
    }

    fn from_all(lines: usize, rows: Vec<Row>) -> Self {
        let mut first = 0;
        let mut blocks = Vec::new();
        for r in &rows {
            if matches!(r, Row::Block { .. }) {
                blocks.push((first, r.clone()));
            }
            first += r.lines();
        }
        Self::new(lines, Some(rows), blocks)
    }

    /// The lines the first `k` blocks have without being their first.
    fn lost_by(&self, k: usize) -> usize {
        match k.checked_sub(1) {
            Some(i) => self.blocks[i].2 + self.blocks[i].1.lines() - 1,
            None => 0,
        }
    }

    fn len(&self) -> usize {
        self.lines - self.lost_by(self.blocks.len())
    }

    fn row_of_line(&self, line: usize) -> usize {
        let k = self.blocks.partition_point(|(f, ..)| *f <= line);
        if let Some((f, r, lost)) = k.checked_sub(1).map(|i| &self.blocks[i])
            && line < f + r.lines()
        {
            return f - lost;
        }
        line - self.lost_by(k)
    }

    fn first_line(&self, row: usize) -> usize {
        let k = self.blocks.partition_point(|(f, _, lost)| f - lost <= row);
        if let Some((f, _, lost)) = k.checked_sub(1).map(|i| &self.blocks[i])
            && f - lost == row
        {
            return *f;
        }
        row + self.lost_by(k)
    }

    fn block(&self, row: usize) -> Option<&Row> {
        let k = self.blocks.partition_point(|(f, _, lost)| f - lost < row);
        self.blocks
            .get(k)
            .filter(|(f, _, lost)| f - lost == row)
            .map(|(_, r, _)| r)
    }

    fn lines_of(&self, row: usize) -> usize {
        self.block(row).map_or(1, Row::lines)
    }

    /// Whether the blocks are where they were, with as many lines.
    fn same_blocks(&self, other: &Structure) -> bool {
        self.blocks.len() == other.blocks.len()
            && self
                .blocks
                .iter()
                .zip(&other.blocks)
                .all(|(a, b)| a.0 == b.0 && a.1.lines() == b.1.lines())
    }
}

/// How tall the rows are: what they measured when they were last laid out, or an estimate.
#[derive(Default)]
struct Heights {
    /// By line: what the row that starts there measured, or -1.
    by_line: Vec<i32>,
    /// How far down each row starts, right up to `clean`, and then where the last one ends.
    tops: Vec<i64>,
    clean: usize,
    /// What estimates come from, learned from the rows laid out.
    line: i32,
    char_w: f32,
    width: i32,
    wraps: bool,
}

impl Heights {
    fn estimate(&self, lines: usize, len: usize) -> i32 {
        let line = self.line.max(1);
        if lines > 1 {
            return line * lines as i32;
        }
        if self.wraps && self.char_w > 0.0 && self.width > 0 {
            let per_line = (self.width as f32 / self.char_w).max(1.0);
            line * (len as f32 / per_line).ceil().max(1.0) as i32
        } else {
            line
        }
    }

    fn of(&self, s: &Structure, starts: &[usize], text_len: usize, row: usize) -> i32 {
        let first = s.first_line(row);
        match self.by_line.get(first) {
            Some(&h) if h >= 0 => h,
            _ => {
                let end = starts.get(first + 1).map_or(text_len, |&e| e - 1);
                self.estimate(s.lines_of(row), end - starts[first])
            }
        }
    }

    fn dirty(&mut self, row: usize) {
        self.clean = self.clean.min(row);
    }

    fn sum_up(&mut self, s: &Structure, starts: &[usize], text_len: usize) {
        let n = s.len();
        self.tops.resize(n + 1, 0);
        self.clean = self.clean.min(n);
        let mut at = self.tops[self.clean];
        if s.blocks.is_empty() && !self.wraps {
            // A row for each line, each a line tall unless it measured otherwise: code.
            let line = self.line.max(1);
            for (row, &h) in self.by_line[self.clean..n].iter().enumerate() {
                at += if h >= 0 { h } else { line } as i64;
                self.tops[self.clean + row + 1] = at;
            }
        } else {
            for row in self.clean..n {
                at += self.of(s, starts, text_len, row) as i64;
                self.tops[row + 1] = at;
            }
        }
        self.clean = n;
    }

    /// The row at `y`, after `sum_up`.
    fn row_at(&self, y: i64) -> usize {
        let n = self.tops.len() - 1;
        self.tops[1..]
            .partition_point(|&b| b <= y)
            .min(n.saturating_sub(1))
    }
}

/// The style of a row's box.
/// `gutter` is the width of the line numbers, in logical pixels that come back as the same device
/// pixels.
fn row_style(style: Option<&Rc<Style>>, gutter: Option<f32>) -> Style {
    let s = style.map(|s| (**s).clone()).unwrap_or_default();
    match gutter {
        Some(w) => s.relative().pl(w),
        None => s,
    }
}

/// The style of the box of a line's number.
fn gutter_style(g: &Gutter, x: i32, w: i32) -> Style {
    (*g.style)
        .clone()
        .pl(g.padding.0)
        .pr(g.padding.1)
        .absolute()
        .left(Length::Device(x))
        .top(0.0)
        .bottom(0.0)
        .w(Length::Device(w))
        .text_align(TextAlign::Right)
        .select_none()
}

fn link_of(host: NodeId, line: &str, d: &Display, preedit: Option<&Preedit>) -> RowLink {
    RowLink {
        host,
        pieces: d.pieces.clone(),
        len: line.len(),
        shown: d.shown,
        selected: None,
        preedit: d
            .preedit
            .clone()
            .map(|r| (r, preedit.and_then(|p| p.cursor))),
        caret: None,
        drop_caret: None,
    }
}

fn text_node(text: String) -> Node {
    Node::new(NodeKind::Text(TextData {
        text,
        version: 0,
        layout: TextLayout::default(),
        zoom: 1.0,
        in_svg: false,
        rich: None,
        row: None,
    }))
}

/// What a row's line number looks like when it is made.
struct GutterLook<'a> {
    gutter: &'a Gutter,
    number: usize,
    current: bool,
    x: i32,
    w: i32,
    /// `w` in logical pixels.
    logical: f32,
}

/// Makes a row's nodes, under the current owner.
fn build_row(
    host: NodeId,
    row: &Row,
    line: &str,
    preedit: Option<(usize, &Preedit)>,
    gutter: Option<&GutterLook>,
) -> RowSlot {
    let gutter_w = gutter.map(|g| g.logical);
    let (div, text, children) = match row {
        Row::Line(look) => {
            let d = display(line, look, preedit.map(|(p, pe)| (p, pe.text.as_str())));
            let children: Vec<NodeId> = d.boxes.iter().map(|(_, w)| (w.build)().0).collect();
            let boxes = d
                .boxes
                .iter()
                .zip(&children)
                .map(|((at, _), &node)| BoxData {
                    at: *at,
                    node,
                    alt: String::new(),
                    align: VerticalAlign::Baseline,
                    size: Default::default(),
                    baseline: None,
                })
                .collect();
            let link = link_of(host, line, &d, preedit.map(|p| p.1));
            let mut node = text_node(d.text);
            if let NodeKind::Text(t) = &mut node.kind {
                t.rich = Some(Box::new(RichData {
                    spans: d.spans,
                    boxes,
                    resolved: Vec::new(),
                    hovered: None,
                }));
                t.row = Some(Box::new(link));
            }
            let text = create_node(node);
            with_doc(|doc| doc.set_children(text, children));
            let mut div = Node::new(NodeKind::Element);
            div.style.base = row_style(look.style.as_ref(), gutter_w);
            (create_node(div), Some(text), vec![text])
        }
        Row::Block { style, widget, .. } => {
            let w = (widget.build)();
            let mut div = Node::new(NodeKind::Element);
            div.style.base = row_style(style.as_ref(), gutter_w);
            (create_node(div), None, vec![w.0])
        }
    };
    let mut children = children;
    // Lines shown as one node have no numbers: folded lines are out of sight.
    let gutter = gutter.filter(|_| text.is_some()).map(|g| {
        let number = create_node(text_node((g.number + 1).to_string()));
        let mut node = Node::new(NodeKind::Element);
        node.style.base = gutter_style(g.gutter, g.x, g.w);
        node.style.dynamic = current_style(g.gutter, g.current);
        let boxed = create_node(node);
        with_doc(|doc| doc.set_children(boxed, vec![number]));
        children.push(boxed);
        GutterSlot {
            div: boxed,
            text: number,
            number: g.number,
            current: g.current,
        }
    });
    with_doc(|doc| doc.set_children(div, children));
    RowSlot {
        first: 0,
        src: 0..0,
        look: row.clone(),
        div,
        text,
        gutter,
        owner: None,
    }
}

/// Shows another line in a row's nodes without making them again.
#[allow(clippy::too_many_arguments)]
fn refill(
    doc: &mut Document,
    host: NodeId,
    slot: &RowSlot,
    look: &Rc<LineLook>,
    line: &str,
    preedit: Option<(usize, &Preedit)>,
    text_changed: bool,
    gutter_w: Option<f32>,
) {
    let Row::Line(old) = &slot.look else {
        return;
    };
    if old.style != look.style
        && let Some(n) = doc.nodes.get_mut(slot.div)
    {
        n.style.base = row_style(look.style.as_ref(), gutter_w);
        doc.mark_style_of(slot.div);
    }
    // Only the box looks different: the letters stay as they are.
    let same_letters = !text_changed
        && preedit.is_none()
        && old.marks == look.marks
        && old.replace == look.replace
        && old.raised == look.raised;
    let Some(text) = slot.text else {
        return;
    };
    if same_letters {
        return;
    }
    let d = display(line, look, preedit.map(|(p, pe)| (p, pe.text.as_str())));
    let link = link_of(host, line, &d, preedit.map(|p| p.1));
    if let Some(n) = doc.nodes.get_mut(text)
        && let NodeKind::Text(t) = &mut n.kind
    {
        t.text = d.text;
        t.version += 1;
        t.rich = Some(Box::new(RichData {
            spans: d.spans,
            boxes: Vec::new(),
            resolved: Vec::new(),
            hovered: None,
        }));
        t.row = Some(Box::new(link));
    }
    doc.mark_style_of(text);
    doc.invalidate_layout(text);
}

/// Shows another number in a row's line number, or the same one in another style.
fn renumber(doc: &mut Document, g: &mut GutterSlot, look: &GutterLook) {
    if g.number != look.number
        && let Some(n) = doc.nodes.get_mut(g.text)
        && let NodeKind::Text(t) = &mut n.kind
    {
        t.text = (look.number + 1).to_string();
        t.version += 1;
        g.number = look.number;
        doc.invalidate_layout(g.text);
    }
    if g.current != look.current {
        g.current = look.current;
        if let Some(n) = doc.nodes.get_mut(g.div) {
            n.style.dynamic = current_style(look.gutter, look.current);
            doc.mark_style_of(g.div);
        }
    }
}

/// What the line number of the line with the caret has on top of the others' style.
fn current_style(g: &Gutter, current: bool) -> Vec<Style> {
    match (&g.current, current) {
        (Some(s), true) => vec![(**s).clone()],
        _ => Vec::new(),
    }
}

/// Fits what `decorate` gave to the lines there are.
fn fit_rows(mut rows: Vec<Row>, lines: usize, plain: &Rc<LineLook>) -> Vec<Row> {
    let mut covered = 0;
    rows.retain_mut(|r| {
        if covered >= lines {
            return false;
        }
        if let Row::Block { lines: n, .. } = r {
            *n = (*n).clamp(1, lines - covered);
        }
        covered += r.lines();
        true
    });
    rows.extend((covered..lines).map(|_| Row::Line(plain.clone())));
    rows
}

enum Act {
    Keep(usize),
    /// The old slot, and whether its text changed.
    Refill(usize, bool),
    Build,
}

/// How wide the line numbers of a text of `lines` lines are.
fn gutter_width(g: &Gutter, lines: usize, digit: f32, scale: crate::geometry::Scale) -> i32 {
    (digits(lines).max(g.digits) as f32 * digit).ceil() as i32 + scale.px(g.padding.0 + g.padding.1)
}

/// How many digits the biggest line number has.
fn digits(lines: usize) -> usize {
    lines.max(1).ilog10() as usize + 1
}

/// Where a line went in an edit that kept `same.0` lines at the start and `same.1` at the end:
/// `None` if it was in what changed and there is no line in its place.
fn follow_line(line: usize, same: Option<(usize, usize)>, old: usize, new: usize) -> Option<usize> {
    let Some((start, end)) = same else {
        return Some(line);
    };
    if line < start {
        Some(line)
    } else if line + end >= old {
        (line + new).checked_sub(old)
    } else {
        (line + end < new).then_some(line)
    }
}

/// Whether a row's lines were in what changed.
fn was_touched(first: usize, lines: usize, same: Option<(usize, usize)>, old: usize) -> bool {
    same.is_some_and(|(start, end)| first + lines > start && first + end < old)
}

/// Brings the rows of an editor up to date with its text, selection, pre-edit and view. It calls
/// the decorations and makes nodes, so it runs outside of the document.
pub(crate) fn sync(host: NodeId) {
    struct Taken {
        key: ShownKey,
        changed: bool,
        text: Rc<String>,
        starts: Rc<Vec<usize>>,
        old: Structure,
        heights: Heights,
        slots: Vec<RowSlot>,
        window: Range<usize>,
        touched: Option<(usize, usize)>,
        preedit_line: Option<usize>,
        view: i32,
        font: f32,
        scale: crate::geometry::Scale,
        old_gutter_w: i32,
        gutter_x: i32,
    }
    let Some(t) = with_doc(|d| {
        let focused = d.state.focused == Some(host);
        let view = d.viewport.height;
        let font = d.nodes.get(host)?.computed.font_size * d.scale.as_f32();
        let scale = d.scale;
        let e = d.get_mut(host)?.editor_mut()?;
        let key = ShownKey {
            version: e.version,
            anchor: e.anchor,
            focus: e.focus,
            focused,
            preedit: e.preedit.clone(),
            looks: e.looks_version,
        };
        let changed = e.shown_key.as_ref() != Some(&key);
        Some(Taken {
            key,
            changed,
            text: e.text.clone(),
            starts: e.starts.clone(),
            old: std::mem::take(&mut e.structure),
            heights: std::mem::take(&mut e.heights),
            slots: std::mem::take(&mut e.rows),
            window: e.window.clone(),
            touched: e.touched.take(),
            preedit_line: e.preedit_line,
            view,
            font,
            scale,
            old_gutter_w: e.gutter_w,
            gutter_x: e.gutter_x,
        })
    }) else {
        return;
    };
    let (decorate, decorate_lines, blocks, plain, outer, gutter) = with_doc(|d| {
        let e = d.get(host).and_then(Node::editor).expect("taken from it");
        (
            e.decorate.clone(),
            e.decorate_lines.clone(),
            e.blocks.clone(),
            e.plain.clone(),
            e.owner,
            e.gutter.clone(),
        )
    });
    let Taken {
        key,
        changed,
        text,
        starts,
        old,
        mut heights,
        slots,
        window,
        touched,
        preedit_line,
        view,
        font,
        scale,
        old_gutter_w,
        gutter_x,
    } = t;
    let lines = starts.len();
    let cx = DecorateContext {
        text: &text,
        version: key.version,
        starts: &starts,
        anchor: key.anchor,
        focus: key.focus,
        focused: key.focused,
    };

    // How the lines are grouped into rows.
    let fresh = old.lines == 0 && heights.by_line.is_empty();
    let structure = if changed || fresh {
        match (&decorate, &blocks) {
            (Some(f), _) => Structure::from_all(lines, fit_rows(untrack(|| f(&cx)), lines, &plain)),
            (None, Some(f)) => Structure::new(lines, None, untrack(|| f(&cx))),
            (None, None) => Structure::new(lines, None, Vec::new()),
        }
    } else {
        old_structure_for(&old, lines)
    };
    let old_lines = if fresh { lines } else { old.lines };
    let follow = |l: usize| follow_line(l, touched, old_lines, lines);

    // The heights follow the lines.
    if heights.by_line.len() != old_lines {
        heights.by_line = vec![-1; old_lines];
        heights.dirty(0);
    }
    if let Some((start, end)) = touched {
        // The lines that changed keep what they measured, likely the same, until measured again.
        let gone = old_lines.saturating_sub(start + end);
        let came = lines.saturating_sub(start + end);
        if came != gone {
            let kept: Vec<i32> = heights.by_line[start..start + gone.min(came)].to_vec();
            heights.by_line.splice(
                start..start + gone,
                kept.into_iter()
                    .chain(std::iter::repeat_n(-1, came - gone.min(came))),
            );
            heights.dirty(structure.row_of_line(start.min(lines - 1)));
        }
    }
    if !structure.same_blocks(&old) {
        heights.dirty(0);
    }
    if heights.line <= 0 {
        heights.line = (font * 1.3).round().max(1.0) as i32;
        heights.dirty(0);
    }
    heights.sum_up(&structure, &starts, text.len());

    // Which rows get nodes: those around the view and the one with the caret.
    let n = structure.len();
    let caret_row = structure.row_of_line(line_of(&starts, key.focus));
    let (lo, hi) = if window.is_empty() {
        let hi = heights.row_at(2 * view as i64) + 1;
        (0, hi.min(n))
    } else {
        let a = follow(window.start.min(old_lines - 1)).unwrap_or(window.start.min(lines - 1));
        let b = follow((window.end - 1).min(old_lines - 1)).unwrap_or(a);
        let (a, b) = (a.min(lines - 1), b.min(lines - 1).max(a));
        (structure.row_of_line(a), structure.row_of_line(b) + 1)
    };
    let hi = hi.min(lo + MOST_ROWS);
    let mut wanted: Vec<usize> = (lo..hi).collect();
    if !(lo..hi).contains(&caret_row) {
        let at = wanted.partition_point(|&r| r < caret_row);
        wanted.insert(at, caret_row);
    }
    let window_lines =
        structure.first_line(lo)..structure.first_line(hi - 1) + structure.lines_of(hi - 1);

    // The text of a row's lines, with their breaks.
    let src_of = |row: usize| {
        let f = structure.first_line(row);
        let end = starts
            .get(f + structure.lines_of(row))
            .copied()
            .unwrap_or(text.len());
        starts[f]..end
    };
    let line_text = |row: usize| {
        let r = src_of(row);
        let end = if structure.lines_of(row) == 1 {
            r.end - (text[r.clone()].ends_with('\n') as usize)
        } else {
            r.end
        };
        &text[r.start..end]
    };
    let preedit_line_now = key.preedit.as_ref().map(|_| line_of(&starts, key.focus));
    let preedit_at = |row: usize| -> Option<(usize, &Preedit)> {
        let pe = key.preedit.as_ref()?;
        let f = structure.first_line(row);
        (preedit_line_now == Some(f)).then(|| (key.focus - starts[f], pe))
    };
    let old_preedit = preedit_line.and_then(follow);

    // Where the rows that have nodes went.
    let mut slot_at: Vec<(usize, usize)> = Vec::new();
    for (i, s) in slots.iter().enumerate() {
        let Some(f) = follow(s.first).filter(|&f| f < lines) else {
            continue;
        };
        let row = structure.row_of_line(f);
        if structure.first_line(row) == f && slot_at.last().is_none_or(|&(r, _)| r < row) {
            slot_at.push((row, i));
        }
    }
    let slot_for = |row: usize| {
        slot_at
            .binary_search_by_key(&row, |&(r, _)| r)
            .ok()
            .map(|k| slot_at[k].1)
    };

    // How each wanted row looks.
    let mut looks: Vec<Option<Row>> = vec![None; wanted.len()];
    for (k, &row) in wanted.iter().enumerate() {
        if let Some(all) = &structure.all {
            looks[k] = Some(all[row].clone());
        } else if let Some(b) = structure.block(row) {
            looks[k] = Some(b.clone());
        } else if decorate_lines.is_none() {
            looks[k] = Some(Row::Line(plain.clone()));
        } else if !changed
            && let Some(i) = slot_for(row)
            && !was_touched(slots[i].first, 1, touched, old_lines)
        {
            looks[k] = Some(slots[i].look.clone());
        }
    }
    if let Some(f) = &decorate_lines {
        // The lines still to ask for, in runs.
        let mut k = 0;
        while k < wanted.len() {
            if looks[k].is_some() {
                k += 1;
                continue;
            }
            let mut j = k + 1;
            while j < wanted.len() && looks[j].is_none() && wanted[j] == wanted[j - 1] + 1 {
                j += 1;
            }
            let first = structure.first_line(wanted[k]);
            let got = untrack(|| f(&cx, first..first + (j - k)));
            for (m, look) in (k..j).zip(got.into_iter().chain(std::iter::repeat(plain.clone()))) {
                looks[m] = Some(Row::Line(look));
            }
            k = j;
        }
    }
    let looks: Vec<Row> = looks
        .into_iter()
        .map(|l| l.unwrap_or_else(|| plain_row(&plain)))
        .collect();

    // The line numbers.
    let gutter_w = gutter.as_ref().map(|g| {
        let digit = with_doc(|d| d.get(host).and_then(Node::editor).map(|e| e.digit_w))
            .filter(|&w| w > 0.0)
            .unwrap_or(font * 0.6);
        let w = gutter_width(g, lines, digit, scale);
        if old_gutter_w > 0 && (old_gutter_w - w).abs() <= 1 {
            old_gutter_w
        } else {
            w
        }
    });
    let restyle = gutter_w.is_some_and(|w| w != old_gutter_w);
    let caret_line = line_of(&starts, key.focus);
    let gutter_look = |row: usize| {
        gutter.as_ref().map(|g| GutterLook {
            gutter: g,
            number: structure.first_line(row),
            current: (structure.first_line(row)
                ..structure.first_line(row) + structure.lines_of(row))
                .contains(&caret_line),
            x: gutter_x,
            w: gutter_w.unwrap_or_default(),
            logical: gutter_w.unwrap_or_default() as f32 / scale.as_f32(),
        })
    };

    // What becomes of each wanted row.
    let mut acts = Vec::with_capacity(wanted.len());
    let mut used = vec![false; slots.len()];
    for (k, &row) in wanted.iter().enumerate() {
        let Some(i) = slot_for(row) else {
            acts.push(Act::Build);
            continue;
        };
        let s = &slots[i];
        let text_changed = was_touched(s.first, s.look.lines(), touched, old_lines)
            || s.look.lines() != looks[k].lines();
        let pre = old_preedit.is_some_and(|l| (row_lines(&structure, row)).contains(&l))
            || preedit_at(row).is_some();
        let can_refill = matches!((&s.look, &looks[k]), (Row::Line(_), Row::Line(_)))
            && !s.look.has_widgets()
            && !looks[k].has_widgets();
        if !text_changed && !pre && s.look.same_look(&looks[k]) && !restyle {
            acts.push(Act::Keep(i));
        } else if can_refill && !restyle {
            acts.push(Act::Refill(i, text_changed || pre));
        } else {
            acts.push(Act::Build);
            continue;
        }
        used[i] = true;
    }

    // New nodes, made outside of the document.
    let mut built: Vec<Option<RowSlot>> = Vec::with_capacity(acts.len());
    for (k, act) in acts.iter().enumerate() {
        if !matches!(act, Act::Build) {
            built.push(None);
            continue;
        }
        let row = wanted[k];
        let g = gutter_look(row);
        let make = || build_row(host, &looks[k], line_text(row), preedit_at(row), g.as_ref());
        let (mut slot, owner) = match outer {
            Some(o) => o.with(|| create_child_owner(make)),
            None => create_child_owner(make),
        };
        slot.owner = Some(owner);
        built.push(Some(slot));
    }

    // The gaps between them.
    let mut gaps: Vec<(usize, i64)> = Vec::new();
    let mut at = 0;
    for (k, &row) in wanted.iter().enumerate() {
        if row > at {
            gaps.push((k, heights.tops[row] - heights.tops[at]));
        }
        at = row + 1;
    }
    if n > at {
        gaps.push((wanted.len(), heights.tops[n] - heights.tops[at]));
    }
    let have = with_doc(|d| {
        d.get(host)
            .and_then(Node::editor)
            .map_or(0, |e| e.spacers.len())
    });
    let mut new_spacers = Vec::new();
    for _ in have..gaps.len() {
        let make = || create_node(Node::new(NodeKind::Element));
        new_spacers.push(match outer {
            Some(o) => o.with(make),
            None => make(),
        });
    }

    let mut disposed = Vec::new();
    with_doc(|d| {
        // A row that stays and shows is where the view holds on to while the rest moves.
        let anchor = acts.iter().find_map(|a| match a {
            Act::Keep(i) | Act::Refill(i, _) => {
                let div = slots[*i].div;
                crate::visibility::is_in_view(d, div).then_some(div)
            }
            Act::Build => None,
        });
        if let Some(a) = anchor {
            crate::visibility::hold_in_place(d, a);
        }
        let Some(e) = d.get_mut(host).and_then(Node::editor_mut) else {
            return;
        };
        e.spacers.extend(new_spacers);
        let spacers = e.spacers.clone();
        let mut old: Vec<Option<RowSlot>> = slots.into_iter().map(Some).collect();
        for (i, u) in used.iter().enumerate() {
            if !u && let Some(slot) = old[i].take() {
                disposed.push(slot.owner);
            }
        }
        let mut next = Vec::with_capacity(acts.len());
        let mut refills = Vec::new();
        for (k, (act, b)) in acts.iter().zip(built).enumerate() {
            let mut slot = match act {
                Act::Keep(i) => old[*i].take().unwrap(),
                Act::Refill(i, changed) => {
                    refills.push((k, *changed));
                    old[*i].take().unwrap()
                }
                Act::Build => b.unwrap(),
            };
            slot.first = structure.first_line(wanted[k]);
            slot.src = src_of(wanted[k]);
            if matches!(act, Act::Keep(_)) {
                slot.look = looks[k].clone();
            }
            next.push(slot);
        }
        let mut children = Vec::with_capacity(next.len() + gaps.len());
        let mut g = 0;
        for (k, slot) in next.iter().enumerate() {
            if gaps.get(g).is_some_and(|&(at, _)| at == k) {
                children.push(spacers[g]);
                g += 1;
            }
            children.push(slot.div);
        }
        if g < gaps.len() {
            children.push(spacers[g]);
        }
        e.window = window_lines;
        e.preedit_line = preedit_line_now;
        e.gutter_w = gutter_w.unwrap_or_default();
        e.shown_key = Some(key.clone());
        for (g, &(_, h)) in gaps.iter().enumerate() {
            set_spacer(d, spacers[g], h);
        }
        for &(k, changed) in &refills {
            let Row::Line(look) = &looks[k] else { continue };
            let logical = gutter_w.map(|w| w as f32 / scale.as_f32());
            refill(
                d,
                host,
                &next[k],
                look,
                line_text(wanted[k]),
                preedit_at(wanted[k]),
                changed,
                logical,
            );
            next[k].look = looks[k].clone();
        }
        for (k, slot) in next.iter_mut().enumerate() {
            if let (Some(g), Some(look)) = (&mut slot.gutter, gutter_look(wanted[k])) {
                renumber(d, g, &look);
            }
        }
        let Some(e) = d.get_mut(host).and_then(Node::editor_mut) else {
            return;
        };
        e.rows = next;
        if d.get(host).is_some_and(|n| n.children != children) {
            d.set_children(host, children);
        }
        update_links(d, host);
        d.mark_paint();
    });
    with_doc(|d| {
        if let Some(e) = d.get_mut(host).and_then(Node::editor_mut) {
            e.structure = structure;
            e.heights = heights;
        }
    });
    for owner in disposed.into_iter().flatten() {
        owner.dispose();
    }
}

fn plain_row(plain: &Rc<LineLook>) -> Row {
    Row::Line(plain.clone())
}

fn row_lines(s: &Structure, row: usize) -> Range<usize> {
    let f = s.first_line(row);
    f..f + s.lines_of(row)
}

fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

/// The structure as it was, when nothing that `decorate` sees changed.
fn old_structure_for(old: &Structure, lines: usize) -> Structure {
    Structure::new(
        lines,
        old.all.clone(),
        old.blocks.iter().map(|(f, r, _)| (*f, r.clone())).collect(),
    )
}

/// Makes a gap between the rows that have nodes this tall.
fn set_spacer(d: &mut Document, id: NodeId, h: i64) {
    let h = Length::Device(h.min(i32::MAX as i64) as i32);
    let Some(n) = d.nodes.get_mut(id) else {
        return;
    };
    if n.style.base.height != Some(h) {
        n.style.base = Style::default().h(h).flex_none();
        d.mark_style_of(id);
        d.invalidate_layout(id);
    }
}

/// After layout: learns how tall the rows are, and decides whether other rows should have nodes
/// for where the view now is. Returns whether the editor has to be synced, and whether anything
/// changed that has to be laid out again.
fn measure(d: &mut Document, host: NodeId) -> (bool, bool) {
    let Some(h) = d.get(host) else {
        return (false, false);
    };
    let scroll_x =
        (h.computed.overflow_x == anti_webui_style::Overflow::Scroll).then_some(h.scroll.x);
    let Some(first_child) = h.children.first().copied() else {
        return (false, false);
    };
    let visible =
        d.visible_part(h, h.abs)
            .intersect(&RectI::new(0, 0, d.viewport.width, d.viewport.height));
    let top = d.window_rect(first_child).map(|r| r.y);
    let Some(e) = h.editor() else {
        return (false, false);
    };
    if e.rows.is_empty() {
        return (false, false);
    }

    // What the rows measured.
    let mut measured = Vec::new();
    let mut plain_lines = Vec::new();
    let mut char_ws = Vec::new();
    let mut wraps = None;
    let mut width = 0;
    let mut digit = None;
    for slot in &e.rows {
        let Some(n) = d.nodes.get(slot.div) else {
            continue;
        };
        if !n.visible || n.layout_dirty || n.skipped {
            continue;
        }
        let px = |r: crate::layout::RLen| r.resolve(None).unwrap_or(0);
        let m = &n.resolved.margin;
        measured.push((slot.first, n.rect.height + px(m.top) + px(m.bottom)));
        if let (Row::Line(look), Some(t)) = (&slot.look, slot.text.and_then(|t| d.nodes.get(t)))
            && let NodeKind::Text(td) = &t.kind
        {
            {
                wraps.get_or_insert(t.computed.white_space != anti_webui_style::WhiteSpace::Pre);
                width = width.max(t.rect.width);
                if look.style.is_none() && !look.has_widgets() && td.layout.layout.len() == 1 {
                    plain_lines.push(n.rect.height);
                    let chars = slot.src.len();
                    if chars >= 8 {
                        char_ws.push(td.layout.layout.full_width() / chars as f32);
                    }
                }
            }
        }
        if digit.is_none()
            && let Some(g) = &slot.gutter
            && let Some(t) = d.nodes.get(g.text)
            && let NodeKind::Text(td) = &t.kind
            && !t.layout_dirty
            && !td.text.is_empty()
        {
            digit = Some(td.layout.layout.full_width() / td.text.len() as f32);
        }
    }
    let median = |v: &mut Vec<i32>| {
        v.sort_unstable();
        v.get(v.len() / 2).copied()
    };
    let line = median(&mut plain_lines);
    char_ws.sort_by(f32::total_cmp);
    let char_w = char_ws.get(char_ws.len() / 2).copied();
    let window = e.window.clone();
    // A row that shows, and where the rows' coordinates put it now: when what it learned moves the
    // estimates above it, the view is still about that row.
    let pin = e
        .rows
        .iter()
        .find(|r| crate::visibility::is_in_view(d, r.div))
        .map(|r| e.structure.row_of_line(r.first))
        .filter(|&row| row < e.heights.tops.len())
        .map(|row| (row, e.heights.tops[row]));

    let scale = d.scale;
    let e = d
        .get_mut(host)
        .and_then(Node::editor_mut)
        .expect("looked at above");
    let mut relaid = false;
    let mut resync = false;
    {
        let s = &e.structure;
        let hs = &mut e.heights;
        for (first, h) in measured {
            let Some(&b) = hs.by_line.get(first) else {
                continue;
            };
            if b != h {
                // Only a height that isn't what was counted moves what follows.
                let row = s.row_of_line(first);
                let counted = hs.of(s, &e.starts, e.text.len(), row);
                hs.by_line[first] = h;
                if counted != h {
                    hs.dirty(row);
                }
            }
        }
        let mut learned = false;
        if let Some(l) = line
            && l != hs.line
        {
            hs.line = l;
            learned = true;
        }
        if let Some(w) = char_w
            && (w - hs.char_w).abs() > 0.05
        {
            hs.char_w = w;
            learned = true;
        }
        if let Some(w) = wraps
            && w != hs.wraps
        {
            hs.wraps = w;
            learned = true;
        }
        if hs.wraps && width > 0 && width != hs.width {
            hs.width = width;
            learned = true;
        }
        if learned {
            hs.dirty(0);
        }
        hs.sum_up(s, &e.starts, e.text.len());
    }
    if let Some(w) = digit
        && w > 0.0
        && (w - e.digit_w).abs() > 0.01
    {
        e.digit_w = w;
        if let Some(g) = &e.gutter {
            let want = gutter_width(g, e.starts.len(), w, scale);
            if (want - e.gutter_w).abs() > 1 {
                resync = true;
            }
        }
    }
    if let Some(x) = scroll_x
        && e.gutter.is_some()
        && x != e.gutter_x
    {
        e.gutter_x = x;
        let (gutter, w) = (e.gutter.clone().unwrap_or_default(), e.gutter_w);
        let divs: Vec<_> = e
            .rows
            .iter()
            .filter_map(|s| s.gutter.as_ref().map(|g| g.div))
            .collect();
        for div in divs {
            if let Some(n) = d.nodes.get_mut(div) {
                n.style.base = gutter_style(&gutter, x, w);
                d.mark_style_of(div);
            }
        }
        relaid = true;
    }
    let e = d
        .get_mut(host)
        .and_then(Node::editor_mut)
        .expect("looked at above");

    // Where the view is, in the rows' coordinates.
    let (Some(top), false) = (top, visible.is_empty()) else {
        return (resync, relaid);
    };
    let (mut y0, mut y1) = ((visible.y - top) as i64, (visible.bottom() - top) as i64);
    // The row that shows keeps its place among the rows, wherever the estimates above it put it
    // now. Otherwise the rows wanted would be about other lines, the rows that show would go, and
    // nothing would hold the view in place.
    if let Some((row, before)) = pin
        && let Some(&now) = e.heights.tops.get(row)
    {
        y0 += now - before;
        y1 += now - before;
    }
    let view = (y1 - y0).max(1);
    let s = &e.structure;
    let hs = &e.heights;
    let n = s.len();
    let need = (hs.row_at(y0 - view / 2), hs.row_at(y1 + view / 2));
    let have = (
        s.row_of_line(window.start.min(s.lines - 1)),
        s.row_of_line(window.end.saturating_sub(1).min(s.lines - 1)),
    );
    if window.is_empty() || need.0 < have.0 || need.1 > have.1 {
        let lo = hs.row_at(y0 - view);
        let hi = (hs.row_at(y1 + view) + 1).min(n);
        let hi = hi.min(lo + MOST_ROWS);
        e.window = s.first_line(lo)..s.first_line(hi - 1) + s.lines_of(hi - 1);
        return (true, relaid);
    }

    // The gaps take the heights of what they stand for.
    let mut gaps = Vec::new();
    let mut at = 0;
    for slot in &e.rows {
        let row = s.row_of_line(slot.first);
        if row > at {
            gaps.push(hs.tops[row] - hs.tops[at]);
        }
        at = row + 1;
    }
    if n > at {
        gaps.push(hs.tops[n] - hs.tops[at]);
    }
    let spacers = e.spacers.clone();
    let rows: Vec<NodeId> = e.rows.iter().map(|r| r.div).collect();
    let stale = gaps.iter().zip(&spacers).any(|(&h, &id)| {
        d.nodes.get(id).is_none_or(|n| {
            n.style.base.height != Some(Length::Device(h.min(i32::MAX as i64) as i32))
        })
    });
    if stale {
        if let Some(a) = rows
            .into_iter()
            .find(|&r| crate::visibility::is_in_view(d, r))
        {
            crate::visibility::hold_in_place(d, a);
        }
        for (&h, &id) in gaps.iter().zip(&spacers) {
            set_spacer(d, id, h);
        }
        relaid = true;
    }
    (resync, relaid)
}

/// After layout: brings the rows of every editor in line with where it is seen. Returns whether
/// anything has to be laid out again.
pub(crate) fn follow_views() -> bool {
    let hosts = with_doc(|d| d.editors.clone());
    let mut changed = false;
    for host in hosts {
        let (resync, relaid) = with_doc(|d| measure(d, host));
        if resync {
            sync(host);
        }
        changed |= resync || relaid;
    }
    changed
}

/// The row with nodes that holds line `line`, if there is one.
fn slot_of_line(rows: &[RowSlot], line: usize) -> Option<usize> {
    let i = rows.partition_point(|r| r.first <= line).checked_sub(1)?;
    (line < rows[i].first + rows[i].look.lines()).then_some(i)
}

/// Writes in the rows' text nodes what of the selection and the caret is on them.
fn update_links(d: &mut Document, host: NodeId) {
    let Some(e) = d.get(host).and_then(Node::editor) else {
        return;
    };
    if e.rows.is_empty() {
        return;
    }
    let (a, b) = e.selection_range();
    let mut selected: Vec<(NodeId, Range<usize>)> = Vec::new();
    let mut blocks = Vec::new();
    let link = |id: NodeId| match &d.nodes.get(id)?.kind {
        NodeKind::Text(t) => t.row.as_deref(),
        _ => None,
    };
    if a < b {
        let (la, lb) = (e.line_of(a), e.line_of(b));
        let lo = e.rows.partition_point(|r| r.first + r.look.lines() <= la);
        let hi = e.rows.partition_point(|r| r.first <= lb);
        for r in &e.rows[lo..hi] {
            let Some(id) = r.text else {
                // A node for some lines is selected when some of their text is.
                let start = e.starts[r.first];
                let end = start + (r.src.end - r.src.start);
                if a < end && b > start {
                    blocks.push(r.div);
                }
                continue;
            };
            let Some(l) = link(id) else { continue };
            let ls = e.starts[r.first];
            let le = ls + l.len;
            if b < ls || a > le {
                continue;
            }
            let da = l.to_display(a.max(ls) - ls);
            let db = if b > le {
                l.shown + 1
            } else {
                l.to_display(b - ls)
            };
            if da < db {
                selected.push((id, da..db));
            }
        }
    }
    let caret = slot_of_line(&e.rows, e.line_of(e.focus)).and_then(|i| {
        let r = &e.rows[i];
        r.text.and_then(|id| {
            let l = link(id)?;
            let at = match &l.preedit {
                Some((range, cursor)) => range.start + cursor.map_or(range.len(), |c| c.1),
                None => l.to_display(e.focus - e.starts[r.first]),
            };
            Some((id, r.div, at))
        })
    });
    let old_selected = e.selected_rows.clone();
    let old_caret = e.caret_row;
    let set = |d: &mut Document, id: NodeId, f: &dyn Fn(&mut RowLink)| {
        if let Some(n) = d.nodes.get_mut(id)
            && let NodeKind::Text(t) = &mut n.kind
            && let Some(l) = &mut t.row
        {
            f(l);
        }
    };
    for id in old_selected {
        set(d, id, &|l| l.selected = None);
    }
    for (id, r) in &selected {
        set(d, *id, &|l| l.selected = Some(r.clone()));
    }
    if let Some((id, _)) = old_caret {
        set(d, id, &|l| l.caret = None);
    }
    if let Some((id, _, at)) = caret {
        set(d, id, &|l| l.caret = Some(at));
    }
    if let Some(e) = d.get_mut(host).and_then(Node::editor_mut) {
        e.selected_rows = selected.into_iter().map(|(id, _)| id).collect();
        e.selected_blocks = blocks;
        e.caret_row = caret.map(|(id, div, _)| (id, div));
    }
}

/// The box of the row with the caret, which is laid out wherever it is.
pub(crate) fn caret_rows(doc: &Document) -> Vec<NodeId> {
    doc.editors
        .iter()
        .filter_map(|&id| doc.get(id)?.editor()?.caret_row.map(|(_, div)| div))
        .collect()
}

/// Shows where what is dragged over the editor would go, at byte `at`, or nowhere.
pub(crate) fn set_drop_caret(d: &mut Document, host: NodeId, at: Option<usize>) {
    let Some(e) = d.get(host).and_then(Node::editor) else {
        return;
    };
    let next = at.and_then(|at| {
        let r = &e.rows[slot_of_line(&e.rows, e.line_of(at))?];
        let id = r.text?;
        let NodeKind::Text(t) = &d.nodes.get(id)?.kind else {
            return None;
        };
        let shown = t.row.as_ref()?.to_display(at - e.starts[r.first]);
        Some((id, shown))
    });
    let old = e.drop_row;
    let set = |d: &mut Document, id: NodeId, at: Option<usize>| {
        if let Some(n) = d.nodes.get_mut(id)
            && let NodeKind::Text(t) = &mut n.kind
            && let Some(l) = &mut t.row
        {
            l.drop_caret = at;
        }
    };
    if let Some(id) = old {
        set(d, id, None);
    }
    if let Some((id, shown)) = next {
        set(d, id, Some(shown));
    }
    if let Some(e) = d.get_mut(host).and_then(Node::editor_mut) {
        e.drop_row = next.map(|(id, _)| id);
    }
    if old.is_some() || next.is_some() {
        d.mark_paint();
    }
}

/// After the text changed: the rows, the bound signal and the listener.
pub(crate) fn after_edit(host: NodeId, outcome: EditorOutcome) {
    if !(outcome.changed || outcome.moved) {
        return;
    }
    // Those who follow the text hear of each change before the rows are decorated.
    let told = with_doc(|d| {
        let e = d.get_mut(host)?.editor_mut()?;
        Some((e.on_edit.clone()?, std::mem::take(&mut e.edits)))
    });
    if let Some((f, edits)) = told {
        untrack(|| {
            for edit in &edits {
                f(edit);
            }
        });
    }
    sync(host);
    if outcome.changed
        && let Some((value, handler, text)) = with_doc(|d| {
            let e = d.get(host)?.editor()?;
            Some((e.value, e.on_change.clone(), e.text.clone()))
        })
    {
        batch(|| {
            if let Some(v) = value {
                v.set_if_changed((*text).clone());
            }
            if let Some(h) = handler {
                h(&text);
            }
        });
    }
    // After the text is told, so that what follows the selection has the same text.
    let selected = with_doc(|d| {
        let e = d.get(host)?.editor()?;
        Some((e.on_select.clone()?, e.anchor, e.focus))
    });
    if let Some((f, anchor, focus)) = selected {
        untrack(|| f(anchor, focus));
    }
}

/// Disposes of what the rows were made under.
pub(crate) fn dispose_rows(host: NodeId) {
    let owners: Vec<Owner> = with_doc(|d| {
        d.get_mut(host)
            .and_then(Node::editor_mut)
            .map(|e| std::mem::take(&mut e.rows))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.owner)
            .collect()
    });
    for o in owners {
        o.dispose();
    }
}

// ---------------------------------------------------------------------------------------------
// Where things are

/// A row's text node, with its layout and link, when it is laid out.
fn laid_out<'a>(
    doc: &'a Document,
    slot: &RowSlot,
) -> Option<(&'a Node, &'a TextLayout, &'a RowLink)> {
    if doc.nodes.get(slot.div)?.skipped {
        return None;
    }
    let n = doc.nodes.get(slot.text?)?;
    match &n.kind {
        NodeKind::Text(t) => Some((n, &t.layout, t.row.as_deref()?)),
        _ => None,
    }
}

/// The lines of a layout from top to bottom: their top, bottom and middle.
fn visual_lines(layout: &TextLayout) -> Vec<(f32, f32)> {
    layout
        .layout
        .lines()
        .map(|l| {
            let m = l.metrics();
            (m.block_min_coord, m.block_max_coord)
        })
        .collect()
}

/// The caret's rectangle, in the rows' coordinates.
pub(crate) fn caret_rect(doc: &Document, host: NodeId, width: i32) -> Option<RectI> {
    let e = doc.get(host)?.editor()?;
    let (text, _) = e.caret_row?;
    let n = doc.nodes.get(text)?;
    let NodeKind::Text(t) = &n.kind else {
        return None;
    };
    let at = t.row.as_ref()?.caret?;
    if n.held {
        return None;
    }
    Some(
        t.layout
            .caret_rect(at, Affinity::Downstream, width)
            .translate(n.abs.x, n.abs.y),
    )
}

/// The byte of the text at a point in the rows' coordinates.
fn hit_rows(doc: &Document, e: &EditorData, x: f32, y: f32) -> usize {
    if e.rows.is_empty() {
        return 0;
    }
    let i = e
        .rows
        .partition_point(|r| doc.nodes.get(r.div).is_some_and(|n| (n.abs.y as f32) <= y))
        .saturating_sub(1);
    let slot = &e.rows[i];
    // Below a row and above the next one with nodes: in rows that have none, out of sight.
    if let Some(n) = doc.nodes.get(slot.div)
        && y >= (n.abs.y + n.abs.height) as f32
        && e.rows
            .get(i + 1)
            .is_none_or(|r| r.first > slot.first + slot.look.lines())
    {
        let line = (slot.first + slot.look.lines()).min(e.line_count() - 1);
        return e.starts[line];
    }
    let start = e.starts[slot.first];
    match laid_out(doc, slot) {
        Some((n, layout, link)) => {
            let (at, _) = layout.hit(x - n.abs.x as f32, y - n.abs.y as f32);
            start + link.to_source(at)
        }
        None => start,
    }
}

/// A point in window coordinates in the rows' coordinates.
fn rows_point(doc: &Document, e: &EditorData, p: PointF) -> Option<PointF> {
    let first = doc.nodes.get(e.rows.first()?.div)?;
    doc.from_window(first, p)
}

/// The byte of the text under a point in window coordinates.
pub(crate) fn hit(doc: &Document, host: NodeId, p: PointF) -> Option<usize> {
    let e = doc.get(host)?.editor()?;
    let q = rows_point(doc, e, p)?;
    Some(hit_rows(doc, e, q.x, q.y))
}

/// How far down the top of the editor's box is in the rows' coordinates (device pixels from the
/// top of the first row), going by its scroll offset and the last layout.
fn view_top(doc: &Document, host: NodeId) -> Option<i64> {
    let h = doc.get(host)?;
    let first = doc.get(*h.children.first()?)?;
    Some(h.scroll.y as i64 - first.rect.y as i64)
}

/// The line at the top of the editor's box, with how far down into it (`10.5` is halfway down
/// line 10). Rows of several lines count as their lines, evenly.
pub(crate) fn top_line(doc: &Document, host: NodeId) -> Option<f64> {
    let y = view_top(doc, host)?;
    let e = doc.get(host)?.editor()?;
    let (s, hs) = (&e.structure, &e.heights);
    if hs.tops.len() != s.len() + 1 || s.len() == 0 {
        return None;
    }
    let row = hs.row_at(y);
    let (top, bottom) = (hs.tops[row], hs.tops[row + 1]);
    let into = if bottom > top {
        ((y - top) as f64 / (bottom - top) as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some(s.first_line(row) as f64 + into * s.lines_of(row) as f64)
}

/// Scrolls the editor until `line` (as `top_line` gives it) is at the top of its box, as far as
/// it goes.
pub(crate) fn scroll_to_line(doc: &mut Document, host: NodeId, line: f64) {
    let (Some(y), Some(h)) = (view_top(doc, host), doc.get(host)) else {
        return;
    };
    let Some(e) = h.editor() else {
        return;
    };
    let (s, hs) = (&e.structure, &e.heights);
    if hs.tops.len() != s.len() + 1 || s.lines == 0 {
        return;
    }
    let l = line.max(0.0);
    let row = s.row_of_line((l as usize).min(s.lines - 1));
    let into = ((l - s.first_line(row) as f64) / s.lines_of(row) as f64).clamp(0.0, 1.0);
    let (top, bottom) = (hs.tops[row], hs.tops[row + 1]);
    let want = top + ((bottom - top) as f64 * into).round() as i64;
    let pad = h.padding_abs();
    let most = (h.content_size.height - pad.height).max(0) as i64;
    let to = (h.scroll.y as i64 + want - y).clamp(0, most) as i32;
    if to != h.scroll.y {
        let h = doc.get_mut(host).expect("looked at above");
        h.scroll.y = to;
        doc.mark_scrolled(host);
    }
}

/// Moves the caret, by the text or by how it is laid out.
pub(crate) fn move_caret(
    doc: &mut Document,
    host: NodeId,
    motion: Motion,
    extend: bool,
    page: i32,
) -> EditorOutcome {
    let width = crate::paint::caret_width(doc);
    let caret = caret_rect(doc, host, width);
    let Some(e) = doc.get(host).and_then(Node::editor) else {
        return EditorOutcome::default();
    };
    if e.preedit.is_some() {
        return EditorOutcome::default();
    }
    let to = match motion {
        Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
            let down = matches!(motion, Motion::Down | Motion::PageDown);
            let paged = matches!(motion, Motion::PageUp | Motion::PageDown);
            let goal = e.goal_x.or(caret.map(|c| c.x as f32)).unwrap_or_default();
            let to = match caret {
                Some(c) if paged => {
                    let y = c.y + c.height / 2 + if down { page } else { -page };
                    hit_rows(doc, e, goal, y as f32)
                }
                Some(c) => vertical(doc, e, goal, c, down),
                None => {
                    // Out of the row it is in, which may be several lines shown as one node.
                    let s = &e.structure;
                    let row = s.row_of_line(e.line_of(e.focus).min(s.lines.saturating_sub(1)));
                    let first = s.first_line(row) as isize;
                    let line = if down {
                        first + s.lines_of(row) as isize
                    } else {
                        first - 1
                    };
                    let line = line.clamp(0, e.line_count() as isize - 1) as usize;
                    e.starts[line]
                }
            };
            let e = doc.get_mut(host).and_then(Node::editor_mut).unwrap();
            e.put_caret(to, extend);
            e.goal_x = Some(goal);
            return EditorOutcome::MOVED;
        }
        Motion::LineStart | Motion::LineEnd => {
            let end = motion == Motion::LineEnd;
            let line = e.line(e.line_of(e.focus));
            let slot = slot_of_line(&e.rows, e.line_of(e.focus)).map(|i| &e.rows[i]);
            match (caret, slot.and_then(|slot| laid_out(doc, slot))) {
                (Some(c), Some((n, layout, link))) => {
                    let y = (c.y + c.height / 2 - n.abs.y) as f32;
                    let x = if end { 1e7 } else { -1e7 };
                    let (at, _) = layout.hit(x, y);
                    let at = link.to_source(at);
                    // The end of a line that wraps is before the space it wraps at.
                    let at = if end && at < link.len && line.start + at > e.focus {
                        e.text[line.start..line.start + at]
                            .trim_end_matches(' ')
                            .len()
                            .max(e.focus - line.start)
                    } else {
                        at
                    };
                    line.start + at
                }
                _ if end => line.end,
                _ => line.start,
            }
        }
        _ => {
            let e = doc.get_mut(host).and_then(Node::editor_mut).unwrap();
            e.move_in_text(motion, extend);
            return EditorOutcome::MOVED;
        }
    };
    let e = doc.get_mut(host).and_then(Node::editor_mut).unwrap();
    e.put_caret(to, extend);
    EditorOutcome::MOVED
}

/// Where up or down goes from the caret at `c`: the line above or below in its row, or the
/// nearest line of the row before or after.
fn vertical(doc: &Document, e: &EditorData, x: f32, c: RectI, down: bool) -> usize {
    let line = e.line_of(e.focus);
    let Some(ri) = slot_of_line(&e.rows, line) else {
        let to = if down { line + 1 } else { line.wrapping_sub(1) };
        return e
            .starts
            .get(to)
            .copied()
            .unwrap_or(if down { e.text.len() } else { 0 });
    };
    let mid = (c.y + c.height / 2) as f32;
    if let Some((n, layout, link)) = laid_out(doc, &e.rows[ri]) {
        let lines = visual_lines(layout);
        let local = mid - n.abs.y as f32;
        let at = lines
            .iter()
            .position(|&(_, b)| local < b)
            .unwrap_or(lines.len().saturating_sub(1));
        let next = if down { at + 1 } else { at.wrapping_sub(1) };
        if let Some(&(t, b)) = lines.get(next) {
            let (i, _) = layout.hit(x - n.abs.x as f32, (t + b) / 2.0);
            return e.starts[e.rows[ri].first] + link.to_source(i);
        }
    }
    let here = &e.rows[ri];
    let to = if down { ri + 1 } else { ri.wrapping_sub(1) };
    let next = if down {
        e.rows
            .get(to)
            .filter(|r| r.first == here.first + here.look.lines())
    } else {
        e.rows
            .get(to)
            .filter(|r| r.first + r.look.lines() == here.first)
    };
    let Some(slot) = next else {
        // The next row has no nodes: its first line, or the end.
        let line = if down {
            here.first + here.look.lines()
        } else {
            here.first.wrapping_sub(1)
        };
        return match e.starts.get(line) {
            Some(&s) if down => s,
            Some(_) => e.starts[e.line_of(e.starts[line])],
            None if down => e.text.len(),
            None => 0,
        };
    };
    let first = e.starts[slot.first];
    match (&slot.look, laid_out(doc, slot)) {
        (Row::Line(_), Some((n, layout, link))) => {
            let lines = visual_lines(layout);
            let (t, b) = if down { lines.first() } else { lines.last() }
                .copied()
                .unwrap_or_default();
            let (i, _) = layout.hit(x - n.abs.x as f32, (t + b) / 2.0);
            first + link.to_source(i)
        }
        (Row::Block { lines, .. }, _) if !down => e.starts[slot.first + lines - 1],
        _ => first,
    }
}

/// A press at a point in window coordinates: places the caret, or selects a word or a line.
pub(crate) fn pointer_down(doc: &mut Document, host: NodeId, p: PointF, clicks: u32, extend: bool) {
    let Some(at) = hit(doc, host, p) else {
        return;
    };
    let Some(e) = doc.get_mut(host).and_then(Node::editor_mut) else {
        return;
    };
    let granularity = match clicks {
        1 => Granularity::Char,
        2 => Granularity::Word,
        _ => Granularity::Line,
    };
    let (a, b) = match granularity {
        Granularity::Char => (at, at),
        Granularity::Word => e.word_at(at),
        Granularity::Line => {
            let l = e.line_of(at);
            let r = e.line(l);
            (r.start, (r.end + 1).min(e.text.len()))
        }
    };
    if extend && granularity == Granularity::Char {
        e.focus = at;
    } else {
        e.anchor = a;
        e.focus = b;
    }
    e.drag = Some((granularity, a, b));
    e.goal_x = None;
    e.preedit = None;
    e.moved();
}

/// The pointer moved while pressed.
pub(crate) fn pointer_drag(doc: &mut Document, host: NodeId, p: PointF) -> bool {
    let Some(at) = hit(doc, host, p) else {
        return false;
    };
    let Some(e) = doc.get_mut(host).and_then(Node::editor_mut) else {
        return false;
    };
    let Some((granularity, ia, ib)) = e.drag else {
        return false;
    };
    let before = (e.anchor, e.focus);
    let (a, b) = match granularity {
        Granularity::Char => (at, at),
        Granularity::Word => e.word_at(at),
        Granularity::Line => {
            let r = e.line(e.line_of(at));
            (r.start, (r.end + 1).min(e.text.len()))
        }
    };
    if granularity == Granularity::Char {
        e.focus = at;
    } else if a < ia {
        e.anchor = ib;
        e.focus = a;
    } else {
        e.anchor = ia;
        e.focus = b;
    }
    let moved = before != (e.anchor, e.focus);
    if moved {
        e.moved();
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> EditorData {
        let mut e = EditorData::new();
        e.set_text(text);
        e.undo.clear();
        e.select(0, 0);
        e
    }

    #[test]
    fn line_starts_follow_the_changes() {
        let mut e = editor("ab\ncd\nef");
        e.select(4, 4);
        e.insert("X\nY");
        assert_eq!(*e.text, "ab\ncX\nYd\nef");
        assert_eq!(*e.starts, line_starts(&e.text));
        e.select(1, 8);
        e.delete(false, false);
        assert_eq!(*e.text, "a\nef");
        assert_eq!(*e.starts, line_starts(&e.text));
        e.undo();
        assert_eq!(*e.starts, line_starts(&e.text));
        e.undo();
        assert_eq!(*e.text, "ab\ncd\nef");
        assert_eq!(*e.starts, line_starts(&e.text));
    }

    #[test]
    fn typing_and_deleting_are_one_step_each() {
        let mut e = editor("hello");
        e.select(5, 5);
        for c in ["a", "b", "c"] {
            e.insert(c);
        }
        e.delete(false, false);
        e.delete(false, false);
        assert_eq!(*e.text, "helloa");
        e.undo();
        assert_eq!(*e.text, "helloabc");
        e.undo();
        assert_eq!(*e.text, "hello");
        assert_eq!(e.focus, 5);
        e.redo();
        e.redo();
        assert_eq!(*e.text, "helloa");
        assert_eq!(e.focus, 6);
    }

    #[test]
    fn a_new_text_from_outside_is_one_change_and_the_caret_stays_by_its_text() {
        let mut e = editor("one two three");
        e.select(9, 9);
        e.set_text("one 2 three");
        assert_eq!(e.focus, 7);
        e.undo();
        assert_eq!(*e.text, "one two three");
    }

    #[test]
    fn words_stay_in_their_line() {
        let e = editor("foo bar\n  baz");
        assert_eq!(e.word_left(7), 4);
        assert_eq!(e.word_left(4), 0);
        assert_eq!(e.word_left(8), 7);
        assert_eq!(e.word_right(0), 3);
        assert_eq!(e.word_right(7), 8);
        assert_eq!(e.word_right(8), 13);
        assert_eq!(e.word_at(5), (4, 7));
    }

    #[test]
    fn what_is_shown_maps_back_to_the_line() {
        // `**bold** and [link](url)` with the marks hidden.
        let line = "**bold** and [link](url)";
        let style = Rc::new(Style::default());
        let look = LineLook {
            style: None,
            marks: vec![(2..6, style.clone()), (14..18, style)],
            replace: vec![(0..2, None), (6..8, None), (13..14, None), (18..24, None)],
            raised: vec![],
        };
        let d = display(line, &look, None);
        assert_eq!(d.text, "bold and link ");
        assert_eq!(d.shown, 13);
        let spans: Vec<_> = d.spans.iter().map(|s| s.range.clone()).collect();
        assert_eq!(spans, [0..4, 9..13]);
        let link = link_of(NodeId::default(), line, &d, None);
        assert_eq!(link.to_display(2), 0);
        assert_eq!(link.to_display(0), 0);
        assert_eq!(link.to_display(7), 4);
        assert_eq!(link.to_display(10), 6);
        assert_eq!(link.to_source(0), 2);
        assert_eq!(link.to_source(4), 6);
        assert_eq!(link.to_source(5), 9);
        assert_eq!(link.to_source(13), 24);
    }

    #[test]
    fn the_pre_edit_is_shown_at_the_caret_in_the_style_around_it() {
        let style = Rc::new(Style::default());
        let look = LineLook {
            style: None,
            marks: vec![(2..6, style)],
            replace: vec![(0..2, None), (6..8, None)],
            raised: vec![],
        };
        let d = display("**bold**", &look, Some((4, "XY")));
        assert_eq!(d.text, "boXYld ");
        assert_eq!(d.preedit, Some(2..4));
        let spans: Vec<_> = d.spans.iter().map(|s| s.range.clone()).collect();
        assert_eq!(spans, [0..2, 2..4, 4..6]);
    }

    #[test]
    fn a_node_takes_the_place_of_what_it_shows() {
        let look = LineLook {
            style: None,
            marks: vec![],
            replace: vec![(2..9, Some(Widget::new(1, crate::view::div)))],
            raised: vec![],
        };
        let d = display("a ![x](y) b", &look, None);
        assert_eq!(d.text, "a  b ");
        assert_eq!(d.boxes.len(), 1);
        assert_eq!(d.boxes[0].0, 2);
        let link = link_of(NodeId::default(), "a ![x](y) b", &d, None);
        assert_eq!(link.to_source(2), 2);
        assert_eq!(link.to_source(3), 10);
    }
}
