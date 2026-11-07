//! Popups as override-redirect windows placed by us, and bubbles as child windows (see
//! "ポップアップ" in `docs/design/35-platform-x11.md`).

use crate::present::Frames;
use crate::state::State;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{Placement, PopupId, PopupKind, PopupParams};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask,
    GrabMode, GrabStatus, PropMode, Rectangle, Visualid, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

pub(crate) struct Popup {
    pub window: Window,
    pub depth: u8,
    colormap: Option<u32>,
    pub params: PopupParams,
    /// The window's origin in window coordinates (physical), as the core knows it.
    pub origin: PointI,
    pub frames: Frames,
    /// It was uncovered and has to be drawn again in full.
    pub exposed: bool,
}

/// The box of a popup in window coordinates, given the anchor, the size it would like and the
/// area it has to stay in, all physical. Like `xdg_positioner`: the other side if it doesn't fit,
/// then along the edge, and smaller as a last resort.
pub(crate) fn place(
    placement: Placement,
    anchor: RectI,
    gap: i32,
    size: SizeI,
    area: RectI,
) -> RectI {
    let (w, h) = (size.width.max(1), size.height.max(1));
    let center_x = anchor.x + (anchor.width - w) / 2;
    let below = anchor.bottom() + gap;
    let above = anchor.y - gap - h;
    let (x, y) = match placement {
        Placement::BelowStart => (anchor.x, below),
        Placement::BelowEnd => (anchor.right() - w, below),
        Placement::BelowCenter => (center_x, below),
        Placement::AboveStart => (anchor.x, above),
        Placement::AboveEnd => (anchor.right() - w, above),
        Placement::AboveCenter => (center_x, above),
        Placement::RightStart => (anchor.right() + gap, anchor.y),
        Placement::LeftStart => (anchor.x - gap - w, anchor.y),
    };
    let mut r = RectI::new(x, y, w, h);
    let sideways = matches!(placement, Placement::RightStart | Placement::LeftStart);
    if sideways {
        let flipped = match placement {
            Placement::RightStart => anchor.x - gap - w,
            _ => anchor.right() + gap,
        };
        if (r.x < area.x || r.right() > area.right())
            && flipped >= area.x
            && flipped + w <= area.right()
        {
            r.x = flipped;
        }
        r.y = slide(r.y, h, area.y, area.bottom());
        if r.bottom() > area.bottom() {
            r.height = (area.bottom() - r.y).max(1);
        }
    } else {
        let below_room = area.bottom() - below;
        let above_room = anchor.y - gap - area.y;
        let goes_below = matches!(
            placement,
            Placement::BelowStart | Placement::BelowEnd | Placement::BelowCenter
        );
        let (room, other_room) = if goes_below {
            (below_room, above_room)
        } else {
            (above_room, below_room)
        };
        let flip = room < h && other_room > room;
        let on_below = goes_below != flip;
        let fit = if on_below { below_room } else { above_room };
        r.height = h.min(fit.max(1));
        r.y = if on_below {
            below
        } else {
            anchor.y - gap - r.height
        };
        r.x = slide(r.x, w, area.x, area.right());
    }
    r
}

/// Moves `[at, at + len)` into `[lo, hi)` if it can.
fn slide(at: i32, len: i32, lo: i32, hi: i32) -> i32 {
    if at + len > hi {
        (hi - len).max(lo)
    } else if at < lo {
        lo
    } else {
        at
    }
}

/// A TrueColor visual of 32 bits, for what has alpha.
pub(crate) fn argb_visual(conn: &impl Connection, screen: usize) -> Option<Visualid> {
    let depth = conn.setup().roots[screen]
        .allowed_depths
        .iter()
        .find(|d| d.depth == 32)?;
    depth
        .visuals
        .iter()
        .find(|v| v.class == x11rb::protocol::xproto::VisualClass::TRUE_COLOR)
        .map(|v| v.visual_id)
}

