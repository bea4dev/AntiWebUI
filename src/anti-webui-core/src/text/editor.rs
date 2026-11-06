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
use anti_webui_style::{Style, Styled};
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
    first: usize,
    /// The text of its lines with their breaks, in `EditorData::shown`.
    src: Range<usize>,
    look: Row,
    div: NodeId,
    text: Option<NodeId>,
    owner: Option<Owner>,
}

/// What the rows were made from, to know when they are out of date.
#[derive(Clone, PartialEq)]
struct ShownKey {
    version: u64,
    anchor: usize,
    focus: usize,
    focused: bool,
    preedit: Option<Preedit>,
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
    pub text: String,
    /// Where each line starts.
    starts: Vec<usize>,
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
    /// What the rows are made under.
    pub(crate) owner: Option<Owner>,
    plain: Rc<LineLook>,

    pub(crate) rows: Vec<RowSlot>,
    /// The text the rows were made from.
    shown: String,
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
            text: String::new(),
            starts: vec![0],
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
            owner: None,
            plain: Rc::default(),
            rows: Vec::new(),
            shown: String::new(),
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
        let lo = self.starts.partition_point(|&s| s <= at);
        let hi = self.starts.partition_point(|&s| s <= at + removed);
        let new: Vec<usize> = inserted
            .match_indices('\n')
            .map(|(i, _)| at + i + 1)
            .collect();
        let n = new.len();
        self.starts.splice(lo..hi, new);
        let delta = inserted.len() as isize - removed as isize;
        for s in &mut self.starts[lo + n..] {
            *s = (*s as isize + delta) as usize;
        }
    }

    fn apply(&mut self, at: usize, removed: usize, inserted: &str) {
        self.text.replace_range(at..at + removed, inserted);
        self.shift_starts(at, removed, inserted);
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
        if self.text == *text {
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

/// The style of a row's box: its look, laid out only near the screen.
fn row_style(style: Option<&Rc<Style>>) -> Style {
    style
        .map(|s| (**s).clone())
        .unwrap_or_default()
        .content_visibility_auto()
        .estimate_contents()
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

/// Makes a row's nodes, under the current owner.
fn build_row(host: NodeId, row: &Row, line: &str, preedit: Option<(usize, &Preedit)>) -> RowSlot {
    match row {
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
                })
                .collect();
            let link = link_of(host, line, &d, preedit.map(|p| p.1));
            let text = create_node(Node::new(NodeKind::Text(TextData {
                text: d.text,
                version: 0,
                layout: TextLayout::default(),
                zoom: 1.0,
                in_svg: false,
                rich: Some(Box::new(RichData {
                    spans: d.spans,
                    boxes,
                    resolved: Vec::new(),
                    hovered: None,
                })),
                row: Some(Box::new(link)),
            })));
            let mut div = Node::new(NodeKind::Element);
            div.style.base = row_style(look.style.as_ref());
            let div = create_node(div);
            with_doc(|doc| {
                doc.set_children(text, children);
                doc.set_children(div, vec![text]);
            });
            RowSlot {
                first: 0,
                src: 0..0,
                look: row.clone(),
                div,
                text: Some(text),
                owner: None,
            }
        }
        Row::Block { style, widget, .. } => {
            let w = (widget.build)();
            let mut div = Node::new(NodeKind::Element);
            div.style.base = row_style(style.as_ref());
            let div = create_node(div);
            with_doc(|doc| doc.set_children(div, vec![w.0]));
            RowSlot {
                first: 0,
                src: 0..0,
                look: row.clone(),
                div,
                text: None,
                owner: None,
            }
        }
    }
}

/// Shows another line in a row's nodes without making them again.
fn refill(
    doc: &mut Document,
    host: NodeId,
    slot: &RowSlot,
    look: &Rc<LineLook>,
    line: &str,
    preedit: Option<(usize, &Preedit)>,
) {
    let Row::Line(old) = &slot.look else {
        return;
    };
    if old.style != look.style
        && let Some(n) = doc.nodes.get_mut(slot.div)
    {
        n.style.base = row_style(look.style.as_ref());
        doc.mark_style_of(slot.div);
    }
    let Some(text) = slot.text else {
        return;
    };
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
    Refill(usize),
    Build,
}

