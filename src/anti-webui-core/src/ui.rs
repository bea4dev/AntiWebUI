//! The per-window UI state machine: input handling, selection, editing, focus and frames.

use crate::animation;
use crate::event::*;
use crate::geometry::PointI;
use crate::geometry::{PointF, RectI, Scale, SizeI};
use crate::layout::{
    build_stacking, child_clip, layout_document, position_document, reposition_children,
    resolve_styles,
};
use crate::paint::{CARET_BLINK, Frame, caret_width, edit_text_origin, paint_document};
use crate::platform::{
    CursorIcon, ImeState, Placement, PlatformRequest, PopupId, PopupKind, PopupParams,
};
use crate::reactive::{Owner, batch, create_root};
use crate::scene::Scene;
use crate::scroll::{Booster, FLING_SPEED_RATIO, Fling, VelocityTracker};
use crate::settings::with_settings;
use crate::text::edit::{EditOutcome, Motion};
use crate::text::selection::{self, Granularity};
use crate::text::with_text;
use crate::tree::{Document, Handler, Node, NodeId, NodeKind, popup_id, popup_node, with_doc};
use crate::view::IntoView;
use anti_webui_style::{AlignItems, Display, FlexDirection, Overflow, Style, UserSelect};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const DOUBLE_CLICK_MS: u32 = 500;
/// Logical pixels the pointer may move before it stops inertia: fingers rarely leave a touchpad at
/// the same moment, and the last one can still move the pointer a little.
const MOMENTUM_MOVE_SLOP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    /// Selecting document text.
    Select,
    /// Selecting inside an edit node.
    Edit(NodeId),
    /// Dragging a scrollbar thumb: node, vertical?, pointer offset into the content.
    Scrollbar(NodeId, bool, f32),
}

struct Press {
    target: Option<NodeId>,
    button: PointerButton,
    drag: Drag,
}

/// A touchpad gesture from its first motion to the end of its inertia, which goes to the same
/// nodes however the content moves under the pointer.
struct ScrollGesture {
    position: PointF,
    /// The nodes under the pointer when it began, innermost first.
    chain: Vec<NodeId>,
    /// What it scrolls, once there was motion in a direction something can scroll to.
    scroller: Option<NodeId>,
    source: ScrollSource,
    /// Shift turns vertical motion into horizontal scrolling.
    shift: bool,
    velocity: VelocityTracker,
    /// The fingers left; only inertia follows.
    lifted: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Scrolled {
    /// A handler took it.
    Handled,
    /// The content moved, or would with more motion.
    Moving,
    /// There is nothing to scroll in this direction.
    Stuck,
}

pub struct Ui {
    owner: Owner,
    pointer: Option<PointF>,
    modifiers: Modifiers,
    press: Option<Press>,
    last_click: Option<(u32, PointF, PointerButton, u32)>,
    /// The focus being set is reached with the keyboard.
    focus_by_keyboard: bool,
    cursor: Option<CursorIcon>,
    ime_sent: Option<Option<ImeState>>,
    /// Where a pending paste should go.
    paste_target: Option<NodeId>,
    gesture: Option<ScrollGesture>,
    fling: Option<Fling>,
    booster: Option<Booster>,
    /// The fraction of a pixel not yet scrolled, and the node it belongs to.
    scroll_rest: Option<(NodeId, f32, f32)>,
}

fn pointer_event(
    position: PointF,
    rect: RectI,
    button: Option<PointerButton>,
    modifiers: Modifiers,
    click_count: u32,
) -> PointerEvent {
    PointerEvent {
        position,
        local: PointF::new(position.x - rect.x as f32, position.y - rect.y as f32),
        rect,
        button,
        modifiers,
        click_count,
        stopped: Cell::new(false),
    }
}

#[derive(Clone, Copy)]
enum PointerKind {
    Down,
    Up,
    Move,
    Click,
}

impl Ui {
    /// Mounts `root` as the window content.
    pub fn new<V: IntoView>(root: impl FnOnce() -> V) -> Self {
        let ((), owner) = create_root(|| {
            let mut node = Node::new(NodeKind::Element);
            node.style.base = Style {
                display: Some(Display::Flex),
                flex_direction: Some(FlexDirection::Column),
                align_items: Some(AlignItems::Stretch),
                ..Default::default()
            };
            let root_id = with_doc(|d| d.create(node));
            let view = root().into_view();
            with_doc(|d| {
                d.root = Some(root_id);
                d.set_children(root_id, vec![view.0]);
            });
        });
        Self {
            owner,
            pointer: None,
            modifiers: Modifiers::default(),
            press: None,
            last_click: None,
            focus_by_keyboard: false,
            cursor: None,
            ime_sent: None,
            paste_target: None,
            gesture: None,
            fling: None,
            booster: None,
            scroll_rest: None,
        }
    }

    /// Recomputes styles, layout and positions as needed.
    pub fn update(&mut self) {
        self.apply_pending_focus();
        // Content moving under a stationary pointer can change what is hovered, which can change
        // styles and even layout again.
        for _ in 0..4 {
            let changed = self.update_once();
            if changed
                && self.press.is_none()
                && let Some(p) = self.pointer
            {
                self.update_hover(p);
            }
            if !with_doc(|d| Self::needs_update(d)) {
                break;
            }
        }
        self.sync_ime();
        self.sync_popups();
    }

    /// Brings styles, layout and positions up to date. Returns whether anything moved.
    fn update_once(&mut self) -> bool {
        with_doc(|doc| {
            with_text(|ts| {
                let mut changed = false;
                if doc.dirty.style {
                    resolve_styles(doc);
                    doc.dirty.style = false;
                }
                if doc.dirty.layout {
                    layout_document(doc, ts);
                    doc.dirty.layout = false;
                    doc.dirty.position = true;
                    changed = true;
                }
                if doc.dirty.position {
                    position_document(doc);
                    doc.dirty.position = false;
                    doc.reposition.clear();
                    doc.dirty.carets = true;
                    doc.dirty.paint = true;
                    changed = true;
                } else if !doc.reposition.is_empty() {
                    for id in std::mem::take(&mut doc.reposition) {
                        reposition_children(doc, id);
                    }
                    doc.dirty.paint = true;
                    changed = true;
                }
                if std::mem::take(&mut doc.transform_moved) {
                    changed = true;
                }
                if doc.stacking_dirty {
                    build_stacking(doc);
                    doc.dirty.paint = true;
                    doc.dirty.content = true;
                    changed = true;
                }
                if !doc.list_places.is_empty() {
                    animation::start_moves(doc);
                }
                if doc.dirty.carets {
                    doc.dirty.carets = false;
                    scroll_carets_into_view(doc);
                }
                for id in std::mem::take(&mut doc.pending_scroll) {
                    scroll_into_view(doc, id);
                }
                changed
            })
        })
    }

    fn needs_update(d: &Document) -> bool {
        d.dirty.style
            || d.dirty.layout
            || d.dirty.position
            || d.dirty.carets
            || d.stacking_dirty
            || !d.pending_scroll.is_empty()
            || !d.reposition.is_empty()
    }

    pub fn needs_redraw(&self) -> bool {
        self.animating() || with_doc(|d| d.dirty.paint || Self::needs_update(d))
    }

    /// Whether frames must keep coming even if nothing else changes. See [`Ui::animate`].
    pub fn animating(&self) -> bool {
        self.fling.is_some()
            || animation::frame_requested()
            || with_doc(|d| d.settling || !d.transitioning.is_empty())
    }

    /// Advances animations to `now`. Call before [`Ui::update`] when producing a frame.
    pub fn animate(&mut self, now: Instant) {
        animation::run_frame_callbacks(now);
        with_doc(|d| animation::advance_transitions(d, now));
        let Some(fling) = &mut self.fling else {
            return;
        };
        let (dx, dy, mut done) = fling.step(now);
        if dx != 0.0 || dy != 0.0 {
            let p = self.pointer;
            done |= self.deliver_scroll(p, dx, dy, ScrollPhase::Momentum) == Scrolled::Stuck;
        }
        if done {
            self.end_scroll_gesture();
        }
    }

