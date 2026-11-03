//! Text shaping and layout on top of parley, following the pixel model:
//! fonts are shaped at their physical size, line boxes are whole pixels, glyph x positions keep
//! sub-pixel precision.

pub mod edit;
pub mod selection;

use crate::geometry::{RectI, Scale, SizeI};
use crate::tree::{Document, with_doc};
use anti_webui_style::{
    ComputedStyle, FontFamily, FontStyle, FontWeight, LineHeight, TextAlign, WhiteSpace,
};
use icu_properties::props::Script as IcuScript;
use icu_properties::{CodePointMapData, PropertyNamesShort};
use parley::fontique::{Blob, FallbackKey, FontInfoOverride, Script};
use parley::{
    Alignment, AlignmentOptions, FontContext, Layout, LayoutContext, StyleProperty, TextWrapMode,
};
use std::cell::RefCell;
use std::sync::Arc;

pub use parley::{Affinity, Cursor, Selection as LayoutSelection};

/// The brush stored in parley layouts. It is a span index, so color changes never require
/// reshaping: the actual color is looked up at paint time.
pub type Brush = u32;

pub struct TextSystem {
    pub fcx: FontContext,
    pub lcx: LayoutContext<Brush>,
    pub locale: Option<parley::Language>,
    /// The scripts [`TextSystem::prepare_fallbacks`] has been through.
    prepared: Vec<IcuScript>,
    /// How many times fonts were added: what was shaped before may look different now.
    fonts_generation: u64,
}

impl TextSystem {
    fn new() -> Self {
        let locale = std::env::var("LC_ALL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("LC_CTYPE").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("LANG").ok())
            .and_then(|l| {
                let tag = l.split(['.', '@']).next()?.replace('_', "-");
                parley::Language::parse(&tag).ok()
            });
        Self {
            fcx: FontContext::new(),
            lcx: LayoutContext::new(),
            locale,
            prepared: Vec::new(),
            fonts_generation: 0,
        }
    }

    /// fontique doesn't keep what fontconfig says for a script and a locale it doesn't track, such
    /// as Latin in any locale, and would ask again for every text (see `docs/design/04-text.md`).
    /// For those it uses what it has for the script alone, which is the same question: ask it once.
    fn prepare_fallbacks(&mut self, text: &str) {
        // What has no script of its own counts as Latin.
        self.prepare_fallback(IcuScript::Latin);
        if text.is_ascii() {
            return;
        }
        let scripts = CodePointMapData::<IcuScript>::new();
        let mut last = IcuScript::Latin;
        for c in text.chars() {
            let script = scripts.get(c);
            if script != last {
                last = script;
                self.prepare_fallback(script);
            }
        }
    }

    fn prepare_fallback(&mut self, script: IcuScript) {
        if self.prepared.contains(&script) {
            return;
        }
        self.prepared.push(script);
        let Some(script) = PropertyNamesShort::<IcuScript>::new()
            .get(script)
            .and_then(|name| Script::parse(name).ok())
        else {
            return;
        };
        if !FallbackKey::new(script, self.locale.as_ref()).is_tracked() {
            let _ = self.fcx.collection.fallback_families(script);
        }
    }
}

thread_local! {
    static TEXT: RefCell<TextSystem> = RefCell::new(TextSystem::new());
}

/// The data given to [`register_font`] holds no font that can be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotAFont;

impl std::fmt::Display for NotAFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the data holds no font that can be used")
    }
}

impl std::error::Error for NotAFont {}

/// Makes the fonts in a font file that comes with the application available by their names, like
/// `@font-face` in CSS. Returns the names of the families. Text already shown is shaped again.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// # let font_file: &'static [u8] = &[];
/// register_font(font_file).unwrap();
/// let view = text("Hello").font_family("Example");
/// ```
pub fn register_font(
    data: impl AsRef<[u8]> + Send + Sync + 'static,
) -> Result<Vec<String>, NotAFont> {
    register(data, None)
}

