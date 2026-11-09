//! Builders that create nodes and wire them to the reactive system.
//!
//! A builder only collects configuration; nodes are created in [`IntoView::into_view`], inside
//! the current reactive owner, and removed again when that owner is disposed.

use crate::animation::{ListPlaces, request_frame};
use crate::drag::{DragData, DropConfig, DropEvent, GestureConfig, GrabEvent};
use crate::event::DragResult;
use crate::event::{ClipboardKind, KeyboardEvent, PinchEvent, PointerEvent, ScrollEvent};
use crate::geometry::{PointF, PointI, RectF, RectI, Scale};
use crate::images::Bitmap;
use crate::platform::{ImePurpose, Placement, PlatformRequest, PopupKind};
use crate::reactive::{
    Owner, Signal, batch, create_child_owner, create_root, effect, memo, on_cleanup, signal,
    untrack,
};
use crate::shader::{Image, IntoImage, IntoPaint, ShaderPaint};
use crate::text::TextLayout;
use crate::text::edit::{EditData, TextCallback};
use crate::text::editor::EditorOutcome;
use crate::tree::{
    Document, Handler, Listeners, Node, NodeId, NodeKind, PopupData, TextData, with_doc,
};
use anti_webui_style::{
    AlignItems, Display as Layout, FlexDirection, ImageSource, Sheet, Style, Styled, Transition,
    UserSelect,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;

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
pub(crate) fn create_node(node: Node) -> NodeId {
    let id = with_doc(|d| d.create(node));
    on_cleanup(move || with_doc(|d| d.remove(id)));
    id
}

/// A handle to a node that can be attached with `.node_ref()`. It remembers the window the node is
/// in, so it can be used from the handlers of another window on the same thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef(Signal<Option<(NodeId, u64)>>);

pub fn node_ref() -> NodeRef {
    NodeRef(signal(None))
}

impl NodeRef {
    pub fn get(&self) -> Option<NodeId> {
        self.0.get().map(|(id, _)| id)
    }

    pub(crate) fn id_untracked(&self) -> Option<NodeId> {
        self.0.get_untracked().map(|(id, _)| id)
    }

    pub(crate) fn bind(&self, id: NodeId) {
        self.0.set(Some((id, crate::window::active_id())));
    }

    /// Runs `f` with the node, in its window.
    fn in_window<R>(&self, f: impl FnOnce(NodeId) -> R) -> Option<R> {
        let (id, window) = self.0.get_untracked()?;
        Some(crate::window::within(window, || f(id)))
    }

    /// Shows the node alone on all of the screen, whatever its style and what it is in: the rest of
    /// the window stays as it is and comes back with [`exit_fullscreen`] or Escape. Unless the window
    /// is already on all of the screen, that takes a moment and the window comes back too.
    pub fn request_fullscreen(&self) {
        self.in_window(|id| {
            with_doc(|d| d.request_fullscreen(id));
            sync_fullscreen();
        });
    }

    /// Whether this is what shows on all of the screen. Reading it tracks it.
    pub fn is_fullscreen(&self) -> bool {
        let Some((id, window)) = self.0.get() else {
            return false;
        };
        crate::window::within(window, || fullscreen_signals().1.get() == Some(id))
    }

    /// Scrolls what contains the node, after the next layout, until the node is in view.
    pub fn scroll_into_view(&self) {
        self.in_window(|id| {
            with_doc(|d| {
                d.pending_scroll.push(id);
                d.dirty.paint = true;
            })
        });
    }

    /// The border box in window physical pixels, as of the last layout.
    pub fn rect(&self) -> Option<RectI> {
        self.in_window(|id| with_doc(|d| d.window_rect(id)))?
    }

    /// The border box in window logical pixels, as of the last layout. This is what sizes and
    /// places in styles are written in.
    pub fn logical_rect(&self) -> Option<RectF> {
        self.in_window(|id| {
            with_doc(|d| {
                let r = d.window_rect(id)?;
                let l = |v: i32| d.scale.to_logical(v as f32);
                Some(RectF::new(l(r.x), l(r.y), l(r.width), l(r.height)))
            })
        })?
    }

    /// Where the node's border box is in what an ancestor scrolls (from the ancestor's border box,
    /// as if it hadn't scrolled), in logical pixels, as of the last layout. Unlike
    /// [`rect`](Self::rect), it doesn't change when the ancestor scrolls.
    pub fn offset_in(&self, ancestor: NodeRef) -> Option<PointF> {
        let top = ancestor.id_untracked()?;
        self.in_window(|id| {
            with_doc(|d| {
                let (mut x, mut y) = (0, 0);
                let mut at = id;
                while at != top {
                    let n = d.get(at)?;
                    x += n.rect.x;
                    y += n.rect.y;
                    at = n.parent?;
                    if at != top {
                        let p = d.get(at)?;
                        x -= p.scroll.x;
                        y -= p.scroll.y;
                    }
                }
                Some(PointF::new(
                    d.scale.to_logical(x as f32),
                    d.scale.to_logical(y as f32),
                ))
            })
        })?
    }

    /// How far a scrolling node has scrolled, in logical pixels.
    pub fn scroll_offset(&self) -> PointF {
        self.in_window(|id| {
            with_doc(|d| {
                let n = d.get(id)?;
                Some(PointF::new(
                    d.scale.to_logical(n.scroll.x as f32),
                    d.scale.to_logical(n.scroll.y as f32),
                ))
            })
        })
        .flatten()
        .unwrap_or_default()
    }

    /// Scrolls a scrolling node to this offset in logical pixels, as far as it goes.
    pub fn scroll_to(&self, x: f32, y: f32) {
        self.in_window(|id| {
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
            })
        });
    }

    /// Scrolls a scrolling node to this offset in physical pixels, after the next layout, so that
    /// content changing size in the same frame moves along with it.
    pub fn scroll_to_after_layout(&self, x: i32, y: i32) {
        self.in_window(|id| {
            with_doc(|d| {
                d.pending_scroll_to.push((id, PointI::new(x, y)));
                d.dirty.paint = true;
            })
        });
    }

    /// Whether a point of the window, like [`PointerEvent::position`], is on a scrollbar of this
    /// node or of what it is in, where pressing drags the bar rather than anything else.
    ///
    /// [`PointerEvent::position`]: crate::event::PointerEvent::position
    pub fn is_over_scrollbar(&self, position: PointF) -> bool {
        self.in_window(|id| with_doc(|d| crate::ui::scrollbar_hit(d, id, position).is_some()))
            .unwrap_or(false)
    }

    pub fn focus(&self) {
        self.in_window(|id| with_doc(|d| d.pending_focus = Some(Some(id))));
    }

    pub fn blur(&self) {
        let window = self.0.get_untracked().map_or(0, |(_, w)| w);
        crate::window::within(window, || with_doc(|d| d.pending_focus = Some(None)));
    }

    /// Replaces the text of a text input and selects from one byte offset of it to another, as
    /// something that checks what is typed does. A signal bound with `value` has to be told the same
    /// text, which then changes nothing more. It isn't something to undo.
    pub fn set_text(&self, text: &str, selection: (usize, usize)) {
        self.in_window(|id| {
            with_doc(|d| {
                if let Some(e) = d.get_mut(id).and_then(Node::edit_mut) {
                    e.set_text(text.to_owned());
                    e.select_range(selection.0, selection.1);
                    d.invalidate_layout(id);
                }
            })
        });
    }

    /// Puts the caret of a text input after what it has.
    pub fn select_end(&self) {
        self.in_window(|id| {
            with_doc(|d| {
                if let Some(e) = d.get_mut(id).and_then(Node::edit_mut) {
                    e.caret_to_end();
                    d.mark_paint();
                }
            })
        });
    }
}