    /// Produces the window's part of the frame. Call [`Ui::update`] first.
    pub fn paint(&mut self, now: Instant) -> Scene {
        self.paint_frame(now).window
    }

    /// Produces the frame. Call [`Ui::update`] first.
    pub fn paint_frame(&mut self, now: Instant) -> Frame {
        with_doc(|d| {
            let frame = paint_document(d, now);
            d.dirty.paint = false;
            d.dirty.content = false;
            frame
        })
    }

    /// Tells the platform what changed about the popups.
    fn sync_popups(&mut self) {
        for id in with_doc(|d| d.popups.clone()) {
            let Some(params) = with_doc(|d| popup_params(d, id)) else {
                let sent = with_doc(|d| {
                    d.nodes.get_mut(id)?.popup_mut()?.granted = None;
                    d.nodes.get_mut(id)?.popup_mut()?.sent.take()
                });
                if sent.is_some() {
                    with_doc(|d| d.request(PlatformRequest::ClosePopup(popup_id(id))));
                }
                continue;
            };
            let sent = with_doc(|d| d.nodes[id].popup().and_then(|p| p.sent.clone()));
            if sent.as_ref() == Some(&params) {
                continue;
            }
            // A grab goes on the topmost grabbing popup, so other menus have to go first.
            if sent.is_none() && params.kind == PopupKind::Menu {
                let others = with_doc(|d| open_menus_outside(d, Some(id)));
                self.dismiss_popups(&others);
            }
            with_doc(|d| {
                let req = if sent.is_some() {
                    PlatformRequest::UpdatePopup(params.clone())
                } else {
                    PlatformRequest::ShowPopup(params.clone())
                };
                d.request(req);
                d.nodes[id].popup_mut().unwrap().sent = Some(params);
            });
        }
    }

    /// Closes these popups and what is attached to them, and tells whoever wants to know.
    fn dismiss_popups(&mut self, ids: &[NodeId]) {
        let handlers = with_doc(|d| {
            let mut handlers = Vec::new();
            let all: Vec<NodeId> = d
                .popups
                .iter()
                .copied()
                .filter(|&p| ids.iter().any(|&id| d.is_ancestor(id, p)))
                .collect();
            for id in all {
                let Some(data) = d.nodes[id].popup_mut() else {
                    continue;
                };
                if data.dismissed {
                    continue;
                }
                data.dismissed = true;
                data.granted = None;
                let sent = data.sent.take().is_some();
                handlers.extend(data.on_dismiss.clone());
                if sent {
                    d.request(PlatformRequest::ClosePopup(popup_id(id)));
                }
            }
            d.dirty.position = true;
            d.mark_paint();
            handlers
        });
        batch(|| {
            for h in handlers {
                h();
            }
        });
    }

    fn popup_configured(&mut self, id: PopupId, origin: PointI, size: SizeI) {
        with_doc(|d| {
            let node = popup_node(id);
            let Some(data) = d.get_mut(node).and_then(Node::popup_mut) else {
                return;
            };
            if data.dismissed {
                return;
            }
            let resized = data.granted != Some(size);
            data.origin = origin;
            data.granted = Some(size);
            if resized {
                d.invalidate_layout(node);
            }
            d.mark_paint();
        });
    }

    pub fn take_requests(&mut self) -> Vec<PlatformRequest> {
        with_doc(|d| std::mem::take(&mut d.requests))
    }

    /// The next time the UI needs a repaint without input (caret blinking).
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        with_doc(|d| {
            let id = d.state.focused?;
            if !d.state.window_focused {
                return None;
            }
            let edit = d.get(id)?.edit()?;
            let elapsed = now.saturating_duration_since(edit.caret_epoch);
            let periods = elapsed.as_millis() / CARET_BLINK.as_millis() + 1;
            Some(edit.caret_epoch + CARET_BLINK * periods as u32)
        })
    }

    /// Call when a deadline from [`Ui::next_deadline`] passes.
    pub fn tick(&mut self, _now: Instant) {
        with_doc(|d| {
            if d.state
                .focused
                .and_then(|id| d.get(id))
                .is_some_and(Node::is_edit)
            {
                d.mark_paint();
            }
        });
    }

    pub fn scale(&self) -> Scale {
        with_doc(|d| d.scale)
    }

