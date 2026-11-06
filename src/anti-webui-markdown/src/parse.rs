//! Markdown to a light list of blocks, with `pulldown-cmark`. What is shown is made from these, so
//! nothing of the parser stays around once a document is read.

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use std::ops::Range;
use std::rc::Rc;

/// What the text of a run is marked as.
pub(crate) mod mark {
    pub const STRONG: u8 = 1;
    pub const EMPHASIS: u8 = 2;
    pub const STRIKE: u8 = 4;
    pub const CODE: u8 = 8;
    pub const SUPER: u8 = 16;
    pub const SUB: u8 = 32;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Marks {
    pub bits: u8,
    pub link: Option<Rc<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Piece {
    Text(String, Marks),
    Image {
        url: String,
        alt: String,
        link: Option<Rc<str>>,
    },
    FootnoteRef(String),
}

pub(crate) type Inlines = Vec<Piece>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Heading(u8, Inlines),
    Paragraph(Inlines),
    /// A paragraph of a tight list, without room around it.
    Plain(Inlines),
    Code {
        lang: String,
        text: String,
    },
    Quote(Option<Alert>, Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Inlines>,
        rows: Vec<Vec<Inlines>>,
    },
    Rule,
    Html(String),
    Footnote(String, Vec<Block>),
}

/// A block at the top of a document, with where it is in the source.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Top {
    pub range: Range<usize>,
    pub block: Block,
}

pub(crate) fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
}

enum Frame {
    Quote(Option<Alert>, Vec<Block>),
    List(Option<u64>, Vec<Item>),
    Item(Option<bool>, Vec<Block>, Inlines),
    Footnote(String, Vec<Block>),
    Table {
        aligns: Vec<Align>,
        head: Vec<Inlines>,
        rows: Vec<Vec<Inlines>>,
        row: Vec<Inlines>,
    },
    Inlines(Inlines),
    Image {
        url: String,
        alt: String,
    },
    Code(String, String),
    Html(String),
}

struct Builder {
    tops: Vec<Top>,
    stack: Vec<Frame>,
    marks: Marks,
    /// How many of each mark are open, as they nest.
    depth: [u8; 6],
    links: Vec<Option<Rc<str>>>,
    start: usize,
}

impl Builder {
    fn bits(&self) -> u8 {
        self.depth
            .iter()
            .enumerate()
            .filter(|(_, d)| **d > 0)
            .fold(0, |b, (i, _)| b | (1 << i))
    }

    fn open_mark(&mut self, bit: u8, open: bool) {
        let i = bit.trailing_zeros() as usize;
        if open {
            self.depth[i] += 1;
        } else {
            self.depth[i] = self.depth[i].saturating_sub(1);
        }
        self.marks.bits = self.bits();
    }

    /// Where inline things go now: a paragraph, a heading, a cell, the alt of an image, or a
    /// list item whose text is not in a paragraph.
    fn inlines(&mut self) -> Option<&mut Inlines> {
        match self.stack.last_mut()? {
            Frame::Inlines(i) | Frame::Item(_, _, i) => Some(i),
            _ => None,
        }
    }

    fn text(&mut self, text: &str, extra: u8) {
        if let Some(Frame::Image { alt, .. }) = self.stack.last_mut() {
            alt.push_str(text);
            return;
        }
        if let Some(Frame::Code(_, code) | Frame::Html(code)) = self.stack.last_mut() {
            code.push_str(text);
            return;
        }
        let mut marks = self.marks.clone();
        marks.bits |= extra;
        let Some(inlines) = self.inlines() else {
            return;
        };
        match inlines.last_mut() {
            Some(Piece::Text(t, m)) if *m == marks => t.push_str(text),
            _ => inlines.push(Piece::Text(text.to_owned(), marks)),
        }
    }

    /// Puts a finished block where it goes.
    fn block(&mut self, block: Block, end: usize) {
        match self.stack.last_mut() {
            Some(Frame::Quote(_, blocks) | Frame::Footnote(_, blocks)) => blocks.push(block),
            Some(Frame::Item(_, blocks, inlines)) => {
                if !inlines.is_empty() {
                    blocks.push(Block::Plain(std::mem::take(inlines)));
                }
                blocks.push(block);
            }
            _ => self.tops.push(Top {
                range: self.start..end,
                block,
            }),
        }
    }