/// Brings the rows of an editor up to date with its text, selection and pre-edit. It calls
/// `decorate` and makes nodes, so it runs outside of the document.
pub(crate) fn sync(host: NodeId) {
    let Some((key, text, starts, decorate, plain, outer)) = with_doc(|d| {
        let focused = d.state.focused == Some(host);
        let e = d.get(host)?.editor()?;
        let key = ShownKey {
            version: e.version,
            anchor: e.anchor,
            focus: e.focus,
            focused,
            preedit: e.preedit.clone(),
        };
        if e.shown_key.as_ref() == Some(&key) {
            return None;
        }
        Some((
            key,
            e.text.clone(),
            e.starts.clone(),
            e.decorate.clone(),
            e.plain.clone(),
            e.owner,
        ))
    }) else {
        return;
    };
    let rows = {
        let cx = DecorateContext {
            text: &text,
            version: key.version,
            starts: &starts,
            anchor: key.anchor,
            focus: key.focus,
            focused: key.focused,
        };
        match &decorate {
            Some(f) => untrack(|| f(&cx)),
            None => Vec::new(),
        }
    };
    let rows = fit_rows(rows, starts.len(), &plain);
    // Where each new row starts, and its text.
    let mut firsts = Vec::with_capacity(rows.len());
    let mut first = 0;
    for r in &rows {
        firsts.push(first);
        first += r.lines();
    }
    let src_of = |i: usize| {
        let f = firsts[i];
        let end = starts
            .get(f + rows[i].lines())
            .copied()
            .unwrap_or(text.len());
        starts[f]..end
    };
    let preedit_line = key.preedit.as_ref().map(|_| {
        starts
            .partition_point(|&s| s <= key.focus)
            .saturating_sub(1)
    });
    let preedit_at = |i: usize| -> Option<(usize, &Preedit)> {
        let pe = key.preedit.as_ref()?;
        (preedit_line == Some(firsts[i])).then(|| (key.focus - starts[firsts[i]], pe))
    };

    // What becomes of each row.
    let Some((acts, gone)) = with_doc(|d| {
        let e = d.get(host)?.editor()?;
        let old = &e.rows;
        let old_pre = e.shown_key.as_ref().and_then(|k| {
            k.preedit.as_ref()?;
            Some(
                e.shown.as_bytes()[..k.focus]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count(),
            )
        });
        let touched =
            |o: &RowSlot| old_pre.is_some_and(|l| (o.first..o.first + o.look.lines()).contains(&l));
        let same = |o: &RowSlot, n: usize| {
            o.look.same_look(&rows[n])
                && e.shown[o.src.clone()] == text[src_of(n)]
                && !touched(o)
                && preedit_at(n).is_none()
        };
        let (no, nn) = (old.len(), rows.len());
        let mut p = 0;
        while p < no.min(nn) && same(&old[p], p) {
            p += 1;
        }
        let mut s = 0;
        while s < no.min(nn) - p && same(&old[no - 1 - s], nn - 1 - s) {
            s += 1;
        }
        let mut acts = Vec::with_capacity(nn);
        acts.extend((0..p).map(Act::Keep));
        let mut gone = Vec::new();
        for k in 0..(no - s - p).max(nn - s - p) {
            let (o, n) = (p + k, p + k);
            match (o < no - s, n < nn - s) {
                (true, true) => {
                    let can_refill =
                        matches!((&old[o].look, &rows[n]), (Row::Line(_), Row::Line(_)))
                            && !old[o].look.has_widgets()
                            && !rows[n].has_widgets();
                    if same(&old[o], n) {
                        acts.push(Act::Keep(o));
                    } else if can_refill {
                        acts.push(Act::Refill(o));
                    } else {
                        acts.push(Act::Build);
                        gone.push(o);
                    }
                }
                (false, true) => acts.push(Act::Build),
                (true, false) => gone.push(o),
                (false, false) => {}
            }
        }
        acts.extend((no - s..no).map(Act::Keep));
        Some((acts, gone))
    }) else {
        return;
    };

    // New nodes, made outside of the document.
    let mut built: Vec<Option<RowSlot>> = Vec::with_capacity(acts.len());
    for (i, act) in acts.iter().enumerate() {
        if !matches!(act, Act::Build) {
            built.push(None);
            continue;
        }
        let r = src_of(i);
        let line_end = if rows[i].lines() == 1 {
            r.end - (text[r.clone()].ends_with('\n') as usize)
        } else {
            r.end
        };
        let line = &text[r.start..line_end];
        let make = || build_row(host, &rows[i], line, preedit_at(i));
        let (mut slot, owner) = match outer {
            Some(o) => o.with(|| create_child_owner(make)),
            None => create_child_owner(make),
        };
        slot.owner = Some(owner);
        built.push(Some(slot));
    }

    let mut disposed = Vec::new();
    with_doc(|d| {
        let Some(e) = d.get_mut(host).and_then(Node::editor_mut) else {
            return;
        };
        let mut old: Vec<Option<RowSlot>> =
            std::mem::take(&mut e.rows).into_iter().map(Some).collect();
        for &o in &gone {
            if let Some(slot) = old[o].take() {
                disposed.push(slot.owner);
            }
        }
        let structure = !gone.is_empty() || built.iter().any(Option::is_some);
        let mut next = Vec::with_capacity(acts.len());
        let mut refills = Vec::new();
        for (i, (act, b)) in acts.iter().zip(built).enumerate() {
            let mut slot = match act {
                Act::Keep(o) => old[*o].take().unwrap(),
                Act::Refill(o) => {
                    refills.push(i);
                    old[*o].take().unwrap()
                }
                Act::Build => b.unwrap(),
            };
            slot.first = firsts[i];
            slot.src = src_of(i);
            if matches!(act, Act::Keep(_)) {
                slot.look = rows[i].clone();
            }
            next.push(slot);
        }
        let divs: Vec<NodeId> = next.iter().map(|s| s.div).collect();
        e.shown = text.clone();
        e.shown_key = Some(key.clone());
        // Refilling needs the document, so the slots go back to the editor after it.
        for &i in &refills {
            let Row::Line(look) = &rows[i] else { continue };
            let r = src_of(i);
            let line = &text[r.start..r.end - (text[r.clone()].ends_with('\n') as usize)];
            refill(d, host, &next[i], look, line, preedit_at(i));
            next[i].look = rows[i].clone();
        }
        let Some(e) = d.get_mut(host).and_then(Node::editor_mut) else {
            return;
        };
        e.rows = next;
        if structure {
            d.set_children(host, divs);
        }
        update_links(d, host);
        d.mark_paint();
    });
    for owner in disposed.into_iter().flatten() {
        owner.dispose();
    }
}

