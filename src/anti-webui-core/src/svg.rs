//! Vector drawing in the tree: `svg` and the shapes in it, which are nodes like any other as far as
//! styles, events and reactivity go, but take their place from their own coordinates. See
//! `docs/design/25-svg.md`.

use crate::drag::{DragData, DropEvent, GrabEvent};
use crate::event::{DragResult, KeyboardEvent, PinchEvent, PointerEvent, ScrollEvent};
use crate::geometry::{Affine, PointF, RectI};
use crate::scene::{ClipQuad, PathDraw, PathGeometry, PathPaint, StrokeStyle};
use crate::shader::IntoImage;
use crate::tree::{Node, NodeKind, with_doc};
use crate::view::{
    ElementConfig, IntoTextSource, IntoView, NodeRef, Text, View, also, also_bool,
    text as text_view,
};
use anti_webui_style::{ComputedStyle, LineCap, LineJoin, Sheet, Style, Styled, SvgPaint};
use kurbo::{BezPath, ParamCurveNearest, Point, Shape as _};
use std::rc::Rc;
use std::sync::Arc;

#[cfg(feature = "svg")]
pub use crate::svg_file::parse;

pub use crate::geometry::Affine as Transform;
#[doc(hidden)]
pub use kurbo;

/// How a `view_box` goes in the box when their proportions differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgAspect {
    /// Where it goes within the box: 0 at the start, 0.5 in the middle, 1 at the end. `None` means
    /// stretching it to the box (`preserveAspectRatio="none"`).
    pub align: Option<(f32, f32)>,
    /// All of the box is covered, cutting off what doesn't fit, rather than all of it showing.
    pub slice: bool,
}

