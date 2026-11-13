//! The Wayland backend (see `docs/design/06-platform-wayland.md`).

#![cfg(target_os = "linux")]

mod decor;
mod drag;
mod grab;
mod keyboard;
mod layer;
mod lock;
mod main_device;
mod popup;
mod present;
mod state;

pub use anti_webui_core::platform::{
    BufferEvent, BufferId, DmabufFeedback, DmabufPlaneDesc, DmabufTranche, Presentation, Shown,
    SyncPoints, TimelineId,
};

use anti_webui_core::event::{ClipboardKind, DragAction, WindowEvent};
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::layer::{LayerChange, LayerSpec};
use anti_webui_core::platform::{CursorIcon, DragOffer, ImeState, PopupId, PopupParams};
use anti_webui_core::window::{DecorationMode, Decorations, ResizeEdge};
pub use main_device::compositor_main_device;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use rustix::event::{PollFd, PollFlags};
use state::State;
pub use state::SurfaceId;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_compositor, wl_seat, wl_shm, wl_subcompositor};
use wayland_client::{Connection, Proxy};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1;
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
use wayland_protocols::xdg::dialog::v1::client::xdg_wm_dialog_v1;
use wayland_protocols::xdg::shell::client::{xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1;

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
    /// Whether it opens on all of the screen.
    pub fullscreen: bool,
    pub decorations: DecorationMode,
    /// What it stays above.
    pub parent: Option<ParentLink>,
    /// Keeps input from the parent, if the compositor supports `xdg-dialog`.
    pub modal: bool,
    /// Opens a layer surface of a desktop shell instead of a window. `width` and `height` are
    /// what it asks for along the axes that follow what is in it.
    pub layer: Option<LayerSpec>,
    /// Opens a lock surface over the output of this connector name instead of a window, locking
    /// the session until the last one closes.
    pub lock: Option<String>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: String::new(),
            app_id: String::new(),
            width: 640,
            height: 480,
            fullscreen: false,
            decorations: DecorationMode::Auto,
            parent: None,
            modal: false,
            layer: None,
            lock: None,
        }
    }
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
    fn woken(&mut self, ctx: &mut Ctx<'_>) {
        (**self).woken(ctx)
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
        if let Some(t) = self.state.toplevel() {
            t.set_title(title.to_owned());
        }
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        match self.state.toplevel() {
            Some(t) if on => t.set_fullscreen(None),
            Some(t) => t.unset_fullscreen(),
            None => {}
        }
    }

    /// Changes a layer surface. Nothing for a window.
    pub fn change_layer(&mut self, change: LayerChange) {
        self.state.change_layer(change);
    }

    /// The size of what is in a layer surface, along the axes that follow it.
    pub fn set_layer_content_size(&mut self, size: SizeI) {
        self.state.set_layer_content_size(size.width, size.height);
    }

    /// The size of the window in logical pixels.
    pub fn logical_size(&self) -> SizeI {
        SizeI::new(self.state.logical_size.0, self.state.logical_size.1)
    }

    pub fn exit(&mut self) {
        *self.exit = true;
    }

    /// Lets the compositor move the window with the pointer, from the press going on.
    pub fn start_move(&mut self) {
        self.state.start_move();
    }

    /// Lets the compositor resize the window from an edge, from the press going on.
    pub fn start_resize(&mut self, edge: ResizeEdge) {
        self.state.start_resize(edge);
    }

    pub fn minimize(&mut self) {
        if let Some(t) = self.state.toplevel() {
            t.set_minimized();
        }
    }

    pub fn set_maximized(&mut self, on: bool) {
        match self.state.toplevel() {
            Some(t) if on => t.set_maximized(),
            Some(t) => t.unset_maximized(),
            None => {}
        }
    }

    /// The compositor's menu for the window, where the pointer is.
    pub fn show_window_menu(&mut self) {
        self.state.show_window_menu();
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

    /// Whether the surface has to be drawn again in full. Never on Wayland: the compositor keeps
    /// what was shown.
    pub fn take_exposed(&mut self, _surface: SurfaceId) -> bool {
        false
    }

    /// Raw handles for creating a GPU surface. Valid until the surface closes.
    pub fn raw_handles(&self, surface: SurfaceId) -> Option<(RawDisplayHandle, RawWindowHandle)> {
        let display =
            NonNull::new(self.conn.backend().display_ptr() as *mut _).expect("wl_display");
        let surface = self.state.wl_surface(surface).expect("an open surface");
        let surface = NonNull::new(surface.id().as_ptr() as *mut _).expect("wl_surface");
        Some((
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
        ))
    }
}

