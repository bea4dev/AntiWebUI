//! The Outlined style of Material Symbols, with every icon in a font of its own, so that a program
//! only carries the icons it uses. See `docs/design/18-icons.md`.
//!
//! The names are those of [Google Fonts](https://fonts.google.com/icons) in capitals, with `_` in
//! front of the ones that start with a digit: `HOME`, `ARROW_BACK`, `_10K`.

/// One glyph of a font. The icons here are references to statics: a constant that held the font
/// itself would give the bytes one copy, and one address, for every place that uses it.
pub struct Symbol {
    /// The font file.
    pub font: &'static [u8],
    /// The family to register the font as.
    pub family: &'static str,
    /// The character of the glyph.
    pub ch: char,
}

#[rustfmt::skip]
mod generated;
pub use generated::*;
