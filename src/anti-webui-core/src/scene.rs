//! The renderer-independent display list produced each frame. Coordinates are physical pixels.

use crate::geometry::RectI;
use anti_webui_style::Color;
pub use parley::FontData;

/// A clip region: a rectangle, optionally with rounded corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub rect: RectI,
    /// Top-left, top-right, bottom-right, bottom-left, in physical pixels.
    pub radii: [f32; 4],
}

impl Clip {
    pub const NONE: Clip = Clip {
        rect: RectI::INFINITE,
        radii: [0.0; 4],
    };

    pub fn is_rounded(&self) -> bool {
        self.radii.iter().any(|r| *r > 0.0)
    }
}

impl From<RectI> for Clip {
    fn from(rect: RectI) -> Self {
        Clip {
            rect,
            radii: [0.0; 4],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Quad {
    pub rect: RectI,
    pub background: Color,
    /// Top, right, bottom, left.
    pub border_widths: [i32; 4],
    pub border_color: Color,
    /// Top-left, top-right, bottom-right, bottom-left, in physical pixels.
    pub radii: [f32; 4],
    pub clip: Clip,
}

#[derive(Clone, Debug)]
pub struct Shadow {
    pub rect: RectI,
    pub radii: [f32; 4],
    pub blur: f32,
    pub color: Color,
    pub clip: Clip,
}

#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub struct GlyphRun {
    pub font: FontData,
    /// Physical font size.
    pub size: f32,
    pub normalized_coords: Vec<i16>,
    pub embolden: bool,
    /// Synthetic italic skew in degrees.
    pub skew: Option<f32>,
    pub color: Color,
    pub glyphs: Vec<Glyph>,
    pub clip: Clip,
}

#[derive(Clone, Debug)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Glyphs(GlyphRun),
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub clear: Color,
    pub primitives: Vec<Primitive>,
}

impl Scene {
    pub fn fill(&mut self, rect: RectI, color: Color, clip: Clip) {
        if color.is_transparent() || rect.is_empty() {
            return;
        }
        self.primitives.push(Primitive::Quad(Quad {
            rect,
            background: color,
            border_widths: [0; 4],
            border_color: Color::TRANSPARENT,
            radii: [0.0; 4],
            clip,
        }));
    }
}
