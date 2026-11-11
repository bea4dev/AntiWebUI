//! Popups: `xdg_popup` for what the compositor places, `wl_subsurface` for what we place ourselves
//! (see `docs/design/09-shadows-stacking-popups.md`).

use crate::state::{State, SurfaceId};
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{PointI, RectI, SizeI};
use anti_webui_core::platform::{Placement, PopupId, PopupKind, PopupParams};
use wayland_client::protocol::{wl_subcompositor, wl_subsurface, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_surface_v1;
use wayland_protocols::wp::viewporter::client::wp_viewport;
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface};

/// What a popup needs from the frame loop, like a window: see `State::window_frame`.
#[derive(Default)]
pub(crate) struct SurfaceFrame {
    /// A frame was committed at this time and its callback hasn't come.
    pub pending: Option<std::time::Instant>,
    /// A frame callback was requested but not committed yet.
    pub callback_queued: bool,
    /// When the last frame callback came.
    pub last_callback: Option<std::time::Instant>,
    /// A buffer was committed, so popups can attach to it.
    pub mapped: bool,
    pub opaque: bool,
    pub opaque_applied: Option<(i32, i32)>,
    pub syncobj: Option<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1>,
}

enum Role {
    Xdg {
        xdg_surface: xdg_surface::XdgSurface,
        popup: xdg_popup::XdgPopup,
    },
    Sub(wl_subsurface::WlSubsurface),
}

pub(crate) struct Popup {
    pub surface: wl_surface::WlSurface,
    viewport: Option<wp_viewport::WpViewport>,
    role: Role,
    pub frame: SurfaceFrame,
    pub params: PopupParams,
    /// The last `xdg_popup.configure`: position relative to the parent's box, and size.
    configure: (i32, i32, i32, i32),
    /// The surface's origin relative to the window's surface, in logical pixels.
    pub offset: PointI,
    /// The same in physical pixels, the way the core knows it.
    pub origin: PointI,
    repositions: u32,
}

/// `xdg_positioner`'s anchor and gravity, and how to deal with not fitting on the screen: first the
/// other side, then along the edge, and shrink as a last resort.
fn positioning(
    placement: Placement,
) -> (
    xdg_positioner::Anchor,
    xdg_positioner::Gravity,
    xdg_positioner::ConstraintAdjustment,
) {
    use xdg_positioner::{Anchor, ConstraintAdjustment as C, Gravity};
    let vertical = C::FlipY | C::SlideX | C::ResizeY;
    let sideways = C::FlipX | C::SlideY | C::ResizeY;
    match placement {
        Placement::BelowStart => (Anchor::BottomLeft, Gravity::BottomRight, vertical),
        Placement::BelowEnd => (Anchor::BottomRight, Gravity::BottomLeft, vertical),
        Placement::BelowCenter => (Anchor::Bottom, Gravity::Bottom, vertical),
        Placement::AboveStart => (Anchor::TopLeft, Gravity::TopRight, vertical),
        Placement::AboveEnd => (Anchor::TopRight, Gravity::TopLeft, vertical),
        Placement::AboveCenter => (Anchor::Top, Gravity::Top, vertical),
        Placement::RightStart => (Anchor::TopRight, Gravity::BottomRight, sideways),
        Placement::LeftStart => (Anchor::TopLeft, Gravity::BottomLeft, sideways),
    }
}

/// Part of an anchor along one axis, which the protocol wants non-empty and inside of the parent.
fn anchor_span(lo: i32, hi: i32, parent: i32) -> (i32, i32) {
    let lo = lo.clamp(0, (parent - 1).max(0));
    let hi = hi.clamp(lo + 1, parent.max(lo + 1));
    (lo, hi - lo)
}

impl State {
    /// A parent's box relative to the window's surface, in logical pixels.
    fn parent_box(&self, parent: Option<PopupId>) -> RectI {
        match parent.and_then(|p| self.popups.get(&p)) {
            Some(p) => p.params.geometry.translate(p.offset.x, p.offset.y),
            None => RectI::new(0, 0, self.logical_size.0, self.logical_size.1),
        }
    }

