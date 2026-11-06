//! Turns the laid-out tree into a [`Scene`].

use crate::damage::primitive_bounds;
use crate::geometry::Affine;
use crate::geometry::{PointF, PointI, RectI, SizeI};
use crate::layout::{child_clip, mark_outer_layers_stale};
use crate::platform::PopupId;
use crate::scene::{
    BackdropDraw, ChunkKey, Clip, Glyph, GlyphRun, ImageDraw, Layer, LayerFilter, Primitive, Quad,
    Scene, ShaderDraw, Shadow, VideoDraw,
};
use crate::shader::{Image, ShaderPaint};
use crate::text::selection::{node_range, selectable_text_nodes};
use crate::text::{Brush, TextLayout};
use crate::tree::{Document, LayerMemo, Node, NodeId, NodeKind, popup_id};
use anti_webui_style::{
    BackgroundRepeat, BackgroundSize, Color, ComputedStyle, ImageRendering, Length, ObjectFit,
    Overflow, SvgPaint,
};
use parley::PositionedLayoutItem;
use slotmap::{Key, KeyData};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const CARET_BLINK: Duration = Duration::from_millis(530);

/// Width of scrollbar thumbs in logical pixels.
const SCROLLBAR_WIDTH: f32 = 6.0;

/// A frame: what to show in the window and in each popup on the screen.
#[derive(Debug, Default)]
pub struct Frame {
    pub window: Scene,
    pub popups: Vec<(PopupId, Scene)>,
    /// What shows with the pointer for something being dragged, when it isn't in the window.
    pub drag_icon: Option<DragIconFrame>,
}

/// What goes on the surface of a drag.
#[derive(Debug)]
pub struct DragIconFrame {
    pub scene: Scene,
    /// The size of the surface.
    pub logical: SizeI,
    /// Where the pointer is in it, in logical pixels.
    pub hotspot: PointI,
}

/// Identifies what shows for something being dragged in scenes.
const DRAG_LAYER: u64 = u64::MAX - 1;

/// What it is painted from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DragContent {
    /// What was painted when it started.
    Snapshot,
    /// What is in this portal.
    Live(NodeId),
}

/// What is painted, what it touches, and the box of what it is of.
pub(crate) type DragPainted = (Arc<Scene>, RectI, RectI);

/// What shows for something being dragged.
pub(crate) struct DragVisual {
    pub content: DragContent,
    /// Where the pointer is, in window coordinates.
    pub position: PointI,
    /// Where in the box the pointer is, from 0 to 1.
    pub grip: (f32, f32),
    pub opacity: f32,
    /// It goes on a surface of its own.
    pub icon: bool,
    pub painted: Option<DragPainted>,
}

impl DragVisual {
    /// Where the pointer is in what is painted.
    fn hotspot(&self, b: RectI) -> PointI {
        PointI::new(
            b.x + (b.width as f32 * self.grip.0).round() as i32,
            b.y + (b.height as f32 * self.grip.1).round() as i32,
        )
    }

    /// What goes in a scene, with its hotspot at `at`.
    fn layer(&self, at: PointI) -> Option<Layer> {
        let (content, touched, b) = self.painted.as_ref()?;
        let hot = self.hotspot(*b);
        Some(Layer {
            key: DRAG_LAYER,
            clip: Clip::NONE,
            origin: PointI::new(at.x - hot.x, at.y - hot.y),
            transform: Affine::IDENTITY,
            bounds: *touched,
            opacity: snapped_opacity(self.opacity),
            content: content.clone(),
            filter: None,
        })
    }
}

/// Gives the layers in a scene keys of their own, because the nodes they are of are also painted
/// where they are.
fn rekeyed(mut scene: Scene) -> Scene {
    for p in &mut scene.primitives {
        if let Primitive::Layer(l) = p {
            l.key = !l.key;
            l.content = Arc::new(rekeyed((*l.content).clone()));
        }
    }
    scene
}

/// What a node looks like now, for what is dragged.
pub(crate) fn paint_snapshot(doc: &Document, id: NodeId, now: Instant) -> Option<DragPainted> {
    let b = doc.get(id)?.abs;
    let mut scene = Scene::default();
    let mut painter = Painter {
        doc,
        scene: &mut scene,
        now,
        screen: RectI::new(0, 0, doc.viewport.width, doc.viewport.height),
        old_layers: HashMap::new(),
        new_layers: HashMap::new(),
        reuse_layers: false,
        cull: false,
        settling: Vec::new(),
        magnified: 1.0,
        pictures: 0,
    };
    painter.node_contents(id, 1.0, RectI::INFINITE);
    let touched = what_it_touches(&scene);
    Some((Arc::new(rekeyed(scene)), touched, b))
}

/// Part of a layout with its coordinates at `origin`: the clusters in the byte range.
fn selected_runs(
    scene: &mut Scene,
    layout: &TextLayout,
    origin: PointI,
    (a, b): (usize, usize),
    style: &ComputedStyle,
    clip: Clip,
) {
    let color = style.color;
    let (ox, oy) = (origin.x as f32, origin.y as f32);
    for line in layout.layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            let baseline = oy + glyph_run.baseline();
            let reach = run.font_size() * 2.0;
            if baseline + reach < clip.rect.y as f32 || baseline - reach > clip.rect.bottom() as f32
            {
                continue;
            }
            let mut x = glyph_run.offset();
            let mut glyphs = Vec::new();
            let (mut left, mut right) = (f32::MAX, f32::MIN);
            for cluster in run.visual_clusters() {
                let range = cluster.text_range();
                let keep = range.start < b && range.end > a;
                for g in cluster.glyphs() {
                    if keep {
                        glyphs.push(Glyph {
                            id: g.id,
                            x: ox + x + g.x,
                            y: (baseline + g.y).round(),
                        });
                        left = left.min(ox + x);
                        right = right.max(ox + x + g.advance);
                    }
                    x += g.advance;
                }
            }
            if glyphs.is_empty() {
                continue;
            }
            let synthesis = run.synthesis();
            scene.primitives.push(Primitive::Glyphs(GlyphRun {
                font: run.font().clone(),
                size: run.font_size(),
                normalized_coords: rounded_coords(
                    run.normalized_coords().iter().map(|c| c.to_bits()),
                ),
                embolden: synthesis.embolden(),
                skew: synthesis.skew(),
                color,
                glyphs,
                clip,
            }));
            let m = run.font_metrics();
            let (x0, x1) = (left.round() as i32, right.round() as i32);
            let line_y = baseline.round() as i32;
            if style.underline {
                let t = m.underline_size.round().max(1.0) as i32;
                let y = line_y - m.underline_offset.round() as i32;
                scene.fill(RectI::new(x0, y, x1 - x0, t), color, clip);
            }
            if style.strikethrough {
                let t = m.strikethrough_size.round().max(1.0) as i32;
                let y = line_y - m.strikethrough_offset.round() as i32;
                scene.fill(RectI::new(x0, y, x1 - x0, t), color, clip);
            }
        }
    }
}

