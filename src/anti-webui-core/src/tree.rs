//! The retained node tree (a small DOM) and the per-window document state.

use crate::event::{KeyboardEvent, PointerEvent, ScrollEvent};
use crate::geometry::{PointF, PointI, RectI, Scale, SizeI};
use crate::layout::{MeasureCache, MeasureKey, Resolved, child_clip};
use crate::platform::{Placement, PlatformRequest, PopupId, PopupKind, PopupParams};
use crate::scene::Scene;
use crate::text::TextLayout;
use crate::text::edit::EditData;
use crate::text::selection::DocSelection;
use crate::view::NodeRef;
use anti_webui_style::{ComputedStyle, Display, Style};
use slotmap::{Key, KeyData, SlotMap, new_key_type};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

new_key_type! {
    /// Identifies a node in the UI tree.
    pub struct NodeId;
}

pub type Handler<E> = Rc<dyn Fn(&E)>;

#[derive(Default, Clone)]
pub struct Listeners {
    pub click: Option<Handler<PointerEvent>>,
    pub pointer_down: Option<Handler<PointerEvent>>,
    pub pointer_up: Option<Handler<PointerEvent>>,
    pub pointer_move: Option<Handler<PointerEvent>>,
    pub hover: Option<Rc<dyn Fn(bool)>>,
    pub key_down: Option<Handler<KeyboardEvent>>,
    pub key_up: Option<Handler<KeyboardEvent>>,
    pub scroll: Option<Handler<ScrollEvent>>,
    pub focus: Option<Rc<dyn Fn(bool)>>,
}

/// Style layers, applied in order: base → dynamic → hover → focus → active.
#[derive(Default)]
pub struct NodeStyle {
    pub base: Style,
    pub dynamic: Vec<Style>,
    pub hover: Option<Style>,
    pub focus: Option<Style>,
    pub active: Option<Style>,
}

pub struct TextData {
    pub text: String,
    pub version: u64,
    pub layout: TextLayout,
}

/// What a popup's surface needs to hold its box and what sticks out of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PopupSurface {
    pub buffer: SizeI,
    pub logical: SizeI,
    /// The box, in logical pixels.
    pub geometry: RectI,
}

#[derive(Default)]
pub struct PopupData {
    pub kind: PopupKind,
    pub placement: Placement,
    /// Logical pixels between the anchor and the popup.
    pub gap: i32,
    /// What it hangs from; by default the closest ancestor with a box.
    pub anchor: Option<NodeRef>,
    /// Takes precedence over `anchor`, in window coordinates.
    pub anchor_rect: Option<RectI>,
    /// At least as wide as the anchor.
    pub match_anchor_width: bool,
    pub on_dismiss: Option<Rc<dyn Fn()>>,

    /// The size it would like to have, in physical pixels.
    pub natural: SizeI,
    pub surface: PopupSurface,
    /// The last request to the platform.
    pub sent: Option<PopupParams>,
    /// The origin of its surface in window coordinates, once the platform placed it.
    pub origin: PointI,
    /// The logical size the platform granted to its box.
    pub granted: Option<SizeI>,
    /// Closed for good: only building it again brings it back.
    pub dismissed: bool,
}

impl PopupData {
    /// Whether it is on the screen: it has a surface in a known place.
    pub fn shown(&self) -> bool {
        self.sent.is_some() && self.granted.is_some() && !self.dismissed
    }
}

pub fn popup_id(id: NodeId) -> PopupId {
    PopupId(id.data().as_ffi())
}

pub fn popup_node(id: PopupId) -> NodeId {
    KeyData::from_ffi(id.0).into()
}

#[allow(
    clippy::large_enum_variant,
    reason = "text nodes are the common case; boxing them would add an indirection per paint"
)]
pub enum NodeKind {
    Element,
    Text(TextData),
    Edit(Box<EditData>),
    /// Generates no box; children take part in the parent's layout.
    Fragment,
    /// The root of what is shown in a surface of its own. It stays in the tree for its lifetime,
    /// inherited styles and events, and is out of the way for layout and painting.
    Popup(Box<PopupData>),
}

pub struct Node {
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub kind: NodeKind,
    pub style: NodeStyle,
    pub listeners: Listeners,
    pub focusable: bool,
    pub disabled: bool,

