//! The macOS backend, on AppKit (see `docs/design/37-platform-macos.md`).

#![cfg(target_os = "macos")]

mod app;
mod dnd;
mod frames;
mod input;
mod keyboard;
mod popup;
mod remote;
mod state;
#[doc(hidden)]
pub mod testing;
mod view;

use anti_webui_core::event::{ClipboardKind, DragAction, DragResult, WindowEvent};
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{
    BufferEvent, BufferId, CursorIcon, DmabufFeedback, DmabufPlaneDesc, DragOffer, ImeState,
    PopupId, PopupParams, Presentation, SurfaceId, SyncPoints, TimelineId,
};
use anti_webui_core::window::{DecorationMode, ResizeEdge};
use frames::Frames;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_io_surface::IOSurfaceRef;
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use remote::{RemoteLink, ThreadWaker};
use state::State;
use std::os::fd::BorrowedFd;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub struct WindowOptions {
    pub title: String,
    /// Unused: macOS knows a program by its bundle.
    pub app_id: String,
    /// Initial size in logical pixels.
    pub width: i32,
    pub height: i32,
    /// Whether it opens on all of the screen.
    pub fullscreen: bool,
    pub decorations: DecorationMode,
    /// What it stays above (`addChildWindow:ordered:`).
    pub parent: Option<ParentLink>,
    /// The core keeps input from the parent; macOS is told nothing more.
    pub modal: bool,
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
        }
    }
}

/// A window to open others above: its `windowNumber`.
#[derive(Clone, Copy, Debug)]
pub struct ParentLink(pub(crate) isize);

