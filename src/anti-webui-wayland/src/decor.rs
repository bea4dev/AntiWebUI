//! The frame of the window: who draws it (xdg-decoration), the states the compositor tells, moving
//! and resizing for a frame of the window's own, and the edges to resize it from outside its
//! surface (see "窓の飾り" in `docs/design/36-multi-window.md`).

use crate::state::State;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::window::{DecorationMode, Decorations, ResizeEdge};
use std::os::fd::AsFd;
use wayland_client::protocol::{
    wl_buffer, wl_pointer, wl_shm, wl_shm_pool, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use wayland_protocols::wp::viewporter::client::wp_viewport;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1;
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::shell::client::xdg_toplevel;

/// How far out of the window its edges can be taken hold of, in logical pixels.
const BORDER: i32 = 8;
/// How far along an edge from a corner taking hold resizes from the corner.
const CORNER: i32 = 20;

#[derive(Default)]
pub(crate) struct Decor {
    pub wanted: DecorationMode,
    pub object: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    pub dialog: Option<xdg_dialog_v1::XdgDialogV1>,
    /// What the core was told.
    told: Option<Decorations>,
    pub decorations: Decorations,
    pub maximized: bool,
    /// Against an edge of the screen or another window, where resizing makes no sense.
    pub tiled: bool,
    pub pending: Option<(bool, bool)>,
    borders: Option<Borders>,
}

/// Transparent surfaces around the window that resize it.
struct Borders {
    parts: Vec<Part>,
    buffer: wl_buffer::WlBuffer,
    /// What they were placed for.
    placed: Option<(i32, i32)>,
}

struct Part {
    surface: wl_surface::WlSurface,
    sub: wl_subsurface::WlSubsurface,
    viewport: wp_viewport::WpViewport,
}

/// Which side of the window a part of the borders is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Side(usize);

const TOP: usize = 0;
const BOTTOM: usize = 1;
const LEFT: usize = 2;
// The fourth is on the right.

impl State {
    /// What was agreed with the compositor; tells the core when it changes.
    pub(crate) fn decorations_agreed(&mut self, decorations: Decorations) {
        self.decor.decorations = decorations;
        if self.decor.told != Some(decorations) {
            self.decor.told = Some(decorations);
            self.events.push(WindowEvent::Decorations(decorations));
        }
        self.update_borders();
    }

    /// The states of an `xdg_toplevel.configure`, which apply with the `xdg_surface.configure` after
    /// it.
    pub(crate) fn toplevel_states(&mut self, states: &[u8]) {
        let states: Vec<u32> = states
            .chunks_exact(4)
            .map(|s| u32::from_ne_bytes(s.try_into().unwrap()))
            .collect();
        let has = |s: xdg_toplevel::State| states.contains(&(s as u32));
        let maximized = has(xdg_toplevel::State::Maximized);
        let tiled = [
            xdg_toplevel::State::TiledLeft,
            xdg_toplevel::State::TiledRight,
            xdg_toplevel::State::TiledTop,
            xdg_toplevel::State::TiledBottom,
        ]
        .into_iter()
        .any(has);
        self.decor.pending = Some((maximized, tiled));
    }

    pub(crate) fn apply_toplevel_states(&mut self) {
        let Some((maximized, tiled)) = self.decor.pending.take() else {
            return;
        };
        self.decor.tiled = tiled;
        if maximized != self.decor.maximized {
            self.decor.maximized = maximized;
            self.events.push(WindowEvent::Maximized(maximized));
        }
        self.update_borders();
    }

    pub(crate) fn start_move(&mut self) {
        if let (Some(seat), Some(toplevel)) = (&self.seat, self.toplevel()) {
            toplevel._move(seat, self.press_serial);
        }
    }

    pub(crate) fn start_resize(&mut self, edge: ResizeEdge) {
        if let (Some(seat), Some(toplevel)) = (&self.seat, self.toplevel()) {
            toplevel.resize(seat, self.press_serial, resize_edge(edge));
        }
    }

    pub(crate) fn show_window_menu(&mut self) {
        if let (Some(seat), Some(toplevel)) = (&self.seat, self.toplevel()) {
            let (x, y) = match self.pointer_surface {
                anti_webui_core::platform::SurfaceId::Window => self.pointer_pos,
                _ => (0.0, 0.0),
            };
            toplevel.show_window_menu(seat, self.press_serial, x as i32, y as i32);
        }
    }

    /// Whether the window needs edges of its own to resize it from.
    fn wants_borders(&self) -> bool {
        self.decor.decorations == Decorations::Client
            && !self.decor.maximized
            && !self.decor.tiled
            && !self.is_fullscreen()
    }

    /// Puts the edges up or takes them down, and places them around the window's size.
    pub(crate) fn update_borders(&mut self) {
        if !self.wants_borders() {
            if let Some(b) = self.decor.borders.take() {
                for part in b.parts {
                    part.viewport.destroy();
                    part.sub.destroy();
                    part.surface.destroy();
                }
                b.buffer.destroy();
            }
            return;
        }
        if self.decor.borders.is_none() {
            self.decor.borders = self.make_borders();
        }
        let (w, h) = self.logical_size;
        let Some(b) = &mut self.decor.borders else {
            return;
        };
        if b.placed == Some((w, h)) {
            return;
        }
        b.placed = Some((w, h));
        // Top and bottom take the corners in.
        let places = [
            (-BORDER, -BORDER, w + 2 * BORDER, BORDER),
            (-BORDER, h, w + 2 * BORDER, BORDER),
            (-BORDER, 0, BORDER, h),
            (w, 0, BORDER, h),
        ];
        for (part, (x, y, pw, ph)) in b.parts.iter().zip(places) {
            part.sub.set_position(x, y);
            part.viewport.set_destination(pw.max(1), ph.max(1));
            part.surface.commit();
        }
    }

    fn make_borders(&mut self) -> Option<Borders> {
        let (sub_mgr, viewporter, shm) = (
            self.subcompositor.as_ref()?,
            self.viewporter.as_ref()?,
            self.shm.as_ref()?,
        );
        // One transparent pixel, stretched over each edge.
        let fd =
            rustix::fs::memfd_create("anti-webui-border", rustix::fs::MemfdFlags::CLOEXEC).ok()?;
        rustix::fs::ftruncate(&fd, 4).ok()?;
        let pool = shm.create_pool(fd.as_fd(), 4, &self.qh, ());
        let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        let parts = (0..4)
            .map(|i| {
                let surface = self.compositor.create_surface(&self.qh, Side(i));
                let sub = sub_mgr.get_subsurface(&surface, &self.surface, &self.qh, ());
                sub.place_below(&self.surface);
                let viewport = viewporter.get_viewport(&surface, &self.qh, ());
                surface.attach(Some(&buffer), 0, 0);
                surface.damage_buffer(0, 0, 1, 1);
                Part {
                    surface,
                    sub,
                    viewport,
                }
            })
            .collect();
        Some(Borders {
            parts,
            buffer,
            placed: None,
        })
    }

    /// Which of the edges a surface is, if it is one of this window's.
    pub(crate) fn border_side(&self, surface: &wl_surface::WlSurface) -> Option<Side> {
        let b = self.decor.borders.as_ref()?;
        b.parts.iter().position(|p| &p.surface == surface).map(Side)
    }

    /// Where on an edge the pointer is: what it resizes from there.
    pub(crate) fn border_edge(&self, side: Side, x: f64, y: f64) -> ResizeEdge {
        let (w, h) = self.logical_size;
        let (x, y) = (x as i32, y as i32);
        let along_top = |x: i32| {
            if x < CORNER {
                0
            } else if x > w + 2 * BORDER - CORNER {
                2
            } else {
                1
            }
        };
        let along_side = |y: i32| {
            if y < CORNER - BORDER {
                0
            } else if y > h - (CORNER - BORDER) {
                2
            } else {
                1
            }
        };
        match side.0 {
            TOP => [ResizeEdge::TopLeft, ResizeEdge::Top, ResizeEdge::TopRight][along_top(x)],
            BOTTOM => [
                ResizeEdge::BottomLeft,
                ResizeEdge::Bottom,
                ResizeEdge::BottomRight,
            ][along_top(x)],
            LEFT => [
                ResizeEdge::TopLeft,
                ResizeEdge::Left,
                ResizeEdge::BottomLeft,
            ][along_side(y)],
            _ => [
                ResizeEdge::TopRight,
                ResizeEdge::Right,
                ResizeEdge::BottomRight,
            ][along_side(y)],
        }
    }

    /// The pointer is on an edge: shows what pressing does.
    pub(crate) fn border_cursor(&self, edge: ResizeEdge) {
        let Some(dev) = &self.cursor_device else {
            return;
        };
        let shape = match edge {
            ResizeEdge::Top => Shape::NResize,
            ResizeEdge::Bottom => Shape::SResize,
            ResizeEdge::Left => Shape::WResize,
            ResizeEdge::Right => Shape::EResize,
            ResizeEdge::TopLeft => Shape::NwResize,
            ResizeEdge::TopRight => Shape::NeResize,
            ResizeEdge::BottomLeft => Shape::SwResize,
            ResizeEdge::BottomRight => Shape::SeResize,
        };
        dev.set_shape(self.pointer_serial, shape);
    }

    /// Handles what the pointer does on the edges, which the core never hears of. Whether the
    /// event was that.
    pub(crate) fn pointer_on_border(&mut self, event: &wl_pointer::Event) -> bool {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                let Some(side) = self.border_side(surface) else {
                    self.pointer_border = None;
                    return false;
                };
                self.pointer_serial = *serial;
                let edge = self.border_edge(side, *surface_x, *surface_y);
                self.pointer_border = Some((side, edge));
                self.border_cursor(edge);
                true
            }
            wl_pointer::Event::Leave { .. } => self.pointer_border.take().is_some(),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let Some((side, edge)) = self.pointer_border else {
                    return false;
                };
                let now = self.border_edge(side, *surface_x, *surface_y);
                if now != edge {
                    self.pointer_border = Some((side, now));
                    self.border_cursor(now);
                }
                true
            }
            wl_pointer::Event::Button { serial, state, .. } => {
                let Some((_, edge)) = self.pointer_border else {
                    return false;
                };
                if matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed)) {
                    self.press_serial = *serial;
                    self.start_resize(edge);
                }
                true
            }
            _ => self.pointer_border.is_some(),
        }
    }

    /// Lets go of what the window has on the compositor, before its queue goes.
    pub(crate) fn destroy(&mut self) {
        let popups: Vec<_> = self.popups.keys().copied().collect();
        for id in popups {
            self.close_popup(id);
        }
        self.close_drag_icon();
        self.decor.maximized = false;
        self.decor.decorations = Decorations::Server;
        self.update_borders();
        if let Some(d) = self.decor.dialog.take() {
            d.destroy();
        }
        if let Some(d) = self.decor.object.take() {
            d.destroy();
        }
        if let Some(t) = self.text_input.take() {
            t.destroy();
        }
        if let Some(p) = self.pinch_gesture.take() {
            p.destroy();
        }
        if let Some(h) = self.hold_gesture.take() {
            h.destroy();
        }
        if let Some(d) = self.cursor_device.take() {
            d.destroy();
        }
        if let Some(p) = self.pointer.take() {
            p.release();
        }
        if let Some(k) = self.keyboard.take() {
            k.release();
        }
        if let Some(d) = self.data_device.take() {
            d.release();
        }
        if let Some(d) = self.primary_device.take() {
            d.destroy();
        }
        if let Some(f) = self.present.feedback_object.take() {
            f.destroy();
        }
        // The compositor keeps the memory of a buffer until it is destroyed.
        for (_, buffer) in self.present.buffers.drain() {
            buffer.destroy();
        }
        for (_, timeline) in self.present.timelines.drain() {
            timeline.destroy();
        }
        if let Some(s) = self.window_frame.syncobj.take() {
            s.destroy();
        }
        if let Some(f) = self.fractional.take() {
            f.destroy();
        }
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        match &self.role {
            crate::state::SurfaceRole::Toplevel {
                xdg_surface,
                toplevel,
            } => {
                toplevel.destroy();
                xdg_surface.destroy();
            }
            crate::state::SurfaceRole::Layer(l) => l.object.destroy(),
            crate::state::SurfaceRole::Lock(l) => l.destroy(),
        }
        if let Some(outputs) = self.outputs.take() {
            outputs.release();
        }
        self.surface.destroy();
        self.closed = true;
    }
}

fn resize_edge(edge: ResizeEdge) -> xdg_toplevel::ResizeEdge {
    match edge {
        ResizeEdge::Top => xdg_toplevel::ResizeEdge::Top,
        ResizeEdge::Bottom => xdg_toplevel::ResizeEdge::Bottom,
        ResizeEdge::Left => xdg_toplevel::ResizeEdge::Left,
        ResizeEdge::Right => xdg_toplevel::ResizeEdge::Right,
        ResizeEdge::TopLeft => xdg_toplevel::ResizeEdge::TopLeft,
        ResizeEdge::TopRight => xdg_toplevel::ResizeEdge::TopRight,
        ResizeEdge::BottomLeft => xdg_toplevel::ResizeEdge::BottomLeft,
        ResizeEdge::BottomRight => xdg_toplevel::ResizeEdge::BottomRight,
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            let decorations = match mode {
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) => Decorations::Server,
                _ => Decorations::Client,
            };
            state.decorations_agreed(decorations);
        }
    }
}

impl Dispatch<wl_surface::WlSurface, Side> for State {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &Side,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
delegate_noop!(State: ignore xdg_dialog_v1::XdgDialogV1);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
