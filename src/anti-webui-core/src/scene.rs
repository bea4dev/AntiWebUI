//! The renderer-independent display list produced each frame. Coordinates are physical pixels.

use crate::geometry::{PointI, RectI};
use anti_webui_style::Color;
pub use parley::FontData;
use std::sync::Arc;

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

    /// The clip moved by `(dx, dy)`. "No clip" stays exactly that, so it compares equal at any
    /// translation.
    pub fn translated(&self, dx: i32, dy: i32) -> Clip {
        if self.rect == RectI::INFINITE {
            *self
        } else {
            Clip {
                rect: self.rect.translate(dx, dy),
                radii: self.radii,
            }
        }
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

/// Scrolled content, which the renderer keeps in a texture so that scrolling only draws what
/// it exposes (see `docs/design/05-rendering.md`).
#[derive(Clone, Debug)]
pub struct ScrollLayer {
    /// Identifies the layer across frames.
    pub key: u64,
    /// Where the layer shows, in window coordinates.
    pub clip: Clip,
    /// The window position of the content's origin. Scrolling only changes this.
    pub origin: PointI,
    /// What to draw, in content coordinates and without this layer's clip. Layers don't nest.
    /// Sharing the allocation means the content is unchanged.
    pub content: Arc<Scene>,
}

impl PartialEq for ScrollLayer {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.clip == other.clip
            && self.origin == other.origin
            && (Arc::ptr_eq(&self.content, &other.content)
                || (self.content.primitives == other.content.primitives
                    && self.content.chunks == other.content.chunks))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Glyphs(GlyphRun),
    Layer(ScrollLayer),
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
