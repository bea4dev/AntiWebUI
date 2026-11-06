use crate::Color;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A logical length.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// Logical pixels.
    Px(f32),
    /// Percentage (`0.0..=100.0`) of the containing block's corresponding axis.
    Percent(f32),
    /// Physical pixels, exactly: for what adds up many rounded lengths, like the rows of a long
    /// list, where logical pixels would drift.
    Device(i32),
    #[default]
    Auto,
}

impl Length {
    pub fn is_auto(&self) -> bool {
        matches!(self, Length::Auto)
    }
}

impl From<f32> for Length {
    fn from(v: f32) -> Self {
        Length::Px(v)
    }
}

impl From<i32> for Length {
    fn from(v: i32) -> Self {
        Length::Px(v as f32)
    }
}

pub fn px(v: f32) -> Length {
    Length::Px(v)
}

pub fn pct(v: f32) -> Length {
    Length::Percent(v)
}

pub const AUTO: Length = Length::Auto;

/// Values for the four sides of a box, in CSS order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    pub const fn all(v: T) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub fn map<U>(self, f: impl Fn(T) -> U) -> Edges<U> {
        Edges {
            top: f(self.top),
            right: f(self.right),
            bottom: f(self.bottom),
            left: f(self.left),
        }
    }
}

/// Values for the four corners of a box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corners<T> {
    pub top_left: T,
    pub top_right: T,
    pub bottom_right: T,
    pub bottom_left: T,
}

impl<T: Copy> Corners<T> {
    pub const fn all(v: T) -> Self {
        Self {
            top_left: v,
            top_right: v,
            bottom_right: v,
            bottom_left: v,
        }
    }

    pub fn map<U>(self, f: impl Fn(T) -> U) -> Corners<U> {
        Corners {
            top_left: f(self.top_left),
            top_right: f(self.top_right),
            bottom_right: f(self.bottom_right),
            bottom_left: f(self.bottom_left),
        }
    }
}

/// Pixels to show as they are: straight alpha, four bytes each, row by row.
#[derive(Debug, PartialEq, Eq)]
pub struct RgbaPixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Where a picture comes from. Two sources are the same picture when they name the same file or
/// share the same memory. See `docs/design/22-images.md`.
#[derive(Clone, Debug)]
pub enum ImageSource {
    Path(Arc<Path>),
    /// An encoded file in the program: `include_bytes!("logo.png")`.
    Static(&'static [u8]),
    /// An encoded file in memory.
    Bytes(Arc<[u8]>),
    Pixels(Arc<RgbaPixels>),
}

impl ImageSource {
    /// Pixels with straight alpha, four bytes each, row by row.
    ///
    /// # Panics
    ///
    /// If `data` doesn't hold exactly `width * height` pixels.
    pub fn rgba(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len() as u64,
            width as u64 * height as u64 * 4,
            "{width}x{height} pixels take four bytes each"
        );
        ImageSource::Pixels(Arc::new(RgbaPixels {
            width,
            height,
            data,
        }))
    }
}

impl PartialEq for ImageSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImageSource::Path(a), ImageSource::Path(b)) => a == b,
            (ImageSource::Static(a), ImageSource::Static(b)) => std::ptr::eq(*a, *b),
            (ImageSource::Bytes(a), ImageSource::Bytes(b)) => Arc::ptr_eq(a, b),
            (ImageSource::Pixels(a), ImageSource::Pixels(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for ImageSource {}

impl Hash for ImageSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ImageSource::Path(p) => p.hash(state),
            ImageSource::Static(b) => std::ptr::from_ref(*b).hash(state),
            ImageSource::Bytes(b) => Arc::as_ptr(b).hash(state),
            ImageSource::Pixels(p) => Arc::as_ptr(p).hash(state),
        }
    }
}

impl From<&str> for ImageSource {
    fn from(path: &str) -> Self {
        ImageSource::Path(Arc::from(Path::new(path)))
    }
}

impl From<String> for ImageSource {
    fn from(path: String) -> Self {
        ImageSource::from(path.as_str())
    }
}

