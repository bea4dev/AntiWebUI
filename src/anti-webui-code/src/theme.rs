//! How code looks: the colors of what the grammar finds, and of the editor around it.

use anti_webui_core::prelude::*;
use anti_webui_core::style::{FontFamily, FontWeight};
use std::rc::Rc;

/// The fonts code is shown in, the first one there is.
pub const MONO: &str = "\"JetBrains Mono\", \"Cascadia Code\", \"Source Code Pro\", \"DejaVu Sans Mono\", \"Noto Sans Mono\", ui-monospace, monospace";

/// How the editor and the code in it look.
#[derive(Clone, Debug)]
pub struct CodeTheme {
    pub background: Color,
    pub foreground: Color,
    pub line_numbers: Color,
    pub current_line_number: Color,
    pub current_line: Color,
    pub selection: Color,
    pub match_background: Color,
    pub current_match_background: Color,
    pub bracket_background: Color,
    pub fold_marker: Color,
    pub font_family: String,
    pub font_size: f32,
    /// The style of each name the grammars give (`keyword`, `function.method`, ...). A name that
    /// isn't here takes the style of the name before its last dot (`function`).
    pub captures: Vec<(&'static str, Rc<Style>)>,
}

fn color(c: u32) -> Rc<Style> {
    Rc::new(Style::default().text_color(rgb(c)))
}

fn italic(c: u32) -> Rc<Style> {
    Rc::new(Style::default().text_color(rgb(c)).italic())
}

fn bold(c: u32) -> Rc<Style> {
    Rc::new(
        Style::default()
            .text_color(rgb(c))
            .font_weight(FontWeight::BOLD),
    )
}

/// The colors of a palette, by what they are for.
struct Palette {
    comment: u32,
    keyword: u32,
    string: u32,
    number: u32,
    function: u32,
    kind: u32,
    property: u32,
    operator: u32,
    punctuation: u32,
    builtin: u32,
    attribute: u32,
    tag: u32,
    escape: u32,
    heading: u32,
    link: u32,
}

fn captures(p: &Palette) -> Vec<(&'static str, Rc<Style>)> {
    vec![
        ("comment", italic(p.comment)),
        ("keyword", color(p.keyword)),
        ("conditional", color(p.keyword)),
        ("repeat", color(p.keyword)),
        ("include", color(p.keyword)),
        ("exception", color(p.keyword)),
        ("storageclass", color(p.keyword)),
        ("string", color(p.string)),
        ("string.special", color(p.escape)),
        ("string.escape", color(p.escape)),
        ("escape", color(p.escape)),
        ("character", color(p.string)),
        ("number", color(p.number)),
        ("float", color(p.number)),
        ("boolean", color(p.number)),
        ("constant", color(p.number)),
        ("constant.builtin", color(p.number)),
        ("function", color(p.function)),
        ("function.builtin", color(p.builtin)),
        ("function.macro", color(p.builtin)),
        ("method", color(p.function)),
        ("constructor", color(p.kind)),
        ("type", color(p.kind)),
        ("type.builtin", color(p.kind)),
        ("namespace", color(p.kind)),
        ("module", color(p.kind)),
        ("property", color(p.property)),
        ("field", color(p.property)),
        ("variable.member", color(p.property)),
        ("variable.builtin", color(p.builtin)),
        ("variable.parameter", color(p.property)),
        ("label", color(p.attribute)),
        ("attribute", color(p.attribute)),
        ("tag", color(p.tag)),
        ("tag.attribute", color(p.attribute)),
        ("operator", color(p.operator)),
        ("punctuation", color(p.punctuation)),
        ("punctuation.special", color(p.keyword)),
        ("embedded", color(p.operator)),
        // Markdown.
        ("text.title", bold(p.heading)),
        ("markup.heading", bold(p.heading)),
        ("text.literal", color(p.string)),
        ("markup.raw", color(p.string)),
        ("text.uri", color(p.link)),
        ("markup.link.url", color(p.link)),
        ("text.reference", color(p.link)),
        ("markup.link.label", color(p.link)),
        ("text.emphasis", Rc::new(Style::default().italic())),
        ("markup.italic", Rc::new(Style::default().italic())),
        (
            "text.strong",
            Rc::new(Style::default().font_weight(FontWeight::BOLD)),
        ),
        (
            "markup.strong",
            Rc::new(Style::default().font_weight(FontWeight::BOLD)),
        ),
        ("markup.list", color(p.keyword)),
        ("markup.quote", italic(p.comment)),
    ]
}

impl CodeTheme {
    pub fn dark() -> Self {
        let p = Palette {
            comment: 0x7d8695,
            keyword: 0xcf94f0,
            string: 0xa9d18e,
            number: 0xf4a96a,
            function: 0x75b5ff,
            kind: 0x5fd4c4,
            property: 0xe8c483,
            operator: 0xb9c2cf,
            punctuation: 0x9aa4b2,
            builtin: 0xf2898a,
            attribute: 0xe8c483,
            tag: 0xf2898a,
            escape: 0x5ed0e0,
            heading: 0xf2898a,
            link: 0x75b5ff,
        };
        CodeTheme {
            background: rgb(0x1d1f24),
            foreground: rgb(0xd6d9df),
            line_numbers: rgb(0x5c6370),
            current_line_number: rgb(0xc8ccd4),
            current_line: rgba(0xffffff0b),
            selection: rgba(0x4d7ce07a),
            match_background: rgba(0xe5b75a44),
            current_match_background: rgba(0xf09c3caa),
            bracket_background: rgba(0xffffff26),
            fold_marker: rgb(0x8a93a3),
            font_family: MONO.into(),
            font_size: 14.0,
            captures: captures(&p),
        }
    }

    pub fn light() -> Self {
        let p = Palette {
            comment: 0x6f7782,
            keyword: 0x9a2fb4,
            string: 0x2f7d32,
            number: 0xb35900,
            function: 0x1a5fc2,
            kind: 0x00796b,
            property: 0x8a5a00,
            operator: 0x444b55,
            punctuation: 0x5c6470,
            builtin: 0xc0392b,
            attribute: 0x8a5a00,
            tag: 0xc0392b,
            escape: 0x00838f,
            heading: 0xc0392b,
            link: 0x1a5fc2,
        };
        CodeTheme {
            background: rgb(0xffffff),
            foreground: rgb(0x24292f),
            line_numbers: rgb(0x9aa1ab),
            current_line_number: rgb(0x3d434b),
            current_line: rgba(0x0000000a),
            selection: rgba(0x3b82f655),
            match_background: rgba(0xf5c34f66),
            current_match_background: rgba(0xf08c2abb),
            bracket_background: rgba(0x0000001f),
            fold_marker: rgb(0x6f7782),
            font_family: MONO.into(),
            font_size: 14.0,
            captures: captures(&p),
        }
    }

    /// The style of a name the grammar gives, or of the name before its last dot, and so on.
    pub fn style_of(&self, name: &str) -> Option<Rc<Style>> {
        let mut name = name;
        loop {
            if let Some((_, s)) = self.captures.iter().find(|(n, _)| *n == name) {
                return Some(s.clone());
            }
            name = &name[..name.rfind('.')?];
        }
    }

    pub(crate) fn font(&self) -> FontFamily {
        FontFamily::from(self.font_family.as_str())
    }
}

impl Default for CodeTheme {
    fn default() -> Self {
        Self::dark()
    }
}
