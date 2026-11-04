use crate::Color;
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
