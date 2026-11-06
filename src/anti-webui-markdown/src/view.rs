//! Blocks to ordinary nodes: paragraphs and headings are `rich_text`, lists and tables are boxes.

use crate::parse::{self, Alert, Align, Block, Inlines, Item, Piece, Top, mark};
use crate::theme::MarkdownTheme;
use anti_webui_core::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

/// What a `:name:` in the text is: text in its place (`😄`), or a picture as tall as the letters.
#[derive(Clone, Debug)]
pub enum Emoji {
    Text(String),
    Image(ImageSource),
}

enum Source {
    Static(Rc<str>),
    Dynamic(Box<dyn Fn() -> String>),
}

/// What `markdown_view` shows: text, or a closure that reads signals.
pub trait IntoMarkdown {
    fn into_markdown(self) -> MarkdownSource;
}

/// See [`IntoMarkdown`].
pub struct MarkdownSource(Source);

impl IntoMarkdown for &str {
    fn into_markdown(self) -> MarkdownSource {
        MarkdownSource(Source::Static(Rc::from(self)))
    }
}

impl IntoMarkdown for String {
    fn into_markdown(self) -> MarkdownSource {
        MarkdownSource(Source::Static(Rc::from(self)))
    }
}

impl<F: Fn() -> S + 'static, S: Into<String>> IntoMarkdown for F {
    fn into_markdown(self) -> MarkdownSource {
        MarkdownSource(Source::Dynamic(Box::new(move || self().into())))
    }
}

type Highlight = dyn Fn(&str, &str) -> Vec<(Range<usize>, Style)>;
type OnLink = Option<Rc<dyn Fn(&str)>>;
type Pictures = Option<Rc<dyn Fn(&str) -> ImageSource>>;
type Emojis = Option<Rc<dyn Fn(&str) -> Option<Emoji>>>;

/// A Markdown document, shown with ordinary nodes. When the source is a closure, the blocks whose
/// source didn't change stay as they are.
pub struct MarkdownView {
    source: Source,
    theme: MarkdownTheme,
    on_link: OnLink,
    image: Pictures,
    emoji: Emojis,
    highlight: Option<Rc<Highlight>>,
    style: Style,
}

pub fn markdown_view(source: impl IntoMarkdown) -> MarkdownView {
    MarkdownView {
        source: source.into_markdown().0,
        theme: MarkdownTheme::default(),
        on_link: None,
        image: None,
        emoji: None,
        highlight: None,
        style: Style::default(),
    }
}

