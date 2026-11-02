//! Turns the laid-out tree into a [`Scene`].

use crate::geometry::{PointI, RectI};
use crate::layout::child_clip;
use crate::scene::{ChunkKey, Clip, Glyph, GlyphRun, Primitive, Quad, Scene, ScrollLayer, Shadow};
use crate::text::selection::node_range;
use crate::text::{Brush, TextLayout};
use crate::tree::{Document, Node, NodeId, NodeKind};
use anti_webui_style::{Color, Overflow};
use parley::PositionedLayoutItem;
use slotmap::Key;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const CARET_BLINK: Duration = Duration::from_millis(530);

/// Width of scrollbar thumbs in logical pixels.
const SCROLLBAR_WIDTH: f32 = 6.0;

pub fn paint_document(doc: &mut Document, now: Instant) -> Scene {
    let mut scene = Scene {
        width: doc.viewport.width.max(0) as u32,
        height: doc.viewport.height.max(0) as u32,
        clear: Color::WHITE,
        primitives: Vec::new(),
        chunks: Vec::new(),
    };
    let old_layers = std::mem::take(&mut doc.layer_contents);
    let doc_ref = &*doc;
    let mut painter = Painter {
        doc: doc_ref,
        scene: &mut scene,
        now,
        offset: PointI::default(),
        in_layer: false,
        old_layers,
        new_layers: HashMap::new(),
        reuse_layers: !doc_ref.dirty.content,
    };
    if let Some(root) = doc_ref.root {
        painter.node(root, 1.0, Clip::NONE);
    }
    let new_layers = painter.new_layers;
    doc.layer_contents = new_layers;
    scene
}

/// Whether the node's content is painted into a scroll layer: it scrolls and overflows.
fn scrolls_content(node: &Node) -> bool {
    if node.is_edit() {
        return false;
    }
    let c = &node.computed;
    let pad = node.padding_abs();
    (c.overflow_y == Overflow::Scroll && node.content_size.height > pad.height)
        || (c.overflow_x == Overflow::Scroll && node.content_size.width > pad.width)
}

fn fade(c: Color, opacity: f32) -> Color {
    if opacity >= 1.0 {
        c
    } else {
        c.with_alpha(c.a * opacity)
    }
}

/// Where an edit node's text origin is, in window coordinates.
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
}

struct Painter<'a> {
    doc: &'a Document,
    scene: &'a mut Scene,
    now: Instant,
    /// Added to window coordinates to get the scene's: non-zero while painting a layer's content.
    offset: PointI,
    in_layer: bool,
    /// Layer contents of the last frame, and whether they are still right.
    old_layers: HashMap<NodeId, Arc<Scene>>,
    new_layers: HashMap<NodeId, Arc<Scene>>,
    reuse_layers: bool,
}

