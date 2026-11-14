//! A Markdown editor that shows what it says while it is written, as Obsidian's live preview does:
//! `text_editor` with decorations made from the Markdown. The marks hide unless the caret touches
//! what they mark, and diagrams, tables, rules, pictures and check boxes show as they will look
//! until the caret goes into them. See "M4" in `docs/design/28-markdown.md`.

use crate::parse::{options, scripts};
use crate::theme::MarkdownTheme;
use crate::view::{Ctx, Highlight, OnLink, Pictures};
use anti_webui_core::event::KeyboardEvent;
use anti_webui_core::prelude::*;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser, Tag, TagEnd};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

/// What the letters of a part of a line are marked as.
mod bit {
    pub const STRONG: u16 = 1;
    pub const EMPHASIS: u16 = 2;
    pub const STRIKE: u16 = 4;
    pub const CODE: u16 = 8;
    pub const LINK: u16 = 16;
    /// A mark that shows because the caret is at what it marks.
    pub const SYNTAX: u16 = 32;
    /// The text of a task that is done.
    pub const DONE: u16 = 64;
    pub const SUPER: u16 = 128;
    pub const SUB: u16 = 256;
}

/// What a line is part of, for the style of its box.
mod kind {
    pub const QUOTE: u16 = 1;
    pub const CODE: u16 = 2;
    /// The first and last lines of a fenced code block.
    pub const FENCE_TOP: u16 = 4;
    pub const FENCE_BOTTOM: u16 = 8;
    /// The source of a table, of HTML or of metadata: letters of one width.
    pub const MONO: u16 = 16;
    /// Which alert of GitHub a quote is, from 1, in the three bits from here.
    pub const ALERT_SHIFT: u16 = 5;
    /// The definition of a footnote.
    pub const FOOTNOTE: u16 = 1 << 12;
    /// A heading's level, in the bits from here.
    pub const HEADING_SHIFT: u16 = 8;
}

const SYNTAX_COLOR: Color = Color::from_rgb(0x8c959f);

/// How a line is made up, while a block is looked at.
#[derive(Default, Clone)]
struct LineBuild {
    kind: u16,
    /// Parts and what they are marked as; they may overlap.
    bits: Vec<(Range<usize>, u16)>,
    /// Parts with a style given from outside (a code block's colors).
    styled: Vec<(Range<usize>, Rc<Style>)>,
    replace: Vec<(Range<usize>, Option<Widget>)>,
}

/// The rows of a top-level block, as they were last made.
struct Entry {
    rows: Rc<Vec<Row>>,
    /// Where the block starts in the text now, for what its widgets change.
    start: Rc<Cell<usize>>,
}

/// Which block an entry is for: its source, which of the blocks with that source it is, and where
/// the selection touches it.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    hash: u64,
    nth: u32,
    selection: Option<(usize, usize)>,
    /// The whole selection is in it.
    holds: bool,
}

/// The blocks at the top of a document, with the hashes of their lines.
type Blocks = Vec<(Range<usize>, u64)>;

/// What is known of the whole document, for the version of its text it was found in.
#[derive(Default)]
struct Tops {
    version: Option<u64>,
    blocks: Blocks,
    /// The definitions of its footnotes, empty, to put after each block that is looked at alone,
    /// so that what refers to them is known as a reference.
    footnotes: String,
    /// The numbers of its formulas on lines of their own.
    numbers: Rc<crate::numbers::Numbers>,
}

fn hash_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Makes the rows of a document, keeping those of blocks that didn't change.
pub(crate) struct Decorator {
    theme: MarkdownTheme,
    ctx: Rc<Ctx>,
    highlight: Option<Rc<Highlight>>,
    editor: EditorRef,
    marks: RefCell<HashMap<u16, Rc<Style>>>,
    kinds: RefCell<HashMap<u16, Option<Rc<Style>>>>,
    plain: Rc<LineLook>,
    entries: RefCell<HashMap<Key, Entry>>,
    /// The blocks at the top and the hashes of their lines, for the version of the text they were
    /// found in.
    tops: RefCell<Tops>,
    uid: Cell<u64>,
}

impl Decorator {
    /// Forgets the rows it made, to make them all again.
    fn forget(&self) {
        self.entries.borrow_mut().clear();
    }

    pub(crate) fn new(
        theme: MarkdownTheme,
        ctx: Rc<Ctx>,
        highlight: Option<Rc<Highlight>>,
        editor: EditorRef,
    ) -> Self {
        Self {
            theme,
            ctx,
            highlight,
            editor,
            marks: RefCell::default(),
            kinds: RefCell::default(),
            plain: Rc::default(),
            entries: RefCell::default(),
            tops: RefCell::default(),
            uid: Cell::new(0),
        }
    }

    /// The style of letters marked with `bits`.
    fn mark_style(&self, bits: u16) -> Rc<Style> {
        if let Some(s) = self.marks.borrow().get(&bits) {
            return s.clone();
        }
        let t = &self.theme;
        let mut s = Style::default();
        if bits & bit::STRONG != 0 {
            s.refine(&t.strong);
        }
        if bits & bit::EMPHASIS != 0 {
            s.refine(&t.emphasis);
        }
        if bits & bit::STRIKE != 0 {
            s.refine(&t.strikethrough);
        }
        if bits & bit::CODE != 0 {
            s.refine(&t.code);
        }
        if bits & bit::LINK != 0 {
            s.refine(&t.link);
        }
        if bits & bit::DONE != 0 {
            s.refine(&t.task_done);
        }
        if bits & (bit::SUPER | bit::SUB) != 0 {
            s.font_size = Some(t.base_size() * 0.75);
        }
        if bits & bit::SYNTAX != 0 {
            s = s.text_color(SYNTAX_COLOR);
        }
        let s = Rc::new(s);
        self.marks.borrow_mut().insert(bits, s.clone());
        s
    }