pub(crate) type StyleFn = Box<dyn Fn(Style) -> Style>;

/// Keeps the node's dynamic style layers in sync with their closures.
pub(crate) fn bind_dynamic_styles(id: NodeId, layers: Vec<StyleFn>) {
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
pub(crate) fn also<E: 'static>(
    old: Option<Handler<E>>,
    f: impl Fn(&E) + 'static,
) -> Option<Handler<E>> {
    Some(match old {
        Some(old) => Rc::new(move |e| {
            old(e);
            f(e);
        }),
        None => Rc::new(f),
    })
}

/// The same for listeners that are told whether something is so.
pub(crate) fn also_bool(
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
    static FULLSCREEN: Cell<Option<FullscreenSignals>> = const { Cell::new(None) };
}

/// What each window has of its own here; see `crate::window`.
#[derive(Default)]
pub(crate) struct Locals {
    scale: Option<Signal<Scale>>,
    fullscreen: Option<FullscreenSignals>,
}

/// Trades what the window being worked on has for `other`.
pub(crate) fn swap_locals(other: &mut Locals) {
    WINDOW_SCALE.with(|s| other.scale = s.replace(other.scale));
    FULLSCREEN.with(|s| other.fullscreen = s.replace(other.fullscreen));
}

/// Whether the window is on all of the screen, and what shows there alone.
type FullscreenSignals = (Signal<bool>, Signal<Option<NodeId>>);

fn fullscreen_signals() -> FullscreenSignals {
    FULLSCREEN.with(|s| {
        s.get().unwrap_or_else(|| {
            let (signals, _) = create_root(|| {
                let (window, element) = with_doc(|d| (d.fullscreen.window, d.fullscreen.element));
                (signal(window), signal(element))
            });
            s.set(Some(signals));
            signals
        })
    })
}

/// Tells those who are watching what has become of the window. Not while the document is borrowed.
pub(crate) fn sync_fullscreen() {
    let (window, element) = fullscreen_signals();
    let now = with_doc(|d| (d.fullscreen.window, d.fullscreen.element));
    batch(|| {
        window.set_if_changed(now.0);
        element.set_if_changed(now.1);
    });
}

/// Whether the window is on all of the screen, as the platform says. Reading it tracks it.
pub fn window_fullscreen() -> bool {
    fullscreen_signals().0.get()
}