    pub computed: ComputedStyle,
    pub resolved: Resolved,
    /// Border box relative to the parent's border box origin (before scrolling).
    pub rect: RectI,
    /// Extent of the children relative to the padding box origin, for scrolling.
    pub content_size: SizeI,
    /// Border box after scrolling, in window coordinates, or in content coordinates for the
    /// descendants of a scroll root (see [`Document::window_rect`]).
    pub abs: RectI,
    /// Clip that applies to this node, in the same coordinates as `abs`. For the descendants of a
    /// scroll root it leaves out the root's own clip.
    pub clip: RectI,
    /// Corner radii of `clip` (non-zero when an ancestor with rounded corners clips).
    pub clip_radii: [f32; 4],
    pub scroll: PointI,
    pub doc_order: u32,
    /// False inside `display: none` subtrees.
    pub visible: bool,
    /// The outermost scroller that overflows: its descendants don't move when it scrolls.
    pub scroll_root: bool,
    /// The scroll root whose content this node is part of, or the popup whose surface it is on.
    pub space: Option<NodeId>,
    /// Painted from its stacking context's list instead of in tree order.
    pub stacked: bool,
    /// Its stacked descendants are ordered among themselves, and stay above its background.
    pub stacking_context: bool,
    /// Contains everything the subtree paints, in the same coordinates as `abs`. It may be too
    /// big, never too small.
    pub subtree_bounds: RectI,
    /// The `subtree_bounds` of each child, so that culling doesn't need to touch the children.
    pub child_bounds: Vec<RectI>,
    /// Nodes in the subtree that are not `display: none`, itself included.
    pub(crate) subtree_nodes: u32,
    /// The children may have moved since the node was last positioned.
    pub(crate) position_dirty: bool,
    /// The node's style inputs changed since it was last resolved.
    pub(crate) style_dirty: bool,
    /// The node or a descendant changed since it was last laid out.
    pub(crate) layout_dirty: bool,
    /// Inputs and result of the last layout pass, reused while the subtree is clean.
    pub(crate) last_perform: Option<(MeasureKey, SizeI)>,
    pub(crate) measure_cache: MeasureCache,
}

impl Node {
    pub fn new(kind: NodeKind) -> Self {
        Self {
            parent: None,
            children: Vec::new(),
            kind,
            style: NodeStyle::default(),
            listeners: Listeners::default(),
            focusable: false,
            disabled: false,
            computed: ComputedStyle::default(),
            resolved: Resolved::default(),
            rect: RectI::default(),
            content_size: SizeI::default(),
            abs: RectI::default(),
            clip: RectI::INFINITE,
            clip_radii: [0.0; 4],
            scroll: PointI::default(),
            doc_order: 0,
            visible: false,
            scroll_root: false,
            space: None,
            stacked: false,
            stacking_context: false,
            subtree_bounds: RectI::default(),
            child_bounds: Vec::new(),
            subtree_nodes: 0,
            position_dirty: true,
            style_dirty: false,
            layout_dirty: true,
            last_perform: None,
            measure_cache: MeasureCache::default(),
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, NodeKind::Text(_))
    }

    pub fn is_edit(&self) -> bool {
        matches!(self.kind, NodeKind::Edit(_))
    }

    pub fn is_popup(&self) -> bool {
        matches!(self.kind, NodeKind::Popup(_))
    }

    pub fn popup(&self) -> Option<&PopupData> {
        match &self.kind {
            NodeKind::Popup(p) => Some(p),
            _ => None,
        }
    }

    pub fn popup_mut(&mut self) -> Option<&mut PopupData> {
        match &mut self.kind {
            NodeKind::Popup(p) => Some(p),
            _ => None,
        }
    }

    pub fn edit(&self) -> Option<&EditData> {
        match &self.kind {
            NodeKind::Edit(e) => Some(e),
            _ => None,
        }
    }

    pub fn edit_mut(&mut self) -> Option<&mut EditData> {
        match &mut self.kind {
            NodeKind::Edit(e) => Some(e),
            _ => None,
        }
    }

    /// The content box, in the same coordinates as `abs`.
    pub fn content_abs(&self) -> RectI {
        let r = &self.resolved;
        self.abs.inset(
            r.border.top + r.padding.top,
            r.border.right + r.padding.right,
            r.border.bottom + r.padding.bottom,
            r.border.left + r.padding.left,
        )
    }

