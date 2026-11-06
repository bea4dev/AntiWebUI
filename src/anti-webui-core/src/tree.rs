//! The retained node tree (a small DOM) and the per-window document state.

use crate::drag::{DropConfig, GestureConfig};
use crate::event::{KeyboardEvent, PinchEvent, PointerEvent, ScrollEvent};
use crate::geometry::{Affine, PointF, PointI, RectI, Scale, SizeI};
use crate::layout::{MeasureCache, MeasureKey, Resolved, child_clip};
use crate::platform::{Placement, PlatformRequest, PopupId, PopupKind, PopupParams};
use crate::scene::Scene;
use crate::shader::{Image, ShaderPaint};
use crate::text::TextLayout;
use crate::text::edit::EditData;
use crate::text::selection::DocSelection;
use crate::view::NodeRef;
use anti_webui_style::{ComputedStyle, Display, Sheet, Style};
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
    pub pinch: Option<Handler<PinchEvent>>,
    pub focus: Option<Rc<dyn Fn(bool)>>,
    pub active: Option<Rc<dyn Fn(bool)>>,
    pub focus_visible: Option<Rc<dyn Fn(bool)>>,
    pub scrolled: Option<Rc<dyn Fn(PointF)>>,
    pub resized: Option<Rc<dyn Fn(f32, f32)>>,
    pub(crate) gesture: Option<Rc<GestureConfig>>,
    pub(crate) drop: Option<Rc<DropConfig>>,
}

/// Style layers, applied in order: sheet → base → dynamic → hover → focus → active → dragging →
/// drag_over → disabled. The sheet has its part in each of them, below what the node has itself.
#[derive(Default)]
pub struct NodeStyle {
    pub sheet: Option<Rc<Sheet>>,
    pub base: Style,
    pub dynamic: Vec<Style>,
    pub hover: Option<Style>,
    pub focus: Option<Style>,
    pub active: Option<Style>,
    pub dragging: Option<Style>,
    pub drag_over: Option<Style>,
}

impl NodeStyle {
    /// Whether going in and out of the state makes a difference.
    pub fn depends_on_hover(&self) -> bool {
        self.hover.is_some() || self.sheet.as_ref().is_some_and(|s| s.hover.is_some())
    }

    pub fn depends_on_active(&self) -> bool {
        self.active.is_some() || self.sheet.as_ref().is_some_and(|s| s.active.is_some())
    }
}