/// Asks to put the window on all of the screen, with all that is in it, or to take it off.
/// [`window_fullscreen`] says when it has happened: the system may have something to say about it.
pub fn set_window_fullscreen(on: bool) {
    with_doc(|d| d.request(PlatformRequest::SetFullscreen(on)));
}

/// Ends what [`NodeRef::request_fullscreen`] started.
pub fn exit_fullscreen() {
    with_doc(Document::exit_fullscreen);
    sync_fullscreen();
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

/// What the pointer and the keyboard are doing to a box, for building a look from: put it on the
/// box with `.interaction(i)`. Reading it tracks it.
#[derive(Clone, Copy)]
pub struct Interaction {
    pub(crate) hovered: Signal<bool>,
    pub(crate) pressed: Signal<bool>,
    pub(crate) focused: Signal<bool>,
    pub(crate) focus_visible: Signal<bool>,
}

impl Default for Interaction {
    fn default() -> Self {
        Self::new()
    }
}

impl Interaction {
    pub fn new() -> Self {
        Self {
            hovered: signal(false),
            pressed: signal(false),
            focused: signal(false),
            focus_visible: signal(false),
        }
    }

    /// The pointer is over the box.
    pub fn hovered(&self) -> bool {
        self.hovered.get()
    }

    /// The box (or something in it) is being pressed.
    pub fn pressed(&self) -> bool {
        self.pressed.get()
    }

    /// The box has the focus.
    pub fn focused(&self) -> bool {
        self.focused.get()
    }

    /// The box has the focus and it should show: it came from the keyboard (CSS `:focus-visible`).
    pub fn focus_visible(&self) -> bool {
        self.focus_visible.get()
    }
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
pub(crate) struct ElementConfig {
    pub(crate) sheet: Option<SheetFn>,
    pub(crate) style: Style,
    pub(crate) hover: Option<Style>,
    pub(crate) focus: Option<Style>,
    pub(crate) active: Option<Style>,
    pub(crate) dragging: Option<Style>,
    pub(crate) drag_over: Option<Style>,
    pub(crate) gesture: GestureConfig,
    pub(crate) drop: DropConfig,
    pub(crate) dynamic: Vec<StyleFn>,
    pub(crate) fill: Option<PaintFn>,
    pub(crate) overlay: Option<PaintFn>,
    pub(crate) filter: Option<ImageFn>,
    pub(crate) listeners: Listeners,
    pub(crate) focusable: bool,
    pub(crate) disabled: Option<Box<dyn Fn() -> bool>>,
    pub(crate) node_ref: Vec<NodeRef>,
    pub(crate) access: Vec<crate::access::AccessFn>,
}

impl ElementConfig {
    pub(crate) fn build(self, mut node: Node) -> NodeId {
        node.style.base = self.style;
        node.style.hover = self.hover.map(Box::new);
        node.style.focus = self.focus.map(Box::new);
        node.style.active = self.active.map(Box::new);
        node.style.dragging = self.dragging.map(Box::new);
        node.style.drag_over = self.drag_over.map(Box::new);
        node.listeners = self.listeners;
        if !self.gesture.is_empty() {
            node.listeners.gesture = Some(Rc::new(self.gesture));
        }
        if !self.drop.is_empty() {
            node.listeners.drop = Some(Rc::new(self.drop));
        }
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
        if !self.access.is_empty() {
            let access = self.access;
            effect(move || {
                let said = access
                    .iter()
                    .fold(crate::access::Access::default(), |a, f| f(a));
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id)
                        && n.access.as_deref() != Some(&said)
                    {
                        n.access = Some(Box::new(said));
                        d.access_stale = true;
                    }
                });
            });
        }
        for r in self.node_ref {
            r.bind(id);
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

        /// A press and release of the primary button on the node, or its activation from the
        /// keyboard. The other buttons are [`on_aux_click`](Self::on_aux_click), as in the DOM.
        pub fn on_click(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.click = also(self.cfg.listeners.click.take(), f);
            self
        }

        /// A press and release of a button other than the primary one (`auxclick`): the middle
        /// button, or the secondary one.
        pub fn on_aux_click(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.aux_click = also(self.cfg.listeners.aux_click.take(), f);
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

        /// Called while fingers move apart or together on a touch surface over the node. Nothing
        /// zooms by itself: it is up to this what that does.
        pub fn on_pinch(mut self, f: impl Fn(&PinchEvent) + 'static) -> Self {
            self.cfg.listeners.pinch = also(self.cfg.listeners.pinch.take(), f);
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

        /// Keeps `i` up to date with what is done to the node: hovered, pressed, focused and
        /// focused from the keyboard. For building a control's look from those.
        pub fn interaction(self, i: crate::view::Interaction) -> Self {
            self.on_hover(move |v| i.hovered.set(v))
                .on_active(move |v| i.pressed.set(v))
                .on_focus_change(move |v| i.focused.set(v))
                .on_focus_visible(move |v| i.focus_visible.set(v))
        }

        /// Called while the node is moved with the primary button held, wherever the pointer goes.
        /// It starts after the pointer moved a little, and pressing without moving stays a click;
        /// moving it far enough keeps it from being one. See `docs/design/23-drag-and-drop.md`.
        pub fn on_grab(mut self, f: impl Fn(&GrabEvent) + 'static) -> Self {
            self.cfg.gesture.grab = also(self.cfg.gesture.grab.take(), f);
            self
        }

        /// How many logical pixels it has to move before it counts as grabbed or dragged. It is 4
        /// by default. With 0, `on_grab` starts when pressed.
        pub fn grab_threshold(mut self, logical: f32) -> Self {
            self.cfg.gesture.threshold = Some(logical);
            self
        }

        /// Scrolls what contains the node when it is held near the edge of that, telling
        /// `on_grab` each time.
        pub fn grab_autoscroll(mut self) -> Self {
            self.cfg.gesture.autoscroll = true;
            self
        }

        /// Makes the node something that can be dragged to where it is taken. When it starts to
        /// move, `f` makes what it carries: any value for this program, or a [`DragData`] to also
        /// offer it to others.
        pub fn draggable<T: 'static>(mut self, f: impl Fn() -> T + 'static) -> Self {
            self.cfg.gesture.data = Some(Rc::new(move || DragData::from_any(f())));
            self
        }

        /// Style applied to the node while it is being dragged.
        pub fn dragging(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.dragging = Some(f(self.cfg.dragging.take().unwrap_or_default()));
            self
        }

        /// What shows with the pointer when a `T` is dragged from here, instead of what the node
        /// looked like. It lives on its own: it shouldn't read signals that go with the node.
        pub fn drag_preview<T: 'static, V: IntoView>(
            mut self,
            f: impl Fn(&T) -> V + 'static,
        ) -> Self {
            self.cfg.gesture.preview = Some(Rc::new(move |data: &DragData| {
                data.value::<T>().map(|v| f(v).into_view())
            }));
            self
        }

        /// Called when what was dragged from here has been dropped or wasn't, if the node is still
        /// there.
        pub fn on_drag_end(mut self, f: impl Fn(DragResult) + 'static) -> Self {
            let old = self.cfg.gesture.end.take();
            self.cfg.gesture.end = Some(Rc::new(move |r| {
                if let Some(old) = &old {
                    old(r);
                }
                f(r);
            }));
            self
        }

        /// Takes a `T` dropped here. It can be given for several types.
        pub fn drop_target<T: 'static>(mut self, f: impl Fn(&T, &DropEvent) + 'static) -> Self {
            self.cfg.drop.typed_drop(f);
            self
        }

        /// Only takes the `T` this says yes to.
        pub fn drop_accepts<T: 'static>(mut self, f: impl Fn(&T) -> bool + 'static) -> Self {
            self.cfg.drop.typed_filter(f);
            self
        }

        /// Called with where a `T` is while it would be taken here. Use [`ExternalDrag`] for what
        /// comes from other programs.
        pub fn on_drag_over<T: 'static>(mut self, f: impl Fn(&T, &DropEvent) + 'static) -> Self {
            self.cfg.drop.typed_over(f);
            self
        }

        /// Told when what is dragged starts and stops being over here where it would be taken.
        /// Also when it is dropped.
        pub fn on_drag_hover(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.drop.hover = also_bool(self.cfg.drop.hover.take(), f);
            self
        }

        /// Style applied while what is dragged is over here where it would be taken.
        pub fn drag_over(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.drag_over = Some(f(self.cfg.drag_over.take().unwrap_or_default()));
            self
        }

        /// Takes files dropped here.
        pub fn on_drop_files(
            mut self,
            f: impl Fn(&[std::path::PathBuf], &DropEvent) + 'static,
        ) -> Self {
            self.cfg.drop.files(f);
            self
        }

        /// Takes text dropped here.
        pub fn on_drop_text(mut self, f: impl Fn(&str, &DropEvent) + 'static) -> Self {
            self.cfg.drop.text(f);
            self
        }

        /// Takes data of this format dropped here.
        pub fn on_drop_data(mut self, mime: &str, f: impl Fn(&[u8], &DropEvent) + 'static) -> Self {
            self.cfg.drop.data(mime, f);
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
            self.cfg.listeners.scrolled = Some(match self.cfg.listeners.scrolled.take() {
                Some(old) => Rc::new(move |p| {
                    old(p);
                    f(p);
                }),
                None => Rc::new(f),
            });
            self
        }

        /// Called with the node's width and height in logical pixels when it first gets a size and
        /// whenever that changes, before anything is painted with it.
        pub fn on_resized(mut self, f: impl Fn(f32, f32) + 'static) -> Self {
            self.cfg.listeners.resized = Some(match self.cfg.listeners.resized.take() {
                Some(old) => Rc::new(move |w, h| {
                    old(w, h);
                    f(w, h);
                }),
                None => Rc::new(f),
            });
            self
        }

        /// Makes the node reachable with Tab and focusable by clicking.
        pub fn focusable(mut self) -> Self {
            self.cfg.focusable = true;
            self
        }

        /// Disables pointer and keyboard handlers while `f` returns true. Given more than once, it
        /// is disabled while any of them says so.
        pub fn disabled(mut self, f: impl Fn() -> bool + 'static) -> Self {
            self.cfg.disabled = Some(match self.cfg.disabled.take() {
                Some(old) => Box::new(move || old() || f()),
                None => Box::new(f),
            });
            self
        }

        /// Says what the node is to assistive technology, like a screen reader: its role, label and
        /// state. `f` runs again when what it reads changes. Given more than once, they run in
        /// order, so that what is said later goes above. See `docs/design/34-accessibility.md`.
        ///
        /// ```ignore
        /// div().access(move |a| a.role(Role::Checkbox).checked(on.get()))
        /// ```
        pub fn access(
            mut self,
            f: impl Fn(crate::access::Access) -> crate::access::Access + 'static,
        ) -> Self {
            self.cfg.access.push(std::rc::Rc::new(f));
            self
        }

        /// Lets `r` refer to this node. A node can have several of them.
        pub fn node_ref(mut self, r: NodeRef) -> Self {
            self.cfg.node_ref.push(r);
            self
        }
    };
}