    pub fn handle_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Resized { size, scale } => self.resize(size, scale),
            WindowEvent::PointerMoved {
                position,
                modifiers,
            } => {
                self.modifiers = modifiers;
                if self.gesture.as_ref().is_some_and(|g| {
                    let slop = MOMENTUM_MOVE_SLOP * with_doc(|d| d.scale).as_f64() as f32;
                    g.lifted && (position.x - g.position.x).hypot(position.y - g.position.y) > slop
                }) {
                    self.end_scroll_gesture();
                }
                self.pointer_moved(position);
            }
            WindowEvent::PointerLeft => {
                self.end_scroll_gesture();
                self.pointer = None;
                if self.press.is_none() {
                    self.set_hovered(Vec::new());
                }
            }
            WindowEvent::PointerButton {
                position,
                button,
                pressed,
                modifiers,
                time,
            } => {
                self.modifiers = modifiers;
                self.pointer = Some(position);
                if pressed {
                    self.end_scroll_gesture();
                    self.pointer_down(position, button, time);
                } else {
                    self.pointer_up(position, button);
                }
            }
            WindowEvent::Scroll {
                position,
                delta_x,
                delta_y,
                modifiers,
                source,
                phase,
                time,
            } => {
                self.modifiers = modifiers;
                self.scroll(position, delta_x, delta_y, source, phase, time);
            }
            WindowEvent::Key(key) => {
                self.modifiers = key.modifiers;
                if key.pressed {
                    self.end_scroll_gesture();
                }
                self.key(key);
            }
            WindowEvent::Ime(update) => self.ime(update),
            WindowEvent::Paste { text, .. } => self.paste(text),
            WindowEvent::Focused(focused) => {
                with_doc(|d| {
                    d.state.window_focused = focused;
                    if let Some(id) = d.state.focused
                        && let Some(e) = d.get_mut(id).and_then(Node::edit_mut)
                    {
                        e.caret_epoch = Instant::now();
                    }
                    d.mark_paint();
                });
                if !focused {
                    self.cancel_press();
                }
            }
            WindowEvent::CloseRequested => with_doc(|d| d.request(PlatformRequest::Close)),
            WindowEvent::PopupConfigured { id, origin, size } => {
                self.popup_configured(id, origin, size)
            }
            WindowEvent::PopupDismissed(id) => self.dismiss_popups(&[popup_node(id)]),
        }
        self.sync_ime();
    }

    fn resize(&mut self, size: SizeI, scale: Scale) {
        with_doc(|d| {
            if d.viewport != size || d.scale != scale {
                if d.scale != scale {
                    // Every resolved style and text measurement depends on the scale.
                    d.mark_style_all();
                    d.invalidate_all_layout();
                }
                d.viewport = size;
                d.scale = scale;
                d.mark_layout();
            }
        });
    }

    fn ensure_layout(&mut self) {
        if with_doc(|d| Self::needs_update(d)) {
            self.update();
        }
    }

    fn hit_test(&self, p: PointF) -> Option<NodeId> {
        with_doc(|d| match d.popup_at(p) {
            Some((popup, local)) => hit(d, popup, local),
            None => d.root.and_then(|r| hit(d, r, p)),
        })
    }

    fn update_hover(&mut self, p: PointF) {
        let target = self.hit_test(p);
        let chain = target
            .map(|t| with_doc(|d| d.ancestors(t)))
            .unwrap_or_default();
        self.set_hovered(chain);
        let cursor = with_doc(|d| cursor_for(d, target));
        if self.cursor != Some(cursor) {
            self.cursor = Some(cursor);
            with_doc(|d| d.request(PlatformRequest::SetCursor(cursor)));
        }
    }

    fn set_hovered(&mut self, chain: Vec<NodeId>) {
        let (entered, left) = with_doc(|d| {
            if d.state.hovered == chain {
                return (Vec::new(), Vec::new());
            }
            let old = std::mem::replace(&mut d.state.hovered, chain.clone());
            for &id in old.iter().filter(|i| !chain.contains(i)) {
                if d.get(id).is_some_and(|n| n.style.hover.is_some()) {
                    d.mark_style_of(id);
                }
            }
            for &id in chain.iter().filter(|i| !old.contains(i)) {
                if d.get(id).is_some_and(|n| n.style.hover.is_some()) {
                    d.mark_style_of(id);
                }
            }
            let collect = |ids: &mut dyn Iterator<Item = NodeId>| -> Vec<Rc<dyn Fn(bool)>> {
                ids.filter_map(|id| d.get(id).and_then(|n| n.listeners.hover.clone()))
                    .collect()
            };
            let entered = collect(&mut chain.iter().copied().filter(|i| !old.contains(i)));
            let left = collect(&mut old.iter().copied().filter(|i| !chain.contains(i)));
            (entered, left)
        });
        if entered.is_empty() && left.is_empty() {
            return;
        }
        batch(|| {
            for h in left {
                h(false);
            }
            for h in entered {
                h(true);
            }
        });
    }

    fn dispatch_pointer(
        &self,
        kind: PointerKind,
        target: NodeId,
        position: PointF,
        button: Option<PointerButton>,
        clicks: u32,
    ) {
        let chain = with_doc(|d| d.ancestors(target));
        for id in chain {
            let found: Option<(Handler<PointerEvent>, RectI)> = with_doc(|d| {
                let n = d.get(id)?;
                if n.disabled {
                    return None;
                }
                let l = &n.listeners;
                let h = match kind {
                    PointerKind::Down => l.pointer_down.clone(),
                    PointerKind::Up => l.pointer_up.clone(),
                    PointerKind::Move => l.pointer_move.clone(),
                    PointerKind::Click => l.click.clone(),
                }?;
                Some((h, d.window_rect(id).unwrap_or_default()))
            });
            let Some((handler, rect)) = found else {
                continue;
            };
            let ev = pointer_event(position, rect, button, self.modifiers, clicks);
            batch(|| handler(&ev));
            if ev.stopped.get() {
                break;
            }
        }
    }

    fn pointer_moved(&mut self, p: PointF) {
        self.pointer = Some(p);
        // Hit testing needs a fresh layout; during a drag the captured target is known and the
        // previous frame's geometry is good enough, so moves don't force a re-layout each.
        if self.press.is_none() {
            self.ensure_layout();
            self.update_hover(p);
        }
        let drag = self.press.as_ref().map(|pr| pr.drag).unwrap_or(Drag::None);
        match drag {
            Drag::Select => with_doc(|d| {
                if let Some(sel) = d.state.selection
                    && let Some(next) = selection::extend(d, &sel, p)
                    && next != sel
                {
                    d.state.selection = Some(next);
                    d.mark_paint();
                }
            }),
            Drag::Edit(id) => with_doc(|d| {
                let at = edit_point(d, id, p);
                if let Some(e) = d.get_mut(id).and_then(Node::edit_mut) {
                    e.pointer_drag(at.x, at.y);
                    d.mark_paint();
                }
            }),
            Drag::Scrollbar(id, vertical, grab) => {
                with_doc(|d| scrollbar_drag(d, id, vertical, grab, p))
            }
            Drag::None => {}
        }
        // While a button is held, moves go to the pressed node (implicit pointer capture).
        let target = match &self.press {
            Some(press) => press.target,
            None => self.hit_test(p),
        };
        if let Some(target) = target {
            self.dispatch_pointer(PointerKind::Move, target, p, None, 0);
        }
    }

    fn pointer_down(&mut self, p: PointF, button: PointerButton, time: u32) {
        self.ensure_layout();
        let target = self.hit_test(p);
        let slop = with_doc(|d| d.scale.pxf(4.0));
        let clicks = match self.last_click {
            Some((t, pos, b, n))
                if b == button
                    && time.wrapping_sub(t) < DOUBLE_CLICK_MS
                    && pos.distance(p) <= slop =>
            {
                n + 1
            }
            _ => 1,
        };
        self.last_click = Some((time, p, button, clicks));

        // Pressing outside of the open menus closes them, and does nothing else if it was in the
        // window. The platform leaves this to us: during a grab our surfaces get input as usual.
        let popup = with_doc(|d| target.and_then(|t| d.popup_of(t)));
        let outside = with_doc(|d| open_menus_outside(d, popup));
        if !outside.is_empty() {
            self.dismiss_popups(&outside);
            if popup.is_none() {
                return;
            }
        }

        // Focus follows the nearest focusable ancestor; clicking elsewhere blurs. What opened a
        // popup keeps the focus when pressing in it, unless there's something else to focus.
        let focus_target = target.and_then(|t| {
            with_doc(|d| {
                d.ancestors(t)
                    .into_iter()
                    .take_while(|id| Some(*id) != popup.and_then(|p| d.get(p)?.parent))
                    .find(|id| d.get(*id).is_some_and(|n| n.focusable && !n.disabled))
            })
        });
        if button != PointerButton::Secondary && (focus_target.is_some() || popup.is_none()) {
            self.set_focus(focus_target);
        }

        let mut drag = Drag::None;
        if button == PointerButton::Primary {
            if let Some(t) = target {
                let chain = with_doc(|d| d.ancestors(t));
                set_active(chain);
            }
            if let Some((id, vertical, grab)) =
                with_doc(|d| target.and_then(|t| scrollbar_hit(d, t, p)))
            {
                drag = Drag::Scrollbar(id, vertical, grab);
            } else if let Some(edit) =
                target.filter(|t| with_doc(|d| d.get(*t).is_some_and(Node::is_edit)))
            {
                let extend = self.modifiers.shift;
                with_doc(|d| {
                    let at = edit_point(d, edit, p);
                    if let Some(e) = d.get_mut(edit).and_then(Node::edit_mut) {
                        e.pointer_down(at.x, at.y, clicks, extend);
                    }
                    if d.state.selection.take().is_some() {
                        d.mark_paint();
                    }
                    d.mark_paint();
                });
                drag = Drag::Edit(edit);
            } else if target.is_some_and(|t| {
                with_doc(|d| {
                    d.get(t)
                        .is_some_and(|n| n.computed.user_select == UserSelect::Text)
                })
            }) {
                let granularity = match clicks {
                    1 => Granularity::Char,
                    2 => Granularity::Word,
                    _ => Granularity::Line,
                };
                let extend = self.modifiers.shift;
                with_doc(|d| {
                    let next = match (&d.state.selection, extend) {
                        (Some(sel), true) => selection::extend_from_anchor(d, sel, p),
                        _ => selection::begin(d, p, granularity),
                    };
                    d.state.selection = next;
                    d.mark_paint();
                });
                drag = Drag::Select;
            }
        } else if button == PointerButton::Middle
            && let Some(edit) =
                target.filter(|t| with_doc(|d| d.get(*t).is_some_and(Node::is_edit)))
        {
            with_doc(|d| {
                let at = edit_point(d, edit, p);
                if let Some(e) = d.get_mut(edit).and_then(Node::edit_mut) {
                    e.pointer_down(at.x, at.y, 1, false);
                    e.drag = None;
                }
                d.request(PlatformRequest::Paste(ClipboardKind::Primary));
                d.mark_paint();
            });
            self.paste_target = Some(edit);
        }

        self.press = Some(Press {
            target,
            button,
            drag,
        });
        if let Some(t) = target {
            self.dispatch_pointer(PointerKind::Down, t, p, Some(button), clicks);
        }
    }

    fn pointer_up(&mut self, p: PointF, button: PointerButton) {
        let Some(press) = self.press.take() else {
            return;
        };
        let target = self.hit_test(p);
        // Like moves, the release goes to the pressed node (implicit pointer capture), so a
        // drag that ends outside of it still finishes.
        if let Some(t) = press.target.or(target) {
            self.dispatch_pointer(PointerKind::Up, t, p, Some(button), 0);
        }
        // Click goes to the deepest common ancestor of the press and release targets.
        if press.button == button
            && let (Some(a), Some(b)) = (press.target, target)
        {
            let common = with_doc(|d| {
                let chain = d.ancestors(a);
                d.ancestors(b).into_iter().find(|n| chain.contains(n))
            });
            if let Some(c) = common {
                let clicks = self.last_click.map(|(_, _, _, n)| n).unwrap_or(1);
                self.dispatch_pointer(PointerKind::Click, c, p, Some(button), clicks);
            }
        }
        match press.drag {
            Drag::Select => with_doc(|d| {
                if let Some(sel) = d.state.selection
                    && !sel.is_collapsed()
                {
                    let text = selection::selected_text(d, &sel);
                    if !text.is_empty() {
                        d.request(PlatformRequest::SetClipboard {
                            kind: ClipboardKind::Primary,
                            text,
                        });
                    }
                }
            }),
            Drag::Edit(id) => with_doc(|d| {
                if let Some(e) = d.get_mut(id).and_then(Node::edit_mut) {
                    e.drag = None;
                    if e.has_selection() {
                        let text = e.selected_text().to_owned();
                        d.request(PlatformRequest::SetClipboard {
                            kind: ClipboardKind::Primary,
                            text,
                        });
                    }
                }
            }),
            _ => {}
        }
        set_active(Vec::new());
        self.update_hover(p);
    }

    /// Ends a press that will never see its release (e.g. the window lost focus), so handlers
    /// waiting for `pointer_up` do not stay stuck.
    fn cancel_press(&mut self) {
        let Some(press) = self.press.take() else {
            return;
        };
        if let Some(t) = press.target {
            let p = self.pointer.unwrap_or_default();
            self.dispatch_pointer(PointerKind::Up, t, p, Some(press.button), 0);
        }
        with_doc(|d| {
            if let Drag::Edit(id) = press.drag
                && let Some(e) = d.get_mut(id).and_then(Node::edit_mut)
            {
                e.drag = None;
            }
        });
        set_active(Vec::new());
    }

    fn scroll(
        &mut self,
        p: PointF,
        dx: f32,
        dy: f32,
        source: ScrollSource,
        phase: ScrollPhase,
        time: u32,
    ) {
        let factor = with_settings(|s| match source {
            ScrollSource::Wheel => s.scroll.wheel_step * with_doc(|d| d.scale).as_f64() as f32,
            ScrollSource::Touchpad => s.scroll.touchpad_factor,
            ScrollSource::Unknown => 1.0,
        });
        let (dx, dy) = (dx * factor, dy * factor);
        match phase {
            ScrollPhase::Single => {
                self.end_scroll_gesture();
                self.begin_scroll_gesture(p, source);
                self.deliver_scroll(Some(p), dx, dy, phase);
                self.gesture = None;
            }
            ScrollPhase::Began | ScrollPhase::Moved => {
                if phase == ScrollPhase::Began || self.gesture.as_ref().is_none_or(|g| g.lifted) {
                    self.interrupt_scroll_gesture(time);
                    self.begin_scroll_gesture(p, source);
                }
                if let Some(g) = &mut self.gesture {
                    g.velocity.record(time, dx, dy);
                }
                let scale = with_doc(|d| d.scale).as_f64() as f32;
                if self
                    .booster
                    .as_mut()
                    .is_some_and(|b| !b.motion(time, dx / scale, dy / scale))
                {
                    self.booster = None;
                }
                self.deliver_scroll(Some(p), dx, dy, phase);
            }
            ScrollPhase::Lifted { fling } => self.scroll_lifted(p, fling, time),
            ScrollPhase::Held => self.interrupt_scroll_gesture(time),
            ScrollPhase::MomentumEnded => self.end_scroll_gesture(),
            ScrollPhase::Momentum => {
                if self.gesture.as_ref().is_some_and(|g| g.lifted) {
                    self.deliver_scroll(Some(p), dx, dy, phase);
                }
            }
        }
    }

    fn begin_scroll_gesture(&mut self, p: PointF, source: ScrollSource) {
        self.ensure_layout();
        let target = self.hit_test(p).filter(|&t| {
            // Open menus take the input.
            with_doc(|d| {
                let popup = d.popup_of(t);
                popup.is_some() || open_menus_outside(d, popup).is_empty()
            })
        });
        self.gesture = target.map(|target| ScrollGesture {
            position: p,
            // Scrolling doesn't go from a popup to what is behind it.
            chain: with_doc(|d| {
                let mut chain = d.ancestors(target);
                if let Some(i) = chain.iter().position(|&id| d.nodes[id].is_popup()) {
                    chain.truncate(i + 1);
                }
                chain
            }),
            scroller: None,
            source,
            shift: self.modifiers.shift,
            velocity: VelocityTracker::default(),
            lifted: false,
        });
    }

    fn scroll_lifted(&mut self, p: PointF, fling: bool, time: u32) {
        let Some(g) = &mut self.gesture else {
            return;
        };
        g.lifted = true;
        let velocity = g.velocity.velocity(time);
        let since_last = g.velocity.since_last(time);
        self.deliver_scroll(Some(p), 0.0, 0.0, ScrollPhase::Lifted { fling });
        let booster = self.booster.take();
        let (scale, size) = with_doc(|d| (d.scale.as_f64() as f32, d.viewport));
        let kinetic = with_settings(|s| s.scroll.kinetic.clone());
        self.fling = (fling && kinetic.enabled && self.press.is_none())
            .then_some(velocity)
            .flatten()
            .and_then(|v| {
                let v = (
                    v.0 / scale * FLING_SPEED_RATIO,
                    v.1 / scale * FLING_SPEED_RATIO,
                );
                let (v, boost) = booster
                    .as_ref()
                    .and_then(|b| b.boost(time, v))
                    .unwrap_or((v, 1.0));
                // It takes over from the last motion, not from when that was noticed to be it.
                let now = Instant::now();
                let start = now
                    .checked_sub(Duration::from_millis(since_last as u64))
                    .unwrap_or(now);
                let window = (size.width as f32 / scale, size.height as f32 / scale);
                Fling::new(v, boost, &kinetic, window, scale, start)
            });
        // Without inertia the gesture is over, and so is what the platform may still send.
        if !kinetic.enabled || (fling && self.fling.is_none()) {
            self.gesture = None;
        }
    }

    /// Ends the current gesture because fingers came back, keeping what they need to add to its
    /// inertia. Every new gesture passes through here, so nothing older is left to add to.
    fn interrupt_scroll_gesture(&mut self, time: u32) {
        let booster = self.fling.as_ref().map(|f| Booster::new(f, time));
        self.end_scroll_gesture();
        self.booster = booster;
    }

    /// Ends the current gesture and its inertia. Inertia from the platform is ignored from here
    /// on, up to the next gesture.
    fn end_scroll_gesture(&mut self) {
        self.fling = None;
        if self.gesture.as_ref().is_some_and(|g| g.lifted) {
            self.deliver_scroll(None, 0.0, 0.0, ScrollPhase::MomentumEnded);
        }
        self.gesture = None;
    }

    /// Passes motion of the current gesture to the handlers, then to the node it scrolls.
    fn deliver_scroll(
        &mut self,
        p: Option<PointF>,
        dx: f32,
        dy: f32,
        phase: ScrollPhase,
    ) -> Scrolled {
        self.ensure_layout();
        let Some(g) = &self.gesture else {
            return Scrolled::Stuck;
        };
        let source = g.source;
        for &id in &g.chain {
            let handler = with_doc(|d| {
                d.get(id)
                    .filter(|n| !n.disabled)
                    .and_then(|n| n.listeners.scroll.clone())
            });
            if let Some(h) = handler {
                let ev = ScrollEvent {
                    delta_x: dx,
                    delta_y: dy,
                    modifiers: self.modifiers,
                    source,
                    phase,
                    stopped: Cell::new(false),
                };
                batch(|| h(&ev));
                if ev.stopped.get() {
                    return Scrolled::Handled;
                }
            }
        }
        if dx == 0.0 && dy == 0.0 {
            return Scrolled::Stuck;
        }
        let (dx, dy) = if g.shift && dx == 0.0 {
            (dy, 0.0)
        } else {
            (dx, dy)
        };
        let Some(id) = g.scroller.or_else(|| {
            with_doc(|d| {
                g.chain
                    .iter()
                    .copied()
                    .find(|&id| can_scroll(d, id, dx, dy))
            })
        }) else {
            return Scrolled::Stuck;
        };
        if let Some(g) = &mut self.gesture {
            g.scroller = Some(id);
        }
        let mut rest = match self.scroll_rest {
            Some((node, x, y)) if node == id => (x, y),
            _ => (0.0, 0.0),
        };
        let (scrolled, moving) = with_doc(|d| {
            let scrolled = scroll_node(d, id, dx, dy, &mut rest);
            (scrolled, can_scroll(d, id, dx, dy))
        });
        self.scroll_rest = Some((id, rest.0, rest.1));
        if scrolled {
            self.ensure_layout();
            // Keep extending an in-progress selection while the content scrolls under it.
            match p {
                Some(p)
                    if matches!(
                        self.press.as_ref().map(|p| p.drag),
                        Some(Drag::Select | Drag::Edit(_))
                    ) =>
                {
                    self.pointer_moved(p)
                }
                Some(p) => self.update_hover(p),
                None => {}
            }
        }
        if moving {
            Scrolled::Moving
        } else {
            Scrolled::Stuck
        }
    }

    fn set_focus(&mut self, next: Option<NodeId>) {
        let keyboard = self.focus_by_keyboard;
        let (prev, handlers) = with_doc(|d| {
            let prev = d.state.focused;
            if prev == next {
                return (prev, Vec::new());
            }
            d.state.focused = next;
            let mut handlers = Vec::new();
            for (id, focused) in [(prev, false), (next, true)] {
                let Some(n) = id.and_then(|id| d.get(id)) else {
                    continue;
                };
                if let Some(h) = n.listeners.focus.clone() {
                    handlers.push((h, focused));
                }
                if let Some(h) = n.listeners.focus_visible.clone()
                    && (!focused || keyboard)
                {
                    handlers.push((h, focused));
                }
            }
            if let Some(e) = next.and_then(|id| d.get_mut(id)).and_then(Node::edit_mut) {
                e.caret_epoch = Instant::now();
                e.scroll_to_caret = true;
            }
            if let Some(e) = prev.and_then(|id| d.get_mut(id)).and_then(Node::edit_mut) {
                e.preedit = None;
            }
            d.dirty.carets = true;
            for id in [prev, next].into_iter().flatten() {
                d.mark_style_of(id);
            }
            (prev, handlers)
        });
        if prev == next {
            return;
        }
        batch(|| {
            for (h, f) in handlers {
                h(f);
            }
        });
    }

    fn apply_pending_focus(&mut self) {
        if let Some(target) = with_doc(|d| d.pending_focus.take()) {
            self.set_focus(target);
        }
    }

    fn focus_next(&mut self, backwards: bool) {
        let next = with_doc(|d| {
            let order: Vec<NodeId> = d
                .preorder()
                .into_iter()
                .filter(|id| {
                    d.get(*id)
                        .is_some_and(|n| n.focusable && n.visible && !n.disabled)
                })
                .collect();
            if order.is_empty() {
                return None;
            }
            let pos = d
                .state
                .focused
                .and_then(|f| order.iter().position(|id| *id == f));
            let i = match (pos, backwards) {
                (None, false) => 0,
                (None, true) => order.len() - 1,
                (Some(i), false) => (i + 1) % order.len(),
                (Some(i), true) => (i + order.len() - 1) % order.len(),
            };
            Some(order[i])
        });
        if let Some(n) = next {
            self.focus_by_keyboard = true;
            self.set_focus(Some(n));
            self.focus_by_keyboard = false;
            with_doc(|d| {
                if let Some(e) = d.get_mut(n).and_then(Node::edit_mut) {
                    e.select_all();
                }
            });
        }
    }

    fn key(&mut self, key: KeyEvent) {
        self.ensure_layout();
        let focused = with_doc(|d| d.state.focused);
        let target = focused.or_else(|| with_doc(|d| d.root));
        let ev = KeyboardEvent {
            key: key.key.clone(),
            text: key.text.clone(),
            modifiers: key.modifiers,
            repeat: key.repeat,
            stopped: Cell::new(false),
            default_prevented: Cell::new(false),
        };
        if let Some(t) = target {
            let chain = with_doc(|d| d.ancestors(t));
            for id in chain {
                let h = with_doc(|d| {
                    d.get(id).filter(|n| !n.disabled).and_then(|n| {
                        if key.pressed {
                            n.listeners.key_down.clone()
                        } else {
                            n.listeners.key_up.clone()
                        }
                    })
                });
                if let Some(h) = h {
                    batch(|| h(&ev));
                    if ev.stopped.get() {
                        break;
                    }
                }
            }
        }
        if !key.pressed || ev.default_prevented.get() {
            return;
        }
        self.ensure_layout();

        let focused_edit = focused.filter(|id| with_doc(|d| d.get(*id).is_some_and(Node::is_edit)));
        if let Some(id) = focused_edit
            && self.edit_key(id, &key)
        {
            return;
        }

        let m = key.modifiers;
        match &key.key {
            Key::Named(NamedKey::Tab) if !m.ctrl && !m.alt => self.focus_next(m.shift),
            Key::Named(NamedKey::Escape) => {
                let top = with_doc(|d| open_menus_outside(d, None).pop());
                if let Some(top) = top {
                    self.dismiss_popups(&[top]);
                    return;
                }
                with_doc(|d| {
                    if d.state.selection.take().is_some() {
                        d.mark_paint();
                    }
                })
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) if focused.is_some() && !key.repeat => {
                let t = focused.unwrap();
                let rect = with_doc(|d| d.window_rect(t).unwrap_or_default());
                let center = PointF::new(
                    rect.x as f32 + rect.width as f32 / 2.0,
                    rect.y as f32 + rect.height as f32 / 2.0,
                );
                self.dispatch_pointer(PointerKind::Click, t, center, None, 1);
            }
            k if m.primary() && k.is_char('c') => self.copy_document_selection(),
            k if m.primary() && k.is_char('a') => with_doc(|d| {
                d.state.selection = selection::select_all(d);
                d.mark_paint();
            }),
            _ => {}
        }
    }

    fn copy_document_selection(&mut self) {
        with_doc(|d| {
            if let Some(sel) = d.state.selection
                && !sel.is_collapsed()
            {
                let text = selection::selected_text(d, &sel);
                d.request(PlatformRequest::SetClipboard {
                    kind: ClipboardKind::Clipboard,
                    text,
                });
            }
        });
    }

    /// Built-in editing keys. Returns whether the key was consumed.
    fn edit_key(&mut self, id: NodeId, key: &KeyEvent) -> bool {
        let m = key.modifiers;
        let (multiline, page_lines) = with_doc(|d| {
            let n = &d.nodes[id];
            let e = n.edit().unwrap();
            let lines = (n.content_abs().height / e.line_height().max(1)).max(1) as isize;
            (e.multiline, lines)
        });
        let motion = |m: Motion, extend: bool| {
            move |e: &mut crate::text::edit::EditData| e.move_caret(m, extend, page_lines)
        };
        let shift = m.shift;
        let word = m.ctrl;
        type EditFn = Box<dyn FnOnce(&mut crate::text::edit::EditData) -> EditOutcome>;
        let op: Option<EditFn> = match &key.key {
            Key::Named(NamedKey::ArrowLeft) => Some(Box::new(motion(
                if word { Motion::WordLeft } else { Motion::Left },
                shift,
            ))),
            Key::Named(NamedKey::ArrowRight) => Some(Box::new(motion(
                if word {
                    Motion::WordRight
                } else {
                    Motion::Right
                },
                shift,
            ))),
            Key::Named(NamedKey::ArrowUp) => Some(Box::new(motion(
                if multiline {
                    Motion::Up
                } else {
                    Motion::LineStart
                },
                shift,
            ))),
            Key::Named(NamedKey::ArrowDown) => Some(Box::new(motion(
                if multiline {
                    Motion::Down
                } else {
                    Motion::LineEnd
                },
                shift,
            ))),
            Key::Named(NamedKey::Home) => Some(Box::new(motion(
                if word {
                    Motion::DocStart
                } else {
                    Motion::LineStart
                },
                shift,
            ))),
            Key::Named(NamedKey::End) => Some(Box::new(motion(
                if word {
                    Motion::DocEnd
                } else {
                    Motion::LineEnd
                },
                shift,
            ))),
            Key::Named(NamedKey::PageUp) if multiline => {
                Some(Box::new(motion(Motion::PageUp, shift)))
            }
            Key::Named(NamedKey::PageDown) if multiline => {
                Some(Box::new(motion(Motion::PageDown, shift)))
            }
            Key::Named(NamedKey::Backspace) => Some(Box::new(move |e| e.delete(false, word))),
            Key::Named(NamedKey::Delete) if shift && !word => {
                self.edit_clipboard(id, true);
                return true;
            }
            Key::Named(NamedKey::Delete) => Some(Box::new(move |e| e.delete(true, word))),
            Key::Named(NamedKey::Insert) if shift => {
                self.request_paste(id);
                return true;
            }
            Key::Named(NamedKey::Insert) if word => {
                self.edit_clipboard(id, false);
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                if multiline && !m.ctrl {
                    Some(Box::new(|e| e.insert("\n")))
                } else {
                    let submit = with_doc(|d| {
                        let e = d.get(id)?.edit()?;
                        Some((e.on_submit.clone()?, e.text.clone()))
                    });
                    if let Some((h, text)) = submit {
                        batch(|| h(&text));
                    }
                    return true;
                }
            }
            k if m.primary() && k.is_char('a') => Some(Box::new(|e| {
                e.select_all();
                EditOutcome {
                    changed: false,
                    moved: true,
                }
            })),
            k if m.primary() && k.is_char('c') => {
                self.edit_clipboard(id, false);
                return true;
            }
            k if m.primary() && k.is_char('x') => {
                self.edit_clipboard(id, true);
                return true;
            }
            k if m.primary() && k.is_char('v') => {
                self.request_paste(id);
                return true;
            }
            k if m.primary() && k.is_char('z') => {
                if shift {
                    Some(Box::new(|e| e.redo()))
                } else {
                    Some(Box::new(|e| e.undo()))
                }
            }
            k if m.primary() && k.is_char('y') => Some(Box::new(|e| e.redo())),
            _ => match &key.text {
                Some(t) if !m.ctrl && !m.alt && !t.is_empty() => {
                    let t = t.clone();
                    Some(Box::new(move |e| e.insert(&t)))
                }
                _ => None,
            },
        };
        let Some(op) = op else { return false };
        let outcome = with_doc(|d| {
            let out = d
                .get_mut(id)
                .and_then(Node::edit_mut)
                .map(op)
                .unwrap_or_default();
            if out.changed {
                d.invalidate_layout(id);
            } else if out.moved {
                d.mark_caret_moved();
            }
            out
        });
        self.after_edit(id, outcome);
        true
    }

    fn edit_clipboard(&mut self, id: NodeId, cut: bool) {
        let outcome = with_doc(|d| {
            let e = d.get_mut(id)?.edit_mut()?;
            if !e.has_selection() {
                return None;
            }
            let text = e.selected_text().to_owned();
            let out = if cut {
                e.delete(false, false)
            } else {
                EditOutcome::default()
            };
            d.request(PlatformRequest::SetClipboard {
                kind: ClipboardKind::Clipboard,
                text,
            });
            if out.changed {
                d.invalidate_layout(id);
            }
            Some(out)
        });
        if let Some(o) = outcome {
            self.after_edit(id, o);
        }
    }

    fn request_paste(&mut self, id: NodeId) {
        self.paste_target = Some(id);
        with_doc(|d| d.request(PlatformRequest::Paste(ClipboardKind::Clipboard)));
    }

    fn paste(&mut self, text: String) {
        let Some(id) = self
            .paste_target
            .take()
            .or_else(|| with_doc(|d| d.state.focused))
        else {
            return;
        };
        let outcome = with_doc(|d| {
            let out = d
                .get_mut(id)
                .and_then(Node::edit_mut)
                .map(|e| e.insert(&text));
            if out.as_ref().is_some_and(|o| o.changed) {
                d.invalidate_layout(id);
            }
            out
        });
        if let Some(o) = outcome {
            self.after_edit(id, o);
        }
    }

    fn ime(&mut self, update: ImeUpdate) {
        let Some(id) = with_doc(|d| d.state.focused) else {
            return;
        };
        let outcome = with_doc(|d| {
            let out = d
                .get_mut(id)
                .and_then(Node::edit_mut)
                .map(|e| e.apply_ime(&update));
            if out.is_some() {
                d.invalidate_layout(id);
            }
            out
        });
        if let Some(o) = outcome {
            self.after_edit(id, o);
        }
    }

    /// Propagates an edit to the bound signal and the input handler.
    fn after_edit(&mut self, id: NodeId, outcome: EditOutcome) {
        if !outcome.changed {
            return;
        }
        let Some((value, handler, text)) = with_doc(|d| {
            let e = d.get(id)?.edit()?;
            Some((e.value, e.on_input.clone(), e.text.clone()))
        }) else {
            return;
        };
        batch(|| {
            if let Some(v) = value {
                v.set_if_changed(text.clone());
            }
            if let Some(h) = handler {
                h(&text);
            }
        });
    }

    /// Sends the IME state if it changed. The caret rectangle needs an up-to-date layout.
    fn sync_ime(&mut self) {
        let state = with_doc(|d| {
            if d.dirty.layout || d.dirty.position {
                return None;
            }
            let id = d.state.focused?;
            let n = d.get(id)?;
            let e = n.edit()?;
            if e.read_only {
                return Some(None);
            }
            let origin = edit_text_origin(d, id);
            let caret = e.caret_rect(caret_width(d)).translate(origin.x, origin.y);
            Some(Some(e.ime_state(d.rect_to_window(n, caret))))
        });
        let state = match state {
            Some(s) => s,
            None => {
                let focused_edit = with_doc(|d| {
                    d.state
                        .focused
                        .and_then(|id| d.get(id))
                        .is_some_and(Node::is_edit)
                });
                if focused_edit {
                    // Wait for the layout to settle before reporting.
                    return;
                }
                None
            }
        };
        if self.ime_sent.as_ref() != Some(&state) {
            self.ime_sent = Some(state.clone());
            with_doc(|d| d.request(PlatformRequest::SetIme(state)));
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        self.owner.dispose();
    }
}

