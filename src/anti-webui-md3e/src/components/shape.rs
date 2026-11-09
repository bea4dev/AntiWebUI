//! The shapes of the Material 3 Expressive catalog, which change into each other like springs. See
//! `docs/design/15-md3e.md`.

use crate::color::{ColorScheme, color_scheme};
use crate::motion::default_spatial;
use crate::shapes::{SAMPLES, Table, table};
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub use crate::shapes::MaterialShape;

#[derive(ShaderParams, Clone, Copy)]
struct ShapeParams {
    color: Color,
    origin: [f32; 2],
    scale: f32,
    rotation: f32,
    stroke: f32,
    /// The distance to the nearest part of the outline.
    least: f32,
    table: [[f32; 4]; SAMPLES / 4],
}

static SHAPE: Shader<ShapeParams> = Shader::new("shape", include_str!("../../shaders/shape.wgsl"));

pub(super) fn preload_shaders() {
    SHAPE.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&SHAPE)
}

/// Paints a table over the box, `scale` of its size, turned by `radians` around its middle. A
/// `stroke` above 0 paints only that much inside of the outline.
pub(super) fn paint_table(
    t: &Table,
    color: Color,
    scale: f32,
    radians: f32,
    stroke: f32,
) -> ShaderPaint {
    SHAPE.params(ShapeParams {
        color,
        origin: t.origin,
        scale,
        rotation: radians,
        stroke,
        least: t.radii.iter().copied().fold(f32::MAX, f32::min),
        table: t.words(),
    })
}

/// How a [`styled_shape`] looks and moves.
#[derive(Clone)]
pub struct ShapeStyle {
    color: Rc<dyn Fn(&ColorScheme) -> Color>,
    spring: Spring,
    rotation: Option<Rc<dyn Fn() -> f32>>,
    stroke: f32,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            color: Rc::new(|c| c.primary),
            spring: default_spatial(),
            rotation: None,
            stroke: 0.0,
        }
    }
}

impl ShapeStyle {
    /// Filled with `primary`, changing shape with the default spatial spring of the Expressive
    /// motion scheme.
    pub fn new() -> Self {
        Self::default()
    }

    /// The color, which follows the color scheme.
    pub fn color(mut self, f: impl Fn(&ColorScheme) -> Color + 'static) -> Self {
        self.color = Rc::new(f);
        self
    }

    /// How it goes from one shape to the next, and how it turns.
    pub fn spring(mut self, spring: Spring) -> Self {
        self.spring = spring;
        self
    }

    /// Turns it clockwise by what `f` returns, in degrees, around the middle of the box. Changes
    /// go with the same spring as those of the shape. What is turned out of the box is cut off.
    pub fn rotation(mut self, f: impl Fn() -> f32 + 'static) -> Self {
        self.rotation = Some(Rc::new(f));
        self
    }

    /// Paints only a line this wide inside of the outline, instead of filling it.
    pub fn stroke(mut self, width: f32) -> Self {
        self.stroke = width;
        self
    }
}

/// A box filled with the shape `shape` returns, in `primary`. When that changes, the shape changes
/// into the new one like a spring, from wherever it is at the time. See [`styled_shape`].
pub fn shape(shape: impl Fn() -> MaterialShape + 'static) -> Div {
    styled_shape(shape, ShapeStyle::new())
}

/// A box filled with a shape of the catalog, which changes into the next whenever `shape` returns
/// another.
///
/// It is 48 by 48 unless sized otherwise, and the shape is stretched over the box, as it is in
/// Compose; give it a square box to keep it as it is. It is painted under the background of the
/// box, so the box should have none. Its children go on top and are not cut to the shape. While
/// it doesn't change, it asks for no frames.
///
/// ```no_run
/// use anti_webui::prelude::*;
/// use anti_webui_md3e::components::{MaterialShape, ShapeStyle, styled_shape};
///
/// let hovered = signal(false);
/// styled_shape(
///     move || if hovered.get() { MaterialShape::Cookie9Sided } else { MaterialShape::Circle },
///     ShapeStyle::new().color(|c| c.tertiary),
/// )
/// .w(64.0)
/// .h(64.0)
/// .on_hover(move |on| hovered.set(on));
/// ```
pub fn styled_shape(shape: impl Fn() -> MaterialShape + 'static, style: ShapeStyle) -> Div {
    crate::ensure_ready();
    let ShapeStyle {
        color,
        spring,
        rotation,
        stroke,
    } = style;
    let first = untrack(&shape);
    // What it changes from and into, and how far it has got.
    let ends = signal((table(first), table(first)));
    let progress = animated(1.0f32);
    let last = Cell::new(first);
    effect(move || {
        let wanted = shape();
        if last.replace(wanted) != wanted {
            untrack(|| {
                let t = progress.get_untracked();
                let now = ends.with_untracked(|(a, b)| a.mix(b, t));
                ends.set((Rc::new(now), table(wanted)));
                progress.set(0.0);
                progress.spring_to(1.0, spring);
            });
        }
    });
    let turned = rotation.map(|f| follow(move || f(), spring));
    div().w(48.0).h(48.0).flex_none().fill(move || {
        let t = progress.get();
        let degrees = turned.map_or(0.0, |r| r.get());
        let c = color(&color_scheme());
        ends.with(|(a, b)| {
            let paint = |table: &Table| paint_table(table, c, 1.0, degrees.to_radians(), stroke);
            if t == 1.0 {
                paint(b)
            } else {
                paint(&a.mix(b, t))
            }
        })
    })
}
