//! Text shaping and layout on top of parley, following the pixel model:
//! fonts are shaped at their physical size, line boxes are whole pixels, glyph x positions keep
//! sub-pixel precision.

pub mod edit;
pub mod rich;
pub mod selection;

use crate::geometry::{RectI, Scale, SizeI};
use crate::tree::{Document, with_doc};
use anti_webui_style::{
    ComputedStyle, FontAxes, FontAxis, FontFamily, FontStyle, FontWeight, LineHeight, TextAlign,
    WhiteSpace,
};
use icu_properties::props::Script as IcuScript;
use icu_properties::{CodePointMapData, PropertyNamesShort};
use parley::fontique::{Blob, FallbackKey, FontInfoOverride, Script};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamilyName, FontVariation, FontVariations,
    Layout, LayoutContext, PositionedLayoutItem, StyleProperty, TextWrapMode,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

pub use parley::{Affinity, Cursor, Selection as LayoutSelection};

/// The brush stored in parley layouts. It is a span index, so color changes never require
/// reshaping: the actual color is looked up at paint time.
pub type Brush = u32;

/// Fonts of the same metrics, from what fontconfig has in `30-metric-aliases.conf`.
const STAND_INS: &[(&str, &str)] = &[
    ("arial", "\"Liberation Sans\",Arimo"),
    ("helvetica", "\"Liberation Sans\",Arimo"),
    ("times new roman", "\"Liberation Serif\",Tinos"),
    ("times", "\"Liberation Serif\",Tinos"),
    ("courier new", "\"Liberation Mono\",Cousine"),
    ("courier", "\"Liberation Mono\",Cousine"),
    ("calibri", "Carlito"),
    ("cambria", "Caladea"),
];

