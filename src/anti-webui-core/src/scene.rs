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

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub rect: RectI,
    pub radii: [f32; 4],
    pub blur: f32,
    pub color: Color,
    pub clip: Clip,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Glyphs(GlyphRun),
}

/// Identifies a run of primitives across frames: one part of what a node paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub node: u64,
    pub part: u32,
}

/// A run of consecutive primitives painted for one [`ChunkKey`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chunk {
    pub key: ChunkKey,
    /// Index of the first primitive; the chunk ends where the next one starts.
    pub start: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub clear: Color,
    pub primitives: Vec<Primitive>,
    /// Groups the primitives for damage tracking; primitives before the first chunk belong to
    /// none and are compared by position.
    pub chunks: Vec<Chunk>,
}

impl Scene {
    /// Starts a new chunk; following primitives belong to it.
    pub fn begin_chunk(&mut self, key: ChunkKey) {
        let start = self.primitives.len();
        match self.chunks.last_mut() {
            // An empty chunk is replaced rather than kept.
            Some(last) if last.start == start => last.key = key,
            _ => self.chunks.push(Chunk { key, start }),
        }
    }

    /// The primitive range of chunk `i`.
    pub fn chunk_range(&self, i: usize) -> std::ops::Range<usize> {
        let end = self
            .chunks
            .get(i + 1)
            .map_or(self.primitives.len(), |c| c.start);
        self.chunks[i].start..end
    }

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