pub use lock::{LockState, lock_state};

/// Whether a compositor can be reached, for choosing the backend.
pub fn can_connect() -> bool {
    Connection::connect_to_env().is_ok()
}

/// Whether the compositor can be reached and has layer surfaces (`zwlr_layer_shell_v1`).
pub fn supports_layer_shell() -> bool {
    let Ok(conn) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, _)) = registry_queue_init::<State>(&conn) else {
        return false;
    };
    globals.contents().with_list(|list| {
        list.iter()
            .any(|g| g.interface == zwlr_layer_shell_v1::ZwlrLayerShellV1::interface().name)
    })
}

/// A window of a [`Session`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey(u64);

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// What a thread needs to open windows on the connection of another (see
/// `docs/design/36-multi-window.md`).
pub struct SessionShare {
    conn: Connection,
    popups: grab::ConnectionPopups,
}

/// A window to open others above, from any thread.
#[derive(Clone)]
pub struct ParentLink(xdg_toplevel::XdgToplevel);

/// Who a session's windows call: the handler of each.
pub trait Handlers {
    type Handler: WaylandHandler;
    fn handler(&mut self, key: WindowKey) -> Option<&mut Self::Handler>;
}

/// One window: its own queue and protocol objects, as if it were alone. Several share a thread's
/// [`Session`], which reads for all of them.
struct Instance {
    key: WindowKey,
    queue: wayland_client::EventQueue<State>,
    state: State,
    paste_rx: mpsc::Receiver<(ClipboardKind, String)>,
    /// The deadline the session slept until; asking again now would give the one after it.
    timer_deadline: Option<Instant>,
    woken: bool,
    exit: bool,
}

/// The windows of a thread, on a connection it may share with other threads.
pub struct Session {
    conn: Connection,
    /// Shared with the sessions that join this one.
    popups: grab::ConnectionPopups,
    wl_fd: OwnedFd,
    wake_read: OwnedFd,
    wake_write: OwnedFd,
    instances: Vec<Instance>,
}

impl Session {
    /// Windows from `spawn_window` get threads of their own.
    pub const OWN_THREADS: bool = true;

    /// Connects to the compositor.
    pub fn connect() -> Result<Self, Error> {
        Self::on(
            Connection::connect_to_env().map_err(err)?,
            grab::ConnectionPopups::default(),
        )
    }

    /// Uses the connection of another thread's session.
    pub fn join(share: SessionShare) -> Result<Self, Error> {
        Self::on(share.conn, share.popups)
    }