pub struct TextSystem {
    pub fcx: FontContext,
    pub lcx: LayoutContext<Brush>,
    pub locale: Option<parley::Language>,
    /// The scripts [`TextSystem::prepare_fallbacks`] has been through.
    prepared: Vec<IcuScript>,
    /// How many times fonts were added: what was shaped before may look different now.
    fonts_generation: u64,
    /// The `font-family` lists text has been shaped with.
    asked_lists: HashSet<FontFamily>,
    /// The names in those lists, in lower case, as fontique compares them.
    asked_names: HashSet<String>,
    with_stand_ins: HashMap<FontFamily, Arc<str>>,
    centrals: HashMap<(FontFamily, u32, FontWeight), (f32, f32)>,
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
            asked_lists: HashSet::new(),
            asked_names: HashSet::new(),
            with_stand_ins: HashMap::new(),
            centrals: HashMap::new(),
        }
    }

    /// A `font-family` list with fonts of the same metrics after the names that need them, like the
    /// aliases fontconfig gives Chromium. Otherwise "Arial" where it isn't installed goes on to what
    /// is next in the list, usually the default sans-serif, which can look quite different.
    fn list_with_stand_ins(&mut self, family: &FontFamily) -> Arc<str> {
        if let Some(list) = self.with_stand_ins.get(family) {
            return list.clone();
        }
        let mut out = String::new();
        for name in family.0.split(',') {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(name);
            let bare = name.trim().trim_matches(['"', '\'']).to_lowercase();
            if let Some((_, stand_ins)) = STAND_INS.iter().find(|(n, _)| *n == bare) {
                out.push(',');
                out.push_str(stand_ins);
            }
        }
        let list: Arc<str> = out.into();
        self.with_stand_ins.insert(family.clone(), list.clone());
        list
    }

    fn note_asked(&mut self, family: &FontFamily) {
        if self.asked_lists.contains(family) {
            return;
        }
        self.asked_lists.insert(family.clone());
        for name in FontFamilyName::parse_css_list(&family.0).map_while(Result::ok) {
            if let FontFamilyName::Named(name) = name {
                self.asked_names.insert(name.to_lowercase());
            }
        }
    }

    /// Whether a font of this family can't change what was shaped so far: no text asked for it,
    /// and it adds nothing to a family that text could have got to some other way.
    fn is_unseen_family(&mut self, name: &str) -> bool {
        !self.asked_names.contains(&name.to_lowercase())
            && self.fcx.collection.family_id(name).is_none()
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

    /// What the first font of the list that has the axis has it at by default.
    fn axis_default(&mut self, family: &FontFamily, axis: FontAxis) -> Option<f32> {
        for name in FontFamilyName::parse_css_list(&family.0).map_while(Result::ok) {
            let ids: Vec<_> = match name {
                FontFamilyName::Named(name) => {
                    self.fcx.collection.family_id(&name).into_iter().collect()
                }
                FontFamilyName::Generic(generic) => {
                    self.fcx.collection.generic_families(generic).collect()
                }
            };
            for id in ids {
                let Some(family) = self.fcx.collection.family(id) else {
                    continue;
                };
                let found = family
                    .fonts()
                    .iter()
                    .flat_map(|f| f.axes())
                    .find(|a| a.tag.to_be_bytes() == axis.0);
                if let Some(a) = found {
                    return Some(a.default);
                }
            }
        }
        None
    }

    /// The axes to shape with: what is set, and what follows from the rest of the style.
    fn variations(&mut self, style: &ShapeStyle) -> Vec<FontVariation> {
        const WEIGHT: FontAxis = FontAxis(*b"wght");
        const OPTICAL_SIZE: FontAxis = FontAxis(*b"opsz");
        let mut out = Vec::with_capacity(style.axes.iter().count() + 1);
        for v in style.axes.iter() {
            let value = if v.amount >= 1.0 {
                v.value
            } else {
                let unset = match v.axis {
                    WEIGHT => Some(style.weight.0 as f32),
                    OPTICAL_SIZE => Some(style.optical_size),
                    axis => self.axis_default(&style.family, axis),
                };
                unset.map_or(v.value, |u| u + (v.value - u) * v.amount)
            };
            out.push(FontVariation {
                tag: parley::setting::Tag::new(&v.axis.0),
                value,
            });
        }
        if style.axes.get(OPTICAL_SIZE).is_none() {
            out.push(FontVariation {
                tag: parley::setting::Tag::new(&OPTICAL_SIZE.0),
                value: style.optical_size,
            });
        }
        out
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
    let (names, unseen) = with_text(|ts| {
        let unseen = family.is_some_and(|name| ts.is_unseen_family(name));
        let info = family.map(|name| FontInfoOverride {
            family_name: Some(name),
            ..Default::default()
        });
        let found = ts
            .fcx
            .collection
            .register_fonts(Blob::new(Arc::new(data)), info);
        if !unseen {
            ts.fonts_generation += 1;
        }
        let mut names: Vec<String> = found
            .iter()
            .filter_map(|(id, _)| ts.fcx.collection.family_name(*id).map(str::to_owned))
            .collect();
        names.dedup();
        (names, unseen)
    });
    if names.is_empty() {
        return Err(NotAFont);
    }
    // What fell back because the font wasn't there can use it now.
    if !unseen {
        with_doc(Document::invalidate_all_layout);
    }
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
    pub axes: FontAxes,
    /// The logical font size, which `opsz` follows unless it is set.
    pub optical_size: f32,
    /// Whole physical pixels, or `None` for the font's own metrics.
    pub line_height: Option<i32>,
    pub letter_spacing: f32,
    pub wrap: bool,
}

/// How far below `dominant-baseline: central` and `middle` the baseline of a font is. It goes by the
/// first font of the list even if the text is in a fallback one, as browsers do, which keeps text
/// from moving by what script it is in.
pub(crate) fn baselines_of(ts: &mut TextSystem, style: &ShapeStyle) -> (f32, f32) {
    let key = (style.family.clone(), style.size.to_bits(), style.weight);
    if let Some(v) = ts.centrals.get(&key) {
        return *v;
    }
    let mut layout = TextLayout::default();
    layout.shape(ts, "x", 1, style);
    layout.break_lines(None);
    let mut out = (0.0, 0.0);
    if let Some(line) = layout.layout.lines().next() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(run) = item {
                let m = run.run().font_metrics();
                let x_height = m.x_height.unwrap_or(m.ascent * 0.5);
                out = ((m.ascent - m.descent) * 0.5, x_height * 0.5);
                break;
            }
        }
    }
    ts.centrals.insert(key, out);
    out
}

