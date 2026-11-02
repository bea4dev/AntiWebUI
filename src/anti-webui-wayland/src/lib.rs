//! The Wayland backend (see `docs/design/06-platform-wayland.md`).

mod keyboard;
mod state;

use anti_webui_core::event::{ClipboardKind, WindowEvent};
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, ImeState};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use rustix::event::{PollFd, PollFlags};
use state::State;
use std::os::fd::AsFd;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Instant;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_compositor, wl_seat};
use wayland_client::{Connection, Proxy};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3;
use wayland_protocols::wp::viewporter::client::wp_viewporter;
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::xdg_wm_base;

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}

pub struct WindowOptions {
    pub title: String,
    pub app_id: String,
    /// Initial size in logical pixels.
    pub width: i32,
    pub height: i32,
}

/// Callbacks from the event loop.
pub trait WaylandHandler {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent);
    /// The compositor is ready for a frame. Returns whether a buffer was presented.
    fn frame(&mut self, ctx: &mut Ctx<'_>) -> bool;
    /// The next time [`WaylandHandler::timer`] should run.
    fn deadline(&mut self, now: Instant) -> Option<Instant>;
    fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant);
    /// Called once before the connection closes; release GPU surfaces here.
    fn exit(&mut self) {}
}

impl<H: WaylandHandler> WaylandHandler for &mut H {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
        (**self).event(ctx, event)
    }
    fn frame(&mut self, ctx: &mut Ctx<'_>) -> bool {
        (**self).frame(ctx)
    }
    fn deadline(&mut self, now: Instant) -> Option<Instant> {
        (**self).deadline(now)
    }
    fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
        (**self).timer(ctx, now)
    }
    fn exit(&mut self) {
        (**self).exit()
    }
}

/// What handlers may ask of the platform.
pub struct Ctx<'a> {
    state: &'a mut State,
    conn: &'a Connection,
    exit: &'a mut bool,
}

impl Ctx<'_> {
    pub fn request_redraw(&mut self) {
        self.state.redraw_requested = true;
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.state.set_cursor(icon);
    }

    pub fn set_ime(&mut self, ime: Option<ImeState>) {
        self.state.ime_wanted = ime;
        self.state.apply_ime();
    }

    pub fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
        self.state.set_clipboard(kind, text);
    }

    pub fn request_paste(&mut self, kind: ClipboardKind) {
        self.state.request_paste(self.conn, kind);
    }

    pub fn set_title(&mut self, title: &str) {
        self.state.toplevel.set_title(title.to_owned());
    }

    pub fn exit(&mut self) {
        *self.exit = true;
    }

    pub fn buffer_size(&self) -> SizeI {
        self.state.buffer_size
    }

    pub fn scale(&self) -> Scale {
        self.state.scale
    }

    /// Raw handles for creating a GPU surface. Valid until the event loop returns.
    pub fn raw_handles(&self) -> (RawDisplayHandle, RawWindowHandle) {
        let display =
            NonNull::new(self.conn.backend().display_ptr() as *mut _).expect("wl_display");
        let surface = NonNull::new(self.state.surface.id().as_ptr() as *mut _).expect("wl_surface");
        (
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
        )
    }
}

/// Opens a window and runs the event loop until the handler exits.
pub fn run<H: WaylandHandler>(options: WindowOptions, mut handler: H) -> Result<(), Error> {
    let conn = Connection::connect_to_env().map_err(err)?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(err)?;
    let qh = queue.handle();

    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(err)?;
    let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).map_err(err)?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title(options.title.clone());
    toplevel.set_app_id(options.app_id.clone());

    let (paste_tx, paste_rx) = mpsc::channel();
    let (wake_read, wake_write) = rustix::pipe::pipe_with(
        rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
    )
    .map_err(err)?;
    let mut state = State::new(
        qh.clone(),
        compositor,
        surface.clone(),
        xdg_surface,
        toplevel.clone(),
        (options.width, options.height),
        paste_tx,
        wake_write,
    );

    if let Ok(vp) = globals.bind::<wp_viewporter::WpViewporter, _, _>(&qh, 1..=1, ()) {
        state.viewport = Some(vp.get_viewport(&surface, &qh, ()));
    }
    if state.viewport.is_some()
        && let Ok(mgr) = globals
            .bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(
                &qh,
                1..=1,
                (),
            )
    {
        state.fractional = Some(mgr.get_fractional_scale(&surface, &qh, ()));
    }
    if let Ok(mgr) =
        globals.bind::<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _, _>(&qh, 1..=1, ())
    {
        let deco = mgr.get_toplevel_decoration(&toplevel, &qh, ());
        deco.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        state._decoration = Some(deco);
    }
    state.cursor_shape_manager = globals
        .bind::<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, _, _>(&qh, 1..=1, ())
        .ok();
    state.text_input_manager = globals
        .bind::<zwp_text_input_manager_v3::ZwpTextInputManagerV3, _, _>(&qh, 1..=1, ())
        .ok();
    state.data_device_manager = globals.bind(&qh, 1..=3, ()).ok();
    state.primary_manager = globals
        .bind::<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, _, _>(
            &qh,
            1..=1,
            (),
        )
        .ok();
    let _seat: wl_seat::WlSeat = globals.bind(&qh, 1..=8, ()).map_err(err)?;

    surface.commit();
    queue.roundtrip(&mut state).map_err(err)?;

    let wl_fd = conn.as_fd().try_clone_to_owned().map_err(err)?;
    let mut lp = Loop {
        conn: &conn,
        queue: &mut queue,
        state: &mut state,
        paste_rx: &paste_rx,
        wl_fd: &wl_fd,
        wake_read: &wake_read,
    };
    let result = lp.run(&mut handler);
    handler.exit();
    result
}