impl MarkdownView {
    pub fn theme(mut self, theme: MarkdownTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Called with the target of a link that is clicked. Links to headings of the document
    /// (`#name`) scroll to them instead.
    pub fn on_link(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_link = Some(Rc::new(f));
        self
    }

    /// Where the picture of `![alt](src)` comes from. By default `src` is a path.
    pub fn image(mut self, f: impl Fn(&str) -> ImageSource + 'static) -> Self {
        self.image = Some(Rc::new(f));
        self
    }

    /// What `:name:` is. Without this, it stays as it is written.
    pub fn emoji(mut self, f: impl Fn(&str) -> Option<Emoji> + 'static) -> Self {
        self.emoji = Some(Rc::new(f));
        self
    }

    /// Colors the code of a code block: given the language and the code, the ranges of the code
    /// with their styles, in order.
    pub fn highlight(
        mut self,
        f: impl Fn(&str, &str) -> Vec<(Range<usize>, Style)> + 'static,
    ) -> Self {
        self.highlight = Some(Rc::new(f));
        self
    }
}

impl Styled for MarkdownView {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

/// What the blocks of a document are made with.
struct Ctx {
    theme: MarkdownTheme,
    on_link: OnLink,
    image: Pictures,
    emoji: Emojis,
    highlight: Option<Rc<Highlight>>,
    /// The style of each kind of span, shared by all of them.
    spans: RefCell<HashMap<(u8, bool, u32), Rc<Style>>>,
    link_hover: Rc<Style>,
    /// Where the headings are, by their ids.
    anchors: RefCell<HashMap<String, NodeRef>>,
    /// The number of each footnote, in the order they are referred to.
    footnotes: RefCell<HashMap<String, usize>>,
    footnote_refs: RefCell<HashMap<String, NodeRef>>,
}

/// A block at the top of a document as it is shown.
struct Entry {
    key: (u64, u32, u32),
    gap: f32,
    kind: EntryKind,
}

enum EntryKind {
    Block(Block),
    Footnotes(Vec<(String, Vec<Block>)>),
}

fn px_of(l: Option<Length>) -> f32 {
    match l {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    }
}

fn margins(style: &Style) -> (f32, f32) {
    (px_of(style.margin_top), px_of(style.margin_bottom))
}

fn hash_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl Ctx {
    fn follow(&self, url: &str) {
        if let Some(id) = url.strip_prefix('#') {
            let target = self
                .anchors
                .borrow()
                .get(id)
                .copied()
                .or_else(|| self.footnote_refs.borrow().get(id).copied());
            if let Some(r) = target {
                r.scroll_into_view();
                return;
            }
        }
        if let Some(f) = &self.on_link {
            f(url);
        }
    }

    fn picture(&self, url: &str) -> ImageSource {
        match &self.image {
            Some(f) => f(url),
            None => ImageSource::from(url),
        }
    }

    fn margins_of(&self, block: &Block) -> (f32, f32) {
        let t = &self.theme;
        match block {
            Block::Heading(level, _) => margins(&t.headings[(*level as usize).clamp(1, 6) - 1]),
            Block::Paragraph(_) | Block::Html(_) => margins(&t.paragraph),
            Block::Plain(_) | Block::Footnote(..) => (0.0, 0.0),
            Block::Code { lang, .. } if lang == "mermaid" => margins(&t.diagram),
            Block::Code { .. } => margins(&t.code_block),
            Block::Quote(..) => margins(&t.quote),
            Block::List { .. } => margins(&t.list),
            Block::Table { .. } => margins(&t.table),
            Block::Rule => margins(&t.rule),
        }
    }

    /// The style of text with these marks, at a font size for what is raised or lowered.
    fn span_style(&self, bits: u8, link: bool, size: f32) -> Rc<Style> {
        let key = (bits, link, size.to_bits());
        if let Some(s) = self.spans.borrow().get(&key) {
            return s.clone();
        }
        let t = &self.theme;
        let mut s = Style::default();
        if bits & mark::STRONG != 0 {
            s.refine(&t.strong);
        }
        if bits & mark::EMPHASIS != 0 {
            s.refine(&t.emphasis);
        }
        if bits & mark::STRIKE != 0 {
            s.refine(&t.strikethrough);
        }
        if bits & mark::CODE != 0 {
            s.refine(&t.code);
        }
        if link {
            s.refine(&t.link);
        }
        if bits & (mark::SUPER | mark::SUB) != 0 {
            s.font_size = Some(size * 0.75);
        }
        let s = Rc::new(s);
        self.spans.borrow_mut().insert(key, s.clone());
        s
    }

    /// Splits text at the `:name:` that are emoji.
    fn emoji_runs(
        self: &Rc<Self>,
        text: &str,
        size: f32,
        out: &mut Vec<Run>,
        styled: &dyn Fn(String) -> Run,
    ) {
        let Some(resolve) = &self.emoji else {
            out.push(styled(text.to_owned()));
            return;
        };
        let mut rest = text;
        let mut plain = String::new();
        while let Some(start) = rest.find(':') {
            let after = &rest[start + 1..];
            let name_len = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-')))
                .unwrap_or(after.len());
            let emoji = (name_len > 0 && after[name_len..].starts_with(':'))
                .then(|| resolve(&after[..name_len]))
                .flatten();
            match emoji {
                Some(e) => {
                    plain.push_str(&rest[..start]);
                    let written = &rest[start..start + name_len + 2];
                    match e {
                        Emoji::Text(t) => plain.push_str(&t),
                        Emoji::Image(source) => {
                            if !plain.is_empty() {
                                out.push(styled(std::mem::take(&mut plain)));
                            }
                            let side = size * self.theme.emoji_size;
                            out.push(Run::Inline(
                                inline(img(source).w(side).h(side))
                                    .alt(written)
                                    .vertical_align(VerticalAlign::Px(
                                        -(side - size) / 2.0 - size * 0.12,
                                    )),
                            ));
                        }
                    }
                    rest = &rest[start + name_len + 2..];
                }
                None => {
                    plain.push_str(&rest[..start + 1]);
                    rest = after;
                }
            }
        }
        plain.push_str(rest);
        if !plain.is_empty() {
            out.push(styled(plain));
        }
    }

    /// A paragraph of inlines, at font size `size` (for emoji and what is raised).
    fn inlines(self: &Rc<Self>, pieces: &Inlines, size: f32) -> View {
        if let [Piece::Text(t, m)] = &pieces[..]
            && m.bits == 0
            && m.link.is_none()
            && self.emoji.is_none()
        {
            return text(t.clone()).into_view();
        }
        let mut runs: Vec<Run> = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Text(t, m) => {
                    if m.bits == 0 && m.link.is_none() {
                        self.emoji_runs(t, size, &mut runs, &|s| Run::Text(s));
                        continue;
                    }
                    let style = self.span_style(m.bits, m.link.is_some(), size);
                    let ctx = self.clone();
                    let link = m.link.clone();
                    let bits = m.bits;
                    let hover = self.link_hover.clone();
                    let make = move |s: String| {
                        let mut sp = span(s).shared_style(style.clone());
                        if bits & mark::SUPER != 0 {
                            sp = sp.vertical_align(VerticalAlign::Super);
                        } else if bits & mark::SUB != 0 {
                            sp = sp.vertical_align(VerticalAlign::Sub);
                        }
                        if let Some(url) = link.clone() {
                            let ctx = ctx.clone();
                            sp = sp
                                .shared_hover(hover.clone())
                                .on_click(move || ctx.follow(&url));
                        }
                        Run::Span(sp)
                    };
                    if bits & mark::CODE != 0 {
                        runs.push(make(t.clone()));
                    } else {
                        self.emoji_runs(t, size, &mut runs, &make);
                    }
                }
                Piece::Image { url, alt, link } => {
                    let pic = img(self.picture(url))
                        .style(&self.theme.image)
                        .max_w(pct(100.0));
                    let view = match link {
                        Some(url) => {
                            let ctx = self.clone();
                            let url = url.clone();
                            div()
                                .cursor(CursorIcon::Pointer)
                                .on_click(move |_| ctx.follow(&url))
                                .child(pic)
                                .into_view()
                        }
                        None => pic.into_view(),
                    };
                    runs.push(Run::Inline(inline(view).alt(alt.clone())));
                }
                Piece::FootnoteRef(label) => {
                    let n = self.footnotes.borrow().get(label).copied();
                    let shown = n.map_or_else(|| format!("[^{label}]"), |n| n.to_string());
                    let style = self.span_style(mark::SUPER, true, size);
                    let ctx = self.clone();
                    let target = format!("#fn-{label}");
                    runs.push(Run::Span(
                        span(shown)
                            .shared_style(style)
                            .vertical_align(VerticalAlign::Super)
                            .shared_hover(self.link_hover.clone())
                            .on_click(move || ctx.follow(&target)),
                    ));
                }
            }
        }
        rich_text(runs).into_view()
    }