impl ShapeStyle {
    /// Makes it bigger, as what is around text in an `svg` does.
    pub(crate) fn zoomed(mut self, zoom: f32) -> Self {
        if zoom != 1.0 {
            self.size *= zoom;
            self.letter_spacing *= zoom;
            self.line_height = self
                .line_height
                .map(|h| (h as f32 * zoom).round().max(1.0) as i32);
        }
        self
    }

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
            axes: style.font_axes.clone(),
            optical_size: style.font_size,
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
    rich: Option<RichKey>,
}

/// What of a `rich_text` changes how it is shaped.
#[derive(Clone, Debug, PartialEq)]
struct RichKey {
    spans: Vec<(ShapeStyle, parley::VerticalAlign, (i32, i32))>,
    boxes: Vec<(SizeI, parley::VerticalAlign)>,
}

impl RichKey {
    fn of(rich: &rich::RichData, scale: Scale) -> Self {
        Self {
            spans: rich
                .resolved
                .iter()
                .map(|r| (r.shape.clone(), r.align, r.pad))
                .collect(),
            boxes: rich
                .boxes
                .iter()
                .map(|b| (b.size, b.align.parley(scale)))
                .collect(),
        }
    }
}

/// The id of the empty box that makes the padding on side `end` of span `i`. The boxes of
/// `inline` have the index of their `BoxData` as theirs.
const PAD: u64 = 1 << 63;

fn pad_id(i: usize, end: bool) -> u64 {
    PAD | ((i as u64) << 1) | end as u64
}

/// Which span and side an id of a box of padding is for.
pub(crate) fn pad_of(id: u64) -> Option<(usize, bool)> {
    (id & PAD != 0).then_some((((id & !PAD) >> 1) as usize, id & 1 == 1))
}

/// What a span changes that needs the text system to work out.
struct SpanShape {
    family: Option<Arc<str>>,
    variations: Option<Vec<FontVariation>>,
}