impl From<&Path> for ImageSource {
    fn from(path: &Path) -> Self {
        ImageSource::Path(Arc::from(path))
    }
}

impl From<PathBuf> for ImageSource {
    fn from(path: PathBuf) -> Self {
        ImageSource::Path(Arc::from(path))
    }
}

impl From<&'static [u8]> for ImageSource {
    fn from(bytes: &'static [u8]) -> Self {
        ImageSource::Static(bytes)
    }
}

impl<const N: usize> From<&'static [u8; N]> for ImageSource {
    fn from(bytes: &'static [u8; N]) -> Self {
        ImageSource::Static(bytes)
    }
}

impl From<Vec<u8>> for ImageSource {
    fn from(bytes: Vec<u8>) -> Self {
        ImageSource::Bytes(Arc::from(bytes))
    }
}

impl From<Arc<[u8]>> for ImageSource {
    fn from(bytes: Arc<[u8]>) -> Self {
        ImageSource::Bytes(bytes)
    }
}

/// CSS `object-fit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ObjectFit {
    /// As big as the box, whatever that does to its proportions.
    #[default]
    Fill,
    /// All of it in the box, in proportion.
    Contain,
    /// All of the box covered, in proportion.
    Cover,
    /// As big as it is.
    None,
    /// `None` or `Contain`, whichever comes out smaller.
    ScaleDown,
}

/// CSS `image-rendering`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageRendering {
    #[default]
    Auto,
    /// Pixels stay squares when they get bigger.
    Pixelated,
}

/// CSS `background-size`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BackgroundSize {
    /// As big as the picture is.
    #[default]
    Auto,
    Cover,
    Contain,
    /// Width and height. One that is `auto` keeps the proportions, percentages are of the padding
    /// box.
    Size(Length, Length),
}

