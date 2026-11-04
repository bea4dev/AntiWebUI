//! Builders that create nodes and wire them to the reactive system.
//!
//! A builder only collects configuration; nodes are created in [`IntoView::into_view`], inside
//! the current reactive owner, and removed again when that owner is disposed.

use crate::animation::{ListPlaces, request_frame};
use crate::event::{KeyboardEvent, PointerEvent, ScrollEvent};
use crate::geometry::{PointF, PointI, RectI, Scale};
use crate::platform::{ImePurpose, Placement, PopupKind};
use crate::reactive::{
    Owner, Signal, create_child_owner, create_root, effect, memo, on_cleanup, signal, untrack,
};
use crate::shader::{Image, IntoImage, IntoPaint, ShaderPaint};
use crate::text::TextLayout;
use crate::text::edit::{EditData, TextCallback};
use crate::tree::{
    Document, Handler, Listeners, Node, NodeId, NodeKind, PopupData, TextData, with_doc,
};
use anti_webui_style::{
    AlignItems, Display as Layout, FlexDirection, Sheet, Style, Styled, Transition, UserSelect,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::Rc;

/// The root node of a built piece of UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View(pub NodeId);

pub trait IntoView {
    fn into_view(self) -> View;
}

impl IntoView for View {
    fn into_view(self) -> View {
        self
    }
}

/// Inserts a node and removes it when the current owner is disposed.
fn create_node(node: Node) -> NodeId {
    let id = with_doc(|d| d.create(node));
    on_cleanup(move || with_doc(|d| d.remove(id)));
    id
}

/// A handle to a node that can be attached with `.node_ref()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef(Signal<Option<NodeId>>);

pub fn node_ref() -> NodeRef {
    NodeRef(signal(None))
}

impl NodeRef {
    pub fn get(&self) -> Option<NodeId> {
        self.0.get()
    }

    pub(crate) fn id_untracked(&self) -> Option<NodeId> {
        self.0.get_untracked()
    }

    /// Scrolls what contains the node, after the next layout, until the node is in view.
    pub fn scroll_into_view(&self) {
        if let Some(id) = self.0.get_untracked() {
            with_doc(|d| {
                d.pending_scroll.push(id);
                d.dirty.paint = true;
            });
        }
    }

    /// The border box in window physical pixels, as of the last layout.
    pub fn rect(&self) -> Option<RectI> {
        let id = self.0.get_untracked()?;
        with_doc(|d| d.window_rect(id))
    }

    /// How far a scrolling node has scrolled, in logical pixels.
    pub fn scroll_offset(&self) -> PointF {
        self.0
            .get_untracked()
            .and_then(|id| {
                with_doc(|d| {
                    let n = d.get(id)?;
                    Some(PointF::new(
                        d.scale.to_logical(n.scroll.x as f32),
                        d.scale.to_logical(n.scroll.y as f32),
                    ))
                })
            })
            .unwrap_or_default()
    }

    /// Scrolls a scrolling node to this offset in logical pixels, as far as it goes.
    pub fn scroll_to(&self, x: f32, y: f32) {
        let Some(id) = self.0.get_untracked() else {
            return;
        };
        with_doc(|d| {
            let (x, y) = (d.scale.px(x), d.scale.px(y));
            let Some(n) = d.get_mut(id) else {
                return;
            };
            let pad = n.padding_abs();
            let to = PointI::new(
                x.clamp(0, (n.content_size.width - pad.width).max(0)),
                y.clamp(0, (n.content_size.height - pad.height).max(0)),
            );
            if n.scroll != to {
                n.scroll = to;
                d.mark_scrolled(id);
            }
        });
    }

    pub fn focus(&self) {
        if let Some(id) = self.0.get_untracked() {
            with_doc(|d| d.pending_focus = Some(Some(id)));
        }
    }

    pub fn blur(&self) {
        with_doc(|d| d.pending_focus = Some(None));
    }
}

type StyleFn = Box<dyn Fn(Style) -> Style>;

/// Keeps the node's dynamic style layers in sync with their closures.
fn bind_dynamic_styles(id: NodeId, layers: Vec<StyleFn>) {
    with_doc(|d| {
        if let Some(n) = d.get_mut(id) {
            n.style.dynamic = vec![Style::default(); layers.len()];
        }
    });
    for (slot, f) in layers.into_iter().enumerate() {
        effect(move || {
            let style = f(Style::default());
            with_doc(|d| {
                if let Some(n) = d.get_mut(id)
                    && n.style.dynamic[slot] != style
                {
                    n.style.dynamic[slot] = style;
                    d.mark_style_of(id);
                }
            });
        });
    }
}

type PaintFn = Box<dyn Fn() -> Option<ShaderPaint>>;
type ImageFn = Box<dyn Fn() -> Option<Image>>;

/// Keeps what a node's shader paints in sync with its closure. `slot` says which one it is.
fn bind_paint(id: NodeId, f: PaintFn, slot: fn(&mut Node) -> &mut Option<ShaderPaint>) {
    effect(move || {
        let paint = f();
        with_doc(|d| {
            if let Some(n) = d.get_mut(id)
                && *slot(n) != paint
            {
                *slot(n) = paint;
                d.mark_shader_of(id);
            }
        });
    });
}

fn bind_filter(id: NodeId, f: ImageFn) {
    effect(move || {
        let image = f();
        with_doc(|d| {
            let Some(n) = d.get_mut(id) else {
                return;
            };
            if n.filter == image {
                return;
            }
            let layer_changes = n.filter.is_some() != image.is_some();
            n.filter = image;
            if layer_changes {
                // It starts or stops being a layer, and a stacking context.
                d.mark_style_of(id);
                d.dirty.content = true;
            }
            d.mark_filter_of(id);
        });
    });
}

/// Adds a listener to those a node already has: what builds an element and what uses it can each
/// listen without knowing of the other.
fn also<E: 'static>(old: Option<Handler<E>>, f: impl Fn(&E) + 'static) -> Option<Handler<E>> {
    Some(match old {
        Some(old) => Rc::new(move |e| {
            old(e);
            f(e);
        }),
        None => Rc::new(f),
    })
}

