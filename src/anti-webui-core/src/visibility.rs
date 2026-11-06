//! Which contents `content-visibility: auto` skips. It is decided after the nodes are positioned,
//! and what changes is laid out again in the same update, so no frame shows a box without its
//! contents. See `docs/design/29-content-visibility.md`.

use crate::geometry::RectI;
use crate::tree::{Document, NodeId};
use anti_webui_style::{ContentVisibility, Overflow};
use std::collections::HashSet;

/// How many updates a node to scroll to waits for the contents around it to be laid out.
const WAITS: u8 = 8;

#[derive(Default)]
pub(crate) struct Visibility {
    /// The nodes with `content-visibility: auto`.
    pub auto: HashSet<NodeId>,
    /// What they depend on may have changed: where things are, scrolling, the window.
    pub check: bool,
    /// The ends of the selection and the focus they were last checked with.
    last: (Option<(NodeId, NodeId)>, Option<NodeId>),
    /// The lines with the caret of a `text_editor`, which are laid out wherever they are.
    carets: Vec<NodeId>,
    /// For each scroller with contents that change: a node in its view, and how far down its
    /// content that is, which stays the same.
    anchors: Vec<(NodeId, NodeId, i32)>,
    /// How long nodes to scroll to have waited.
    waited: u8,
}

/// Decides again which contents are skipped, and marks what changed to be laid out again.
/// Returns whether anything did.
pub(crate) fn check(doc: &mut Document) -> bool {
    let inputs = (
        doc.state
            .selection
            .as_ref()
            .map(|s| (s.anchor.node, s.focus.node)),
        doc.state.focused,
    );
    let carets = crate::text::editor::caret_rows(doc);
    if inputs != doc.visibility.last || carets != doc.visibility.carets {
        doc.visibility.last = inputs;
        doc.visibility.carets = carets.clone();
        doc.visibility.check = true;
    }
    if !std::mem::take(&mut doc.visibility.check) || doc.visibility.auto.is_empty() {
        return false;
    }
    let nodes = &doc.nodes;
    let order = |id: NodeId| nodes.get(id).map(|n| n.doc_order);
    let selected = inputs
        .0
        .and_then(|(a, b)| Some((order(a)?, order(b)?)))
        .map(|(a, b)| (a.min(b), a.max(b)));
    let focused = inputs.1.and_then(order);
    let targets: Vec<u32> = doc
        .pending_scroll
        .iter()
        .chain(&carets)
        .filter_map(|&id| order(id))
        .collect();
    let window = RectI::new(0, 0, doc.viewport.width, doc.viewport.height);
    let margin = (doc.viewport.width / 2, doc.viewport.height / 2);
    doc.visibility.auto.retain(|id| nodes.contains_key(*id));
    let mut flips = Vec::new();
    for &id in &doc.visibility.auto {
        let n = &doc.nodes[id];
        // Inside contents that are skipped, it waits until they aren't.
        if !n.visible || n.computed.content_visibility != ContentVisibility::Auto {
            continue;
        }
        let (lo, hi) = (n.doc_order, n.doc_order + n.subtree_nodes.max(1));
        let inside = |o: u32| (lo..hi).contains(&o);
        let relevant = focused.is_some_and(inside)
            || selected.is_some_and(|(a, b)| a < hi && b >= lo)
            || targets.iter().any(|&o| inside(o))
            || {
                let near = RectI::new(
                    n.abs.x - margin.0,
                    n.abs.y - margin.1,
                    n.abs.width + 2 * margin.0,
                    n.abs.height + 2 * margin.1,
                );
                !doc.visible_part(n, near).intersect(&window).is_empty()
            };
        if relevant != n.relevant {
            flips.push((id, relevant));
        }
    }
    let mut scrollers: Vec<NodeId> = flips
        .iter()
        .filter_map(|&(id, _)| scroller_of(doc, id))
        .collect();
    scrollers.sort_unstable();
    scrollers.dedup();
    for s in scrollers {
        if !doc.visibility.anchors.iter().any(|a| a.0 == s)
            && let Some(anchor) = anchor_in(doc, s)
            && let Some(y) = offset_in(doc, anchor, s)
        {
            doc.visibility.anchors.push((s, anchor, y));
        }
    }
    for &(id, relevant) in &flips {
        doc.nodes[id].relevant = relevant;
        doc.invalidate_layout(id);
    }
    !flips.is_empty()
}

/// What scrolls `id` up and down.
fn scroller_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    doc.ancestors(id)
        .into_iter()
        .skip(1)
        .find(|&a| doc.nodes[a].computed.overflow_y == Overflow::Scroll)
}

/// The first node of contents that are laid out that shows in the view of `scroller`, which stays
/// where it is while what is around it takes its real size (CSS scroll anchoring).
fn anchor_in(doc: &Document, scroller: NodeId) -> Option<NodeId> {
    let s = doc.nodes.get(scroller)?;
    let view = doc.rect_to_window(s, s.padding_abs());
    doc.visibility
        .auto
        .iter()
        .copied()
        .filter(|&id| {
            let n = &doc.nodes[id];
            n.visible
                && n.relevant
                && !n.skipped
                && doc.rect_to_window(n, n.abs).intersects(&view)
                && scroller_of(doc, id) == Some(scroller)
        })
        .min_by_key(|&id| doc.nodes[id].doc_order)
}

/// How far down the content of `scroller` the top of `id` is, as of the last layout.
fn offset_in(doc: &Document, id: NodeId, scroller: NodeId) -> Option<i32> {
    let mut y = 0;
    let mut at = id;
    while at != scroller {
        let n = doc.nodes.get(at)?;
        y += n.rect.y;
        at = n.parent?;
    }
    Some(y)
}

/// After layout: what is around the anchors took its real size, so their scrollers move by as much
/// as they did, and what shows stays where it was.
pub(crate) fn keep_in_place(doc: &mut Document) {
    for (scroller, anchor, before) in std::mem::take(&mut doc.visibility.anchors) {
        let Some(after) = offset_in(doc, anchor, scroller) else {
            continue;
        };
        if after != before
            && let Some(s) = doc.nodes.get_mut(scroller)
        {
            s.scroll.y += after - before;
            doc.mark_scrolled(scroller);
        }
    }
}

/// Whether a node to scroll to has to wait for the contents around it to be laid out, which
/// `check` sees to.
pub(crate) fn must_wait(doc: &mut Document, id: NodeId) -> bool {
    let mut wait = false;
    for a in doc.ancestors(id).into_iter().skip(1) {
        let n = &doc.nodes[a];
        if n.skipped {
            if n.computed.content_visibility != ContentVisibility::Auto {
                return false;
            }
            wait = true;
        }
    }
    let v = &mut doc.visibility;
    if wait && v.waited < WAITS {
        v.waited += 1;
        v.check = true;
        true
    } else {
        v.waited = 0;
        false
    }
}