    /// The style of the box of a line that is `kind`.
    fn line_style(&self, kind: u16) -> Option<Rc<Style>> {
        if kind == 0 {
            return None;
        }
        if let Some(s) = self.kinds.borrow().get(&kind) {
            return s.clone();
        }
        let t = &self.theme;
        let mut s = Style::default();
        let level = (kind >> kind::HEADING_SHIFT) & 7;
        if level > 0 {
            let h = &t.headings[(level as usize).clamp(1, 6) - 1];
            s.font_size = h.font_size;
            s.font_weight = h.font_weight;
            s.line_height = h.line_height;
            s.color = h.color;
            s = s.pt(if level <= 2 { 12.0 } else { 6.0 });
        }
        if kind & kind::QUOTE != 0 {
            s.color = t.quote.color;
            s = s
                .border_l(t.quote.border_left_width.unwrap_or(4.0))
                .border_color(t.quote.border_color.unwrap_or(SYNTAX_COLOR))
                .pl(t.quote.padding_left.unwrap_or(16.0));
        }
        let alert = (kind >> kind::ALERT_SHIFT) & 7;
        if alert > 0 {
            // As the viewer has them: the line in the alert's color, the text as any other.
            s = s.border_color(t.alerts[(alert as usize - 1) % 5]);
            s.color = t.root.color;
        }
        if kind & kind::FOOTNOTE != 0 {
            s.font_size = Some(t.base_size() * 0.875);
            s.color = t.footnotes.color;
        }
        if kind & (kind::CODE | kind::MONO) != 0 {
            s.font_family = t.code_text.font_family.clone();
            s.font_size = t.code_text.font_size;
            s.line_height = t.code_text.line_height;
            s = s.whitespace_nowrap();
        }
        if kind & kind::CODE != 0 {
            s.background = t.code_block.background;
            s = s.px(16.0);
            if kind & kind::FENCE_TOP != 0 {
                s = s.rounded_t(6.0).pt(4.0).text_size(t.base_size() * 0.75);
            }
            if kind & kind::FENCE_BOTTOM != 0 {
                s = s.rounded_b(6.0).pb(4.0).text_size(t.base_size() * 0.75);
            }
        }
        let s = Some(Rc::new(s));
        self.kinds.borrow_mut().insert(kind, s.clone());
        s
    }

    fn next_uid(&self) -> u64 {
        let u = self.uid.get() + 1;
        self.uid.set(u);
        u
    }

    /// The rows of the whole text.
    pub(crate) fn rows(&self, cx: &DecorateContext) -> Vec<Row> {
        let text = cx.text;
        let tops = self.top_blocks(text, cx.version);
        let footnotes = hash_of(&tops.footnotes);
        let sel = cx.selection();
        let mut rows = Vec::with_capacity(cx.line_count());
        let mut line = 0;
        let mut old = std::mem::take(&mut *self.entries.borrow_mut());
        let mut next = HashMap::with_capacity(old.len());
        let mut seen: HashMap<u64, u32> = HashMap::new();
        for &(ref b, hash) in tops.blocks.iter() {
            // Made again when the numbers it shows change, though it doesn't.
            let hash = hash ^ footnotes ^ tops.numbers.hash_of(b.clone(), &text[b.clone()]);
            let first = cx.line_of(b.start);
            let last = cx.line_of(b.end.saturating_sub(1).max(b.start));
            if first < line {
                continue;
            }
            rows.extend((line..first).map(|_| Row::Line(self.plain.clone())));
            let start = cx.line(first).start;
            let end = cx.line(last).end;
            let src = &text[start..end];
            let nth = seen.entry(hash).or_insert(0);
            *nth += 1;
            // Without the focus, nothing is being written: it all shows as it looks.
            let touched = (cx.focused && sel.start <= end && sel.end >= start)
                .then(|| (sel.start.max(start) - start, sel.end.min(end) - start));
            let holds = touched.is_some() && sel.start >= start && sel.end <= end;
            let key = Key {
                hash,
                nth: *nth,
                selection: touched,
                holds,
            };
            let entry = match old.remove(&key).or_else(|| next.remove(&key)) {
                Some(e) => e,
                None => {
                    let cell = Rc::new(Cell::new(start));
                    Entry {
                        rows: Rc::new(self.block(src, &tops.footnotes, touched, holds, &cell)),
                        start: cell,
                    }
                }
            };
            entry.start.set(start);
            rows.extend(entry.rows.iter().cloned());
            next.insert(key, entry);
            line = last + 1;
        }
        rows.extend((line..cx.line_count()).map(|_| Row::Line(self.plain.clone())));
        *self.entries.borrow_mut() = next;
        rows
    }