/// The same for listeners that are told whether something is so.
fn also_bool(
    old: Option<Rc<dyn Fn(bool)>>,
    f: impl Fn(bool) + 'static,
) -> Option<Rc<dyn Fn(bool)>> {
    Some(match old {
        Some(old) => Rc::new(move |v| {
            old(v);
            f(v);
        }),
        None => Rc::new(f),
    })
}

thread_local! {
    static WINDOW_SCALE: Cell<Option<Signal<Scale>>> = const { Cell::new(None) };
}

fn window_scale_signal() -> Signal<Scale> {
    WINDOW_SCALE.with(|s| {
        s.get().unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever asks for it first.
            let (signal, _) = create_root(|| signal(with_doc(|d| d.scale)));
            s.set(Some(signal));
            signal
        })
    })
}

/// The scale of the window. Reading it tracks it.
pub fn window_scale() -> Scale {
    window_scale_signal().get()
}

pub(crate) fn tell_window_scale(scale: Scale) {
    window_scale_signal().set_if_changed(scale);
}

type SheetFn = Box<dyn Fn() -> Rc<Sheet>>;

/// Common element configuration shared by `div` and `text_input`.
#[derive(Default)]
struct ElementConfig {
    sheet: Option<SheetFn>,
    style: Style,
    hover: Option<Style>,
    focus: Option<Style>,
    active: Option<Style>,
    dynamic: Vec<StyleFn>,
    fill: Option<PaintFn>,
    overlay: Option<PaintFn>,
    filter: Option<ImageFn>,
    listeners: Listeners,
    focusable: bool,
    disabled: Option<Box<dyn Fn() -> bool>>,
    node_ref: Vec<NodeRef>,
}

impl ElementConfig {
    fn build(self, mut node: Node) -> NodeId {
        node.style.base = self.style;
        node.style.hover = self.hover;
        node.style.focus = self.focus;
        node.style.active = self.active;
        node.listeners = self.listeners;
        node.focusable |= self.focusable;
        let id = create_node(node);
        if let Some(f) = self.sheet {
            effect(move || {
                let sheet = f();
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id)
                        && !n
                            .style
                            .sheet
                            .as_ref()
                            .is_some_and(|s| Rc::ptr_eq(s, &sheet))
                    {
                        n.style.sheet = Some(sheet);
                        d.mark_style_of(id);
                    }
                });
            });
        }
        bind_dynamic_styles(id, self.dynamic);
        if let Some(f) = self.fill {
            bind_paint(id, f, |n| &mut n.fill);
        }
        if let Some(f) = self.overlay {
            bind_paint(id, f, |n| &mut n.overlay);
        }
        if let Some(f) = self.filter {
            bind_filter(id, f);
        }
        if let Some(disabled) = self.disabled {
            effect(move || {
                let v = disabled();
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id) {
                        n.disabled = v;
                    }
                    if v && d.state.focused == Some(id) {
                        d.pending_focus = Some(None);
                    }
                    d.mark_style_of(id);
                });
            });
        }
        for r in self.node_ref {
            r.0.set(Some(id));
        }
        id
    }
}