    fn start(&mut self, tag: Tag<'_>, at: usize) {
        if self.stack.is_empty() {
            self.start = at;
        }
        match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => {
                self.stack.push(Frame::Inlines(Vec::new()));
            }
            Tag::BlockQuote(kind) => self.stack.push(Frame::Quote(
                kind.map(|k| match k {
                    BlockQuoteKind::Note => Alert::Note,
                    BlockQuoteKind::Tip => Alert::Tip,
                    BlockQuoteKind::Important => Alert::Important,
                    BlockQuoteKind::Warning => Alert::Warning,
                    BlockQuoteKind::Caution => Alert::Caution,
                }),
                Vec::new(),
            )),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.stack.push(Frame::Code(lang, String::new()));
            }
            Tag::HtmlBlock => self.stack.push(Frame::Html(String::new())),
            Tag::List(start) => self.stack.push(Frame::List(start, Vec::new())),
            Tag::Item => self.stack.push(Frame::Item(None, Vec::new(), Vec::new())),
            Tag::FootnoteDefinition(label) => {
                self.stack
                    .push(Frame::Footnote(label.to_string(), Vec::new()));
            }
            Tag::Table(aligns) => self.stack.push(Frame::Table {
                aligns: aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::None => Align::None,
                        Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    })
                    .collect(),
                head: Vec::new(),
                rows: Vec::new(),
                row: Vec::new(),
            }),
            Tag::TableHead | Tag::TableRow => {}
            Tag::Emphasis => self.open_mark(mark::EMPHASIS, true),
            Tag::Strong => self.open_mark(mark::STRONG, true),
            Tag::Strikethrough => self.open_mark(mark::STRIKE, true),
            Tag::Superscript => self.open_mark(mark::SUPER, true),
            Tag::Subscript => self.open_mark(mark::SUB, true),
            Tag::Link { dest_url, .. } => {
                self.links.push(self.marks.link.take());
                self.marks.link = Some(Rc::from(dest_url.as_ref()));
            }
            Tag::Image { dest_url, .. } => self.stack.push(Frame::Image {
                url: dest_url.to_string(),
                alt: String::new(),
            }),
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd, end: usize) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                let Some(Frame::Inlines(inlines)) = self.stack.pop() else {
                    return;
                };
                let block = match tag {
                    TagEnd::Heading(level) => Block::Heading(level_of(level), inlines),
                    _ => Block::Paragraph(inlines),
                };
                self.block(block, end);
            }
            TagEnd::TableCell => {
                let Some(Frame::Inlines(inlines)) = self.stack.pop() else {
                    return;
                };
                if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                    row.push(inlines);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Frame::Table {
                    head, rows, row, ..
                }) = self.stack.last_mut()
                {
                    let cells = std::mem::take(row);
                    if tag == TagEnd::TableHead {
                        *head = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            TagEnd::Table => {
                if let Some(Frame::Table {
                    aligns, head, rows, ..
                }) = self.stack.pop()
                {
                    self.block(Block::Table { aligns, head, rows }, end);
                }
            }
            TagEnd::BlockQuote(_) => {
                if let Some(Frame::Quote(kind, blocks)) = self.stack.pop() {
                    self.block(Block::Quote(kind, blocks), end);
                }
            }
            TagEnd::CodeBlock => {
                if let Some(Frame::Code(lang, mut text)) = self.stack.pop() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.block(Block::Code { lang, text }, end);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(Frame::Html(mut text)) = self.stack.pop() {
                    while text.ends_with('\n') {
                        text.pop();
                    }
                    self.block(Block::Html(text), end);
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List(start, items)) = self.stack.pop() {
                    self.block(Block::List { start, items }, end);
                }
            }
            TagEnd::Item => {
                if let Some(Frame::Item(task, mut blocks, inlines)) = self.stack.pop() {
                    if !inlines.is_empty() {
                        blocks.push(Block::Plain(inlines));
                    }
                    if let Some(Frame::List(_, items)) = self.stack.last_mut() {
                        items.push(Item { task, blocks });
                    }
                }
            }
            TagEnd::FootnoteDefinition => {
                if let Some(Frame::Footnote(label, blocks)) = self.stack.pop() {
                    self.block(Block::Footnote(label, blocks), end);
                }
            }
            TagEnd::Emphasis => self.open_mark(mark::EMPHASIS, false),
            TagEnd::Strong => self.open_mark(mark::STRONG, false),
            TagEnd::Strikethrough => self.open_mark(mark::STRIKE, false),
            TagEnd::Superscript => self.open_mark(mark::SUPER, false),
            TagEnd::Subscript => self.open_mark(mark::SUB, false),
            TagEnd::Link => self.marks.link = self.links.pop().flatten(),
            TagEnd::Image => {
                if let Some(Frame::Image { url, alt }) = self.stack.pop() {
                    let link = self.marks.link.clone();
                    if let Some(inlines) = self.inlines() {
                        inlines.push(Piece::Image { url, alt, link });
                    }
                }
            }
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => {}
        }
    }
}