impl Default for SvgAspect {
    fn default() -> Self {
        Self {
            align: Some((0.5, 0.5)),
            slice: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SvgData {
    /// `x y width height`: what shows in the box.
    pub view_box: Option<[f32; 4]>,
    pub aspect: SvgAspect,
}

impl SvgData {
    pub(crate) fn ratio(&self) -> Option<f32> {
        self.view_box
            .filter(|v| v[2] > 0.0 && v[3] > 0.0)
            .map(|v| v[2] / v[3])
    }

    /// From its coordinates to those of a box of this size, in physical pixels.
    pub(crate) fn to_box(&self, width: f64, height: f64, logical: f64) -> Affine {
        let Some(v) = self.view_box.filter(|v| v[2] > 0.0 && v[3] > 0.0) else {
            return Affine::scaling(logical, logical);
        };
        let (vw, vh) = (v[2] as f64, v[3] as f64);
        let (sx, sy, ax, ay) = match self.aspect.align {
            None => (width / vw, height / vh, 0.0, 0.0),
            Some((ax, ay)) => {
                let (sx, sy) = (width / vw, height / vh);
                let s = if self.aspect.slice {
                    sx.max(sy)
                } else {
                    sx.min(sy)
                };
                (s, s, ax as f64, ay as f64)
            }
        };
        let e = (width - vw * sx) * ax - v[0] as f64 * sx;
        let f = (height - vh * sy) * ay - v[1] as f64 * sy;
        Affine {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e,
            f,
        }
    }
}

/// A shape, or a group when it has no geometry.
#[derive(Clone, Debug)]
pub struct ShapeData {
    pub geometry: Option<Arc<PathGeometry>>,
    pub transform: Affine,
    /// From its coordinates to the box of the `svg`, in physical pixels.
    pub world: Affine,
    /// A group cuts what is in it to this, in its coordinates.
    pub clip: Option<ClipQuad>,
    /// What it is cut to by the groups around it, in the coordinates of the box of the `svg`.
    pub clips: Option<Arc<[ClipQuad]>>,
}

impl Default for ShapeData {
    fn default() -> Self {
        Self {
            geometry: None,
            transform: Affine::IDENTITY,
            world: Affine::IDENTITY,
            clip: None,
            clips: None,
        }
    }
}

/// Which part of a text is at its `x`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

/// Which part of a text is at its `y`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextBaseline {
    #[default]
    Alphabetic,
    /// Half of the height of an `x` above the baseline.
    Middle,
    /// Half way between the ascent and the descent of the font.
    Central,
    /// The top of its line box.
    Hanging,
}

#[derive(Clone, Debug, Default)]
pub struct SvgTextData {
    pub x: f32,
    pub y: f32,
    pub anchor: TextAnchor,
    pub baseline: TextBaseline,
    pub transform: Option<Affine>,
    /// How wide it can get before it wraps, in its coordinates. Without one it stays on its lines.
    pub max_width: Option<f32>,
}

/// The inner box around an outline, for what can stick out of the curves.
pub(crate) fn stroke_reach(c: &ComputedStyle) -> f64 {
    if matches!(c.stroke_paint, SvgPaint::None) || c.stroke_width <= 0.0 {
        return 0.0;
    }
    let half = c.stroke_width as f64 * 0.5;
    let joints: f64 = match c.line_join {
        LineJoin::Miter => (c.miter_limit as f64).max(1.0),
        _ => 1.0,
    };
    let caps: f64 = match c.line_cap {
        LineCap::Square => std::f64::consts::SQRT_2,
        _ => 1.0,
    };
    half * joints.max(caps)
}

/// The box around what a shape can touch, in the coordinates of the `svg`.
pub(crate) fn shape_rect(
    geometry: &PathGeometry,
    world: &Affine,
    c: &ComputedStyle,
    logical: f64,
) -> RectI {
    let b = geometry.bounds();
    if b.is_zero_area() && matches!(c.stroke_paint, SvgPaint::None) {
        return RectI::default();
    }
    let mut reach = stroke_reach(c);
    if c.non_scaling_stroke {
        reach *= logical / world.max_scale().max(1e-6);
    }
    let (x0, y0, x1, y1) = (b.x0 - reach, b.y0 - reach, b.x1 + reach, b.y1 + reach);
    let corners = [
        world.apply(x0, y0),
        world.apply(x1, y0),
        world.apply(x0, y1),
        world.apply(x1, y1),
    ];
    let (mut left, mut top, mut right, mut bottom) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in corners {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    // Far enough for the edges of the pixels it can touch.
    let (left, top) = (left.floor() as i32 - 1, top.floor() as i32 - 1);
    let (right, bottom) = (right.ceil() as i32 + 1, bottom.ceil() as i32 + 1);
    RectI::new(left, top, right - left, bottom - top)
}

/// Whether a point in the coordinates of the `svg` is on what the shape paints.
pub(crate) fn shape_contains(node: &Node, data: &ShapeData, p: PointF) -> bool {
    let Some(geometry) = &data.geometry else {
        return false;
    };
    let (ox, oy) = (
        (node.abs.x - node.rect.x) as f32,
        (node.abs.y - node.rect.y) as f32,
    );
    let Some(inverse) = data.world.inverse() else {
        return false;
    };
    let (x, y) = inverse.apply((p.x - ox) as f64, (p.y - oy) as f64);
    let point = Point::new(x, y);
    let c = &node.computed;
    if !matches!(c.fill_paint, SvgPaint::None) {
        let winding = geometry.path().winding(point);
        let inside = match c.fill_rule {
            anti_webui_style::FillRule::NonZero => winding != 0,
            anti_webui_style::FillRule::EvenOdd => winding % 2 != 0,
        };
        if inside {
            return true;
        }
    }
    if matches!(c.stroke_paint, SvgPaint::None) || c.stroke_width <= 0.0 {
        return false;
    }
    let reach = c.stroke_width as f64 * 0.5;
    geometry
        .path()
        .segments()
        .any(|segment| segment.nearest(point, 0.05).distance_sq <= reach * reach)
}

/// What the shape paints, in the order of SVG: the inside, then the outline.
pub(crate) fn shape_draws(
    node: &Node,
    data: &ShapeData,
    opacity: f32,
    logical: f64,
    mut out: impl FnMut(PathDraw),
) {
    let Some(geometry) = &data.geometry else {
        return;
    };
    let c = &node.computed;
    let (ox, oy) = (
        (node.abs.x - node.rect.x) as f64,
        (node.abs.y - node.rect.y) as f64,
    );
    let transform = Affine::translation(ox, oy).then_apply(&data.world);
    let quads: Option<Arc<[ClipQuad]>> = data.clips.as_ref().map(|clips| {
        clips
            .iter()
            .map(|q| q.map(|(x, y)| (x + ox, y + oy)))
            .collect()
    });
    let draw = |paint, stroke, opacity: f32| PathDraw {
        geometry: geometry.clone(),
        stroke,
        rule: c.fill_rule,
        paint,
        opacity,
        transform,
        bounds: node.abs,
        clip: node.clip_shape(),
        quads: quads.clone(),
    };
    let paint_of = |paint: &SvgPaint| match paint {
        SvgPaint::None => None,
        SvgPaint::Color(color) if color.is_transparent() => None,
        SvgPaint::Color(color) => Some(PathPaint::Color(*color)),
        SvgPaint::CurrentColor => Some(PathPaint::Color(c.color)),
        SvgPaint::Gradient(g) => Some(PathPaint::Gradient(g.clone())),
    };
    if let Some(paint) = paint_of(&c.fill_paint) {
        out(draw(paint, None, c.fill_opacity * opacity));
    }
    if c.stroke_width > 0.0
        && let Some(paint) = paint_of(&c.stroke_paint)
    {
        let width = if c.non_scaling_stroke {
            c.stroke_width as f64 * logical / data.world.max_scale().max(1e-6)
        } else {
            c.stroke_width as f64
        };
        let stroke = StrokeStyle {
            width: width as f32,
            cap: c.line_cap,
            join: c.line_join,
            miter_limit: c.miter_limit,
            dashes: c.dash_array.clone(),
            dash_offset: c.dash_offset,
        };
        out(draw(
            paint,
            Some(Box::new(stroke)),
            c.stroke_opacity * opacity,
        ));
    }
}

/// How big a line of text is with a style, in logical pixels, for laying out diagrams before they are
/// made. What `style` doesn't say is the default, as nothing is inherited here.
pub fn measure_text(text: &str, style: &Style) -> (f32, f32) {
    let scale = with_doc(|d| d.scale);
    let mut computed = ComputedStyle::default();
    style.apply_to(&mut computed);
    let shape = crate::text::ShapeStyle::from_computed(&computed, scale);
    crate::text::with_text(|ts| {
        let mut layout = crate::text::TextLayout::default();
        layout.shape(ts, text, 1, &shape);
        layout.break_lines(None);
        let size = layout.size();
        (
            scale.to_logical(size.width as f32),
            scale.to_logical(size.height as f32),
        )
    })
}

/// What a shape's outline is.
pub trait IntoGeometry {
    fn into_geometry(self) -> BezPath;
}

impl IntoGeometry for BezPath {
    fn into_geometry(self) -> BezPath {
        self
    }
}

impl IntoGeometry for &str {
    /// Path data as in the `d` of SVG. What can't be read comes out as nothing.
    fn into_geometry(self) -> BezPath {
        BezPath::from_svg(self).unwrap_or_else(|e| {
            log::warn!("path data that can't be read: {e}");
            BezPath::new()
        })
    }
}

impl IntoGeometry for String {
    fn into_geometry(self) -> BezPath {
        self.as_str().into_geometry()
    }
}

pub enum PathSource {
    Static(BezPath),
    Dynamic(Box<dyn Fn() -> BezPath>),
}

/// What a [`path`] follows: path data, curves, or a closure saying which, for shapes that change.
pub trait IntoPathSource {
    fn into_path_source(self) -> PathSource;
}

impl IntoPathSource for BezPath {
    fn into_path_source(self) -> PathSource {
        PathSource::Static(self)
    }
}

impl IntoPathSource for &str {
    fn into_path_source(self) -> PathSource {
        PathSource::Static(self.into_geometry())
    }
}

impl IntoPathSource for String {
    fn into_path_source(self) -> PathSource {
        PathSource::Static(self.into_geometry())
    }
}

impl<F, S> IntoPathSource for F
where
    F: Fn() -> S + 'static,
    S: IntoGeometry,
{
    fn into_path_source(self) -> PathSource {
        PathSource::Dynamic(Box::new(move || self().into_geometry()))
    }
}

/// Vector drawing, like `<svg>`: a box with its own coordinates for the shapes in it. Without a
/// [`Svg::view_box`], those are logical pixels.
pub struct Svg {
    cfg: ElementConfig,
    data: SvgData,
    children: Vec<View>,
}

/// An empty `svg`, to put shapes in.
pub fn svg() -> Svg {
    Svg {
        cfg: ElementConfig::default(),
        data: SvgData::default(),
        children: Vec::new(),
    }
}

impl Styled for Svg {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Svg {
    crate::view::element_methods!();

    /// What shows in the box: from its coordinates `x, y`, this wide and this high.
    pub fn view_box(mut self, x: f32, y: f32, width: f32, height: f32) -> Self {
        self.data.view_box = Some([x, y, width, height]);
        self
    }

    pub fn aspect(mut self, aspect: SvgAspect) -> Self {
        self.data.aspect = aspect;
        self
    }

    pub fn child(mut self, child: impl IntoView) -> Self {
        self.children.push(child.into_view());
        self
    }

    pub fn children<V: IntoView>(mut self, children: impl IntoIterator<Item = V>) -> Self {
        self.children
            .extend(children.into_iter().map(IntoView::into_view));
        self
    }
}

impl IntoView for Svg {
    fn into_view(mut self) -> View {
        // Like the `svg` of a browser in a page: what is outside of the box doesn't show.
        let style = &mut self.cfg.style;
        style
            .overflow_x
            .get_or_insert(anti_webui_style::Overflow::Hidden);
        style
            .overflow_y
            .get_or_insert(anti_webui_style::Overflow::Hidden);
        let id = self
            .cfg
            .build(Node::new(NodeKind::Svg(Box::new(self.data))));
        let children: Vec<_> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        View(id)
    }
}

/// A shape in an `svg`.
pub struct Shape {
    cfg: ElementConfig,
    source: PathSource,
    transform: Affine,
    dyn_transform: Option<Box<dyn Fn() -> Affine>>,
}

/// A group of shapes, like `<g>`: what is set on it goes down to them.
pub struct Group {
    cfg: ElementConfig,
    clip: Option<ClipQuad>,
    transform: Affine,
    dyn_transform: Option<Box<dyn Fn() -> Affine>>,
    children: Vec<View>,
}

pub fn group() -> Group {
    Group {
        cfg: ElementConfig::default(),
        clip: None,
        transform: Affine::IDENTITY,
        dyn_transform: None,
        children: Vec::new(),
    }
}

/// A shape along path data as in the `d` of SVG, along curves, or along what a closure says.
pub fn path(source: impl IntoPathSource) -> Shape {
    Shape {
        cfg: ElementConfig::default(),
        source: source.into_path_source(),
        transform: Affine::IDENTITY,
        dyn_transform: None,
    }
}

pub fn rect(x: f64, y: f64, width: f64, height: f64) -> Shape {
    path(kurbo::Rect::new(x, y, x + width, y + height).to_path(0.1))
}

pub fn rounded_rect(x: f64, y: f64, width: f64, height: f64, radius: f64) -> Shape {
    let r = kurbo::RoundedRect::new(x, y, x + width, y + height, radius);
    path(r.to_path(0.05))
}

pub fn circle(cx: f64, cy: f64, r: f64) -> Shape {
    path(kurbo::Circle::new((cx, cy), r).to_path(0.05))
}

pub fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Shape {
    path(kurbo::Ellipse::new((cx, cy), (rx, ry), 0.0).to_path(0.05))
}

pub fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Shape {
    let mut p = BezPath::new();
    p.move_to((x1, y1));
    p.line_to((x2, y2));
    path(p)
}

fn through(points: impl IntoIterator<Item = (f64, f64)>) -> BezPath {
    let mut p = BezPath::new();
    for (i, point) in points.into_iter().enumerate() {
        if i == 0 {
            p.move_to(point);
        } else {
            p.line_to(point);
        }
    }
    p
}

pub fn polyline(points: impl IntoIterator<Item = (f64, f64)>) -> Shape {
    path(through(points))
}

pub fn polygon(points: impl IntoIterator<Item = (f64, f64)>) -> Shape {
    let mut p = through(points);
    p.close_path();
    path(p)
}

impl Styled for Shape {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Styled for Group {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Shape {
    crate::view::element_methods!();

    /// Where its coordinates are in those around it, like the `transform` of SVG.
    pub fn transform(mut self, transform: Affine) -> Self {
        self.transform = transform;
        self
    }

    /// The same, for what changes.
    pub fn dyn_transform(mut self, f: impl Fn() -> Affine + 'static) -> Self {
        self.dyn_transform = Some(Box::new(f));
        self
    }
}

impl Group {
    crate::view::element_methods!();

    /// Cuts the shapes in it to a rectangle, in its coordinates. Text isn't cut.
    pub fn clip_rect(self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.clip_quad([
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
        ])
    }

    pub(crate) fn clip_quad(mut self, quad: ClipQuad) -> Self {
        self.clip = Some(quad);
        self
    }

    /// Where its coordinates are in those around it, like the `transform` of SVG.
    pub fn transform(mut self, transform: Affine) -> Self {
        self.transform = transform;
        self
    }

    /// The same, for what changes.
    pub fn dyn_transform(mut self, f: impl Fn() -> Affine + 'static) -> Self {
        self.dyn_transform = Some(Box::new(f));
        self
    }

    pub fn child(mut self, child: impl IntoView) -> Self {
        self.children.push(child.into_view());
        self
    }

    pub fn children<V: IntoView>(mut self, children: impl IntoIterator<Item = V>) -> Self {
        self.children
            .extend(children.into_iter().map(IntoView::into_view));
        self
    }
}

fn bind_transform(id: crate::tree::NodeId, dynamic: Option<Box<dyn Fn() -> Affine>>) {
    let Some(f) = dynamic else {
        return;
    };
    crate::reactive::effect(move || {
        let transform = f();
        with_doc(|d| {
            if let Some(n) = d.get_mut(id)
                && let NodeKind::SvgShape(s) = &mut n.kind
                && s.transform != transform
            {
                s.transform = transform;
                d.invalidate_layout(id);
            }
        });
    });
}

impl IntoView for Shape {
    fn into_view(self) -> View {
        let (geometry, dynamic) = match self.source {
            PathSource::Static(p) => (PathGeometry::new(p), None),
            PathSource::Dynamic(f) => (PathGeometry::new(BezPath::new()), Some(f)),
        };
        let data = ShapeData {
            geometry: Some(geometry),
            transform: self.transform,
            ..ShapeData::default()
        };
        let id = self
            .cfg
            .build(Node::new(NodeKind::SvgShape(Box::new(data))));
        if let Some(f) = dynamic {
            crate::reactive::effect(move || {
                let geometry = PathGeometry::new(f());
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id)
                        && let NodeKind::SvgShape(s) = &mut n.kind
                    {
                        s.geometry = Some(geometry);
                        d.invalidate_layout(id);
                    }
                });
            });
        }
        bind_transform(id, self.dyn_transform);
        View(id)
    }
}

impl IntoView for Group {
    fn into_view(self) -> View {
        let data = ShapeData {
            transform: self.transform,
            clip: self.clip,
            ..ShapeData::default()
        };
        let id = self
            .cfg
            .build(Node::new(NodeKind::SvgShape(Box::new(data))));
        let children: Vec<_> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        bind_transform(id, self.dyn_transform);
        View(id)
    }
}

/// Text in an `svg`: it starts at a point rather than being in a box, and scales with what is around
/// it. It is painted with `fill`, and selected like other text.
pub struct SvgText {
    cfg: ElementConfig,
    data: SvgTextData,
    text: Text,
}

/// Text at the start of its coordinates, until told otherwise with [`SvgText::at`].
pub fn text(source: impl IntoTextSource) -> SvgText {
    SvgText {
        cfg: ElementConfig::default(),
        data: SvgTextData::default(),
        text: text_view(source),
    }
}

impl Styled for SvgText {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl SvgText {
    crate::view::element_methods!();

    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.data.x = x;
        self.data.y = y;
        self
    }

    pub fn anchor(mut self, anchor: TextAnchor) -> Self {
        self.data.anchor = anchor;
        self
    }

    pub fn baseline(mut self, baseline: TextBaseline) -> Self {
        self.data.baseline = baseline;
        self
    }

    pub fn transform(mut self, transform: Affine) -> Self {
        self.data.transform = Some(transform);
        self
    }

    /// Wraps at this width, in its coordinates.
    pub fn max_width(mut self, width: f32) -> Self {
        self.data.max_width = Some(width);
        self
    }
}

impl IntoView for SvgText {
    fn into_view(self) -> View {
        let id = self
            .cfg
            .build(Node::new(NodeKind::SvgText(Box::new(self.data))));
        let child = self.text.into_view().0;
        with_doc(|d| {
            if let Some(n) = d.get_mut(child)
                && let NodeKind::Text(t) = &mut n.kind
            {
                t.in_svg = true;
            }
            d.set_children(id, vec![child]);
        });
        View(id)
    }
}
