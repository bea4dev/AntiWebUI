//! `code_editor`: `text_editor` with the colors of a tree-sitter grammar, line numbers, the keys of
//! a code editor, folding and a find bar. See `docs/design/31-code-editor.md`.

use crate::edits::{self, Replace};
use crate::highlight::{self, Done, Highlighter, NONE};
use crate::language::Language;
use crate::search::{self, Query};
use crate::theme::CodeTheme;
use anti_webui_core::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// Lines longer than this show only this much, then a mark: shaping a line of megabytes would
/// hold everything up (VS Code's `stopRenderingLineAfter`).
const LONG_LINE: usize = 10_000;
/// Texts longer than this aren't colored, as in VS Code.
pub const HIGHLIGHT_LIMIT: usize = 20 * 1024 * 1024;

/// What lies over a byte, besides its color.
mod over {
    pub const MATCH: u8 = 1;
    pub const CURRENT: u8 = 2;
    pub const BRACKET: u8 = 4;
}

type OnChange = Rc<dyn Fn(&str)>;
type OnScrolled = Rc<dyn Fn(PointF)>;
type OnKey = Rc<dyn Fn(&KeyboardEvent)>;

/// A text version and caret, and the brackets found at them.
type BracketsAt = (u64, usize, Option<(usize, usize)>);

/// A code editor.
pub struct CodeEditor {
    text: String,
    value: Option<Signal<String>>,
    language: Language,
    theme: CodeTheme,
    style: Style,
    on_change: Option<OnChange>,
    on_scrolled: Option<OnScrolled>,
    on_key_down: Option<OnKey>,
    editor_ref: Option<EditorRef>,
    read_only: bool,
    wrap: bool,
    line_numbers: bool,
    highlight_limit: usize,
}

/// A code editor that starts with `text`.
pub fn code_editor(text: impl Into<String>) -> CodeEditor {
    CodeEditor {
        text: text.into(),
        value: None,
        language: Language::PlainText,
        theme: CodeTheme::default(),
        style: Style::default(),
        on_change: None,
        on_scrolled: None,
        on_key_down: None,
        editor_ref: None,
        read_only: false,
        wrap: false,
        line_numbers: true,
        highlight_limit: HIGHLIGHT_LIMIT,
    }
}

impl Styled for CodeEditor {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl CodeEditor {
    pub fn language(mut self, language: Language) -> Self {
        self.language = language;
        self
    }