fn level_of(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Reads a whole document.
pub(crate) fn parse(source: &str) -> Vec<Top> {
    let mut b = Builder {
        tops: Vec::new(),
        stack: Vec::new(),
        marks: Marks::default(),
        depth: [0; 6],
        links: Vec::new(),
        start: 0,
    };
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => b.start(tag, range.start),
            Event::End(tag) => b.end(tag, range.end),
            Event::Text(t) => b.text(&t, 0),
            Event::Code(t) => b.text(&t, mark::CODE),
            Event::InlineMath(t) | Event::DisplayMath(t) => b.text(&t, mark::CODE),
            Event::Html(t) => {
                if b.stack.is_empty() {
                    b.start = range.start;
                    b.block(Block::Html(t.trim_end().to_owned()), range.end);
                } else {
                    b.text(&t, 0);
                }
            }
            Event::InlineHtml(t) => {
                // Line breaks written as HTML are common in READMEs; other tags show as written.
                let tag = t.trim().to_ascii_lowercase();
                if matches!(tag.as_str(), "<br>" | "<br/>" | "<br />") {
                    b.text("\n", 0);
                } else {
                    b.text(&t, 0);
                }
            }
            Event::FootnoteReference(label) => {
                if let Some(inlines) = b.inlines() {
                    inlines.push(Piece::FootnoteRef(label.to_string()));
                }
            }
            Event::SoftBreak => b.text(" ", 0),
            Event::HardBreak => b.text("\n", 0),
            Event::Rule => {
                if b.stack.is_empty() {
                    b.start = range.start;
                }
                b.block(Block::Rule, range.end);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item(task, ..)) = b.stack.last_mut() {
                    *task = Some(done);
                }
            }
        }
    }
    b.tops
}