/// Like [`register_font`], under a name of your choosing.
pub fn register_font_as(
    data: impl AsRef<[u8]> + Send + Sync + 'static,
    family: &str,
) -> Result<Vec<String>, NotAFont> {
    register(data, Some(family))
}

fn register(
    data: impl AsRef<[u8]> + Send + Sync + 'static,
    family: Option<&str>,
) -> Result<Vec<String>, NotAFont> {
    let names = with_text(|ts| {
        let info = family.map(|name| FontInfoOverride {
            family_name: Some(name),
            ..Default::default()
        });
        let found = ts
            .fcx
            .collection
            .register_fonts(Blob::new(Arc::new(data)), info);
        ts.fonts_generation += 1;
        let mut names: Vec<String> = found
            .iter()
            .filter_map(|(id, _)| ts.fcx.collection.family_name(*id).map(str::to_owned))
            .collect();
        names.dedup();
        names
    });
    if names.is_empty() {
        return Err(NotAFont);
    }
    // What fell back because the font wasn't there can use it now.
    with_doc(Document::invalidate_all_layout);
    Ok(names)
}

pub fn with_text<R>(f: impl FnOnce(&mut TextSystem) -> R) -> R {
    TEXT.with(|t| f(&mut t.borrow_mut()))
}

/// Every property that affects shaping or line breaking, already in physical units.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeStyle {
    pub family: FontFamily,
    pub size: f32,
    pub weight: FontWeight,
    pub style: FontStyle,
    /// Whole physical pixels, or `None` for the font's own metrics.
    pub line_height: Option<i32>,
    pub letter_spacing: f32,
    pub wrap: bool,
}