/// The selected text and nothing else: what Blink paints with `kSelectionDragImageOnly`, in window
/// coordinates. `edit` says it is the selection in that input rather than the document's.
pub(crate) fn paint_selected_text(doc: &Document, edit: Option<NodeId>) -> Option<DragPainted> {
    let screen = RectI::new(0, 0, doc.viewport.width, doc.viewport.height);
    let mut scene = Scene::default();
    let mut boxes = RectI::default();
    let mut add =
        |id: NodeId, layout: &TextLayout, origin: PointI, range: (usize, usize), inner| {
            let n = &doc.nodes[id];
            let visible = doc.visible_part(n, inner).intersect(&screen);
            if visible.is_empty() {
                return;
            }
            let there = doc.to_window(n, PointF::new(origin.x as f32, origin.y as f32));
            let at = PointI::new(there.x.round() as i32, there.y.round() as i32);
            let clip = Clip {
                rect: visible,
                radii: [0.0; 4],
            };
            selected_runs(&mut scene, layout, at, range, &n.computed, clip);
            for r in layout.range_rects(range.0, range.1, at.x, at.y) {
                boxes = boxes.union(&r.intersect(&visible));
            }
        };
    match edit {
        Some(id) => {
            let n = doc.get(id)?;
            let e = n.edit()?;
            let inner = child_clip(n, doc.scale, n.clip_shape()).rect;
            let origin = edit_text_origin(doc, id);
            add(id, &e.layout, origin, e.selection_range(), inner);
        }
        None => {
            let sel = doc.state.selection.as_ref()?;
            for id in selectable_text_nodes(doc) {
                let n = &doc.nodes[id];
                if let (Some(range), NodeKind::Text(t)) = (node_range(doc, sel, id), &n.kind) {
                    add(id, &t.layout, n.abs.origin(), range, n.abs);
                }
            }
        }
    }
    if boxes.is_empty() {
        return None;
    }
    // What hangs out of its line stays in, and the pointer is always on it.
    let touched = what_it_touches(&scene).union(&boxes);
    Some((Arc::new(scene), touched, boxes))
}

/// Paints what shows for what is dragged again, if it can change.
fn paint_drag_preview(
    doc: &mut Document,
    now: Instant,
    old_layers: &mut HashMap<NodeId, LayerMemo>,
    new_layers: &mut HashMap<NodeId, LayerMemo>,
    settling: &mut Vec<NodeId>,
) {
    let Some(DragContent::Live(portal)) = doc.drag.as_ref().map(|d| d.content) else {
        return;
    };
    let doc_ref = &*doc;
    let Some(inner) = doc_ref
        .get(portal)
        .and_then(|n| n.children.first().copied())
    else {
        return;
    };
    let b = doc_ref.nodes[inner].abs;
    let mut scene = Scene::default();
    let mut painter = Painter {
        doc: doc_ref,
        scene: &mut scene,
        now,
        screen: RectI::new(0, 0, doc_ref.viewport.width, doc_ref.viewport.height),
        old_layers: std::mem::take(old_layers),
        new_layers: std::mem::take(new_layers),
        reuse_layers: !doc_ref.dirty.content,
        cull: true,
        settling: Vec::new(),
        magnified: 1.0,
        pictures: 0,
    };
    painter.contents(portal, 1.0, RectI::INFINITE);
    settling.append(&mut painter.settling);
    *old_layers = painter.old_layers;
    *new_layers = painter.new_layers;
    let touched = what_it_touches(&scene);
    let drag = doc.drag.as_mut().expect("just looked");
    // Sharing the allocation is what says the content stays.
    let same = drag.painted.as_ref().is_some_and(|(old, _, _)| {
        old.primitives == scene.primitives && old.chunks == scene.chunks
    });
    if same {
        if let Some(painted) = &mut drag.painted {
            painted.1 = touched;
            painted.2 = b;
        }
    } else {
        drag.painted = Some((Arc::new(scene), touched, b));
    }
}

/// The surface of what is dragged, if it has one.
fn drag_icon_frame(doc: &Document) -> Option<DragIconFrame> {
    let drag = doc.drag.as_ref().filter(|d| d.icon)?;
    let (_, touched, b) = drag.painted.as_ref()?;
    if touched.is_empty() {
        return None;
    }
    let (logical, buffer) = crate::drag::icon_sizes(touched.size(), doc.scale);
    let hot = drag.hotspot(*b);
    // In the scene the pointer goes where it is relative to what is touched.
    let at = PointI::new(hot.x - touched.x, hot.y - touched.y);
    let layer = drag.layer(at)?;
    let scene = Scene {
        width: buffer.width.max(1) as u32,
        height: buffer.height.max(1) as u32,
        clear: Color::TRANSPARENT,
        primitives: vec![Primitive::Layer(layer)],
        chunks: Vec::new(),
    };
    Some(DragIconFrame {
        scene,
        logical,
        hotspot: crate::drag::logical_point(at, doc.scale),
    })
}

pub fn paint_document(doc: &mut Document, now: Instant) -> Frame {
    let mut old_layers = std::mem::take(&mut doc.layer_contents);
    let mut new_layers = HashMap::new();
    let mut settling = Vec::new();
    paint_drag_preview(doc, now, &mut old_layers, &mut new_layers, &mut settling);
    let doc_ref = &*doc;
    let window = doc_ref.shown_root().map(|root| {
        let size = (doc_ref.viewport.width, doc_ref.viewport.height);
        paint_root(
            doc_ref,
            root,
            size,
            window_clear(doc_ref),
            now,
            &mut old_layers,
            &mut new_layers,
            &mut settling,
        )
    });
    let popups = doc_ref
        .popups
        .iter()
        .filter_map(|&id| {
            let data = doc_ref.nodes[id].popup().filter(|d| d.shown())?;
            let size = (data.surface.buffer.width, data.surface.buffer.height);
            let scene = paint_root(
                doc_ref,
                id,
                size,
                Color::TRANSPARENT,
                now,
                &mut old_layers,
                &mut new_layers,
                &mut settling,
            );
            Some((popup_id(id), scene))
        })
        .collect();
    let drag_icon = drag_icon_frame(doc_ref);
    doc.layer_contents = new_layers;
    doc.images.get_mut().end_frame();
    doc.stale_layers.clear();
    doc.settling = !settling.is_empty();
    for id in settling {
        // They hold it as it was shown, and it is about to be shown differently.
        mark_outer_layers_stale(doc, id);
    }
    Frame {
        window: window.unwrap_or_default(),
        popups,
        drag_icon,
    }
}

