//! The KaTeX fonts, in the program: each becomes a font only when a formula uses it.

use anti_webui_core::drawing::{FontData, static_font};
use ratex_font::FontId;
use read_fonts::{FontRef, TableProvider};
use std::sync::OnceLock;

macro_rules! fonts {
    ($($id:ident => $file:literal,)*) => {
        pub(crate) const FONTS: &[(FontId, &[u8])] = &[
            $((FontId::$id, include_bytes!(concat!("../fonts/KaTeX_", $file, ".ttf"))),)*
        ];
    };
}

fonts! {
    AmsRegular => "AMS-Regular",
    CaligraphicRegular => "Caligraphic-Regular",
    FrakturRegular => "Fraktur-Regular",
    FrakturBold => "Fraktur-Bold",
    MainBold => "Main-Bold",
    MainBoldItalic => "Main-BoldItalic",
    MainItalic => "Main-Italic",
    MainRegular => "Main-Regular",
    MathBoldItalic => "Math-BoldItalic",
    MathItalic => "Math-Italic",
    SansSerifBold => "SansSerif-Bold",
    SansSerifItalic => "SansSerif-Italic",
    SansSerifRegular => "SansSerif-Regular",
    ScriptRegular => "Script-Regular",
    Size1Regular => "Size1-Regular",
    Size2Regular => "Size2-Regular",
    Size3Regular => "Size3-Regular",
    Size4Regular => "Size4-Regular",
    TypewriterRegular => "Typewriter-Regular",
}

/// A KaTeX font, for drawing and for finding its glyphs.
pub(crate) struct Font {
    pub data: FontData,
    bytes: &'static [u8],
}

impl Font {
    /// The glyph of a character, if the font has one.
    pub fn glyph(&self, ch: u32) -> Option<u32> {
        let cmap = FontRef::new(self.bytes).ok()?.cmap().ok()?;
        cmap.map_codepoint(ch)
            .map(|g| g.to_u32())
            .filter(|&g| g != 0)
    }
}

/// The font, or `None` for the ones that are not KaTeX's (what RaTeX leaves to system fonts).
pub(crate) fn font(id: FontId) -> Option<&'static Font> {
    static MADE: [OnceLock<Font>; FONTS.len()] = [const { OnceLock::new() }; FONTS.len()];
    let i = FONTS.iter().position(|(f, _)| *f == id)?;
    Some(MADE[i].get_or_init(|| {
        let bytes = FONTS[i].1;
        Font {
            // One blob per font for the life of the program, so the renderer keeps one set of
            // glyphs for it.
            data: static_font(bytes, 0),
            bytes,
        }
    }))
}