    /// The clip that applies to this node.
    pub fn clip_shape(&self) -> crate::scene::Clip {
        crate::scene::Clip {
            rect: self.clip,
            radii: self.clip_radii,
        }
    }

    /// The padding box, in the same coordinates as `abs`.
    pub fn padding_abs(&self) -> RectI {
        let b = &self.resolved.border;
        self.abs.inset(b.top, b.right, b.bottom, b.left)
    }
}

/// What needs recomputing before the next frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dirty {
    pub style: bool,
    pub layout: bool,
    pub position: bool,
    pub paint: bool,
    /// What scroll layers hold is stale. Scrolling a layer alone doesn't set this.
    pub content: bool,
    /// An edit's caret moved and may need scrolling into view.
    pub carets: bool,
}

/// Per-window interaction state.
#[derive(Default)]
pub struct WindowState {
    /// Hovered nodes from the innermost outwards.
    pub hovered: Vec<NodeId>,
    /// The pressed node and its ancestors while the primary button is down.
    pub active: Vec<NodeId>,
    pub focused: Option<NodeId>,
    pub selection: Option<DocSelection>,
    pub window_focused: bool,
}

pub struct Document {
    pub nodes: SlotMap<NodeId, Node>,
    pub root: Option<NodeId>,
    pub state: WindowState,
    pub dirty: Dirty,
    pub scale: Scale,
    pub viewport: SizeI,
    pub requests: Vec<PlatformRequest>,
    /// Programmatic focus change requested through a `NodeRef`.
    pub pending_focus: Option<Option<NodeId>>,
    /// What each scroll layer painted last and the rectangle it covers, reused while the content
    /// is unchanged and the view stays inside the rectangle.
    pub layer_contents: HashMap<NodeId, (Arc<Scene>, RectI)>,
    /// Nodes whose style must be resolved again; see `style_all`.
    pub style_dirty: Vec<NodeId>,
    /// Every node's style must be resolved again.
    pub style_all: bool,
    /// Nodes whose descendants must be positioned again, short of a whole `dirty.position`.
    pub reposition: Vec<NodeId>,
    /// All edit nodes, which are few, to look through for pending caret scrolling.
    pub edits: Vec<NodeId>,
    /// All nodes with `stacked` set.
    pub stacked: HashSet<NodeId>,
    /// The stacked descendants of each stacking context that has any, in paint order. See
    /// `layout::build_stacking`.
    pub stacking: HashMap<NodeId, StackingList>,
    /// `stacking` must be built again.
    pub stacking_dirty: bool,
    /// All popups, parents before what is attached to them once `stacking` is built.
    pub popups: Vec<NodeId>,
    /// Nodes to bring into view after the next layout.
    pub pending_scroll: Vec<NodeId>,
}

/// What a stacking context paints above its own background, besides its children in tree order.
#[derive(Default)]
pub struct StackingList {
    pub nodes: Vec<NodeId>,
    /// `nodes[..negative]` go below the children.
    pub negative: usize,
}

thread_local! {
    static DOC: RefCell<Document> = RefCell::new(Document {
        nodes: SlotMap::with_key(),
        root: None,
        state: WindowState::default(),
        dirty: Dirty::default(),
        scale: Scale::ONE,
        viewport: SizeI::default(),
        requests: Vec::new(),
        pending_focus: None,
        layer_contents: HashMap::new(),
        style_dirty: Vec::new(),
        style_all: false,
        reposition: Vec::new(),
        edits: Vec::new(),
        stacked: HashSet::new(),
        stacking: HashMap::new(),
        stacking_dirty: false,
        popups: Vec::new(),
        pending_scroll: Vec::new(),
    });
}

/// Borrows the document. Never write signals or call user handlers inside `f`.
pub fn with_doc<R>(f: impl FnOnce(&mut Document) -> R) -> R {
    DOC.with(|d| f(&mut d.borrow_mut()))
}

impl Document {
    /// The stacked descendants of `id` that paint below and above its children.
    pub fn stacked_around(&self, id: NodeId) -> (&[NodeId], &[NodeId]) {
        match self.nodes.get(id).filter(|n| n.stacking_context) {
            Some(_) => self
                .stacking
                .get(&id)
                .map_or((&[][..], &[][..]), |l| l.nodes.split_at(l.negative)),
            None => (&[][..], &[][..]),
        }
    }