struct Loop<'a> {
    conn: &'a Connection,
    queue: &'a mut wayland_client::EventQueue<State>,
    state: &'a mut State,
    paste_rx: &'a mpsc::Receiver<(ClipboardKind, String)>,
    wl_fd: &'a std::os::fd::OwnedFd,
    wake_read: &'a std::os::fd::OwnedFd,
}

impl Loop<'_> {
    fn run<H: WaylandHandler>(&mut self, handler: &mut H) -> Result<(), Error> {
        let Loop {
            conn,
            queue,
            state,
            paste_rx,
            wl_fd,
            wake_read,
        } = self;
        let (conn, paste_rx, wl_fd, wake_read) = (*conn, *paste_rx, *wl_fd, *wake_read);
        let qh = queue.handle();
        let mut exit = false;
        loop {
            queue.dispatch_pending(*state).map_err(err)?;

            let now = Instant::now();
            while let Some(keycode) = state.key_repeat_due(now) {
                if let Some(ev) = state.kb.key_event(keycode, true, true) {
                    state.events.push(WindowEvent::Key(ev));
                }
            }
            while let Ok((kind, text)) = paste_rx.try_recv() {
                state.events.push(WindowEvent::Paste { kind, text });
            }

            let mut events = std::mem::take(&mut state.events);
            // Only the last of consecutive pointer moves matters; high-rate mice send many per
            // frame.
            events.dedup_by(|next, prev| {
                let both = matches!(prev, WindowEvent::PointerMoved { .. })
                    && matches!(next, WindowEvent::PointerMoved { .. });
                if both {
                    *prev = next.clone();
                }
                both
            });
            for ev in events {
                let mut ctx = Ctx {
                    state,
                    conn,
                    exit: &mut exit,
                };
                handler.event(&mut ctx, ev);
            }
            if let Some(d) = handler.deadline(now)
                && d <= Instant::now()
            {
                let mut ctx = Ctx {
                    state,
                    conn,
                    exit: &mut exit,
                };
                handler.timer(&mut ctx, Instant::now());
            }
            if exit || state.closed {
                break;
            }

            if state.configured && state.redraw_requested && !state.frame_pending {
                state.redraw_requested = false;
                state.surface.frame(&qh, ());
                state.frame_pending = true;
                let mut ctx = Ctx {
                    state,
                    conn,
                    exit: &mut exit,
                };
                if !handler.frame(&mut ctx) {
                    // Commit anyway so the frame callback fires.
                    state.surface.commit();
                }
            }

            conn.flush().map_err(err)?;

            if !state.events.is_empty() {
                continue;
            }
            let Some(guard) = queue.prepare_read() else {
                continue;
            };
            let now = Instant::now();
            let mut deadline = handler.deadline(now);
            if let Some(r) = &state.kb.repeat {
                deadline = Some(deadline.map_or(r.next, |d| d.min(r.next)));
            }
            let timeout = deadline.map(|d| {
                let dur = d.saturating_duration_since(now);
                rustix::time::Timespec {
                    tv_sec: dur.as_secs() as _,
                    tv_nsec: dur.subsec_nanos() as _,
                }
            });
            let mut fds = [
                PollFd::new(wl_fd, PollFlags::IN),
                PollFd::new(wake_read, PollFlags::IN),
            ];
            match rustix::event::poll(&mut fds, timeout.as_ref()) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(err(e)),
            }
            let wl_ready = fds[0].revents().contains(PollFlags::IN);
            let wake_ready = fds[1].revents().contains(PollFlags::IN);
            if wl_ready {
                if let Err(e) = guard.read()
                    && !matches!(e, wayland_client::backend::WaylandError::Io(ref io) if io.kind() == std::io::ErrorKind::WouldBlock)
                {
                    return Err(err(e));
                }
            } else {
                drop(guard);
            }
            if wake_ready {
                let mut buf = [0u8; 64];
                while matches!(rustix::io::read(wake_read, &mut buf), Ok(n) if n > 0) {}
            }
        }
        Ok(())
    }
}