/// Listeners to tell, and what to tell them.
type ActiveChanges = Vec<(Rc<dyn Fn(bool)>, bool)>;

/// Sets the pressed nodes, and tells those that listen.
fn set_active(chain: Vec<NodeId>) {
    let listeners = with_doc(|d| d.set_active(chain));
    if !listeners.is_empty() {
        batch(|| {
            for (f, active) in listeners {
                f(active);
            }
        });
    }
}

impl Document {
    /// Sets the pressed nodes, invalidating those whose style depends on it. Returns who to tell.
    fn set_active(&mut self, chain: Vec<NodeId>) -> ActiveChanges {
        let old = std::mem::replace(&mut self.state.active, chain);
        let new = self.state.active.clone();
        let mut listeners = Vec::new();
        let changes = old
            .iter()
            .filter(|i| !new.contains(i))
            .map(|i| (*i, false))
            .chain(new.iter().filter(|i| !old.contains(i)).map(|i| (*i, true)));
        for (id, active) in changes {
            let Some(n) = self.get(id) else {
                continue;
            };
            let has_style = n.style.active.is_some();
            if let Some(f) = n.listeners.active.clone().filter(|_| !n.disabled) {
                listeners.push((f, active));
            }
            if has_style {
                self.mark_style_of(id);
            }
        }
        listeners
    }