/// Callbacks from the event loop: the same as the Wayland and X11 backends'.
pub trait MacHandler {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent);
    /// The display link came after the last frame: the time for the next.
    fn frame(&mut self, ctx: &mut Ctx<'_>);
    /// The next time [`MacHandler::timer`] should run.
    fn deadline(&mut self, now: Instant) -> Option<Instant>;
    fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant);
    /// Called once before the window closes; release GPU surfaces here.
    fn exit(&mut self) {}
    /// The last frame is on the screen: the time to present what was held back.
    fn surfaces_ready(&mut self, _ctx: &mut Ctx<'_>) {}
    /// Whether a frame can start now even though the last one isn't on the screen yet.
    fn may_render_ahead(&mut self, _ctx: &mut Ctx<'_>) -> bool {
        false
    }
    /// Another thread called the function from [`Ctx::waker`].
    fn woken(&mut self, _ctx: &mut Ctx<'_>) {}
}

impl<H: MacHandler> MacHandler for &mut H {
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

/// The window of a [`Ctx`]: here, or on the main thread for a window drawn on its own thread.
enum Window<'a> {
    Local(&'a mut State),
    Remote(&'a RemoteLink),
}

/// What handlers may ask of the platform. The methods are those of the Wayland backend's; what
/// is about dmabufs and their synchronization has nothing to do here.
pub struct Ctx<'a> {
    frames: &'a mut Frames,
    window: Window<'a>,
    exit: &'a mut bool,
    wake: &'a Waker,
}

impl Ctx<'_> {
    /// Does something to the window, which is on the main thread: now if this is it, otherwise
    /// when the main thread gets to it, in the order asked.
    fn on_window(&mut self, f: impl FnOnce(&mut State) + Send + 'static) {
        match &mut self.window {
            Window::Local(state) => f(state),
            Window::Remote(link) => link.run(f),
        }
    }

    pub fn request_redraw(&mut self) {
        self.frames.redraw_requested = true;
    }

    /// Requests a redraw after `delay`, for retrying a frame that could not be presented.
    pub fn request_redraw_after(&mut self, delay: Duration) {
        let t = Instant::now() + delay;
        self.frames.redraw_at = Some(self.frames.redraw_at.map_or(t, |r| r.min(t)));
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.on_window(move |s| s.set_cursor(icon));
    }

    pub fn set_ime(&mut self, ime: Option<ImeState>) {
        self.on_window(move |s| s.set_ime(ime));
    }

    pub fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
        self.on_window(move |s| s.set_clipboard(kind, text));
    }

    /// Asks for the clipboard, which comes as [`WindowEvent::Paste`].
    pub fn request_paste(&mut self, kind: ClipboardKind) {
        self.on_window(move |s| s.request_paste(kind));
    }

    /// Something another thread can call to wake up the event loop, which then calls
    /// [`MacHandler::woken`].
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.wake.clone();
        Some(move || wake.wake())
    }

    /// Moves the window with the pointer, from the press going on.
    pub fn start_move(&mut self) {
        self.on_window(State::start_move);
    }

    /// The system resizes from the edges by itself, whoever draws the frame.
    pub fn start_resize(&mut self, _edge: ResizeEdge) {}

    pub fn minimize(&mut self) {
        self.on_window(State::minimize);
    }

    pub fn set_maximized(&mut self, on: bool) {
        self.on_window(move |s| s.set_maximized(on));
    }

    /// macOS has no menu for windows.
    pub fn show_window_menu(&mut self) {}

    pub fn set_title(&mut self, title: &str) {
        let title = title.to_owned();
        self.on_window(move |s| s.set_title(&title));
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        self.on_window(move |s| s.set_fullscreen(on));
    }

    pub fn exit(&mut self) {
        *self.exit = true;
    }

    pub fn buffer_size(&self) -> SizeI {
        self.frames.size
    }

    pub fn scale(&self) -> Scale {
        self.frames.scale
    }

    pub fn dmabuf_feedback(&self) -> Option<&DmabufFeedback> {
        None
    }

    pub fn create_dmabuf_buffer(
        &mut self,
        _width: u32,
        _height: u32,
        _format: u32,
        _modifier: u64,
        _planes: &[DmabufPlaneDesc<'_>],
    ) -> Option<BufferId> {
        None
    }

    /// Wraps an `IOSurface` for showing with [`Ctx::present_buffer`].
    pub fn create_iosurface_buffer(&mut self, surface: &IOSurfaceRef) -> Option<BufferId> {
        Some(self.frames.add_buffer(surface))
    }

    pub fn destroy_buffer(&mut self, id: BufferId) {
        self.frames.remove_buffer(id);
    }

    /// The window server says nothing of the buffers it is done with: the presenter asks the
    /// `IOSurface`s whether they are in use.
    pub fn take_buffer_events(&mut self) -> Vec<BufferEvent> {
        Vec::new()
    }

    /// Asks to be told when the next presentation to the window is on the screen, which
    /// [`Ctx::take_presentations`] later returns: when the display link comes after it.
    pub fn request_presentation(&mut self, surface: SurfaceId, token: u64) -> bool {
        if surface != SurfaceId::Window {
            return false;
        }
        self.frames.request_presentation(token);
        true
    }

    pub fn take_presentations(&mut self) -> Vec<Presentation> {
        std::mem::take(&mut self.frames.presentations)
    }

    /// The clock of presentation times: `CLOCK_UPTIME_RAW`, which `CACurrentMediaTime` reads.
    pub fn presentation_clock(&self) -> Option<u32> {
        Some(libc::CLOCK_UPTIME_RAW)
    }

    /// Whether the last presentation to the window isn't on the screen yet.
    pub fn frame_pending(&mut self, surface: SurfaceId) -> bool {
        surface == SurfaceId::Window && self.frames.frame_pending(Instant::now())
    }

    /// Whether there are popups, which have to keep in step with the window.
    pub fn has_popups(&self) -> bool {
        self.frames.has_popups()
    }

    /// When the display link last came after a presentation to the window.
    pub fn last_frame_callback(&mut self, surface: SurfaceId) -> Option<Instant> {
        (surface == SurfaceId::Window)
            .then_some(self.frames.last_frame)
            .flatten()
    }

    pub fn explicit_sync_available(&self) -> bool {
        false
    }

    pub fn import_timeline(&mut self, _fd: BorrowedFd<'_>) -> Option<TimelineId> {
        None
    }

    pub fn destroy_timeline(&mut self, _id: TimelineId) {}

    pub fn show_popup(&mut self, params: PopupParams) {
        let layer = self.frames.add_popup(params.id, params.parent);
        self.on_window(move |s| s.show_popup(params, layer));
    }

    pub fn update_popup(&mut self, params: PopupParams) {
        self.on_window(move |s| s.update_popup(params));
    }

    /// Whether drags can be handed to other programs.
    pub fn supports_drag(&self) -> bool {
        true
    }

    /// Hands a drag to the system, once its picture is drawn: at the end of this pass of the
    /// loop.
    pub fn start_drag(&mut self, offer: DragOffer) {
        self.frames.drag.offer = Some(offer);
    }

    pub fn cancel_drag(&mut self) {
        if self.frames.drag.offer.take().is_some() {
            self.frames
                .events
                .push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
            return;
        }
        self.on_window(State::cancel_drag);
    }

    pub fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI) {
        self.frames.drag.place = Some((logical, hotspot));
    }

    pub fn close_drag_icon(&mut self) {
        self.frames.drag.icon = None;
        self.frames.drag.place = None;
    }

    pub fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
        self.on_window(move |s| s.accept_drag(wanted));
    }

    /// Reads what was dropped, which comes as [`WindowEvent::DropData`].
    pub fn receive_drop(&mut self, mime: String) {
        self.on_window(move |s| s.receive_drop(mime));
    }

    pub fn finish_drop(&mut self) {
        self.on_window(State::finish_drop);
    }

    /// Closes a popup and what is attached to it. GPU surfaces on them must go first.
    pub fn close_popup(&mut self, id: PopupId) {
        self.frames.remove_popup(id);
        self.on_window(move |s| s.close_popup(id));
    }

    /// What closes along with a popup.
    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.frames.popups_attached_to(id)
    }

    pub fn disable_explicit_sync(&mut self) {}

    /// Shows an `IOSurface` of ours on the surface. Core Animation takes no damage.
    pub fn present_buffer(
        &mut self,
        surface: SurfaceId,
        id: BufferId,
        _damage: &[RectI],
        _sync: Option<SyncPoints>,
    ) {
        self.frames.present(surface, id);
    }

    /// To call before something else presents to the surface, like a GPU surface.
    pub fn prepare_commit(&mut self, _surface: SurfaceId) {}

    /// To call after something else presented to the surface.
    pub fn committed(&mut self, surface: SurfaceId) {
        if surface == SurfaceId::Window {
            self.frames.presented();
        }
    }

    pub fn set_opaque(&mut self, surface: SurfaceId, opaque: bool) {
        if self.frames.set_opaque(surface, opaque) {
            return;
        }
        // A GPU surface on the view's own layer.
        let Window::Local(state) = &self.window else {
            return;
        };
        let view = match surface {
            SurfaceId::Window => Some(&state.view),
            SurfaceId::Popup(id) => state.popups.get(&id).map(|p| &p.view),
            SurfaceId::DragIcon => None,
        };
        if let Some(layer) = view.and_then(|v| v.metal_layer())
            && layer.isOpaque() != opaque
        {
            layer.setOpaque(opaque);
        }
    }

    /// Whether the surface came back into view and has to be drawn again in full.
    pub fn take_exposed(&mut self, surface: SurfaceId) -> bool {
        surface == SurfaceId::Window && self.frames.signals.take_exposed()
    }

    /// Raw handles for creating a GPU surface on a view's layer. Valid until the surface closes.
    /// `None` for a window drawn on its own thread, as AppKit's views are the main thread's.
    pub fn raw_handles(&self, surface: SurfaceId) -> Option<(RawDisplayHandle, RawWindowHandle)> {
        let Window::Local(state) = &self.window else {
            return None;
        };
        let view = match surface {
            SurfaceId::Window => &state.view,
            SurfaceId::Popup(id) => &state.popups.get(&id)?.view,
            SurfaceId::DragIcon => return None,
        };
        let ptr = NonNull::from(&**view).cast();
        Some((
            RawDisplayHandle::AppKit(AppKitDisplayHandle::new()),
            RawWindowHandle::AppKit(AppKitWindowHandle::new(ptr)),
        ))
    }
}