impl Painter<'_> {
    fn chunk(&mut self, id: NodeId, part: u32) {
        self.scene.begin_chunk(ChunkKey {
            node: id.data().as_ffi(),
            part,
        });
    }

    fn to_scene(&self, rect: RectI) -> RectI {
        rect.translate(self.offset.x, self.offset.y)
    }

    fn clip_to_scene(&self, clip: Clip) -> Clip {
        clip.translated(self.offset.x, self.offset.y)
    }

    /// Fills a rectangle given in window coordinates.
    fn fill(&mut self, rect: RectI, color: Color, clip: Clip) {
        let (rect, clip) = (self.to_scene(rect), self.clip_to_scene(clip));
        self.scene.fill(rect, color, clip);
    }

    /// Paints the node's children as a layer; `clip` is where they show.
    fn layer(&mut self, id: NodeId, opacity: f32, clip: Clip) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let origin = PointI::new(node.abs.x - node.scroll.x, node.abs.y - node.scroll.y);
        let reused = self
            .reuse_layers
            .then(|| self.old_layers.remove(&id))
            .flatten();
        let content = reused.unwrap_or_else(|| {
            let outer = std::mem::take(self.scene);
            self.offset = PointI::new(-origin.x, -origin.y);
            self.in_layer = true;
            for &child in &node.children {
                self.node(child, opacity, Clip::NONE);
            }
            self.in_layer = false;
            self.offset = PointI::default();
            Arc::new(std::mem::replace(self.scene, outer))
        });
        self.new_layers.insert(id, content.clone());
        self.chunk(id, part::LAYER);
        self.scene.primitives.push(Primitive::Layer(ScrollLayer {
            key: id.data().as_ffi(),
            clip,
            origin,
            content,
        }));
    }

    /// `clip` is what the parent clips the node to, in window coordinates.
    fn node(&mut self, id: NodeId, opacity: f32, clip: Clip) {
        let doc = self.doc;
        let Some(node) = doc.nodes.get(id) else {
            return;
        };
        if !node.visible {
            return;
        }
        let c = &node.computed;
        let opacity = opacity * c.opacity;
        if opacity <= 0.0 {
            return;
        }
        match &node.kind {
            NodeKind::Fragment => {
                let inner = child_clip(node, doc.scale, clip);
                for &child in &node.children {
                    self.node(child, opacity, inner);
                }
                return;
            }
            NodeKind::Text(t) => {
                if let Some(sel) = &doc.state.selection
                    && let Some((a, b)) = node_range(doc, sel, id)
                {
                    self.chunk(id, part::SELECTION);
                    let color = fade(c.selection_color, opacity);
                    for r in t.layout.range_rects(a, b, node.abs.x, node.abs.y) {
                        self.fill(r, color, clip);
                    }
                }
                self.chunk(id, part::CONTENT);
                self.text_layout(
                    &t.layout,
                    node.abs.origin(),
                    fade(c.color, opacity),
                    clip,
                    c.underline,
                    c.strikethrough,
                );
                return;
            }
            _ => {}
        }

        self.chunk(id, part::BOX);
        self.box_decorations(id, opacity, clip);

        if let NodeKind::Edit(_) = &node.kind {
            self.edit(id, opacity, clip);
        }

        let inner = child_clip(node, doc.scale, clip);
        if !self.in_layer && scrolls_content(node) {
            self.layer(id, opacity, inner);
        } else {
            for &child in &node.children {
                self.node(child, opacity, inner);
            }
        }

        self.chunk(id, part::AFTER);
        self.scrollbars(id, opacity, clip);
        self.outline(id, opacity, clip);
    }

    fn box_decorations(&mut self, id: NodeId, opacity: f32, clip: Clip) {
        let clip = self.clip_to_scene(clip);
        let node = &self.doc.nodes[id];
        let c = &node.computed;
        let scale = self.doc.scale;
        let radii = c.radii().map(|r| scale.pxf(r));
        let radii = [
            radii.top_left,
            radii.top_right,
            radii.bottom_right,
            radii.bottom_left,
        ];
        for s in &c.shadows {
            let spread = scale.px(s.spread);
            let rect = node
                .abs
                .translate(scale.px(s.offset_x), scale.px(s.offset_y))
                .outset(spread);
            let rect = self.to_scene(rect);
            self.scene.primitives.push(Primitive::Shadow(Shadow {
                rect,
                radii: radii.map(|r| if r > 0.0 { r + spread as f32 } else { r }),
                blur: scale.pxf(s.blur),
                color: fade(s.color, opacity),
                clip,
            }));
        }
        let b = &node.resolved.border;
        let has_border =
            (b.top | b.right | b.bottom | b.left) != 0 && !c.border_color.is_transparent();
        if c.background.is_transparent() && !has_border {
            return;
        }
        self.scene.primitives.push(Primitive::Quad(Quad {
            rect: self.to_scene(node.abs),
            background: fade(c.background, opacity),
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

    fn outline(&mut self, id: NodeId, opacity: f32, clip: Clip) {
        let clip = self.clip_to_scene(clip);
        let node = &self.doc.nodes[id];
        let c = &node.computed;
        if c.outline_width <= 0.0 || c.outline_color.is_transparent() {
            return;
        }
        let scale = self.doc.scale;
        let w = scale.px_min1(c.outline_width);
        let offset = scale.px(c.outline_offset);
        let rect = self.to_scene(node.abs.outset(offset + w));
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

    fn scrollbars(&mut self, id: NodeId, opacity: f32, clip: Clip) {
        let clip = self.clip_to_scene(clip);
        let node = &self.doc.nodes[id];
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
            let rect = self.to_scene(RectI::new(
                pad.right() - thickness - inset,
                y,
                thickness,
                thumb,
            ));
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
            let rect = self.to_scene(RectI::new(
                x,
                pad.bottom() - thickness - inset,
                thumb,
                thickness,
            ));
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

    fn edit(&mut self, id: NodeId, opacity: f32, clip: Clip) {
        let doc = self.doc;
        let node = &doc.nodes[id];
        let c = &node.computed;
        let edit = node.edit().unwrap();
        let clip = child_clip(node, doc.scale, clip);
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
                self.fill(r, fade(color, opacity), clip);
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
                self.fill(line, fade(c.color, opacity), clip);
            }
            if let Some((a, b)) = pre.cursor
                && a != b
            {
                for r in edit
                    .layout
                    .range_rects(start + a, start + b, origin.x, origin.y)
                {
                    let line = RectI::new(r.x, r.bottom() - thin * 3, r.width, thin * 2);
                    self.fill(line, fade(c.color, opacity), clip);
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
                self.fill(caret, fade(c.caret_color, opacity), clip);
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
        // Computed in scene coordinates so that glyph positions don't depend on the scroll offset.
        let clip = self.clip_to_scene(clip);
        let (ox, oy) = (
            (origin.x + self.offset.x) as f32,
            (origin.y + self.offset.y) as f32,
        );
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
            normalized_coords: run
                .normalized_coords()
                .iter()
                .map(|c| c.to_bits())
                .collect(),
            embolden: synthesis.embolden(),
            skew: synthesis.skew(),
            color,
            glyphs,
            clip,
        }));
    }
}