    fn on(conn: Connection, popups: grab::ConnectionPopups) -> Result<Self, Error> {
        let (wake_read, wake_write) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )
        .map_err(err)?;
        let wl_fd = conn.as_fd().try_clone_to_owned().map_err(err)?;
        Ok(Self {
            conn,
            popups,
            wl_fd,
            wake_read,
            wake_write,
            instances: Vec::new(),
        })
    }

    pub fn share(&self) -> SessionShare {
        SessionShare {
            conn: self.conn.clone(),
            popups: self.popups.clone(),
        }
    }

    /// Something another thread can call to wake up the session, which then calls
    /// [`WaylandHandler::woken`] of every window.
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.wake_write.try_clone().ok()?;
        Some(move || {
            let _ = rustix::io::write(&wake, &[1]);
        })
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Opens a window. It shows once its handler presents a first frame.
    pub fn open(&mut self, options: WindowOptions) -> Result<WindowKey, Error> {
        let key = WindowKey(NEXT_KEY.fetch_add(1, Ordering::Relaxed));
        let wake = self.wake_write.try_clone().map_err(err)?;
        let instance = open_instance(&self.conn, self.popups.clone(), key, options, wake)?;
        self.instances.push(instance);
        Ok(key)
    }

    /// For opening windows above this one, from any thread.
    pub fn parent_link(&self, key: WindowKey) -> Option<ParentLink> {
        let i = self.instances.iter().find(|i| i.key == key)?;
        Some(ParentLink(i.state.toplevel()?.clone()))
    }

    /// Closes a window. GPU surfaces on it must go first.
    pub fn close(&mut self, key: WindowKey) {
        if let Some(at) = self.instances.iter().position(|i| i.key == key) {
            let mut instance = self.instances.remove(at);
            instance.state.destroy();
            let _ = instance.queue.dispatch_pending(&mut instance.state);
            let _ = self.conn.flush();
        }
    }

    /// Draws a window soon, as when something on another window of the thread changed it.
    pub fn request_redraw(&mut self, key: WindowKey) {
        if let Some(i) = self.instances.iter_mut().find(|i| i.key == key) {
            let now = Instant::now();
            i.state.redraw_at = Some(i.state.redraw_at.map_or(now, |r| r.min(now)));
        }
    }

    /// Whether a window is already going to be drawn.
    pub fn redraw_pending(&self, key: WindowKey) -> bool {
        self.instances
            .iter()
            .find(|i| i.key == key)
            .is_some_and(|i| i.state.redraw_requested || i.state.redraw_at.is_some())
    }

    /// The windows whose handlers asked to close them, through [`Ctx::exit`].
    pub fn exited(&self) -> Vec<WindowKey> {
        self.instances
            .iter()
            .filter(|i| i.exit || i.state.closed)
            .map(|i| i.key)
            .collect()
    }

    /// Hands every window what came for it, and draws those that are due. Doesn't wait.
    pub fn dispatch<M: Handlers>(&mut self, handlers: &mut M) -> Result<(), Error> {
        for instance in &mut self.instances {
            if instance.exit {
                continue;
            }
            let Some(handler) = handlers.handler(instance.key) else {
                continue;
            };
            instance.turn(&self.conn, handler)?;
        }
        self.conn.flush().map_err(err)
    }

    /// Waits until something comes for a window, or one of them has something to do.
    pub fn wait<M: Handlers>(&mut self, handlers: &mut M) -> Result<(), Error> {
        self.wait_until(handlers, None)
    }

    /// As [`Session::wait`], but waking by `until` at the latest.
    pub fn wait_until<M: Handlers>(
        &mut self,
        handlers: &mut M,
        until: Option<Instant>,
    ) -> Result<(), Error> {
        let busy = |instances: &[Instance]| {
            instances
                .iter()
                .any(|i| i.exit || !i.state.events.is_empty())
        };
        if busy(&self.instances) {
            return Ok(());
        }
        for i in &mut self.instances {
            i.queue.dispatch_pending(&mut i.state).map_err(err)?;
        }
        if busy(&self.instances) {
            return Ok(());
        }
        let Some(guard) = self.conn.prepare_read() else {
            return Ok(());
        };
        // Another thread on the connection may have read for these queues since: from here on it
        // waits for this one to read too.
        let mut more = 0;
        for i in &mut self.instances {
            more += i.queue.dispatch_pending(&mut i.state).map_err(err)?;
        }
        if more > 0 || busy(&self.instances) {
            return Ok(());
        }
        let now = Instant::now();
        let mut deadline: Option<Instant> = None;
        let mut earlier = |t: Instant| deadline = Some(deadline.map_or(t, |d| d.min(t)));
        if let Some(t) = until {
            earlier(t);
        }
        for i in &mut self.instances {
            i.timer_deadline = handlers.handler(i.key).and_then(|h| h.deadline(now));
            if let Some(t) = i.timer_deadline {
                earlier(t);
            }
            if let Some(r) = &i.state.kb.repeat {
                earlier(r.next);
            }
            if let Some(t) = i.state.redraw_at {
                earlier(t);
            }
            if i.state.redraw_requested
                && let Err(Some(t)) = i.state.frame_wait(now)
            {
                earlier(t);
            }
        }
        let timeout = deadline.map(|d| {
            let dur = d.saturating_duration_since(now);
            rustix::time::Timespec {
                tv_sec: dur.as_secs() as _,
                tv_nsec: dur.subsec_nanos() as _,
            }
        });
        let mut fds = [
            PollFd::new(&self.wl_fd, PollFlags::IN),
            PollFd::new(&self.wake_read, PollFlags::IN),
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
            while matches!(rustix::io::read(&self.wake_read, &mut buf), Ok(n) if n > 0) {}
            for i in &mut self.instances {
                i.woken = true;
            }
        }
        Ok(())
    }
}