/// What is for boxes: a shape in an `svg` has `fill` for something else.
macro_rules! box_paint_methods {
    () => {
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
    };
}

pub(crate) use element_methods;

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
    box_paint_methods!();

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

/// Where an `img` takes its picture from: something that makes an [`ImageSource`], or a closure that
/// returns one.
pub trait IntoImageSource {
    fn into_image_source(self) -> Box<dyn Fn() -> ImageSource>;
}

impl<F, S> IntoImageSource for F
where
    F: Fn() -> S + 'static,
    S: Into<ImageSource>,
{
    fn into_image_source(self) -> Box<dyn Fn() -> ImageSource> {
        Box::new(move || self().into())
    }
}

macro_rules! image_sources {
    ($($ty:ty),* $(,)?) => {$(
        impl IntoImageSource for $ty {
            fn into_image_source(self) -> Box<dyn Fn() -> ImageSource> {
                let source: ImageSource = self.into();
                Box::new(move || source.clone())
            }
        }
    )*};
}

image_sources!(
    ImageSource,
    &str,
    String,
    &std::path::Path,
    std::path::PathBuf,
    &'static [u8],
    Vec<u8>,
    std::sync::Arc<[u8]>,
);

impl<const N: usize> IntoImageSource for &'static [u8; N] {
    fn into_image_source(self) -> Box<dyn Fn() -> ImageSource> {
        Box::new(move || self.into())
    }
}