    /// Blocks one under the other, with the larger of the margins of two neighbors between them.
    fn stack(self: &Rc<Self>, blocks: &[Block], depth: usize) -> Vec<View> {
        let mut prev = None;
        blocks
            .iter()
            .filter(|b| !matches!(b, Block::Footnote(..)))
            .map(|b| {
                let (top, bottom) = self.margins_of(b);
                let gap = prev.map_or(0.0, |p: f32| p.max(top));
                prev = Some(bottom);
                self.block(b, gap, depth)
            })
            .collect()
    }

    fn size_of(&self, style: &Style) -> f32 {
        style.font_size.unwrap_or(self.theme.base_size())
    }

    fn block(self: &Rc<Self>, block: &Block, gap: f32, depth: usize) -> View {
        let t = &self.theme;
        let base = t.base_size();
        match block {
            Block::Paragraph(p) => div()
                .style(&t.paragraph)
                .mt(gap)
                .mb(0.0)
                .child(self.inlines(p, base))
                .into_view(),
            Block::Plain(p) => div().mt(gap).child(self.inlines(p, base)).into_view(),
            Block::Heading(level, p) => {
                let style = &t.headings[(*level as usize).clamp(1, 6) - 1];
                let r = node_ref();
                let id = parse::slug(&parse::plain(p));
                self.anchors.borrow_mut().entry(id).or_insert(r);
                div()
                    .style(style)
                    .mt(gap)
                    .mb(0.0)
                    .node_ref(r)
                    .child(self.inlines(p, self.size_of(style)))
                    .into_view()
            }
            Block::Code { lang, text: code } => {
                #[cfg(feature = "mermaid")]
                if lang == "mermaid" {
                    return div()
                        .style(&t.diagram)
                        .mt(gap)
                        .mb(0.0)
                        .child(crate::mermaid::diagram(code))
                        .into_view();
                }
                self.code(lang, code, gap)
            }
            Block::Quote(alert, blocks) => self.quote(*alert, blocks, gap, depth),
            Block::List { start, items } => self.list(*start, items, gap, depth),
            Block::Table { aligns, head, rows } => self.table(aligns, head, rows, gap),
            Block::Rule => div().style(&t.rule).mt(gap).mb(0.0).into_view(),
            Block::Html(html) => {
                let (pieces, center) = html_inlines(html);
                if pieces.is_empty() {
                    return div().into_view();
                }
                let mut d = div().style(&t.paragraph).mt(gap).mb(0.0);
                if center {
                    d = d.text_align(TextAlign::Center);
                }
                d.child(self.inlines(&pieces, base)).into_view()
            }
            Block::Footnote(..) => div().into_view(),
        }
    }