fn open_instance(
    conn: &Connection,
    popups: grab::ConnectionPopups,
    key: WindowKey,
    options: WindowOptions,
    wake: OwnedFd,
) -> Result<Instance, Error> {
    let (globals, mut queue) = registry_queue_init::<State>(conn).map_err(err)?;
    let qh = queue.handle();

    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(err)?;
    let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).map_err(err)?;
    let surface = compositor.create_surface(&qh, SurfaceId::Window);
    let size = (options.width, options.height);
    let mut outputs = None;
    let role = match options.layer.clone() {
        None if options.lock.is_some() => {
            let name = options.lock.as_deref().unwrap_or_default();
            let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 = globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| Error("the compositor can't lock the session".into()))?;
            let bound = layer::Outputs::bind(conn, &globals)?;
            let output = bound
                .find(name)
                .cloned()
                .ok_or_else(|| Error(format!("no output named {name} to lock")))?;
            outputs = Some(bound);
            state::SurfaceRole::Lock(lock::LockRole::create(&manager, &surface, &output, &qh))
        }
        Some(spec) => {
            let shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals
                .bind(&qh, 1..=5, ())
                .map_err(|_| Error("the compositor has no layer shell".into()))?;
            let output = match &spec.output {
                Some(name) => {
                    let bound = layer::Outputs::bind(conn, &globals)?;
                    let found = bound.find(name).cloned();
                    if found.is_none() {
                        eprintln!("[anti-webui] no output named {name}; the compositor chooses");
                    }
                    outputs = Some(bound);
                    found
                }
                None => None,
            };
            state::SurfaceRole::Layer(layer::LayerRole::create(
                &shell,
                &surface,
                output.as_ref(),
                spec,
                size,
                &qh,
            ))
        }
        None => {
            let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, SurfaceId::Window);
            let toplevel = xdg_surface.get_toplevel(&qh, ());
            toplevel.set_title(options.title.clone());
            toplevel.set_app_id(options.app_id.clone());
            if let Some(parent) = &options.parent {
                toplevel.set_parent(Some(&parent.0));
            }
            if options.fullscreen {
                toplevel.set_fullscreen(None);
            }
            state::SurfaceRole::Toplevel {
                xdg_surface,
                toplevel,
            }
        }
    };

    let (paste_tx, paste_rx) = mpsc::channel();
    let mailbox = grab::Mailbox::new(wake.try_clone().map_err(err)?);
    let mut state = State::new(
        qh.clone(),
        compositor,
        wm_base,
        surface.clone(),
        role,
        size,
        paste_tx,
        wake,
        popups,
        mailbox,
    );
    state.outputs = outputs;
    state.apply_input_region();

    if let Ok(vp) = globals.bind::<wp_viewporter::WpViewporter, _, _>(&qh, 1..=1, ()) {
        state.viewport = Some(vp.get_viewport(&surface, &qh, ()));
        state.viewporter = Some(vp);
    }
    state.subcompositor = globals
        .bind::<wl_subcompositor::WlSubcompositor, _, _>(&qh, 1..=1, ())
        .ok();
    state.shm = globals.bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ()).ok();
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
    state.decor.wanted = options.decorations;
    let toplevel = state.toplevel().cloned();
    if let Some(toplevel) = &toplevel
        && let Ok(mgr) = globals.bind::<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _, _>(
            &qh,
            1..=1,
            (),
        )
    {
        let deco = mgr.get_toplevel_decoration(toplevel, &qh, ());
        deco.set_mode(match options.decorations {
            DecorationMode::Client => zxdg_toplevel_decoration_v1::Mode::ClientSide,
            DecorationMode::Auto | DecorationMode::Server => {
                zxdg_toplevel_decoration_v1::Mode::ServerSide
            }
        });
        state.decor.object = Some(deco);
    } else if toplevel.is_some() {
        // Nobody draws a frame for it.
        state.decorations_agreed(Decorations::Client);
    }
    if let Some(toplevel) = &toplevel
        && options.modal
        && options.parent.is_some()
        && let Ok(mgr) = globals.bind::<xdg_wm_dialog_v1::XdgWmDialogV1, _, _>(&qh, 1..=1, ())
    {
        let dialog = mgr.get_xdg_dialog(toplevel, &qh, ());
        dialog.set_modal();
        state.decor.dialog = Some(dialog);
    }
    state.cursor_shape_manager = globals
        .bind::<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, _, _>(&qh, 1..=1, ())
        .ok();
    state.text_input_manager = globals
        .bind::<zwp_text_input_manager_v3::ZwpTextInputManagerV3, _, _>(&qh, 1..=1, ())
        .ok();
    state.pointer_gestures = globals
        .bind::<zwp_pointer_gestures_v1::ZwpPointerGesturesV1, _, _>(&qh, 1..=3, ())
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

    // xdg and layer surfaces commit once to be configured. A lock surface is configured already
    // and may not commit before it acks that: compositors end the connection for it.
    if !matches!(state.role, state::SurfaceRole::Lock(_)) {
        surface.commit();
    }
    queue.roundtrip(&mut state).map_err(err)?;
    Ok(Instance {
        key,
        queue,
        state,
        paste_rx,
        timer_deadline: None,
        woken: false,
        exit: false,
    })
}