impl State {
    /// Whether something composites windows, so that alpha shows what is behind.
    pub(crate) fn composited(&self) -> bool {
        if self.xwayland {
            return true;
        }
        let name = format!("_NET_WM_CM_S{}", self.screen);
        self.conn
            .intern_atom(false, name.as_bytes())
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| self.conn.get_selection_owner(r.atom).ok()?.reply().ok())
            .is_some_and(|r| r.owner != NONE)
    }

    /// The window's origin on the screen.
    pub(crate) fn window_on_screen(&self) -> PointI {
        self.conn
            .translate_coordinates(self.window, self.root, 0, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(PointI::default(), |r| {
                PointI::new(r.dst_x as i32, r.dst_y as i32)
            })
    }

    /// The part of the screen a popup at `at` (screen coordinates) has to stay in.
    fn area_at(&self, at: PointI) -> RectI {
        let screen = &self.conn.setup().roots[self.screen];
        let mut area = RectI::new(
            0,
            0,
            screen.width_in_pixels as i32,
            screen.height_in_pixels as i32,
        );
        if let Some(monitors) = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
        {
            let rects = monitors
                .monitors
                .iter()
                .map(|m| RectI::new(m.x as i32, m.y as i32, m.width as i32, m.height as i32));
            let rects: Vec<RectI> = rects.collect();
            let at = anti_webui_core::geometry::PointF::new(at.x as f32, at.y as f32);
            if let Some(r) = rects.iter().find(|r| r.contains(at)).or(rects.first()) {
                area = *r;
            }
        }
        let workarea = self
            .conn
            .get_property(
                false,
                self.root,
                self.atoms._NET_WORKAREA,
                AtomEnum::CARDINAL,
                0,
                4,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>()));
        if let Some(w) = workarea.filter(|w| w.len() == 4) {
            let w = RectI::new(w[0] as i32, w[1] as i32, w[2] as i32, w[3] as i32);
            let i = area.intersect(&w);
            if !i.is_empty() {
                area = i;
            }
        }
        area
    }

    /// Where a popup's window goes in window coordinates, and the logical size of its box.
    fn popup_placement(&self, params: &PopupParams) -> (PointI, SizeI) {
        let s = self.scale;
        let margin = PointI::new(
            s.px(params.geometry.x as f32),
            s.px(params.geometry.y as f32),
        );
        if let Some(position) = params.position.filter(|_| params.kind == PopupKind::Bubble) {
            let origin = PointI::new(s.px(position.x as f32), s.px(position.y as f32));
            let size = SizeI::new(params.geometry.width, params.geometry.height);
            return (origin, size);
        }
        let on_screen = self.window_on_screen();
        let anchor = params.anchor;
        let center = PointI::new(
            on_screen.x + anchor.x + anchor.width / 2,
            on_screen.y + anchor.y + anchor.height / 2,
        );
        let area = self.area_at(center).translate(-on_screen.x, -on_screen.y);
        let size = SizeI::new(
            s.px(params.wanted.width as f32),
            s.px(params.wanted.height as f32),
        );
        let gap = s.px(params.gap as f32);
        let b = place(params.placement, anchor, gap, size, area);
        let granted = SizeI::new(
            params.wanted.width.min(logical_floor(b.width, s)).max(1),
            params.wanted.height.min(logical_floor(b.height, s)).max(1),
        );
        (PointI::new(b.x - margin.x, b.y - margin.y), granted)
    }

    pub(crate) fn show_popup(&mut self, params: PopupParams) {
        if self.popups.contains_key(&params.id) {
            return self.update_popup(params);
        }
        let Some(popup) = self.create_popup(&params) else {
            self.events.push(WindowEvent::PopupDismissed(params.id));
            return;
        };
        let id = params.id;
        self.popups.insert(id, popup);
        self.place_popup(id);
        if params.kind == PopupKind::Menu {
            self.grab_for_menus();
        }
    }

    fn create_popup(&mut self, params: &PopupParams) -> Option<Popup> {
        let conn = &*self.conn;
        let bubble = params.kind == PopupKind::Bubble && params.position.is_some();
        let alpha = if bubble {
            // The server composites what differs in depth from its parent.
            argb_visual(conn, self.screen)
        } else if self.composited() {
            argb_visual(conn, self.screen)
        } else {
            None
        };
        let (depth, visual) = match alpha {
            Some(v) => (32, v),
            None => (self.depth, conn.setup().roots[self.screen].root_visual),
        };
        let parent = match (bubble, params.parent.and_then(|p| self.popups.get(&p))) {
            (true, Some(p)) => p.window,
            (true, None) => self.window,
            (false, _) => self.root,
        };
        let window = conn.generate_id().ok()?;
        let mut aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY)
            .override_redirect(u32::from(!bubble));
        let colormap = if depth == 32 {
            let c = conn.generate_id().ok()?;
            conn.create_colormap(ColormapAlloc::NONE, c, self.root, visual)
                .ok()?;
            aux = aux.colormap(c);
            Some(c)
        } else {
            None
        };
        let b = params.buffer;
        conn.create_window(
            depth,
            window,
            parent,
            0,
            0,
            b.width.clamp(1, u16::MAX as i32) as u16,
            b.height.clamp(1, u16::MAX as i32) as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &aux,
        )
        .ok()?;
        let a = &self.atoms;
        if !bubble {
            let kind = match (params.kind, params.parent) {
                (PopupKind::Tooltip, _) => a._NET_WM_WINDOW_TYPE_TOOLTIP,
                (_, None) => a._NET_WM_WINDOW_TYPE_DROPDOWN_MENU,
                (_, Some(_)) => a._NET_WM_WINDOW_TYPE_POPUP_MENU,
            };
            let _ = conn.change_property32(
                PropMode::REPLACE,
                window,
                a._NET_WM_WINDOW_TYPE,
                AtomEnum::ATOM,
                &[kind],
            );
            let _ = conn.change_property32(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                &[self.window],
            );
        }
        self.select_pointer(window);
        self.present.select(&self.conn, window);
        let popup = Popup {
            window,
            depth,
            colormap,
            params: params.clone(),
            origin: PointI::default(),
            frames: Frames::default(),
            exposed: false,
        };
        self.shape_popup(&popup);
        let _ = self.conn.map_window(window);
        Some(popup)
    }

    /// Input goes through what is only shadow, and through tooltips altogether. Without alpha, the
    /// shadow isn't shown either.
    fn shape_popup(&self, popup: &Popup) {
        let s = self.scale;
        let g = popup.params.geometry;
        let rect = Rectangle {
            x: s.px(g.x as f32) as i16,
            y: s.px(g.y as f32) as i16,
            width: s.px(g.width as f32).max(1) as u16,
            height: s.px(g.height as f32).max(1) as u16,
        };
        let Ok(region) = self.conn.generate_id() else {
            return;
        };
        let input: &[Rectangle] = if popup.params.kind == PopupKind::Tooltip {
            &[]
        } else {
            std::slice::from_ref(&rect)
        };
        if self.conn.xfixes_create_region(region, input).is_err() {
            return;
        }
        let _ =
            self.conn
                .xfixes_set_window_shape_region(popup.window, shape::SK::INPUT, 0, 0, region);
        if popup.depth != 32 {
            let _ = self.conn.xfixes_set_region(region, &[rect]);
            let _ = self.conn.xfixes_set_window_shape_region(
                popup.window,
                shape::SK::BOUNDING,
                0,
                0,
                region,
            );
        }
        let _ = self.conn.xfixes_destroy_region(region);
    }

    /// Puts the popup where it goes and tells the core.
    fn place_popup(&mut self, id: PopupId) {
        let Some(p) = self.popups.get(&id) else {
            return;
        };
        let (origin, size) = self.popup_placement(&p.params);
        let bubble = p.params.kind == PopupKind::Bubble && p.params.position.is_some();
        let at = if bubble {
            let base = p.params.parent.and_then(|q| self.popups.get(&q));
            let base = base.map_or(PointI::default(), |q| q.origin);
            PointI::new(origin.x - base.x, origin.y - base.y)
        } else {
            let on_screen = self.window_on_screen();
            PointI::new(on_screen.x + origin.x, on_screen.y + origin.y)
        };
        let b = p.params.buffer;
        let aux = ConfigureWindowAux::new()
            .x(at.x)
            .y(at.y)
            .width(b.width.max(1) as u32)
            .height(b.height.max(1) as u32);
        let _ = self.conn.configure_window(p.window, &aux);
        if let Some(p) = self.popups.get_mut(&id) {
            p.origin = origin;
        }
        self.events
            .push(WindowEvent::PopupConfigured { id, origin, size });
        self.redraw_requested = true;
    }

    pub(crate) fn update_popup(&mut self, params: PopupParams) {
        let id = params.id;
        let Some(p) = self.popups.get_mut(&id) else {
            return;
        };
        let old = std::mem::replace(&mut p.params, params.clone());
        if old.geometry != params.geometry || old.buffer != params.buffer {
            let p = &self.popups[&id];
            self.shape_popup(p);
        }
        let moved = old.anchor != params.anchor
            || old.wanted != params.wanted
            || old.placement != params.placement
            || old.gap != params.gap
            || old.position != params.position
            || old.geometry != params.geometry
            || old.buffer != params.buffer;
        if moved {
            self.place_popup(id);
        }
    }

    pub(crate) fn close_popup(&mut self, id: PopupId) {
        for child in self.popups_attached_to(id) {
            self.close_popup(child);
        }
        let Some(p) = self.popups.remove(&id) else {
            return;
        };
        let _ = self.conn.destroy_window(p.window);
        if let Some(c) = p.colormap {
            let _ = self.conn.free_colormap(c);
        }
        if p.params.kind == PopupKind::Menu
            && !self
                .popups
                .values()
                .any(|p| p.params.kind == PopupKind::Menu)
        {
            self.ungrab();
        }
    }

    pub(crate) fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.popups
            .iter()
            .filter(|(_, p)| p.params.parent == Some(id))
            .map(|(child, _)| *child)
            .collect()
    }

    /// Tells the core that menus closed, the last ones first, as `popup_done` does on Wayland.
    pub(crate) fn dismiss(&mut self, kinds: &[PopupKind]) {
        let mut ids: Vec<(usize, PopupId)> = self
            .popups
            .iter()
            .filter(|(_, p)| kinds.contains(&p.params.kind))
            .map(|(&id, _)| (self.popup_depth(id), id))
            .collect();
        ids.sort_by_key(|(depth, id)| (std::cmp::Reverse(*depth), id.0));
        for (_, id) in ids {
            self.events.push(WindowEvent::PopupDismissed(id));
        }
    }

    fn popup_depth(&self, mut id: PopupId) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.popups.get(&id).and_then(|p| p.params.parent) {
            depth += 1;
            id = parent;
        }
        depth
    }

    /// Takes the pointer and the keyboard, so that pressing outside of the app can close menus.
    /// Our own windows still get their events.
    fn grab_for_menus(&mut self) {
        let Some(top) = self
            .popups
            .iter()
            .filter(|(_, p)| p.params.kind == PopupKind::Menu)
            .max_by_key(|(id, _)| self.popup_depth(**id))
            .map(|(_, p)| p.window)
        else {
            return;
        };
        if self.xi_opcode.is_some() {
            let grabbed = self
                .conn
                .xinput_xi_grab_device(
                    top,
                    CURRENT_TIME,
                    NONE,
                    self.pointer_device,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    xinput::GrabOwner::OWNER,
                    &[u32::from(self.pointer_mask())],
                )
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| r.status == GrabStatus::SUCCESS);
            self.grabbed = grabbed;
        }
        let _ = self
            .conn
            .grab_keyboard(true, top, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC);
    }

    fn ungrab(&mut self) {
        if std::mem::take(&mut self.grabbed) {
            let _ = self
                .conn
                .xinput_xi_ungrab_device(CURRENT_TIME, self.pointer_device);
        }
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
    }

    /// Where a window of ours is in window coordinates, if it is ours.
    pub(crate) fn origin_of(&self, window: Window) -> Option<PointI> {
        if window == self.window {
            return Some(PointI::default());
        }
        self.popups
            .values()
            .find(|p| p.window == window)
            .map(|p| p.origin)
    }

    /// Whether a press during the grab was outside of all our windows. With owner events, what
    /// was on our windows went to them, and the rest comes to the grabbing popup, outside of its
    /// box (its shadow lets input through).
    pub(crate) fn press_outside(&self, window: Window, x: f64, y: f64) -> bool {
        if !self.grabbed {
            return false;
        }
        let Some(p) = self.popups.values().find(|p| p.window == window) else {
            return false;
        };
        let s = self.scale;
        let g = p.params.geometry;
        let in_box = x >= s.px(g.x as f32) as f64
            && y >= s.px(g.y as f32) as f64
            && x < s.px(g.right() as f32) as f64
            && y < s.px(g.bottom() as f32) as f64;
        !in_box
    }
}