    fn code(self: &Rc<Self>, lang: &str, code: &str, gap: f32) -> View {
        let t = &self.theme;
        let scroll = Style {
            overflow_x: Some(Overflow::Scroll),
            ..Style::default()
        };
        let body = match self.highlight.as_ref().map(|f| f(lang, code)) {
            Some(ranges) if !ranges.is_empty() => {
                let mut runs: Vec<Run> = Vec::new();
                let mut at = 0;
                for (range, style) in ranges {
                    if range.start < at || range.end > code.len() || range.start >= range.end {
                        continue;
                    }
                    if range.start > at {
                        runs.push(Run::Text(code[at..range.start].to_owned()));
                    }
                    runs.push(Run::Span(
                        span(code[range.clone()].to_owned()).shared_style(Rc::new(style)),
                    ));
                    at = range.end;
                }
                if at < code.len() {
                    runs.push(Run::Text(code[at..].to_owned()));
                }
                rich_text(runs)
                    .style(&t.code_text)
                    .whitespace_nowrap()
                    .into_view()
            }
            _ => text(code.to_owned())
                .style(&t.code_text)
                .whitespace_nowrap()
                .into_view(),
        };
        div()
            .style(&t.code_block)
            .style(&scroll)
            .mt(gap)
            .mb(0.0)
            .child(body)
            .into_view()
    }

    fn quote(
        self: &Rc<Self>,
        alert: Option<Alert>,
        blocks: &[Block],
        gap: f32,
        depth: usize,
    ) -> View {
        let t = &self.theme;
        let mut d = div().style(&t.quote).mt(gap).mb(0.0);
        let mut children = Vec::new();
        if let Some(a) = alert {
            let (i, name) = match a {
                Alert::Note => (0, "Note"),
                Alert::Tip => (1, "Tip"),
                Alert::Important => (2, "Important"),
                Alert::Warning => (3, "Warning"),
                Alert::Caution => (4, "Caution"),
            };
            let color = t.alerts[i];
            d = d.border_color(color).py(8.0);
            if let Some(c) = t.root.color {
                d = d.text_color(c);
            }
            children.push(
                div()
                    .mb(4.0)
                    .child(text(name).font_weight(FontWeight(600)).text_color(color))
                    .into_view(),
            );
        }
        children.extend(self.stack(blocks, depth));
        d.children(children).into_view()
    }

