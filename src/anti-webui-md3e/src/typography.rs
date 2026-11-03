//! The type scale of Material 3, the part of it that the components use.

use anti_webui::prelude::*;

/// Roboto Flex comes with the crate; what it has no glyphs for falls back to the system's fonts.
pub const FONT_FAMILY: &str = "Roboto Flex, system-ui, sans-serif";

/// A text style of the scale: size, line height, letter spacing and weight in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub weight: u16,
}

impl TextStyle {
    pub const LABEL_LARGE: Self = Self::new(14.0, 20.0, 0.1, 500);
    pub const TITLE_MEDIUM: Self = Self::new(16.0, 24.0, 0.15, 500);
    pub const HEADLINE_SMALL: Self = Self::new(24.0, 32.0, 0.0, 400);
    pub const HEADLINE_LARGE: Self = Self::new(32.0, 40.0, 0.0, 400);

    const fn new(size: f32, line_height: f32, letter_spacing: f32, weight: u16) -> Self {
        Self {
            size,
            line_height,
            letter_spacing,
            weight,
        }
    }

    /// Applies the style to what can be styled.
    pub fn apply<S: Styled>(self, styled: S) -> S {
        styled
            .font_family(FONT_FAMILY)
            .text_size(self.size)
            .line_height(LineHeight::Px(self.line_height))
            .letter_spacing(self.letter_spacing)
            .font_weight(FontWeight(self.weight))
    }
}