/// The largest logical length within `physical` pixels.
fn logical_floor(physical: i32, s: Scale) -> i32 {
    (physical as f64 / s.as_f64()).floor() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: RectI = RectI {
        x: 0,
        y: 0,
        width: 1000,
        height: 800,
    };

    #[test]
    fn below_the_anchor_with_the_gap() {
        let anchor = RectI::new(100, 100, 80, 30);
        let r = place(Placement::BelowStart, anchor, 4, SizeI::new(200, 300), AREA);
        assert_eq!(r, RectI::new(100, 134, 200, 300));
        let r = place(Placement::BelowEnd, anchor, 4, SizeI::new(200, 300), AREA);
        assert_eq!(r.x, 0, "would start left of the area");
        let r = place(Placement::AboveCenter, anchor, 4, SizeI::new(40, 20), AREA);
        assert_eq!(r, RectI::new(120, 76, 40, 20));
    }

    #[test]
    fn flips_when_there_is_more_room_on_the_other_side() {
        let anchor = RectI::new(100, 700, 80, 30);
        let r = place(Placement::BelowStart, anchor, 4, SizeI::new(200, 300), AREA);
        assert_eq!(r, RectI::new(100, 700 - 4 - 300, 200, 300));
    }

    #[test]
    fn slides_along_the_edge_and_shrinks_as_a_last_resort() {
        let anchor = RectI::new(950, 100, 40, 30);
        let r = place(
            Placement::BelowStart,
            anchor,
            0,
            SizeI::new(200, 2000),
            AREA,
        );
        assert_eq!(r.x, 800, "slid in from the right");
        assert_eq!((r.y, r.height), (130, 670), "the longer side, made to fit");
    }

    #[test]
    fn submenus_go_to_the_other_side_at_the_edge() {
        let anchor = RectI::new(850, 200, 100, 30);
        let r = place(Placement::RightStart, anchor, 2, SizeI::new(120, 100), AREA);
        assert_eq!(r, RectI::new(850 - 2 - 120, 200, 120, 100));
        let anchor = RectI::new(300, 750, 100, 30);
        let r = place(Placement::RightStart, anchor, 2, SizeI::new(120, 100), AREA);
        assert_eq!(r.y, 700, "slid up");
    }
}