/// A picture in a box, like `<img>`. See `docs/design/22-images.md`.
pub struct Img {
    cfg: ElementConfig,
    source: Box<dyn Fn() -> ImageSource>,
    data: crate::tree::ImageData,
    children: Vec<View>,
}

/// A picture from a file, or from something in memory. When a closure says something else, what
/// shows stays until that is there to show.
pub fn img(source: impl IntoImageSource) -> Img {
    Img {
        cfg: ElementConfig::default(),
        source: source.into_image_source(),
        data: crate::tree::ImageData {
            density: 1.0,
            ..Default::default()
        },
        children: Vec::new(),
    }
}

impl Styled for Img {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Img {
    element_methods!();
    box_paint_methods!();

    /// How many pixels of the picture make a logical pixel, for a picture made for screens with
    /// more pixels (`srcset="a.png 2x"`). The box gets smaller, and the picture sharper.
    pub fn density(mut self, density: f32) -> Self {
        self.data.density = density.max(f32::EPSILON);
        self
    }

    /// Called when the size of the picture is known, in logical pixels.
    pub fn on_load(mut self, f: impl Fn(f32, f32) + 'static) -> Self {
        self.data.on_load = Some(Rc::new(f));
        self
    }

    /// Called when there are pixels to show. This waits until the picture comes near the screen.
    pub fn on_decoded(mut self, f: impl Fn() + 'static) -> Self {
        self.data.on_decoded = Some(Rc::new(f));
        self
    }

    /// Called when the picture can't be shown, and with why.
    pub fn on_error(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.data.on_error = Some(Rc::new(f));
        self
    }

