//! The editing model for text inputs (see `docs/design/04-text.md`).

use super::{Affinity, Cursor, LayoutSelection, ShapeStyle, TextLayout, TextSystem};
use crate::event::ImeUpdate;
use crate::geometry::{PointI, RectI, SizeI};
use crate::platform::{ImePurpose, ImeState};
use crate::reactive::Signal;
use crate::text::selection::Granularity;
use anti_webui_style::TextAlign;
use std::rc::Rc;
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;

/// A handler that receives the current text.
pub type TextCallback = Rc<dyn Fn(&str)>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

#[derive(Clone)]
struct Snapshot {
    text: String,
    anchor: usize,
    focus: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

pub struct EditData {
    pub text: String,
    pub version: u64,
    pub anchor: usize,
    pub focus: usize,
    pub affinity: Affinity,
    pub preedit: Option<Preedit>,
    pub multiline: bool,
    pub rows: u32,
    pub placeholder: String,
    pub purpose: ImePurpose,
    pub read_only: bool,

    pub layout: TextLayout,
    pub placeholder_layout: TextLayout,
    metrics_layout: TextLayout,
    display_version: u64,
    /// Cached parley selection, which remembers the horizontal position for up/down moves.
    nav: Option<(u64, LayoutSelection)>,
    pub content_size: SizeI,
    /// The width and alignment chosen by the last layout pass.
    pub(crate) final_layout: Option<(i32, TextAlign)>,
    /// Internal scroll offset in physical pixels.
    pub scroll: PointI,
    pub scroll_to_caret: bool,
    pub caret_epoch: Instant,
    pub drag: Option<(Granularity, usize, usize)>,
    /// Where what is dragged over it would go.
    pub drop_caret: Option<usize>,

    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: EditKind,