/// What shows where nothing else does: behind what is on all of the screen, that is `::backdrop`.
fn window_clear(doc: &Document) -> Color {
    if doc.fullscreen.element.is_some() {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_root(
    doc: &Document,
    root: NodeId,
    size: (i32, i32),
    clear: Color,
    now: Instant,
    old_layers: &mut HashMap<NodeId, LayerMemo>,
    new_layers: &mut HashMap<NodeId, LayerMemo>,
    settling: &mut Vec<NodeId>,
) -> Scene {
    let mut scene = Scene {
        width: size.0.max(0) as u32,
        height: size.1.max(0) as u32,
        clear,
        primitives: Vec::new(),
        chunks: Vec::new(),
    };
    let screen = RectI::new(0, 0, scene.width as i32, scene.height as i32);
    let mut painter = Painter {
        doc,
        scene: &mut scene,
        now,
        screen,
        old_layers: std::mem::take(old_layers),
        new_layers: std::mem::take(new_layers),
        reuse_layers: !doc.dirty.content,
        cull: true,
        settling: Vec::new(),
        magnified: 1.0,
        pictures: 0,
    };
    if doc.nodes[root].is_popup() {
        painter.contents(root, 1.0, screen);
    } else {
        if doc.fullscreen.element == Some(root) {
            // Whatever it is in the document, which isn't showing.
            painter.node_contents(root, 1.0, screen);
        } else {
            painter.node(root, 1.0, screen);
        }
        for &portal in &doc.popups {
            let node = &doc.nodes[portal];
            if node.visible
                && !doc.holds_fullscreen(portal)
                && node
                    .popup()
                    .is_some_and(|p| p.portal && p.preview_of.is_none())
            {
                painter.contents(portal, 1.0, screen);
            }
        }
        if let Some(drag) = doc.drag.as_ref().filter(|d| !d.icon)
            && let Some(layer) = drag.layer(drag.position)
        {
            painter.scene.begin_chunk(ChunkKey {
                node: DRAG_LAYER,
                part: part::LAYER,
            });
            painter.scene.primitives.push(Primitive::Layer(layer));
        }
    }
    settling.append(&mut painter.settling);
    *old_layers = painter.old_layers;
    *new_layers = painter.new_layers;
    scene
}

/// A difference too small for 8 bits looks the same, so it shouldn't cause a repaint.
fn snapped_opacity(opacity: f32) -> f32 {
    (opacity * 255.0).round() / 255.0
}

/// The color of text in an `svg`: glyphs have one color, so a gradient gives them its last one.
fn text_fill(c: &ComputedStyle) -> Color {
    let (color, opacity) = (
        match &c.fill_paint {
            SvgPaint::None => Color::TRANSPARENT,
            SvgPaint::Color(color) => *color,
            SvgPaint::CurrentColor => c.color,
            SvgPaint::Gradient(g) => g.stops.last().map_or(Color::TRANSPARENT, |s| s.color),
        },
        c.fill_opacity,
    );
    fade(color, opacity)
}

/// Whether what a subtree paints can overlap, so that fading each thing on its own would show one
/// through the other. Decided from the tree alone, so that culling has no say in it.
fn paints_overlapping(doc: &Document, id: NodeId) -> bool {
    fn count(doc: &Document, id: NodeId, things: &mut u32) {
        let Some(node) = doc.nodes.get(id).filter(|n| n.visible) else {
            return;
        };
        let c = &node.computed;
        match &node.kind {
            NodeKind::Popup(_) => return,
            // The lines and runs of a text don't overlap, but the backgrounds of its spans are
            // under them.
            NodeKind::Text(t) => {
                *things += 1 + u32::from(t.rich.is_some());
            }
            NodeKind::Edit(_) => *things += 2,
            NodeKind::Fragment | NodeKind::SvgText(_) => {}
            NodeKind::SvgShape(shape) => {
                if shape.geometry.is_some() {
                    *things += u32::from(!matches!(c.fill_paint, SvgPaint::None));
                    *things += u32::from(!matches!(c.stroke_paint, SvgPaint::None));
                }
            }
            NodeKind::Image(_) | NodeKind::Video(_) => {
                *things += 1 + u32::from(!c.background.is_transparent());
                *things += u32::from(c.background_image.is_some());
                *things += c.shadows.len() as u32;
            }
            _ => {
                let b = &node.resolved.border;
                let border =
                    (b.top | b.right | b.bottom | b.left) != 0 && !c.border_color.is_transparent();
                *things += u32::from(!c.background.is_transparent() || border);
                *things += u32::from(c.background_image.is_some());
                *things += c.shadows.len() as u32;
                *things += u32::from(node.fill.is_some()) + u32::from(node.overlay.is_some());
                *things += u32::from(c.backdrop_blur > 0.0);
                *things += u32::from(c.outline_width > 0.0);
                if c.overflow_x == Overflow::Scroll || c.overflow_y == Overflow::Scroll {
                    *things += 2;
                }
            }
        }
        for &child in &node.children {
            if *things > 1 {
                return;
            }
            count(doc, child, things);
        }
    }
    let mut things = 0;
    count(doc, id, &mut things);
    things > 1
}

/// Whether a node is painted through a layer of its own, not counting scrolling.
fn is_layer(doc: &Document, id: NodeId) -> bool {
    let node = &doc.nodes[id];
    node.transformed
        || node.is_filtered()
        || (node.computed.opacity < 1.0 && paints_overlapping(doc, id))
}

/// What a layer's content can touch. A node's bounds may be too big, which would only cost
/// repainting pixels that stay the same.
fn what_it_touches(content: &Scene) -> RectI {
    content
        .primitives
        .iter()
        .fold(RectI::default(), |r, p| r.union(&primitive_bounds(p)))
}

/// The nodes of the layers directly in a scene.
fn nested_layers(scene: &Scene) -> Vec<NodeId> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Layer(l) => Some(NodeId::from(KeyData::from_ffi(l.key))),
            _ => None,
        })
        .collect()
}

/// The radii of the content edge: what the border and the padding leave of the corners of a box.
fn content_radii(node: &Node, scale: crate::geometry::Scale) -> [f32; 4] {
    let b = &node.resolved.border;
    let p = &node.resolved.padding;
    let r = physical_radii(node, scale);
    [
        (r[0] - (b.left + p.left).max(b.top + p.top) as f32).max(0.0),
        (r[1] - (b.right + p.right).max(b.top + p.top) as f32).max(0.0),
        (r[2] - (b.right + p.right).max(b.bottom + p.bottom) as f32).max(0.0),
        (r[3] - (b.left + p.left).max(b.bottom + p.bottom) as f32).max(0.0),
    ]
}

fn physical_radii(node: &Node, scale: crate::geometry::Scale) -> [f32; 4] {
    let r = node.computed.radii().map(|r| scale.pxf(r));
    [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
}

/// Where a length along an axis puts something of size `size` in `free` room: percentages line up
/// that point of both, like `object-position` and `background-position`.
fn placed(at: Length, room: f32, size: f32, scale: crate::geometry::Scale) -> f32 {
    match at {
        Length::Px(v) => scale.pxf(v),
        Length::Percent(p) => (room - size) * p / 100.0,
        Length::Device(v) => v as f32,
        Length::Auto => (room - size) * 0.5,
    }
}

/// Where the whole picture goes for `object-fit` and `object-position`. Rounded like everything
/// else: on the screen, parts of pixels would blur all of it.
fn fitted_rect(
    content: RectI,
    natural: Option<(f32, f32)>,
    fit: ObjectFit,
    position: (Length, Length),
    scale: crate::geometry::Scale,
) -> RectI {
    let Some((nw, nh)) = natural.filter(|n| n.0 > 0.0 && n.1 > 0.0) else {
        return content;
    };
    let (cw, ch) = (content.width as f32, content.height as f32);
    let (w, h) = match fit {
        ObjectFit::Fill => return content,
        ObjectFit::None => (nw, nh),
        ObjectFit::Cover => {
            let k = (cw / nw).max(ch / nh);
            (nw * k, nh * k)
        }
        ObjectFit::Contain => {
            let k = (cw / nw).min(ch / nh);
            (nw * k, nh * k)
        }
        ObjectFit::ScaleDown => {
            let k = (cw / nw).min(ch / nh).min(1.0);
            (nw * k, nh * k)
        }
    };
    let x = content.x as f32 + placed(position.0, cw, w, scale);
    let y = content.y as f32 + placed(position.1, ch, h, scale);
    let (x0, y0) = (x.round() as i32, y.round() as i32);
    RectI::new(
        x0,
        y0,
        (x + w).round() as i32 - x0,
        (y + h).round() as i32 - y0,
    )
}

/// Where one copy of a background goes, for `background-size` and `background-position`, and how
/// big a copy is: copies follow each other at their exact size, as measured in Chrome, while one on
/// its own goes on whole pixels.
fn background_rect(
    area: RectI,
    natural: (f32, f32),
    size: BackgroundSize,
    position: (Length, Length),
    scale: crate::geometry::Scale,
) -> (RectI, (f32, f32)) {
    let (aw, ah) = (area.width as f32, area.height as f32);
    let (nw, nh) = natural;
    if nw <= 0.0 || nh <= 0.0 {
        return (RectI::default(), (0.0, 0.0));
    }
    let side = |l: Length, room: f32| match l {
        Length::Px(v) => Some(scale.pxf(v)),
        Length::Percent(p) => Some(room * p / 100.0),
        Length::Device(v) => Some(v as f32),
        Length::Auto => None,
    };
    let (w, h) = match size {
        BackgroundSize::Auto => (nw, nh),
        BackgroundSize::Cover => {
            let k = (aw / nw).max(ah / nh);
            (nw * k, nh * k)
        }
        BackgroundSize::Contain => {
            let k = (aw / nw).min(ah / nh);
            (nw * k, nh * k)
        }
        BackgroundSize::Size(w, h) => match (side(w, aw), side(h, ah)) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w * nh / nw),
            (None, Some(h)) => (h * nw / nh, h),
            (None, None) => (nw, nh),
        },
    };
    let x = area.x as f32 + placed(position.0, aw, w, scale);
    let y = area.y as f32 + placed(position.1, ah, h, scale);
    let (x0, y0) = (x.round() as i32, y.round() as i32);
    (
        RectI::new(
            x0,
            y0,
            ((x + w).round() as i32 - x0).max(1),
            ((y + h).round() as i32 - y0).max(1),
        ),
        (w, h),
    )
}