pub struct TextData {
    pub text: String,
    pub version: u64,
    pub layout: TextLayout,
    /// How much bigger it is than it says, from what is around it in an `svg`.
    pub zoom: f32,
    /// It is in an `svg`, where text is painted with `fill`.
    pub in_svg: bool,
    /// Its spans and the nodes among its words, for a `rich_text`.
    pub rich: Option<Box<crate::text::rich::RichData>>,
    /// The editor it is a line of, for a line of a `text_editor`.
    pub row: Option<Box<crate::text::editor::RowLink>>,
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
    /// In the window's own surface, above its content, with the size of the window.
    pub portal: bool,
    /// While it exists, Tab stays inside of it.
    pub modal: bool,
    /// What a drag shows, which the painter puts where the pointer is. It goes by what was dragged
    /// for what is inherited.
    pub preview_of: Option<Box<ComputedStyle>>,

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
/// What an `img` shows.
#[derive(Default)]
pub struct ImageData {
    pub source: Option<anti_webui_style::ImageSource>,
    /// In logical pixels when one pixel of the picture is one of them, or `None` while it isn't
    /// known. It stays from the last picture until the next one is known.
    pub natural: Option<(f32, f32)>,
    /// How many pixels of the picture make a logical pixel.
    pub density: f32,
    /// What to show until the source shows, when the source changed.
    pub fallback: Option<Arc<crate::images::Bitmap>>,
    /// `on_decoded` has been called for this source.
    pub told: std::cell::Cell<bool>,
    pub on_load: Option<Rc<dyn Fn(f32, f32)>>,
    pub on_decoded: Option<Rc<dyn Fn()>>,
    pub on_error: Option<ErrorListener>,
}

/// Told why a picture can't be shown.
pub type ErrorListener = Rc<dyn Fn(&str)>;

// Text is what there is the most of, so it is worth the room over going through a pointer.
#[allow(clippy::large_enum_variant)]
pub enum NodeKind {
    Element,
    Text(TextData),
    Edit(Box<EditData>),
    /// Text edited line by line, its lines as children: `text_editor`. See "M3" in
    /// `docs/design/28-markdown.md`.
    Editor(Box<crate::text::editor::EditorData>),
    /// A picture in its box: `img`. See `docs/design/22-images.md`.
    Image(Box<ImageData>),
    /// A moving picture in its box: `video`. See `docs/design/24-video.md`.
    Video(Box<crate::video::VideoData>),
    /// A box with coordinates of its own for shapes: `svg`. See `docs/design/25-svg.md`.
    Svg(Box<crate::svg::SvgData>),
    /// A shape in an `svg`, or a group of them.
    SvgShape(Box<crate::svg::ShapeData>),
    /// What puts a text at a point in an `svg`.
    SvgText(Box<crate::svg::SvgTextData>),
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
    /// Paints the box below its background. See `docs/design/14-shaders.md`.
    pub fill: Option<ShaderPaint>,
    /// Paints the box above its background and border, below its children.
    pub overlay: Option<ShaderPaint>,
    /// What to make of what the node and its children paint. See `docs/design/16-filters.md`.
    pub filter: Option<Image>,

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
    /// The scroll root whose content this node is part of, the popup whose surface it is on, or
    /// the transformed node it is painted through, which may be itself.
    pub space: Option<NodeId>,
    /// Painted through a transform: `abs` and the descendants are in its own space, and the two
    /// below say how it sits in its parent's (see `docs/design/12-transform.md`).
    pub transformed: bool,
    /// When `transformed`: the clip that applies in the parent's coordinates.
    pub layer_clip: crate::scene::Clip,
    /// When `transformed`: the `space` of the parent's coordinates.
    pub outer_space: Option<NodeId>,
    /// Painted from its stacking context's list instead of in tree order.
    pub stacked: bool,
    /// Its stacked descendants are ordered among themselves, and stay above its background.
    pub stacking_context: bool,
    /// Contains everything the subtree paints, in the same coordinates as `abs`. It may be too
    /// big, never too small.
    pub subtree_bounds: RectI,
    /// The `subtree_bounds` of each child, so that culling doesn't need to touch the children.
    pub child_bounds: Vec<RectI>,
    /// The transitions going on, in no particular order. See `layout::apply_transitions`.
    pub(crate) transitions: Vec<crate::animation::RunningTransition>,
    /// When it comes and goes as part of a `show` with `enter` or `exit`.
    pub(crate) presence: Option<Box<crate::animation::Presence>>,
    /// On its way to where a `for_each` with `moves` has put it.
    pub(crate) moving: Option<Box<crate::animation::Moving>>,
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
    /// With `content-visibility: auto`: near the screen, focused, selected or scrolled to, so its
    /// contents are laid out. See `docs/design/29-content-visibility.md`.
    pub(crate) relevant: bool,
    /// Its contents were skipped by the last layout.
    pub(crate) skipped: bool,
    /// Not shown only because it is in contents that are skipped: it still counts for the
    /// selection.
    pub(crate) held: bool,
    /// Its size the last time its contents were laid out, with what its width came from.
    pub(crate) remembered: Option<(crate::layout::WidthInputs, SizeI)>,
    /// What its contents were estimated to need, until something in them changes.
    pub(crate) estimated: Option<(crate::layout::WidthInputs, SizeI)>,
}

impl Node {
    /// Whether it doesn't react, because of itself or of something around it.
    pub fn is_disabled(&self) -> bool {
        self.disabled || self.computed.disabled
    }