fn parley_style(style: FontStyle) -> parley::FontStyle {
    match style {
        FontStyle::Normal => parley::FontStyle::Normal,
        FontStyle::Italic => parley::FontStyle::Italic,
    }
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
        self.shape_rich(ts, text, version, style, None, Scale(120))
    }

    /// Like `shape`, with the spans and boxes of a `rich_text`, once they are resolved and
    /// measured.
    pub fn shape_rich(
        &mut self,
        ts: &mut TextSystem,
        text: &str,
        version: u64,
        style: &ShapeStyle,
        rich: Option<&rich::RichData>,
        scale: Scale,
    ) -> bool {
        let key = ShapeKey {
            version,
            fonts: ts.fonts_generation,
            style: style.clone(),
            rich: rich.map(|r| RichKey::of(r, scale)),
        };
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        ts.note_asked(&style.family);
        ts.prepare_fallbacks(text);
        let variations = ts.variations(style);
        let locale = ts.locale;
        let stand_ins = ts.list_with_stand_ins(&style.family);
        // What each span changes, worked out before the builder holds on to the contexts.
        let spans: Vec<(Range<usize>, SpanShape)> = rich
            .map(|r| {
                r.spans
                    .iter()
                    .zip(&r.resolved)
                    .filter(|(s, _)| !s.range.is_empty())
                    .map(|(s, resolved)| {
                        let shape = &resolved.shape;
                        if shape.family != style.family {
                            ts.note_asked(&shape.family);
                        }
                        (
                            s.range.clone(),
                            SpanShape {
                                family: (shape.family != style.family)
                                    .then(|| ts.list_with_stand_ins(&shape.family)),
                                variations: (shape.axes != style.axes
                                    || shape.weight != style.weight
                                    || shape.optical_size != style.optical_size)
                                    .then(|| ts.variations(shape)),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut builder = ts.lcx.ranged_builder(&mut ts.fcx, text, 1.0, true);
        builder.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
            stand_ins.as_ref().to_owned().into(),
        )));
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::FontWeight(parley::FontWeight::new(
            style.weight.0 as f32,
        )));
        builder.push_default(StyleProperty::FontStyle(parley_style(style.style)));
        builder.push_default(StyleProperty::FontVariations(FontVariations::List(
            variations.as_slice().into(),
        )));
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
        if let Some(rich) = rich {
            let mut index = 0;
            for (i, (s, resolved)) in rich.spans.iter().zip(&rich.resolved).enumerate() {
                if s.range.is_empty() {
                    continue;
                }
                let (range, changes) = &spans[index];
                index += 1;
                let shape = &resolved.shape;
                let range = range.clone();
                builder.push(StyleProperty::Brush(i as Brush + 1), range.clone());
                if let Some(family) = &changes.family {
                    builder.push(
                        StyleProperty::FontFamily(parley::FontFamily::Source(
                            family.as_ref().to_owned().into(),
                        )),
                        range.clone(),
                    );
                }
                if shape.size != style.size {
                    builder.push(StyleProperty::FontSize(shape.size), range.clone());
                }
                if shape.weight != style.weight {
                    builder.push(
                        StyleProperty::FontWeight(parley::FontWeight::new(shape.weight.0 as f32)),
                        range.clone(),
                    );
                }
                if shape.style != style.style {
                    builder.push(
                        StyleProperty::FontStyle(parley_style(shape.style)),
                        range.clone(),
                    );
                }
                if let Some(v) = &changes.variations {
                    builder.push(
                        StyleProperty::FontVariations(FontVariations::List(v.as_slice().into())),
                        range.clone(),
                    );
                }
                if shape.line_height != style.line_height {
                    builder.push(
                        StyleProperty::LineHeight(match shape.line_height {
                            Some(px) => parley::LineHeight::Absolute(px as f32),
                            None => parley::LineHeight::MetricsRelative(1.0),
                        }),
                        range.clone(),
                    );
                }
                if shape.letter_spacing != style.letter_spacing {
                    builder.push(
                        StyleProperty::LetterSpacing(shape.letter_spacing),
                        range.clone(),
                    );
                }
                if resolved.align != parley::VerticalAlign::BASELINE {
                    builder.push(StyleProperty::VerticalAlign(resolved.align), range.clone());
                }
            }
            // The boxes go in the order of the text, a span's padding around what is in it.
            let mut boxes: Vec<parley::InlineBox> = Vec::new();
            let mut put = |id: u64, index: usize, size: SizeI, align: parley::VerticalAlign| {
                boxes.push(parley::InlineBox {
                    id,
                    kind: parley::InlineBoxKind::InFlow,
                    index,
                    width: size.width as f32,
                    height: size.height as f32,
                    baseline: None,
                    vertical_align: align,
                });
            };
            for (i, b) in rich.boxes.iter().enumerate() {
                put(i as u64, b.at, b.size, b.align.parley(scale));
            }
            for (i, (s, r)) in rich.spans.iter().zip(&rich.resolved).enumerate() {
                if r.pad.0 > 0 {
                    put(
                        pad_id(i, false),
                        s.range.start,
                        SizeI::new(r.pad.0, 0),
                        parley::VerticalAlign::BASELINE,
                    );
                }
                if r.pad.1 > 0 {
                    put(
                        pad_id(i, true),
                        s.range.end,
                        SizeI::new(r.pad.1, 0),
                        parley::VerticalAlign::BASELINE,
                    );
                }
            }
            // Stable, so that what is at the same place stays in the order it was put.
            boxes.sort_by_key(|b| (b.index, pad_of(b.id).is_some_and(|(_, end)| !end)));
            for b in boxes {
                builder.push_inline_box(b);
            }
        }
        builder.build_into(&mut self.layout, text);
        self.key = Some(key);
        self.broken_at = None;
        self.aligned = None;
        self.final_break = None;
        true
    }

    /// The byte where the cluster at a point starts, if there is one right there.
    pub fn cluster_at(&self, x: f32, y: f32) -> Option<usize> {
        let (cluster, _) = parley::Cluster::from_point_exact(&self.layout, x, y)?;
        Some(cluster.text_range().start)
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

    /// How far down the first baseline is.
    pub fn first_baseline(&self) -> f32 {
        self.layout
            .lines()
            .next()
            .map_or(0.0, |line| line.metrics().baseline)
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
            axes: FontAxes::default(),
            optical_size: 14.0,
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