macro_rules! element_methods {
    () => {
        /// Style applied while the pointer is over the node.
        pub fn hover(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.hover = Some(f(self.cfg.hover.take().unwrap_or_default()));
            self
        }

        /// Style applied while the node has keyboard focus.
        pub fn focused(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.focus = Some(f(self.cfg.focus.take().unwrap_or_default()));
            self
        }

        /// Style applied while the node (or a descendant) is pressed.
        pub fn active(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.active = Some(f(self.cfg.active.take().unwrap_or_default()));
            self
        }

        /// A reactive style layer, recomputed whenever the signals it reads change.
        pub fn dyn_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
            self.cfg.dynamic.push(Box::new(f));
            self
        }

        /// What the node looks like in each state, below everything else it is given. `f` runs
        /// again whenever the signals it reads change, and what it returns counts as changed when
        /// it is another [`Rc`]. See `docs/design/21-widgets.md`.
        pub fn sheet(mut self, f: impl Fn() -> Rc<Sheet> + 'static) -> Self {
            self.cfg.sheet = Some(Box::new(f));
            self
        }

        pub fn on_click(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.click = also(self.cfg.listeners.click.take(), f);
            self
        }

        pub fn on_pointer_down(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.pointer_down = also(self.cfg.listeners.pointer_down.take(), f);
            self
        }

        pub fn on_pointer_up(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.pointer_up = also(self.cfg.listeners.pointer_up.take(), f);
            self
        }

        pub fn on_pointer_move(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.pointer_move = also(self.cfg.listeners.pointer_move.take(), f);
            self
        }

        pub fn on_hover(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.listeners.hover = also_bool(self.cfg.listeners.hover.take(), f);
            self
        }

        pub fn on_key_down(mut self, f: impl Fn(&KeyboardEvent) + 'static) -> Self {
            self.cfg.listeners.key_down = also(self.cfg.listeners.key_down.take(), f);
            self
        }

        pub fn on_key_up(mut self, f: impl Fn(&KeyboardEvent) + 'static) -> Self {
            self.cfg.listeners.key_up = also(self.cfg.listeners.key_up.take(), f);
            self
        }

        pub fn on_scroll(mut self, f: impl Fn(&ScrollEvent) + 'static) -> Self {
            self.cfg.listeners.scroll = also(self.cfg.listeners.scroll.take(), f);
            self
        }

        pub fn on_focus_change(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.listeners.focus = also_bool(self.cfg.listeners.focus.take(), f);
            self
        }

        /// Like [`on_focus_change`](Self::on_focus_change), for focus that should be shown: what
        /// was reached with the keyboard, and not what was clicked (CSS `:focus-visible`).
        pub fn on_focus_visible(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.listeners.focus_visible =
                also_bool(self.cfg.listeners.focus_visible.take(), f);
            self
        }

        /// Told when the node (or a descendant) starts and stops being pressed, also when the
        /// pointer is released somewhere else.
        pub fn on_active(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.listeners.active = also_bool(self.cfg.listeners.active.take(), f);
            self
        }

        /// Paints the box with a shader, above its outer shadows and below its background, cut to
        /// its rounded shape. `f` runs again whenever the signals it reads change. See
        /// `docs/design/14-shaders.md`.
        pub fn fill<P: IntoPaint>(mut self, f: impl Fn() -> P + 'static) -> Self {
            self.cfg.fill = Some(Box::new(move || f().into_paint()));
            self
        }

        /// Paints the box with a shader, above its background and border and below its children,
        /// cut to its rounded shape. Returning `None` paints nothing and costs nothing.
        pub fn overlay<P: IntoPaint>(mut self, f: impl Fn() -> P + 'static) -> Self {
            self.cfg.overlay = Some(Box::new(move || f().into_paint()));
            self
        }

        /// Shows a picture made of what the node and its children paint instead of that as it is.
        /// `f` runs again whenever the signals it reads change, without painting them again. See
        /// `docs/design/16-filters.md`.
        pub fn filter<I: IntoImage>(mut self, f: impl Fn() -> I + 'static) -> Self {
            self.cfg.filter = Some(Box::new(move || f().into_image()));
            self
        }

        /// Called when the node has scrolled, however that happened, with its offset in logical
        /// pixels. Unlike `on_scroll` it doesn't depend on input, and comes before the next frame
        /// is laid out.
        pub fn on_scrolled(mut self, f: impl Fn(PointF) + 'static) -> Self {
            self.cfg.listeners.scrolled = Some(Rc::new(f));
            self
        }

        /// Called with the node's width and height in logical pixels when it first gets a size and
        /// whenever that changes, before anything is painted with it.
        pub fn on_resized(mut self, f: impl Fn(f32, f32) + 'static) -> Self {
            self.cfg.listeners.resized = Some(Rc::new(f));
            self
        }

        /// Makes the node reachable with Tab and focusable by clicking.
        pub fn focusable(mut self) -> Self {
            self.cfg.focusable = true;
            self
        }

        /// Disables pointer and keyboard handlers while `f` returns true.
        pub fn disabled(mut self, f: impl Fn() -> bool + 'static) -> Self {
            self.cfg.disabled = Some(Box::new(f));
            self
        }

        /// Lets `r` refer to this node. A node can have several of them.
        pub fn node_ref(mut self, r: NodeRef) -> Self {
            self.cfg.node_ref.push(r);
            self
        }
    };
}