    pub fn new(kind: NodeKind) -> Self {
        Self {
            parent: None,
            children: Vec::new(),
            kind,
            style: NodeStyle::default(),
            listeners: Listeners::default(),
            focusable: false,
            disabled: false,
            fill: None,
            overlay: None,
            filter: None,
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
            transformed: false,
            layer_clip: crate::scene::Clip::NONE,
            outer_space: None,
            stacked: false,
            stacking_context: false,
            subtree_bounds: RectI::default(),
            child_bounds: Vec::new(),
            transitions: Vec::new(),
            presence: None,
            moving: None,
            subtree_nodes: 0,
            position_dirty: true,
            style_dirty: false,
            layout_dirty: true,
            last_perform: None,
            measure_cache: MeasureCache::default(),
            relevant: false,
            skipped: false,
            held: false,
            remembered: None,
            estimated: None,
        }
    }

    pub fn image(&self) -> Option<&ImageData> {
        match &self.kind {
            NodeKind::Image(d) => Some(d),
            _ => None,
        }
    }

    pub fn video(&self) -> Option<&crate::video::VideoData> {
        match &self.kind {
            NodeKind::Video(d) => Some(d),
            _ => None,
        }
    }

    pub fn video_mut(&mut self) -> Option<&mut crate::video::VideoData> {
        match &mut self.kind {
            NodeKind::Video(d) => Some(d),
            _ => None,
        }
    }

    /// How big what it holds is, if it has the size of what it holds: a picture or a video. The
    /// second part says how many pixels of it make a logical pixel.
    pub fn replaced(&self) -> Option<(Option<(f32, f32)>, f32)> {
        match &self.kind {
            NodeKind::Image(d) => Some((d.natural, d.density)),
            NodeKind::Video(d) => Some((d.natural, 1.0)),
            _ => None,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, NodeKind::Text(_))
    }

    pub fn is_edit(&self) -> bool {
        matches!(self.kind, NodeKind::Edit(_))
    }

    pub fn is_editor(&self) -> bool {
        matches!(self.kind, NodeKind::Editor(_))
    }

    pub fn editor(&self) -> Option<&crate::text::editor::EditorData> {
        match &self.kind {
            NodeKind::Editor(e) => Some(e),
            _ => None,
        }
    }