    /// Lays something over the picture, in its content box. It has no say in the size of the box.
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

impl IntoView for Img {
    fn into_view(self) -> View {
        let id = self
            .cfg
            .build(Node::new(NodeKind::Image(Box::new(self.data))));
        let children: Vec<NodeId> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        let source = self.source;
        effect(move || {
            let source = source();
            with_doc(|d| d.set_image_source(id, source));
        });
        View(id)
    }
}

/// Pixels the program made itself, filling the content box. See [`bitmap_view`].
pub struct BitmapView {
    cfg: ElementConfig,
    bitmap: Box<dyn Fn() -> Option<Arc<Bitmap>>>,
}

/// Ends the selection of the text of the application, for what has a selection of its own: there is
/// one selection to be seen.
pub fn clear_text_selection() {
    with_doc(|d| {
        if d.state.selection.take().is_some() {
            d.mark_paint();
        }
    });
}

/// Puts text on a clipboard, to be pasted from it in any program.
pub fn set_clipboard(kind: ClipboardKind, text: impl Into<String>) {
    let text = text.into();
    with_doc(|d| d.request(PlatformRequest::SetClipboard { kind, text }));
}

/// Shows pixels the program made, as they are, without decoding, scaling or caching anything: no
/// bitmap shows nothing. `bitmap` runs again when the signals it reads change, and a new bitmap
/// shows from the next frame. The size of the box is up to the style.
pub fn bitmap_view(bitmap: impl Fn() -> Option<Arc<Bitmap>> + 'static) -> BitmapView {
    BitmapView {
        cfg: ElementConfig::default(),
        bitmap: Box::new(bitmap),
    }
}

impl Styled for BitmapView {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl BitmapView {
    element_methods!();
    box_paint_methods!();
}

impl IntoView for BitmapView {
    fn into_view(self) -> View {
        let id = self.cfg.build(Node::new(NodeKind::Image(Box::new(
            crate::tree::ImageData {
                density: 1.0,
                ..Default::default()
            },
        ))));
        let bitmap = self.bitmap;
        effect(move || {
            let bitmap = bitmap();
            with_doc(|d| {
                if let Some(NodeKind::Image(data)) = d.get_mut(id).map(|n| &mut n.kind) {
                    data.fallback = bitmap;
                    d.mark_paint();
                }
            });
        });
        View(id)
    }
}

/// A moving picture in a box, like `<video>`. See `docs/design/24-video.md`.
pub struct Video {
    cfg: ElementConfig,
    source: Box<dyn Fn() -> String>,
    data: crate::video::VideoData,
    autoplay: bool,
    muted: bool,
    children: Vec<View>,
}

/// What a [`video`] plays: a path, a URL, or a closure saying which.
pub trait IntoVideoSource {
    fn into_video_source(self) -> Box<dyn Fn() -> String>;
}

impl IntoVideoSource for &str {
    fn into_video_source(self) -> Box<dyn Fn() -> String> {
        let source = self.to_owned();
        Box::new(move || source.clone())
    }
}

impl IntoVideoSource for String {
    fn into_video_source(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}

impl IntoVideoSource for &std::path::Path {
    fn into_video_source(self) -> Box<dyn Fn() -> String> {
        self.to_string_lossy().into_owned().into_video_source()
    }
}

impl IntoVideoSource for std::path::PathBuf {
    fn into_video_source(self) -> Box<dyn Fn() -> String> {
        self.to_string_lossy().into_owned().into_video_source()
    }
}

impl<F, S> IntoVideoSource for F
where
    F: Fn() -> S + 'static,
    S: Into<String>,
{
    fn into_video_source(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self().into())
    }
}

/// A video from a file or a URL. It shows its first frame until told to play. When a closure says
/// something else, it starts over with that.
pub fn video(source: impl IntoVideoSource) -> Video {
    Video {
        cfg: ElementConfig::default(),
        source: source.into_video_source(),
        data: crate::video::VideoData::default(),
        autoplay: false,
        muted: false,
        children: Vec::new(),
    }
}

impl Styled for Video {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Video {
    element_methods!();
    box_paint_methods!();

    /// What tells it what to do, and says how far it has come.
    pub fn player(mut self, player: crate::video::VideoPlayer) -> Self {
        self.data.player = Some(player);
        self
    }

    /// Starts playing as soon as it can.
    pub fn autoplay(mut self, autoplay: bool) -> Self {
        self.autoplay = autoplay;
        self
    }

    /// Starts over when it ends.
    pub fn looped(mut self, looped: bool) -> Self {
        self.data.looped = looped;
        self
    }

    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Called when its size and length are known: the width and the height in logical pixels, and
    /// the seconds, or `NaN` for what has no known end.
    pub fn on_load(mut self, f: impl Fn(f32, f32, f64) + 'static) -> Self {
        self.data.on_load = Some(Rc::new(f));
        self
    }

    /// Called when it has played to the end.
    pub fn on_ended(mut self, f: impl Fn() + 'static) -> Self {
        self.data.on_ended = Some(Rc::new(f));
        self
    }

    /// Called when it can't be shown, and with why.
    pub fn on_error(mut self, f: impl Fn(&crate::video::VideoError) + 'static) -> Self {
        self.data.on_error = Some(Rc::new(f));
        self
    }