/// A generic box element.
#[derive(Default)]
pub struct Div {
    cfg: ElementConfig,
    children: Vec<View>,
}

pub fn div() -> Div {
    Div::default()
}

/// A box painted by a shader: `div().fill(f)`.
pub fn shader<P: IntoPaint>(f: impl Fn() -> P + 'static) -> Div {
    div().fill(f)
}

impl Styled for Div {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Div {
    element_methods!();

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

impl IntoView for Div {
    fn into_view(self) -> View {
        let id = self.cfg.build(Node::new(NodeKind::Element));
        let children: Vec<NodeId> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        View(id)
    }
}

/// The source of a text node's content.
pub enum TextSource {
    Static(String),
    Dynamic(Box<dyn Fn() -> String>),
}

pub trait IntoTextSource {
    fn into_text_source(self) -> TextSource;
}

impl IntoTextSource for &str {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self.to_owned())
    }
}

impl IntoTextSource for String {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self)
    }
}

impl<F, S> IntoTextSource for F
where
    F: Fn() -> S + 'static,
    S: Display,
{
    fn into_text_source(self) -> TextSource {
        TextSource::Dynamic(Box::new(move || self().to_string()))
    }
}

/// A run of text. Selectable by default.
pub struct Text {
    source: TextSource,
    style: Style,
    dynamic: Vec<StyleFn>,
}

/// Creates a text node. Pass a closure for reactive text: `text(move || count.get())`.
pub fn text(source: impl IntoTextSource) -> Text {
    Text {
        source: source.into_text_source(),
        style: Style::default(),
        dynamic: Vec::new(),
    }
}

impl Text {
    /// A reactive style layer, recomputed whenever the signals it reads change.
    pub fn dyn_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.dynamic.push(Box::new(f));
        self
    }
}

impl Styled for Text {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl IntoView for Text {
    fn into_view(self) -> View {
        let initial = match &self.source {
            TextSource::Static(s) => s.clone(),
            TextSource::Dynamic(_) => String::new(),
        };
        let mut node = Node::new(NodeKind::Text(TextData {
            text: initial,
            version: 0,
            layout: TextLayout::default(),
        }));
        node.style.base = self.style;
        let id = create_node(node);
        bind_dynamic_styles(id, self.dynamic);
        if let TextSource::Dynamic(f) = self.source {
            effect(move || {
                let s = f();
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id)
                        && let NodeKind::Text(t) = &mut n.kind
                        && t.text != s
                    {
                        t.text = s;
                        t.version += 1;
                        d.invalidate_layout(id);
                    }
                });
            });
        }
        View(id)
    }
}

impl IntoView for &str {
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl IntoView for String {
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl<V: IntoView> IntoView for Option<V> {
    fn into_view(self) -> View {
        match self {
            Some(v) => v.into_view(),
            None => fragment(Vec::<View>::new()),
        }
    }
}

impl<V: IntoView> IntoView for Vec<V> {
    fn into_view(self) -> View {
        fragment(self)
    }
}

/// Groups views without generating a box.
pub fn fragment<V: IntoView>(children: impl IntoIterator<Item = V>) -> View {
    let id = create_node(Node::new(NodeKind::Fragment));
    let children: Vec<NodeId> = children.into_iter().map(|v| v.into_view().0).collect();
    with_doc(|d| d.set_children(id, children));
    View(id)
}

/// Rebuilds its content whenever the signals read by `f` change.
pub fn dyn_view<V: IntoView>(f: impl Fn() -> V + 'static) -> View {
    let id = create_node(Node::new(NodeKind::Fragment));
    effect(move || {
        // Nodes created here are owned by this effect and disposed before the next run.
        let child = f().into_view();
        with_doc(|d| d.set_children(id, vec![child.0]));
    });
    View(id)
}

/// How something comes and goes, for [`show`] and [`for_each`].
struct PresenceConfig {
    enter: Option<Style>,
    exit: Option<Style>,
    timing: Transition,
    appear: bool,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            enter: None,
            exit: None,
            timing: Transition::ms(150).ease_out(),
            appear: false,
        }
    }
}

impl PresenceConfig {
    /// Gets what was just built under `root` to come, if it is to.
    fn begin(&self, root: NodeId, starting: bool) {
        let Some(enter) = self.enter.as_ref().filter(|_| starting) else {
            return;
        };
        with_doc(|d| {
            let mut targets = Vec::new();
            presence_targets(d, root, &mut targets);
            for t in targets {
                let p = d.nodes[t].presence.get_or_insert_with(Default::default);
                p.starting = Some(enter.clone());
                p.timing = self.timing;
            }
        });
    }

