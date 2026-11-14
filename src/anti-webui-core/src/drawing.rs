//! A fixed list of glyphs, rectangles and shapes measured in em, drawn at the node's text size:
//! what a formula is made of. See `docs/design/40-math.md`.

use crate::drag::{DragData, DropEvent, GrabEvent};
use crate::event::{DragResult, KeyboardEvent, PinchEvent, PointerEvent, ScrollEvent};
use crate::geometry::{Affine, PointF, RectI, Scale};
pub use crate::scene::{FontData, Glyph, PathGeometry};
use crate::scene::{GlyphRun, PathDraw, PathPaint, Primitive, Scene};
use crate::shader::IntoImage;
use crate::tree::{Node, NodeKind};
use crate::view::{ElementConfig, IntoView, NodeRef, View, also, also_bool};
use anti_webui_style::{Color, FillRule, LineCap, LineJoin, Sheet, Style, Styled};
use std::rc::Rc;
use std::sync::Arc;

/// What a [`drawing`] shows. Lengths are in em; `y` is down from the baseline, so what is above it
/// is negative.
#[derive(Clone, Debug, Default)]
pub struct Drawing {
    pub width: f32,
    /// How far it reaches above the baseline.
    pub ascent: f32,
    /// How far it reaches below the baseline.
    pub descent: f32,
    pub items: Vec<DrawingItem>,
}

/// One thing of a [`Drawing`]. A color of `None` is the node's text color.
#[derive(Clone, Debug)]
pub enum DrawingItem {
    Glyphs {
        font: FontData,
        /// The font size in em.
        size: f32,
        color: Option<Color>,
        glyphs: Vec<Glyph>,
    },
    /// Its edges go to whole pixels, and it is at least a pixel across, as the borders that
    /// make lines in a page are.
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Option<Color>,
    },
    Path {
        /// In em, from the origin on the baseline.
        geometry: Arc<PathGeometry>,
        rule: FillRule,
        /// Draws the outline this many em wide, and at least a pixel, rather than the inside.
        stroke: Option<f32>,
        color: Option<Color>,
    },
}

/// A font made of bytes that are there for the life of the program, for [`DrawingItem::Glyphs`].
/// Make it once: the renderer keeps the glyphs of each font it is given.
pub fn static_font(bytes: &'static [u8], index: u32) -> FontData {
    FontData::new(parley::fontique::Blob::new(Arc::new(bytes)), index)
}

impl Drawing {
    pub fn new(width: f32, ascent: f32, descent: f32) -> Self {
        Self {
            width,
            ascent,
            descent,
            items: Vec::new(),
        }
    }
}

#[doc(hidden)]
pub struct DrawingData {
    pub drawing: Arc<Drawing>,
    /// How many of the inherited text size make an em.
    pub em_scale: f32,
}

impl DrawingData {
    /// Logical pixels per em.
    pub fn em(&self, node: &Node) -> f32 {
        node.computed.font_size * self.em_scale
    }

    /// The size it would like, in logical pixels.
    pub fn natural(&self, node: &Node) -> (f32, f32) {
        let em = self.em(node);
        let d = &self.drawing;
        (d.width * em, (d.ascent + d.descent) * em)
    }

    /// The baseline below the top of the content box, in physical pixels.
    pub fn baseline(&self, node: &Node, scale: Scale) -> i32 {
        scale.px(self.drawing.ascent * self.em(node))
    }
}

/// A box showing a [`Drawing`], with the inherited text size as its em. It is as big as the drawing,
/// like a picture, and its baseline is the drawing's when it is among the words of a `rich_text`.
pub fn drawing(drawing: Arc<Drawing>) -> DrawingBox {
    DrawingBox {
        cfg: ElementConfig::default(),
        data: DrawingData {
            drawing,
            em_scale: 1.0,
        },
    }
}

pub struct DrawingBox {
    cfg: ElementConfig,
    data: DrawingData,
}

impl Styled for DrawingBox {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl DrawingBox {
    crate::view::element_methods!();

    /// How many of the inherited text size make an em: KaTeX draws formulas at 1.21 of the text
    /// around them.
    pub fn em_scale(mut self, k: f32) -> Self {
        self.data.em_scale = k.max(0.0);
        self
    }
}

impl IntoView for DrawingBox {
    fn into_view(self) -> View {
        View(
            self.cfg
                .build(Node::new(NodeKind::Drawing(Box::new(self.data)))),
        )
    }
}

/// What the node's drawing paints, faded by `opacity`.
pub(crate) fn paint(
    node: &Node,
    data: &DrawingData,
    scale: Scale,
    opacity: f32,
    scene: &mut Scene,
) {
    let em = scale.pxf(data.em(node));
    if em <= 0.0 {
        return;
    }
    let (b, p) = (&node.resolved.border, &node.resolved.padding);
    let ox = (node.abs.x + b.left + p.left) as f32;
    let base = (node.abs.y + b.top + p.top + data.baseline(node, scale)) as f32;
    let clip = node.clip_shape();
    let color_of = |c: Option<Color>| {
        let c = c.unwrap_or(node.computed.color);
        c.with_alpha(c.a * opacity)
    };
    for item in &data.drawing.items {
        match item {
            DrawingItem::Glyphs {
                font,
                size,
                color,
                glyphs,
            } => {
                let color = color_of(*color);
                if color.is_transparent() || glyphs.is_empty() {
                    continue;
                }
                scene.primitives.push(Primitive::Glyphs(GlyphRun {
                    font: font.clone(),
                    size: size * em,
                    normalized_coords: Vec::new(),
                    embolden: false,
                    skew: None,
                    color,
                    glyphs: glyphs
                        .iter()
                        .map(|g| Glyph {
                            id: g.id,
                            x: ox + g.x * em,
                            y: (base + g.y * em).round(),
                        })
                        .collect(),
                    clip,
                }));
            }
            DrawingItem::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                let edge = |at: f32, len: f32, from: f32| {
                    let a = (from + at * em).round() as i32;
                    let b = (from + (at + len) * em).round() as i32;
                    // A line thinner than a pixel still shows, a pixel thick.
                    if len > 0.0 {
                        (a, (b - a).max(1))
                    } else {
                        (a, 0)
                    }
                };
                let (x0, w) = edge(*x, *width, ox);
                let (y0, h) = edge(*y, *height, base);
                scene.fill(RectI::new(x0, y0, w, h), color_of(*color), clip);
            }
            DrawingItem::Path {
                geometry,
                rule,
                stroke,
                color,
            } => {
                let c = color.unwrap_or(node.computed.color);
                if c.is_transparent() {
                    continue;
                }
                let stroke = stroke.map(|w| {
                    Box::new(crate::scene::StrokeStyle {
                        width: (w * em).max(1.0) / em,
                        cap: LineCap::Butt,
                        join: LineJoin::Miter,
                        miter_limit: 4.0,
                        dashes: None,
                        dash_offset: 0.0,
                    })
                });
                scene.primitives.push(Primitive::Path(PathDraw {
                    geometry: geometry.clone(),
                    stroke,
                    rule: *rule,
                    paint: PathPaint::Color(c),
                    opacity,
                    transform: Affine::translation(ox as f64, base as f64)
                        .then_apply(&Affine::scaling(em as f64, em as f64)),
                    bounds: node.abs,
                    clip,
                    quads: None,
                }));
            }
        }
    }
}