    /// Lays something over the video, in its content box, like controls. It has no say in the size
    /// of the box.
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

impl IntoView for Video {
    fn into_view(mut self) -> View {
        // Without one given there is still one to play and to pause by.
        let player = *self
            .data
            .player
            .get_or_insert_with(crate::video::video_player);
        if self.autoplay {
            player.playing.set(true);
        }
        if self.muted {
            player.muted.set(true);
        }
        let id = self
            .cfg
            .build(Node::new(NodeKind::Video(Box::new(self.data))));
        let children: Vec<NodeId> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        let source = self.source;
        effect(move || {
            let source = source();
            player.ended.set(false);
            player.time.set(0.0);
            player.duration.set(f64::NAN);
            with_doc(|d| crate::video::set_source(d, id, source));
        });
        effect(move || {
            let playing = player.playing.get();
            crate::video::control(id, |s| s.set_playing(playing));
        });
        effect(move || {
            let muted = player.muted.get();
            crate::video::control(id, |s| s.set_muted(muted));
        });
        effect(move || {
            let volume = player.volume.get();
            crate::video::control(id, |s| s.set_volume(volume));
        });
        effect(move || {
            let (seconds, asked) = player.seek.get();
            if asked > 0 {
                crate::video::control(id, |s| s.seek(seconds));
            }
        });
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

impl IntoTextSource for TextSource {
    fn into_text_source(self) -> TextSource {
        self
    }
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
            zoom: 1.0,
            in_svg: false,
            rich: None,
            row: None,
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
    pub(crate) fn data_mut(&mut self) -> &mut PopupData {
        &mut self.data
    }

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

/// A handle to a `text_editor`, to change its text from outside as one step of undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorRef(Signal<Option<NodeId>>);

pub fn editor_ref() -> EditorRef {
    EditorRef(signal(None))
}

impl EditorRef {
    fn change(&self, f: impl FnOnce(&mut crate::text::editor::EditorData) -> EditorOutcome) {
        let Some(id) = self.0.get_untracked() else {
            return;
        };
        let out = with_doc(|d| d.get_mut(id).and_then(Node::editor_mut).map(f));
        if let Some(out) = out {
            crate::text::editor::after_edit(id, out);
        }
    }

    /// Replaces a range of bytes of the text, as one step of undo.
    pub fn replace(&self, range: std::ops::Range<usize>, text: &str) {
        self.change(|e| e.replace(range, text));
    }

    /// Replaces a range of bytes of the text, as one step of undo, and then selects from one byte
    /// to another in the new text: the same as `replace` and `select`, in one go.
    pub fn replace_and_select(
        &self,
        range: std::ops::Range<usize>,
        text: &str,
        anchor: usize,
        focus: usize,
    ) {
        self.change(|e| {
            let out = e.replace(range, text);
            e.select(anchor, focus);
            e.keep_selection_after();
            EditorOutcome {
                changed: out.changed,
                moved: true,
            }
        });
    }

    /// Selects from one byte to another.
    pub fn select(&self, anchor: usize, focus: usize) {
        self.change(|e| {
            e.select(anchor, focus);
            EditorOutcome {
                changed: false,
                moved: true,
            }
        });
    }

    /// Looks at the text and the selection, without tracking them.
    pub fn with<R>(&self, f: impl FnOnce(&crate::text::editor::EditorData) -> R) -> Option<R> {
        let id = self.0.get_untracked()?;
        with_doc(|d| d.get(id)?.editor().map(f))
    }

    /// The byte of the text under a point in window coordinates.
    pub fn offset_at(&self, p: PointF) -> Option<usize> {
        let id = self.0.get_untracked()?;
        with_doc(|d| crate::text::editor::hit(d, id, p))
    }

    /// Asks the decorations again for the rows that are shown, when what they depend on changed
    /// without the text or the selection changing (colors that came from another thread).
    pub fn redecorate(&self) {
        let Some(id) = self.0.get_untracked() else {
            return;
        };
        with_doc(|d| {
            if let Some(e) = d.get_mut(id).and_then(Node::editor_mut) {
                e.looks_version += 1;
            }
        });
        crate::text::editor::sync(id);
    }

    /// The line at the top of the editor's box as of the last layout, with how far down into it:
    /// `10.5` is halfway down line 10 (from 0). For keeping something else in step with it.
    pub fn top_line(&self) -> Option<f64> {
        let id = self.0.get_untracked()?;
        with_doc(|d| crate::text::editor::top_line(d, id))
    }

    /// Scrolls until a line, as [`top_line`](Self::top_line) gives it, is at the top of the
    /// editor's box, as far as it goes.
    pub fn scroll_to_line(&self, line: f64) {
        if let Some(id) = self.0.get_untracked() {
            with_doc(|d| crate::text::editor::scroll_to_line(d, id, line));
        }
    }

    /// The text, without tracking it.
    pub fn text(&self) -> String {
        self.0
            .get_untracked()
            .and_then(|id| with_doc(|d| Some(d.get(id)?.editor()?.text.to_string())))
            .unwrap_or_default()
    }

    /// Gives it the keyboard focus.
    pub fn focus(&self) {
        if let Some(id) = self.0.get_untracked() {
            with_doc(|d| d.pending_focus = Some(Some(id)));
        }
    }
}

/// Text edited line by line, each line a node shaped only near the screen, with decorations that
/// style, hide and replace parts of it. See "M3" in `docs/design/28-markdown.md`.
pub struct TextEditor {
    cfg: ElementConfig,
    text: Option<String>,
    value: Option<Signal<String>>,
    on_change: Option<TextCallback>,
    decorate: Option<Rc<crate::text::editor::Decorate>>,
    decorate_lines: Option<Rc<crate::text::editor::DecorateLines>>,
    blocks: Option<Rc<crate::text::editor::Blocks>>,
    on_edit: Option<Rc<crate::text::editor::OnEdit>>,
    on_select: Option<Rc<crate::text::editor::OnSelect>>,
    gutter: Option<crate::text::editor::Gutter>,
    editor_ref: Option<EditorRef>,
    read_only: bool,
}

pub fn text_editor() -> TextEditor {
    TextEditor {
        cfg: ElementConfig::default(),
        text: None,
        value: None,
        on_change: None,
        decorate: None,
        decorate_lines: None,
        blocks: None,
        on_edit: None,
        on_select: None,
        gutter: None,
        editor_ref: None,
        read_only: false,
    }
}

impl Styled for TextEditor {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl TextEditor {
    element_methods!();
    box_paint_methods!();

    /// The text it starts with, when it isn't bound to a signal. A long text is cheaper this way:
    /// a bound signal is given a copy of the whole text at every change.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Two-way binds the text to a signal.
    pub fn value(mut self, value: Signal<String>) -> Self {
        self.value = Some(value);
        self
    }

    /// Called with the text whenever it changes.
    pub fn on_change(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// How the lines look: given the text, its lines and the selection, a row for each line or
    /// for some lines together, from the first line to the last. It runs whenever the text or the
    /// selection changes, so it should give the same `Rc` for what stays the same.
    pub fn decorate(
        mut self,
        f: impl Fn(&crate::text::editor::DecorateContext) -> Vec<crate::text::editor::Row> + 'static,
    ) -> Self {
        self.decorate = Some(Rc::new(f));
        self
    }

    /// How the lines that get rows look, asked only for those: when they come into view, when the
    /// text or the selection changes, and on [`EditorRef::redecorate`]. One look for each line of
    /// the range. It should give the same `Rc` for a line that looks the same.
    pub fn decorate_lines(
        mut self,
        f: impl Fn(
            &crate::text::editor::DecorateContext,
            std::ops::Range<usize>,
        ) -> Vec<Rc<crate::text::editor::LineLook>>
        + 'static,
    ) -> Self {
        self.decorate_lines = Some(Rc::new(f));
        self
    }

    /// With `decorate_lines`: the rows that show several lines as one node (folded code), each by
    /// its first line, in order. Asked whenever the text or the selection changes.
    pub fn blocks(
        mut self,
        f: impl Fn(&crate::text::editor::DecorateContext) -> Vec<(usize, crate::text::editor::Row)>
        + 'static,
    ) -> Self {
        self.blocks = Some(Rc::new(f));
        self
    }

    /// Told of every change to the text, before the decorations are asked again.
    pub fn on_edit(mut self, f: impl Fn(&crate::text::editor::TextEdit) + 'static) -> Self {
        self.on_edit = Some(Rc::new(f));
        self
    }

    /// Told of the selection, as its anchor and focus in bytes, whenever it moves or the text
    /// changes.
    pub fn on_select(mut self, f: impl Fn(usize, usize) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Line numbers on the left, which stay there when the text scrolls across.
    pub fn gutter(mut self, gutter: crate::text::editor::Gutter) -> Self {
        self.gutter = Some(gutter);
        self
    }

    pub fn editor_ref(mut self, r: EditorRef) -> Self {
        self.editor_ref = Some(r);
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }
}

impl IntoView for TextEditor {
    fn into_view(mut self) -> View {
        if self.cfg.style.cursor.is_none() {
            self.cfg.style.cursor = Some(Some(anti_webui_style::CursorIcon::Text));
        }
        let mut data = crate::text::editor::EditorData::new();
        let start = self.value.map(|v| v.get_untracked()).or(self.text);
        if let Some(text) = start {
            data.set_text(&text);
            data.select(0, 0);
            data.clear_history();
        }
        data.value = self.value;
        data.on_change = self.on_change;
        data.decorate = self.decorate;
        data.decorate_lines = self.decorate_lines;
        data.blocks = self.blocks;
        data.on_edit = self.on_edit;
        data.on_select = self.on_select;
        data.gutter = self.gutter;
        data.read_only = self.read_only;
        data.owner = Owner::current();
        let mut node = Node::new(NodeKind::Editor(Box::new(data)));
        node.focusable = true;
        let id = self.cfg.build(node);
        if let Some(r) = self.editor_ref {
            r.0.set(Some(id));
        }
        on_cleanup(move || crate::text::editor::dispose_rows(id));
        crate::text::editor::sync(id);
        if let Some(value) = self.value {
            effect(move || {
                let v = value.get();
                let out = with_doc(|d| {
                    d.get_mut(id)
                        .and_then(Node::editor_mut)
                        .map(|e| e.set_text(&v))
                });
                if let Some(out) = out
                    && out.changed
                {
                    crate::text::editor::after_edit(id, out);
                }
            });
        }
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
    box_paint_methods!();

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