/// The coordinates of a variable font on steps of 1/64, so that what moves along an axis doesn't
/// fill the glyph atlas. All at the default is none at all (see `docs/design/17-font-axes.md`).
fn rounded_coords(coords: impl Iterator<Item = i16> + Clone) -> Vec<i16> {
    const STEP: i32 = 1 << 8;
    let round = |c: i16| ((c as i32 + STEP / 2) & !(STEP - 1)) as i16;
    if coords.clone().all(|c| round(c) == 0) {
        return Vec::new();
    }
    coords.map(round).collect()
}

fn fade(c: Color, opacity: f32) -> Color {
    if opacity >= 1.0 {
        c.snapped()
    } else {
        c.with_alpha(c.a * opacity).snapped()
    }
}

/// Paints without culling or reusing anything, as a reference for tests.
#[doc(hidden)]
pub fn paint_everything(doc: &Document, now: Instant) -> Scene {
    let mut scene = Scene {
        width: doc.viewport.width.max(0) as u32,
        height: doc.viewport.height.max(0) as u32,
        clear: window_clear(doc),
        primitives: Vec::new(),
        chunks: Vec::new(),
    };
    let mut painter = Painter {
        doc,
        scene: &mut scene,
        now,
        screen: RectI::new(0, 0, doc.viewport.width, doc.viewport.height),
        old_layers: HashMap::new(),
        new_layers: HashMap::new(),
        reuse_layers: false,
        cull: false,
        settling: Vec::new(),
        magnified: 1.0,
        pictures: 0,
    };
    if let Some(root) = doc.fullscreen.element {
        painter.node_contents(root, 1.0, RectI::INFINITE);
    } else if let Some(root) = doc.root {
        painter.node(root, 1.0, RectI::INFINITE);
    }
    if let Some(drag) = doc.drag.as_ref().filter(|d| !d.icon)
        && let Some(layer) = drag.layer(drag.position)
    {
        scene.primitives.push(Primitive::Layer(layer));
    }
    scene
}

/// Where an edit node's text origin is, in the same coordinates as the node.
pub fn edit_text_origin(doc: &Document, id: NodeId) -> PointI {
    let node = &doc.nodes[id];
    let content = node.content_abs();
    let Some(edit) = node.edit() else {
        return content.origin();
    };
    let mut y = content.y - edit.scroll.y;
    if !edit.multiline {
        let lh = edit.line_height();
        if content.height > lh {
            y += (content.height - lh) / 2;
        }
    }
    PointI::new(content.x - edit.scroll.x, y)
}

/// Caret width in physical pixels.
pub fn caret_width(doc: &Document) -> i32 {
    doc.scale.px_min1(1.0)
}

/// The parts of a node's painting, chunked separately so that a change in one (a blinking caret,
/// a moving selection) does not damage the others.
mod part {
    pub const BOX: u32 = 0;
    pub const SELECTION: u32 = 1;
    pub const CONTENT: u32 = 2;
    pub const CARET: u32 = 3;
    /// Painted after the children: scrollbars and the outline.
    pub const AFTER: u32 = 4;
    pub const LAYER: u32 = 5;
    /// A shader above the box. On its own so that one coming and going doesn't damage the shadow.
    pub const OVERLAY: u32 = 6;
    /// What is behind a node that is a layer, which goes before the layer.
    pub const BACKDROP: u32 = 7;
    /// The picture of an `img`.
    pub const PICTURE: u32 = 8;
}

struct Painter<'a> {
    doc: &'a Document,
    scene: &'a mut Scene,
    now: Instant,
    screen: RectI,
    /// What layers painted in the last frame, and whether it is still right.
    old_layers: HashMap<NodeId, LayerMemo>,
    new_layers: HashMap<NodeId, LayerMemo>,
    reuse_layers: bool,
    cull: bool,
    /// Transformed nodes shown between pixels.
    settling: Vec<NodeId>,
    /// How much bigger what is being painted shows, by the transforms around it.
    magnified: f64,
    /// Counts the pictures painted, to find out whether a layer has any.
    pictures: u32,
}