    /// Caret or selection moved without changing text: repaint and re-scroll only.
    fn mark_caret_moved(&mut self) {
        self.dirty.carets = true;
        self.mark_paint();
    }

    /// `id` scrolled. Nothing moves in a scroll root's content; a scroller inside of one is part
    /// of that content.
    pub fn mark_scrolled(&mut self, id: NodeId) {
        self.dirty.paint = true;
        if !self.get(id).is_some_and(|n| n.scroll_root) {
            self.reposition.push(id);
            self.dirty.content = true;
        }
    }
}

/// A point in window coordinates relative to an edit node's text origin.
fn edit_point(doc: &Document, id: NodeId, p: PointF) -> PointF {
    let origin = edit_text_origin(doc, id);
    let p = doc.get(id).and_then(|n| doc.from_window(n, p)).unwrap_or(p);
    PointF::new(p.x - origin.x as f32, p.y - origin.y as f32)
}

/// `p` is in the same coordinates as the node.
fn hit(doc: &Document, id: NodeId, p: PointF) -> Option<NodeId> {
    let node = doc.get(id)?;
    if !node.visible {
        return None;
    }
    let p = if node.transformed {
        if !node.layer_clip.rect.contains(p) {
            return None;
        }
        node.layer_matrix(doc.scale).inverse()?.apply_point(p)
    } else {
        p
    };
    if !node.clip.contains(p) {
        return None;
    }
    // A scroll root's children are in content coordinates and clipped by the root only here.
    let child_p = if node.scroll_root {
        child_clip(node, doc.scale, node.clip_shape())
            .rect
            .contains(p)
            .then(|| {
                PointF::new(
                    p.x - (node.abs.x - node.scroll.x) as f32,
                    p.y - (node.abs.y - node.scroll.y) as f32,
                )
            })
    } else {
        Some(p)
    };
    if let Some(cp) = child_p {
        let (below, above) = doc.stacked_around(id);
        if let Some(h) = hit_stacked(doc, above, cp) {
            return Some(h);
        }
        for (i, &c) in node.children.iter().enumerate().rev() {
            if node.child_bounds.get(i).is_none_or(|b| b.contains(cp))
                && let Some(h) = doc
                    .get(c)
                    .filter(|c| !c.stacked)
                    .and_then(|_| hit(doc, c, cp))
            {
                return Some(h);
            }
        }
        if let Some(h) = hit_stacked(doc, below, cp) {
            return Some(h);
        }
    }
    if matches!(node.kind, NodeKind::Fragment) || !node.computed.pointer_events {
        return None;
    }
    node.abs.contains(p).then_some(id)
}