/// A window of a [`Session`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey(u64);

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// What a thread needs to open windows like another: nothing, as AppKit is the main thread's.
pub struct SessionShare;

/// Who a session's windows call: the handler of each.
pub trait Handlers {
    type Handler: MacHandler;
    fn handler(&mut self, key: WindowKey) -> Option<&mut Self::Handler>;
}

/// What wakes a session: the main thread's loop, or the run loop of a thread of its own.
#[derive(Clone)]
struct Waker {
    woken: Arc<AtomicBool>,
    thread: Option<Arc<ThreadWaker>>,
}

impl Waker {
    fn wake(&self) {
        self.woken.store(true, Ordering::Release);
        match &self.thread {
            Some(t) => t.wake(),
            None => app::wake_main(),
        }
    }
}

/// Where a window is: here, or on the main thread for a session of another thread.
enum Place {
    Local(State),
    Remote(Arc<RemoteLink>),
}

struct Instance {
    key: WindowKey,
    place: Place,
    frames: Frames,
    parent: ParentLink,
    /// The deadline the session slept until; asking again now would give the one after it.
    timer_deadline: Option<Instant>,
    woken: bool,
    exit: bool,
}

/// The windows of a thread. AppKit's windows are all on the main thread; a session of another
/// thread draws its windows there and has the main thread do the rest (see "複数ウィンドウ" in
/// `docs/design/37-platform-macos.md`).
pub struct Session {
    mtm: Option<MainThreadMarker>,
    wake: Waker,
    instances: Vec<Instance>,
}