/// The plain text of some inlines, as a slug for a heading is made from it.
/// Where `^raised^` and `~lowered~` are in a text, as Pandoc has them: no spaces between the marks,
/// and inside words too (`x^2^`, `H~2~O`), which `pulldown-cmark` leaves as they are. A `~~` is
/// a line through. Says for each whether it is raised.
pub(crate) fn scripts(s: &str) -> Vec<(Range<usize>, bool)> {
    let b = s.as_bytes();
    let tilde_run = |i: usize| b.get(i + 1) == Some(&b'~') || (i > 0 && b[i - 1] == b'~');
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if (c == b'^' || (c == b'~' && !tilde_run(i))) && i + 1 < b.len() {
            let mut j = i + 1;
            while j < b.len() && b[j] != c && !b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == c && j > i + 1 && !(c == b'~' && tilde_run(j)) {
                out.push((i..j + 1, c == b'^'));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The inlines with what [`scripts`] finds in their text raised or lowered. `None` when there is
/// nothing.
pub(crate) fn split_scripts(pieces: &Inlines) -> Option<Inlines> {
    let found = |t: &str, m: &Marks| m.bits & mark::CODE == 0 && t.contains(['^', '~']);
    if !pieces
        .iter()
        .any(|p| matches!(p, Piece::Text(t, m) if found(t, m)))
    {
        return None;
    }
    let mut out = Vec::with_capacity(pieces.len() + 2);
    for p in pieces {
        let Piece::Text(t, m) = p else {
            out.push(p.clone());
            continue;
        };
        let runs = if found(t, m) { scripts(t) } else { Vec::new() };
        let mut at = 0;
        for (r, up) in runs {
            if r.start > at {
                out.push(Piece::Text(t[at..r.start].to_owned(), m.clone()));
            }
            let mut inner = m.clone();
            inner.bits |= if up { mark::SUPER } else { mark::SUB };
            out.push(Piece::Text(t[r.start + 1..r.end - 1].to_owned(), inner));
            at = r.end;
        }
        if at < t.len() {
            out.push(Piece::Text(t[at..].to_owned(), m.clone()));
        }
    }
    Some(out)
}

pub(crate) fn plain(inlines: &Inlines) -> String {
    let mut out = String::new();
    for p in inlines {
        match p {
            Piece::Text(t, _) => out.push_str(t),
            Piece::Image { alt, .. } => out.push_str(alt),
            Piece::FootnoteRef(_) => {}
        }
    }
    out
}

/// The id GitHub gives a heading: lowercase, spaces to `-`, and only letters, digits, `-` and `_`.
pub(crate) fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| {
            if c == ' ' {
                Some('-')
            } else if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c.to_lowercase().next().unwrap_or(c))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str, bits: u8) -> Piece {
        Piece::Text(t.into(), Marks { bits, link: None })
    }

    #[test]
    fn raised_and_lowered_inside_words() {
        let s = "x^2^ and H~2~O, 上付き^2^と ~~line~~ a^b c^";
        let found: Vec<(&str, bool)> = scripts(s).into_iter().map(|(r, up)| (&s[r], up)).collect();
        assert_eq!(found, [("^2^", true), ("~2~", false), ("^2^", true)]);
    }

    #[test]
    fn blocks_and_marks() {
        let doc = parse("# Title\n\nSome **bold _both_** and `code`.\n\n---\n");
        assert_eq!(doc.len(), 3);
        assert_eq!(doc[0].block, Block::Heading(1, vec![text("Title", 0)]));
        assert_eq!(doc[0].range, 0..8);
        let Block::Paragraph(p) = &doc[1].block else {
            panic!()
        };
        assert_eq!(
            p,
            &vec![
                text("Some ", 0),
                text("bold ", mark::STRONG),
                text("both", mark::STRONG | mark::EMPHASIS),
                text(" and ", 0),
                text("code", mark::CODE),
                text(".", 0),
            ]
        );
        assert_eq!(doc[2].block, Block::Rule);
    }

    #[test]
    fn lists_tasks_tables_and_quotes() {
        let doc = parse(
            "- [x] done\n- [ ] todo\n  1. inner\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\n> [!NOTE]\n> hi\n",
        );
        let Block::List { start: None, items } = &doc[0].block else {
            panic!("{:?}", doc[0].block)
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert!(matches!(
            items[1].blocks[1],
            Block::List { start: Some(1), .. }
        ));
        let Block::Table { aligns, head, rows } = &doc[1].block else {
            panic!()
        };
        assert_eq!(aligns, &[Align::Left, Align::Right]);
        assert_eq!(head.len(), 2);
        assert_eq!(rows[0][1], vec![text("2", 0)]);
        assert!(matches!(&doc[2].block, Block::Quote(Some(Alert::Note), b) if b.len() == 1));
    }

    #[test]
    fn links_images_and_code() {
        let doc = parse("[a **b**](u) ![alt](p.png)\n\n```mermaid x\ngraph TD\n```\n");
        let Block::Paragraph(p) = &doc[0].block else {
            panic!()
        };
        let link = Some(Rc::from("u"));
        assert_eq!(
            p[0],
            Piece::Text(
                "a ".into(),
                Marks {
                    bits: 0,
                    link: link.clone()
                }
            )
        );
        assert_eq!(
            p[1],
            Piece::Text(
                "b".into(),
                Marks {
                    bits: mark::STRONG,
                    link
                }
            )
        );
        assert_eq!(
            p[3],
            Piece::Image {
                url: "p.png".into(),
                alt: "alt".into(),
                link: None
            }
        );
        assert_eq!(
            doc[1].block,
            Block::Code {
                lang: "mermaid".into(),
                text: "graph TD".into()
            }
        );
    }

    #[test]
    fn slugs_are_githubs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("API の使い方"), "api-の使い方");
    }
}