impl Painter<'_> {
    fn chunk(&mut self, id: NodeId, part: u32) {
        self.scene.begin_chunk(ChunkKey {
            node: id.data().as_ffi(),
            part,
        });
    }

    /// Paints the scroll root's children as a layer.
    fn layer(&mut self, id: NodeId, opacity: f32) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let clip = child_clip(node, doc.scale, node.clip_shape());
        let origin = PointI::new(node.abs.x - node.scroll.x, node.abs.y - node.scroll.y);
        let view = clip
            .rect
            .intersect(&self.screen)
            .translate(-origin.x, -origin.y);
        let stale = self.doc.stale_layers.contains(&id);
        let reused = self
            .old_layers
            .remove(&id)
            .filter(|m| self.reuse_layers && !stale && m.covers.intersect(&view) == view);
        let before = self.pictures;
        let (content, covers) = if let Some(m) = reused {
            self.keep_nested(&m.nested);
            self.pictures += u32::from(m.pictures);
            (m.content, m.covers)
        } else {
            // A screenful around the view, so that scrolling doesn't paint on every frame.
            let (w, h) = (self.screen.width, self.screen.height);
            let covers = RectI::new(
                view.x - w,
                view.y - h,
                view.width + 2 * w,
                view.height + 2 * h,
            );
            let outer = std::mem::take(self.scene);
            self.contents(id, opacity, covers);
            (Arc::new(std::mem::replace(self.scene, outer)), covers)
        };
        self.new_layers.insert(
            id,
            LayerMemo {
                nested: nested_layers(&content),
                content: content.clone(),
                covers,
                touched: RectI::INFINITE,
                transform: Affine::IDENTITY,
                pictures: self.pictures != before,
            },
        );
        self.chunk(id, part::LAYER);
        self.scene.primitives.push(Primitive::Layer(Layer {
            key: id.data().as_ffi(),
            clip,
            origin,
            transform: Affine::IDENTITY,
            bounds: RectI::INFINITE,
            opacity: 1.0,
            content,
            filter: None,
        }));
    }

    /// Paints a transformed node and what is under it as a layer. What can be seen of it through
    /// `cull` decides what gets painted, as a screenful around it does for scrolling.
    fn transformed_layer(&mut self, id: NodeId, opacity: f32, cull: RectI) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let raw = node.layer_matrix(doc.scale);
        let Some(inverse) = raw.inverse() else {
            return;
        };
        let clip = node.layer_clip;
        let bounds = node.subtree_bounds;
        let reach = node.filter_reach(doc.scale);
        let view = if self.cull {
            inverse
                .bounds_of(cull.intersect(&clip.rect))
                .outset(reach)
                .intersect(&bounds)
        } else {
            bounds
        };
        let old = self.old_layers.remove(&id);
        // A translation between pixels is for while it moves: text stays sharp when it stops.
        let stopped = !self.cull || old.as_ref().is_some_and(|m| m.transform == raw);
        let mut transform = raw;
        if raw.is_translation() && !raw.is_whole_translation() {
            if stopped {
                transform.e = raw.e.round();
                transform.f = raw.f.round();
            } else {
                self.settling.push(id);
            }
        }
        let stale = doc.stale_layers.contains(&id);
        let size = raw.max_scale();
        // The pictures in it may have to be there in more detail.
        let grew = old
            .as_ref()
            .is_some_and(|m| m.pictures && size > m.transform.max_scale());
        let reused = old
            .filter(|m| self.reuse_layers && !stale && !grew && m.covers.intersect(&view) == view);
        let before = self.pictures;
        let (content, covers, touched) = if let Some(m) = reused {
            self.keep_nested(&m.nested);
            self.pictures += u32::from(m.pictures);
            (m.content, m.covers, m.touched)
        } else {
            let covers = if self.cull {
                view.outset(self.screen.width.max(self.screen.height))
            } else {
                RectI::INFINITE
            };
            let outer = std::mem::take(self.scene);
            let (around, reuse) = (self.magnified, self.reuse_layers);
            self.magnified *= size;
            self.reuse_layers &= !grew;
            self.node_body(id, 1.0, covers);
            self.magnified = around;
            self.reuse_layers = reuse;
            let content = std::mem::replace(self.scene, outer);
            let touched = what_it_touches(&content);
            (Arc::new(content), covers, touched)
        };
        self.new_layers.insert(
            id,
            LayerMemo {
                nested: nested_layers(&content),
                content: content.clone(),
                covers,
                touched,
                transform: raw,
                pictures: self.pictures != before,
            },
        );
        self.backdrop_of_layer(id, opacity, transform, clip);
        self.chunk(id, part::LAYER);
        let (filter, bounds) = self.layer_filter(node, touched);
        self.scene.primitives.push(Primitive::Layer(Layer {
            key: id.data().as_ffi(),
            clip,
            origin: PointI::default(),
            transform,
            bounds,
            opacity: snapped_opacity(opacity),
            content,
            filter,
        }));
    }

    /// What is inside of a layer stays as it is when the layer does.
    fn keep_nested(&mut self, nested: &[NodeId]) {
        for &id in nested {
            if let Some(m) = self.old_layers.remove(&id) {
                self.keep_nested(&m.nested);
                self.new_layers.insert(id, m);
            }
        }
    }

    /// Paints the children whose bounds touch `cull`.
    fn children(&mut self, node: &Node, opacity: f32, cull: RectI) {
        for (i, &child) in node.children.iter().enumerate() {
            if !self.cull || node.child_bounds.get(i).is_none_or(|b| b.intersects(&cull)) {
                self.node(child, opacity, cull);
            }
        }
    }

    /// Paints what is above the node's background: its children and, for a stacking context, the
    /// stacked descendants around them.
    fn contents(&mut self, id: NodeId, opacity: f32, cull: RectI) {
        let doc = self.doc;
        let (below, above) = doc.stacked_around(id);
        self.stacked(below, opacity, cull);
        self.children(&doc.nodes[id], opacity, cull);
        self.stacked(above, opacity, cull);
    }

    /// A stacked node has only 1 for opacity between it and its stacking context, which is where
    /// `opacity` comes from.
    fn stacked(&mut self, nodes: &[NodeId], opacity: f32, cull: RectI) {
        let doc = self.doc;
        for &id in nodes {
            let Some(n) = doc.nodes.get(id) else {
                continue;
            };
            let cull = cull.intersect(&n.clip);
            if !self.cull || n.bounds_in_parent(doc.scale).intersects(&cull) {
                self.node_contents(id, opacity, cull);
            }
        }
    }

    /// `cull` is the rectangle worth painting, in the same coordinates as the node.
    fn node(&mut self, id: NodeId, opacity: f32, cull: RectI) {
        if self.doc.nodes.get(id).is_some_and(|n| !n.stacked) {
            self.node_contents(id, opacity, cull);
        }
    }

    fn node_contents(&mut self, id: NodeId, opacity: f32, cull: RectI) {
        let doc = self.doc;
        let Some(node) = doc.nodes.get(id) else {
            return;
        };
        if !node.visible {
            return;
        }
        let opacity = opacity * node.computed.opacity;
        if opacity <= 0.0 {
            return;
        }
        if node.transformed {
            self.transformed_layer(id, opacity, cull);
        } else if is_layer(doc, id) {
            self.faded_layer(id, opacity, cull);
        } else {
            self.node_body(id, opacity, cull);
        }
    }

    /// Paints a node as one image, since what is in it would show through itself otherwise. It
    /// stays in the coordinates it is in.
    fn faded_layer(&mut self, id: NodeId, opacity: f32, cull: RectI) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        // What is inside keeps the clips that apply to it, rounded ones too.
        let clip = Clip::from(node.clip);
        let bounds = node.subtree_bounds;
        let reach = node.filter_reach(doc.scale);
        let view = if self.cull {
            cull.intersect(&clip.rect).outset(reach).intersect(&bounds)
        } else {
            bounds
        };
        let stale = doc.stale_layers.contains(&id);
        // Layers inside can change without anything saying so to this one, which holds them.
        let reused = self.old_layers.remove(&id).filter(|m| {
            self.reuse_layers && !stale && m.nested.is_empty() && m.covers.intersect(&view) == view
        });
        let before = self.pictures;
        let (content, covers, touched) = if let Some(m) = reused {
            self.pictures += u32::from(m.pictures);
            (m.content, m.covers, m.touched)
        } else {
            let covers = if self.cull {
                view.outset(self.screen.width.max(self.screen.height))
            } else {
                RectI::INFINITE
            };
            let outer = std::mem::take(self.scene);
            self.node_body(id, 1.0, covers);
            let content = std::mem::replace(self.scene, outer);
            let touched = what_it_touches(&content);
            (Arc::new(content), covers, touched)
        };
        self.new_layers.insert(
            id,
            LayerMemo {
                nested: nested_layers(&content),
                content: content.clone(),
                covers,
                touched,
                transform: Affine::IDENTITY,
                pictures: self.pictures != before,
            },
        );
        self.backdrop_of_layer(id, opacity, Affine::IDENTITY, node.clip_shape());
        self.chunk(id, part::LAYER);
        let (filter, bounds) = self.layer_filter(node, touched);
        self.scene.primitives.push(Primitive::Layer(Layer {
            key: id.data().as_ffi(),
            clip,
            origin: PointI::default(),
            transform: Affine::IDENTITY,
            bounds,
            opacity: snapped_opacity(opacity),
            content,
            filter,
        }));
    }

    /// The filter of a node's layer, and how far the layer can touch with it.
    fn layer_filter(&self, node: &Node, touched: RectI) -> (Option<Box<LayerFilter>>, RectI) {
        let Some(image) = node.filter_image() else {
            return (None, touched);
        };
        let scale = self.doc.scale;
        let reach = image.reach_px(scale.as_f32());
        let b = &node.resolved.border;
        let p = &node.resolved.padding;
        let bounds = if touched.is_empty() {
            touched
        } else {
            touched.outset(reach)
        };
        let filter = LayerFilter {
            image,
            rect: node.abs,
            border: [b.top, b.right, b.bottom, b.left],
            padding: [p.top, p.right, p.bottom, p.left],
            radii: physical_radii(node, scale),
            scale: scale.as_f32(),
            reach,
        };
        (Some(Box::new(filter)), bounds)
    }

    /// What is behind a node that is a layer: outside of the layer, or it wouldn't see it.
    fn backdrop_of_layer(&mut self, id: NodeId, opacity: f32, transform: Affine, clip: Clip) {
        if self.doc.nodes[id].computed.backdrop_blur > 0.0 {
            self.chunk(id, part::BACKDROP);
            self.backdrop(id, opacity, transform, clip);
        }
    }

    fn backdrop(&mut self, id: NodeId, opacity: f32, transform: Affine, clip: Clip) {
        let node = &self.doc.nodes[id];
        let scale = self.doc.scale;
        self.scene
            .primitives
            .push(Primitive::Backdrop(BackdropDraw {
                image: Image::backdrop().blur(node.computed.backdrop_blur),
                rect: node.abs,
                radii: physical_radii(node, scale),
                transform,
                scale: scale.as_f32(),
                opacity: snapped_opacity(opacity),
                clip,
            }));
    }

    /// What the node paints in its own coordinates. `opacity` is what to apply to it.
    fn node_body(&mut self, id: NodeId, opacity: f32, cull: RectI) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let c = &node.computed;
        match &node.kind {
            NodeKind::Fragment => {
                self.children(node, opacity, cull);
                return;
            }
            NodeKind::Popup(_) => return,
            NodeKind::SvgText(_) => {
                self.contents(id, opacity, cull);
                return;
            }
            NodeKind::SvgShape(shape) => {
                self.chunk(id, part::CONTENT);
                let logical = doc.scale.as_f64();
                let scene = &mut self.scene;
                crate::svg::shape_draws(node, shape, opacity, logical, |d| {
                    scene.primitives.push(Primitive::Path(d));
                });
                self.contents(id, opacity, cull);
                return;
            }
            NodeKind::Text(t) => {
                if let Some(rich) = &t.rich {
                    self.chunk(id, part::BOX);
                    self.span_backgrounds(
                        &t.layout,
                        rich,
                        node.abs.origin(),
                        opacity,
                        node.clip_shape(),
                    );
                }
                if let Some(sel) = &doc.state.selection
                    && let Some((a, b)) = node_range(doc, sel, id)
                {
                    self.chunk(id, part::SELECTION);
                    let color = fade(c.selection_color, opacity);
                    for r in t.layout.range_rects(a, b, node.abs.x, node.abs.y) {
                        self.scene.fill(r, color, node.clip_shape());
                    }
                }
                self.chunk(id, part::CONTENT);
                let color = if t.in_svg { text_fill(c) } else { c.color };
                let base = (fade(color, opacity), c.underline, c.strikethrough);
                match &t.rich {
                    Some(rich) => self.text_layout_with(
                        &t.layout,
                        node.abs.origin(),
                        node.clip_shape(),
                        |brush| match brush.checked_sub(1).and_then(|i| rich.paint(i as usize)) {
                            Some(p) => (fade(p.color, opacity), p.underline, p.strikethrough),
                            None => base,
                        },
                    ),
                    None => self.text_layout(
                        &t.layout,
                        node.abs.origin(),
                        base.0,
                        node.clip_shape(),
                        base.1,
                        base.2,
                    ),
                }
                if !node.children.is_empty() {
                    self.contents(id, opacity, cull);
                }
                return;
            }
            _ => {}
        }

        self.chunk(id, part::BOX);
        self.box_decorations(id, opacity);

        if let NodeKind::Edit(_) = &node.kind {
            self.edit(id, opacity);
        }
        if let NodeKind::Image(_) = &node.kind {
            self.chunk(id, part::PICTURE);
            self.picture(id, opacity);
        }
        if let NodeKind::Video(_) = &node.kind {
            self.chunk(id, part::PICTURE);
            self.video(id, opacity);
        }

        if node.scroll_root {
            self.layer(id, opacity);
        } else {
            let inner = child_clip(node, doc.scale, node.clip_shape());
            self.contents(id, opacity, cull.intersect(&inner.rect));
        }

        self.chunk(id, part::AFTER);
        self.scrollbars(id, opacity);
        self.outline(id, opacity);
    }

    fn box_decorations(&mut self, id: NodeId, opacity: f32) {
        let node = &self.doc.nodes[id];
        let clip = node.clip_shape();
        let c = &node.computed;
        let scale = self.doc.scale;
        let radii = physical_radii(node, scale);
        // Before the node's own shadows: they aren't behind it. A layer has it outside.
        if c.backdrop_blur > 0.0 && !is_layer(self.doc, id) {
            self.backdrop(id, opacity, Affine::IDENTITY, clip);
        }
        let node = &self.doc.nodes[id];
        let c = &node.computed;
        let b = &node.resolved.border;
        let has_border =
            (b.top | b.right | b.bottom | b.left) != 0 && !c.border_color.is_transparent();
        let background = fade(c.background, opacity);
        // A rounded edge where shadow and box each cover part of a pixel would let what is behind
        // show through. An opaque background hides a shadow that goes a little under it.
        let rounded = radii.iter().any(|r| *r > 0.0);
        let under = if rounded && background.a >= 1.0 {
            scale.px_min1(1.0)
        } else {
            0
        };
        let cut = node.abs.inset(under, under, under, under);
        let cut_radii = radii.map(|r| (r - under as f32).max(0.0));
        // The first shadow is on top.
        for s in c.shadows.iter().rev().filter(|s| !s.inset) {
            let spread = scale.px(s.spread);
            let rect = node
                .abs
                .translate(scale.px(s.offset_x), scale.px(s.offset_y))
                .outset(spread);
            if rect.is_empty() {
                continue;
            }
            self.scene.primitives.push(Primitive::Shadow(Shadow {
                rect,
                radii: radii.map(|r| {
                    if r > 0.0 {
                        (r + spread as f32).max(0.0)
                    } else {
                        0.0
                    }
                }),
                blur: scale.pxf(s.blur),
                color: fade(s.color, opacity),
                clip,
                cut,
                cut_radii,
                inset: false,
            }));
        }
        if let Some(paint) = &node.fill {
            let draw = self.shader_draw(node, radii, paint, opacity);
            self.scene.primitives.push(draw);
        }
        let picture = c
            .background_image
            .as_ref()
            .and_then(|source| self.background_picture(id, source, radii, clip, opacity));
        // A picture goes between the color and the border.
        let together = picture.is_none();
        if !c.background.is_transparent() || (has_border && together) {
            self.scene.primitives.push(Primitive::Quad(Quad {
                rect: node.abs,
                background,
                border_widths: if has_border && together {
                    [b.top, b.right, b.bottom, b.left]
                } else {
                    [0; 4]
                },
                border_color: fade(c.border_color, opacity),
                radii,
                clip,
            }));
        }
        if let Some(picture) = picture {
            self.scene.primitives.push(Primitive::Image(picture));
            if has_border {
                self.scene.primitives.push(Primitive::Quad(Quad {
                    rect: node.abs,
                    background: Color::TRANSPARENT,
                    border_widths: [b.top, b.right, b.bottom, b.left],
                    border_color: fade(c.border_color, opacity),
                    radii,
                    clip,
                }));
            }
        }
        if c.shadows.iter().any(|s| s.inset) {
            let pad = node.padding_abs();
            let inner = [
                (radii[0] - b.left.max(b.top) as f32).max(0.0),
                (radii[1] - b.right.max(b.top) as f32).max(0.0),
                (radii[2] - b.right.max(b.bottom) as f32).max(0.0),
                (radii[3] - b.left.max(b.bottom) as f32).max(0.0),
            ];
            for s in c.shadows.iter().rev().filter(|s| s.inset) {
                let spread = scale.px(s.spread);
                let shape = pad
                    .translate(scale.px(s.offset_x), scale.px(s.offset_y))
                    .outset(-spread);
                self.scene.primitives.push(Primitive::Shadow(Shadow {
                    rect: RectI::new(shape.x, shape.y, shape.width.max(0), shape.height.max(0)),
                    radii: inner.map(|r| {
                        if r > 0.0 {
                            (r - spread as f32).max(0.0)
                        } else {
                            0.0
                        }
                    }),
                    blur: scale.pxf(s.blur),
                    color: fade(s.color, opacity),
                    clip,
                    cut: pad,
                    cut_radii: inner,
                    inset: true,
                }));
            }
        }
        if let Some(paint) = &node.overlay {
            let draw = self.shader_draw(node, radii, paint, opacity);
            self.chunk(id, part::OVERLAY);
            self.scene.primitives.push(draw);
        }
    }

    /// The picture of an `img`, in its content box.
    fn picture(&mut self, id: NodeId, opacity: f32) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let Some(data) = node.image() else {
            return;
        };
        let c = &node.computed;
        let scale = doc.scale;
        let content = node.content_abs();
        if content.is_empty() {
            return;
        }
        let natural = data
            .natural
            .map(|(w, h)| (scale.pxf(w / data.density), scale.pxf(h / data.density)));
        let rect = fitted_rect(content, natural, c.object_fit, c.object_position, scale);
        let pixelated = c.image_rendering == ImageRendering::Pixelated;
        let showing = data.source.as_ref().and_then(|source| {
            doc.images
                .borrow_mut()
                .want(id, source, self.wanted(rect), pixelated)
        });
        let (bitmap, frame) = match (showing, &data.fallback) {
            (Some(s), _) => {
                if !data.told.replace(true) {
                    doc.images.borrow_mut().tell_decoded(id);
                }
                (s.bitmap, s.frame)
            }
            (None, Some(old)) => (old.clone(), 0),
            (None, None) => return,
        };
        self.pictures += 1;
        let radii = content_radii(node, scale);
        self.scene.primitives.push(Primitive::Image(ImageDraw {
            bitmap,
            frame,
            rect,
            tile: (rect.width as f32, rect.height as f32),
            repeat: (false, false),
            shape: Clip {
                rect: content,
                radii,
            },
            clip: node.clip_shape(),
            opacity: snapped_opacity(opacity * c.image_opacity),
            pixelated,
        }));
    }

    /// The frame of a `video` that is due, in its content box.
    fn video(&mut self, id: NodeId, opacity: f32) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let Some(data) = node.video() else {
            return;
        };
        let c = &node.computed;
        let scale = doc.scale;
        let content = node.content_abs();
        if content.is_empty() {
            return;
        }
        let natural = data.natural.map(|(w, h)| (scale.pxf(w), scale.pxf(h)));
        let rect = fitted_rect(content, natural, c.object_fit, c.object_position, scale);
        let Some(frame) = doc.videos.borrow_mut().frame(id, self.now) else {
            return;
        };
        self.scene.primitives.push(Primitive::Video(VideoDraw {
            frame,
            rect,
            shape: Clip {
                rect: content,
                radii: content_radii(node, scale),
            },
            clip: node.clip_shape(),
            opacity: snapped_opacity(opacity * c.image_opacity),
        }));
    }

    /// The physical pixels a rectangle takes where it shows in the end.
    fn wanted(&self, rect: RectI) -> (u32, u32) {
        let k = self.magnified;
        (
            (rect.width.max(0) as f64 * k).ceil() as u32,
            (rect.height.max(0) as f64 * k).ceil() as u32,
        )
    }

    /// The picture of a node's background, if it is there to show.
    fn background_picture(
        &mut self,
        id: NodeId,
        source: &anti_webui_style::ImageSource,
        radii: [f32; 4],
        clip: Clip,
        opacity: f32,
    ) -> Option<ImageDraw> {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let c = &node.computed;
        let scale = doc.scale;
        let info = doc.images.borrow_mut().info(id, source)?;
        let natural = (scale.pxf(info.natural.0), scale.pxf(info.natural.1));
        let area = node.padding_abs();
        let (rect, size) = background_rect(
            area,
            natural,
            c.background_size,
            c.background_position,
            scale,
        );
        if rect.is_empty() {
            return None;
        }
        let pixelated = c.image_rendering == ImageRendering::Pixelated;
        let wanted = self.wanted(rect);
        let showing = doc
            .images
            .borrow_mut()
            .want(id, source, wanted, pixelated)?;
        self.pictures += 1;
        let repeat = match c.background_repeat {
            BackgroundRepeat::Repeat => (true, true),
            BackgroundRepeat::RepeatX => (true, false),
            BackgroundRepeat::RepeatY => (false, true),
            BackgroundRepeat::NoRepeat => (false, false),
        };
        Some(ImageDraw {
            bitmap: showing.bitmap,
            frame: showing.frame,
            rect,
            tile: (
                if repeat.0 { size.0 } else { rect.width as f32 },
                if repeat.1 { size.1 } else { rect.height as f32 },
            ),
            repeat,
            shape: Clip {
                rect: node.abs,
                radii,
            },
            clip,
            opacity: snapped_opacity(opacity),
            pixelated,
        })
    }

    /// What it takes to paint the box of `node` with a shader.
    fn shader_draw(
        &self,
        node: &Node,
        radii: [f32; 4],
        paint: &ShaderPaint,
        opacity: f32,
    ) -> Primitive {
        let b = &node.resolved.border;
        let p = &node.resolved.padding;
        Primitive::Shader(ShaderDraw {
            paint: paint.clone(),
            rect: node.abs,
            border: [b.top, b.right, b.bottom, b.left],
            padding: [p.top, p.right, p.bottom, p.left],
            radii,
            scale: self.doc.scale.as_f32(),
            opacity: snapped_opacity(opacity),
            clip: node.clip_shape(),
        })
    }

    fn outline(&mut self, id: NodeId, opacity: f32) {
        let node = &self.doc.nodes[id];
        let clip = node.clip_shape();
        let c = &node.computed;
        if c.outline_width <= 0.0 || c.outline_color.is_transparent() {
            return;
        }
        let scale = self.doc.scale;
        let w = scale.px_min1(c.outline_width);
        let offset = scale.px(c.outline_offset);
        let rect = node.abs.outset(offset + w);
        let grow = (offset + w) as f32;
        let radii = c
            .radii()
            .map(|r| if r > 0.0 { scale.pxf(r) + grow } else { 0.0 });
        self.scene.primitives.push(Primitive::Quad(Quad {
            rect,
            background: Color::TRANSPARENT,
            border_widths: [w; 4],
            border_color: fade(c.outline_color, opacity),
            radii: [
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ],
            clip,
        }));
    }

    fn scrollbars(&mut self, id: NodeId, opacity: f32) {
        let node = &self.doc.nodes[id];
        let clip = node.clip_shape();
        let c = &node.computed;
        let scale = self.doc.scale;
        let pad = node.padding_abs();
        let thickness = scale.px_min1(SCROLLBAR_WIDTH);
        let inset = scale.px(2.0);
        let color = fade(c.scrollbar_color, opacity);
        if c.overflow_y == Overflow::Scroll
            && node.content_size.height > pad.height
            && pad.height > 0
        {
            let track = pad.height - inset * 2;
            let thumb =
                ((track as i64 * pad.height as i64) / node.content_size.height as i64) as i32;
            let thumb = thumb.max(scale.px(24.0)).min(track);
            let max_scroll = node.content_size.height - pad.height;
            let y = pad.y
                + inset
                + ((track - thumb) as i64 * node.scroll.y as i64 / max_scroll.max(1) as i64) as i32;
            let rect = RectI::new(pad.right() - thickness - inset, y, thickness, thumb);
            self.scene.primitives.push(Primitive::Quad(Quad {
                rect,
                background: color,
                border_widths: [0; 4],
                border_color: Color::TRANSPARENT,
                radii: [thickness as f32 / 2.0; 4],
                clip,
            }));
        }
        if c.overflow_x == Overflow::Scroll && node.content_size.width > pad.width && pad.width > 0
        {
            let track = pad.width - inset * 2;
            let thumb = ((track as i64 * pad.width as i64) / node.content_size.width as i64) as i32;
            let thumb = thumb.max(scale.px(24.0)).min(track);
            let max_scroll = node.content_size.width - pad.width;
            let x = pad.x
                + inset
                + ((track - thumb) as i64 * node.scroll.x as i64 / max_scroll.max(1) as i64) as i32;
            let rect = RectI::new(x, pad.bottom() - thickness - inset, thumb, thickness);
            self.scene.primitives.push(Primitive::Quad(Quad {
                rect,
                background: color,
                border_widths: [0; 4],
                border_color: Color::TRANSPARENT,
                radii: [thickness as f32 / 2.0; 4],
                clip,
            }));
        }
    }

    fn edit(&mut self, id: NodeId, opacity: f32) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let c = &node.computed;
        let edit = node.edit().unwrap();
        let clip = child_clip(node, doc.scale, node.clip_shape());
        let origin = edit_text_origin(doc, id);
        let focused = doc.state.focused == Some(id);

        if edit.has_selection() && edit.preedit.is_none() {
            self.chunk(id, part::SELECTION);
            let (a, b) = edit.selection_range();
            let color = if focused && doc.state.window_focused {
                c.selection_color
            } else {
                c.selection_color.with_alpha(c.selection_color.a * 0.5)
            };
            for r in edit.layout.range_rects(a, b, origin.x, origin.y) {
                self.scene.fill(r, fade(color, opacity), clip);
            }
        }

        self.chunk(id, part::CONTENT);
        if edit.text.is_empty() && edit.preedit.is_none() && !edit.placeholder.is_empty() {
            let color = c.color.with_alpha(c.color.a * 0.45);
            self.text_layout(
                &edit.placeholder_layout,
                origin,
                fade(color, opacity),
                clip,
                false,
                false,
            );
        } else {
            self.text_layout(
                &edit.layout,
                origin,
                fade(c.color, opacity),
                clip,
                c.underline,
                c.strikethrough,
            );
        }

        // Pre-edit: underline the composition, thicker under the IME's active segment.
        if let Some(pre) = &edit.preedit {
            let start = edit.focus;
            let end = start + pre.text.len();
            let thin = self.doc.scale.px_min1(1.0);
            for r in edit.layout.range_rects(start, end, origin.x, origin.y) {
                let line = RectI::new(r.x, r.bottom() - thin * 2, r.width, thin);
                self.scene.fill(line, fade(c.color, opacity), clip);
            }
            if let Some((a, b)) = pre.cursor
                && a != b
            {
                for r in edit
                    .layout
                    .range_rects(start + a, start + b, origin.x, origin.y)
                {
                    let line = RectI::new(r.x, r.bottom() - thin * 3, r.width, thin * 2);
                    self.scene.fill(line, fade(c.color, opacity), clip);
                }
            }
        }

        if let Some(at) = edit.drop_caret {
            self.chunk(id, part::CARET);
            let caret = edit
                .drop_caret_rect(at, caret_width(doc))
                .translate(origin.x, origin.y);
            self.scene
                .fill(caret, fade(c.caret_color.unwrap_or(c.color), opacity), clip);
        } else if focused && doc.state.window_focused && !edit.read_only {
            let elapsed = self.now.saturating_duration_since(edit.caret_epoch);
            let on = (elapsed.as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2);
            if on || edit.preedit.is_some() {
                self.chunk(id, part::CARET);
                let caret = edit
                    .caret_rect(caret_width(doc))
                    .translate(origin.x, origin.y);
                self.scene
                    .fill(caret, fade(c.caret_color.unwrap_or(c.color), opacity), clip);
            }
        }
    }

    fn text_layout(
        &mut self,
        layout: &TextLayout,
        origin: PointI,
        color: Color,
        clip: Clip,
        underline: bool,
        strikethrough: bool,
    ) {
        self.text_layout_with(layout, origin, clip, |_| (color, underline, strikethrough));
    }

    /// Paints the runs of a text with the color and lines `style` gives for their brush.
    fn text_layout_with(
        &mut self,
        layout: &TextLayout,
        origin: PointI,
        clip: Clip,
        style: impl Fn(crate::text::Brush) -> (Color, bool, bool),
    ) {
        let (ox, oy) = (origin.x as f32, origin.y as f32);
        for line in layout.layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let (color, underline, strikethrough) = style(glyph_run.style().brush);
                self.glyph_run(&glyph_run, ox, oy, color, clip);
                let run = glyph_run.run();
                let m = run.font_metrics();
                let x0 = (ox + glyph_run.offset()).round() as i32;
                let x1 = (ox + glyph_run.offset() + glyph_run.advance()).round() as i32;
                let baseline = (oy + glyph_run.baseline()).round() as i32;
                let thickness = m.underline_size.round().max(1.0) as i32;
                if underline {
                    let y = baseline - m.underline_offset.round() as i32;
                    self.scene
                        .fill(RectI::new(x0, y, x1 - x0, thickness), color, clip);
                }
                if strikethrough {
                    let t = m.strikethrough_size.round().max(1.0) as i32;
                    let y = baseline - m.strikethrough_offset.round() as i32;
                    self.scene.fill(RectI::new(x0, y, x1 - x0, t), color, clip);
                }
            }
        }
    }

    /// The backgrounds of the spans of a text, one box per line a span is on. They are as tall
    /// as the font, not as the line, like the backgrounds of inline elements in CSS.
    fn span_backgrounds(
        &mut self,
        layout: &TextLayout,
        rich: &crate::text::rich::RichData,
        origin: PointI,
        opacity: f32,
        clip: Clip,
    ) {
        let (ox, oy) = (origin.x as f32, origin.y as f32);
        let painted = |i: usize| {
            rich.paint(i)
                .filter(|p| !p.background.is_transparent())
                .is_some()
        };
        if !(0..rich.spans.len()).any(painted) {
            return;
        }
        // Span → left, right, top and bottom on the line.
        let mut boxes: Vec<(usize, [f32; 4])> = Vec::new();
        for line in layout.layout.lines() {
            boxes.clear();
            let mut grow = |i: usize, x0: f32, x1: f32, y: Option<(f32, f32)>| {
                let at = match boxes.iter().position(|(s, _)| *s == i) {
                    Some(at) => at,
                    None => {
                        boxes.push((i, [f32::MAX, f32::MIN, f32::MAX, f32::MIN]));
                        boxes.len() - 1
                    }
                };
                let b = &mut boxes[at].1;
                b[0] = b[0].min(x0);
                b[1] = b[1].max(x1);
                if let Some((y0, y1)) = y {
                    b[2] = b[2].min(y0);
                    b[3] = b[3].max(y1);
                }
            };
            for item in line.items() {
                match item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        let Some(i) = run.style().brush.checked_sub(1).map(|i| i as usize) else {
                            continue;
                        };
                        if !painted(i) {
                            continue;
                        }
                        let m = run.run().font_metrics();
                        let baseline = oy + run.baseline();
                        let x0 = ox + run.offset();
                        grow(
                            i,
                            x0,
                            x0 + run.advance(),
                            Some((baseline - m.ascent, baseline + m.descent)),
                        );
                    }
                    PositionedLayoutItem::InlineBox(b) => {
                        if let Some((i, _)) = crate::text::pad_of(b.id)
                            && painted(i)
                        {
                            grow(i, ox + b.x, ox + b.x + b.width, None);
                        }
                    }
                }
            }
            for (i, [x0, x1, y0, y1]) in boxes.iter().copied() {
                // Padding alone on a line, with no letters to say how tall it is.
                if y0 > y1 {
                    continue;
                }
                let Some(p) = rich.paint(i) else { continue };
                let rect = RectI::new(
                    x0.round() as i32,
                    y0.round() as i32,
                    (x1.round() - x0.round()) as i32,
                    (y1.round() - y0.round()) as i32,
                );
                self.scene.primitives.push(Primitive::Quad(Quad {
                    rect,
                    background: fade(p.background, opacity),
                    border_widths: [0; 4],
                    border_color: Color::TRANSPARENT,
                    radii: p.radii,
                    clip,
                }));
            }
        }
    }

    fn glyph_run(
        &mut self,
        glyph_run: &parley::GlyphRun<'_, Brush>,
        ox: f32,
        oy: f32,
        color: Color,
        clip: Clip,
    ) {
        let run = glyph_run.run();
        let synthesis = run.synthesis();
        let glyphs: Vec<Glyph> = glyph_run
            .positioned_glyphs()
            .map(|g| Glyph {
                id: g.id,
                x: ox + g.x,
                y: (oy + g.y).round(),
            })
            .collect();
        if glyphs.is_empty() {
            return;
        }
        self.scene.primitives.push(Primitive::Glyphs(GlyphRun {
            font: run.font().clone(),
            size: run.font_size(),
            normalized_coords: rounded_coords(run.normalized_coords().iter().map(|c| c.to_bits())),
            embolden: synthesis.embolden(),
            skew: synthesis.skew(),
            color,
            glyphs,
            clip,
        }));
    }
}