    /// Gets what is under `root` going, to `exit`.
    fn go(&self, root: NodeId, exit: &Style) {
        with_doc(|d| {
            let mut targets = Vec::new();
            presence_targets(d, root, &mut targets);
            for t in &targets {
                let p = d.nodes[*t].presence.get_or_insert_with(Default::default);
                p.exit = exit.clone();
                p.timing = self.timing;
                p.exiting = true;
                p.returning = false;
                d.mark_style_of(*t);
            }
            if let Some(focused) = d.state.focused
                && targets.iter().any(|t| d.is_ancestor(*t, focused))
            {
                d.pending_focus = Some(None);
            }
            // It no longer takes the pointer.
            d.transform_moved = true;
        });
    }
}

macro_rules! presence_methods {
    () => {
        /// What it goes from when it comes.
        pub fn enter(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.presence.enter = Some(f(Style::default()));
            self
        }

        /// What it goes to before it goes away.
        pub fn exit(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.presence.exit = Some(f(Style::default()));
            self
        }

        /// How long coming and going takes, and how it goes.
        pub fn transition(mut self, timing: Transition) -> Self {
            self.presence.timing = timing;
            self
        }

        /// Also goes through `enter` if it is shown from the start.
        pub fn appear(mut self) -> Self {
            self.presence.appear = true;
            self
        }
    };
}

/// Coming and going applies to boxes, so it goes through what doesn't have one.
fn has_no_box(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Fragment | NodeKind::Popup(_))
        || n.computed.display == Layout::Contents
        || n.style.base.display == Some(Layout::Contents)
}

fn presence_targets(d: &Document, id: NodeId, out: &mut Vec<NodeId>) {
    let Some(n) = d.get(id) else {
        return;
    };
    if has_no_box(n) {
        for &c in &n.children {
            presence_targets(d, c, out);
        }
    } else {
        out.push(id);
    }
}

/// Stops what is under `root` from going: it comes back from where it got to, as the same nodes.
fn come_back(root: NodeId) {
    with_doc(|d| {
        let mut targets = Vec::new();
        presence_targets(d, root, &mut targets);
        for t in targets {
            if let Some(p) = &mut d.nodes[t].presence {
                p.exiting = false;
                p.returning = true;
            }
            d.mark_style_of(t);
        }
        d.transform_moved = true;
    });
}

/// Whether what is under `root` has finished going.
fn has_gone(d: &Document, root: NodeId) -> bool {
    let Some(n) = d.get(root) else {
        return true;
    };
    if has_no_box(n) {
        n.children.iter().all(|c| has_gone(d, *c))
    } else {
        // What can't be seen has nothing to wait for, whatever goes on in it.
        !n.visible || (n.transitions.is_empty() && !n.style_dirty)
    }
}

/// Shows `view` while `when` returns true. `view` is only rebuilt when `when` flips.
///
/// With [`Show::enter`] and [`Show::exit`] it animates coming and going, which keeps what it shows
/// around until it has gone (see `docs/design/13-animation.md`).
pub fn show<V: IntoView>(
    when: impl Fn() -> bool + 'static,
    view: impl Fn() -> V + 'static,
) -> Show {
    Show {
        when: Box::new(when),
        view: Box::new(move || view().into_view()),
        presence: PresenceConfig::default(),
    }
}

/// See [`show`].
pub struct Show {
    when: Box<dyn Fn() -> bool>,
    view: Box<dyn Fn() -> View>,
    presence: PresenceConfig,
}

impl Show {
    presence_methods!();
}

/// What is shown now.
struct Shown {
    root: NodeId,
    owner: Owner,
    exiting: bool,
}

/// Removes what was shown once it has finished going.
fn wait_for_exit(state: Rc<RefCell<Option<Shown>>>) {
    request_frame(move |_| {
        let shown = state
            .borrow()
            .as_ref()
            .map(|s| (s.root, s.owner, s.exiting));
        let Some((root, owner, true)) = shown else {
            return;
        };
        if !owner.is_alive() {
            return;
        }
        if with_doc(|d| has_gone(d, root)) {
            *state.borrow_mut() = None;
            owner.dispose();
        } else {
            wait_for_exit(state);
        }
    });
}

