//! Style resolution and the integer flex layout engine.
//!
//! All sizes are physical pixels (`i32`) and boxes use `box-sizing: border-box`.
//! See `docs/design/03-layout.md`.

use crate::animation::RunningTransition;
use crate::geometry::{PointI, RectI, Scale, SizeI};
use crate::scene::Clip;
use crate::text::{ShapeStyle, TextSystem};
use crate::tree::{Document, Node, NodeId, NodeKind, PopupSurface};
use anti_webui_style::{
    ANIMATED_FIELDS, AlignItems, ComputedStyle, Display, Edges, JustifyContent, Length, Overflow,
    Position, WhiteSpace,
};
use std::collections::BinaryHeap;
use std::time::Instant;

/// A length after conversion to physical pixels; percentages stay unresolved until the
/// containing block is known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum RLen {
    Px(i32),
    Percent(f32),
    #[default]
    Auto,
}

impl RLen {
    fn from_length(l: Length, scale: Scale) -> Self {
        match l {
            Length::Px(v) => RLen::Px(scale.px(v)),
            Length::Percent(p) => RLen::Percent(p),
            Length::Auto => RLen::Auto,
        }
    }

    pub fn resolve(self, base: Option<i32>) -> Option<i32> {
        match self {
            RLen::Px(v) => Some(v),
            RLen::Percent(p) => base.map(|b| (b as f64 * p as f64 / 100.0).round() as i32),
            RLen::Auto => None,
        }
    }

    pub fn is_auto(self) -> bool {
        matches!(self, RLen::Auto)
    }
}

/// The layout-relevant part of a computed style, in physical pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    pub width: RLen,
    pub height: RLen,
    pub min_width: RLen,
    pub min_height: RLen,
    pub max_width: RLen,
    pub max_height: RLen,
    pub flex_basis: RLen,
    pub margin: Edges<RLen>,
    pub inset: Edges<RLen>,
    pub padding: Edges<i32>,
    pub border: Edges<i32>,
    pub row_gap: i32,
    pub column_gap: i32,
}

impl Resolved {
    pub fn from_computed(c: &ComputedStyle, scale: Scale) -> Self {
        let l = |v: Length| RLen::from_length(v, scale);
        Self {
            width: l(c.width),
            height: l(c.height),
            min_width: l(c.min_width),
            min_height: l(c.min_height),
            max_width: l(c.max_width),
            max_height: l(c.max_height),
            flex_basis: l(c.flex_basis),
            margin: c.margin().map(l),
            inset: c.inset().map(l),
            padding: c.padding().map(|v| scale.px(v.max(0.0))),
            border: c.border_widths().map(|v| scale.px_min1(v)),
            row_gap: scale.px(c.row_gap),
            column_gap: scale.px(c.column_gap),
        }
    }
}

