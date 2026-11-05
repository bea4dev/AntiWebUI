//! The document-wide text selection (see `docs/design/04-text.md`).

use crate::geometry::{PointF, PointI, RectI};
use crate::layout::child_clip;
use crate::tree::{Document, NodeId, NodeKind};
use anti_webui_style::UserSelect;
use std::cmp::Ordering;

/// A position inside a text node (byte offset at a cluster boundary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPoint {
    pub node: NodeId,
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Granularity {
    #[default]
    Char,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocSelection {
    pub anchor: TextPoint,
    pub focus: TextPoint,
    pub granularity: Granularity,
    /// The unit (word or line) under the initial click, kept while extending by that unit.
    pub initial: (TextPoint, TextPoint),
}

impl DocSelection {
    pub fn collapsed(p: TextPoint) -> Self {
        Self {
            anchor: p,
            focus: p,
            granularity: Granularity::Char,
            initial: (p, p),
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

pub fn compare(doc: &Document, a: TextPoint, b: TextPoint) -> Ordering {
    let oa = doc.nodes.get(a.node).map(|n| n.doc_order).unwrap_or(0);
    let ob = doc.nodes.get(b.node).map(|n| n.doc_order).unwrap_or(0);
    oa.cmp(&ob).then(a.offset.cmp(&b.offset))
}

/// `(start, end)` in document order.
pub fn ordered(doc: &Document, sel: &DocSelection) -> (TextPoint, TextPoint) {
    if compare(doc, sel.anchor, sel.focus) == Ordering::Greater {
        (sel.focus, sel.anchor)
    } else {
        (sel.anchor, sel.focus)
    }
}

/// Selectable, visible text nodes in document order.
pub fn selectable_text_nodes(doc: &Document) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .nodes
        .iter()
        .filter(|(_, n)| n.visible && n.is_text() && n.computed.user_select == UserSelect::Text)
        .map(|(id, _)| id)
        .collect();
    out.sort_by_key(|id| doc.nodes[*id].doc_order);
    out
}

fn text_len(doc: &Document, id: NodeId) -> usize {
    match &doc.nodes[id].kind {
        NodeKind::Text(t) => t.text.len(),
        _ => 0,
    }
}

/// The part of the selection inside `node`, as a byte range.
pub fn node_range(doc: &Document, sel: &DocSelection, node: NodeId) -> Option<(usize, usize)> {
    let (start, end) = ordered(doc, sel);
    let order = doc.nodes.get(node)?.doc_order;
    let so = doc.nodes.get(start.node)?.doc_order;
    let eo = doc.nodes.get(end.node)?.doc_order;
    if order < so || order > eo {
        return None;
    }
    let len = text_len(doc, node);
    let a = if node == start.node { start.offset } else { 0 };
    let b = if node == end.node { end.offset } else { len };
    (a < b).then_some((a, b.min(len)))
}

/// Finds the text position nearest to `p`, the way browsers do while dragging a selection:
/// the point may be outside of any text, between lines, or far away from the anchor.
pub fn hit_text_point(doc: &Document, p: PointF) -> Option<TextPoint> {
    let mut best = None;
    let (root, offset) = match doc.popup_at(p) {
        Some((popup, _)) => (popup, doc.nodes[popup].popup()?.origin),
        None => (doc.shown_root()?, PointI::default()),
    };
    nearest_text(doc, root, offset, RectI::INFINITE, p, &mut best);
    let (_, id, at): ((f32, f32), NodeId, PointF) = best?;
    let n = &doc.nodes[id];
    let NodeKind::Text(t) = &n.kind else {
        return None;
    };
    let (offset, _) = t.layout.hit(at.x, at.y);
    Some(TextPoint { node: id, offset })
}

/// Looks through the selectable text that isn't clipped away, in document order. `offset` takes
/// the node's coordinates to window coordinates, and `view` is the window rectangle left by the
/// scroll root it is in. What is found comes with the closest point relative to the text.
fn nearest_text(
    doc: &Document,
    id: NodeId,
    offset: PointI,
    view: RectI,
    p: PointF,
    best: &mut Option<((f32, f32), NodeId, PointF)>,
) {
    let Some(n) = doc.nodes.get(id) else {
        return;
    };
    if !n.visible {
        return;
    }
    // `offset` and `view` say nothing about a transformed space.
    let transformed = n.space.is_some_and(|s| doc.nodes[s].transformed);
    if n.is_text() && n.computed.user_select == UserSelect::Text {
        let vis = if transformed {
            doc.visible_window_rect(id).unwrap_or_default()
        } else {
            let clip = n.clip_shape().translated(offset.x, offset.y).rect;
            n.abs
                .translate(offset.x, offset.y)
                .intersect(&clip)
                .intersect(&view)
        };
        if !vis.is_empty() {
            let (dx, dy) = vis.axis_distance(p);
            let key = (dy, dx);
            if best.as_ref().is_none_or(|(k, _, _)| key < *k) {
                let q = vis.clamp_point(p);
                let at = if transformed {
                    let q = doc.from_window(n, q).unwrap_or(q);
                    PointF::new(q.x - n.abs.x as f32, q.y - n.abs.y as f32)
                } else {
                    PointF::new(
                        q.x - (n.abs.x + offset.x) as f32,
                        q.y - (n.abs.y + offset.y) as f32,
                    )
                };
                *best = Some((key, id, at));
            }
        }
    }
    let (offset, view) = if n.scroll_root {
        let clip = child_clip(n, doc.scale, n.clip_shape());
        (
            PointI::new(
                offset.x + n.abs.x - n.scroll.x,
                offset.y + n.abs.y - n.scroll.y,
            ),
            view.intersect(&clip.rect.translate(offset.x, offset.y)),
        )
    } else {
        (offset, view)
    };
    for (i, &c) in n.children.iter().enumerate() {
        let touches = transformed
            || n.child_bounds
                .get(i)
                .is_none_or(|b| b.translate(offset.x, offset.y).intersects(&view));
        if touches {
            nearest_text(doc, c, offset, view, p, best);
        }
    }
}

fn unit_at(
    doc: &Document,
    p: TextPoint,
    at: PointF,
    granularity: Granularity,
) -> (TextPoint, TextPoint) {
    let n = &doc.nodes[p.node];
    let NodeKind::Text(t) = &n.kind else {
        return (p, p);
    };
    let vis = doc.visible_window_rect(p.node).unwrap_or_default();
    let q = vis.clamp_point(at);
    let q = doc.from_window(n, q).unwrap_or(q);
    let (x, y) = (q.x - n.abs.x as f32, q.y - n.abs.y as f32);
    let (a, b) = match granularity {
        Granularity::Char => (p.offset, p.offset),
        Granularity::Word => t.layout.word_at(x, y),
        Granularity::Line => t.layout.hard_line_at(x, y),
    };
    (
        TextPoint {
            node: p.node,
            offset: a,
        },
        TextPoint {
            node: p.node,
            offset: b,
        },
    )
}

/// Starts a selection at `at` with the given granularity (1, 2 or 3 clicks).
pub fn begin(doc: &Document, at: PointF, granularity: Granularity) -> Option<DocSelection> {
    let p = hit_text_point(doc, at)?;
    let (a, b) = unit_at(doc, p, at, granularity);
    Some(DocSelection {
        anchor: a,
        focus: b,
        granularity,
        initial: (a, b),
    })
}

/// Extends the selection to `at`, snapping to the selection's granularity.
pub fn extend(doc: &Document, sel: &DocSelection, at: PointF) -> Option<DocSelection> {
    let p = hit_text_point(doc, at)?;
    let (ua, ub) = unit_at(doc, p, at, sel.granularity);
    let (ia, ib) = sel.initial;
    let mut next = *sel;
    if sel.granularity == Granularity::Char {
        next.focus = p;
    } else if compare(doc, ua, ia) == Ordering::Less {
        next.anchor = ib;
        next.focus = ua;
    } else {
        next.anchor = ia;
        next.focus = ub;
    }
    Some(next)
}

/// Extends from the current anchor (Shift+click).
pub fn extend_from_anchor(doc: &Document, sel: &DocSelection, at: PointF) -> Option<DocSelection> {
    let p = hit_text_point(doc, at)?;
    let mut next = *sel;
    next.focus = p;
    next.granularity = Granularity::Char;
    next.initial = (sel.anchor, sel.anchor);
    Some(next)
}

/// Selects every selectable text node.
pub fn select_all(doc: &Document) -> Option<DocSelection> {
    let nodes = selectable_text_nodes(doc);
    let first = *nodes.first()?;
    let last = *nodes.last()?;
    let a = TextPoint {
        node: first,
        offset: 0,
    };
    let b = TextPoint {
        node: last,
        offset: text_len(doc, last),
    };
    Some(DocSelection {
        anchor: a,
        focus: b,
        granularity: Granularity::Char,
        initial: (a, a),
    })
}

/// The selected text. Nodes on different visual lines are joined with `\n`, nodes on the same
/// line with a space.
pub fn selected_text(doc: &Document, sel: &DocSelection) -> String {
    let mut out = String::new();
    let mut prev: Option<RectI> = None;
    for id in selectable_text_nodes(doc) {
        let Some((a, b)) = node_range(doc, sel, id) else {
            continue;
        };
        let n = &doc.nodes[id];
        let NodeKind::Text(t) = &n.kind else { continue };
        let rect = doc.window_rect(id).unwrap_or_default();
        if let Some(p) = prev {
            out.push(if rect.y >= p.bottom() - 1 { '\n' } else { ' ' });
        }
        out.push_str(&t.text[a..b]);
        prev = Some(rect);
    }
    out
}