    pub fn editor_mut(&mut self) -> Option<&mut crate::text::editor::EditorData> {
        match &mut self.kind {
            NodeKind::Editor(e) => Some(e),
            _ => None,
        }
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

    /// What to show of what the node and its children paint, when it isn't that as it is.
    pub fn filter_image(&self) -> Option<Image> {
        let blur = self.computed.blur;
        match &self.filter {
            Some(image) => Some(image.clone().blur(blur)),
            None if blur > 0.0 => Some(Image::content().blur(blur)),
            None => None,
        }
    }

    /// Whether what the node and its children paint goes through a filter.
    pub fn is_filtered(&self) -> bool {
        self.filter.is_some() || self.computed.blur > 0.0
    }

    /// How far past what the node and its children paint a filter can touch.
    pub fn filter_reach(&self, scale: Scale) -> i32 {
        self.filter_image()
            .map_or(0, |image| image.reach_px(scale.as_f32()))
    }

    /// From the coordinates of a transformed node to its parent's.
    pub fn layer_matrix(&self, scale: Scale) -> Affine {
        let c = &self.computed;
        let origin = (
            self.abs.x as f64 + self.abs.width as f64 * c.transform_origin_x as f64,
            self.abs.y as f64 + self.abs.height as f64 * c.transform_origin_y as f64,
        );
        Affine::translation(
            origin.0 + scale.pxf(c.translate_x) as f64 + c.move_x as f64,
            origin.1 + scale.pxf(c.translate_y) as f64 + c.move_y as f64,
        )
        .then_apply(&Affine::rotation(c.rotate as f64))
        .then_apply(&Affine::scaling(c.scale_x as f64, c.scale_y as f64))
        .then_apply(&Affine::translation(-origin.0, -origin.1))
    }

    /// What the subtree can touch, in the parent's coordinates.
    pub fn bounds_in_parent(&self, scale: Scale) -> RectI {
        if self.transformed {
            self.layer_matrix(scale)
                .bounds_of(self.subtree_bounds.outset(1))
                .intersect(&self.layer_clip.rect)
        } else {
            self.subtree_bounds
        }
    }
}

/// What a layer painted last.
pub struct LayerMemo {
    pub content: Arc<Scene>,
    /// What `content` covers, in its own coordinates.
    pub covers: RectI,
    /// What `content` can touch, in its own coordinates.
    pub touched: RectI,
    /// The transform as the style has it, before any rounding.
    pub transform: Affine,
    /// The layers directly in `content`.
    pub nested: Vec<NodeId>,
    /// There are pictures somewhere in `content`.
    pub pictures: bool,
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
    /// The span of a `rich_text` the pointer is over.
    pub hovered_span: Option<(NodeId, usize)>,
    /// The pressed node and its ancestors while the primary button is down.
    pub active: Vec<NodeId>,
    pub focused: Option<NodeId>,
    /// The focus should be shown: it came from the keyboard.
    pub focus_visible: bool,
    pub selection: Option<DocSelection>,
    pub window_focused: bool,
    /// What is being dragged, for as long as it exists.
    pub dragging: Option<NodeId>,
    /// What would take what is being dragged.
    pub drag_over: Option<NodeId>,
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
    /// What each layer painted last, reused while the content is unchanged and the view stays
    /// inside the rectangle it covers.
    pub layer_contents: HashMap<NodeId, LayerMemo>,
    /// Layers to paint again although `dirty.content` isn't set: a transform inside moved.
    pub stale_layers: HashSet<NodeId>,
    /// A transform or a cursor changed, so what is under the pointer may have too.
    pub transform_moved: bool,
    /// A translation is between pixels: one more frame shows it on whole pixels if it stops.
    pub settling: bool,
    /// Nodes with transitions going on.
    pub(crate) transitioning: HashSet<NodeId>,
    /// Where things were in lists that changed since the last layout.
    pub(crate) list_places: Vec<crate::animation::ListPlaces>,
    /// Nodes whose style must be resolved again; see `style_all`.
    pub style_dirty: Vec<NodeId>,
    /// Every node's style must be resolved again.
    pub style_all: bool,
    /// Nodes whose descendants must be positioned again, short of a whole `dirty.position`.
    pub reposition: Vec<NodeId>,
    /// All edit nodes, which are few, to look through for pending caret scrolling.
    pub edits: Vec<NodeId>,
    /// The `text_editor`s.
    pub editors: Vec<NodeId>,
    /// All nodes with `stacked` set.
    pub stacked: HashSet<NodeId>,
    /// The stacked descendants of each stacking context that has any, in paint order. See
    /// `layout::build_stacking`.
    pub stacking: HashMap<NodeId, StackingList>,
    /// `stacking` must be built again.
    pub stacking_dirty: bool,
    /// All popups, parents before what is attached to them once `stacking` is built.
    pub popups: Vec<NodeId>,
    /// What shows for something being dragged.
    pub(crate) drag: Option<crate::paint::DragVisual>,
    /// Nodes to bring into view after the next layout.
    pub pending_scroll: Vec<NodeId>,
    /// Scrolling nodes to move after the next layout, and where to.
    pub(crate) pending_scroll_to: Vec<(NodeId, PointI)>,
    /// Nodes that scrolled and have someone waiting to hear about it.
    pub scrolled: Vec<NodeId>,
    /// Nodes that got another size and have someone waiting to hear about it.
    pub resized: Vec<NodeId>,
    /// What [`start_drag`](crate::drag::start_drag) asked for, which the next event picks up.
    pub(crate) drag_request: Option<crate::drag::DragRequest>,
    /// Painting asks for pictures while the rest of the document stays as it is.
    pub images: RefCell<crate::images::ImageStore>,
    pub videos: RefCell<crate::video::VideoStore>,
    pub fullscreen: Fullscreen,
    /// Errors to tell of when the document isn't borrowed any more.
    pub(crate) videos_failed: Vec<(crate::video::VideoErrorListener, crate::video::VideoError)>,
    /// What decides which contents to skip. See `docs/design/29-content-visibility.md`.
    pub(crate) visibility: crate::visibility::Visibility,
}

/// Who is on all of the screen. See `docs/design/27-fullscreen.md`.
#[derive(Default)]
pub struct Fullscreen {
    /// What the platform says of the window.
    pub window: bool,
    /// What shows instead of the root.
    pub element: Option<NodeId>,
    /// What shows instead of the root as soon as the window is on all of the screen.
    pub wanted: Option<NodeId>,
    /// The window went there for the element, and comes back with it.
    pub window_follows: bool,
    /// What had the focus when the element took it.
    pub focus_before: Option<Option<NodeId>>,
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
        stale_layers: HashSet::new(),
        transform_moved: false,
        settling: false,
        transitioning: HashSet::new(),
        list_places: Vec::new(),
        style_dirty: Vec::new(),
        style_all: false,
        reposition: Vec::new(),
        edits: Vec::new(),
        editors: Vec::new(),
        stacked: HashSet::new(),
        stacking: HashMap::new(),
        stacking_dirty: false,
        popups: Vec::new(),
        drag: None,
        pending_scroll: Vec::new(),
        pending_scroll_to: Vec::new(),
        scrolled: Vec::new(),
        resized: Vec::new(),
        drag_request: None,
        images: RefCell::default(),
        videos: RefCell::default(),
        fullscreen: Fullscreen::default(),
        videos_failed: Vec::new(),
        visibility: Default::default(),
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