    pub fn theme(mut self, theme: CodeTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Called with the text whenever it changes.
    pub fn on_change(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// Two-way binds the text to a signal, instead of the text it starts with. The signal is given
    /// a copy of the whole text at every change.
    pub fn value(mut self, value: Signal<String>) -> Self {
        self.value = Some(value);
        self
    }

    /// Called when the text has scrolled, however that happened, with its offset in logical
    /// pixels.
    pub fn on_scrolled(mut self, f: impl Fn(PointF) + 'static) -> Self {
        self.on_scrolled = Some(Rc::new(f));
        self
    }

    /// Called with each key before the editor's own keys. Preventing the default keeps the editor
    /// from doing anything with it.
    pub fn on_key_down(mut self, f: impl Fn(&KeyboardEvent) + 'static) -> Self {
        self.on_key_down = Some(Rc::new(f));
        self
    }

    /// To change the text and the selection from outside, and to read the text.
    pub fn editor_ref(mut self, r: EditorRef) -> Self {
        self.editor_ref = Some(r);
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Long lines wrap instead of scrolling across.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn line_numbers(mut self, on: bool) -> Self {
        self.line_numbers = on;
        self
    }

    /// Texts longer than this many bytes aren't colored.
    pub fn highlight_limit(mut self, bytes: usize) -> Self {
        self.highlight_limit = bytes;
        self
    }
}

/// How a line was decorated, to give the same look when nothing changed.
#[derive(Clone, PartialEq)]
struct LookKey {
    runs: Vec<(u32, u32, u16, u8)>,
    current: bool,
    cut: Option<usize>,
    folded: Option<usize>,
}

/// The find bar.
#[derive(Default)]
struct Find {
    open: bool,
    query: Query,
    matches: Vec<Range<usize>>,
    /// The text version the matches were found in.
    version: Option<u64>,
    current: Option<usize>,
    error: Option<String>,
}

/// What the decorations share.
struct State {
    hl: Highlighter,
    theme: CodeTheme,
    styles: Vec<Rc<Style>>,
    by_name: HashMap<String, u16>,
    combined: HashMap<(u16, u8), Rc<Style>>,
    looks: HashMap<usize, (LookKey, Rc<LineLook>)>,
    current_line: Rc<Style>,
    find: Find,
    /// Folded lines: each fold's first line, which stays, and the line after its last.
    folds: Vec<Range<usize>>,
    /// The brackets at the caret, for the text version and caret they were found at.
    brackets: Option<BracketsAt>,
    language: Language,
}

impl State {
    fn style_id(
        by_name: &mut HashMap<String, u16>,
        styles: &mut Vec<Rc<Style>>,
        theme: &CodeTheme,
        name: &str,
    ) -> u16 {
        if let Some(&id) = by_name.get(name) {
            return id;
        }
        let id = match theme.style_of(name) {
            Some(s) => match styles.iter().position(|t| Rc::ptr_eq(t, &s)) {
                Some(i) => i as u16,
                None => {
                    styles.push(s);
                    (styles.len() - 1) as u16
                }
            },
            None => NONE,
        };
        by_name.insert(name.to_owned(), id);
        id
    }

    fn style(&mut self, id: u16, flags: u8) -> Rc<Style> {
        if flags == 0 {
            return self.styles[id as usize].clone();
        }
        let theme = &self.theme;
        let styles = &self.styles;
        self.combined
            .entry((id, flags))
            .or_insert_with(|| {
                let base = styles
                    .get(id as usize)
                    .map(|s| (**s).clone())
                    .unwrap_or_default();
                let bg = if flags & over::CURRENT != 0 {
                    theme.current_match_background
                } else if flags & over::MATCH != 0 {
                    theme.match_background
                } else {
                    theme.bracket_background
                };
                Rc::new(base.bg(bg))
            })
            .clone()
    }

    /// The brackets at the caret.
    fn brackets(&mut self, cx: &DecorateContext) -> Option<(usize, usize)> {
        if !cx.focused || cx.anchor != cx.focus {
            return None;
        }
        if let Some((v, at, b)) = self.brackets
            && v == cx.version
            && at == cx.focus
            && !self.hl.stale
        {
            return b;
        }
        let found = self
            .hl
            .tree
            .as_ref()
            .and_then(|t| highlight::brackets(t, cx.text, cx.focus));
        self.brackets = Some((cx.version, cx.focus, found));
        found
    }
}

/// The text of the fold mark at the end of a folded line.
fn fold_mark(theme: &CodeTheme, on_click: impl Fn() + 'static) -> View {
    div()
        .ml(6.0)
        .px(6.0)
        .rounded(4.0)
        .bg(theme.bracket_background)
        .text_color(theme.fold_marker)
        .cursor(CursorIcon::Pointer)
        .select_none()
        .on_click(move |_| on_click())
        .child(text("⋯"))
        .into_view()
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// How the lines of a range look.
fn decorate(
    state: &Rc<RefCell<State>>,
    editor: EditorRef,
    cx: &DecorateContext,
    lines: Range<usize>,
) -> Vec<Rc<LineLook>> {
    let mut guard = state.borrow_mut();
    let st = &mut *guard;
    st.hl.prepare(cx.text);
    let a = cx.line(lines.start).start;
    let last = cx.line(lines.end - 1);
    let b = (last.end + 1).min(cx.text.len()).max(a);
    let mut ids = vec![NONE; b - a];
    {
        let State {
            hl,
            by_name,
            styles,
            theme,
            ..
        } = st;
        hl.paint(cx.text, a..b, &mut ids, &mut |name| {
            State::style_id(by_name, styles, theme, name)
        });
    }
    let mut flags = vec![0u8; b - a];
    let mut mark = |r: &Range<usize>, f: u8| {
        let (s, e) = (r.start.max(a), r.end.min(b));
        if s < e {
            for x in &mut flags[s - a..e - a] {
                *x |= f;
            }
        }
    };
    if st.find.open {
        let m = &st.find.matches;
        let first = m.partition_point(|r| r.end <= a);
        for (i, r) in m[first..]
            .iter()
            .enumerate()
            .take_while(|(_, r)| r.start < b)
        {
            let f = if st.find.current == Some(first + i) {
                over::CURRENT
            } else {
                over::MATCH
            };
            mark(r, f);
        }
    }
    if let Some((p, q)) = st.brackets(cx) {
        mark(&(p..p + 1), over::BRACKET);
        mark(&(q..q + 1), over::BRACKET);
    }
    let caret_line = cx.line_of(cx.focus);
    let current = cx.focused && cx.anchor == cx.focus;

    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let r = cx.line(line);
        let len = r.len();
        let cut = (len > LONG_LINE).then(|| {
            let mut c = LONG_LINE;
            while !cx.text.is_char_boundary(r.start + c) {
                c -= 1;
            }
            c
        });
        let shown = cut.unwrap_or(len);
        let mut runs = Vec::new();
        let mut i = 0;
        while i < shown {
            let at = r.start + i - a;
            let (id, f) = (ids[at], flags[at]);
            let mut j = i + 1;
            while j < shown && ids[r.start + j - a] == id && flags[r.start + j - a] == f {
                j += 1;
            }
            if id != NONE || f != 0 {
                runs.push((i as u32, j as u32, id, f));
            }
            i = j;
        }
        let folded = st.folds.iter().find(|f| f.start == line).map(|f| f.end);
        let key = LookKey {
            runs,
            current: current && line == caret_line,
            cut,
            folded,
        };
        if let Some((k, look)) = st.looks.get(&line)
            && *k == key
        {
            out.push(look.clone());
            continue;
        }
        let marks = key
            .runs
            .iter()
            .map(|&(s, e, id, f)| (s as usize..e as usize, st.style(id, f)))
            .collect();
        let mut replace = Vec::new();
        if let Some(c) = cut {
            let more = len - c;
            let fg = st.theme.fold_marker;
            replace.push((
                c..len,
                Some(Widget::new(hash_of(("cut", line, more)), move || {
                    text(format!(" ⋯ {more} more bytes")).text_color(fg)
                })),
            ));
        }
        if let Some(end) = folded {
            let theme = st.theme.clone();
            let state = state.clone();
            replace.push((
                len..len,
                Some(Widget::new(hash_of(("fold", line, end)), move || {
                    let state = state.clone();
                    fold_mark(&theme, move || {
                        state.borrow_mut().folds.retain(|f| f.start != line);
                        editor.redecorate();
                    })
                })),
            ));
        }
        let look = Rc::new(LineLook {
            style: key.current.then(|| st.current_line.clone()),
            marks,
            replace,
            raised: Vec::new(),
        });
        st.looks.insert(line, (key, look.clone()));
        out.push(look);
    }
    // Forget the lines far from these.
    if st.looks.len() > 4 * out.len() + 256 {
        let keep = cx.line_of(a).saturating_sub(out.len())..cx.line_of(b) + out.len();
        st.looks.retain(|l, _| keep.contains(l));
    }
    out
}

/// The folded lines, as rows that hide them, but for those the selection is in, which open.
fn blocks(state: &Rc<RefCell<State>>, cx: &DecorateContext) -> Vec<(usize, Row)> {
    let mut st = state.borrow_mut();
    let sel = cx.selection();
    let (la, lb) = (cx.line_of(sel.start), cx.line_of(sel.end));
    let lines = cx.line_count();
    st.folds
        .retain(|f| f.end <= lines && !(la < f.end && lb > f.start));
    st.folds
        .iter()
        .map(|f| {
            (
                f.start + 1,
                Row::Block {
                    lines: f.end - f.start - 1,
                    style: None,
                    widget: Widget::new(hash_of(("hidden", f.start, f.end)), || div().h(0.0)),
                },
            )
        })
        .collect()
}

/// The folds after an edit: those after it move with their lines, those it touches open.
fn fold_after_edit(folds: &mut Vec<Range<usize>>, e: &TextEdit) {
    let (start, old_end, new_end) = (e.start_point.0, e.old_end_point.0, e.new_end_point.0);
    let delta = new_end as isize - old_end as isize;
    folds.retain_mut(|f| {
        if old_end < f.start || (start == f.start && old_end == f.start && new_end == f.start) {
            if old_end < f.start {
                f.start = (f.start as isize + delta) as usize;
                f.end = (f.end as isize + delta) as usize;
            }
            true
        } else {
            start >= f.end
        }
    });
}

/// Applies what a key does.
fn apply(editor: EditorRef, r: Replace) {
    match r.select {
        Some((a, b)) => editor.replace_and_select(r.range, &r.with, a, b),
        None => editor.replace(r.range, &r.with),
    }
}

/// What a key does, decided from the text.
enum Action {
    Replace(Replace),
    Find(Option<String>, bool),
    Step(bool),
    CloseFind,
    Fold(Range<usize>, usize),
    Unfold(usize),
    /// Taken, with nothing to do.
    Nothing,
}

/// What a key does to the editor, if it is one of a code editor's.
fn action(
    ed: &anti_webui_core::text::editor::EditorData,
    lang: Language,
    find_open: bool,
    e: &KeyboardEvent,
) -> Option<Action> {
    let m = e.modifiers;
    let text: &str = &ed.text;
    let (a, b) = ed.selection_range();
    let line = ed.line_of(ed.focus);
    let unit = lang.indent_unit();
    let ch = |c: &str| matches!(&e.key, Key::Character(k) if k.eq_ignore_ascii_case(c));
    let replace = |r: Option<Replace>| Some(r.map_or(Action::Nothing, Action::Replace));
    match &e.key {
        Key::Named(NamedKey::Enter) if !m.ctrl && !m.alt => {
            replace(Some(edits::enter(text, a, b, unit, lang.colon_opens())))
        }
        Key::Named(NamedKey::Tab) if !m.ctrl && !m.alt => {
            if m.shift || text[a..b].contains('\n') {
                replace(edits::indent_lines(text, a, b, unit, m.shift))
            } else {
                replace(Some(Replace {
                    range: a..b,
                    with: unit.to_owned(),
                    select: None,
                }))
            }
        }
        Key::Named(NamedKey::Backspace) if !m.ctrl && !m.alt && a == b => {
            edits::backspace(text, a, unit).map(Action::Replace)
        }
        Key::Named(NamedKey::Escape) if find_open => Some(Action::CloseFind),
        Key::Named(NamedKey::F(3)) => Some(Action::Step(!m.shift)),
        _ if m.ctrl && !m.alt && ch("/") => replace(edits::toggle_comment(
            text,
            a,
            b,
            lang.line_comment(),
            lang.block_comment(),
        )),
        _ if m.ctrl && !m.alt && (ch("f") || ch("h")) => {
            let selected = (a < b && !text[a..b].contains('\n')).then(|| text[a..b].to_owned());
            Some(Action::Find(selected, ch("h")))
        }
        _ if m.ctrl && m.shift && (ch("[") || ch("{")) => {
            let line_text = |i: usize| &text[ed.line(i)];
            Some(
                match edits::fold_around(&line_text, ed.line_count(), line) {
                    Some(f) => {
                        let start = ed.line(f.start).start;
                        let caret = start + edits::indentation(line_text(f.start)).len();
                        Action::Fold(f, caret)
                    }
                    None => Action::Nothing,
                },
            )
        }
        _ if m.ctrl && m.shift && (ch("]") || ch("}")) => Some(Action::Unfold(line)),
        Key::Character(c) if !m.ctrl && !m.alt && a == b => {
            let mut chars = c.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            edits::close_bracket(text, ed.focus, c, unit).map(Action::Replace)
        }
        _ => None,
    }
}

/// The keys of a code editor, before `text_editor` gets them. What they do is decided while
/// looking at the text, and done after, so that the text isn't held while it changes (which would
/// copy it).
fn keys(editor: EditorRef, state: &Rc<RefCell<State>>, find: &FindBar, e: &KeyboardEvent) {
    let lang = state.borrow().language;
    let open = find.open.get_untracked();
    let Some(Some(act)) = editor.with(|ed| action(ed, lang, open, e)) else {
        return;
    };
    e.prevent_default();
    match act {
        Action::Replace(r) => apply(editor, r),
        Action::Find(selected, replacing) => find.open(selected, replacing),
        Action::Step(forward) => find.step(forward),
        Action::CloseFind => find.close(),
        Action::Fold(f, caret) => {
            {
                let mut st = state.borrow_mut();
                st.folds.retain(|g| !(g.start < f.end && f.start < g.end));
                st.folds.push(f);
                st.folds.sort_by_key(|g| g.start);
            }
            // The caret goes to the line that stays.
            editor.select(caret, caret);
            editor.redecorate();
        }
        Action::Unfold(line) => {
            state
                .borrow_mut()
                .folds
                .retain(|f| !(f.start..f.end).contains(&line));
            editor.redecorate();
        }
        Action::Nothing => {}
    }
}

/// The find bar and what it does.
#[derive(Clone)]
struct FindBar {
    open: Signal<bool>,
    replacing: Signal<bool>,
    query: Signal<String>,
    with: Signal<String>,
    case: Signal<bool>,
    regex: Signal<bool>,
    count: Signal<String>,
    input: NodeRef,
    editor: EditorRef,
    state: Rc<RefCell<State>>,
}

impl FindBar {
    fn open(&self, selected: Option<String>, replacing: bool) {
        batch(|| {
            if let Some(s) = selected {
                self.query.set(s);
            }
            self.open.set(true);
            self.replacing.set(replacing);
        });
        self.input.focus();
        self.search(true);
    }

    fn close(&self) {
        self.open.set(false);
        self.state.borrow_mut().find.open = false;
        self.editor.redecorate();
        self.editor.focus();
    }

    /// Finds again in the text as it is, and goes to the match at or after the caret.
    fn search(&self, go: bool) {
        let q = Query {
            text: self.query.get_untracked(),
            case: self.case.get_untracked(),
            regex: self.regex.get_untracked(),
        };
        let Some((found, version, caret)) = self.editor.with(|e| {
            (
                search::find_all(&e.text, &q),
                e.version,
                e.selection_range().0,
            )
        }) else {
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            let f = &mut st.find;
            f.open = true;
            f.query = q;
            f.version = Some(version);
            match found {
                Ok(m) => {
                    f.current = search::next_from(&m, caret);
                    f.matches = m;
                    f.error = None;
                }
                Err(e) => {
                    f.matches.clear();
                    f.current = None;
                    f.error = Some(e);
                }
            }
        }
        self.show_count();
        if go {
            self.select_current();
        }
        self.editor.redecorate();
    }

    fn show_count(&self) {
        let st = self.state.borrow();
        let f = &st.find;
        let s = match (&f.error, f.current) {
            (Some(_), _) => "invalid".to_owned(),
            (None, _) if f.query.text.is_empty() => String::new(),
            (None, None) => "No results".to_owned(),
            (None, Some(i)) if f.matches.len() >= search::MOST_MATCHES => {
                format!("{} of {}+", i + 1, f.matches.len())
            }
            (None, Some(i)) => format!("{} of {}", i + 1, f.matches.len()),
        };
        self.count.set_if_changed(s);
    }

    fn select_current(&self) {
        let r = {
            let st = self.state.borrow();
            st.find
                .current
                .and_then(|i| st.find.matches.get(i).cloned())
        };
        if let Some(r) = r {
            self.editor.select(r.start, r.end);
        }
    }

    /// The next (or previous) match.
    fn step(&self, forward: bool) {
        let stale = {
            let st = self.state.borrow();
            st.find.version != self.editor.with(|e| e.version)
        };
        if stale || !self.state.borrow().find.open {
            self.search(false);
        }
        {
            let mut st = self.state.borrow_mut();
            let n = st.find.matches.len();
            if n == 0 {
                return;
            }
            st.find.current = Some(match st.find.current {
                Some(i) if forward => (i + 1) % n,
                Some(i) => (i + n - 1) % n,
                None => 0,
            });
        }
        self.show_count();
        self.select_current();
        self.editor.redecorate();
    }

    fn replace_one(&self) {
        let (r, q) = {
            let st = self.state.borrow();
            (
                st.find
                    .current
                    .and_then(|i| st.find.matches.get(i).cloned()),
                st.find.query.clone(),
            )
        };
        let Some(r) = r else {
            return;
        };
        let Some(with) = self
            .editor
            .with(|e| search::replacement(&e.text, r.clone(), &q, &self.with.get_untracked()))
        else {
            return;
        };
        self.editor.replace(r.clone(), &with);
        let at = r.start + with.len();
        self.editor.select(at, at);
        self.search(true);
    }

    fn replace_all(&self) {
        let (matches, q) = {
            let st = self.state.borrow();
            (st.find.matches.clone(), st.find.query.clone())
        };
        let (Some(first), Some(last)) = (matches.first(), matches.last()) else {
            return;
        };
        let with = self.with.get_untracked();
        let Some(new) = self.editor.with(|e| {
            let mut out = String::with_capacity(last.end - first.start);
            let mut at = first.start;
            for m in &matches {
                out.push_str(&e.text[at..m.start]);
                out.push_str(&search::replacement(&e.text, m.clone(), &q, &with));
                at = m.end;
            }
            out
        }) else {
            return;
        };
        // One step of undo for all of them.
        self.editor.replace(first.start..last.end, &new);
        self.search(false);
    }

    fn view(&self, theme: &CodeTheme) -> View {
        let bar = self.clone();
        let (fg, bg, line) = (theme.foreground, theme.background, theme.line_numbers);
        let field = move |value: Signal<String>, placeholder: &'static str| {
            text_input()
                .value(value)
                .placeholder(placeholder)
                .w(240.0)
                .px(6.0)
                .py(3.0)
                .rounded(4.0)
                .border(1.0)
                .border_color(line)
                .text_color(fg)
                .bg(bg)
        };
        let toggle = move |label: &'static str, on: Signal<bool>, then: FindBar| {
            div()
                .px(6.0)
                .py(2.0)
                .rounded(4.0)
                .cursor(CursorIcon::Pointer)
                .select_none()
                .text_color(fg)
                .dyn_style(move |s| if on.get() { s.bg(rgba(0x7f7f7f55)) } else { s })
                .on_click(move |_| {
                    on.set(!on.get_untracked());
                    then.search(true);
                })
                .child(text(label))
        };
        let button = move |label: &'static str, f: Rc<dyn Fn()>| {
            div()
                .px(6.0)
                .py(2.0)
                .rounded(4.0)
                .cursor(CursorIcon::Pointer)
                .select_none()
                .text_color(fg)
                .hover(|s| s.bg(rgba(0x7f7f7f33)))
                .on_click(move |_| f())
                .child(text(label))
        };
        let b = bar.clone();
        let input = field(self.query, "Find")
            .node_ref(self.input)
            .on_key_down(move |e| {
                if let Key::Named(NamedKey::Enter) = e.key {
                    e.prevent_default();
                    b.step(!e.modifiers.shift);
                } else if let Key::Named(NamedKey::Escape) = e.key {
                    e.prevent_default();
                    b.close();
                }
            });
        let b = bar.clone();
        let input = input.on_input(move |_| b.search(true));
        let b = bar.clone();
        let with_value = self.with;
        let with_input = move || {
            let b = b.clone();
            field(with_value, "Replace").on_key_down(move |e| {
                if let Key::Named(NamedKey::Enter) = e.key {
                    e.prevent_default();
                    b.replace_one();
                } else if let Key::Named(NamedKey::Escape) = e.key {
                    e.prevent_default();
                    b.close();
                }
            })
        };
        let (b1, b2, b3, b4, b5) = (
            bar.clone(),
            bar.clone(),
            bar.clone(),
            bar.clone(),
            bar.clone(),
        );
        let replacing = self.replacing;
        let count = self.count;
        let view = div()
            .flex()
            .flex_col()
            .gap(4.0)
            .px(8.0)
            .py(6.0)
            .bg(bg)
            .text_size(13.0)
            .border_b(1.0)
            .border_color(line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(4.0)
                    .child(input)
                    .child(toggle("Aa", self.case, bar.clone()))
                    .child(toggle(".*", self.regex, bar.clone()))
                    .child(
                        div()
                            .min_w(80.0)
                            .text_color(line)
                            .child(text(move || count.get())),
                    )
                    .child(button("↑", Rc::new(move || b1.step(false))))
                    .child(button("↓", Rc::new(move || b2.step(true))))
                    .child(button(
                        "⇄",
                        Rc::new(move || replacing.set(!replacing.get_untracked())),
                    ))
                    .child(button("✕", Rc::new(move || b3.close()))),
            )
            .child(show(
                move || replacing.get(),
                move || {
                    let (b4, b5) = (b4.clone(), b5.clone());
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(4.0)
                        .child(with_input())
                        .child(button("Replace", Rc::new(move || b4.replace_one())))
                        .child(button("All", Rc::new(move || b5.replace_all())))
                },
            ))
            .into_view();
        // It is made when the bar opens, which wants what is typed to go into it.
        self.input.focus();
        view
    }
}

impl IntoView for CodeEditor {
    fn into_view(self) -> View {
        let editor = self.editor_ref.unwrap_or_else(editor_ref);
        let theme = self.theme.clone();
        let current_line = Rc::new(Style::default().bg(theme.current_line));
        let state = Rc::new(RefCell::new(State {
            hl: Highlighter::new(self.language, self.highlight_limit, Arc::new(|_| {})),
            theme: theme.clone(),
            styles: Vec::new(),
            by_name: HashMap::new(),
            combined: HashMap::new(),
            looks: HashMap::new(),
            current_line,
            find: Find::default(),
            folds: Vec::new(),
            brackets: None,
            language: self.language,
        }));
        // What the worker finds comes back here.
        let st = state.clone();
        let sender = ui_channel(move |done: Done| {
            if st.borrow_mut().hl.done(done) {
                editor.redecorate();
            }
        });
        {
            let sender = sender.clone();
            let send: highlight::SendFn = Arc::new(move |d| sender.send(d));
            let mut st = state.borrow_mut();
            let (lang, limit) = (st.hl.language, st.hl.limit);
            st.hl = Highlighter::new(lang, limit, send);
        }
        let find = FindBar {
            open: signal(false),
            replacing: signal(false),
            query: signal(String::new()),
            with: signal(String::new()),
            case: signal(false),
            regex: signal(false),
            count: signal(String::new()),
            input: node_ref(),
            editor,
            state: state.clone(),
        };

        let gutter = self.line_numbers.then(|| Gutter {
            style: Rc::new(
                Style::default()
                    .bg(theme.background)
                    .text_color(theme.line_numbers),
            ),
            current: Some(Rc::new(
                Style::default().text_color(theme.current_line_number),
            )),
            digits: 3,
            padding: (12.0, 16.0),
        });
        let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
        let f1 = find.clone();
        let f2 = find.clone();
        let on_change = self.on_change.clone();
        let on_key = self.on_key_down.clone();
        let mut te = text_editor()
            .text(self.text)
            .editor_ref(editor)
            .read_only(self.read_only)
            .font_family(theme.font())
            .text_size(theme.font_size)
            .line_height(LineHeight::Relative(1.5))
            .text_color(theme.foreground)
            .bg(theme.background)
            .selection_color(theme.selection)
            .caret_color(theme.foreground)
            .flex_1()
            .min_h(0.0)
            .overflow_scroll()
            .pt(4.0)
            .decorate_lines(move |cx, lines| decorate(&s1, editor, cx, lines))
            .blocks(move |cx| blocks(&s2, cx))
            .on_edit(move |e| {
                let mut st = s3.borrow_mut();
                st.hl.edit(&highlight::input_edit(e));
                fold_after_edit(&mut st.folds, e);
                st.brackets = None;
            })
            .on_change(move |t| {
                if f1.open.get_untracked() {
                    f1.search(false);
                }
                if let Some(f) = &on_change {
                    f(t);
                }
            })
            .on_key_down(move |e| {
                if let Some(f) = &on_key {
                    f(e);
                    if e.is_default_prevented() {
                        return;
                    }
                }
                keys(editor, &s4, &f2, e)
            });
        if let Some(v) = self.value {
            te = te.value(v);
        }
        if let Some(f) = self.on_scrolled {
            te = te.on_scrolled(move |p| f(p));
        }
        if !self.wrap {
            te = te.whitespace_nowrap();
        }
        if let Some(g) = gutter {
            te = te.gutter(g);
        }
        let open = find.open;
        let bar_theme = theme.clone();
        div()
            .flex()
            .flex_col()
            .overflow_clip()
            .bg(theme.background)
            .style(&self.style)
            .child(show(move || open.get(), move || find.view(&bar_theme)))
            .child(te)
            .into_view()
    }
}
