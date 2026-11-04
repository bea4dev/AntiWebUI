//! The renderer-independent display list produced each frame. Coordinates are physical pixels.

use crate::geometry::{Affine, PointI, RectI};
use crate::shader::{Image, ShaderPaint};
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

    /// The clip for content drawn `k` times bigger.
    pub fn scaled(&self, k: i32) -> Clip {
        if self.rect == RectI::INFINITE {
            *self
        } else {
            Clip {
                rect: self.rect.scaled(k),
                radii: self.radii.map(|r| r * k as f32),
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
    /// The shape that is blurred.
    pub rect: RectI,
    pub radii: [f32; 4],
    pub blur: f32,
    pub color: Color,
    pub clip: Clip,
    /// An outer shadow shows only outside of this rounded rectangle, an inset one only inside.
    pub cut: RectI,
    pub cut_radii: [f32; 4],
    pub inset: bool,
}

impl Quad {
    /// The same box drawn `k` times bigger.
    pub fn scaled(&self, k: i32) -> Quad {
        Quad {
            rect: self.rect.scaled(k),
            background: self.background,
            border_widths: self.border_widths.map(|w| w * k),
            border_color: self.border_color,
            radii: self.radii.map(|r| r * k as f32),
            clip: self.clip.scaled(k),
        }
    }
}

impl Shadow {
    /// The same shadow drawn `k` times bigger.
    pub fn scaled(&self, k: i32) -> Shadow {
        let f = k as f32;
        Shadow {
            rect: self.rect.scaled(k),
            radii: self.radii.map(|r| r * f),
            blur: self.blur * f,
            color: self.color,
            clip: self.clip.scaled(k),
            cut: self.cut.scaled(k),
            cut_radii: self.cut_radii.map(|r| r * f),
            inset: self.inset,
        }
    }

    /// How far the blur reaches past [`Shadow::rect`]: three standard deviations.
    pub fn extent(&self) -> i32 {
        (self.blur * 1.5 + 1.0).ceil() as i32
    }

    /// A rectangle containing every pixel the shadow can touch, before clipping.
    pub fn bounds(&self) -> RectI {
        if self.inset {
            self.cut
        } else {
            self.rect.outset(self.extent())
        }
    }

    /// A rectangle in which the shadow is known to be fully transparent, so that drawing can
    /// skip it. See `docs/design/09-shadows-stacking-popups.md`.
    pub fn hole(&self) -> RectI {
        // A corner's arc stays outside of the square cut `r * (1 - 1/sqrt 2)` in from the corner;
        // one more pixel covers the antialiased edge.
        let corners = |radii: &[f32; 4]| {
            (radii.iter().copied().fold(0.0, f32::max) * (1.0 - std::f32::consts::FRAC_1_SQRT_2))
                .ceil() as i32
                + 1
        };
        let (rect, inset) = if self.inset {
            (self.rect, corners(&self.radii) + self.extent())
        } else {
            (self.cut, corners(&self.cut_radii))
        };
        rect.inset(inset, inset, inset, inset)
    }
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

/// Content the renderer keeps in a texture and composites: scrolled content, so that scrolling
/// only draws what it exposes (see `docs/design/05-rendering.md`), or a transformed subtree (see
/// `docs/design/12-transform.md`).
#[derive(Clone, Debug)]
pub struct Layer {
    /// Identifies the layer across frames.
    pub key: u64,
    /// Where the layer may show, in the coordinates of the scene it is in.
    pub clip: Clip,
    /// The position of the content's origin. Scrolling only changes this.
    pub origin: PointI,
    /// Applied after `origin`.
    pub transform: Affine,
    /// What the content can touch, in its own coordinates. [`RectI::INFINITE`] for scrolled
    /// content, which `clip` limits instead.
    pub bounds: RectI,
    /// Applied to the content as one image.
    pub opacity: f32,
    /// What to draw, in content coordinates and without this layer's clip. Sharing the allocation
    /// means the content is unchanged.
    pub content: Arc<Scene>,
    /// What to show instead of the content as it is.
    pub filter: Option<Box<LayerFilter>>,
}

/// A picture made of a layer's content. See `docs/design/16-filters.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerFilter {
    pub image: Image,
    /// The box of the node, for shaders. In content coordinates.
    pub rect: RectI,
    /// Top, right, bottom, left.
    pub border: [i32; 4],
    /// Top, right, bottom, left.
    pub padding: [i32; 4],
    /// Top-left, top-right, bottom-right, bottom-left, in physical pixels.
    pub radii: [f32; 4],
    /// Physical pixels per logical pixel.
    pub scale: f32,
    /// How far a pixel of the result depends on the content.
    pub reach: i32,
}

impl Layer {
    /// From the content's coordinates to those of the scene the layer is in.
    pub fn matrix(&self) -> Affine {
        self.transform.then_apply(&Affine::translation(
            self.origin.x as f64,
            self.origin.y as f64,
        ))
    }

    /// Whether content pixels land exactly on pixels of the scene it is in.
    pub fn is_pixel_aligned(&self) -> bool {
        self.transform.is_whole_translation()
    }

    /// What the layer can touch in the scene it is in.
    pub fn bounds_on_parent(&self) -> RectI {
        if self.bounds == RectI::INFINITE {
            self.clip.rect
        } else {
            self.parent_rect(self.bounds).intersect(&self.clip.rect)
        }
    }

    /// What a rectangle of the content can touch in the scene the layer is in.
    pub fn parent_rect(&self, r: RectI) -> RectI {
        if self.is_pixel_aligned() {
            let m = self.matrix();
            r.translate(m.e.round() as i32, m.f.round() as i32)
        } else {
            // Filtering reads one pixel further.
            self.matrix().bounds_of(r.outset(1))
        }
    }

    /// Whether everything that decides where and how the content shows is the same. `bounds` only
    /// says what the content can touch: it may be too big, and changes without pixels changing.
    pub fn same_placement(&self, other: &Layer) -> bool {
        self.clip == other.clip
            && self.origin == other.origin
            && self.transform == other.transform
            && self.opacity == other.opacity
            && self.filter == other.filter
    }
}

impl PartialEq for Layer {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.same_placement(other)
            && (Arc::ptr_eq(&self.content, &other.content)
                || (self.content.primitives == other.content.primitives
                    && self.content.chunks == other.content.chunks))
    }
}

/// A box painted by a shader, cut to its rounded shape. See `docs/design/14-shaders.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderDraw {
    pub paint: ShaderPaint,
    pub rect: RectI,
    /// Top, right, bottom, left.
    pub border: [i32; 4],
    /// Top, right, bottom, left.
    pub padding: [i32; 4],
    /// Top-left, top-right, bottom-right, bottom-left, in physical pixels.
    pub radii: [f32; 4],
    /// Physical pixels per logical pixel.
    pub scale: f32,
    pub opacity: f32,
    pub clip: Clip,
}

/// What is behind a box made into a picture and shown in the box, which can be transformed: this
/// is how it goes with a layer of the same node. See `docs/design/16-filters.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct BackdropDraw {
    pub image: Image,
    pub rect: RectI,
    /// Top-left, top-right, bottom-right, bottom-left, in physical pixels.
    pub radii: [f32; 4],
    pub transform: Affine,
    /// Physical pixels per logical pixel.
    pub scale: f32,
    pub opacity: f32,
    pub clip: Clip,
}

impl BackdropDraw {
    /// What it can touch, before clipping.
    pub fn outer_rect(&self) -> RectI {
        if self.transform.is_whole_translation() {
            self.rect.translate(
                self.transform.e.round() as i32,
                self.transform.f.round() as i32,
            )
        } else {
            self.transform.bounds_of(self.rect.outset(1))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Glyphs(GlyphRun),
    Layer(Layer),
    Shader(ShaderDraw),
    Backdrop(BackdropDraw),
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