/// CSS `background-repeat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BackgroundRepeat {
    #[default]
    Repeat,
    RepeatX,
    RepeatY,
    NoRepeat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Display {
    /// Children are stacked vertically and stretched to the container width.
    #[default]
    Block,
    Flex,
    None,
    /// The node generates no box; its children participate in the parent's layout.
    Contents,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlexDirection {
    #[default]
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

impl FlexDirection {
    pub fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }

    pub fn is_reverse(self) -> bool {
        matches!(
            self,
            FlexDirection::RowReverse | FlexDirection::ColumnReverse
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    /// Lines go the other way along the cross axis.
    WrapReverse,
}

/// How the lines of a wrapping flex container share its cross axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlignContent {
    #[default]
    Stretch,
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

impl AlignContent {
    /// The same way of spacing things out, for what isn't stretched.
    pub fn as_justify(self) -> JustifyContent {
        match self {
            AlignContent::Stretch | AlignContent::Start => JustifyContent::Start,
            AlignContent::End => JustifyContent::End,
            AlignContent::Center => JustifyContent::Center,
            AlignContent::SpaceBetween => JustifyContent::SpaceBetween,
            AlignContent::SpaceAround => JustifyContent::SpaceAround,
            AlignContent::SpaceEvenly => JustifyContent::SpaceEvenly,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum JustifyContent {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlignItems {
    Start,
    End,
    Center,
    #[default]
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Position {
    #[default]
    Relative,
    /// Positioned against the parent's padding box using `inset`.
    Absolute,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
    Scroll,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UserSelect {
    /// Text can be selected.
    #[default]
    Text,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorIcon {
    #[default]
    Default,
    Pointer,
    Text,
    NotAllowed,
    Grab,
    Grabbing,
    Move,
    EwResize,
    NsResize,
    Crosshair,
    /// No cursor at all.
    None,
    /// What is dragged is going to be copied.
    Copy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Start,
    End,
    Left,
    Center,
    Right,
}

/// Whether the contents are laid out and painted (CSS `content-visibility`). See
/// `docs/design/29-content-visibility.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContentVisibility {
    #[default]
    Visible,
    /// Only while it is near the screen, focused, selected or scrolled to.
    Auto,
    /// Never.
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhiteSpace {
    /// Preserve whitespace and wrap at the box edge.
    #[default]
    PreWrap,
    /// Preserve whitespace and never wrap (except at hard breaks).
    Pre,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

/// CSS-like font weight (100..=900).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const THIN: FontWeight = FontWeight(100);
    pub const LIGHT: FontWeight = FontWeight(300);
    pub const NORMAL: FontWeight = FontWeight(400);
    pub const MEDIUM: FontWeight = FontWeight(500);
    pub const SEMIBOLD: FontWeight = FontWeight(600);
    pub const BOLD: FontWeight = FontWeight(700);
    pub const BLACK: FontWeight = FontWeight(900);
}

/// The tag of an axis of a variable font, like `b"wght"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontAxis(pub [u8; 4]);

impl From<&[u8; 4]> for FontAxis {
    fn from(tag: &[u8; 4]) -> Self {
        Self(*tag)
    }
}

impl std::fmt::Debug for FontAxis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

/// What an axis is set to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontAxisValue {
    pub axis: FontAxis,
    /// In the units of the font.
    pub value: f32,
    /// How much `value` counts against what the axis would be without it: `1.0` unless it is on
    /// its way to or from not being set.
    pub amount: f32,
}

/// The axes that are set, each one on its own: setting one leaves the others alone, in a parent as
/// in a state (see `docs/design/17-font-axes.md`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontAxes(Vec<FontAxisValue>);

impl FontAxes {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FontAxisValue> {
        self.0.iter()
    }

    pub fn get(&self, axis: FontAxis) -> Option<&FontAxisValue> {
        self.position(axis).ok().map(|i| &self.0[i])
    }

    fn position(&self, axis: FontAxis) -> Result<usize, usize> {
        self.0.binary_search_by_key(&axis, |v| v.axis)
    }

    pub fn set(&mut self, axis: FontAxis, value: f32) {
        self.put(FontAxisValue {
            axis,
            value,
            amount: 1.0,
        });
    }

    fn put(&mut self, value: FontAxisValue) {
        match self.position(value.axis) {
            Ok(i) => self.0[i] = value,
            Err(i) => self.0.insert(i, value),
        }
    }

    /// Takes the axes that `other` sets.
    pub fn merge(&mut self, other: &FontAxes) {
        for v in &other.0 {
            self.put(*v);
        }
    }

    /// The axes at `t` of the way to `to`. An axis that only one side has goes from or to what it
    /// would be without it, like a color with alpha: the value and the amount go on their own.
    pub fn interpolate(&self, to: &FontAxes, t: f32) -> FontAxes {
        let values = self.union(to).into_iter().map(|axis| {
            let (a, b) = (self.weighted(axis), to.weighted(axis));
            let total = a.0 + (b.0 - a.0) * t;
            let amount = a.1 + (b.1 - a.1) * t;
            let value = if amount > 0.0 {
                total / amount
            } else {
                self.get(axis).or(to.get(axis)).map_or(0.0, |v| v.value)
            };
            FontAxisValue {
                axis,
                value,
                amount,
            }
        });
        FontAxes(values.collect())
    }

    /// How far apart they are, for telling how far along a transition is.
    pub fn distance(&self, other: &FontAxes) -> f32 {
        self.union(other)
            .into_iter()
            .map(|axis| {
                let (a, b) = (self.weighted(axis), other.weighted(axis));
                (a.0 - b.0).abs() + (a.1 - b.1).abs()
            })
            .sum()
    }

    fn union(&self, other: &FontAxes) -> Vec<FontAxis> {
        let mut axes: Vec<_> = self.0.iter().chain(&other.0).map(|v| v.axis).collect();
        axes.sort_unstable();
        axes.dedup();
        axes
    }

    /// The value times the amount, and the amount.
    fn weighted(&self, axis: FontAxis) -> (f32, f32) {
        self.get(axis)
            .map_or((0.0, 0.0), |v| (v.value * v.amount, v.amount))
    }
}

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// A CSS `font-family` list, e.g. `"Inter, Noto Sans CJK JP, sans-serif"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFamily(pub Arc<str>);

impl Default for FontFamily {
    fn default() -> Self {
        FontFamily("system-ui, sans-serif".into())
    }
}

impl From<&str> for FontFamily {
    fn from(s: &str) -> Self {
        FontFamily(s.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    /// The font's own metrics (`line-height: normal`).
    Normal,
    /// Multiple of the font size.
    Relative(f32),
    /// Logical pixels.
    Px(f32),
}

impl Default for LineHeight {
    fn default() -> Self {
        LineHeight::Relative(1.4)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    /// Drawn inside the padding box, above the background and below the children.
    pub inset: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ComputedStyle, Style, Styled};

    const GRAD: FontAxis = FontAxis(*b"GRAD");
    const WDTH: FontAxis = FontAxis(*b"wdth");

    fn axes(list: &[(FontAxis, f32)]) -> FontAxes {
        let mut out = FontAxes::default();
        for (axis, value) in list {
            out.set(*axis, *value);
        }
        out
    }

    /// What the axis comes to when it is `unset` without the value.
    fn effective(axes: &FontAxes, axis: FontAxis, unset: f32) -> f32 {
        axes.get(axis)
            .map_or(unset, |v| unset + (v.value - unset) * v.amount)
    }

    #[test]
    fn an_axis_set_again_takes_the_new_value() {
        let a = axes(&[(WDTH, 1.0), (GRAD, 2.0), (WDTH, 3.0)]);
        assert_eq!(a.iter().count(), 2);
        assert_eq!(a.get(WDTH).unwrap().value, 3.0);
        assert_eq!(a.get(GRAD).unwrap().value, 2.0);
    }

    #[test]
    fn merging_keeps_what_the_other_does_not_set() {
        let mut a = axes(&[(WDTH, 1.0), (GRAD, 2.0)]);
        a.merge(&axes(&[(GRAD, 3.0)]));
        assert_eq!(a, axes(&[(WDTH, 1.0), (GRAD, 3.0)]));
    }

    #[test]
    fn styles_add_their_axes_up() {
        let base = Style::default()
            .font_axis(b"wdth", 1.0)
            .font_axis(b"GRAD", 2.0);
        let state = Style::default().font_axis(b"GRAD", 3.0);

        let mut refined = base.clone();
        refined.refine(&state);
        assert_eq!(refined.font_axes, Some(axes(&[(WDTH, 1.0), (GRAD, 3.0)])));

        let mut computed = ComputedStyle::default();
        base.apply_to(&mut computed);
        state.apply_to(&mut computed);
        assert_eq!(computed.font_axes, axes(&[(WDTH, 1.0), (GRAD, 3.0)]));

        let child = ComputedStyle::inherit_from(&computed);
        assert_eq!(child.font_axes, computed.font_axes);
    }

    #[test]
    fn a_style_with_no_axes_changes_none() {
        let mut computed = ComputedStyle::default();
        Style::default()
            .font_axis(b"GRAD", 3.0)
            .apply_to(&mut computed);
        Style::default().apply_to(&mut computed);
        assert_eq!(computed.font_axes, axes(&[(GRAD, 3.0)]));
    }

    #[test]
    fn an_axis_on_both_sides_goes_straight() {
        let m = axes(&[(GRAD, 10.0)]).interpolate(&axes(&[(GRAD, 30.0)]), 0.25);
        assert_eq!(m, axes(&[(GRAD, 15.0)]));
    }

    #[test]
    fn an_axis_on_one_side_goes_from_and_to_not_being_set() {
        let (none, set) = (FontAxes::default(), axes(&[(GRAD, 100.0)]));
        for unset in [0.0, 40.0, -30.0] {
            for (t, from_none) in [(0.0, unset), (0.5, unset + (100.0 - unset) * 0.5)] {
                let m = none.interpolate(&set, t);
                assert_eq!(effective(&m, GRAD, unset), from_none);
            }
            let m = set.interpolate(&none, 0.75);
            assert_eq!(effective(&m, GRAD, unset), 100.0 + (unset - 100.0) * 0.75);
        }
        assert_eq!(none.interpolate(&set, 1.0), set);
    }

    /// Whatever the axis turns out to be without the value.
    #[test]
    fn going_on_from_part_of_the_way_is_a_straight_line_too() {
        let (none, to) = (FontAxes::default(), axes(&[(GRAD, 200.0)]));
        let partway = none.interpolate(&axes(&[(GRAD, 100.0)]), 0.5);
        for unset in [0.0, 40.0] {
            let from = effective(&partway, GRAD, unset);
            let m = partway.interpolate(&to, 0.5);
            assert!((effective(&m, GRAD, unset) - (from + 200.0) * 0.5).abs() < 1e-3);
        }
        let back = partway.interpolate(&none, 0.5);
        for unset in [0.0, 40.0] {
            let from = effective(&partway, GRAD, unset);
            assert!((effective(&back, GRAD, unset) - (from + unset) * 0.5).abs() < 1e-3);
        }
    }

    #[test]
    fn the_distance_goes_down_along_the_way() {
        let (a, b) = (axes(&[(WDTH, 10.0)]), axes(&[(GRAD, 100.0)]));
        let whole = a.distance(&b);
        assert!(whole > 0.0);
        let half = a.interpolate(&b, 0.5).distance(&b);
        assert!((half - whole * 0.5).abs() < 1e-3, "{half} {whole}");
        assert_eq!(b.distance(&b), 0.0);
        assert!(axes(&[(GRAD, 0.0)]).distance(&FontAxes::default()) > 0.0);
    }

    #[test]
    fn a_tag_comes_from_four_bytes() {
        assert_eq!(FontAxis::from(b"GRAD"), GRAD);
        assert_eq!(format!("{GRAD:?}"), "GRAD");
    }
}

/// What the inside or the outline of a shape in an `svg` is painted with.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SvgPaint {
    #[default]
    None,
    Color(Color),
    /// The color text has here.
    CurrentColor,
    Gradient(std::sync::Arc<Gradient>),
}

impl From<Color> for SvgPaint {
    fn from(color: Color) -> Self {
        SvgPaint::Color(color)
    }
}

impl From<Gradient> for SvgPaint {
    fn from(gradient: Gradient) -> Self {
        SvgPaint::Gradient(std::sync::Arc::new(gradient))
    }
}

/// Which side of an outline is inside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// From 0 to 1.
    pub offset: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientShape {
    Linear {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    /// The circle of the last stop, and where the first stop is.
    Radial {
        cx: f32,
        cy: f32,
        r: f32,
        fx: f32,
        fy: f32,
    },
}

/// What is beyond the first and the last stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GradientSpread {
    #[default]
    Pad,
    Reflect,
    Repeat,
}

/// What the numbers of a gradient are relative to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GradientUnits {
    /// The box of the shape, from 0 to 1.
    #[default]
    ObjectBoundingBox,
    /// The coordinates the shape is in.
    UserSpace,
}

/// An SVG gradient.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub shape: GradientShape,
    pub stops: Vec<GradientStop>,
    pub spread: GradientSpread,
    pub units: GradientUnits,
    /// `gradientTransform`: `a b c d e f`.
    pub transform: [f64; 6],
}

impl Gradient {
    fn new(shape: GradientShape) -> Self {
        Self {
            shape,
            stops: Vec::new(),
            spread: GradientSpread::Pad,
            units: GradientUnits::ObjectBoundingBox,
            transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }

    pub fn linear(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self::new(GradientShape::Linear { x1, y1, x2, y2 })
    }

    pub fn radial(cx: f32, cy: f32, r: f32) -> Self {
        Self::new(GradientShape::Radial {
            cx,
            cy,
            r,
            fx: cx,
            fy: cy,
        })
    }

    pub fn stop(mut self, offset: f32, color: impl Into<Color>) -> Self {
        self.stops.push(GradientStop {
            offset,
            color: color.into(),
        });
        self
    }

    pub fn spread(mut self, spread: GradientSpread) -> Self {
        self.spread = spread;
        self
    }

    pub fn units(mut self, units: GradientUnits) -> Self {
        self.units = units;
        self
    }

    pub fn transform(mut self, transform: [f64; 6]) -> Self {
        self.transform = transform;
        self
    }
}