    fn list(self: &Rc<Self>, start: Option<u64>, items: &[Item], gap: f32, depth: usize) -> View {
        let t = &self.theme;
        let base = t.base_size();
        let line = base * 1.5;
        // A loose list has room between its items, as its paragraphs do.
        let loose = items
            .iter()
            .any(|i| i.blocks.iter().any(|b| matches!(b, Block::Paragraph(_))));
        let between = if loose {
            margins(&t.paragraph).1
        } else {
            t.item_gap
        };
        let bullets = ["•", "◦", "▪"];
        let rows = items.iter().enumerate().map(|(i, item)| {
            let marker: View = match (item.task, start) {
                (Some(done), _) => {
                    let side = 13.0;
                    let mut b = div()
                        .w(side)
                        .h(side)
                        .mt((line - side) / 2.0)
                        .border(1.0)
                        .border_color(Color::from_rgb(0x59636e))
                        .rounded(3.0)
                        .flex()
                        .items_center()
                        .justify_center();
                    if done {
                        b = b
                            .bg(Color::from_rgb(0x0969da))
                            .border_color(Color::from_rgb(0x0969da))
                            .child(
                                text("✓")
                                    .text_size(11.0)
                                    .line_height(LineHeight::Relative(1.0))
                                    .text_color(Color::WHITE)
                                    .select_none(),
                            );
                    }
                    b.into_view()
                }
                (None, Some(n)) => text(format!("{}.", n + i as u64)).select_none().into_view(),
                (None, None) => text(bullets[depth % 3]).select_none().into_view(),
            };
            div()
                .flex()
                .flex_row()
                .items_start()
                .mt(if i > 0 { between } else { 0.0 })
                .child(
                    div()
                        .w(t.indent)
                        .flex_none()
                        .flex()
                        .justify_end()
                        .pr(base * 0.4)
                        .child(marker),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(0.0)
                        .children(self.stack(&item.blocks, depth + 1)),
                )
        });
        div()
            .style(&t.list)
            .mt(gap)
            .mb(0.0)
            .flex()
            .flex_col()
            .children(rows)
            .into_view()
    }

    /// A table as columns: each is as wide as its widest cell, and the cells of a row are as
    /// tall as each other when they have one line each, which is the usual case.
    fn table(
        self: &Rc<Self>,
        aligns: &[Align],
        head: &[Inlines],
        rows: &[Vec<Inlines>],
        gap: f32,
    ) -> View {
        let t = &self.theme;
        let base = t.base_size();
        let columns = rows
            .iter()
            .map(Vec::len)
            .chain([head.len(), aligns.len()])
            .max()
            .unwrap_or(0);
        let border = t.table_cell.border_top_width.unwrap_or(0.0);
        let empty = Vec::new();
        // The tallest cell of each row, which the others of the row are as tall as: a part with
        // another font can make a line a little taller.
        let tallest: Vec<Signal<f32>> = (0..=rows.len()).map(|_| signal(0.0)).collect();
        let cell = |c: usize, r: usize, pieces: &Inlines| {
            let row = tallest[r];
            let mut d = div()
                .style(&t.table_cell)
                .style(&Style {
                    border_top_width: Some(if r == 0 { border } else { 0.0 }),
                    border_left_width: Some(if c == 0 { border } else { 0.0 }),
                    ..Style::default()
                })
                .whitespace_nowrap();
            if r == 0 {
                d = d.style(&t.table_head);
            } else if r.is_multiple_of(2) {
                d = d.bg(t.table_stripe);
            }
            d = match aligns.get(c).copied().unwrap_or(Align::None) {
                Align::Center => d.text_align(TextAlign::Center),
                Align::Right => d.text_align(TextAlign::End),
                _ => d,
            };
            d.on_resized(move |_, h| {
                if h > row.get_untracked() {
                    row.set(h);
                }
            })
            .dyn_style(move |s| s.min_h(row.get()))
            .child(self.inlines(pieces, base))
        };
        let cols = (0..columns).map(|c| {
            let mut cells = vec![cell(c, 0, head.get(c).unwrap_or(&empty))];
            for (r, row) in rows.iter().enumerate() {
                cells.push(cell(c, r + 1, row.get(c).unwrap_or(&empty)));
            }
            div().flex().flex_col().flex_none().children(cells)
        });
        let scroll = Style {
            overflow_x: Some(Overflow::Scroll),
            ..Style::default()
        };
        div()
            .style(&t.table)
            .style(&scroll)
            .mt(gap)
            .mb(0.0)
            .child(div().flex().flex_row().items_start().children(cols))
            .into_view()
    }