fn hit_stacked(doc: &Document, nodes: &[NodeId], p: PointF) -> Option<NodeId> {
    nodes.iter().rev().find_map(|&id| {
        doc.get(id)
            .filter(|n| n.bounds_in_parent(doc.scale).contains(p))
            .and_then(|_| hit(doc, id, p))
    })
}

fn cursor_for(doc: &Document, target: Option<NodeId>) -> CursorIcon {
    let Some(n) = target.and_then(|t| doc.get(t)) else {
        return CursorIcon::Default;
    };
    if let Some(c) = n.computed.cursor {
        return c;
    }
    if n.is_edit() || (n.is_text() && n.computed.user_select == UserSelect::Text) {
        return CursorIcon::Text;
    }
    CursorIcon::Default
}

/// Whether motion in this direction would move `id`, given enough of it.
fn can_scroll(doc: &Document, id: NodeId, dx: f32, dy: f32) -> bool {
    let Some(n) = doc.get(id) else {
        return false;
    };
    let room = |scroll: i32, max: i32, delta: f32| {
        (delta > 0.0 && scroll < max) || (delta < 0.0 && scroll > 0)
    };
    let pad = n.padding_abs();
    (n.computed.overflow_y == Overflow::Scroll
        && room(n.scroll.y, n.content_size.height - pad.height, dy))
        || (n.computed.overflow_x == Overflow::Scroll
            && room(n.scroll.x, n.content_size.width - pad.width, dx))
        || n.edit().is_some_and(|e| {
            e.multiline && room(e.scroll.y, e.content_size.height - pad.height, dy)
        })
}