impl IntoView for Show {
    fn into_view(self) -> View {
        let Show {
            when,
            view,
            presence,
        } = self;
        let cond = memo(when);
        if presence.enter.is_none() && presence.exit.is_none() {
            return dyn_view(move || {
                if cond.get() {
                    Some(untrack(&view))
                } else {
                    None
                }
            });
        }
        let id = create_node(Node::new(NodeKind::Fragment));
        // What it shows belongs out here: this effect running again doesn't end it.
        let outer = Owner::current();
        let state: Rc<RefCell<Option<Shown>>> = Rc::new(RefCell::new(None));
        let first = Cell::new(true);
        let run = move |on: bool| {
            let starting = !first.replace(false) || presence.appear;
            let current = state
                .borrow()
                .as_ref()
                .map(|s| (s.root, s.owner, s.exiting));
            match (on, current) {
                (true, None) => {
                    let build = || create_child_owner(&view);
                    let (root, owner) = match outer {
                        Some(o) => o.with(build),
                        None => create_root(&view),
                    };
                    presence.begin(root.0, starting);
                    with_doc(|d| d.set_children(id, vec![root.0]));
                    *state.borrow_mut() = Some(Shown {
                        root: root.0,
                        owner,
                        exiting: false,
                    });
                }
                (true, Some((root, _, true))) => {
                    come_back(root);
                    if let Some(s) = &mut *state.borrow_mut() {
                        s.exiting = false;
                    }
                }
                (false, Some((root, owner, false))) => {
                    let Some(exit) = &presence.exit else {
                        *state.borrow_mut() = None;
                        owner.dispose();
                        return;
                    };
                    presence.go(root, exit);
                    if let Some(s) = &mut *state.borrow_mut() {
                        s.exiting = true;
                    }
                    wait_for_exit(state.clone());
                }
                _ => {}
            }
        };
        effect(move || {
            let on = cond.get();
            untrack(|| run(on));
        });
        View(id)
    }
}

/// A keyed list. Items whose key survives an update keep their nodes; only new items are built.
///
/// With [`ForEach::enter`] and [`ForEach::exit`] items animate coming and going, and what is going
/// stays behind what was in front of it until it has gone (see `docs/design/13-animation.md`).
pub fn for_each<T, K, V>(
    items: impl Fn() -> Vec<T> + 'static,
    key: impl Fn(&T) -> K + 'static,
    view: impl Fn(T) -> V + 'static,
) -> ForEach<T, K>
where
    T: 'static,
    K: Eq + Hash + Clone + 'static,
    V: IntoView,
{
    ForEach {
        items: Box::new(items),
        key: Box::new(key),
        view: Box::new(move |item| view(item).into_view()),
        presence: PresenceConfig::default(),
        moves: None,
    }
}

/// See [`for_each`].
pub struct ForEach<T, K> {
    items: Box<dyn Fn() -> Vec<T>>,
    key: Box<dyn Fn(&T) -> K>,
    view: Box<dyn Fn(T) -> View>,
    presence: PresenceConfig,
    moves: Option<Transition>,
}

impl<T, K> ForEach<T, K> {
    presence_methods!();

    /// Has items go to their new place when the list changes, from where they are shown, instead
    /// of being there at once.
    pub fn moves(mut self, timing: Transition) -> Self {
        self.moves = Some(timing).filter(|t| !t.duration.is_zero());
        self
    }
}

/// Keeps where the boxes of `roots` are for when the next layout has them elsewhere.
fn remember_places(
    d: &mut Document,
    list: NodeId,
    timing: Transition,
    roots: impl Iterator<Item = NodeId>,
) {
    // What was there before the first of these changes is what is on screen.
    if d.list_places.iter().any(|p| p.list == list) {
        return;
    }
    let mut targets = Vec::new();
    for root in roots {
        presence_targets(d, root, &mut targets);
    }
    let before = targets
        .into_iter()
        .filter_map(|t| {
            let n = d.get(t).filter(|n| n.visible)?;
            Some((t, PointI::new(n.rect.x, n.rect.y)))
        })
        .collect();
    d.list_places.push(ListPlaces {
        list,
        timing,
        before,
    });
}

struct Row {
    root: NodeId,
    owner: Owner,
    going: bool,
}

/// What a [`for_each`] shows now.
struct Rows<K> {
    rows: HashMap<K, Row>,
    /// The keys in the order they are shown, with what is going. Only kept if things can go.
    shown: Vec<K>,
    /// The keys of what is going.
    going: Vec<K>,
    waiting: bool,
    moves: Option<Transition>,
}

impl<K> Default for Rows<K> {
    fn default() -> Self {
        Self {
            rows: HashMap::new(),
            shown: Vec::new(),
            going: Vec::new(),
            waiting: false,
            moves: None,
        }
    }
}

/// Puts what is going in `rows` back into `list`, each behind what was in front of it in `shown`.
fn keep_places<K: Eq + Hash + Clone>(shown: &[K], list: Vec<K>, rows: &HashMap<K, Row>) -> Vec<K> {
    let mut first = Vec::new();
    let mut behind: HashMap<&K, Vec<&K>> = HashMap::new();
    let mut front = None;
    for k in shown {
        if !rows[k].going {
            front = Some(k);
        } else if let Some(front) = front {
            behind.entry(front).or_default().push(k);
        } else {
            first.push(k);
        }
    }
    let mut out = Vec::with_capacity(rows.len());
    out.extend(first.into_iter().cloned());
    for k in list {
        let following = behind.remove(&k);
        out.push(k);
        out.extend(following.into_iter().flatten().cloned());
    }
    out
}

