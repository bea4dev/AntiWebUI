//! The X11 backend (see `docs/design/35-platform-x11.md`).

mod atoms;
mod cursor;
mod ime;
mod input;
mod keyboard;
mod popup;
mod present;
mod selection;
mod settings;
mod state;
mod xdnd;

pub use present::server_main_device;

use anti_webui_core::event::{ClipboardKind, DragAction, WindowEvent};
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{
    BufferEvent, BufferId, CursorIcon, DmabufFeedback, DmabufPlaneDesc, DragOffer, ImeState,
    PopupId, PopupParams, Presentation, SurfaceId, SyncPoints, TimelineId,
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use rustix::event::{PollFd, PollFlags};
use state::State;
use std::num::NonZeroU32;
use std::os::fd::BorrowedFd;
use std::ptr::NonNull;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;

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
}

/// Callbacks from the event loop: the same as the Wayland backend's.
pub trait X11Handler {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent);
    /// The server took the last frame of every window: the time for the next.
    fn frame(&mut self, ctx: &mut Ctx<'_>);
    /// The next time [`X11Handler::timer`] should run.
    fn deadline(&mut self, now: Instant) -> Option<Instant>;
    fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant);
    /// Called once before the connection closes; release GPU surfaces here.
    fn exit(&mut self) {}
    /// The server took the last frame of every window: the time to present what was held back.
    fn surfaces_ready(&mut self, _ctx: &mut Ctx<'_>) {}
    /// Whether a frame can start now even though the server hasn't taken the last one.
    fn may_render_ahead(&mut self, _ctx: &mut Ctx<'_>) -> bool {
        false
    }
    /// Another thread called the function from [`Ctx::waker`].
    fn woken(&mut self, _ctx: &mut Ctx<'_>) {}
}

impl<H: X11Handler> X11Handler for &mut H {
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

/// What handlers may ask of the platform. The methods are those of the Wayland backend's.
pub struct Ctx<'a> {
    state: &'a mut State,
    exit: &'a mut bool,
}

impl Ctx<'_> {
    pub fn request_redraw(&mut self) {
        self.state.redraw_requested = true;
    }

    /// Requests a redraw after `delay`, for retrying a frame that could not be presented.
    pub fn request_redraw_after(&mut self, delay: Duration) {
        let t = Instant::now() + delay;
        self.state.redraw_at = Some(self.state.redraw_at.map_or(t, |r| r.min(t)));
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.state.set_cursor(icon);
    }

    pub fn set_ime(&mut self, ime: Option<ImeState>) {
        self.state.set_ime(ime);
    }

    pub fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
        self.state.set_clipboard(kind, text);
    }

    /// Asks for the selection, which comes as [`WindowEvent::Paste`].
    pub fn request_paste(&mut self, kind: ClipboardKind) {
        self.state.request_paste(kind);
    }

    /// Something another thread can call to wake up the event loop, which then calls
    /// [`X11Handler::woken`].
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.state.wake.try_clone().ok()?;
        Some(move || {
            let _ = rustix::io::write(&wake, &[1]);
        })
    }

    pub fn set_title(&mut self, title: &str) {
        self.state.set_title(title);
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        self.state.set_fullscreen(on);
    }

    pub fn exit(&mut self) {
        *self.exit = true;
    }

    pub fn buffer_size(&self) -> SizeI {
        self.state.size
    }

    pub fn scale(&self) -> Scale {
        self.state.scale
    }

    /// What dmabufs the server takes, if DRI3 and Present can show them.
    pub fn dmabuf_feedback(&self) -> Option<&DmabufFeedback> {
        self.state.present.feedback.as_ref()
    }

    /// Keeps a dmabuf to make pixmaps of. Failure comes later as [`BufferEvent::Failed`].
    pub fn create_dmabuf_buffer(
        &mut self,
        width: u32,
        height: u32,
        _format: u32,
        modifier: u64,
        planes: &[DmabufPlaneDesc<'_>],
    ) -> Option<BufferId> {
        self.state
            .present
            .create_buffer(width, height, modifier, planes)
    }

    pub fn destroy_buffer(&mut self, id: BufferId) {
        self.state.present.destroy_buffer(&self.state.conn, id);
    }

    pub fn take_buffer_events(&mut self) -> Vec<BufferEvent> {
        std::mem::take(&mut self.state.present.buffer_events)
    }

    /// Asks to be told what becomes of the next presentation to the surface, which
    /// [`Ctx::take_presentations`] later returns. `false` if the server can't tell.
    pub fn request_presentation(&mut self, surface: SurfaceId, token: u64) -> bool {
        if self.state.present.clock().is_none() {
            return false;
        }
        match self.state.frames(surface) {
            Some(f) => {
                f.request_presentation(token);
                true
            }
            None => false,
        }
    }

    pub fn take_presentations(&mut self) -> Vec<Presentation> {
        std::mem::take(&mut self.state.present.presentations)
    }

    /// The clock presentation times are in, as a `clockid_t`.
    pub fn presentation_clock(&self) -> Option<u32> {
        self.state.present.clock()
    }

    /// Whether a presentation to the surface is still waiting for the server.
    pub fn frame_pending(&mut self, surface: SurfaceId) -> bool {
        self.state
            .frames(surface)
            .is_some_and(|f| f.pending(Instant::now()))
    }

    /// Whether there are popups, which have to keep in step with the window.
    pub fn has_popups(&self) -> bool {
        !self.state.popups.is_empty()
    }

    /// When the server last said a presentation to the surface was done.
    pub fn last_frame_callback(&mut self, surface: SurfaceId) -> Option<Instant> {
        self.state.frames(surface)?.last_complete
    }

    /// Whether the server takes DRM syncobj timelines (DRI3 1.4 and Present 1.4).
    pub fn explicit_sync_available(&self) -> bool {
        self.state.present.explicit_sync()
    }

    pub fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Option<TimelineId> {
        let window = self.state.window;
        self.state
            .present
            .import_timeline(&self.state.conn, window, fd)
    }

    pub fn destroy_timeline(&mut self, id: TimelineId) {
        self.state.present.destroy_timeline(&self.state.conn, id);
    }

    pub fn show_popup(&mut self, params: PopupParams) {
        self.state.show_popup(params);
    }

    pub fn update_popup(&mut self, params: PopupParams) {
        self.state.update_popup(params);
    }

    /// Whether drags can be handed to others.
    pub fn supports_drag(&self) -> bool {
        self.state.supports_drag()
    }

    /// Hands what is dragged to XDND. See `docs/design/23-drag-and-drop.md`.
    pub fn start_drag(&mut self, offer: DragOffer) {
        self.state.start_drag(offer);
    }

    /// Ends what was started with [`Ctx::start_drag`].
    pub fn cancel_drag(&mut self) {
        self.state.cancel_drag();
    }

    /// Sets the size of the picture of a drag, and where the pointer is on it.
    pub fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI) {
        self.state.place_drag_icon(logical, hotspot);
    }

    /// Closes the picture of a drag. GPU surfaces on it must go first.
    pub fn close_drag_icon(&mut self) {
        self.state.close_drag_icon();
    }

    /// Says how what is over the window would be taken.
    pub fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
        self.state.accept_drag(wanted);
    }

    /// Reads what was dropped, which comes as [`WindowEvent::DropData`].
    pub fn receive_drop(&mut self, mime: String) {
        self.state.receive_drop(mime);
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
        self.state.present.disable_explicit_sync();
    }

    /// Presents the buffer with `damage` (buffer pixels) as what changed.
    pub fn present_buffer(
        &mut self,
        surface: SurfaceId,
        id: BufferId,
        damage: &[RectI],
        sync: Option<SyncPoints>,
    ) {
        let Some((window, depth)) = self.state.window_of(surface) else {
            return;
        };
        let state = &mut *self.state;
        let Some(frames) = (match surface {
            SurfaceId::Window => Some(&mut state.window_frames),
            SurfaceId::Popup(id) => state.popups.get_mut(&id).map(|p| &mut p.frames),
            SurfaceId::DragIcon => None,
        }) else {
            return;
        };
        state
            .present
            .present(&state.conn, frames, window, depth, id, damage, sync);
        if surface == SurfaceId::Window {
            let size = state.size;
            state.window_presented(size);
        }
    }

    /// To call before something else presents to the surface, like a GPU surface.
    pub fn prepare_commit(&mut self, _surface: SurfaceId) {}

    /// To call after something else presented to the surface.
    pub fn committed(&mut self, surface: SurfaceId) {
        if let Some(f) = self.state.frames(surface) {
            f.committed();
        }
        if surface == SurfaceId::Window {
            let size = self.state.size;
            self.state.window_presented(size);
        }
    }

    /// The window has no alpha; nothing to say.
    pub fn set_opaque(&mut self, _surface: SurfaceId, _opaque: bool) {}

    /// Whether the surface was uncovered and has to be drawn again in full.
    pub fn take_exposed(&mut self, surface: SurfaceId) -> bool {
        match surface {
            SurfaceId::Window => std::mem::take(&mut self.state.exposed),
            SurfaceId::Popup(id) => self
                .state
                .popups
                .get_mut(&id)
                .is_some_and(|p| std::mem::take(&mut p.exposed)),
            SurfaceId::DragIcon => false,
        }
    }

    /// Raw handles for creating a GPU surface. Valid until the surface closes.
    pub fn raw_handles(&self, surface: SurfaceId) -> (RawDisplayHandle, RawWindowHandle) {
        let conn = NonNull::new(self.state.conn.get_raw_xcb_connection()).expect("xcb");
        let window = self
            .state
            .window_of(surface)
            .map_or(self.state.window, |(w, _)| w);
        (
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(conn), self.state.screen as i32)),
            RawWindowHandle::Xcb(XcbWindowHandle::new(
                NonZeroU32::new(window).expect("a window"),
            )),
        )
    }
}