/// Opens a window alone and runs until its handler exits.
pub fn run<H: WaylandHandler>(options: WindowOptions, handler: H) -> Result<(), Error> {
    struct One<H>(WindowKey, H);
    impl<H: WaylandHandler> Handlers for One<H> {
        type Handler = H;
        fn handler(&mut self, key: WindowKey) -> Option<&mut H> {
            (key == self.0).then_some(&mut self.1)
        }
    }
    let mut session = Session::connect()?;
    let key = session.open(options)?;
    let mut one = One(key, handler);
    let result = (|| loop {
        session.dispatch(&mut one)?;
        if !session.exited().is_empty() {
            return Ok(());
        }
        session.wait(&mut one)?;
    })();
    one.1.exit();
    session.close(key);
    result
}

impl Instance {
    /// What one pass of the event loop does for the window.
    fn turn<H: WaylandHandler>(&mut self, conn: &Connection, handler: &mut H) -> Result<(), Error> {
        let Instance {
            queue,
            state,
            paste_rx,
            timer_deadline,
            woken,
            exit,
            ..
        } = self;
        queue.dispatch_pending(state).map_err(err)?;
        state.take_ended_popups();

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
        // Only the last of consecutive pointer moves matters; high-rate mice send many per frame.
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
            let mut ctx = Ctx { state, conn, exit };
            handler.event(&mut ctx, ev);
        }
        if std::mem::take(woken) {
            let mut ctx = Ctx { state, conn, exit };
            handler.woken(&mut ctx);
        }
        if timer_deadline.is_some_and(|d| d <= Instant::now()) {
            *timer_deadline = None;
            let mut ctx = Ctx { state, conn, exit };
            handler.timer(&mut ctx, Instant::now());
        }
        if *exit || state.closed {
            return Ok(());
        }

        if state.redraw_at.is_some_and(|t| t <= Instant::now()) {
            state.redraw_at = None;
            state.redraw_requested = true;
        }
        if state.configured {
            let ready = state.frame_wait(Instant::now()).is_ok();
            let mut ctx = Ctx { state, conn, exit };
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
        conn.flush().map_err(err)
    }
}