    /// Where showing the window starts: the root, or what is on all of the screen instead of it.
    pub fn shown_root(&self) -> Option<NodeId> {
        self.fullscreen.element.or(self.root)
    }

    /// Whether a node is out of sight behind what is on all of the screen. What is in a popup isn't,
    /// as popups go on top of it.
    pub(crate) fn behind_fullscreen(&self, id: NodeId) -> bool {
        self.fullscreen.element.is_some_and(|element| {
            !self.is_ancestor(element, id)
                && !self.ancestors(id).iter().any(|&a| self.nodes[a].is_popup())
        })
    }

    /// Whether a popup has what is on all of the screen in it, which then doesn't show as a popup.
    pub(crate) fn holds_fullscreen(&self, popup: NodeId) -> bool {
        self.fullscreen
            .element
            .is_some_and(|element| self.is_ancestor(popup, element))
    }

    pub(crate) fn request_fullscreen(&mut self, id: NodeId) {
        if self.fullscreen.window {
            self.set_fullscreen_element(Some(id));
        } else {
            self.fullscreen.wanted = Some(id);
            self.fullscreen.window_follows = true;
            self.request(PlatformRequest::SetFullscreen(true));
        }
    }

    pub(crate) fn exit_fullscreen(&mut self) {
        let on_its_way = self.fullscreen.wanted.take().is_some();
        if on_its_way && !self.fullscreen.window {
            // The window was asked to go there and would stay there.
            self.fullscreen.window_follows = false;
            self.request(PlatformRequest::SetFullscreen(false));
        } else if self.fullscreen.window_follows && self.fullscreen.window {
            // What shows changes when the window has changed, in the same frame.
            self.request(PlatformRequest::SetFullscreen(false));
        } else {
            self.fullscreen.window_follows = false;
            self.set_fullscreen_element(None);
        }
    }

    pub(crate) fn window_fullscreen_changed(&mut self, on: bool) {
        self.fullscreen.window = on;
        if on {
            if let Some(id) = self.fullscreen.wanted.take() {
                self.set_fullscreen_element(Some(id));
            }
        } else {
            self.fullscreen.wanted = None;
            self.fullscreen.window_follows = false;
            self.set_fullscreen_element(None);
        }
    }