impl ShapeStyle {
    pub fn from_computed(style: &ComputedStyle, scale: Scale) -> Self {
        let size = scale.pxf(style.font_size);
        let line_height = match style.line_height {
            LineHeight::Normal => None,
            LineHeight::Relative(r) => Some(scale.px(r * style.font_size).max(1)),
            LineHeight::Px(v) => Some(scale.px(v).max(1)),
        };
        Self {
            family: style.font_family.clone(),
            size,
            weight: style.font_weight,
            style: style.font_style,
            line_height,
            letter_spacing: scale.pxf(style.letter_spacing),
            wrap: style.white_space == WhiteSpace::PreWrap,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ShapeKey {
    version: u64,
    fonts: u64,
    style: ShapeStyle,
}

/// A shaped paragraph that can be re-broken at different widths cheaply.
#[derive(Default)]
pub struct TextLayout {
    pub layout: Layout<Brush>,
    key: Option<ShapeKey>,
    broken_at: Option<Option<i32>>,
    aligned: Option<(i32, TextAlign)>,
    /// The width and alignment chosen by the last layout pass (what gets painted).
    final_break: Option<(i32, TextAlign)>,
}

impl TextLayout {
    /// Reshapes only if the text version or the style changed. Returns whether it reshaped.
    pub fn shape(
        &mut self,
        ts: &mut TextSystem,
        text: &str,
        version: u64,
        style: &ShapeStyle,
    ) -> bool {
        let key = ShapeKey {
            version,
            fonts: ts.fonts_generation,
            style: style.clone(),
        };
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        ts.prepare_fallbacks(text);
        let locale = ts.locale;
        let mut builder = ts.lcx.ranged_builder(&mut ts.fcx, text, 1.0, true);
        builder.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
            style.family.0.as_ref().to_owned().into(),
        )));
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::FontWeight(parley::FontWeight::new(
            style.weight.0 as f32,
        )));
        builder.push_default(StyleProperty::FontStyle(match style.style {
            FontStyle::Normal => parley::FontStyle::Normal,
            FontStyle::Italic => parley::FontStyle::Italic,
        }));
        builder.push_default(StyleProperty::LineHeight(match style.line_height {
            Some(px) => parley::LineHeight::Absolute(px as f32),
            None => parley::LineHeight::MetricsRelative(1.0),
        }));
        if style.letter_spacing != 0.0 {
            builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
        }
        builder.push_default(StyleProperty::Locale(locale));
        builder.push_default(StyleProperty::Brush(0));
        if !style.wrap {
            builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
        }
        builder.build_into(&mut self.layout, text);
        self.key = Some(key);
        self.broken_at = None;
        self.aligned = None;
        self.final_break = None;
        true
    }

    /// Breaks lines at `max_width` physical pixels (`None` = max-content).
    pub fn break_lines(&mut self, max_width: Option<i32>) {
        if self.broken_at == Some(max_width) {
            return;
        }
        self.layout
            .break_all_lines(max_width.map(|w| w.max(0) as f32));
        self.broken_at = Some(max_width);
        self.aligned = None;
    }

    pub fn align(&mut self, width: i32, align: TextAlign) {
        if self.aligned == Some((width, align)) {
            return;
        }
        let alignment = match align {
            TextAlign::Start => Alignment::Start,
            TextAlign::End => Alignment::End,
            TextAlign::Left => Alignment::Left,
            TextAlign::Center => Alignment::Center,
            TextAlign::Right => Alignment::Right,
        };
        // Alignment uses the width given to `break_all_lines`; re-break at the final width so
        // centered text centers in the box rather than in its own content width.
        if self.broken_at != Some(Some(width)) && alignment != Alignment::Start {
            self.break_lines(Some(width));
        }
        self.layout.align(alignment, AlignmentOptions::default());
        self.aligned = Some((width, align));
    }

    /// Breaks and aligns for painting, and remembers it for [`TextLayout::restore_final`].
    pub fn set_final(&mut self, width: i32, align: TextAlign) {
        self.break_lines(Some(width));
        self.align(width, align);
        self.final_break = Some((width, align));
    }

    /// Re-applies the painted line breaks after the layout was measured at another width.
    pub fn restore_final(&mut self) {
        if let Some((w, a)) = self.final_break {
            self.break_lines(Some(w));
            self.align(w, a);
        }
    }

    /// The size of the broken layout, rounded up to whole pixels.
    pub fn size(&self) -> SizeI {
        SizeI::new(
            self.layout.width().ceil() as i32,
            self.layout.height().ceil() as i32,
        )
    }

    pub fn min_content_width(&self) -> i32 {
        self.layout.calculate_content_widths().min.ceil() as i32
    }

    pub fn max_content_width(&self) -> i32 {
        self.layout.calculate_content_widths().max.ceil() as i32
    }

    /// Height of an empty line in this style (for empty text and carets).
    pub fn first_line_box(&self) -> Option<(i32, i32)> {
        let line = self.layout.lines().next()?;
        let m = line.metrics();
        Some((
            m.block_min_coord.round() as i32,
            m.block_max_coord.round() as i32,
        ))
    }

    /// Visual rectangles covering the byte range, relative to the layout origin.
    /// Vertical extents are whole line boxes so adjacent lines touch exactly.
    pub fn range_rects(
        &self,
        start: usize,
        end: usize,
        origin_x: i32,
        origin_y: i32,
    ) -> Vec<RectI> {
        if start >= end {
            return Vec::new();
        }
        let len = self.layout.text_len();
        let a = Cursor::from_byte_index(&self.layout, start.min(len), Affinity::Downstream);
        let b = Cursor::from_byte_index(&self.layout, end.min(len), Affinity::Upstream);
        let sel = LayoutSelection::new(a, b);
        let mut rects = Vec::new();
        sel.geometry_with(&self.layout, |bb, _line| {
            let x0 = bb.x0.floor() as i32;
            let x1 = (bb.x1.ceil() as i32).max(x0 + 1);
            let y0 = bb.y0.round() as i32;
            let y1 = bb.y1.round() as i32;
            rects.push(RectI::new(origin_x + x0, origin_y + y0, x1 - x0, y1 - y0));
        });
        rects
    }

    /// Caret rectangle for a byte offset, relative to the layout origin.
    pub fn caret_rect(&self, index: usize, affinity: Affinity, width: i32) -> RectI {
        let len = self.layout.text_len();
        let cursor = Cursor::from_byte_index(&self.layout, index.min(len), affinity);
        let bb = cursor.geometry(&self.layout, width as f32);
        let x = bb.x0.round() as i32;
        let y0 = bb.y0.round() as i32;
        let y1 = bb.y1.round() as i32;
        RectI::new(x, y0, width, (y1 - y0).max(1))
    }

    /// The byte offset closest to a point relative to the layout origin.
    pub fn hit(&self, x: f32, y: f32) -> (usize, Affinity) {
        let c = Cursor::from_point(&self.layout, x, y);
        (c.index(), c.affinity())
    }

    pub fn word_at(&self, x: f32, y: f32) -> (usize, usize) {
        let s = LayoutSelection::word_from_point(&self.layout, x, y);
        let r = s.text_range();
        (r.start, r.end)
    }

    pub fn hard_line_at(&self, x: f32, y: f32) -> (usize, usize) {
        let s = LayoutSelection::hard_line_from_point(&self.layout, x, y);
        let r = s.text_range();
        (r.start, r.end)
    }

    /// Index of the line containing `y` (clamped), with its box.
    pub fn line_box_at(&self, y: f32) -> Option<(i32, i32)> {
        let mut last = None;
        for line in self.layout.lines() {
            let m = line.metrics();
            let (y0, y1) = (m.block_min_coord, m.block_max_coord);
            last = Some((y0.round() as i32, y1.round() as i32));
            if y < y1 {
                return last;
            }
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn japanese() -> TextSystem {
        let mut ts = TextSystem::new();
        ts.locale = parley::Language::parse("ja-JP").ok();
        ts
    }

    #[test]
    fn each_script_of_a_text_is_prepared_once() {
        let mut ts = japanese();
        ts.prepare_fallbacks("abc 123");
        assert_eq!(ts.prepared, [IcuScript::Latin]);
        ts.prepare_fallbacks("abc 123");
        assert_eq!(ts.prepared.len(), 1);
        ts.prepare_fallbacks("ひらがな、カタカナ abc ไทย");
        for script in [IcuScript::Hiragana, IcuScript::Katakana, IcuScript::Thai] {
            assert!(ts.prepared.contains(&script), "{script:?}");
        }
        let count = ts.prepared.len();
        ts.prepare_fallbacks("カタカナ ไทย ひらがな");
        assert_eq!(ts.prepared.len(), count);
    }

    #[test]
    fn shaping_prepares_what_it_needs() {
        let mut ts = japanese();
        let style = ShapeStyle {
            family: FontFamily::default(),
            size: 14.0,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            line_height: None,
            letter_spacing: 0.0,
            wrap: true,
        };
        TextLayout::default().shape(&mut ts, "ไทย", 1, &style);
        assert!(ts.prepared.contains(&IcuScript::Thai));
    }

    /// What preparing relies on: otherwise every text goes all the way to fontconfig again.
    #[test]
    fn a_pair_that_is_not_tracked_takes_what_there_is_for_the_script() {
        let mut ts = japanese();
        let latin = Script::from_bytes(*b"Latn");
        let key = FallbackKey::new(latin, ts.locale.as_ref());
        assert!(!key.is_tracked());
        ts.fcx.collection.set_fallbacks(latin, std::iter::empty());
        assert_eq!(ts.fcx.collection.fallback_families(key).count(), 0);
    }

    /// One tracked for its locale has an answer of its own, which preparing must not replace.
    #[test]
    fn a_tracked_pair_is_left_alone() {
        let mut ts = japanese();
        let han = Script::from_bytes(*b"Hani");
        assert!(FallbackKey::new(han, ts.locale.as_ref()).is_tracked());
        let own: Vec<_> = ts
            .fcx
            .collection
            .fallback_families(FallbackKey::new(han, ts.locale.as_ref()))
            .collect();
        ts.prepare_fallbacks("漢字");
        let after: Vec<_> = ts
            .fcx
            .collection
            .fallback_families(FallbackKey::new(han, ts.locale.as_ref()))
            .collect();
        assert_eq!(own, after);
    }
}