impl Session {
    /// Windows from `spawn_window` are drawn on threads of their own.
    pub const OWN_THREADS: bool = true;

    /// The session of the main thread, which has to be the one calling.
    pub fn connect() -> Result<Self, Error> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            Error("AppKit runs on the main thread only: call App::run there".into())
        })?;
        app::setup(mtm);
        Ok(Self {
            mtm: Some(mtm),
            wake: Waker {
                woken: Arc::new(AtomicBool::new(false)),
                thread: None,
            },
            instances: Vec::new(),
        })
    }

    /// A session for this thread, which the main thread's runs the windows of.
    pub fn join(_share: SessionShare) -> Result<Self, Error> {
        if MainThreadMarker::new().is_some() {
            return Self::connect();
        }
        Ok(Self {
            mtm: None,
            wake: Waker {
                woken: Arc::new(AtomicBool::new(false)),
                thread: Some(ThreadWaker::here()),
            },
            instances: Vec::new(),
        })
    }

    pub fn share(&self) -> SessionShare {
        SessionShare
    }

    /// Something another thread can call to wake up the session, which then calls
    /// [`MacHandler::woken`] of every window.
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.wake.clone();
        Some(move || wake.wake())
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn open(&mut self, options: WindowOptions) -> Result<WindowKey, Error> {
        let key = WindowKey(NEXT_KEY.fetch_add(1, Ordering::Relaxed));
        let (place, frames, parent) = match (self.mtm, &self.wake.thread) {
            (Some(mtm), _) => {
                let (state, frames) = autoreleasepool(|_| State::new(mtm, &options, None))?;
                let parent = state.parent_link();
                (Place::Local(state), frames, parent)
            }
            (None, Some(thread)) => {
                let link = RemoteLink::new(key, thread.clone());
                let mut opened = None;
                // The main thread may be busy for a while; nothing it does waits for this one.
                dispatch2::DispatchQueue::main().exec_sync(|| {
                    opened = Some(autoreleasepool(|_| remote::host(&options, link.clone())));
                });
                let (frames, parent) = opened.expect("ran on the main thread")?;
                let frames = frames.0;
                frames.run_display_link_here();
                (Place::Remote(link), frames, parent)
            }
            (None, None) => unreachable!("a session is on the main thread or has a run loop"),
        };
        self.instances.push(Instance {
            key,
            place,
            frames,
            parent,
            timer_deadline: None,
            woken: false,
            exit: false,
        });
        Ok(key)
    }

    pub fn parent_link(&self, key: WindowKey) -> Option<ParentLink> {
        let i = self.instances.iter().find(|i| i.key == key)?;
        Some(i.parent)
    }

    /// Closes a window. GPU surfaces on it must go first.
    pub fn close(&mut self, key: WindowKey) {
        let Some(at) = self.instances.iter().position(|i| i.key == key) else {
            return;
        };
        let mut instance = self.instances.remove(at);
        autoreleasepool(|_| instance.frames.close());
        match instance.place {
            Place::Local(mut state) => autoreleasepool(|_| state.close()),
            Place::Remote(_) => {
                dispatch2::DispatchQueue::main().exec_async(move || remote::unhost(key))
            }
        }
    }

    /// The windows whose handlers asked to close them, through [`Ctx::exit`].
    pub fn exited(&self) -> Vec<WindowKey> {
        self.instances
            .iter()
            .filter(|i| i.exit)
            .map(|i| i.key)
            .collect()
    }

    /// Hands every window what came for it, and draws those that are due. Doesn't wait.
    pub fn dispatch<M: Handlers>(&mut self, handlers: &mut M) -> Result<(), Error> {
        autoreleasepool(|_| {
            if self.mtm.is_some() {
                if app::take_quit()
                    && let Some(Instance {
                        place: Place::Local(first),
                        ..
                    }) = self.instances.first()
                {
                    first.shared.push(WindowEvent::CloseRequested);
                }
                remote::forward_events();
            }
            if self.wake.woken.swap(false, Ordering::Acquire) {
                for i in &mut self.instances {
                    i.woken = true;
                }
            }
            for i in &mut self.instances {
                if i.exit {
                    continue;
                }
                if let Some(handler) = handlers.handler(i.key) {
                    i.turn(handler, &self.wake);
                }
            }
        });
        Ok(())
    }

    /// Waits until something comes for a window, or one of them has something to do. AppKit's
    /// own loops, like that of a live resize, run [`Session::dispatch`] from inside.
    pub fn wait<M: Handlers>(&mut self, handlers: &mut M) -> Result<(), Error> {
        let now = Instant::now();
        let mut deadline: Option<Instant> = None;
        let mut earlier = |t: Instant| deadline = Some(deadline.map_or(t, |d| d.min(t)));
        for i in &mut self.instances {
            let f = &i.frames;
            let pending = match &i.place {
                Place::Local(s) => s.has_pending(),
                Place::Remote(link) => link.has_events(),
            };
            if i.exit || i.woken || pending || f.has_events() || f.signals.has_tick() {
                return Ok(());
            }
            i.timer_deadline = handlers.handler(i.key).and_then(|h| h.deadline(now));
            if let Some(t) = i.timer_deadline {
                earlier(t);
            }
            if let Some(t) = f.redraw_at {
                earlier(t);
            }
            if f.redraw_requested && f.visible() {
                // The display link wakes the loop; this is for when it doesn't come.
                earlier(f.frame_deadline().unwrap_or(now));
            }
        }
        if self.wake.woken.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(mtm) = self.mtm else {
            if let Some(thread) = &self.wake.thread {
                autoreleasepool(|_| thread.wait(deadline));
            }
            return Ok(());
        };
        if remote::has_pending() {
            return Ok(());
        }
        let session: *mut Self = self;
        let handlers: *mut M = handlers;
        let mut pump = move || {
            // SAFETY: `wait` doesn't touch either while AppKit runs, and the pump runs only
            // from inside `pump_events` below (see `app::with_pump`).
            let _ = unsafe { (*session).dispatch(&mut *handlers) };
        };
        app::with_pump(&mut pump, || app::pump_events(mtm, deadline));
        Ok(())
    }

    /// Lets the main thread do what other threads ask of it for a while, as they end.
    pub fn idle(&mut self, timeout: Duration) {
        match self.mtm {
            Some(mtm) => autoreleasepool(|_| app::pump_events(mtm, Some(Instant::now() + timeout))),
            None => std::thread::sleep(timeout),
        }
    }
}

