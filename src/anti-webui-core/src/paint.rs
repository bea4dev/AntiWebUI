//! Turns the laid-out tree into a [`Scene`].

use crate::damage::primitive_bounds;
use crate::geometry::Affine;
use crate::geometry::{PointI, RectI};
use crate::layout::{child_clip, mark_outer_layers_stale};
use crate::platform::PopupId;
use crate::scene::{
    BackdropDraw, ChunkKey, Clip, Glyph, GlyphRun, Layer, LayerFilter, Primitive, Quad, Scene,
    ShaderDraw, Shadow,
};
use crate::shader::{Image, ShaderPaint};
use crate::text::selection::node_range;
use crate::text::{Brush, TextLayout};
use crate::tree::{Document, LayerMemo, Node, NodeId, NodeKind, popup_id};
use anti_webui_style::{Color, Overflow};
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
}

pub fn paint_document(doc: &mut Document, now: Instant) -> Frame {
    let mut old_layers = std::mem::take(&mut doc.layer_contents);
    let mut new_layers = HashMap::new();
    let mut settling = Vec::new();
    let doc_ref = &*doc;
    let window = doc_ref.root.map(|root| {
        let size = (doc_ref.viewport.width, doc_ref.viewport.height);
        paint_root(
            doc_ref,
            root,
            size,
            Color::WHITE,
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
    doc.layer_contents = new_layers;
    doc.stale_layers.clear();
    doc.settling = !settling.is_empty();
    for id in settling {
        // They hold it as it was shown, and it is about to be shown differently.
        mark_outer_layers_stale(doc, id);
    }
    Frame {
        window: window.unwrap_or_default(),
        popups,
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
    };
    if doc.nodes[root].is_popup() {
        painter.contents(root, 1.0, screen);
    } else {
        painter.node(root, 1.0, screen);
        for &portal in &doc.popups {
            let node = &doc.nodes[portal];
            if node.visible && node.popup().is_some_and(|p| p.portal) {
                painter.contents(portal, 1.0, screen);
            }
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
            // The lines and runs of a text don't overlap.
            NodeKind::Text(_) => {
                *things += 1;
                return;
            }
            NodeKind::Edit(_) => *things += 2,
            NodeKind::Fragment => {}
            _ => {
                let b = &node.resolved.border;
                let border =
                    (b.top | b.right | b.bottom | b.left) != 0 && !c.border_color.is_transparent();
                *things += u32::from(!c.background.is_transparent() || border);
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

fn physical_radii(node: &Node, scale: crate::geometry::Scale) -> [f32; 4] {
    let r = node.computed.radii().map(|r| scale.pxf(r));
    [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
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
        clear: Color::WHITE,
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
    };
    if let Some(root) = doc.root {
        painter.node(root, 1.0, RectI::INFINITE);
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
        let (content, covers) = if let Some(m) = reused {
            self.keep_nested(&m.nested);
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
        let reused =
            old.filter(|m| self.reuse_layers && !stale && m.covers.intersect(&view) == view);
        let (content, covers, touched) = if let Some(m) = reused {
            self.keep_nested(&m.nested);
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
                transform: raw,
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
        let (content, covers, touched) = if let Some(m) = reused {
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
            NodeKind::Text(t) => {
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
                self.text_layout(
                    &t.layout,
                    node.abs.origin(),
                    fade(c.color, opacity),
                    node.clip_shape(),
                    c.underline,
                    c.strikethrough,
                );
                return;
            }
            _ => {}
        }

        self.chunk(id, part::BOX);
        self.box_decorations(id, opacity);

        if let NodeKind::Edit(_) = &node.kind {
            self.edit(id, opacity);
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
        if !c.background.is_transparent() || has_border {
            self.scene.primitives.push(Primitive::Quad(Quad {
                rect: node.abs,
                background,
                border_widths: if has_border {
                    [b.top, b.right, b.bottom, b.left]
                } else {
                    [0; 4]
                },
                border_color: fade(c.border_color, opacity),
                radii,
                clip,
            }));
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
        let color = fade(Color::from_rgba(0x00000055), opacity);
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

        if focused && doc.state.window_focused && !edit.read_only {
            let elapsed = self.now.saturating_duration_since(edit.caret_epoch);
            let on = (elapsed.as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2);
            if on || edit.preedit.is_some() {
                self.chunk(id, part::CARET);
                let caret = edit
                    .caret_rect(caret_width(doc))
                    .translate(origin.x, origin.y);
                self.scene.fill(caret, fade(c.caret_color, opacity), clip);
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
        let (ox, oy) = (origin.x as f32, origin.y as f32);
        for line in layout.layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
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
