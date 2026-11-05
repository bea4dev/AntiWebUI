//! The Wayland backend (see `docs/design/06-platform-wayland.md`).

mod drag;
mod keyboard;
mod popup;
mod present;
mod state;

pub use present::{
    BufferEvent, BufferId, DmabufFeedback, DmabufPlaneDesc, DmabufTranche, Presentation, Shown,
    SyncPoints, TimelineId,
};

use anti_webui_core::event::{ClipboardKind, DragAction, WindowEvent};
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, DragOffer, ImeState, PopupId, PopupParams};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use rustix::event::{PollFd, PollFlags};
use state::State;
pub use state::SurfaceId;
use std::os::fd::{AsFd, BorrowedFd};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Instant;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_compositor, wl_seat, wl_subcompositor};
use wayland_client::{Connection, Proxy};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1;
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_manager_v1;
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1;
use wayland_protocols::wp::presentation_time::client::wp_presentation;
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
    /// The compositor is ready for a frame on every surface.
    fn frame(&mut self, ctx: &mut Ctx<'_>);
    /// The next time [`WaylandHandler::timer`] should run.
    fn deadline(&mut self, now: Instant) -> Option<Instant>;
    fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant);
    /// Called once before the connection closes; release GPU surfaces here.
    fn exit(&mut self) {}
    /// The compositor is ready for a commit on every surface: the time to commit what was held
    /// back.
    fn surfaces_ready(&mut self, _ctx: &mut Ctx<'_>) {}
    /// Whether a frame can start now even though the compositor hasn't taken the last one.
    fn may_render_ahead(&mut self, _ctx: &mut Ctx<'_>) -> bool {
        false
    }
    /// Another thread called the function from [`Ctx::waker`].
    fn woken(&mut self, _ctx: &mut Ctx<'_>) {}
}

impl<H: WaylandHandler> WaylandHandler for &mut H {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
        (**self).event(ctx, event)
    }
    fn frame(&mut self, ctx: &mut Ctx<'_>) {
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
    fn surfaces_ready(&mut self, ctx: &mut Ctx<'_>) {
        (**self).surfaces_ready(ctx)
    }
    fn may_render_ahead(&mut self, ctx: &mut Ctx<'_>) -> bool {
        (**self).may_render_ahead(ctx)
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

    /// Requests a redraw after `delay`, for retrying a frame that could not be presented.
    pub fn request_redraw_after(&mut self, delay: std::time::Duration) {
        let t = Instant::now() + delay;
        self.state.redraw_at = Some(self.state.redraw_at.map_or(t, |r| r.min(t)));
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

    /// Something another thread can call to wake up the event loop, which then calls
    /// [`WaylandHandler::woken`].
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.state.wake.try_clone().ok()?;
        Some(move || {
            let _ = rustix::io::write(&wake, &[1]);
        })
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

    /// What dmabufs the compositor accepts, if it supports linux-dmabuf v4.
    pub fn dmabuf_feedback(&self) -> Option<&DmabufFeedback> {
        self.state.present.feedback.as_ref()
    }

    /// Wraps a dmabuf in a `wl_buffer`. Failure is reported later as [`BufferEvent::Failed`].
    pub fn create_dmabuf_buffer(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        planes: &[DmabufPlaneDesc<'_>],
    ) -> Option<BufferId> {
        self.state
            .create_dmabuf_buffer(width, height, format, modifier, planes)
    }

    pub fn destroy_buffer(&mut self, id: BufferId) {
        self.state.destroy_buffer(id);
    }

    pub fn take_buffer_events(&mut self) -> Vec<BufferEvent> {
        std::mem::take(&mut self.state.present.buffer_events)
    }

    /// Asks to be told what becomes of the next commit of the surface, which
    /// [`Ctx::take_presentations`] later returns. `false` if the compositor can't tell.
    pub fn request_presentation(&mut self, surface: SurfaceId, token: u64) -> bool {
        self.state.request_presentation(surface, token)
    }

    pub fn take_presentations(&mut self) -> Vec<Presentation> {
        std::mem::take(&mut self.state.present.presentations)
    }

    /// The clock presentation times are in, as a `clockid_t`.
    pub fn presentation_clock(&self) -> Option<u32> {
        self.state.present.presentation_clock
    }

    /// Whether a commit of the surface is still waiting for its frame callback.
    pub fn frame_pending(&mut self, surface: SurfaceId) -> bool {
        self.state
            .frame_state(surface)
            .is_some_and(|f| f.pending.is_some())
    }

    /// Whether there are popups, which have to keep in step with the window.
    pub fn has_popups(&self) -> bool {
        !self.state.popups.is_empty()
    }

    /// When the last frame callback of the surface came.
    pub fn last_frame_callback(&mut self, surface: SurfaceId) -> Option<Instant> {
        self.state.frame_state(surface)?.last_callback
    }

    /// Whether the compositor supports explicit sync (`linux-drm-syncobj-v1`).
    pub fn explicit_sync_available(&self) -> bool {
        self.state.present.syncobj_manager.is_some()
    }

    /// Shares a DRM syncobj timeline. The first call turns on explicit sync for the surface:
    /// from then on every buffer commit needs [`SyncPoints`].
    pub fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Option<TimelineId> {
        self.state.import_timeline(fd)
    }

    pub fn destroy_timeline(&mut self, id: TimelineId) {
        self.state.destroy_timeline(id);
    }

    pub fn show_popup(&mut self, params: PopupParams) {
        self.state.show_popup(params);
    }

    pub fn update_popup(&mut self, params: PopupParams) {
        self.state.update_popup(params);
    }

    /// Whether drags can be handed to the compositor.
    pub fn supports_drag(&self) -> bool {
        self.state.supports_drag()
    }

    /// Hands what is dragged to the compositor. See `docs/design/23-drag-and-drop.md`.
    pub fn start_drag(&mut self, offer: DragOffer) {
        self.state.start_drag(offer);
    }

    /// Ends what was started with [`Ctx::start_drag`].
    pub fn cancel_drag(&mut self) {
        self.state.cancel_drag();
    }

    /// Sets the size of the surface of a drag, and where the pointer is on it.
    pub fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI) {
        self.state.place_drag_icon(logical, hotspot);
    }

    /// Closes the surface of a drag. GPU surfaces on it must go first.
    pub fn close_drag_icon(&mut self) {
        self.state.close_drag_icon();
    }

    /// Says how what is over the window would be taken.
    pub fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
        self.state.accept_drag(wanted);
    }

    /// Reads what was dropped, which comes as [`WindowEvent::DropData`].
    pub fn receive_drop(&mut self, mime: String) {
        self.state.receive_drop(self.conn, mime);
    }

    /// Ends a drop that didn't need reading.
    pub fn finish_drop(&mut self) {
        self.state.finish_drop();
    }

    /// Closes a popup and what is attached to it. GPU surfaces on them must go first.
    pub fn close_popup(&mut self, id: PopupId) {
        self.state.close_popup(id);
    }

    /// What closes along with a popup.
    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.state.popups_attached_to(id)
    }

    /// Turns explicit sync off again (before presenting through a GPU surface).
    pub fn disable_explicit_sync(&mut self) {
        self.state.disable_explicit_sync();
    }

    /// Attaches the buffer, damages `damage` (buffer pixels) and commits.
    pub fn present_buffer(
        &mut self,
        surface: SurfaceId,
        id: BufferId,
        damage: &[RectI],
        sync: Option<SyncPoints>,
    ) {
        self.state.present_buffer(surface, id, damage, sync);
    }

    /// To call before something else commits to the surface, like a GPU surface.
    pub fn prepare_commit(&mut self, surface: SurfaceId) {
        self.state.prepare_commit(surface);
    }

    /// To call after something else committed to the surface.
    pub fn committed(&mut self, surface: SurfaceId) {
        self.state.committed(surface);
    }

    /// Marks the whole surface opaque or not, so the compositor can skip what is behind it.
    pub fn set_opaque(&mut self, surface: SurfaceId, opaque: bool) {
        if let Some(f) = self.state.frame_state(surface) {
            f.opaque = opaque;
        }
    }

    /// Raw handles for creating a GPU surface. Valid until the surface closes.
    pub fn raw_handles(&self, surface: SurfaceId) -> (RawDisplayHandle, RawWindowHandle) {
        let display =
            NonNull::new(self.conn.backend().display_ptr() as *mut _).expect("wl_display");
        let surface = self.state.wl_surface(surface).expect("an open surface");
        let surface = NonNull::new(surface.id().as_ptr() as *mut _).expect("wl_surface");
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
    let surface = compositor.create_surface(&qh, SurfaceId::Window);
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, SurfaceId::Window);
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
        wm_base,
        surface.clone(),
        xdg_surface,
        toplevel.clone(),
        (options.width, options.height),
        paste_tx,
        wake_write,
    );

    if let Ok(vp) = globals.bind::<wp_viewporter::WpViewporter, _, _>(&qh, 1..=1, ()) {
        state.viewport = Some(vp.get_viewport(&surface, &qh, ()));
        state.viewporter = Some(vp);
    }
    state.subcompositor = globals
        .bind::<wl_subcompositor::WlSubcompositor, _, _>(&qh, 1..=1, ())
        .ok();
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
    state.pointer_gestures = globals
        .bind::<zwp_pointer_gestures_v1::ZwpPointerGesturesV1, _, _>(&qh, 3..=3, ())
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
    if let Ok(dmabuf) = globals.bind::<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _, _>(&qh, 4..=5, ())
    {
        state.present.feedback_object = Some(dmabuf.get_surface_feedback(&surface, &qh, ()));
        state.present.dmabuf = Some(dmabuf);
    }
    state.present.syncobj_manager = globals
        .bind::<wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1, _, _>(&qh, 1..=1, ())
        .ok();
    state.present.presentation = globals
        .bind::<wp_presentation::WpPresentation, _, _>(&qh, 1..=2, ())
        .ok();

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
        let mut exit = false;
        let mut timer_deadline: Option<Instant> = None;
        let mut woken = false;
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
            state.take_dropped_data();

            if state.keyboard_focused != state.focus_announced {
                state.focus_announced = state.keyboard_focused;
                state
                    .events
                    .insert(0, WindowEvent::Focused(state.keyboard_focused));
            }
            let mut events = std::mem::take(&mut state.events);
            // Only the last of consecutive pointer moves matters; high-rate mice send many per
            // frame.
            events.dedup_by(|next, prev| {
                let both = matches!(
                    (&*prev, &*next),
                    (
                        WindowEvent::PointerMoved { .. },
                        WindowEvent::PointerMoved { .. }
                    ) | (WindowEvent::DragMoved { .. }, WindowEvent::DragMoved { .. })
                );
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
            if std::mem::take(&mut woken) {
                let mut ctx = Ctx {
                    state,
                    conn,
                    exit: &mut exit,
                };
                handler.woken(&mut ctx);
            }
            // The deadline we slept until; asking again now would give the one after it.
            if timer_deadline.is_some_and(|d| d <= Instant::now()) {
                timer_deadline = None;
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

            if state.redraw_at.is_some_and(|t| t <= Instant::now()) {
                state.redraw_at = None;
                state.redraw_requested = true;
            }
            if state.configured {
                let ready = state.frame_wait(Instant::now()).is_ok();
                let mut ctx = Ctx {
                    state,
                    conn,
                    exit: &mut exit,
                };
                if ready {
                    handler.surfaces_ready(&mut ctx);
                    // What it committed mustn't wait for the frame that may start below: the
                    // compositor only takes commits up to some point in a refresh.
                    conn.flush().map_err(err)?;
                }
                if ctx.state.redraw_requested
                    && (ctx.state.frame_wait(Instant::now()).is_ok()
                        || handler.may_render_ahead(&mut ctx))
                {
                    ctx.state.redraw_requested = false;
                    handler.frame(&mut ctx);
                }
            }

            conn.flush().map_err(err)?;

            // Nothing may come to wake us up for it.
            if exit {
                break;
            }
            if !state.events.is_empty() {
                continue;
            }
            let Some(guard) = queue.prepare_read() else {
                continue;
            };
            let now = Instant::now();
            timer_deadline = handler.deadline(now);
            let mut deadline = timer_deadline;
            if let Some(r) = &state.kb.repeat {
                deadline = Some(deadline.map_or(r.next, |d| d.min(r.next)));
            }
            if let Some(t) = state.redraw_at {
                deadline = Some(deadline.map_or(t, |d| d.min(t)));
            }
            if state.redraw_requested
                && let Err(Some(t)) = state.frame_wait(now)
            {
                deadline = Some(deadline.map_or(t, |d| d.min(t)));
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
                woken = true;
            }
        }
        Ok(())
    }
}