    /// Where the blocks at the top of the document are, with the hash of the whole of their
    /// lines, and its footnotes. It looks again only when the text changed.
    fn top_blocks(&self, text: &str, version: u64) -> std::cell::Ref<'_, Tops> {
        if self.tops.borrow().version != Some(version) {
            let mut out = Vec::new();
            let mut footnotes = String::new();
            let mut depth = 0usize;
            let mut start = 0;
            // The formulas on lines of their own, with where each starts, as the viewer reads them.
            let mut formulas: Vec<(usize, Cow<str>)> = Vec::new();
            let mut math_block: Option<(usize, Cow<str>)> = None;
            // What the parser gives is mostly part of the text: it is not copied.
            fn text_of<'a>(t: &CowStr<'a>) -> Cow<'a, str> {
                match t {
                    CowStr::Borrowed(s) => Cow::Borrowed(*s),
                    other => Cow::Owned(other.to_string()),
                }
            }
            for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
                match &event {
                    Event::DisplayMath(t) => formulas.push((range.start, text_of(t))),
                    Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                        if cfg!(feature = "math")
                            && info.split_whitespace().next() == Some("math") =>
                    {
                        math_block = Some((range.start, Cow::Borrowed("")));
                    }
                    Event::Text(t) => {
                        if let Some((_, tex)) = &mut math_block {
                            if tex.is_empty() {
                                *tex = text_of(t);
                            } else {
                                tex.to_mut().push_str(t);
                            }
                        }
                    }
                    Event::End(TagEnd::CodeBlock) => {
                        if let Some((at, tex)) = math_block.take() {
                            let tex = match tex {
                                Cow::Borrowed(s) => {
                                    Cow::Borrowed(s.strip_suffix('\n').unwrap_or(s))
                                }
                                Cow::Owned(mut s) => {
                                    if s.ends_with('\n') {
                                        s.pop();
                                    }
                                    Cow::Owned(s)
                                }
                            };
                            formulas.push((at, tex));
                        }
                    }
                    _ => {}
                }
                let done = match event {
                    Event::Start(tag) => {
                        if depth == 0 {
                            start = range.start;
                            if let Tag::FootnoteDefinition(label) = tag {
                                footnotes.push_str(&format!("\n\n[^{label}]: -"));
                            }
                        }
                        depth += 1;
                        None
                    }
                    Event::End(_) => {
                        depth = depth.saturating_sub(1);
                        (depth == 0).then(|| start..range.end.max(start))
                    }
                    _ if depth == 0 => Some(range),
                    _ => None,
                };
                if let Some(r) = done {
                    // The whole of its lines.
                    let a = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
                    // By bytes: the byte before the end may be inside a letter.
                    let last = r.end.saturating_sub(1).max(r.start);
                    let b = text.as_bytes()[last..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(text.len(), |i| last + i);
                    out.push((r, hash_of(&text[a..b.max(a)])));
                }
            }
            let formulas: Vec<(usize, &str)> = formulas
                .iter()
                .map(|(at, tex)| (*at, tex.as_ref()))
                .collect();
            let numbers = Rc::new(crate::numbers::Numbers::of(
                &self.tops.borrow().numbers,
                &formulas,
            ));
            *self.ctx.numbers.borrow_mut() = numbers.clone();
            *self.tops.borrow_mut() = Tops {
                version: Some(version),
                blocks: out,
                footnotes,
                numbers,
            };
        }
        self.tops.borrow()
    }

    /// The rows of one block, whose source is the whole of its lines. `touched` is where the
    /// selection touches it, from its start, ends included; `holds` says all of the selection is
    /// in it. A table, a diagram or a rule shows its source only then: a selection going over it
    /// leaves it as it looks, marked, so that what is selected doesn't jump about.
    fn block(
        &self,
        src: &str,
        footnotes: &str,
        touched: Option<(usize, usize)>,
        holds: bool,
        start: &Rc<Cell<usize>>,
    ) -> Vec<Row> {
        let uid = self.next_uid();
        let starts: Vec<usize> = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let line_of = |at: usize| starts.partition_point(|&s| s <= at).saturating_sub(1);
        let line_end = |l: usize| starts.get(l + 1).map_or(src.len(), |&s| s - 1);
        let touches = |a: usize, b: usize| touched.is_some_and(|(s, e)| s <= b && e >= a);
        let line_touched = |l: usize| touches(starts[l], line_end(l));
        let mut lines = vec![LineBuild::default(); starts.len()];
        // Adds marks over `range`, line by line.
        let mark = |lines: &mut Vec<LineBuild>, range: Range<usize>, bits: u16| {
            if range.is_empty() {
                return;
            }
            for l in line_of(range.start)..=line_of(range.end - 1) {
                let a = range.start.max(starts[l]);
                let b = range.end.min(line_end(l));
                if a < b {
                    lines[l].bits.push((a - starts[l]..b - starts[l], bits));
                }
            }
        };
        let hide = |lines: &mut Vec<LineBuild>, range: Range<usize>, show: bool| {
            if range.is_empty() {
                return;
            }
            let l = line_of(range.start);
            let end = range.end.min(line_end(l));
            if range.start >= end {
                return;
            }
            let r = range.start - starts[l]..end - starts[l];
            if show {
                lines[l].bits.push((r, bit::SYNTAX));
            } else {
                lines[l].replace.push((r, None));
            }
        };

        struct Open {
            tag: TagEnd,
            range: Range<usize>,
            first: Option<usize>,
            last: usize,
            task: bool,
        }
        let mut stack: Vec<Open> = Vec::new();
        let mut whole: Option<Row> = None;
        let lines_in = |range: &Range<usize>| {
            line_of(range.start)..=line_of(range.end.saturating_sub(1).max(range.start))
        };

        // A run of text, to find what is raised and lowered in it once it ends.
        let mut run: Option<Range<usize>> = None;
        let scripted = |lines: &mut Vec<LineBuild>, run: Range<usize>| {
            for (r, up) in scripts(&src[run.clone()]) {
                let r = r.start + run.start..r.end + run.start;
                mark(
                    lines,
                    r.start + 1..r.end - 1,
                    if up { bit::SUPER } else { bit::SUB },
                );
                let show = touches(r.start, r.end);
                hide(lines, r.start..r.start + 1, show);
                hide(lines, r.end - 1..r.end, show);
            }
        };
        let with_footnotes;
        let parsed = if footnotes.is_empty() {
            src
        } else {
            with_footnotes = format!("{src}{footnotes}");
            &with_footnotes
        };
        for (event, range) in Parser::new_ext(parsed, options()).into_offset_iter() {
            // What was put after the block for its footnotes isn't shown.
            if range.start >= src.len() {
                continue;
            }
            let range = range.start..range.end.min(src.len());
            let in_code = stack.iter().any(|o| {
                matches!(
                    o.tag,
                    TagEnd::CodeBlock
                        | TagEnd::HtmlBlock
                        | TagEnd::MetadataBlock(_)
                        | TagEnd::Image
                )
            });
            match (&event, run.clone()) {
                (Event::Text(_), Some(r)) if !in_code && r.end == range.start => {
                    run = Some(r.start..range.end);
                }
                (_, r) => {
                    if let Some(r) = r {
                        scripted(&mut lines, r);
                    }
                    run = matches!(event, Event::Text(_))
                        .then(|| range.clone())
                        .filter(|_| !in_code);
                }
            }
            // What is in an element tells where its marks end; its own end doesn't.
            if !matches!(event, Event::End(_))
                && let Some(top) = stack.last_mut()
            {
                top.first.get_or_insert(range.start);
                top.last = top.last.max(range.end);
            }
            match event {
                Event::Start(tag) => {
                    let top_level = stack.is_empty();
                    match &tag {
                        Tag::Heading { level, .. } => {
                            let level = *level as u16;
                            for l in lines_in(&range) {
                                lines[l].kind |= level << kind::HEADING_SHIFT;
                            }
                        }
                        Tag::BlockQuote(kind_of)
                            if !stack.iter().any(|o| matches!(o.tag, TagEnd::BlockQuote(_))) =>
                        {
                            for l in lines_in(&range) {
                                lines[l].kind |= kind::QUOTE;
                                let line = &src[starts[l]..line_end(l)];
                                let n = quote_prefix(line);
                                hide(&mut lines, starts[l]..starts[l] + n, line_touched(l));
                            }
                            if let Some(k) = kind_of {
                                let i = *k as usize % 5;
                                for l in lines_in(&range) {
                                    lines[l].kind |= (i as u16 + 1) << kind::ALERT_SHIFT;
                                }
                                // `[!NOTE]` shows as the alert's name.
                                let l = line_of(range.start);
                                let line = &src[starts[l]..line_end(l)];
                                let n = quote_prefix(line);
                                if let Some(close) = line[n..]
                                    .starts_with("[!")
                                    .then(|| line[n..].find(']'))
                                    .flatten()
                                {
                                    let title = starts[l] + n..starts[l] + n + close + 1;
                                    if line_touched(l) {
                                        mark(&mut lines, title, bit::SYNTAX);
                                    } else {
                                        let color = self.theme.alerts[i];
                                        let name =
                                            ["Note", "Tip", "Important", "Warning", "Caution"][i];
                                        let key = hash_of(&format!("alert {uid} {i}"));
                                        let w = Widget::new(key, move || {
                                            text(name)
                                                .font_weight(FontWeight(600))
                                                .text_color(color)
                                                .into_view()
                                        });
                                        lines[l].replace.push((n..n + close + 1, Some(w)));
                                    }
                                }
                            }
                        }
                        Tag::FootnoteDefinition(_) if top_level => {
                            for l in lines_in(&range) {
                                lines[l].kind |= kind::FOOTNOTE;
                            }
                            // `[^1]:` shows as a raised `1`.
                            let l = line_of(range.start);
                            let line = &src[range.start..line_end(l)];
                            if let Some(close) =
                                line.starts_with("[^").then(|| line.find("]:")).flatten()
                            {
                                let a = range.start;
                                mark(&mut lines, a + 2..a + close, bit::LINK | bit::SUPER);
                                let show = line_touched(l);
                                hide(&mut lines, a..a + 2, show);
                                hide(&mut lines, a + close..a + close + 2, show);
                            }
                        }
                        Tag::CodeBlock(kind_of) => {
                            let lang = match kind_of {
                                CodeBlockKind::Fenced(l) => {
                                    l.split_whitespace().next().unwrap_or("").to_owned()
                                }
                                CodeBlockKind::Indented => String::new(),
                            };
                            let fenced = matches!(kind_of, CodeBlockKind::Fenced(_));
                            let ls: Vec<usize> = lines_in(&range).collect();
                            let inside = touches(range.start, range.end);
                            let drawn =
                                lang == "mermaid" || (lang == "math" && cfg!(feature = "math"));
                            if top_level && drawn && !(holds && inside) {
                                whole = Some(self.whole(src, uid, 1, start));
                            }
                            let (first, last) = (ls[0], *ls.last().unwrap());
                            let closed = fenced && last > first && {
                                let t = src[starts[last]..line_end(last)].trim_start();
                                t.starts_with("```") || t.starts_with("~~~")
                            };
                            for &l in &ls {
                                lines[l].kind |= kind::CODE;
                            }
                            if fenced {
                                lines[first].kind |= kind::FENCE_TOP;
                                hide(&mut lines, starts[first]..line_end(first), inside);
                                if closed {
                                    lines[last].kind |= kind::FENCE_BOTTOM;
                                    hide(&mut lines, starts[last]..line_end(last), inside);
                                }
                            }
                            let body_lines = if fenced {
                                first + 1..if closed { last } else { last + 1 }
                            } else {
                                first..last + 1
                            };
                            if let Some(f) = &self.highlight
                                && !body_lines.is_empty()
                            {
                                let (a, b) =
                                    (starts[body_lines.start], line_end(body_lines.end - 1));
                                for (r, style) in untrack(|| f(&lang, &src[a..b])) {
                                    let r = r.start + a..(r.end + a).min(b);
                                    for l in body_lines.clone() {
                                        let x = r.start.max(starts[l]);
                                        let y = r.end.min(line_end(l));
                                        if x < y {
                                            lines[l].styled.push((
                                                x - starts[l]..y - starts[l],
                                                style.clone(),
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        Tag::Table(_) => {
                            if top_level && !(holds && touches(range.start, range.end)) {
                                whole = Some(self.whole(src, uid, 2, start));
                            }
                            for l in lines_in(&range) {
                                lines[l].kind |= kind::MONO;
                            }
                        }
                        Tag::HtmlBlock | Tag::MetadataBlock(_) => {
                            for l in lines_in(&range) {
                                lines[l].kind |= kind::MONO;
                                mark(&mut lines, starts[l]..line_end(l), bit::SYNTAX);
                            }
                        }
                        _ => {}
                    }
                    stack.push(Open {
                        tag: tag.to_end(),
                        range,
                        first: None,
                        last: 0,
                        task: false,
                    });
                }
                Event::End(_) => {
                    let Some(open) = stack.pop() else { continue };
                    if let Some(top) = stack.last_mut() {
                        top.last = top.last.max(open.range.end);
                    }
                    let r = open.range.clone();
                    let inner =
                        open.first.unwrap_or(r.end)..open.last.max(open.first.unwrap_or(r.end));
                    match open.tag {
                        TagEnd::Strong
                        | TagEnd::Emphasis
                        | TagEnd::Strikethrough
                        | TagEnd::Superscript
                        | TagEnd::Subscript => {
                            let c = src.as_bytes()[r.start];
                            let k = src[r.start..r.end]
                                .bytes()
                                .take_while(|&b| b == c)
                                .count()
                                .min(match open.tag {
                                    TagEnd::Strong => 2,
                                    TagEnd::Strikethrough => 2,
                                    _ => 1,
                                });
                            let bits = match open.tag {
                                TagEnd::Strong => bit::STRONG,
                                TagEnd::Emphasis => bit::EMPHASIS,
                                TagEnd::Strikethrough => bit::STRIKE,
                                TagEnd::Superscript => bit::SUPER,
                                _ => bit::SUB,
                            };
                            if r.len() >= 2 * k {
                                mark(&mut lines, r.start + k..r.end - k, bits);
                                let show = touches(r.start, r.end);
                                hide(&mut lines, r.start..r.start + k, show);
                                hide(&mut lines, r.end - k..r.end, show);
                            }
                        }
                        TagEnd::Link => {
                            mark(&mut lines, inner.clone(), bit::LINK);
                            if !inner.is_empty() {
                                let show = touches(r.start, r.end);
                                hide(&mut lines, r.start..inner.start, show);
                                hide(&mut lines, inner.end..r.end, show);
                            }
                        }
                        TagEnd::Image => {
                            if touches(r.start, r.end) {
                                mark(&mut lines, r.clone(), bit::SYNTAX);
                            } else {
                                let l = line_of(r.start);
                                let end = r.end.min(line_end(l));
                                let (url, _) = image_parts(&src[r.start..end]);
                                let picture = self.ctx.picture_of(&url);
                                let key = hash_of(&format!("img {uid} {} {url}", r.start));
                                let w = Widget::new(key, move || {
                                    img(picture.clone()).max_w(pct(100.0)).into_view()
                                });
                                lines[l]
                                    .replace
                                    .push((r.start - starts[l]..end - starts[l], Some(w)));
                            }
                        }
                        TagEnd::Heading(_) => {
                            let l = line_of(r.start);
                            let atx = src[r.start..].starts_with('#');
                            if atx {
                                let show = line_touched(l);
                                let first = open.first.unwrap_or(line_end(l)).min(line_end(l));
                                hide(&mut lines, r.start..first, show);
                                if open.last > 0 && open.last < line_end(l) {
                                    hide(&mut lines, open.last..line_end(l), show);
                                }
                            } else {
                                // The line under the text of a setext heading.
                                let last = line_of(r.end.saturating_sub(1).max(r.start));
                                if last > l {
                                    lines[last].kind &= !(7 << kind::HEADING_SHIFT);
                                    hide(
                                        &mut lines,
                                        starts[last]..line_end(last),
                                        touches(r.start, r.end),
                                    );
                                }
                            }
                        }
                        TagEnd::Item => {
                            let m = marker_at(src, r.start);
                            let c = src.as_bytes().get(m).copied();
                            if !open.task
                                && matches!(c, Some(b'-' | b'*' | b'+'))
                                && !touches(m, m + 1)
                            {
                                let l = line_of(m);
                                let at = m - starts[l];
                                let key = hash_of(&format!("bullet {uid} {at}"));
                                let w = Widget::new(key, bullet);
                                lines[l].replace.push((at..at + 1, Some(w)));
                            }
                        }
                        _ => {}
                    }
                }
                Event::Code(_) => {
                    let k = src[range.clone()]
                        .bytes()
                        .take_while(|&b| b == b'`')
                        .count();
                    if range.len() >= 2 * k {
                        mark(&mut lines, range.start + k..range.end - k, bit::CODE);
                        let show = touches(range.start, range.end);
                        hide(&mut lines, range.start..range.start + k, show);
                        hide(&mut lines, range.end - k..range.end, show);
                    }
                }
                #[cfg(feature = "math")]
                Event::InlineMath(ref tex) | Event::DisplayMath(ref tex) => {
                    let display = matches!(event, Event::DisplayMath(_));
                    let k = src[range.clone()]
                        .bytes()
                        .take_while(|&b| b == b'$')
                        .count();
                    let inside = touches(range.start, range.end);
                    let l = line_of(range.start);
                    // A paragraph that is only a formula on a line of its own shows as the
                    // viewer shows it, until the selection is all in it.
                    let alone = display
                        && stack.len() == 1
                        && matches!(stack[0].tag, TagEnd::Paragraph)
                        && src.trim() == src[range.clone()].trim();
                    if alone {
                        if !(holds && inside) {
                            whole = Some(self.whole(src, uid, 3, start));
                        }
                    } else if inside || line_of(range.end - 1) != l || range.len() < 2 * k {
                        mark(&mut lines, range.start + k..range.end - k, bit::CODE);
                        mark(&mut lines, range.start..range.start + k, bit::SYNTAX);
                        mark(&mut lines, range.end - k..range.end, bit::SYNTAX);
                    } else {
                        // Among the words, at full size if it is meant for a line of its own.
                        let source = if display {
                            format!("\\displaystyle {tex}")
                        } else {
                            tex.to_string()
                        };
                        // In parts a line can break between, each over some of the source. Where
                        // `\\` ends a line, a box as wide as the line, and no higher, makes the
                        // next part start a line of its own, as a line break in the text would.
                        let pieces = self
                            .ctx
                            .math(source.clone())
                            .style(&self.theme.math)
                            .pieces();
                        let (a, b) = (range.start - starts[l], range.end - starts[l]);
                        let n = pieces.len().min(b - a).max(1);
                        for (i, piece) in pieces.into_iter().enumerate().take(n) {
                            let key = hash_of(&format!("math {uid} {a} {i} {source}"));
                            let w = match piece {
                                anti_webui_math::MathPiece::Part(part) => {
                                    Widget::new(key, move || part.clone().into_view())
                                }
                                anti_webui_math::MathPiece::LineBreak => {
                                    Widget::new(key, || div().w_full().h(0.0).into_view())
                                }
                            };
                            let end = if i + 1 == n { b } else { a + i + 1 };
                            lines[l].replace.push((a + i..end, Some(w)));
                        }
                    }
                }
                Event::InlineHtml(_) | Event::Html(_) => mark(&mut lines, range, bit::SYNTAX),
                Event::FootnoteReference(_) if range.len() > 3 => {
                    // `[^1]` shows as a raised `1`.
                    mark(
                        &mut lines,
                        range.start + 2..range.end - 1,
                        bit::LINK | bit::SUPER,
                    );
                    let show = touches(range.start, range.end);
                    hide(&mut lines, range.start..range.start + 2, show);
                    hide(&mut lines, range.end - 1..range.end, show);
                }
                Event::TaskListMarker(done) => {
                    let item = stack.iter_mut().rev().find(|o| o.tag == TagEnd::Item);
                    let item_start = item
                        .as_ref()
                        .map_or(range.start, |o| marker_at(src, o.range.start));
                    if let Some(item) = item {
                        item.task = true;
                    }
                    let l = line_of(range.start);
                    if done {
                        mark(&mut lines, range.end..line_end(l), bit::DONE);
                    }
                    if touches(item_start, range.end) {
                        mark(&mut lines, item_start..range.end, bit::SYNTAX);
                    } else {
                        let marker = range.start..range.end;
                        let key = hash_of(&format!("task {uid} {} {done}", range.start));
                        let editor = self.editor;
                        let start = start.clone();
                        let w = Widget::new(key, move || {
                            let marker = marker.clone();
                            let start = start.clone();
                            check_box(done, move || {
                                let at = start.get();
                                editor.replace(
                                    at + marker.start..at + marker.end,
                                    if done { "[ ]" } else { "[x]" },
                                );
                            })
                        });
                        lines[l]
                            .replace
                            .push((item_start - starts[l]..range.end - starts[l], Some(w)));
                    }
                }
                Event::Rule => {
                    if stack.is_empty() && !(holds && touches(range.start, range.end)) {
                        whole = Some(self.rule(uid));
                    } else {
                        mark(&mut lines, range, bit::SYNTAX);
                    }
                }
                _ => {}
            }
        }
        if let Some(r) = run {
            scripted(&mut lines, r);
        }
        if let Some(row) = whole {
            let Row::Block { style, widget, .. } = row else {
                unreachable!()
            };
            return vec![Row::Block {
                lines: starts.len(),
                style,
                widget,
            }];
        }
        lines
            .into_iter()
            .enumerate()
            .map(|(l, b)| self.look(b, line_end(l) - starts[l]))
            .collect()
    }

    /// A block shown as the viewer shows it.
    fn whole(&self, src: &str, uid: u64, what: u64, start: &Rc<Cell<usize>>) -> Row {
        let ctx = self.ctx.clone();
        let src = src.to_owned();
        let start = start.clone();
        let key = hash_of(&format!("whole {what} {uid}"));
        Row::Block {
            lines: 1,
            style: Some(Rc::new(Style::default().py(4.0))),
            widget: Widget::new(key, move || ctx.block_of(&src, start.get())),
        }
    }

    fn rule(&self, uid: u64) -> Row {
        let color = self.theme.rule.background.unwrap_or(SYNTAX_COLOR);
        Row::Block {
            lines: 1,
            style: None,
            widget: Widget::new(hash_of(&format!("rule {uid}")), move || {
                div().py(11.0).child(div().h(2.0).bg(color)).into_view()
            }),
        }
    }

    /// What a line looks like, from how it is made up.
    fn look(&self, b: LineBuild, len: usize) -> Row {
        let style = self.line_style(b.kind);
        if b.bits.is_empty() && b.styled.is_empty() && b.replace.is_empty() && style.is_none() {
            return Row::Line(self.plain.clone());
        }
        // The marks are flattened: each part gets the style of all that marks it.
        let mut cuts: Vec<usize> = b.bits.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut parts: Vec<(Range<usize>, u16)> = Vec::new();
        for w in cuts.windows(2) {
            let (a, z) = (w[0], w[1].min(len));
            if a >= z {
                continue;
            }
            let bits = b
                .bits
                .iter()
                .filter(|(r, _)| r.start <= a && z <= r.end)
                .fold(0, |acc, (_, x)| acc | x);
            if bits == 0 {
                continue;
            }
            match parts.last_mut() {
                Some((r, x)) if r.end == a && *x == bits => r.end = z,
                _ => parts.push((a..z, bits)),
            }
        }
        let raised = parts
            .iter()
            .filter_map(|(r, bits)| {
                let align = if bits & bit::SUPER != 0 {
                    VerticalAlign::Super
                } else if bits & bit::SUB != 0 {
                    VerticalAlign::Sub
                } else {
                    return None;
                };
                Some((r.clone(), align))
            })
            .collect();
        let mut marks: Vec<(Range<usize>, Rc<Style>)> = parts
            .into_iter()
            .map(|(r, bits)| (r, self.mark_style(bits)))
            .collect();
        // Colors from outside win over the marks.
        if !b.styled.is_empty() {
            let mut styled = b.styled;
            styled.sort_by_key(|(r, _)| r.start);
            marks = styled;
        }
        let mut replace = b.replace;
        replace.sort_by_key(|(r, _)| (r.start, r.end));
        Row::Line(Rc::new(LineLook {
            style,
            marks,
            replace,
            raised,
        }))
    }
}

/// Where the mark of a list item is: an item may start with room before its mark (` - a`).
fn marker_at(src: &str, start: usize) -> usize {
    start
        + src.as_bytes()[start..]
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count()
}

/// How long the `>` marks at the start of a line of a quote are, with their spaces.
fn quote_prefix(line: &str) -> usize {
    let b = line.as_bytes();
    let mut i = 0;
    let mut end = 0;
    loop {
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i < b.len() && b[i] == b'>' {
            i += 1;
            if i < b.len() && b[i] == b' ' {
                i += 1;
            }
            end = i;
        } else {
            return end;
        }
    }
}

/// The address and the text of `![alt](url "title")`.
fn image_parts(s: &str) -> (String, String) {
    let alt_end = s.find("](").unwrap_or(s.len());
    let alt = s.get(2..alt_end).unwrap_or("").to_owned();
    let rest = s.get(alt_end + 2..).unwrap_or("");
    let rest = rest.strip_suffix(')').unwrap_or(rest).trim_start();
    let url = match rest.strip_prefix('<') {
        Some(r) => r.split('>').next().unwrap_or(""),
        None => rest.split_whitespace().next().unwrap_or(""),
    };
    (url.to_owned(), alt)
}

/// The dot of an item of a list, a little above the baseline, as wide as the mark it stands for.
fn bullet() -> View {
    div()
        .w(9.0)
        .h(10.0)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(5.0)
                .h(5.0)
                .rounded(2.5)
                .bg(Color::from_rgb(0x59636e)),
        )
        .into_view()
}

/// A box for a task, ticked when it is done.
fn check_box(done: bool, toggle: impl Fn() + 'static) -> View {
    let blue = Color::from_rgb(0x0969da);
    let b = div()
        .w(14.0)
        .h(14.0)
        .mr(2.0)
        .rounded(3.0)
        .border(1.5)
        .cursor(CursorIcon::Pointer)
        .flex()
        .items_center()
        .justify_center()
        .on_click(move |_| toggle());
    if done {
        b.bg(blue)
            .border_color(blue)
            .child(text("✓").text_size(10.0).text_color(Color::WHITE).bold())
            .into_view()
    } else {
        b.border_color(SYNTAX_COLOR).bg(Color::WHITE).into_view()
    }
}

/// A Markdown editor that shows what it says as it is written, as Obsidian's live preview.
pub struct MarkdownEditor {
    source: Signal<String>,
    theme: MarkdownTheme,
    on_link: OnLink,
    image: Pictures,
    highlight: Option<Rc<Highlight>>,
    #[cfg(feature = "code")]
    code_theme: Option<anti_webui_code::CodeTheme>,
    style: Style,
}

pub fn markdown_editor(source: Signal<String>) -> MarkdownEditor {
    MarkdownEditor {
        source,
        theme: MarkdownTheme::default(),
        on_link: None,
        image: None,
        highlight: None,
        #[cfg(feature = "code")]
        code_theme: None,
        style: Style::default(),
    }
}

impl MarkdownEditor {
    pub fn theme(mut self, theme: MarkdownTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Called with the target of a link clicked with Ctrl held.
    pub fn on_link(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_link = Some(Rc::new(f));
        self
    }

    /// Where the picture of `![alt](src)` comes from. By default `src` is a path.
    pub fn image(mut self, f: impl Fn(&str) -> ImageSource + 'static) -> Self {
        self.image = Some(Rc::new(f));
        self
    }

    /// Colors the code of code blocks, as `markdown_view`'s does.
    pub fn highlight(
        mut self,
        f: impl Fn(&str, &str) -> Vec<(Range<usize>, Rc<Style>)> + 'static,
    ) -> Self {
        self.highlight = Some(Rc::new(f));
        self
    }

    /// The colors of code blocks, as `markdown_view`'s.
    #[cfg(feature = "code")]
    pub fn code_theme(mut self, theme: anti_webui_code::CodeTheme) -> Self {
        self.code_theme = Some(theme);
        self
    }
}

impl Styled for MarkdownEditor {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

/// What a line of a list or a quote starts with, to go on with on the next line: its room on the
/// left and its mark, and whether there is anything after the mark.
fn continuation(line: &str) -> Option<(String, bool)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    if let Some(n) = (quote_prefix(rest) > 0).then(|| quote_prefix(rest)) {
        let more = !rest[n..].trim().is_empty();
        return Some((
            format!(
                "{}{}",
                &line[..indent],
                rest[..n].trim_end().to_owned() + " "
            ),
            more,
        ));
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (mark, after) = if matches!(rest.as_bytes().first(), Some(b'-' | b'*' | b'+'))
        && rest.as_bytes().get(1) == Some(&b' ')
    {
        (rest[..1].to_owned(), &rest[2..])
    } else if digits > 0
        && matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
        && rest.as_bytes().get(digits + 1) == Some(&b' ')
    {
        let n: u64 = rest[..digits].parse().ok()?;
        (
            format!("{}{}", n + 1, &rest[digits..digits + 1]),
            &rest[digits + 2..],
        )
    } else {
        return None;
    };
    let task = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .any(|t| after.starts_with(t));
    let body = if task { &after[4..] } else { after };
    let mark = if task { format!("{mark} [ ]") } else { mark };
    Some((
        format!("{}{mark} ", &line[..indent]),
        !body.trim().is_empty(),
    ))
}

/// Enter in a list or a quote goes on with it, and ends it on an empty item. Tab in an item moves
/// it in or out.
pub(crate) fn list_keys(editor: EditorRef, e: &KeyboardEvent) {
    use anti_webui_core::event::{Key, NamedKey};
    let m = e.modifiers;
    if m.ctrl || m.alt || m.logo {
        return;
    }
    let Some((line, text, caret, selected)) = editor.with(|ed| {
        let l = ed.line(ed.line_of(ed.focus));
        (
            l.clone(),
            ed.text[l].to_owned(),
            ed.focus,
            ed.has_selection(),
        )
    }) else {
        return;
    };
    if selected {
        return;
    }
    match &e.key {
        Key::Named(NamedKey::Enter) if !m.shift => {
            let Some((mark, more)) = continuation(&text) else {
                return;
            };
            e.prevent_default();
            if more || caret < line.start + mark.len() {
                editor.replace(caret..caret, &format!("\n{mark}"));
            } else {
                // An empty item ends the list.
                editor.replace(line.clone(), "");
            }
        }
        Key::Named(NamedKey::Tab) if continuation(&text).is_some() => {
            e.prevent_default();
            if m.shift {
                let n = text.bytes().take(4).take_while(|&b| b == b' ').count();
                let n = if n == 0 && text.starts_with('\t') {
                    1
                } else {
                    n
                };
                if n > 0 {
                    editor.replace(line.start..line.start + n, "");
                }
            } else {
                editor.replace(line.start..line.start, "    ");
            }
        }
        _ => {}
    }
}

/// The link at a byte of the text, by looking at its line.
fn link_at(text: &str, at: usize) -> Option<String> {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = &text[start..end];
    let mut url = None;
    for (event, range) in Parser::new_ext(line, options()).into_offset_iter() {
        if let Event::Start(Tag::Link { dest_url, .. }) = event
            && (range.start..=range.end).contains(&(at - start))
        {
            url = Some(dest_url.to_string());
        }
    }
    url
}

impl IntoView for MarkdownEditor {
    fn into_view(self) -> View {
        let editor = editor_ref();
        let on_link = self.on_link.clone();
        #[cfg(feature = "code")]
        let (highlight, ready) = crate::view::code_colors(self.highlight, self.code_theme);
        #[cfg(not(feature = "code"))]
        let (highlight, ready) = (self.highlight, None::<Signal<u64>>);
        let ctx = Ctx::new(
            self.theme.clone(),
            self.on_link,
            self.image,
            None,
            highlight.clone(),
        );
        let deco = Rc::new(Decorator::new(self.theme.clone(), ctx, highlight, editor));
        if let Some(ready) = ready {
            // A grammar came: the code blocks are made again with its colors.
            let deco = deco.clone();
            effect(move || {
                if ready.get() > 0 {
                    deco.forget();
                    editor.redecorate();
                }
            });
        }
        let mut e = text_editor()
            .value(self.source)
            .editor_ref(editor)
            .style(&self.theme.root)
            .style(&self.style)
            .decorate(move |cx| deco.rows(cx))
            .on_key_down(move |e| list_keys(editor, e));
        if let Some(open) = on_link {
            e = e.on_click(move |p| {
                if !p.modifiers.primary() {
                    return;
                }
                let url = editor
                    .offset_at(p.position)
                    .and_then(|at| link_at(&editor.text(), at));
                if let Some(url) = url {
                    open(&url);
                }
            });
        }
        e.into_view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_quotes_go_on() {
        assert_eq!(continuation("- a"), Some(("- ".into(), true)));
        assert_eq!(continuation("  * "), Some(("  * ".into(), false)));
        assert_eq!(continuation("9. x"), Some(("10. ".into(), true)));
        assert_eq!(continuation("- [x] done"), Some(("- [ ] ".into(), true)));
        assert_eq!(continuation("> quoted"), Some(("> ".into(), true)));
        assert_eq!(continuation("> > deep"), Some(("> > ".into(), true)));
        assert_eq!(continuation("plain"), None);
        assert_eq!(continuation("-not a list"), None);
    }

    #[test]
    fn the_parts_of_a_picture() {
        assert_eq!(
            image_parts("![a cat](cat.png \"Cat\")"),
            ("cat.png".into(), "a cat".into())
        );
        assert_eq!(image_parts("![](<a b.png>)").0, "a b.png");
    }

    #[test]
    fn the_marks_of_a_quote() {
        assert_eq!(quote_prefix("> a"), 2);
        assert_eq!(quote_prefix(" > > a"), 5);
        assert_eq!(quote_prefix("a > b"), 0);
    }

    #[test]
    fn a_link_is_found_by_its_line() {
        let t = "x\nsee [the docs](https://a.b) here\n";
        assert_eq!(link_at(t, 8).as_deref(), Some("https://a.b"));
        assert_eq!(link_at(t, 3), None);
    }
}