/// Splits `total` pixels between items in proportion to `weights`. The result always sums to
/// `total` exactly, and growing `total` never shrinks any share (house monotonicity).
///
/// Equal weights give the first `total % n` items one extra pixel. Unequal weights use the
/// Sainte-Laguë (Webster) divisor method, which avoids the Alabama paradox of largest-remainder.
pub fn distribute(total: i32, weights: &[f32]) -> Vec<i32> {
    let n = weights.len();
    let mut out = vec![0; n];
    if n == 0 || total == 0 {
        return out;
    }
    let sign = total.signum();
    let total = total.abs();
    let active: Vec<usize> = (0..n).filter(|&i| weights[i] > 0.0).collect();
    if active.is_empty() {
        return out;
    }
    let first = weights[active[0]];
    if active.iter().all(|&i| weights[i] == first) {
        let k = active.len() as i32;
        let (q, r) = (total / k, total % k);
        for (j, &i) in active.iter().enumerate() {
            out[i] = sign * (q + (j < r as usize) as i32);
        }
        return out;
    }

    #[derive(PartialEq)]
    struct Entry {
        priority: f64,
        index: usize,
    }
    impl Eq for Entry {}
    impl PartialOrd for Entry {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Entry {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            // Highest priority first; ties go to the lower index.
            self.priority
                .total_cmp(&other.priority)
                .then_with(|| other.index.cmp(&self.index))
        }
    }

    let mut heap: BinaryHeap<Entry> = active
        .iter()
        .map(|&i| Entry {
            priority: weights[i] as f64,
            index: i,
        })
        .collect();
    for _ in 0..total {
        let top = heap.pop().unwrap();
        out[top.index] += 1;
        let seats = out[top.index] as f64;
        heap.push(Entry {
            priority: weights[top.index] as f64 / (2.0 * seats + 1.0),
            index: top.index,
        });
    }
    if sign < 0 {
        for v in &mut out {
            *v = -*v;
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeasureKey {
    known: (Option<i32>, Option<i32>),
    avail_w: Option<i32>,
    base: (Option<i32>, Option<i32>),
}

#[derive(Default)]
pub(crate) struct MeasureCache {
    entries: Vec<(MeasureKey, SizeI)>,
}

impl MeasureCache {
    fn get(&self, key: &MeasureKey) -> Option<SizeI> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, s)| *s)
    }

    fn put(&mut self, key: MeasureKey, size: SizeI) {
        if self.entries.len() >= 16 {
            self.entries.remove(0);
        }
        self.entries.push((key, size));
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Whether two computed styles lay out identically (paint-only properties may differ).
fn same_layout(a: &ComputedStyle, b: &ComputedStyle) -> bool {
    a.display == b.display
        && a.position == b.position
        && a.flex_direction == b.flex_direction
        && a.justify_content == b.justify_content
        && a.align_items == b.align_items
        && a.align_self == b.align_self
        && a.flex_grow == b.flex_grow
        && a.flex_shrink == b.flex_shrink
        && a.overflow_x == b.overflow_x
        && a.overflow_y == b.overflow_y
        && a.font_family == b.font_family
        && a.font_size == b.font_size
        && a.font_weight == b.font_weight
        && a.font_style == b.font_style
        && a.font_axes == b.font_axes
        && a.line_height == b.line_height
        && a.letter_spacing == b.letter_spacing
        && a.text_align == b.text_align
        && a.white_space == b.white_space
}

/// Resolves the styles that may have changed, invalidating the layout of the nodes whose
/// layout-affecting properties did. A node's result depends only on its own style and its parent's
/// result, so the work stops wherever a result comes out the same.
pub fn resolve_styles(doc: &mut Document) {
    let dirty = std::mem::take(&mut doc.style_dirty);
    let mut out = Resolution::default();
    if doc.style_all || dirty.len() * 8 > doc.nodes.len() {
        doc.style_all = false;
        for &id in &dirty {
            if let Some(n) = doc.nodes.get_mut(id) {
                n.style_dirty = false;
            }
        }
        if let Some(root) = doc.root {
            resolve_node(doc, root, &ComputedStyle::default(), true, &mut out);
        }
    } else {
        for id in dirty {
            // Reached from a parent that changed in the meantime?
            let Some(n) = doc.nodes.get(id).filter(|n| n.style_dirty) else {
                continue;
            };
            let parent = n
                .parent
                .and_then(|p| doc.nodes.get(p))
                .map_or_else(ComputedStyle::default, |p| p.computed.clone());
            resolve_node(doc, id, &parent, false, &mut out);
        }
    }
    for id in out.layout {
        doc.invalidate_layout(id);
    }
    for id in out.visual {
        grow_bounds(doc, id);
    }
    for id in out.moved {
        grow_bounds(doc, id);
        doc.transform_moved = true;
        mark_outer_layers_stale(doc, id);
    }
    for id in out.faded {
        mark_outer_layers_stale(doc, id);
    }
    doc.dirty.content |= out.content;
    for id in out.spaces {
        // Positioning doesn't go down where nothing seems to have moved.
        for a in doc.ancestors(id) {
            doc.nodes[a].position_dirty = true;
        }
        doc.dirty.position = true;
    }
    doc.reposition.extend(out.clips);
}

/// The layers around a layer hold how it shows in their content.
pub(crate) fn mark_outer_layers_stale(doc: &mut Document, id: NodeId) {
    let node = &doc.nodes[id];
    let mut space = if node.transformed {
        node.outer_space
    } else {
        node.space
    };
    while let Some(s) = space {
        let Some(n) = doc.nodes.get(s) else {
            return;
        };
        if n.is_popup() {
            return;
        }
        space = if n.transformed {
            n.outer_space
        } else {
            n.space
        };
        doc.stale_layers.insert(s);
    }
}

/// Whether all that differs is what compositing a transformed node's layer applies.
fn same_content(a: &ComputedStyle, b: &ComputedStyle) -> bool {
    let mut b = b.clone();
    b.translate_x = a.translate_x;
    b.translate_y = a.translate_y;
    b.move_x = a.move_x;
    b.move_y = a.move_y;
    b.scale_x = a.scale_x;
    b.scale_y = a.scale_y;
    b.rotate = a.rotate;
    b.transform_origin_x = a.transform_origin_x;
    b.transform_origin_y = a.transform_origin_y;
    b.opacity = a.opacity;
    b.blur = a.blur;
    b.backdrop_blur = a.backdrop_blur;
    b.transitions.clone_from(&a.transitions);
    *a == b
}

#[derive(Default)]
struct Resolution {
    /// Layout-affecting properties changed.
    layout: Vec<NodeId>,
    /// Painted extent changed without necessarily affecting layout.
    visual: Vec<NodeId>,
    /// The clip for the descendants changed without affecting layout.
    clips: Vec<NodeId>,
    /// A transformed node stays one, and its layer may show differently.
    moved: Vec<NodeId>,
    /// A node faded as a layer stays one, and it may show differently.
    faded: Vec<NodeId>,
    /// What layers hold may be out of date.
    content: bool,
    /// Nodes that started or stopped being transformed.
    spaces: Vec<NodeId>,
}

fn resolve_node(
    doc: &mut Document,
    id: NodeId,
    parent: &ComputedStyle,
    all: bool,
    out: &mut Resolution,
) {
    let Some(node) = doc.nodes.get(id) else {
        return;
    };
    let mut computed = ComputedStyle::inherit_from(parent);
    node.style.base.apply_to(&mut computed);
    for layer in &node.style.dynamic {
        layer.apply_to(&mut computed);
    }
    if let Some(s) = &node.style.hover
        && doc.state.hovered.contains(&id)
    {
        s.apply_to(&mut computed);
    }
    if let Some(s) = &node.style.focus
        && doc.state.focused == Some(id)
    {
        s.apply_to(&mut computed);
    }
    if let Some(s) = &node.style.active
        && doc.state.active.contains(&id)
    {
        s.apply_to(&mut computed);
    }
    if matches!(node.kind, NodeKind::Text(_)) {
        // Text nodes are inline content: box properties never apply to them.
        computed.display = Display::Block;
    }
    if let Some(p) = node.presence.as_ref().filter(|p| p.exiting) {
        p.exit.apply_to(&mut computed);
        computed.pointer_events = false;
    }
    if !node.transitions.is_empty() || !computed.transitions.is_empty() || node.presence.is_some() {
        apply_transitions(doc, id, &mut computed);
    }
    apply_move(doc, id, &mut computed);
    let Some(node) = doc.nodes.get(id) else {
        return;
    };
    let resolved = Resolved::from_computed(&computed, doc.scale);
    if node.resolved != resolved || !same_layout(&node.computed, &computed) {
        out.layout.push(id);
    }
    let filtered = node.filter.is_some();
    let (stacked, context) = stacking_role(&node.kind, &computed, filtered, doc.root == Some(id));
    let stacking_changed = stacked != node.stacked
        || context != node.stacking_context
        || (stacked && node.computed.z_index != computed.z_index);
    let old = &node.computed;
    if old.shadows != computed.shadows
        || old.blur != computed.blur
        || old.outline_width != computed.outline_width
        || old.outline_offset != computed.outline_offset
    {
        out.visual.push(id);
    }
    if old.radii() != computed.radii() {
        out.clips.push(id);
    }
    let changed = *old != computed;
    if changed {
        let transformed = node.transformed && computed.has_transform();
        let faded = !node.transformed
            && !computed.has_transform()
            && (old.opacity < 1.0 || old.blur > 0.0 || filtered)
            && (computed.opacity < 1.0 || computed.blur > 0.0 || filtered)
            && doc.layer_contents.contains_key(&id);
        if transformed {
            out.moved.push(id);
        } else if faded {
            out.faded.push(id);
        }
        if !(transformed || faded) || !same_content(old, &computed) {
            out.content = true;
        }
        if old.has_transform() != computed.has_transform() {
            out.spaces.push(id);
        }
    }
    let children = (all || changed).then(|| node.children.clone());
    let node = doc.nodes.get_mut(id).unwrap();
    node.style_dirty = false;
    node.resolved = resolved;
    node.computed = computed;
    if stacking_changed {
        node.stacked = stacked;
        node.stacking_context = context;
        doc.stacking_dirty = true;
        if stacked {
            doc.stacked.insert(id);
        } else {
            doc.stacked.remove(&id);
        }
    }
    if let Some(children) = children {
        let computed = doc.nodes[id].computed.clone();
        for c in children {
            resolve_node(doc, c, &computed, all, out);
        }
    }
}

/// Turns the style a node is going to into the one to show now, starting or redirecting
/// transitions as what it is going to changes (see `docs/design/13-animation.md`).
fn apply_transitions(doc: &mut Document, id: NodeId, target: &mut ComputedStyle) {
    let node = &mut doc.nodes[id];
    // What it goes from when it comes, and how long coming and going takes.
    let mut starting = None;
    let mut timing_of_presence = None;
    if let Some(p) = &mut node.presence {
        if let Some(enter) = p.starting.take() {
            let mut style = target.clone();
            enter.apply_to(&mut style);
            starting = Some(style);
            timing_of_presence = Some(p.timing);
        }
        if p.exiting || std::mem::take(&mut p.returning) {
            timing_of_presence = Some(p.timing);
        }
        if !p.exiting {
            node.presence = None;
        }
    }
    let old = starting.as_ref().unwrap_or(&node.computed);
    // Nothing to go from: it is new, or wasn't shown.
    let shown = starting.is_some()
        || (node.visible && old.display != Display::None && target.display != Display::None);
    let now = Instant::now();
    for (i, field) in ANIMATED_FIELDS.iter().enumerate() {
        let timing = timing_of_presence
            .or_else(|| target.transition_for(field.prop))
            .filter(|t| shown && !t.duration.is_zero());
        if let Some(r) = node.transitions.iter().position(|r| r.field == i) {
            let to = (field.get)(target);
            let running = &mut node.transitions[r];
            if running.to == to {
                let finished = running.finished;
                (field.set)(target, running.current.clone());
                if finished {
                    node.transitions.swap_remove(r);
                }
            } else if let Some(timing) = timing {
                let current = running.current.clone();
                running.redirect(to, timing, now);
                (field.set)(target, current);
            } else {
                node.transitions.swap_remove(r);
            }
        } else if !(field.same)(old, target)
            && let Some(timing) = timing
        {
            let from = (field.get)(old);
            let to = (field.get)(target);
            // Nothing in between: it just changes.
            if from.interpolate(&to, 0.0).is_some() {
                (field.set)(target, from.clone());
                node.transitions
                    .push(RunningTransition::new(i, from, to, timing, now));
            }
        }
    }
    if node.transitions.is_empty() {
        doc.transitioning.remove(&id);
    } else {
        doc.transitioning.insert(id);
    }
}

/// Puts a node where it is on its way to its place in a list, and ends that once it is there.
fn apply_move(doc: &mut Document, id: NodeId, computed: &mut ComputedStyle) {
    let Some(node) = doc.nodes.get_mut(id) else {
        return;
    };
    let Some(m) = &node.moving else {
        return;
    };
    if m.finished {
        node.moving = None;
        if node.transitions.is_empty() {
            doc.transitioning.remove(&id);
        }
    } else {
        (computed.move_x, computed.move_y) = m.current;
        doc.transitioning.insert(id);
    }
}

/// Whether a node is painted from a stacking context's list, and whether it is a stacking context.
/// See `docs/design/09-shadows-stacking-popups.md`.
fn stacking_role(kind: &NodeKind, c: &ComputedStyle, filtered: bool, root: bool) -> (bool, bool) {
    if matches!(kind, NodeKind::Fragment) || c.display == Display::Contents {
        return (false, root);
    }
    let stacked = c.z_index.is_some() || c.position == Position::Absolute;
    let context = root
        || matches!(kind, NodeKind::Popup(_))
        || c.z_index.is_some()
        || c.opacity < 1.0
        || c.blur > 0.0
        || c.backdrop_blur > 0.0
        || filtered
        || c.has_transform()
        || c.overflow_x == Overflow::Scroll
        || c.overflow_y == Overflow::Scroll;
    (stacked, context)
}

/// Builds the paint order of the stacked nodes, which needs their document order.
pub fn build_stacking(doc: &mut Document) {
    doc.stacking_dirty = false;
    let mut lists = std::mem::take(&mut doc.stacking);
    lists.clear();
    for &id in &doc.stacked {
        // The first context above it; nodes that aren't in the document don't get there.
        let mut cur = doc.nodes.get(id).and_then(|n| n.parent);
        let mut context = None;
        while let Some(c) = cur {
            let Some(n) = doc.nodes.get(c) else {
                break;
            };
            if context.is_none() && n.stacking_context {
                context = Some(c);
            }
            cur = n.parent;
            if cur.is_none() && doc.root != Some(c) {
                context = None;
            }
        }
        if let Some(context) = context {
            lists.entry(context).or_default().nodes.push(id);
        }
    }
    for list in lists.values_mut() {
        let key = |id: &NodeId| {
            let n = &doc.nodes[*id];
            (n.computed.z_index.unwrap_or(0), n.doc_order)
        };
        list.nodes.sort_unstable_by_key(key);
        list.negative = list.nodes.partition_point(|id| key(id).0 < 0);
    }
    doc.stacking = lists;
    let mut popups = std::mem::take(&mut doc.popups);
    popups.sort_by_cached_key(|id| doc.ancestors(*id).len());
    doc.popups = popups;
}

/// Makes sure the bounds of `id` and its ancestors contain what `id` paints now. They only grow:
/// too big is harmless, and the next positioning pass tightens them.
pub(crate) fn grow_bounds(doc: &mut Document, id: NodeId) {
    let Some(n) = doc.nodes.get_mut(id) else {
        return;
    };
    let own = own_bounds(n, doc.scale);
    n.subtree_bounds = grown_by_filter(n.subtree_bounds.union(&own), n, doc.scale);
    let mut bounds = n.bounds_in_parent(doc.scale);
    let mut child = id;
    let mut parent = n.parent;
    while let Some(p) = parent {
        let scale = doc.scale;
        let Some(n) = doc.nodes.get_mut(p) else {
            return;
        };
        if let Some(i) = n.children.iter().position(|c| *c == child)
            && let Some(b) = n.child_bounds.get_mut(i)
        {
            *b = bounds;
        }
        if n.scroll_root || n.is_popup() {
            return;
        }
        let clip = child_clip(n, scale, n.clip_shape());
        let grown = n.subtree_bounds.union(&bounds.intersect(&clip.rect));
        if grown == n.subtree_bounds {
            return;
        }
        n.subtree_bounds = grown;
        bounds = n.bounds_in_parent(scale);
        child = p;
        parent = n.parent;
    }
}

/// What is around what a node paints, once a filter is done with it. Nothing stays nothing.
fn grown_by_filter(bounds: RectI, node: &Node, scale: Scale) -> RectI {
    if bounds.is_empty() {
        bounds
    } else {
        bounds.outset(node.filter_reach(scale))
    }
}

/// What the node itself paints, in the same coordinates as `abs`.
fn own_bounds(node: &Node, scale: Scale) -> RectI {
    if matches!(node.kind, NodeKind::Fragment) {
        return RectI::default();
    }
    if let NodeKind::Text(t) = &node.kind {
        // Glyphs can stick out of the line box, by less than a font size in practice.
        let size = t.layout.size();
        let text = RectI::new(node.abs.x, node.abs.y, size.width, size.height);
        return node
            .abs
            .union(&text)
            .outset(scale.px(node.computed.font_size).max(1));
    }
    let c = &node.computed;
    let mut bounds = node.abs;
    for s in c.shadows.iter().filter(|s| !s.inset) {
        let grow = scale.px(s.spread) + (scale.pxf(s.blur) * 1.5 + 1.0).ceil() as i32;
        let rect = node
            .abs
            .translate(scale.px(s.offset_x), scale.px(s.offset_y))
            .outset(grow.max(0));
        bounds = bounds.union(&rect);
    }
    if c.outline_width > 0.0 {
        let w = scale.px_min1(c.outline_width) + scale.px(c.outline_offset);
        bounds = bounds.union(&node.abs.outset(w.max(0)));
    }
    bounds
}

/// Whether the node's content scrolls: it is a scroller and something overflows.
fn overflows(node: &Node) -> bool {
    if node.is_edit() {
        return false;
    }
    let c = &node.computed;
    let pad = node.padding_abs();
    (c.overflow_y == Overflow::Scroll && node.content_size.height > pad.height)
        || (c.overflow_x == Overflow::Scroll && node.content_size.width > pad.width)
}

/// Lays out the whole document against the viewport.
pub fn layout_document(doc: &mut Document, ts: &mut TextSystem) {
    let Some(root) = doc.root else { return };
    let viewport = doc.viewport;
    let scale = doc.scale;
    let mut engine = Engine { doc, ts, scale };
    let size = (Some(viewport.width), Some(viewport.height));
    engine.compute(root, size, Some(viewport.width), size, true);
    engine.doc.nodes[root].rect = RectI::new(0, 0, viewport.width, viewport.height);
    for id in engine.doc.popups.clone() {
        if engine.doc.nodes[id].popup().is_some_and(|p| p.portal) {
            engine.compute(id, size, Some(viewport.width), size, true);
            engine.doc.nodes[id].rect = RectI::new(0, 0, viewport.width, viewport.height);
        } else {
            engine.layout_popup(id, size);
        }
    }
}

/// The clip `node` imposes on its children, given the clip that applies to `node` itself.
///
/// Clipping happens at the padding box. Rounded corners follow the inner border radius. When
/// two rounded clips overlap, the smaller one wins (an approximation of their intersection).
pub fn child_clip(node: &Node, scale: Scale, parent: Clip) -> Clip {
    let clips = node.computed.overflow_x != Overflow::Visible
        || node.computed.overflow_y != Overflow::Visible
        || node.is_edit();
    if !clips {
        return parent;
    }
    let pad = node.padding_abs();
    let rect = parent.rect.intersect(&pad);
    let b = &node.resolved.border;
    let r = node.computed.radii();
    let inner = [
        (scale.pxf(r.top_left) - b.left.max(b.top) as f32).max(0.0),
        (scale.pxf(r.top_right) - b.right.max(b.top) as f32).max(0.0),
        (scale.pxf(r.bottom_right) - b.right.max(b.bottom) as f32).max(0.0),
        (scale.pxf(r.bottom_left) - b.left.max(b.bottom) as f32).max(0.0),
    ];
    let radii = if rect == pad && inner.iter().any(|r| *r > 0.0) {
        inner
    } else if rect == parent.rect {
        parent.radii
    } else {
        [0.0; 4]
    };
    Clip { rect, radii }
}

/// Computes coordinates, clips, bounds and document order, and clamps scroll offsets.
///
/// The descendants of a scroll root get coordinates that don't depend on its scroll offset, so
/// scrolling it doesn't come here at all. Subtrees that layout didn't touch and that stay where
/// they were are skipped.
pub fn position_document(doc: &mut Document) {
    let Some(root) = doc.root else { return };
    let mut order = 0u32;
    position_node(doc, root, PointI::default(), Clip::NONE, None, &mut order);
    for id in doc.popups.clone() {
        position_popup(doc, id, &mut order);
    }
}

/// What a subtree paints, for sizing a surface. Unlike `subtree_bounds`, which culling may take as
/// a generous estimate, this has no room for glyphs sticking out of their lines: every extra pixel
/// is one the compositor has to blend.
fn painted_extent(doc: &Document, id: NodeId) -> RectI {
    let Some(n) = doc.nodes.get(id).filter(|n| n.visible) else {
        return RectI::default();
    };
    if let NodeKind::Text(t) = &n.kind {
        let size = t.layout.size();
        return n
            .abs
            .union(&RectI::new(n.abs.x, n.abs.y, size.width, size.height));
    }
    let own = own_bounds(n, doc.scale);
    if n.computed.overflow_x != Overflow::Visible || n.computed.overflow_y != Overflow::Visible {
        return own;
    }
    n.children
        .iter()
        .filter(|c| doc.nodes.get(**c).is_some_and(|c| !c.is_popup()))
        .fold(own, |bounds, c| bounds.union(&painted_extent(doc, *c)))
}

/// Positions a popup in its surface, leaving room for what sticks out of its box, and works out
/// the surface.
fn position_popup(doc: &mut Document, id: NodeId, order: &mut u32) {
    let parent_visible = doc.nodes[id]
        .parent
        .is_some_and(|p| doc.nodes.get(p).is_some_and(|n| n.visible));
    let open =
        parent_visible && doc.nodes[id].popup().is_some_and(|d| !d.dismissed) && doc.attached(id);
    if !open {
        hide_subtree(doc, id);
        return;
    }
    if doc.nodes[id].popup().is_some_and(|p| p.portal) {
        position_node(doc, id, PointI::default(), Clip::NONE, Some(id), order);
        return;
    }
    let scale = doc.scale;
    let start = *order;
    position_node(doc, id, PointI::default(), Clip::NONE, Some(id), order);
    let node = &mut doc.nodes[id];
    let abs = node.abs;
    let bounds = painted_extent(doc, id).union(&abs);
    let node = &mut doc.nodes[id];
    let geometry = RectI::new(
        scale.logical_ceil(abs.x - bounds.x),
        scale.logical_ceil(abs.y - bounds.y),
        scale.logical_ceil(abs.width).max(1),
        scale.logical_ceil(abs.height).max(1),
    );
    let logical = SizeI::new(
        geometry.right() + scale.logical_ceil(bounds.right() - abs.right()),
        geometry.bottom() + scale.logical_ceil(bounds.bottom() - abs.bottom()),
    );
    let surface = PopupSurface {
        buffer: SizeI::new(
            scale.px(logical.width as f32),
            scale.px(logical.height as f32),
        ),
        logical,
        geometry,
    };
    let at = PointI::new(scale.px(geometry.x as f32), scale.px(geometry.y as f32));
    node.popup_mut().unwrap().surface = surface;
    if abs.origin() != at {
        node.rect.x = at.x;
        node.rect.y = at.y;
        *order = start;
        position_node(doc, id, PointI::default(), Clip::NONE, Some(id), order);
    }
}

fn position_node(
    doc: &mut Document,
    id: NodeId,
    origin: PointI,
    clip: Clip,
    space: Option<NodeId>,
    order: &mut u32,
) {
    let Some(node) = doc.nodes.get_mut(id) else {
        return;
    };
    if node.computed.display == Display::None {
        hide_subtree(doc, id);
        return;
    }
    let abs = node.rect.translate(origin.x, origin.y);
    // Layout places a fragment's children from its parent, without visiting the fragment.
    let fragment =
        matches!(node.kind, NodeKind::Fragment) || node.computed.display == Display::Contents;
    let transformed =
        !fragment && !matches!(node.kind, NodeKind::Popup(_)) && node.computed.has_transform();
    let placed_in = if node.transformed {
        (node.layer_clip, node.outer_space)
    } else {
        (node.clip_shape(), node.space)
    };
    if !fragment
        && !node.position_dirty
        && node.visible
        && node.abs == abs
        && node.transformed == transformed
        && placed_in == (clip, space)
        && node.doc_order == *order
    {
        *order += node.subtree_nodes;
        return;
    }
    let first = *order;
    *order += 1;
    node.position_dirty = false;
    let resized =
        node.listeners.resized.is_some() && (!node.visible || node.abs.size() != abs.size());
    node.visible = true;
    node.abs = abs;
    if resized {
        doc.resized.push(id);
    }
    let Some(node) = doc.nodes.get_mut(id) else {
        return;
    };
    node.transformed = transformed;
    if transformed {
        // Its own space: what is outside of it applies when the layer is composited.
        node.layer_clip = clip;
        node.outer_space = space;
        node.clip = Clip::NONE.rect;
        node.clip_radii = Clip::NONE.radii;
        node.space = Some(id);
    } else {
        node.clip = clip.rect;
        node.clip_radii = clip.radii;
        node.space = space;
    }
    node.doc_order = first;
    let max_x = (node.content_size.width - node.padding_abs().width).max(0);
    let max_y = (node.content_size.height - node.padding_abs().height).max(0);
    let scrolled_before = node.scroll;
    node.scroll.x = if node.computed.overflow_x == Overflow::Scroll {
        node.scroll.x.clamp(0, max_x)
    } else {
        0
    };
    node.scroll.y = if node.computed.overflow_y == Overflow::Scroll {
        node.scroll.y.clamp(0, max_y)
    } else {
        0
    };
    if node.scroll != scrolled_before && node.listeners.scrolled.is_some() {
        doc.scrolled.push(id);
    }
    // Scroll layers aren't inside of other layers; a popup's surface is like a window to them.
    let scroller = overflows(node);
    let space = node.space;
    let scroll_root = scroller && space.is_none_or(|s| doc.nodes[s].is_popup());
    doc.nodes[id].scroll_root = scroll_root;
    position_children(doc, id, order);
    let node = &mut doc.nodes[id];
    node.subtree_nodes = *order - first;
}

/// Positions the children of a node that is itself in place, and updates its bounds.
fn position_children(doc: &mut Document, id: NodeId, order: &mut u32) {
    let scale = doc.scale;
    let node = &mut doc.nodes[id];
    let inner = child_clip(node, scale, node.clip_shape());
    let (origin, clip, space) = if node.scroll_root {
        (PointI::default(), Clip::NONE, Some(id))
    } else {
        (
            PointI::new(node.abs.x - node.scroll.x, node.abs.y - node.scroll.y),
            inner,
            node.space,
        )
    };
    let children = std::mem::take(&mut node.children);
    let mut bounds = std::mem::take(&mut node.child_bounds);
    bounds.clear();
    let mut union = RectI::default();
    for &c in &children {
        // A popup is on another surface, positioned on its own.
        let popup = doc.nodes.get(c).is_some_and(Node::is_popup);
        if !popup {
            position_node(doc, c, origin, clip, space, order);
        }
        let b = doc
            .nodes
            .get(c)
            .filter(|_| !popup)
            .map_or(RectI::default(), |n| n.bounds_in_parent(scale));
        bounds.push(b);
        union = union.union(&b);
    }
    let node = &mut doc.nodes[id];
    node.children = children;
    node.child_bounds = bounds;
    let own = own_bounds(node, scale);
    // A scroll root's descendants are in other coordinates, and within its box anyway.
    let bounds = if node.scroll_root {
        own
    } else {
        own.union(&union.intersect(&inner.rect))
    };
    node.subtree_bounds = grown_by_filter(bounds, node, scale);
}

/// Positions the descendants of `id` again after it scrolled (it is not a scroll root).
pub fn reposition_children(doc: &mut Document, id: NodeId) {
    let Some(n) = doc.nodes.get(id) else {
        return;
    };
    let mut order = n.doc_order + 1;
    position_children(doc, id, &mut order);
}

fn hide_subtree(doc: &mut Document, id: NodeId) {
    let mut stack = vec![id];
    while let Some(id) = stack.pop() {
        if let Some(n) = doc.nodes.get_mut(id) {
            n.visible = false;
            n.subtree_bounds = RectI::default();
            n.scroll_root = false;
            stack.extend(n.children.iter().copied());
        }
    }
}

struct Engine<'a> {
    doc: &'a mut Document,
    ts: &'a mut TextSystem,
    scale: Scale,
}

struct Item {
    id: NodeId,
    r: Resolved,
    grow: f32,
    shrink: f32,
    align: AlignItems,
    /// Margins on the main axis (start, end) and cross axis (start, end).
    margin_main: (i32, i32),
    margin_cross: (i32, i32),
    base: i32,
    hypo: i32,
    min_main: Option<i32>,
    max_main: Option<i32>,
    target: i32,
    frozen: bool,
    cross: i32,
}

fn clamp_opt(v: i32, min: Option<i32>, max: Option<i32>) -> i32 {
    let mut v = v;
    if let Some(max) = max {
        v = v.min(max);
    }
    if let Some(min) = min {
        v = v.max(min);
    }
    v.max(0)
}

impl Engine<'_> {
    /// Children that take part in flow layout, flattening `display: contents` nodes.
    fn flow_children(&self, id: NodeId) -> (Vec<NodeId>, Vec<NodeId>) {
        let mut flow = Vec::new();
        let mut absolute = Vec::new();
        let mut stack: Vec<NodeId> = self.doc.nodes[id].children.iter().rev().copied().collect();
        while let Some(c) = stack.pop() {
            let Some(n) = self.doc.nodes.get(c) else {
                continue;
            };
            if n.is_popup() {
                continue;
            }
            if matches!(n.kind, NodeKind::Fragment) || n.computed.display == Display::Contents {
                stack.extend(n.children.iter().rev());
                continue;
            }
            match n.computed.display {
                Display::None => {}
                _ if n.computed.position == Position::Absolute => absolute.push(c),
                _ => flow.push(c),
            }
        }
        (flow, absolute)
    }

    /// Positions `display: contents` descendants so their boxes coincide with `id`'s, making
    /// child coordinates relative to `id` regardless of nesting.
    fn place_fragments(&mut self, id: NodeId, size: SizeI) {
        let children = self.doc.nodes[id].children.clone();
        for c in children {
            let Some(n) = self.doc.nodes.get(c) else {
                continue;
            };
            if matches!(n.kind, NodeKind::Fragment) || n.computed.display == Display::Contents {
                self.doc.nodes[c].rect = RectI::new(0, 0, size.width, size.height);
                self.doc.nodes[c].content_size = SizeI::default();
                self.place_fragments(c, size);
            } else if n.computed.display == Display::None {
                self.doc.nodes[c].rect = RectI::default();
            }
        }
    }

    /// Computes the border-box size of `id`.
    ///
    /// - `known`: sizes imposed by the parent (e.g. stretched or flexed).
    /// - `avail_w`: available border-box width for content sizing (`None` = max-content).
    /// - `base`: the containing block's inner size for resolving percentages.
    /// - `perform`: also position the descendants.
    fn compute(
        &mut self,
        id: NodeId,
        known: (Option<i32>, Option<i32>),
        avail_w: Option<i32>,
        base: (Option<i32>, Option<i32>),
        perform: bool,
    ) -> SizeI {
        let key = MeasureKey {
            known,
            avail_w,
            base,
        };
        let node = &self.doc.nodes[id];
        if perform {
            // A clean subtree laid out with the same inputs keeps its previous result.
            if !node.layout_dirty
                && let Some((k, s)) = node.last_perform
                && k == key
            {
                return s;
            }
        } else if let Some(s) = node.measure_cache.get(&key) {
            return s;
        }
        let size = self.compute_inner(id, known, avail_w, base, perform);
        let node = &mut self.doc.nodes[id];
        if perform {
            node.last_perform = Some((key, size));
            node.layout_dirty = false;
            node.position_dirty = true;
        } else {
            node.measure_cache.put(key, size);
        }
        size
    }

    fn compute_inner(
        &mut self,
        id: NodeId,
        known: (Option<i32>, Option<i32>),
        avail_w: Option<i32>,
        base: (Option<i32>, Option<i32>),
        perform: bool,
    ) -> SizeI {
        let node = &self.doc.nodes[id];
        let r = node.resolved.clone();
        let display = node.computed.display;
        let is_text = node.is_text();
        let is_edit = node.is_edit();

        let min_w = r.min_width.resolve(base.0);
        let max_w = r.max_width.resolve(base.0);
        let min_h = r.min_height.resolve(base.1);
        let max_h = r.max_height.resolve(base.1);
        let pb_h = r.padding.left + r.padding.right + r.border.left + r.border.right;
        let pb_v = r.padding.top + r.padding.bottom + r.border.top + r.border.bottom;
        let min_w = Some(min_w.unwrap_or(0).max(pb_h));
        let min_h = Some(min_h.unwrap_or(0).max(pb_v));

        let mut w = known
            .0
            .or_else(|| r.width.resolve(base.0))
            .map(|v| clamp_opt(v, min_w, max_w));
        let mut h = known
            .1
            .or_else(|| r.height.resolve(base.1))
            .map(|v| clamp_opt(v, min_h, max_h));

        if is_text {
            return self.compute_text(id, w, avail_w, perform);
        }

        if let (Some(w), Some(h)) = (w, h)
            && !perform
        {
            return SizeI::new(w, h);
        }

        if perform && (w.is_none() || h.is_none()) {
            let s = self.compute(id, (w, h), avail_w, base, false);
            w = Some(s.width);
            h = Some(s.height);
        }

        let inner_w = w.map(|w| (w - pb_h).max(0));
        let inner_h = h.map(|h| (h - pb_v).max(0));
        let inner_avail = inner_w.or(avail_w.map(|a| {
            let a = clamp_opt(a, min_w, max_w);
            (a - pb_h).max(0)
        }));

        let content = if is_edit {
            self.compute_edit(id, inner_w, inner_avail, perform)
        } else {
            match display {
                Display::Flex => self.flex(id, inner_w, inner_h, inner_avail, perform),
                _ => self.block(id, inner_w, inner_h, inner_avail, perform),
            }
        };

        let w = w.unwrap_or_else(|| clamp_opt(content.width + pb_h, min_w, max_w));
        let h = h.unwrap_or_else(|| clamp_opt(content.height + pb_v, min_h, max_h));
        let size = SizeI::new(w, h);

        if perform {
            let node = &mut self.doc.nodes[id];
            node.rect.width = w;
            node.rect.height = h;
            self.place_fragments(id, size);
            self.layout_absolute(id, size);
            self.update_content_size(id);
        }
        size
    }

    fn compute_text(
        &mut self,
        id: NodeId,
        w: Option<i32>,
        avail_w: Option<i32>,
        perform: bool,
    ) -> SizeI {
        let scale = self.scale;
        let node = &mut self.doc.nodes[id];
        let style = ShapeStyle::from_computed(&node.computed, scale);
        let align = node.computed.text_align;
        let wraps = node.computed.white_space == WhiteSpace::PreWrap;
        let NodeKind::Text(data) = &mut node.kind else {
            unreachable!()
        };
        data.layout.shape(self.ts, &data.text, data.version, &style);
        let width = match w {
            Some(w) => w,
            None => {
                data.layout.break_lines(None);
                let max = data.layout.size().width;
                match avail_w {
                    // What doesn't wrap is as wide as it is, and sticks out if it has to.
                    Some(a) if a < max && wraps => {
                        data.layout.break_lines(Some(a.max(0)));
                        data.layout.size().width.min(a.max(0))
                    }
                    _ => max,
                }
            }
        };
        data.layout.break_lines(Some(width));
        let height = data.layout.size().height;
        if perform {
            data.layout.set_final(width, align);
            node.rect.width = width;
            node.rect.height = height;
        } else {
            // Measuring must not disturb the line breaks that are painted.
            data.layout.restore_final();
        }
        SizeI::new(width, height)
    }

    fn compute_edit(
        &mut self,
        id: NodeId,
        inner_w: Option<i32>,
        inner_avail: Option<i32>,
        perform: bool,
    ) -> SizeI {
        let scale = self.scale;
        let node = &mut self.doc.nodes[id];
        let style = ShapeStyle::from_computed(&node.computed, scale);
        let align = node.computed.text_align;
        let edit = node.edit_mut().unwrap();
        let multiline = edit.multiline;
        let default_w = scale.px(if multiline { 240.0 } else { 160.0 });
        let width = inner_w.unwrap_or_else(|| match inner_avail {
            Some(a) => default_w.min(a),
            None => default_w,
        });
        let content = edit.layout_text(self.ts, &style, if multiline { Some(width) } else { None });
        let line = edit.line_height();
        let height = if multiline {
            content.height.max(line * edit.rows.max(1) as i32)
        } else {
            line
        };
        if perform {
            edit.align(width, align);
            edit.content_size = content;
            edit.final_layout = Some((width, align));
        } else if multiline
            && let Some((fw, fa)) = edit.final_layout
            && fw != width
        {
            edit.layout_text(self.ts, &style, Some(fw));
            edit.align(fw, fa);
        }
        SizeI::new(width, height)
    }

    fn resolve_margins(&self, r: &Resolved, base_w: Option<i32>) -> Edges<i32> {
        // Percent margins resolve against the containing block's width, as in CSS.
        r.margin.map(|m| m.resolve(base_w).unwrap_or(0))
    }

    fn block(
        &mut self,
        id: NodeId,
        inner_w: Option<i32>,
        inner_h: Option<i32>,
        inner_avail: Option<i32>,
        perform: bool,
    ) -> SizeI {
        let (flow, _) = self.flow_children(id);
        let r = self.doc.nodes[id].resolved.clone();
        let gap = r.row_gap;
        let base = (inner_w, inner_h);
        let mut y = 0;
        let mut max_w = 0;
        for (i, &c) in flow.iter().enumerate() {
            let cr = self.doc.nodes[c].resolved.clone();
            let m = self.resolve_margins(&cr, inner_w);
            let mx = m.left + m.right;
            if i > 0 {
                y += gap;
            }
            let explicit_w = cr.width.resolve(inner_w);
            let child_w = match (explicit_w, inner_w) {
                (Some(_), _) => None,
                (None, Some(iw)) => Some((iw - mx).max(0)),
                (None, None) => None,
            };
            let avail = inner_avail.map(|a| (a - mx).max(0));
            let size = self.compute(c, (child_w, None), avail, base, false);
            if perform {
                let child = &mut self.doc.nodes[c];
                child.rect.x = r.border.left + r.padding.left + m.left;
                child.rect.y = r.border.top + r.padding.top + y + m.top;
                self.compute(c, (Some(size.width), Some(size.height)), avail, base, true);
            }
            y += m.top + size.height + m.bottom;
            max_w = max_w.max(size.width + mx);
        }
        SizeI::new(max_w, y)
    }

    #[allow(clippy::too_many_arguments)]
    fn flex(
        &mut self,
        id: NodeId,
        inner_w: Option<i32>,
        inner_h: Option<i32>,
        inner_avail: Option<i32>,
        perform: bool,
    ) -> SizeI {
        let (flow, _) = self.flow_children(id);
        let node = &self.doc.nodes[id];
        let r = node.resolved.clone();
        let dir = node.computed.flex_direction;
        let justify = node.computed.justify_content;
        let align_items = node.computed.align_items;
        let row = dir.is_row();
        let base = (inner_w, inner_h);
        let (main_inner, cross_inner) = if row {
            (inner_w, inner_h)
        } else {
            (inner_h, inner_w)
        };
        let gap = if row { r.column_gap } else { r.row_gap };
        let wh =
            |main: Option<i32>, cross: Option<i32>| if row { (main, cross) } else { (cross, main) };

        // 1. Flex base sizes.
        let mut items: Vec<Item> = Vec::with_capacity(flow.len());
        for &c in &flow {
            let n = &self.doc.nodes[c];
            let cr = n.resolved.clone();
            let m = self.resolve_margins(&cr, inner_w);
            let (margin_main, margin_cross) = if row {
                ((m.left, m.right), (m.top, m.bottom))
            } else {
                ((m.top, m.bottom), (m.left, m.right))
            };
            let align = n.computed.align_self.unwrap_or(align_items);
            let (main_style, cross_style) = if row {
                (cr.width, cr.height)
            } else {
                (cr.height, cr.width)
            };
            let (main_base, cross_base) = if row {
                (inner_w, inner_h)
            } else {
                (inner_h, inner_w)
            };
            let (min_s, max_s) = if row {
                (cr.min_width, cr.max_width)
            } else {
                (cr.min_height, cr.max_height)
            };
            let grow = n.computed.flex_grow.max(0.0);
            let shrink = n.computed.flex_shrink.max(0.0);

            // Cross size known up-front when stretching into a definite line.
            let cross_known = cross_style.resolve(cross_base).or_else(|| {
                (align == AlignItems::Stretch)
                    .then_some(cross_inner)
                    .flatten()
                    .map(|ci| (ci - margin_cross.0 - margin_cross.1).max(0))
            });

            let basis = match cr
                .flex_basis
                .resolve(main_inner)
                .or_else(|| main_style.resolve(main_base))
            {
                Some(b) => b,
                None => {
                    let avail = if row {
                        main_inner
                            .or(inner_avail)
                            .map(|a| (a - margin_main.0 - margin_main.1).max(0))
                    } else {
                        cross_known
                            .or(inner_avail.map(|a| (a - margin_cross.0 - margin_cross.1).max(0)))
                    };
                    let (kw, kh) = wh(None, cross_known);
                    let s = self.compute(c, (kw, kh), avail, base, false);
                    if row { s.width } else { s.height }
                }
            };
            let min_main = min_s.resolve(main_base);
            let max_main = max_s.resolve(main_base);
            let hypo = clamp_opt(basis, min_main, max_main);
            items.push(Item {
                id: c,
                r: cr,
                grow,
                shrink,
                align,
                margin_main,
                margin_cross,
                base: basis,
                hypo,
                min_main,
                max_main,
                target: hypo,
                frozen: false,
                cross: 0,
            });
        }

        let n = items.len() as i32;
        let gaps = gap * (n - 1).max(0);
        let outer_hypo: i32 = items
            .iter()
            .map(|it| it.hypo + it.margin_main.0 + it.margin_main.1)
            .sum();

        // 2–3. Resolve flexible lengths with integer distribution.
        if let Some(main_inner) = main_inner {
            let free = main_inner - outer_hypo - gaps;
            let growing = free > 0;
            if free < 0 {
                // Automatic minimum size: flex items with visible overflow do not shrink
                // below their min-content size.
                let candidates: Vec<usize> = (0..items.len())
                    .filter(|&i| {
                        let it = &items[i];
                        let c = &self.doc.nodes[it.id].computed;
                        let visible = if row {
                            c.overflow_x == Overflow::Visible
                        } else {
                            c.overflow_y == Overflow::Visible
                        };
                        it.min_main.is_none() && visible && it.shrink > 0.0
                    })
                    .collect();
                for i in candidates {
                    let it = &items[i];
                    let c = it.id;
                    let cross_style = if row { it.r.height } else { it.r.width };
                    let cross_known = cross_style
                        .resolve(if row { inner_h } else { inner_w })
                        .or_else(|| {
                            (it.align == AlignItems::Stretch)
                                .then_some(cross_inner)
                                .flatten()
                                .map(|ci| (ci - it.margin_cross.0 - it.margin_cross.1).max(0))
                        });
                    let content_min = self.content_min(c, row, cross_known, inner_avail, base);
                    let it = &items[i];
                    let specified = if row { it.r.width } else { it.r.height }.resolve(if row {
                        inner_w
                    } else {
                        inner_h
                    });
                    let auto_min = specified.map_or(content_min, |s| s.min(content_min));
                    let auto_min = auto_min.min(it.max_main.unwrap_or(i32::MAX));
                    items[i].min_main = Some(auto_min);
                }
            }

            for it in &mut items {
                let factor = if growing { it.grow } else { it.shrink };
                if free == 0
                    || factor == 0.0
                    || (growing && it.base > it.hypo)
                    || (!growing && it.base < it.hypo)
                {
                    it.frozen = true;
                    it.target = it.hypo;
                }
            }
            loop {
                let unfrozen: Vec<usize> = (0..items.len()).filter(|&i| !items[i].frozen).collect();
                if unfrozen.is_empty() {
                    break;
                }
                let used: i32 = items
                    .iter()
                    .map(|it| {
                        let size = if it.frozen { it.target } else { it.base };
                        size + it.margin_main.0 + it.margin_main.1
                    })
                    .sum();
                let mut space = main_inner - used - gaps;
                let weights: Vec<f32> = unfrozen
                    .iter()
                    .map(|&i| {
                        let it = &items[i];
                        if growing {
                            it.grow
                        } else {
                            it.shrink * it.base as f32
                        }
                    })
                    .collect();
                if growing {
                    let sum: f32 = weights.iter().sum();
                    if sum < 1.0 {
                        space = (space as f64 * sum as f64).round() as i32;
                    }
                }
                let shares = distribute(space, &weights);
                let mut violation = 0;
                let mut clamped = Vec::with_capacity(unfrozen.len());
                for (j, &i) in unfrozen.iter().enumerate() {
                    let it = &items[i];
                    let candidate = it.base + shares[j];
                    let c = clamp_opt(candidate, it.min_main, it.max_main);
                    violation += c - candidate;
                    clamped.push((i, candidate, c));
                }
                if violation == 0 {
                    for (i, _, c) in clamped {
                        items[i].target = c;
                        items[i].frozen = true;
                    }
                    break;
                }
                for (i, candidate, c) in clamped {
                    let freeze = if violation > 0 {
                        c > candidate
                    } else {
                        c < candidate
                    };
                    if freeze {
                        items[i].target = c;
                        items[i].frozen = true;
                    }
                }
            }
        }

        // 4. Cross sizes.
        let mut line_cross = 0;
        for it in &mut items {
            let cross_style = if row { it.r.height } else { it.r.width };
            let cross_base = if row { inner_h } else { inner_w };
            let explicit = cross_style.resolve(cross_base);
            let content_cross = match explicit {
                Some(v) => v,
                None => {
                    let (kw, kh) = wh(Some(it.target), None);
                    let avail = if row {
                        Some(it.target)
                    } else {
                        inner_avail.map(|a| (a - it.margin_cross.0 - it.margin_cross.1).max(0))
                    };
                    let s = self.compute(it.id, (kw, kh), avail, base, false);
                    if row { s.height } else { s.width }
                }
            };
            it.cross = content_cross;
            line_cross = line_cross.max(content_cross + it.margin_cross.0 + it.margin_cross.1);
        }
        let line_cross = cross_inner.unwrap_or(line_cross);
        for it in &mut items {
            let cross_style = if row { it.r.height } else { it.r.width };
            let (min_c, max_c) = if row {
                (it.r.min_height, it.r.max_height)
            } else {
                (it.r.min_width, it.r.max_width)
            };
            let cross_base = if row { inner_h } else { inner_w };
            if it.align == AlignItems::Stretch && cross_style.is_auto() {
                it.cross = (line_cross - it.margin_cross.0 - it.margin_cross.1).max(0);
            }
            it.cross = clamp_opt(
                it.cross,
                min_c.resolve(cross_base),
                max_c.resolve(cross_base),
            );
        }

        let used_main: i32 = items
            .iter()
            .map(|it| it.target + it.margin_main.0 + it.margin_main.1)
            .sum::<i32>()
            + gaps;

        // 5. Main-axis placement.
        if perform {
            let leftover = main_inner.map(|m| m - used_main).unwrap_or(0).max(0);
            let k = items.len();
            let (lead, between): (i32, Vec<i32>) = match justify {
                JustifyContent::Start => (0, vec![0; k.saturating_sub(1)]),
                JustifyContent::End => (leftover, vec![0; k.saturating_sub(1)]),
                JustifyContent::Center => (leftover / 2, vec![0; k.saturating_sub(1)]),
                JustifyContent::SpaceBetween => {
                    if k > 1 {
                        (0, distribute(leftover, &vec![1.0; k - 1]))
                    } else {
                        (0, vec![])
                    }
                }
                JustifyContent::SpaceAround => {
                    let mut w = vec![2.0; k + 1];
                    w[0] = 1.0;
                    w[k] = 1.0;
                    let d = distribute(leftover, &w);
                    (d[0], d[1..k].to_vec())
                }
                JustifyContent::SpaceEvenly => {
                    let d = distribute(leftover, &vec![1.0; k + 1]);
                    (d[0], d[1..k].to_vec())
                }
            };
            let main_start = if row {
                r.border.left + r.padding.left
            } else {
                r.border.top + r.padding.top
            };
            let cross_start = if row {
                r.border.top + r.padding.top
            } else {
                r.border.left + r.padding.left
            };
            let container_main = main_inner.unwrap_or(used_main);
            let mut pos = lead;
            let order: Vec<usize> = (0..k).collect();
            for (j, &i) in order.iter().enumerate() {
                let it = &items[i];
                let main_pos = pos + it.margin_main.0;
                let main_pos = if dir.is_reverse() {
                    container_main - main_pos - it.target
                } else {
                    main_pos
                };
                let free_cross = line_cross - it.cross - it.margin_cross.0 - it.margin_cross.1;
                let cross_off = match it.align {
                    AlignItems::Start | AlignItems::Stretch => 0,
                    AlignItems::End => free_cross,
                    AlignItems::Center => free_cross.div_euclid(2),
                };
                let cross_pos = cross_off + it.margin_cross.0;
                let (x, y) = if row {
                    (main_start + main_pos, cross_start + cross_pos)
                } else {
                    (cross_start + cross_pos, main_start + main_pos)
                };
                let (w, h) = wh(Some(it.target), Some(it.cross));
                let child = &mut self.doc.nodes[it.id];
                child.rect.x = x;
                child.rect.y = y;
                let avail = w;
                self.compute(it.id, (w, h), avail, base, true);
                pos += it.margin_main.0 + it.target + it.margin_main.1;
                if j + 1 < k {
                    pos += gap + between[j];
                }
            }
        }

        let line_cross_content = items
            .iter()
            .map(|it| it.cross + it.margin_cross.0 + it.margin_cross.1)
            .max()
            .unwrap_or(0);
        let cross = cross_inner.unwrap_or(line_cross_content);
        if row {
            SizeI::new(used_main, cross)
        } else {
            SizeI::new(cross, used_main)
        }
    }

    /// The min-content size of `id` along the main axis, ignoring its own specified main size
    /// (the "content size suggestion" of CSS automatic minimum sizes).
    fn content_min(
        &mut self,
        id: NodeId,
        row: bool,
        cross_known: Option<i32>,
        inner_avail: Option<i32>,
        base: (Option<i32>, Option<i32>),
    ) -> i32 {
        let saved = self.doc.nodes[id].resolved.clone();
        {
            let r = &mut self.doc.nodes[id].resolved;
            if row {
                r.width = RLen::Auto;
                r.max_width = RLen::Auto;
            } else {
                r.height = RLen::Auto;
                r.max_height = RLen::Auto;
            }
        }
        self.doc.nodes[id].measure_cache.clear();
        let v = if row {
            self.compute(id, (None, cross_known), Some(0), base, false)
                .width
        } else {
            let avail = cross_known.or(inner_avail);
            self.compute(id, (cross_known, None), avail, base, false)
                .height
        };
        self.doc.nodes[id].resolved = saved;
        self.doc.nodes[id].measure_cache.clear();
        v
    }

    /// Lays out a popup at the size it would like, as far as the platform grants it.
    fn layout_popup(&mut self, id: NodeId, window: (Option<i32>, Option<i32>)) {
        let natural = self.compute(id, (None, None), None, window, false);
        let doc = &*self.doc;
        let Some(data) = doc.nodes[id].popup() else {
            return;
        };
        let anchor = data
            .anchor
            .and_then(|a| a.id_untracked())
            .or_else(|| doc.anchor_of(id))
            .and_then(|a| doc.nodes.get(a));
        let width = match anchor {
            Some(a) if data.match_anchor_width => natural.width.max(a.rect.width),
            _ => natural.width,
        };
        let natural = SizeI::new(width, natural.height);
        let granted = data.granted.map_or(natural, |g| {
            SizeI::new(
                natural.width.min(self.scale.px(g.width as f32)),
                natural.height.min(self.scale.px(g.height as f32)),
            )
        });
        self.doc.nodes[id].popup_mut().unwrap().natural = natural;
        let known = (Some(granted.width), Some(granted.height));
        self.compute(id, known, Some(granted.width), window, true);
    }

    fn layout_absolute(&mut self, id: NodeId, size: SizeI) {
        let (_, absolute) = self.flow_children(id);
        if absolute.is_empty() {
            return;
        }
        let r = self.doc.nodes[id].resolved.clone();
        let pw = (size.width - r.border.left - r.border.right).max(0);
        let ph = (size.height - r.border.top - r.border.bottom).max(0);
        for c in absolute {
            let cr = self.doc.nodes[c].resolved.clone();
            let m = self.resolve_margins(&cr, Some(pw));
            let left = cr.inset.left.resolve(Some(pw));
            let right = cr.inset.right.resolve(Some(pw));
            let top = cr.inset.top.resolve(Some(ph));
            let bottom = cr.inset.bottom.resolve(Some(ph));
            let w = cr.width.resolve(Some(pw)).or_else(|| match (left, right) {
                (Some(l), Some(rr)) => Some((pw - l - rr - m.left - m.right).max(0)),
                _ => None,
            });
            let h = cr.height.resolve(Some(ph)).or_else(|| match (top, bottom) {
                (Some(t), Some(b)) => Some((ph - t - b - m.top - m.bottom).max(0)),
                _ => None,
            });
            let s = self.compute(c, (w, h), Some(pw), (Some(pw), Some(ph)), false);
            let x = match (left, right) {
                (Some(l), _) => r.border.left + l + m.left,
                (None, Some(rr)) => size.width - r.border.right - rr - s.width - m.right,
                (None, None) => r.border.left + r.padding.left + m.left,
            };
            let y = match (top, bottom) {
                (Some(t), _) => r.border.top + t + m.top,
                (None, Some(b)) => size.height - r.border.bottom - b - s.height - m.bottom,
                (None, None) => r.border.top + r.padding.top + m.top,
            };
            let child = &mut self.doc.nodes[c];
            child.rect.x = x;
            child.rect.y = y;
            self.compute(
                c,
                (Some(s.width), Some(s.height)),
                Some(pw),
                (Some(pw), Some(ph)),
                true,
            );
        }
    }

    /// Records the extent of the children relative to the padding box, for scrolling.
    fn update_content_size(&mut self, id: NodeId) {
        let (flow, absolute) = self.flow_children(id);
        let r = self.doc.nodes[id].resolved.clone();
        let mut right = 0;
        let mut bottom = 0;
        for c in flow.into_iter().chain(absolute) {
            // Children of fragments are positioned relative to `id` as well.
            let n = &self.doc.nodes[c];
            let m = n.resolved.margin.map(|v| v.resolve(None).unwrap_or(0));
            right = right.max(n.rect.right() + m.right - r.border.left);
            bottom = bottom.max(n.rect.bottom() + m.bottom - r.border.top);
        }
        let node = &mut self.doc.nodes[id];
        if node.is_edit() {
            return;
        }
        node.content_size = SizeI::new(right + r.padding.right, bottom + r.padding.bottom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribute_equal_monotone() {
        assert_eq!(distribute(10, &[1.0, 1.0, 1.0]), vec![4, 3, 3]);
        assert_eq!(distribute(11, &[1.0, 1.0, 1.0]), vec![4, 4, 3]);
        assert_eq!(distribute(-10, &[1.0, 1.0, 1.0]), vec![-4, -3, -3]);
        for total in 0..200 {
            let a = distribute(total, &[1.0, 1.0, 1.0, 1.0]);
            let b = distribute(total + 1, &[1.0, 1.0, 1.0, 1.0]);
            assert_eq!(a.iter().sum::<i32>(), total);
            assert!(a.iter().zip(&b).all(|(x, y)| x <= y));
        }
    }

    #[test]
    fn distribute_weighted_sums_and_monotone() {
        let w = [1.0, 2.0, 3.5, 0.0, 1.25];
        let mut prev = distribute(0, &w);
        for total in 1..500 {
            let d = distribute(total, &w);
            assert_eq!(d.iter().sum::<i32>(), total);
            assert_eq!(d[3], 0);
            assert!(
                prev.iter().zip(&d).all(|(x, y)| x <= y),
                "not monotone at {total}"
            );
            prev = d;
        }
    }

    #[test]
    fn distribute_zero_weights() {
        assert_eq!(distribute(5, &[0.0, 0.0]), vec![0, 0]);
        assert_eq!(distribute(5, &[]), Vec::<i32>::new());
    }
}