    fn positioner(&self, params: &PopupParams) -> xdg_positioner::XdgPositioner {
        let s = self.scale;
        let parent = self.parent_box(params.parent);
        let a = params.anchor;
        let (x, w) = anchor_span(
            s.logical_floor(a.x) - parent.x,
            s.logical_ceil(a.right()) - parent.x,
            parent.width,
        );
        let (y, h) = anchor_span(
            s.logical_floor(a.y) - parent.y,
            s.logical_ceil(a.bottom()) - parent.y,
            parent.height,
        );
        let positioner = self.wm_base.create_positioner(&self.qh, ());
        let (anchor, gravity, constraints) = positioning(params.placement);
        positioner.set_size(params.wanted.width.max(1), params.wanted.height.max(1));
        positioner.set_anchor_rect(x, y, w, h);
        positioner.set_anchor(anchor);
        positioner.set_gravity(gravity);
        positioner.set_constraint_adjustment(constraints);
        match params.placement {
            Placement::RightStart => positioner.set_offset(params.gap, 0),
            Placement::LeftStart => positioner.set_offset(-params.gap, 0),
            Placement::AboveStart | Placement::AboveEnd | Placement::AboveCenter => {
                positioner.set_offset(0, -params.gap)
            }
            Placement::BelowStart | Placement::BelowEnd | Placement::BelowCenter => {
                positioner.set_offset(0, params.gap)
            }
        }
        positioner
    }

    fn surface_of(&self, parent: Option<PopupId>) -> Option<&wl_surface::WlSurface> {
        match parent {
            Some(p) => self.popups.get(&p).map(|p| &p.surface),
            None => Some(&self.surface),
        }
    }

    /// Shows the popup, or once what it attaches to has a buffer.
    pub(crate) fn show_popup(&mut self, params: PopupParams) {
        let ready = match params.parent {
            Some(p) => self.popups.get(&p).is_some_and(|p| p.frame.mapped),
            None => self.window_frame.mapped,
        };
        if ready {
            self.create_popup(params);
        } else {
            self.waiting_popups.push(params);
        }
    }

    /// Creates the popups that were waiting for what just got its first buffer.
    pub(crate) fn create_waiting_popups(&mut self) {
        for params in std::mem::take(&mut self.waiting_popups) {
            self.show_popup(params);
        }
    }

