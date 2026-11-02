//! The retained node tree (a small DOM) and the per-window document state.

use crate::event::{KeyboardEvent, PointerEvent, ScrollEvent};
use crate::geometry::{PointI, RectI, Scale, SizeI};
use crate::layout::{MeasureCache, MeasureKey, Resolved};
use crate::platform::PlatformRequest;
use crate::text::TextLayout;
use crate::text::edit::EditData;
use crate::text::selection::DocSelection;
use anti_webui_style::{ComputedStyle, Style};
use slotmap::{SlotMap, new_key_type};
use std::cell::RefCell;
use std::rc::Rc;

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
    /// Border box in window coordinates, after scrolling.
    pub abs: RectI,
    /// Clip that applies to this node, in window coordinates.
    pub clip: RectI,
    /// Corner radii of `clip` (non-zero when an ancestor with rounded corners clips).
    pub clip_radii: [f32; 4],
    pub scroll: PointI,
    pub doc_order: u32,
    /// False inside `display: none` subtrees.
    pub visible: bool,
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

    /// The content box in window coordinates.
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

    /// The padding box in window coordinates.
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
    });
}

/// Borrows the document. Never write signals or call user handlers inside `f`.
pub fn with_doc<R>(f: impl FnOnce(&mut Document) -> R) -> R {
    DOC.with(|d| f(&mut d.borrow_mut()))
}

impl Document {
    pub fn create(&mut self, node: Node) -> NodeId {
        self.nodes.insert(node)
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Styles need recomputing. Layout runs only if a layout-affecting property changed.
    pub fn mark_style(&mut self) {
        self.dirty.style = true;
        self.dirty.paint = true;
    }

    /// Marks `id` and its ancestors for re-layout; clean subtrees keep their cached layout.
    pub fn invalidate_layout(&mut self, id: NodeId) {
        let mut cur = Some(id);
        while let Some(i) = cur {
            let Some(n) = self.nodes.get_mut(i) else {
                break;
            };
            n.layout_dirty = true;
            cur = n.parent;
        }
        self.mark_layout();
    }

    /// Discards every cached layout result.
    pub fn invalidate_all_layout(&mut self) {
        for (_, n) in self.nodes.iter_mut() {
            n.layout_dirty = true;
        }
        self.mark_layout();
    }

    pub fn mark_layout(&mut self) {
        self.dirty.layout = true;
        self.dirty.paint = true;
    }

    pub fn mark_paint(&mut self) {
        self.dirty.paint = true;
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        if let Some(c) = self.nodes.get_mut(child) {
            c.parent = Some(parent);
        }
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children.push(child);
        }
        self.mark_style();
        self.invalidate_layout(parent);
    }

    pub fn set_children(&mut self, parent: NodeId, children: Vec<NodeId>) {
        let old = match self.nodes.get_mut(parent) {
            Some(p) => std::mem::take(&mut p.children),
            None => return,
        };
        for c in old {
            if let Some(n) = self.nodes.get_mut(c)
                && n.parent == Some(parent)
            {
                n.parent = None;
            }
        }
        for &c in &children {
            self.detach(c);
            if let Some(n) = self.nodes.get_mut(c) {
                n.parent = Some(parent);
            }
        }
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children = children;
        }
        self.mark_style();
        self.invalidate_layout(parent);
    }

    pub fn detach(&mut self, id: NodeId) {
        let Some(parent) = self.nodes.get(id).and_then(|n| n.parent) else {
            return;
        };
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children.retain(|c| *c != id);
        }
        if let Some(n) = self.nodes.get_mut(id) {
            n.parent = None;
        }
        self.mark_style();
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
        self.mark_style();
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