/// Moves `scroll` by whole pixels of `delta` and `rest`, leaving the fraction in `rest`. There is
/// no fraction to keep at the ends.
fn advance(scroll: &mut i32, delta: f32, max: i32, rest: &mut f32) -> bool {
    let total = *rest + delta;
    let step = total.round();
    let wanted = *scroll + step as i32;
    let next = wanted.clamp(0, max.max(0));
    *rest = if next == wanted { total - step } else { 0.0 };
    let changed = next != *scroll;
    *scroll = next;
    changed
}

fn scroll_node(doc: &mut Document, id: NodeId, dx: f32, dy: f32, rest: &mut (f32, f32)) -> bool {
    let Some(n) = doc.get_mut(id) else {
        return false;
    };
    let pad = n.padding_abs();
    let mut changed = false;
    if n.computed.overflow_y == Overflow::Scroll && dy != 0.0 {
        let max = n.content_size.height - pad.height;
        changed |= advance(&mut n.scroll.y, dy, max, &mut rest.1);
    }
    if n.computed.overflow_x == Overflow::Scroll && dx != 0.0 {
        let max = n.content_size.width - pad.width;
        changed |= advance(&mut n.scroll.x, dx, max, &mut rest.0);
    }
    if changed {
        doc.mark_scrolled(id);
        return true;
    }
    if let Some(e) = n.edit_mut()
        && e.multiline
        && dy != 0.0
    {
        // Edits scroll their own text.
        let max = e.content_size.height - pad.height;
        if advance(&mut e.scroll.y, dy, max, &mut rest.1) {
            doc.mark_paint();
            return true;
        }
    }
    false
}

/// Hit-tests the scrollbar thumbs of `target`'s scroll containers.
fn scrollbar_hit(doc: &Document, target: NodeId, p: PointF) -> Option<(NodeId, bool, f32)> {
    let grab_width = doc.scale.pxf(12.0);
    for id in doc.ancestors(target) {
        let n = doc.get(id)?;
        if n.is_popup() {
            return None;
        }
        let Some(p) = doc.from_window(n, p) else {
            continue;
        };
        let pad = n.padding_abs();
        if n.computed.overflow_y == Overflow::Scroll
            && n.content_size.height > pad.height
            && p.x >= pad.right() as f32 - grab_width
            && pad.contains(p)
        {
            let ratio = n.content_size.height as f32 / pad.height.max(1) as f32;
            let grab = (p.y - pad.y as f32) * ratio - n.scroll.y as f32;
            return Some((id, true, grab));
        }
        if n.computed.overflow_x == Overflow::Scroll
            && n.content_size.width > pad.width
            && p.y >= pad.bottom() as f32 - grab_width
            && pad.contains(p)
        {
            let ratio = n.content_size.width as f32 / pad.width.max(1) as f32;
            let grab = (p.x - pad.x as f32) * ratio - n.scroll.x as f32;
            return Some((id, false, grab));
        }
    }
    None
}