/// Removes the rows that have finished going, once they have.
fn wait_for_rows<K: Eq + Hash + Clone + 'static>(state: Rc<RefCell<Rows<K>>>, id: NodeId) {
    request_frame(move |_| {
        let mut s = state.borrow_mut();
        let Rows {
            rows,
            shown,
            going,
            moves,
            ..
        } = &mut *s;
        let mut done = Vec::new();
        with_doc(|d| {
            going.retain(|k| {
                let gone = has_gone(d, rows[k].root);
                if gone {
                    done.extend(rows.remove(k));
                }
                !gone
            });
            if !done.is_empty() {
                if let Some(timing) = *moves {
                    remember_places(d, id, timing, rows.values().map(|r| r.root));
                }
                shown.retain(|k| rows.contains_key(k));
                let order = shown.iter().map(|k| rows[k].root).collect();
                d.set_children(id, order);
            }
        });
        let waiting = !going.is_empty();
        s.waiting = waiting;
        drop(s);
        for row in done {
            row.owner.dispose();
        }
        if waiting {
            wait_for_rows(state, id);
        }
    });
}

impl<T, K> IntoView for ForEach<T, K>
where
    T: 'static,
    K: Eq + Hash + Clone + 'static,
{
    fn into_view(self) -> View {
        let ForEach {
            items,
            key,
            view,
            presence,
            moves,
        } = self;
        let id = create_node(Node::new(NodeKind::Fragment));
        let outer = Owner::current();
        let state: Rc<RefCell<Rows<K>>> = Rc::default();
        state.borrow_mut().moves = moves;
        on_cleanup({
            let state = state.clone();
            move || {
                let rows = std::mem::take(&mut *state.borrow_mut()).rows;
                for row in rows.into_values() {
                    row.owner.dispose();
                }
            }
        });
        let animates = presence.enter.is_some() || presence.exit.is_some();
        let first = Cell::new(true);
        effect(move || {
            let list = items();
            let starting = !first.replace(false) || presence.appear;
            let mut old = std::mem::take(&mut state.borrow_mut().rows);
            if let Some(timing) = moves.filter(|_| !old.is_empty()) {
                with_doc(|d| remember_places(d, id, timing, old.values().map(|r| r.root)));
            }
            let mut next = HashMap::with_capacity(list.len());
            let mut order = Vec::with_capacity(list.len());
            let mut keys = Vec::new();
            for item in list {
                let k = key(&item);
                if next.contains_key(&k) {
                    continue;
                }
                let row = match old.remove(&k) {
                    Some(mut row) => {
                        if std::mem::take(&mut row.going) {
                            come_back(row.root);
                        }
                        row
                    }
                    None => {
                        let (root, owner) = match outer {
                            Some(o) => o.with(|| create_child_owner(|| view(item))),
                            None => create_root(|| view(item)),
                        };
                        if animates {
                            presence.begin(root.0, starting);
                        }
                        Row {
                            root: root.0,
                            owner,
                            going: false,
                        }
                    }
                };
                order.push(row.root);
                if presence.exit.is_some() {
                    keys.push(k.clone());
                }
                next.insert(k, row);
            }
            let mut s = state.borrow_mut();
            let mut gone = Vec::new();
            if let Some(exit) = &presence.exit {
                let mut going = std::mem::take(&mut s.going);
                going.retain(|k| old.contains_key(k));
                for (k, mut row) in old {
                    if !row.going {
                        row.going = true;
                        presence.go(row.root, exit);
                        going.push(k.clone());
                    }
                    next.insert(k, row);
                }
                if !going.is_empty() {
                    keys = keep_places(&s.shown, keys, &next);
                    order = keys.iter().map(|k| next[k].root).collect();
                }
                s.going = going;
                s.shown = keys;
            } else {
                gone.extend(old.into_values());
            }
            s.rows = next;
            let wait = !s.going.is_empty() && !std::mem::replace(&mut s.waiting, true);
            drop(s);
            with_doc(|d| d.set_children(id, order));
            // After the children changed in one go: going one by one would take as many walks
            // along the children.
            for row in gone {
                row.owner.dispose();
            }
            if wait {
                wait_for_rows(state.clone(), id);
            }
        });
        View(id)
    }
}

/// Content in a surface of its own, above the window and free to leave it. See
/// `docs/design/09-shadows-stacking-popups.md`.
#[derive(Default)]
pub struct Popup {
    data: PopupData,
    children: Vec<View>,
}

/// Shows its children above everything for as long as it exists, usually inside of a [`show`].
/// By default it is a [`PopupKind::Menu`] below the closest ancestor with a box.
pub fn popup() -> Popup {
    Popup::default()
}

