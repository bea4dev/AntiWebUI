use crate::Color;
use std::sync::Arc;

/// A logical length.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// Logical pixels.
    Px(f32),
    /// Percentage (`0.0..=100.0`) of the containing block's corresponding axis.
    Percent(f32),
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
}
