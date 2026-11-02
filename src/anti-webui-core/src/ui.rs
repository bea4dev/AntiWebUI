//! The per-window UI state machine: input handling, selection, editing, focus and frames.

use crate::event::*;
use crate::geometry::{PointF, RectI, Scale, SizeI};
use crate::layout::{layout_document, position_document, resolve_styles};
use crate::paint::{CARET_BLINK, caret_width, edit_text_origin, paint_document};
use crate::platform::{CursorIcon, ImeState, PlatformRequest};
use crate::reactive::{Owner, batch, create_root};
use crate::scene::Scene;
use crate::text::edit::{EditOutcome, Motion};
use crate::text::selection::{self, Granularity};
use crate::text::with_text;
use crate::tree::{Document, Handler, Node, NodeId, NodeKind, with_doc};
use crate::view::IntoView;
use anti_webui_style::{AlignItems, Display, FlexDirection, Overflow, Style, UserSelect};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const DOUBLE_CLICK_MS: u32 = 500;

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

pub struct Ui {
    owner: Owner,
    pointer: Option<PointF>,
    modifiers: Modifiers,
    press: Option<Press>,
    last_click: Option<(u32, PointF, PointerButton, u32)>,
    cursor: Option<CursorIcon>,
    ime_sent: Option<Option<ImeState>>,
    /// Where a pending paste should go.
    paste_target: Option<NodeId>,
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
            cursor: None,
            ime_sent: None,
            paste_target: None,
        }
    }

    /// Recomputes styles, layout and positions as needed.
    pub fn update(&mut self) {
        self.apply_pending_focus();
        let changed = with_doc(|doc| {
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
                    scroll_carets_into_view(doc);
                    doc.dirty.paint = true;
                    changed = true;
                }
                changed
            })
        });
        if changed && self.press.is_none() {
            // Content may have moved under a stationary pointer.
            if let Some(p) = self.pointer {
                self.update_hover(p);
            }
        }
        self.sync_ime();
    }

    pub fn needs_redraw(&self) -> bool {
        with_doc(|d| d.dirty.paint || d.dirty.layout || d.dirty.style || d.dirty.position)
    }

    /// Produces the frame. Call [`Ui::update`] first.
    pub fn paint(&mut self, now: Instant) -> Scene {
        with_doc(|d| {
            d.dirty.paint = false;
            paint_document(d, now)
        })
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
                self.pointer_moved(position);
            }
            WindowEvent::PointerLeft => {
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
            } => {
                self.modifiers = modifiers;
                self.scroll(position, delta_x, delta_y);
            }
            WindowEvent::Key(key) => {
                self.modifiers = key.modifiers;
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
        }
        self.sync_ime();
    }

    fn resize(&mut self, size: SizeI, scale: Scale) {
        with_doc(|d| {
            if d.viewport != size || d.scale != scale {
                if d.scale != scale {
                    // Every resolved style and text measurement depends on the scale.
                    d.mark_style();
                    d.invalidate_all_layout();
                }
                d.viewport = size;
                d.scale = scale;
                d.mark_layout();
            }
        });
    }

    fn ensure_layout(&mut self) {
        if with_doc(|d| d.dirty.style || d.dirty.layout || d.dirty.position) {
            self.update();
        }
    }

    fn hit_test(&self, p: PointF) -> Option<NodeId> {
        with_doc(|d| d.root.and_then(|r| hit(d, r, p)))
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
            let changed = |id: &NodeId| d.get(*id).is_some_and(|n| n.style.hover.is_some());
            if old.iter().filter(|i| !chain.contains(i)).any(changed)
                || chain.iter().filter(|i| !old.contains(i)).any(changed)
            {
                d.mark_style();
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
                Some((h, n.abs))
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
                let origin = edit_text_origin(d, id);
                if let Some(e) = d.get_mut(id).and_then(Node::edit_mut) {
                    e.pointer_drag(p.x - origin.x as f32, p.y - origin.y as f32);
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

        // Focus follows the nearest focusable ancestor; clicking elsewhere blurs.
        let focus_target = target.and_then(|t| {
            with_doc(|d| {
                d.ancestors(t)
                    .into_iter()
                    .find(|id| d.get(*id).is_some_and(|n| n.focusable && !n.disabled))
            })
        });
        if button != PointerButton::Secondary {
            self.set_focus(focus_target);
        }

        let mut drag = Drag::None;
        if button == PointerButton::Primary {
            if let Some(t) = target {
                let chain = with_doc(|d| d.ancestors(t));
                let active_changed = with_doc(|d| {
                    let any = chain
                        .iter()
                        .any(|id| d.get(*id).is_some_and(|n| n.style.active.is_some()));
                    d.state.active = chain.clone();
                    any
                });
                if active_changed {
                    with_doc(Document::mark_style);
                }
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
                    let origin = edit_text_origin(d, edit);
                    if let Some(e) = d.get_mut(edit).and_then(Node::edit_mut) {
                        e.pointer_down(
                            p.x - origin.x as f32,
                            p.y - origin.y as f32,
                            clicks,
                            extend,
                        );
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
                let origin = edit_text_origin(d, edit);
                if let Some(e) = d.get_mut(edit).and_then(Node::edit_mut) {
                    e.pointer_down(p.x - origin.x as f32, p.y - origin.y as f32, 1, false);
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
        let had_active = with_doc(|d| {
            let any = d
                .state
                .active
                .iter()
                .any(|id| d.get(*id).is_some_and(|n| n.style.active.is_some()));
            d.state.active.clear();
            any
        });
        if had_active {
            with_doc(Document::mark_style);
        }
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
            let had_active = d
                .state
                .active
                .iter()
                .any(|id| d.get(*id).is_some_and(|n| n.style.active.is_some()));
            d.state.active.clear();
            if had_active {
                d.mark_style();
            }
        });
    }

    fn scroll(&mut self, p: PointF, dx: f32, dy: f32) {
        self.ensure_layout();
        let Some(target) = self.hit_test(p) else {
            return;
        };
        let chain = with_doc(|d| d.ancestors(target));
        for &id in &chain {
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
                    stopped: Cell::new(false),
                };
                batch(|| h(&ev));
                if ev.stopped.get() {
                    return;
                }
            }
        }
        // Shift turns vertical wheel motion into horizontal scrolling.
        let (dx, dy) = if self.modifiers.shift && dx == 0.0 {
            (dy, 0.0)
        } else {
            (dx, dy)
        };
        let scrolled = with_doc(|d| {
            for id in chain {
                if scroll_node(d, id, dx, dy) {
                    return true;
                }
            }
            false
        });
        if scrolled {
            self.ensure_layout();
            // Keep extending an in-progress selection while the content scrolls under it.
            if matches!(
                self.press.as_ref().map(|p| p.drag),
                Some(Drag::Select | Drag::Edit(_))
            ) {
                self.pointer_moved(p);
            } else {
                self.update_hover(p);
            }
        }
    }

    fn set_focus(&mut self, next: Option<NodeId>) {
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
            }
            if let Some(e) = next.and_then(|id| d.get_mut(id)).and_then(Node::edit_mut) {
                e.caret_epoch = Instant::now();
                e.scroll_to_caret = true;
            }
            if let Some(e) = prev.and_then(|id| d.get_mut(id)).and_then(Node::edit_mut) {
                e.preedit = None;
            }
            d.mark_style();
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
            self.set_focus(Some(n));
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
            Key::Named(NamedKey::Escape) => with_doc(|d| {
                if d.state.selection.take().is_some() {
                    d.mark_paint();
                }
            }),
            Key::Named(NamedKey::Enter | NamedKey::Space) if focused.is_some() && !key.repeat => {
                let t = focused.unwrap();
                let rect = with_doc(|d| d.get(t).map(|n| n.abs).unwrap_or_default());
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
                d.mark_position_only();
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
            Some(Some(e.ime_state(caret)))
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

impl Document {
    /// Caret or selection moved without changing text: repaint and re-scroll only.
    fn mark_position_only(&mut self) {
        self.dirty.position = true;
        self.dirty.paint = true;
    }
}

fn hit(doc: &Document, id: NodeId, p: PointF) -> Option<NodeId> {
    let node = doc.get(id)?;
    if !node.visible || !node.clip.contains(p) {
        return None;
    }
    for &c in node.children.iter().rev() {
        if let Some(h) = hit(doc, c, p) {
            return Some(h);
        }
    }
    if matches!(node.kind, NodeKind::Fragment) || !node.computed.pointer_events {
        return None;
    }
    node.abs.contains(p).then_some(id)
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

fn scroll_node(doc: &mut Document, id: NodeId, dx: f32, dy: f32) -> bool {
    let Some(n) = doc.get_mut(id) else {
        return false;
    };
    let pad = n.padding_abs();
    let mut changed = false;
    if n.computed.overflow_y == Overflow::Scroll && dy != 0.0 {
        let max = (n.content_size.height - pad.height).max(0);
        let next = (n.scroll.y + dy.round() as i32).clamp(0, max);
        changed |= next != n.scroll.y;
        n.scroll.y = next;
    }
    if n.computed.overflow_x == Overflow::Scroll && dx != 0.0 {
        let max = (n.content_size.width - pad.width).max(0);
        let next = (n.scroll.x + dx.round() as i32).clamp(0, max);
        changed |= next != n.scroll.x;
        n.scroll.x = next;
    }
    if !changed && let Some(e) = n.edit_mut() {
        // Edits scroll their own text.
        let view = pad;
        let max_y = (e.content_size.height - view.height).max(0);
        let next = (e.scroll.y + dy.round() as i32).clamp(0, max_y);
        if e.multiline && next != e.scroll.y {
            e.scroll.y = next;
            changed = true;
        }
    }
    if changed {
        doc.mark_position_only();
    }
    changed
}

/// Hit-tests the scrollbar thumbs of `target`'s scroll containers.
fn scrollbar_hit(doc: &Document, target: NodeId, p: PointF) -> Option<(NodeId, bool, f32)> {
    let grab_width = doc.scale.pxf(12.0);
    for id in doc.ancestors(target) {
        let n = doc.get(id)?;
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
    doc.mark_position_only();
}

/// Adjusts the internal scroll of edits so their caret stays visible.
fn scroll_carets_into_view(doc: &mut Document) {
    let ids: Vec<NodeId> = doc
        .nodes
        .iter()
        .filter(|(_, n)| n.edit().is_some_and(|e| e.scroll_to_caret))
        .map(|(id, _)| id)
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
        doc.dirty.paint = true;
    }
}