    fn set_fullscreen_element(&mut self, element: Option<NodeId>) {
        let old = std::mem::replace(&mut self.fullscreen.element, element);
        if old == element {
            return;
        }
        for id in [old, element].into_iter().flatten() {
            if self.nodes.contains_key(id) {
                // It lays out differently and becomes or stops being a stacking context. That goes
                // up to the root, which is what brings it back when the rest shows again.
                self.mark_style_of(id);
                self.invalidate_layout(id);
            }
        }
        self.stacking_dirty = true;
        self.dirty.position = true;
        let focused = self.state.focused;
        match element {
            Some(id) => {
                if self.fullscreen.focus_before.is_none() {
                    self.fullscreen.focus_before = Some(focused);
                }
                if focused.is_none_or(|f| self.behind_fullscreen(f)) {
                    let focusable = self.nodes.get(id).is_some_and(|n| n.focusable);
                    self.pending_focus = Some(focusable.then_some(id));
                }
            }
            None => {
                let before = self.fullscreen.focus_before.take();
                let was_inside = |f| old.is_some_and(|old| self.is_ancestor(old, f));
                if let Some(before) = before
                    && focused.is_none_or(was_inside)
                {
                    self.pending_focus = Some(before.filter(|b| self.nodes.contains_key(*b)));
                }
            }
        }
    }