fn scrollbar_drag(doc: &mut Document, id: NodeId, vertical: bool, grab: f32, p: PointF) {
    let Some(p) = doc.get(id).and_then(|n| doc.from_window(n, p)) else {
        return;
    };
    let Some(n) = doc.get_mut(id) else { return };
    let pad = n.padding_abs();
    if vertical {
        let ratio = n.content_size.height as f32 / pad.height.max(1) as f32;
        let max = (n.content_size.height - pad.height).max(0);
        n.scroll.y = (((p.y - pad.y as f32) * ratio - grab).round() as i32).clamp(0, max);
    } else {
        let ratio = n.content_size.width as f32 / pad.width.max(1) as f32;
        let max = (n.content_size.width - pad.width).max(0);
        n.scroll.x = (((p.x - pad.x as f32) * ratio - grab).round() as i32).clamp(0, max);
    }
    doc.mark_scrolled(id);
}

/// Scrolls the ancestors of `id` until it is in view, as far as they can.
fn scroll_into_view(doc: &mut Document, id: NodeId) {
    let Some(mut rect) = doc.window_rect(id) else {
        return;
    };
    let ancestors = doc.ancestors(id);
    for &a in &ancestors[1..] {
        let Some(n) = doc.nodes.get(a) else {
            return;
        };
        if n.is_popup() {
            return;
        }
        let pad = doc.rect_to_window(n, n.padding_abs());
        let (c, content) = (&n.computed, n.content_size);
        let wanted = |lo: i32, hi: i32, view_lo: i32, view_hi: i32| {
            if lo < view_lo {
                lo - view_lo
            } else if hi > view_hi {
                (hi - view_hi).min(lo - view_lo)
            } else {
                0
            }
        };
        let mut scroll = n.scroll;
        if c.overflow_y == Overflow::Scroll {
            let d = wanted(rect.y, rect.bottom(), pad.y, pad.bottom());
            scroll.y = (scroll.y + d).clamp(0, (content.height - pad.height).max(0));
        }
        if c.overflow_x == Overflow::Scroll {
            let d = wanted(rect.x, rect.right(), pad.x, pad.right());
            scroll.x = (scroll.x + d).clamp(0, (content.width - pad.width).max(0));
        }
        if scroll != n.scroll {
            rect = rect.translate(n.scroll.x - scroll.x, n.scroll.y - scroll.y);
            doc.nodes[a].scroll = scroll;
            doc.mark_scrolled(a);
        }
    }
}

/// The menus on the screen or about to be, except `inside` and those it is attached to. What
/// is attached to others comes after them.
fn open_menus_outside(doc: &Document, inside: Option<NodeId>) -> Vec<NodeId> {
    doc.popups
        .iter()
        .copied()
        .filter(|&id| {
            doc.nodes[id]
                .popup()
                .is_some_and(|p| p.kind == PopupKind::Menu && p.sent.is_some() && !p.dismissed)
                && inside.is_none_or(|i| !doc.is_ancestor(id, i))
        })
        .collect()
}

/// What the platform should know about a popup, if it should be there. A popup waits for the
/// one it is attached to.
fn popup_params(doc: &Document, id: NodeId) -> Option<PopupParams> {
    let node = doc.nodes.get(id)?;
    let data = node.popup()?;
    if !node.visible || data.dismissed || node.abs.is_empty() {
        return None;
    }
    // A grab has to go on the topmost grabbing popup, or on the window.
    let mut parent = node.parent.and_then(|p| doc.popup_of(p));
    while let Some(p) = parent.filter(|_| data.kind == PopupKind::Menu) {
        if doc.nodes[p].popup()?.kind == PopupKind::Menu {
            break;
        }
        parent = doc.nodes[p].parent.and_then(|q| doc.popup_of(q));
    }
    if let Some(p) = parent
        && !doc.nodes[p].popup()?.shown()
    {
        return None;
    }
    let scale = doc.scale;
    let window = RectI::new(0, 0, doc.viewport.width, doc.viewport.height);
    let anchor = data.anchor_rect.or_else(|| {
        let a = data
            .anchor
            .and_then(|a| a.id_untracked())
            .or_else(|| doc.anchor_of(id))?;
        let on_popup = doc.popup_of(a).is_some();
        let visible = doc.visible_window_rect(a)?;
        Some(if on_popup {
            visible
        } else {
            visible.intersect(&window)
        })
    })?;
    let anchor = RectI::new(
        anchor.x,
        anchor.y,
        anchor.width.max(1),
        anchor.height.max(1),
    );
    let surface = data.surface;
    let wanted = SizeI::new(
        scale.logical_ceil(data.natural.width).max(1),
        scale.logical_ceil(data.natural.height).max(1),
    );
    let position = (data.kind == PopupKind::Bubble).then(|| {
        let at = place_in_window(
            anchor,
            data.placement,
            scale.px(data.gap as f32),
            node.abs.size(),
            window,
        );
        PointI::new(
            scale.logical_floor(at.x) - surface.geometry.x,
            scale.logical_floor(at.y) - surface.geometry.y,
        )
    });
    Some(PopupParams {
        id: popup_id(id),
        parent: parent.map(popup_id),
        kind: data.kind,
        placement: data.placement,
        anchor,
        gap: data.gap,
        wanted,
        buffer: surface.buffer,
        logical: surface.logical,
        geometry: surface.geometry,
        position,
    })
}

/// Where a box goes next to `anchor`: on the other side if it doesn't fit, then moved along the
/// edges as far as it takes to stay in `window`.
fn place_in_window(
    anchor: RectI,
    placement: Placement,
    gap: i32,
    size: SizeI,
    window: RectI,
) -> PointI {
    let below = anchor.bottom() + gap;
    let above = anchor.y - gap - size.height;
    let right = anchor.right() + gap;
    let left = anchor.x - gap - size.width;
    let start_x = anchor.x;
    let end_x = anchor.right() - size.width;
    let (x, y) = match placement {
        Placement::BelowStart => (start_x, below),
        Placement::BelowEnd => (end_x, below),
        Placement::AboveStart => (start_x, above),
        Placement::AboveEnd => (end_x, above),
        Placement::RightStart => (right, anchor.y),
        Placement::LeftStart => (left, anchor.y),
    };
    let (x, y) = match placement {
        Placement::BelowStart | Placement::BelowEnd
            if y + size.height > window.bottom() && above >= window.y =>
        {
            (x, above)
        }
        Placement::AboveStart | Placement::AboveEnd
            if y < window.y && below + size.height <= window.bottom() =>
        {
            (x, below)
        }
        Placement::RightStart if x + size.width > window.right() && left >= window.x => (left, y),
        Placement::LeftStart if x < window.x && right + size.width <= window.right() => (right, y),
        _ => (x, y),
    };
    PointI::new(
        x.min(window.right() - size.width).max(window.x),
        y.min(window.bottom() - size.height).max(window.y),
    )
}

/// Adjusts the internal scroll of edits so their caret stays visible.
fn scroll_carets_into_view(doc: &mut Document) {
    let ids: Vec<NodeId> = doc
        .edits
        .iter()
        .copied()
        .filter(|id| {
            doc.nodes
                .get(*id)
                .and_then(Node::edit)
                .is_some_and(|e| e.scroll_to_caret)
        })
        .collect();
    let cw = caret_width(doc);
    let mut moved = false;
    for id in ids {
        let n = doc.nodes.get_mut(id).unwrap();
        let view = n.content_abs();
        let e = n.edit_mut().unwrap();
        e.scroll_to_caret = false;
        let caret = e.caret_rect(cw);
        let mut s = e.scroll;
        if caret.x - s.x < 0 {
            s.x = caret.x;
        } else if caret.right() - s.x > view.width {
            s.x = caret.right() - view.width;
        }
        if e.multiline {
            if caret.y - s.y < 0 {
                s.y = caret.y;
            } else if caret.bottom() - s.y > view.height {
                s.y = caret.bottom() - view.height;
            }
        }
        let max_x = (e.content_size.width + cw - view.width).max(0);
        let max_y = (e.content_size.height - view.height).max(0);
        s.x = s.x.clamp(0, max_x);
        s.y = if e.multiline { s.y.clamp(0, max_y) } else { 0 };
        if s != e.scroll {
            e.scroll = s;
            moved = true;
        }
    }
    if moved {
        doc.mark_paint();
    }
}