    pub fn create(&mut self, node: Node) -> NodeId {
        let edit = node.is_edit();
        let popup = node.is_popup();
        let id = self.nodes.insert(node);
        if edit {
            self.edits.push(id);
        }
        if popup {
            self.popups.push(id);
        }
        id
    }

    /// Whether `id` hangs from the root.
    pub fn attached(&self, id: NodeId) -> bool {
        let mut cur = id;
        while let Some(n) = self.nodes.get(cur) {
            match n.parent {
                Some(p) => cur = p,
                None => return self.root == Some(cur),
            }
        }
        false
    }

    /// What a popup hangs from when nothing else is said: the closest ancestor with a box.
    pub fn anchor_of(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = self.nodes.get(id)?.parent;
        while let Some(n) = cur.and_then(|c| self.nodes.get(c)) {
            if !matches!(n.kind, NodeKind::Fragment) && n.computed.display != Display::Contents {
                return cur;
            }
            cur = n.parent;
        }
        None
    }

    /// The popup whose surface `id` is on.
    pub fn popup_of(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = Some(id);
        while let Some(n) = cur.and_then(|c| self.nodes.get(c)) {
            if n.is_popup() {
                return cur;
            }
            cur = n.parent;
        }
        None
    }

    /// The popup that holds `p`, a point in window coordinates, and `p` in its surface's
    /// coordinates. Input goes through tooltips.
    pub fn popup_at(&self, p: PointF) -> Option<(NodeId, PointF)> {
        self.popups.iter().rev().find_map(|&id| {
            let n = self.nodes.get(id)?;
            let data = n
                .popup()
                .filter(|d| d.shown() && d.kind != PopupKind::Tooltip)?;
            let local = PointF::new(p.x - data.origin.x as f32, p.y - data.origin.y as f32);
            n.abs.contains(local).then_some((id, local))
        })
    }

    /// What to add to a node's coordinates to get window coordinates.
    pub fn space_offset(&self, node: &Node) -> PointI {
        let mut offset = PointI::default();
        let mut space = node.space.and_then(|r| self.nodes.get(r));
        while let Some(r) = space {
            if let Some(popup) = r.popup() {
                offset.x += popup.origin.x;
                offset.y += popup.origin.y;
                break;
            }
            offset.x += r.abs.x - r.scroll.x;
            offset.y += r.abs.y - r.scroll.y;
            space = r.space.and_then(|s| self.nodes.get(s));
        }
        offset
    }

    /// The node's border box in window coordinates.
    pub fn window_rect(&self, id: NodeId) -> Option<RectI> {
        let n = self.nodes.get(id)?;
        let o = self.space_offset(n);
        Some(n.abs.translate(o.x, o.y))
    }

    /// The part of the node that is not clipped away, in window coordinates.
    pub fn visible_window_rect(&self, id: NodeId) -> Option<RectI> {
        let n = self.nodes.get(id)?;
        let o = self.space_offset(n);
        let clip = n.clip_shape().translated(o.x, o.y).rect;
        let rect = n.abs.translate(o.x, o.y).intersect(&clip);
        Some(match n.space.and_then(|r| self.nodes.get(r)) {
            Some(root) if !root.is_popup() => {
                let o = self.space_offset(root);
                let clip = child_clip(root, self.scale, root.clip_shape());
                rect.intersect(&clip.rect.translate(o.x, o.y))
            }
            _ => rect,
        })
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Styles need recomputing. Layout runs only if a layout-affecting property changed.
    /// The style inputs of `id` changed.
    pub fn mark_style_of(&mut self, id: NodeId) {
        let Some(n) = self.nodes.get_mut(id) else {
            return;
        };
        if !n.style_dirty {
            n.style_dirty = true;
            self.style_dirty.push(id);
        }
        self.dirty.style = true;
        self.dirty.paint = true;
        self.dirty.content = true;
    }

    /// Something every style depends on changed.
    pub fn mark_style_all(&mut self) {
        self.style_all = true;
        self.dirty.style = true;
        self.dirty.paint = true;
        self.dirty.content = true;
    }

    /// Marks `id` and its ancestors for re-layout; clean subtrees keep their cached layout.
    pub fn invalidate_layout(&mut self, id: NodeId) {
        let mut cur = Some(id);
        while let Some(i) = cur {
            let Some(n) = self.nodes.get_mut(i) else {
                break;
            };
            n.layout_dirty = true;
            n.position_dirty = true;
            n.measure_cache.clear();
            // Nothing outside of a popup depends on its size.
            cur = if n.is_popup() { None } else { n.parent };
        }
        self.mark_layout();
    }

    /// Discards every cached layout result.
    pub fn invalidate_all_layout(&mut self) {
        for (_, n) in self.nodes.iter_mut() {
            n.layout_dirty = true;
            n.measure_cache.clear();
        }
        self.mark_layout();
    }

    pub fn mark_layout(&mut self) {
        self.dirty.layout = true;
        self.dirty.paint = true;
        self.dirty.content = true;
    }

    pub fn mark_paint(&mut self) {
        self.dirty.paint = true;
        self.dirty.content = true;
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        self.stacking_dirty = true;
        if let Some(c) = self.nodes.get_mut(child) {
            c.parent = Some(parent);
        }
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children.push(child);
        }
        self.mark_style_of(child);
        self.invalidate_layout(parent);
    }