    pub fn create(&mut self, node: Node) -> NodeId {
        let edit = node.is_edit();
        let editor = node.is_editor();
        let popup = node.is_popup();
        let id = self.nodes.insert(node);
        if edit {
            self.edits.push(id);
        }
        if editor {
            self.editors.push(id);
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
            if n.popup().is_some_and(|p| !p.portal) {
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

    /// The next space out from `space`, and how its coordinates map to it.
    fn step_out(&self, space: NodeId) -> Option<(Affine, Option<NodeId>)> {
        let r = self.nodes.get(space)?;
        if let Some(popup) = r.popup() {
            let o = popup.origin;
            return Some((Affine::translation(o.x as f64, o.y as f64), None));
        }
        if r.transformed {
            return Some((r.layer_matrix(self.scale), r.outer_space));
        }
        Some((
            Affine::translation((r.abs.x - r.scroll.x) as f64, (r.abs.y - r.scroll.y) as f64),
            r.space,
        ))
    }

    /// From a node's coordinates to window coordinates.
    pub fn to_window(&self, node: &Node, p: PointF) -> PointF {
        let (mut x, mut y) = (p.x as f64, p.y as f64);
        let mut space = node.space;
        while let Some((m, next)) = space.and_then(|s| self.step_out(s)) {
            (x, y) = m.apply(x, y);
            space = next;
        }
        PointF::new(x as f32, y as f32)
    }

    /// A rectangle in a node's coordinates, in window coordinates: the box around it if it isn't
    /// axis-aligned anymore.
    pub fn rect_to_window(&self, node: &Node, r: RectI) -> RectI {
        let mut r = r;
        let mut space = node.space;
        while let Some((m, next)) = space.and_then(|s| self.step_out(s)) {
            r = if m.is_whole_translation() {
                r.translate(m.e.round() as i32, m.f.round() as i32)
            } else {
                m.bounds_of(r)
            };
            space = next;
        }
        r
    }

    /// From window coordinates to a node's. `None` if a transform flattens it away.
    pub fn from_window(&self, node: &Node, p: PointF) -> Option<PointF> {
        self.from_window_space(node.space, p)
    }

    /// From window coordinates to the coordinates of a space.
    pub fn from_window_space(&self, space: Option<NodeId>, p: PointF) -> Option<PointF> {
        let Some((m, next)) = space.and_then(|s| self.step_out(s)) else {
            return Some(p);
        };
        let outer = self.from_window_space(next, p)?;
        Some(m.inverse()?.apply_point(outer))
    }

    /// The node's border box in window coordinates.
    pub fn window_rect(&self, id: NodeId) -> Option<RectI> {
        let n = self.nodes.get(id)?;
        Some(self.rect_to_window(n, n.abs))
    }

    /// The part of the node that is not clipped away, in window coordinates.
    pub fn visible_window_rect(&self, id: NodeId) -> Option<RectI> {
        let n = self.nodes.get(id)?;
        Some(self.visible_part(n, n.abs))
    }

    /// The part of a rectangle in a node's coordinates that is not clipped away, in window
    /// coordinates.
    pub(crate) fn visible_part(&self, n: &Node, rect: RectI) -> RectI {
        let mut rect = rect.intersect(&n.clip);
        let mut space = n.space;
        while let Some(r) = space.and_then(|s| self.nodes.get(s)) {
            let Some((m, next)) = space.and_then(|s| self.step_out(s)) else {
                break;
            };
            rect = if m.is_whole_translation() {
                rect.translate(m.e.round() as i32, m.f.round() as i32)
            } else {
                m.bounds_of(rect)
            };
            if r.transformed {
                rect = rect.intersect(&r.layer_clip.rect);
            } else if !r.is_popup() {
                rect = rect.intersect(&child_clip(r, self.scale, r.clip_shape()).rect);
            }
            space = next;
        }
        rect
    }

    /// Whether any of a rectangle in a node's coordinates could be seen.
    pub(crate) fn can_be_seen(&self, n: &Node, rect: RectI) -> bool {
        let rect = self.visible_part(n, rect);
        let window = RectI::new(0, 0, self.viewport.width, self.viewport.height);
        // A popup is free to leave the window.
        !rect.is_empty() && (!self.popups.is_empty() || !rect.intersect(&window).is_empty())
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
    }

    /// Something every style depends on changed.
    pub fn mark_style_all(&mut self) {
        self.style_all = true;
        self.dirty.style = true;
        self.dirty.paint = true;
        self.dirty.content = true;
    }

    /// An `img` is to show `source` now. What it showed stays until this shows.
    pub(crate) fn set_image_source(&mut self, id: NodeId, source: anti_webui_style::ImageSource) {
        let Some(old) = self
            .nodes
            .get(id)
            .and_then(|n| n.image())
            .map(|d| d.source.clone())
        else {
            return;
        };
        if old.as_ref() == Some(&source) {
            return;
        }
        let images = self.images.get_mut();
        let fallback = old.as_ref().and_then(|old| {
            let best = images.best(old);
            images.release(id, old);
            best
        });
        let info = images.acquire(id, &source);
        let ready = images.best(&source).is_some();
        if info.is_some() {
            crate::images::tell_loaded(images, id);
        }
        if let Some(NodeKind::Image(d)) = self.nodes.get_mut(id).map(|n| &mut n.kind) {
            d.source = Some(source);
            d.fallback = fallback.filter(|_| !ready);
            d.told.set(false);
            if let Some(info) = info {
                d.natural = Some(info.natural);
            }
        }
        self.invalidate_layout(id);
        self.mark_shader_of(id);
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
            n.estimated = None;
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
            n.estimated = None;
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

    /// Only what a node's shader paints changed. This can happen on every frame, so of the layers
    /// only those it is in paint again. Marking more ancestors than are layers costs nothing.
    pub fn mark_shader_of(&mut self, id: NodeId) {
        self.dirty.paint = true;
        let mut cur = Some(id);
        while let Some(i) = cur {
            let Some(n) = self.nodes.get(i) else {
                break;
            };
            self.stale_layers.insert(i);
            cur = if n.is_popup() { None } else { n.parent };
        }
    }

    /// Only what a filter makes of the node's content changed: the content stays as it is, while
    /// the layers around it hold how it shows.
    pub fn mark_filter_of(&mut self, id: NodeId) {
        self.dirty.paint = true;
        crate::layout::mark_outer_layers_stale(self, id);
        crate::layout::grow_bounds(self, id);
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
        if self.fullscreen.wanted == Some(id) {
            self.fullscreen.wanted = None;
        }
        if let Some(node) = self.nodes.remove(id) {
            if self.fullscreen.element == Some(id) {
                self.set_fullscreen_element(None);
                if std::mem::take(&mut self.fullscreen.window_follows) && self.fullscreen.window {
                    self.request(PlatformRequest::SetFullscreen(false));
                }
            }
            if let Some(source) = node.image().and_then(|d| d.source.as_ref()) {
                self.images.get_mut().release(id, source);
            }
            if node.video().is_some() {
                crate::video::remove(self, id);
            }
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
        if st.dragging == Some(id) {
            st.dragging = None;
        }
        if st.drag_over == Some(id) {
            st.drag_over = None;
        }
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
        self.editors.retain(|e| *e != id);
        self.stacked.remove(&id);
        self.transitioning.remove(&id);
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