    fn create_popup(&mut self, params: PopupParams) {
        let id = params.id;
        let tag = SurfaceId::Popup(id);
        let surface = self.compositor.create_surface(&self.qh, tag);
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, &self.qh, ()));
        // A lock surface can't be the parent of an xdg popup: its menus are subsurfaces too.
        let locked = matches!(self.role, crate::state::SurfaceRole::Lock(_));
        let role = match (params.kind, &self.subcompositor, params.position) {
            (kind, Some(sub), Some(_)) if kind == PopupKind::Bubble || locked => {
                let Some(parent) = self.surface_of(params.parent) else {
                    surface.destroy();
                    return;
                };
                let role = sub.get_subsurface(&surface, parent, &self.qh, ());
                role.set_desync();
                Role::Sub(role)
            }
            _ => {
                let parent = match params.parent {
                    Some(p) => match self.popups.get(&p).map(|p| &p.role) {
                        Some(Role::Xdg { xdg_surface, .. }) => Some(xdg_surface),
                        _ => None,
                    },
                    None => self.xdg_surface(),
                };
                let ancestors = self.ancestors(params.parent);
                let grabbing = params.kind == PopupKind::Menu && self.seat.is_some();
                let shared = self.connection_popups.clone();
                let mut shared = shared.lock();
                // Another window may have ended the popup it goes on.
                if let Some(p) = params.parent
                    && parent.is_some()
                    && !shared.has(&self.mailbox, p)
                {
                    if let Some(vp) = viewport {
                        vp.destroy();
                    }
                    surface.destroy();
                    self.events.push(WindowEvent::PopupDismissed(id));
                    return;
                }
                if grabbing {
                    shared.end_other_grabs(&self.mailbox, &ancestors);
                }
                let xdg_surface = self.wm_base.get_xdg_surface(&surface, &self.qh, tag);
                let positioner = self.positioner(&params);
                let popup = xdg_surface.get_popup(parent, &positioner, &self.qh, id);
                positioner.destroy();
                // On a layer surface, the popup is given its parent this way.
                if params.parent.is_none()
                    && let crate::state::SurfaceRole::Layer(l) = &self.role
                {
                    l.object.get_popup(&popup);
                }
                if grabbing && let Some(seat) = &self.seat {
                    popup.grab(seat, self.press_serial);
                }
                shared.add(&self.mailbox, id, ancestors, popup.clone(), grabbing);
                Role::Xdg { xdg_surface, popup }
            }
        };
        self.popups.insert(
            id,
            Popup {
                surface,
                viewport,
                role,
                frame: SurfaceFrame::default(),
                params: params.clone(),
                configure: (0, 0, params.wanted.width, params.wanted.height),
                offset: PointI::default(),
                origin: PointI::default(),
                repositions: 0,
            },
        );
        self.apply_popup_surface(id);
        self.popups[&id].surface.commit();
        if matches!(self.popups[&id].role, Role::Sub(_)) {
            self.place_subsurface(id);
        }
    }

    /// Sets what comes with the next commit.
    fn apply_popup_surface(&mut self, id: PopupId) {
        let Some(p) = self.popups.get(&id) else {
            return;
        };
        let (logical, geometry) = (p.params.logical, p.params.geometry);
        if let Some(vp) = &p.viewport {
            vp.set_destination(logical.width.max(1), logical.height.max(1));
        } else {
            p.surface
                .set_buffer_scale((self.scale.0 / 120).max(1) as i32);
        }
        if let Role::Xdg { xdg_surface, .. } = &p.role {
            xdg_surface.set_window_geometry(
                geometry.x,
                geometry.y,
                geometry.width.max(1),
                geometry.height.max(1),
            );
        }
        // Input goes through what is only shadow, and through tooltips altogether.
        let region = self.compositor.create_region(&self.qh, ());
        if p.params.kind != PopupKind::Tooltip {
            region.add(geometry.x, geometry.y, geometry.width, geometry.height);
        }
        p.surface.set_input_region(Some(&region));
        region.destroy();
    }

    /// A subsurface goes where we say, and its position applies when its parent commits.
    fn place_subsurface(&mut self, id: PopupId) {
        let Some(p) = self.popups.get(&id) else {
            return;
        };
        let Some(position) = p.params.position else {
            return;
        };
        let parent = p.params.parent.and_then(|q| self.popups.get(&q));
        let base = parent.map_or(PointI::default(), |q| q.offset);
        if let Role::Sub(sub) = &p.role {
            sub.set_position(position.x - base.x, position.y - base.y);
        }
        let origin = self.logical_to_physical(position);
        let size = SizeI::new(p.params.geometry.width, p.params.geometry.height);
        if let Some(parent) = self.surface_of(p.params.parent) {
            parent.commit();
        }
        if let Some(p) = self.popups.get_mut(&id) {
            p.offset = position;
            p.origin = origin;
        }
        self.events
            .push(WindowEvent::PopupConfigured { id, origin, size });
    }

    fn logical_to_physical(&self, p: PointI) -> PointI {
        PointI::new(self.scale.px(p.x as f32), self.scale.px(p.y as f32))
    }

    pub(crate) fn update_popup(&mut self, params: PopupParams) {
        let id = params.id;
        if let Some(waiting) = self.waiting_popups.iter_mut().find(|p| p.id == id) {
            *waiting = params;
            return;
        }
        let Some(p) = self.popups.get_mut(&id) else {
            return;
        };
        let old = std::mem::replace(&mut p.params, params.clone());
        self.apply_popup_surface(id);
        let moved = old.anchor != params.anchor
            || old.wanted != params.wanted
            || old.placement != params.placement
            || old.gap != params.gap;
        match &self.popups[&id].role {
            Role::Xdg { popup, .. } if moved && popup.version() >= 3 => {
                let popup = popup.clone();
                let positioner = self.positioner(&params);
                let p = self.popups.get_mut(&id).unwrap();
                p.repositions += 1;
                popup.reposition(&positioner, p.repositions);
                positioner.destroy();
            }
            Role::Sub(_) if old.position != params.position => self.place_subsurface(id),
            _ => {}
        }
    }

    /// Closes a popup and everything attached to it, in the order the protocol asks for.
    pub(crate) fn close_popup(&mut self, id: PopupId) {
        self.waiting_popups.retain(|p| p.id != id);
        for child in self.popups_attached_to(id) {
            self.close_popup(child);
        }
        let Some(p) = self.popups.remove(&id) else {
            return;
        };
        if self.pointer_surface == SurfaceId::Popup(id) {
            self.pointer_surface = SurfaceId::Window;
        }
        match p.role {
            Role::Xdg { xdg_surface, .. } => {
                self.connection_popups.lock().destroy(&self.mailbox, id);
                xdg_surface.destroy();
            }
            Role::Sub(sub) => sub.destroy(),
        }
        if let Some(vp) = p.viewport {
            vp.destroy();
        }
        if let Some(s) = p.frame.syncobj {
            s.destroy();
        }
        p.surface.destroy();
    }

    /// The popups that one attached to `parent` is attached to, `parent` first.
    fn ancestors(&self, mut parent: Option<PopupId>) -> Vec<PopupId> {
        let mut ancestors = Vec::new();
        while let Some(p) = parent {
            ancestors.push(p);
            parent = self.popups.get(&p).and_then(|p| p.params.parent);
        }
        ancestors
    }

    /// Popups whose grab another window ended: they are dismissed, as when the compositor ends a
    /// grab. Their surfaces stay until the core closes them.
    pub(crate) fn take_ended_popups(&mut self) {
        for id in self.mailbox.take() {
            if self.popups.contains_key(&id) {
                self.events.push(WindowEvent::PopupDismissed(id));
            }
        }
    }

    pub(crate) fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.popups
            .iter()
            .filter(|(_, p)| p.params.parent == Some(id))
            .map(|(child, _)| *child)
            .collect()
    }

    /// The compositor decided where the popup goes.
    pub(crate) fn popup_placed(&mut self, id: PopupId) {
        let Some(p) = self.popups.get(&id) else {
            return;
        };
        let (x, y, w, h) = p.configure;
        let geometry = p.params.geometry;
        let parent = self.parent_box(p.params.parent);
        let offset = PointI::new(parent.x + x - geometry.x, parent.y + y - geometry.y);
        let origin = self.logical_to_physical(offset);
        let size = SizeI::new(w.max(1), h.max(1));
        if let Some(p) = self.popups.get_mut(&id) {
            p.offset = offset;
            p.origin = origin;
        }
        self.events
            .push(WindowEvent::PopupConfigured { id, origin, size });
        self.redraw_requested = true;
    }
}

impl Dispatch<xdg_popup::XdgPopup, PopupId> for State {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        id: &PopupId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                if let Some(p) = state.popups.get_mut(id) {
                    p.configure = (x, y, width, height);
                }
            }
            xdg_popup::Event::PopupDone => state.events.push(WindowEvent::PopupDismissed(*id)),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore xdg_positioner::XdgPositioner);
delegate_noop!(State: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(State: ignore wl_subsurface::WlSubsurface);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_stay_inside_of_the_parent_and_are_never_empty() {
        assert_eq!(anchor_span(10, 20, 100), (10, 10));
        assert_eq!(anchor_span(-5, 20, 100), (0, 20));
        assert_eq!(anchor_span(90, 120, 100), (90, 10));
        assert_eq!(anchor_span(30, 30, 100), (30, 1));
        // Scrolled out of the window on either side.
        assert_eq!(anchor_span(-40, -30, 100), (0, 1));
        assert_eq!(anchor_span(130, 140, 100), (99, 1));
    }
}