    pub fn set_children(&mut self, parent: NodeId, children: Vec<NodeId>) {
        self.stacking_dirty = true;
        let old = match self.nodes.get_mut(parent) {
            Some(p) => std::mem::take(&mut p.children),
            None => return,
        };
        for c in &old {
            if let Some(n) = self.nodes.get_mut(*c)
                && n.parent == Some(parent)
            {
                n.parent = None;
            }
        }
        let old: HashSet<NodeId> = old.into_iter().collect();
        for &c in &children {
            self.detach(c);
            if let Some(n) = self.nodes.get_mut(c) {
                n.parent = Some(parent);
            }
            // Children that stay inherit the same things as before.
            if !old.contains(&c) {
                self.mark_style_of(c);
            }
        }
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children = children;
        }
        self.invalidate_layout(parent);
    }

    pub fn detach(&mut self, id: NodeId) {
        let Some(parent) = self.nodes.get(id).and_then(|n| n.parent) else {
            return;
        };
        self.stacking_dirty = true;
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children.retain(|c| *c != id);
        }
        if let Some(n) = self.nodes.get_mut(id) {
            n.parent = None;
        }
        self.invalidate_layout(parent);
    }

    /// Removes a single node. Its children are detached, not removed: they belong to owners
    /// that remove them on their own.
    pub fn remove(&mut self, id: NodeId) {
        if !self.nodes.contains_key(id) {
            return;
        }
        self.detach(id);
        if let Some(node) = self.nodes.remove(id) {
            if let Some(popup) = node.popup() {
                self.popups.retain(|p| *p != id);
                if popup.sent.is_some() {
                    self.requests
                        .push(PlatformRequest::ClosePopup(popup_id(id)));
                }
            }
            for c in node.children {
                if let Some(n) = self.nodes.get_mut(c) {
                    n.parent = None;
                }
            }
        }
        let st = &mut self.state;
        st.hovered.retain(|h| *h != id);
        st.active.retain(|a| *a != id);
        if st.focused == Some(id) {
            st.focused = None;
            self.requests.push(PlatformRequest::SetIme(None));
        }
        if let Some(sel) = &st.selection
            && (sel.anchor.node == id || sel.focus.node == id)
        {
            st.selection = None;
        }
        if self.root == Some(id) {
            self.root = None;
        }
        self.edits.retain(|e| *e != id);
        self.stacked.remove(&id);
        self.stacking_dirty = true;
        self.layer_contents.remove(&id);
        self.mark_paint();
    }

    pub fn is_ancestor(&self, ancestor: NodeId, mut node: NodeId) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            match self.nodes.get(node).and_then(|n| n.parent) {
                Some(p) => node = p,
                None => return false,
            }
        }
    }

    /// Ancestors of `node` starting with itself.
    pub fn ancestors(&self, node: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = Some(node);
        while let Some(id) = cur {
            if !self.nodes.contains_key(id) {
                break;
            }
            out.push(id);
            cur = self.nodes[id].parent;
        }
        out
    }

    /// Nodes in document (pre-)order starting from the root.
    pub fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let Some(root) = self.root else { return out };
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(n) = self.nodes.get(id) else {
                continue;
            };
            out.push(id);
            stack.extend(n.children.iter().rev());
        }
        out
    }

    pub fn is_hovered(&self, id: NodeId) -> bool {
        self.state.hovered.contains(&id)
    }

    pub fn request(&mut self, req: PlatformRequest) {
        self.requests.push(req);
    }
}