/// Shows its children above the content of the window, as big as the window, wherever it is
/// declared: nothing around it clips or moves it. It draws in the window's own surface, so it
/// costs no surface and stays inside of the window. What isn't covered by its children lets the
/// pointer through. See `docs/design/20-md3e-components.md`.
pub fn portal() -> Popup {
    let mut portal = Popup::default();
    portal.data.portal = true;
    portal
}

impl Popup {
    pub fn kind(mut self, kind: PopupKind) -> Self {
        self.data.kind = kind;
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.data.placement = placement;
        self
    }

    /// Logical pixels between the anchor and the popup.
    pub fn gap(mut self, gap: i32) -> Self {
        self.data.gap = gap;
        self
    }

    /// Keeps Tab inside of it while it exists, and gives the focus back when it goes.
    pub fn modal(mut self) -> Self {
        self.data.modal = true;
        self
    }

    /// Hangs it from this node.
    pub fn anchor(mut self, anchor: NodeRef) -> Self {
        self.data.anchor = Some(anchor);
        self
    }

    /// Hangs it from a rectangle in window coordinates, such as where the pointer was pressed.
    pub fn anchor_rect(mut self, rect: RectI) -> Self {
        self.data.anchor_rect = Some(rect);
        self
    }

    /// Makes it at least as wide as its anchor.
    pub fn match_anchor_width(mut self) -> Self {
        self.data.match_anchor_width = true;
        self
    }

    /// Called when it closes because of something outside of it. It won't show again until it
    /// is built again, so this is the place to update what decides that.
    pub fn on_dismiss(mut self, f: impl Fn() + 'static) -> Self {
        self.data.on_dismiss = Some(Rc::new(f));
        self
    }

    pub fn child(mut self, child: impl IntoView) -> Self {
        self.children.push(child.into_view());
        self
    }
}

impl IntoView for Popup {
    fn into_view(self) -> View {
        if self.data.modal {
            let before = with_doc(|d| d.state.focused);
            on_cleanup(move || {
                with_doc(|d| {
                    if d.pending_focus.is_none() {
                        d.pending_focus = Some(before.filter(|&id| d.nodes.contains_key(id)));
                    }
                });
            });
        }
        let mut node = Node::new(NodeKind::Popup(Box::new(self.data)));
        node.style.base = Style {
            display: Some(Layout::Flex),
            flex_direction: Some(FlexDirection::Column),
            align_items: Some(AlignItems::Stretch),
            user_select: Some(UserSelect::None),
            ..Default::default()
        };
        let id = create_node(node);
        let children: Vec<NodeId> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        View(id)
    }
}

/// A single- or multi-line text field.
pub struct TextInput {
    cfg: ElementConfig,
    multiline: bool,
    rows: u32,
    placeholder: String,
    value: Option<Signal<String>>,
    on_input: Option<TextCallback>,
    on_submit: Option<TextCallback>,
    purpose: ImePurpose,
    read_only: bool,
}

pub fn text_input() -> TextInput {
    TextInput {
        cfg: ElementConfig::default(),
        multiline: false,
        rows: 1,
        placeholder: String::new(),
        value: None,
        on_input: None,
        on_submit: None,
        purpose: ImePurpose::Normal,
        read_only: false,
    }
}

pub fn text_area() -> TextInput {
    TextInput {
        multiline: true,
        rows: 3,
        ..text_input()
    }
}

impl Styled for TextInput {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl TextInput {
    element_methods!();

    /// Two-way binds the text to a signal.
    pub fn value(mut self, value: Signal<String>) -> Self {
        self.value = Some(value);
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Minimum number of visible lines for multi-line inputs.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows;
        self
    }

    pub fn on_input(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_input = Some(Rc::new(f));
        self
    }

    /// Called on Enter in single-line inputs (Ctrl+Enter in multi-line ones).
    pub fn on_submit(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_submit = Some(Rc::new(f));
        self
    }

    pub fn purpose(mut self, purpose: ImePurpose) -> Self {
        self.purpose = purpose;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }
}

impl IntoView for TextInput {
    fn into_view(self) -> View {
        let mut data = EditData::new(self.multiline);
        data.rows = self.rows;
        data.placeholder = self.placeholder;
        data.value = self.value;
        data.on_input = self.on_input;
        data.on_submit = self.on_submit;
        data.purpose = self.purpose;
        data.read_only = self.read_only;
        let mut node = Node::new(NodeKind::Edit(Box::new(data)));
        node.focusable = true;
        let id = self.cfg.build(node);
        if let Some(value) = self.value {
            effect(move || {
                let v = value.get();
                with_doc(|d| {
                    if let Some(e) = d.get_mut(id).and_then(|n| n.edit_mut())
                        && e.text != v
                    {
                        e.set_text(v);
                        d.invalidate_layout(id);
                    }
                });
            });
        }
        View(id)
    }
}
