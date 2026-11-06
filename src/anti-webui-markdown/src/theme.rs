//! How Markdown looks: a `Style` for each kind of thing, as a stylesheet for it would have. The
//! default is close to GitHub's.

use anti_webui_core::style::{Color, FontFamily, FontWeight, LineHeight, Style, Styled};

/// The styles of everything a document can have. The margins on the top and bottom of blocks
/// collapse as they do in CSS: between two blocks there is the larger of the two.
#[derive(Clone, Debug)]
pub struct MarkdownTheme {
    /// What everything inherits: the font, its size, the color and the line height.
    pub root: Style,
    pub paragraph: Style,
    /// `#` to `######`.
    pub headings: [Style; 6],
    pub strong: Style,
    pub emphasis: Style,
    pub strikethrough: Style,
    /// `code` in a paragraph.
    pub code: Style,
    pub link: Style,
    /// What a link changes to while the pointer is over it.
    pub link_hover: Style,
    /// The box of a code block.
    pub code_block: Style,
    /// The text in a code block.
    pub code_text: Style,
    pub quote: Style,
    /// Each kind of `> [!NOTE]`: note, tip, important, warning and caution.
    pub alerts: [Color; 5],
    pub list: Style,
    /// Between the items of a list.
    pub item_gap: f32,
    /// How far the content of an item is from the edge of its list.
    pub indent: f32,
    pub table: Style,
    pub table_head: Style,
    pub table_cell: Style,
    /// The background of every other row.
    pub table_stripe: Color,
    pub rule: Style,
    pub image: Style,
    /// The box of a diagram.
    pub diagram: Style,
    pub footnotes: Style,
    /// How big an emoji that is a picture is, by the font size around it.
    pub emoji_size: f32,
}

const TEXT: Color = Color::from_rgb(0x1f2328);
const MUTED: Color = Color::from_rgb(0x59636e);
const BORDER: Color = Color::from_rgb(0xd1d9e0);
const BORDER_LIGHT: Color = Color::from_rgba(0xd1d9e0b3);
const MONO: &str =
    "ui-monospace, SFMono-Regular, \"SF Mono\", Menlo, Consolas, \"Liberation Mono\", monospace";

fn heading(size: f32, line: bool) -> Style {
    let s = Style::default()
        .text_size(size)
        .font_weight(FontWeight(600))
        .line_height(LineHeight::Relative(1.25))
        .mt(24.0)
        .mb(16.0);
    if line {
        s.pb(size * 0.3).border_b(1.0).border_color(BORDER_LIGHT)
    } else {
        s
    }
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        let size = 16.0;
        Self {
            root: Style::default()
                .text_size(size)
                .text_color(TEXT)
                .line_height(LineHeight::Relative(1.5)),
            paragraph: Style::default().mb(16.0),
            headings: [
                heading(size * 2.0, true),
                heading(size * 1.5, true),
                heading(size * 1.25, false),
                heading(size, false),
                heading(size * 0.875, false),
                heading(size * 0.85, false).text_color(MUTED),
            ],
            strong: Style::default().font_weight(FontWeight(600)),
            emphasis: Style::default().italic(),
            strikethrough: Style::default().strikethrough(),
            code: Style::default()
                .font_family(FontFamily::from(MONO))
                .text_size(size * 0.85)
                .bg(Color::from_rgba(0x818b981f))
                .rounded(6.0)
                .px(size * 0.85 * 0.4),
            link: Style::default().text_color(Color::from_rgb(0x0969da)),
            link_hover: Style::default().underline(),
            code_block: Style::default()
                .bg(Color::from_rgb(0xf6f8fa))
                .rounded(6.0)
                .p(16.0)
                .mb(16.0),
            code_text: Style::default()
                .font_family(FontFamily::from(MONO))
                .text_size(size * 0.85)
                .line_height(LineHeight::Relative(1.45)),
            quote: Style::default()
                .px(size)
                .border_l(size * 0.25)
                .border_color(BORDER)
                .text_color(MUTED)
                .mb(16.0),
            alerts: [
                Color::from_rgb(0x0969da),
                Color::from_rgb(0x1a7f37),
                Color::from_rgb(0x8250df),
                Color::from_rgb(0x9a6700),
                Color::from_rgb(0xcf222e),
            ],
            list: Style::default().mb(16.0),
            item_gap: size * 0.25,
            indent: size * 2.0,
            table: Style::default().mb(16.0),
            table_head: Style::default().font_weight(FontWeight(600)),
            table_cell: Style::default()
                .py(6.0)
                .px(13.0)
                .border(1.0)
                .border_color(BORDER),
            table_stripe: Color::from_rgb(0xf6f8fa),
            rule: Style::default().h(size * 0.25).bg(BORDER).mt(24.0).mb(24.0),
            image: Style::default(),
            diagram: Style::default().mb(16.0),
            footnotes: Style::default()
                .text_size(12.0)
                .text_color(MUTED)
                .border_t(1.0)
                .border_color(BORDER)
                .pt(16.0)
                .mt(16.0),
            emoji_size: 1.25,
        }
    }
}

impl MarkdownTheme {
    /// The font size everything starts from, in logical pixels.
    pub(crate) fn base_size(&self) -> f32 {
        self.root.font_size.unwrap_or(16.0)
    }
}