/// Opens a window and runs the event loop until the handler exits.
pub fn run<H: X11Handler>(options: WindowOptions, mut handler: H) -> Result<(), Error> {
    let (wake_read, wake_write) = rustix::pipe::pipe_with(
        rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
    )
    .map_err(err)?;
    let mut state = State::new(&options, wake_write)?;
    let result = run_loop(&mut state, &wake_read, &mut handler);
    handler.exit();
    keep_clipboard(&mut state);
    state.present.release(&state.conn);
    let _ = state.conn.flush();
    result
}

/// Lets the clipboard manager take what we own before the window goes, briefly.
fn keep_clipboard(state: &mut State) {
    if !state.save_clipboard() {
        return;
    }
    let _ = state.conn.flush();
    while state.saving_clipboard() {
        match state.conn.poll_for_event() {
            Ok(Some(event)) => state.handle(event),
            Ok(None) => {
                let _ = state.conn.flush();
                let mut fds = [PollFd::new(&state.conn, PollFlags::IN)];
                let wait = rustix::time::Timespec {
                    tv_sec: 0,
                    tv_nsec: 20_000_000,
                };
                let _ = rustix::event::poll(&mut fds, Some(&wait));
            }
            Err(_) => break,
        }
    }
}

fn run_loop<H: X11Handler>(
    state: &mut State,
    wake_read: &std::os::fd::OwnedFd,
    handler: &mut H,
) -> Result<(), Error> {
    let mut exit = false;
    let mut timer_deadline: Option<Instant> = None;
    let mut woken = false;
    loop {
        while let Some(event) = state.conn.poll_for_event().map_err(err)? {
            state.handle(event);
        }
        state.give_up_pastes(Instant::now());
        state.give_up_drops(Instant::now());
        if let Some(focused) = state.take_focus_change() {
            state.events.insert(0, WindowEvent::Focused(focused));
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
            let mut ctx = Ctx {
                state,
                exit: &mut exit,
            };
            handler.event(&mut ctx, ev);
        }
        // Every position of a drop gets an answer, whether or not the core changed its mind.
        state.answer_positions();
        if std::mem::take(&mut woken) {
            let mut ctx = Ctx {
                state,
                exit: &mut exit,
            };
            handler.woken(&mut ctx);
        }
        // The deadline we slept until; asking again now would give the one after it.
        if timer_deadline.is_some_and(|d| d <= Instant::now()) {
            timer_deadline = None;
            let mut ctx = Ctx {
                state,
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
        if state.mapped {
            let ready = state.frame_wait(Instant::now()).is_ok();
            let mut ctx = Ctx {
                state,
                exit: &mut exit,
            };
            if ready {
                handler.surfaces_ready(&mut ctx);
                ctx.state.conn.flush().map_err(err)?;
            }
            if ctx.state.redraw_requested
                && (ctx.state.frame_wait(Instant::now()).is_ok()
                    || handler.may_render_ahead(&mut ctx))
            {
                ctx.state.redraw_requested = false;
                handler.frame(&mut ctx);
            }
        }

        state.conn.flush().map_err(err)?;
        if exit {
            break;
        }
        if !state.events.is_empty() {
            continue;
        }
        // What came in while waiting for replies is already read off the socket, where poll
        // wouldn't see it.
        if let Some(event) = state.conn.poll_for_event().map_err(err)? {
            state.handle(event);
            continue;
        }

        let now = Instant::now();
        timer_deadline = handler.deadline(now);
        let mut deadline = timer_deadline;
        if let Some(t) = state.redraw_at {
            deadline = Some(deadline.map_or(t, |d| d.min(t)));
        }
        if let Some(t) = state.selection_deadline() {
            deadline = Some(deadline.map_or(t, |d| d.min(t)));
        }
        if state.redraw_requested && state.mapped {
            // A presentation the server doesn't answer is given up on after a while.
            let t = now + Duration::from_millis(50);
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
            PollFd::new(&state.conn, PollFlags::IN),
            PollFd::new(wake_read, PollFlags::IN),
        ];
        match rustix::event::poll(&mut fds, timeout.as_ref()) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(err(e)),
        }
        if fds[1].revents().contains(PollFlags::IN) {
            let mut buf = [0u8; 64];
            while matches!(rustix::io::read(wake_read, &mut buf), Ok(n) if n > 0) {}
            woken = true;
        }
    }
    Ok(())
}