/// The row that holds line `line`.
fn row_of_line(rows: &[RowSlot], line: usize) -> usize {
    rows.partition_point(|r| r.first <= line).saturating_sub(1)
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
        let lo = row_of_line(&e.rows, e.line_of(a));
        let hi = row_of_line(&e.rows, e.line_of(b));
        for r in &e.rows[lo..=hi] {
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
    let caret = {
        let r = &e.rows[row_of_line(&e.rows, e.line_of(e.focus))];
        r.text.and_then(|id| {
            let l = link(id)?;
            let at = match &l.preedit {
                Some((range, cursor)) => range.start + cursor.map_or(range.len(), |c| c.1),
                None => l.to_display(e.focus - e.starts[r.first]),
            };
            Some((id, r.div, at))
        })
    };
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
        let r = &e.rows[row_of_line(&e.rows, e.line_of(at))];
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
    sync(host);
    if !outcome.changed {
        return;
    }
    let Some((value, handler, text)) = with_doc(|d| {
        let e = d.get(host)?.editor()?;
        Some((e.value, e.on_change.clone(), e.text.clone()))
    }) else {
        return;
    };
    batch(|| {
        if let Some(v) = value {
            v.set_if_changed(text.clone());
        }
        if let Some(h) = handler {
            h(&text);
        }
    });
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
                    let line = e.line_of(e.focus) as isize + if down { 1 } else { -1 };
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
            let slot = &e.rows[row_of_line(&e.rows, e.line_of(e.focus))];
            match (caret, laid_out(doc, slot)) {
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
    let ri = row_of_line(&e.rows, e.line_of(e.focus));
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
    let to = if down { ri + 1 } else { ri.wrapping_sub(1) };
    let Some(slot) = e.rows.get(to) else {
        return if down { e.text.len() } else { 0 };
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
        assert_eq!(e.text, "ab\ncX\nYd\nef");
        assert_eq!(e.starts, line_starts(&e.text));
        e.select(1, 8);
        e.delete(false, false);
        assert_eq!(e.text, "a\nef");
        assert_eq!(e.starts, line_starts(&e.text));
        e.undo();
        assert_eq!(e.starts, line_starts(&e.text));
        e.undo();
        assert_eq!(e.text, "ab\ncd\nef");
        assert_eq!(e.starts, line_starts(&e.text));
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
        assert_eq!(e.text, "helloa");
        e.undo();
        assert_eq!(e.text, "helloabc");
        e.undo();
        assert_eq!(e.text, "hello");
        assert_eq!(e.focus, 5);
        e.redo();
        e.redo();
        assert_eq!(e.text, "helloa");
        assert_eq!(e.focus, 6);
    }

    #[test]
    fn a_new_text_from_outside_is_one_change_and_the_caret_stays_by_its_text() {
        let mut e = editor("one two three");
        e.select(9, 9);
        e.set_text("one 2 three");
        assert_eq!(e.focus, 7);
        e.undo();
        assert_eq!(e.text, "one two three");
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