    fn footnotes(self: &Rc<Self>, notes: &[(String, Vec<Block>)], gap: f32) -> View {
        let t = &self.theme;
        let rows = notes.iter().enumerate().map(|(i, (label, blocks))| {
            let r = node_ref();
            self.footnote_refs
                .borrow_mut()
                .insert(format!("fn-{label}"), r);
            div()
                .flex()
                .flex_row()
                .items_start()
                .node_ref(r)
                .mt(if i > 0 { 4.0 } else { 0.0 })
                .child(
                    div()
                        .w(t.indent)
                        .flex_none()
                        .flex()
                        .justify_end()
                        .pr(6.0)
                        .child(text(format!("{}.", i + 1)).select_none()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(0.0)
                        .children(self.stack(blocks, 1)),
                )
        });
        div()
            .style(&t.footnotes)
            .mt(gap.max(px_of(t.footnotes.margin_top)))
            .children(rows)
            .into_view()
    }

    /// Numbers the footnotes in the order they are referred to.
    fn number_footnotes(&self, tops: &[Top]) {
        fn walk(blocks: &mut dyn Iterator<Item = &Block>, out: &mut HashMap<String, usize>) {
            let inl = |p: &Inlines, out: &mut HashMap<String, usize>| {
                for piece in p {
                    if let Piece::FootnoteRef(l) = piece {
                        let n = out.len() + 1;
                        out.entry(l.clone()).or_insert(n);
                    }
                }
            };
            for b in blocks {
                match b {
                    Block::Paragraph(p) | Block::Plain(p) | Block::Heading(_, p) => inl(p, out),
                    Block::Quote(_, bs) | Block::Footnote(_, bs) => walk(&mut bs.iter(), out),
                    Block::List { items, .. } => {
                        for i in items {
                            walk(&mut i.blocks.iter(), out);
                        }
                    }
                    Block::Table { head, rows, .. } => {
                        for c in head.iter().chain(rows.iter().flatten()) {
                            inl(c, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = HashMap::new();
        walk(&mut tops.iter().map(|t| &t.block), &mut out);
        *self.footnotes.borrow_mut() = out;
    }

    /// The blocks at the top of a document, with keys that stay while their source does.
    fn entries(self: &Rc<Self>, source: &str) -> Vec<Rc<Entry>> {
        let tops = parse::parse(source);
        self.number_footnotes(&tops);
        let mut out = Vec::new();
        let mut seen: HashMap<u64, u32> = HashMap::new();
        let mut prev: Option<f32> = None;
        let mut notes = Vec::new();
        let mut notes_hash = DefaultHasher::new();
        for top in tops {
            if let Block::Footnote(label, blocks) = top.block {
                source[top.range.clone()].hash(&mut notes_hash);
                notes.push((label, blocks));
                continue;
            }
            let (top_margin, bottom) = self.margins_of(&top.block);
            let gap = prev.map_or(0.0, |p| p.max(top_margin));
            prev = Some(bottom);
            let hash = hash_of(&source[top.range.clone()]);
            let n = seen.entry(hash).or_insert(0);
            *n += 1;
            out.push(Rc::new(Entry {
                key: (hash, *n, gap.to_bits()),
                gap,
                kind: EntryKind::Block(top.block),
            }));
        }
        if !notes.is_empty() {
            let numbers = self.footnotes.borrow();
            notes.sort_by_key(|(l, _)| numbers.get(l).copied().unwrap_or(usize::MAX));
            drop(numbers);
            let gap = prev.unwrap_or(0.0);
            out.push(Rc::new(Entry {
                key: (notes_hash.finish(), 0, gap.to_bits()),
                gap,
                kind: EntryKind::Footnotes(notes),
            }));
        }
        out
    }
}

impl IntoView for MarkdownView {
    fn into_view(self) -> View {
        let ctx = Rc::new(Ctx {
            link_hover: Rc::new(self.theme.link_hover.clone()),
            theme: self.theme,
            on_link: self.on_link,
            image: self.image,
            emoji: self.emoji,
            highlight: self.highlight,
            spans: RefCell::default(),
            anchors: RefCell::default(),
            footnotes: RefCell::default(),
            footnote_refs: RefCell::default(),
        });
        let source = self.source;
        let items = {
            let ctx = ctx.clone();
            move || match &source {
                Source::Static(s) => ctx.entries(s),
                Source::Dynamic(f) => ctx.entries(&f()),
            }
        };
        let view = {
            let ctx = ctx.clone();
            move |e: Rc<Entry>| {
                lazy(match &e.kind {
                    EntryKind::Block(b) => ctx.block(b, e.gap, 0),
                    EntryKind::Footnotes(notes) => ctx.footnotes(notes, e.gap),
                })
            }
        };
        div()
            .style(&ctx.theme.root)
            .style(&self.style)
            .child(for_each(items, |e: &Rc<Entry>| e.key, view))
            .into_view()
    }
}

/// A block that isn't laid out, nor its text shaped, while it is far from the screen. See
/// `docs/design/29-content-visibility.md`. The block is made first, so that what is in it isn't
/// made on top of this on the stack.
fn lazy(block: View) -> View {
    div()
        .content_visibility_auto()
        .estimate_contents()
        .child(block)
        .into_view()
}

/// What of a block of HTML can be shown without a browser: its text, its pictures, its links and
/// its line breaks. Says whether it is centered, as READMEs often are.
fn html_inlines(html: &str) -> (Inlines, bool) {
    let mut out: Inlines = Vec::new();
    let mut link: Option<Rc<str>> = None;
    let mut center = false;
    let mut rest = html;
    let push_text = |out: &mut Inlines, t: &str, link: &Option<Rc<str>>| {
        let t = decode_entities(t);
        // Whitespace between tags is how HTML is laid out in the file, not text.
        let collapsed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            return;
        }
        let marks = parse::Marks {
            bits: 0,
            link: link.clone(),
        };
        match out.last_mut() {
            Some(Piece::Text(s, m)) if *m == marks => {
                if !s.ends_with(['\n', ' ']) {
                    s.push(' ');
                }
                s.push_str(&collapsed);
            }
            _ => out.push(Piece::Text(collapsed, marks)),
        }
    };
    while let Some(open) = rest.find('<') {
        push_text(&mut out, &rest[..open], &link);
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        let tag = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let lower = tag.to_ascii_lowercase();
        let name = lower
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if lower.contains("align=\"center\"") || lower.contains("align=center") {
            center = true;
        }
        let closing = lower.starts_with('/');
        match name {
            "img" if !closing => {
                if let Some(src) = attribute(tag, "src") {
                    out.push(Piece::Image {
                        url: src,
                        alt: attribute(tag, "alt").unwrap_or_default(),
                        link: link.clone(),
                    });
                }
            }
            "a" if !closing => link = attribute(tag, "href").map(|h| Rc::from(h.as_str())),
            "a" => link = None,
            "br" | "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" => {
                if let Some(Piece::Text(s, _)) = out.last_mut()
                    && !s.ends_with('\n')
                {
                    s.push('\n');
                }
            }
            _ => {}
        }
    }
    push_text(&mut out, rest, &link);
    if let Some(Piece::Text(s, _)) = out.last_mut() {
        while s.ends_with('\n') {
            s.pop();
        }
    }
    (out, center)
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let value_at = tag.len() - rest.len() + 1;
        let value = tag[value_at..].trim_start();
        return Some(match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or("").to_owned(),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or("")
                .to_owned(),
        });
    }
    None
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&nbsp;", "\u{a0}")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_of_html_shows() {
        let (pieces, center) = html_inlines(
            "<p align=\"center\">\n  <img src=\"logo.png\" alt=\"Logo\" width=100>\n  <br>\n  <a href=\"x\">Docs &amp; more</a>\n</p>",
        );
        assert!(center);
        assert_eq!(
            pieces[0],
            Piece::Image {
                url: "logo.png".into(),
                alt: "Logo".into(),
                link: None
            }
        );
        assert!(
            matches!(&pieces[1], Piece::Text(t, m) if t == "Docs & more" && m.link.as_deref() == Some("x"))
        );
    }
}