/// Opens a window alone and runs until its handler exits.
pub fn run<H: MacHandler>(options: WindowOptions, handler: H) -> Result<(), Error> {
    struct One<H>(WindowKey, H);
    impl<H: MacHandler> Handlers for One<H> {
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
    fn turn<H: MacHandler>(&mut self, handler: &mut H, wake: &Waker) {
        let Instance {
            place,
            frames,
            timer_deadline,
            woken,
            exit,
            ..
        } = self;
        let (events, window) = match place {
            Place::Local(state) => (state.take_events(), Window::Local(state)),
            Place::Remote(link) => (link.take_events(), Window::Remote(link)),
        };
        let mut ctx = Ctx {
            frames,
            window,
            exit,
            wake,
        };
        for ev in ctx.frames.take_in(events) {
            handler.event(&mut ctx, ev);
        }
        if std::mem::take(woken) {
            handler.woken(&mut ctx);
        }
        if timer_deadline.is_some_and(|d| d <= Instant::now()) {
            *timer_deadline = None;
            handler.timer(&mut ctx, Instant::now());
        }
        if *ctx.exit {
            return;
        }
        let f = &mut *ctx.frames;
        if f.redraw_at.is_some_and(|t| t <= Instant::now()) {
            f.redraw_at = None;
            f.redraw_requested = true;
        }
        let ready = f.take_tick();
        if f.visible() {
            if ready {
                handler.surfaces_ready(&mut ctx);
            }
            if ctx.frames.redraw_requested
                && (!ctx.frames.frame_pending(Instant::now()) || handler.may_render_ahead(&mut ctx))
            {
                ctx.frames.redraw_requested = false;
                handler.frame(&mut ctx);
            }
        }
        ctx.frames.settle_display_link();
        // A drag starts once its picture is drawn.
        if let Some(offer) = ctx.frames.drag.offer.take() {
            let place = ctx.frames.drag.place;
            let icon = ctx.frames.drag.icon.clone();
            ctx.on_window(move |s| s.begin_drag(offer, place, icon));
        }
    }
}