    pub value: Option<Signal<String>>,
    pub on_input: Option<TextCallback>,
    pub on_submit: Option<TextCallback>,
}

/// What the caller has to do after an edit (outside of the document borrow).
#[derive(Default)]
pub struct EditOutcome {
    pub changed: bool,
    pub moved: bool,
}

fn prev_grapheme(text: &str, i: usize) -> usize {
    text[..i]
        .grapheme_indices(true)
        .next_back()
        .map(|(j, _)| j)
        .unwrap_or(0)
}

fn next_grapheme(text: &str, i: usize) -> usize {
    text[i..]
        .grapheme_indices(true)
        .nth(1)
        .map(|(j, _)| i + j)
        .unwrap_or(text.len())
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

impl EditData {
    pub fn new(multiline: bool) -> Self {
        Self {
            text: String::new(),
            version: 0,
            anchor: 0,
            focus: 0,
            affinity: Affinity::Downstream,
            preedit: None,
            multiline,
            rows: if multiline { 3 } else { 1 },
            placeholder: String::new(),
            purpose: ImePurpose::Normal,
            read_only: false,
            layout: TextLayout::default(),
            placeholder_layout: TextLayout::default(),
            metrics_layout: TextLayout::default(),
            display_version: 0,
            nav: None,
            content_size: SizeI::default(),
            final_layout: None,
            scroll: PointI::default(),
            scroll_to_caret: false,
            caret_epoch: Instant::now(),
            drag: None,
            drop_caret: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: EditKind::Other,
            value: None,
            on_input: None,
            on_submit: None,
        }
    }

    fn bump(&mut self) {
        self.version += 1;
        self.display_version += 1;
        self.nav = None;
        self.scroll_to_caret = true;
        self.caret_epoch = Instant::now();
    }

    fn moved(&mut self) {
        self.nav = None;
        self.scroll_to_caret = true;
        self.caret_epoch = Instant::now();
    }

    /// Replaces the whole text (e.g. from a bound signal) without recording undo.
    pub fn set_text(&mut self, text: String) {
        if self.text == text {
            return;
        }
        self.text = text;
        self.anchor = self.anchor.min(self.text.len());
        self.focus = self.focus.min(self.text.len());
        self.anchor = floor_char_boundary(&self.text, self.anchor);
        self.focus = floor_char_boundary(&self.text, self.focus);
        self.bump();
    }

    /// Puts the caret after all of the text.
    pub fn caret_to_end(&mut self) {
        self.anchor = self.text.len();
        self.focus = self.text.len();
        self.scroll_to_caret = true;
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

    fn preedit_len(&self) -> usize {
        self.preedit.as_ref().map(|p| p.text.len()).unwrap_or(0)
    }

    /// The text that is laid out: the model text with the pre-edit inserted at the caret.
    pub fn display_text(&self) -> String {
        match &self.preedit {
            Some(p) if !p.text.is_empty() => {
                let mut s = String::with_capacity(self.text.len() + p.text.len());
                s.push_str(&self.text[..self.focus]);
                s.push_str(&p.text);
                s.push_str(&self.text[self.focus..]);
                s
            }
            _ => self.text.clone(),
        }
    }

    /// Model offset → display offset.
    pub fn to_display(&self, i: usize) -> usize {
        if i > self.focus {
            i + self.preedit_len()
        } else {
            i
        }
    }

    /// Display offset → model offset (offsets inside the pre-edit map to the caret).
    pub fn to_model(&self, i: usize) -> usize {
        let pl = self.preedit_len();
        if i <= self.focus {
            i
        } else if i < self.focus + pl {
            self.focus
        } else {
            i - pl
        }
    }

    /// Shapes and breaks the display text. Returns the content size.
    pub fn layout_text(
        &mut self,
        ts: &mut TextSystem,
        style: &ShapeStyle,
        width: Option<i32>,
    ) -> SizeI {
        let display = self.display_text();
        self.layout.shape(ts, &display, self.display_version, style);
        self.layout.break_lines(width);
        self.metrics_layout.shape(ts, " ", 0, style);
        self.metrics_layout.break_lines(None);
        if !self.placeholder.is_empty() {
            self.placeholder_layout
                .shape(ts, &self.placeholder, 0, style);
            self.placeholder_layout.break_lines(width);
        }
        let mut size = self.layout.size();
        size.height = size.height.max(self.line_height());
        size
    }

    pub fn align(&mut self, width: i32, align: TextAlign) {
        if self.multiline || self.content_size.width <= width {
            self.layout.align(width, align);
            if !self.placeholder.is_empty() {
                self.placeholder_layout.align(width, align);
            }
        }
    }

    pub fn line_height(&self) -> i32 {
        self.metrics_layout
            .first_line_box()
            .map(|(a, b)| b - a)
            .unwrap_or(16)
    }

    /// Caret rectangle relative to the text origin (display coordinates).
    pub fn caret_rect(&self, width: i32) -> RectI {
        let pre = self.preedit.as_ref();
        let index = match pre.and_then(|p| p.cursor) {
            Some((_, end)) => self.focus + end,
            None => {
                self.to_display(self.focus) + if pre.is_some() { self.preedit_len() } else { 0 }
            }
        };
        if self.layout.layout.text_len() == 0 {
            let lh = self.line_height();
            let x = match self.layout.layout.lines().next() {
                Some(l) => l.metrics().offset.round() as i32,
                None => 0,
            };
            return RectI::new(x, 0, width, lh);
        }
        self.layout.caret_rect(index, self.affinity, width)
    }

    /// Caret rectangle for what is dragged over it, relative to the text origin.
    pub fn drop_caret_rect(&self, at: usize, width: i32) -> RectI {
        if self.layout.layout.text_len() == 0 {
            return self.caret_rect(width);
        }
        self.layout
            .caret_rect(self.to_display(at), Affinity::Downstream, width)
    }

    fn nav_selection(&mut self) -> LayoutSelection {
        if let Some((v, sel)) = &self.nav
            && *v == self.display_version
        {
            return *sel;
        }
        let a = Cursor::from_byte_index(&self.layout.layout, self.anchor, Affinity::Downstream);
        let f = Cursor::from_byte_index(&self.layout.layout, self.focus, self.affinity);
        LayoutSelection::new(a, f)
    }

    fn apply_nav(&mut self, sel: LayoutSelection) {
        let r_anchor = sel.anchor().index().min(self.text.len());
        let r_focus = sel.focus().index().min(self.text.len());
        self.anchor = r_anchor;
        self.focus = r_focus;
        self.affinity = sel.focus().affinity();
        self.nav = Some((self.display_version, sel));
        self.scroll_to_caret = true;
        self.caret_epoch = Instant::now();
        self.last_edit = EditKind::Other;
    }

    pub fn move_caret(&mut self, motion: Motion, extend: bool, page_lines: isize) -> EditOutcome {
        if self.preedit.is_some() {
            return EditOutcome::default();
        }
        if !extend && self.has_selection() {
            let (a, b) = self.selection_range();
            let collapse_to = match motion {
                Motion::Left | Motion::WordLeft => Some(a),
                Motion::Right | Motion::WordRight => Some(b),
                _ => None,
            };
            if let Some(p) = collapse_to {
                self.anchor = p;
                self.focus = p;
                self.moved();
                return EditOutcome {
                    changed: false,
                    moved: true,
                };
            }
        }
        let sel = self.nav_selection();
        let layout_ref = &self.layout.layout;
        let next = match motion {
            Motion::Left => sel.previous_visual(layout_ref, extend),
            Motion::Right => sel.next_visual(layout_ref, extend),
            Motion::WordLeft => sel.previous_visual_word(layout_ref, extend),
            Motion::WordRight => sel.next_visual_word(layout_ref, extend),
            Motion::Up => sel.previous_line(layout_ref, extend),
            Motion::Down => sel.next_line(layout_ref, extend),
            Motion::LineStart => sel.line_start(layout_ref, extend),
            Motion::LineEnd => sel.line_end(layout_ref, extend),
            Motion::PageUp => sel.move_lines(layout_ref, -page_lines, extend),
            Motion::PageDown => sel.move_lines(layout_ref, page_lines, extend),
            Motion::DocStart | Motion::DocEnd => {
                let i = if motion == Motion::DocStart {
                    0
                } else {
                    self.text.len()
                };
                let c = Cursor::from_byte_index(layout_ref, i, Affinity::Downstream);
                if extend {
                    sel.extend(c)
                } else {
                    LayoutSelection::from(c)
                }
            }
        };
        self.apply_nav(next);
        EditOutcome {
            changed: false,
            moved: true,
        }
    }

    fn snapshot(&mut self, kind: EditKind) {
        // Consecutive typing or deleting coalesces into one undo step.
        if kind != EditKind::Other && kind == self.last_edit {
            return;
        }
        self.undo.push(Snapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            focus: self.focus,
        });
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_edit = kind;
    }

    fn replace_range(&mut self, a: usize, b: usize, s: &str) {
        self.text.replace_range(a..b, s);
        let p = a + s.len();
        self.anchor = p;
        self.focus = p;
        self.affinity = Affinity::Downstream;
        self.bump();
    }

    /// What is put in becomes this: a single line has no line breaks.
    fn cleaned(&self, s: &str) -> String {
        if self.multiline {
            s.replace("\r\n", "\n")
        } else {
            s.replace(['\r', '\n'], " ")
        }
    }

    /// Replaces the selection with `s`.
    pub fn insert(&mut self, s: &str) -> EditOutcome {
        if self.read_only {
            return EditOutcome::default();
        }
        let s = self.cleaned(s);
        if s.is_empty() && !self.has_selection() {
            return EditOutcome::default();
        }
        let kind = if s.contains([' ', '\n']) || self.has_selection() {
            EditKind::Other
        } else {
            EditKind::Insert
        };
        self.snapshot(kind);
        let (a, b) = self.selection_range();
        self.replace_range(a, b, &s);
        EditOutcome {
            changed: true,
            moved: true,
        }
    }

    /// Selects `len` bytes at `at`, after the text there changed.
    fn select_changed(&mut self, at: usize, len: usize) {
        self.anchor = at;
        self.focus = at + len;
        self.affinity = Affinity::Downstream;
        self.preedit = None;
        self.bump();
    }

    /// Puts what was dropped at `at`, and selects it.
    pub fn drop_text(&mut self, at: usize, s: &str) -> EditOutcome {
        let s = self.cleaned(s);
        if self.read_only || s.is_empty() {
            return EditOutcome::default();
        }
        self.snapshot(EditKind::Other);
        let at = floor_char_boundary(&self.text, at);
        self.text.insert_str(at, &s);
        self.select_changed(at, s.len());
        EditOutcome {
            changed: true,
            moved: true,
        }
    }

    /// Moves the selection to `at`, in one step of undo, and keeps it selected.
    pub fn move_selection(&mut self, at: usize) -> EditOutcome {
        let (a, b) = self.selection_range();
        if self.read_only || a == b || (a..=b).contains(&at) {
            return EditOutcome::default();
        }
        self.snapshot(EditKind::Other);
        let moved = self.text[a..b].to_owned();
        self.text.replace_range(a..b, "");
        let at = floor_char_boundary(&self.text, if at > b { at - moved.len() } else { at });
        self.text.insert_str(at, &moved);
        self.select_changed(at, moved.len());
        EditOutcome {
            changed: true,
            moved: true,
        }
    }

    pub fn delete(&mut self, forward: bool, word: bool) -> EditOutcome {
        if self.read_only {
            return EditOutcome::default();
        }
        let (mut a, mut b) = self.selection_range();
        if a == b {
            if word {
                let c =
                    Cursor::from_byte_index(&self.layout.layout, self.focus, Affinity::Downstream);
                let t = if forward {
                    c.next_logical_word(&self.layout.layout)
                } else {
                    c.previous_logical_word(&self.layout.layout)
                };
                let t = t.index().min(self.text.len());
                if forward { b = t } else { a = t }
            } else if forward {
                b = next_grapheme(&self.text, b);
            } else {
                a = prev_grapheme(&self.text, a);
            }
        }
        if a == b {
            return EditOutcome::default();
        }
        self.snapshot(if word {
            EditKind::Other
        } else {
            EditKind::Delete
        });
        self.replace_range(a, b, "");
        EditOutcome {
            changed: true,
            moved: true,
        }
    }

    /// Selects from one byte offset to another, kept inside of the text and on its characters.
    pub fn select_range(&mut self, anchor: usize, focus: usize) {
        self.anchor = floor_char_boundary(&self.text, anchor.min(self.text.len()));
        self.focus = floor_char_boundary(&self.text, focus.min(self.text.len()));
        self.moved();
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.focus = self.text.len();
        self.moved();
    }

    pub fn undo(&mut self) -> EditOutcome {
        let Some(s) = self.undo.pop() else {
            return EditOutcome::default();
        };
        self.redo.push(Snapshot {
            text: std::mem::replace(&mut self.text, s.text),
            anchor: self.anchor,
            focus: self.focus,
        });
        self.anchor = s.anchor;
        self.focus = s.focus;
        self.last_edit = EditKind::Other;
        self.bump();
        EditOutcome {
            changed: true,
            moved: true,
        }
    }

    pub fn redo(&mut self) -> EditOutcome {
        let Some(s) = self.redo.pop() else {
            return EditOutcome::default();
        };
        self.undo.push(Snapshot {
            text: std::mem::replace(&mut self.text, s.text),
            anchor: self.anchor,
            focus: self.focus,
        });
        self.anchor = s.anchor;
        self.focus = s.focus;
        self.last_edit = EditKind::Other;
        self.bump();
        EditOutcome {
            changed: true,
            moved: true,
        }
    }

    /// Hit-tests a point relative to the text origin; returns a model offset.
    pub fn hit(&self, x: f32, y: f32) -> (usize, Affinity) {
        let (i, aff) = self.layout.hit(x, y);
        (self.to_model(i).min(self.text.len()), aff)
    }

    /// Mouse press at a point relative to the text origin.
    pub fn pointer_down(&mut self, x: f32, y: f32, clicks: u32, extend: bool) {
        let (i, aff) = self.hit(x, y);
        let granularity = match clicks {
            1 => Granularity::Char,
            2 => Granularity::Word,
            _ => Granularity::Line,
        };
        let (a, b) = match granularity {
            Granularity::Char => (i, i),
            Granularity::Word => self.layout.word_at(x, y),
            Granularity::Line => {
                if self.multiline {
                    self.layout.hard_line_at(x, y)
                } else {
                    (0, self.text.len())
                }
            }
        };
        let (a, b) = (self.to_model(a), self.to_model(b));
        if extend && granularity == Granularity::Char {
            self.focus = i;
        } else {
            self.anchor = a;
            self.focus = b;
        }
        self.affinity = aff;
        self.drag = Some((granularity, a, b));
        self.last_edit = EditKind::Other;
        self.moved();
    }

    pub fn pointer_drag(&mut self, x: f32, y: f32) {
        let Some((granularity, ia, ib)) = self.drag else {
            return;
        };
        let (i, aff) = self.hit(x, y);
        match granularity {
            Granularity::Char => self.focus = i,
            Granularity::Word | Granularity::Line => {
                let (a, b) = if granularity == Granularity::Word {
                    self.layout.word_at(x, y)
                } else if self.multiline {
                    self.layout.hard_line_at(x, y)
                } else {
                    (0, self.text.len())
                };
                let (a, b) = (self.to_model(a), self.to_model(b));
                if a < ia {
                    self.anchor = ib;
                    self.focus = a;
                } else {
                    self.anchor = ia;
                    self.focus = b;
                }
            }
        }
        self.affinity = aff;
        self.moved();
    }

    /// Applies an input-method update in the order required by text-input-v3.
    pub fn apply_ime(&mut self, update: &ImeUpdate) -> EditOutcome {
        let mut out = EditOutcome::default();
        let had_preedit = self.preedit.take().is_some();
        if had_preedit {
            self.display_version += 1;
            out.moved = true;
        }
        if let Some((before, after)) = update.delete_surrounding {
            let (a, b) = self.selection_range();
            let start = floor_char_boundary(&self.text, a.saturating_sub(before));
            let end = floor_char_boundary(&self.text, (b + after).min(self.text.len()));
            if start < a || end > b {
                self.snapshot(EditKind::Other);
                self.text.replace_range(b..end, "");
                self.text.replace_range(start..a, "");
                self.anchor = start;
                self.focus = start + (b - a);
                self.bump();
                out.changed = true;
            }
        }
        if let Some(commit) = &update.commit {
            let r = self.insert(commit);
            out.changed |= r.changed;
            out.moved = true;
        }
        if let Some((text, cursor)) = &update.preedit
            && !text.is_empty()
        {
            if self.has_selection() && !self.read_only {
                let (a, b) = self.selection_range();
                self.snapshot(EditKind::Other);
                self.replace_range(a, b, "");
                out.changed = true;
            }
            self.preedit = Some(Preedit {
                text: text.clone(),
                cursor: *cursor,
            });
            self.display_version += 1;
            self.moved();
            out.moved = true;
        }
        if out.moved {
            self.moved();
        }
        out
    }

    /// The state reported to the input method.
    pub fn ime_state(&self, caret: RectI) -> ImeState {
        const MAX: usize = 4000;
        let (a, b) = (self.anchor, self.focus);
        let lo = a.min(b);
        let hi = a.max(b);
        let mut start = lo.saturating_sub(MAX / 2);
        let mut end = (hi + MAX / 2).min(self.text.len());
        if end - start > MAX {
            end = start + MAX;
        }
        start = floor_char_boundary(&self.text, start);
        end = floor_char_boundary(&self.text, end);
        let surrounding = self.text[start..end].to_owned();
        let clamp = |i: usize| i.clamp(start, end) - start;
        ImeState {
            surrounding,
            cursor: clamp(self.focus),
            anchor: clamp(self.anchor),
            cursor_rect: caret,
            multiline: self.multiline,
            purpose: self.purpose,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selecting(text: &str, a: usize, b: usize) -> EditData {
        let mut e = EditData::new(true);
        e.text = text.to_owned();
        e.anchor = a;
        e.focus = b;
        e
    }

    #[test]
    fn moving_the_selection_forward_goes_by_what_was_there() {
        let mut e = selecting("alpha beta gamma", 0, 5);
        assert!(e.move_selection(10).changed);
        assert_eq!(e.text, " betaalpha gamma");
        assert_eq!(e.selected_text(), "alpha");
    }

    #[test]
    fn moving_the_selection_back_goes_by_what_was_there() {
        let mut e = selecting("alpha beta gamma", 11, 16);
        assert!(e.move_selection(6).changed);
        assert_eq!(e.text, "alpha gammabeta ");
        assert_eq!(e.selected_text(), "gamma");
    }

    #[test]
    fn moving_it_is_one_step_of_undo_and_nothing_on_itself() {
        let mut e = selecting("alpha beta", 0, 5);
        for at in [0, 3, 5] {
            assert!(!e.move_selection(at).changed);
        }
        assert_eq!(e.text, "alpha beta");
        e.move_selection(10);
        e.undo();
        assert_eq!(e.text, "alpha beta");
        assert_eq!(e.selected_text(), "alpha");
    }

    #[test]
    fn moving_keeps_to_characters() {
        let mut e = selecting("日本語テキスト", 0, 3);
        e.move_selection(9);
        assert_eq!(e.text, "本語日テキスト");
        assert_eq!(e.selected_text(), "日");
    }

    #[test]
    fn what_was_dropped_ends_up_selected_where_it_was_dropped() {
        let mut e = selecting("abcdef", 1, 2);
        assert!(e.drop_text(4, "XY").changed);
        assert_eq!(e.text, "abcdXYef");
        assert_eq!(e.selected_text(), "XY");
        e.undo();
        assert_eq!(e.text, "abcdef");
    }

    #[test]
    fn read_only_text_takes_nothing() {
        let mut e = selecting("abcdef", 1, 2);
        e.read_only = true;
        assert!(!e.drop_text(4, "XY").changed);
        assert!(!e.move_selection(4).changed);
        assert_eq!(e.text, "abcdef");
    }

    #[test]
    fn grapheme_navigation() {
        let s = "aé👍🏽b";
        assert_eq!(next_grapheme(s, 0), 1);
        assert_eq!(next_grapheme(s, 1), 1 + "é".len());
        let thumbs = 1 + "é".len();
        assert_eq!(next_grapheme(s, thumbs), thumbs + "👍🏽".len());
        assert_eq!(prev_grapheme(s, s.len() - 1), thumbs);
    }

    #[test]
    fn preedit_offset_mapping() {
        let mut e = EditData::new(false);
        e.text = "abcd".into();
        e.anchor = 2;
        e.focus = 2;
        e.preedit = Some(Preedit {
            text: "XYZ".into(),
            cursor: None,
        });
        assert_eq!(e.display_text(), "abXYZcd");
        assert_eq!(e.to_display(3), 6);
        assert_eq!(e.to_model(3), 2);
        assert_eq!(e.to_model(6), 3);
    }

    #[test]
    fn ime_commit_replaces_selection_and_deletes_surrounding() {
        let mut e = EditData::new(false);
        e.text = "hello world".into();
        e.anchor = 5;
        e.focus = 5;
        e.apply_ime(&ImeUpdate {
            preedit: None,
            commit: Some("!".into()),
            delete_surrounding: Some((5, 0)),
        });
        assert_eq!(e.text, "! world");
        assert_eq!(e.focus, 1);
    }

    #[test]
    fn undo_coalesces_typing() {
        let mut e = EditData::new(false);
        e.insert("a");
        e.insert("b");
        e.insert("c");
        assert_eq!(e.text, "abc");
        e.undo();
        assert_eq!(e.text, "");
        e.redo();
        assert_eq!(e.text, "abc");
    }
}
