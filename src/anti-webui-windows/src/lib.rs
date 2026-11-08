//! The Windows backend, on Win32 (see `docs/design/39-platform-windows.md`).

#![cfg(windows)]

mod clipboard;
mod frames;
mod ime;
mod keyboard;
mod manipulation;
mod stack;
mod state;
#[doc(hidden)]
pub mod testing;
mod vblank;
mod wndproc;

use anti_webui_core::event::{ClipboardKind, DragAction, WindowEvent};
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{
    BorrowedFd, BufferEvent, BufferId, CursorIcon, DmabufFeedback, DmabufPlaneDesc, DragOffer,
    ImeState, PopupId, PopupParams, Presentation, SurfaceId, SyncPoints, TimelineId,
};
use anti_webui_core::window::{DecorationMode, ResizeEdge};
use frames::Frames;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use state::State;
use std::num::NonZeroIsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use vblank::{Handle, Ticker, Waker};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};
use windows::core::PCWSTR;

pub use stack::{UI_STACK, with_stack};
pub use vblank::monotonic_ns;

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Self(e.to_string())
    }
}

pub struct WindowOptions {
    pub title: String,
    /// Unused: Windows knows a program by its executable.
    pub app_id: String,
    /// Initial size in logical pixels.
    pub width: i32,
    pub height: i32,
    /// Whether it opens on all of the screen.
    pub fullscreen: bool,
    pub decorations: DecorationMode,
    /// What owns it: it stays above that window.
    pub parent: Option<ParentLink>,
    /// The core keeps input from the parent; Windows is told nothing more.
    pub modal: bool,
    /// A layer surface of a desktop shell, which Windows has no such thing as: opening one fails.
    pub layer: Option<anti_webui_core::layer::LayerSpec>,
    /// A lock surface, which needs Wayland too: opening one fails.
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

/// A window to open others above: its `HWND`.
#[derive(Clone, Copy, Debug)]
pub struct ParentLink(pub(crate) isize);

/// Callbacks from the event loop: the same as the other backends'.
pub trait WinHandler {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent);
    /// The vertical blank came after the last frame: the time for the next.
    fn frame(&mut self, ctx: &mut Ctx<'_>);
    /// The next time [`WinHandler::timer`] should run.
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

impl<H: WinHandler> WinHandler for &mut H {
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

/// What handlers may ask of the platform. The methods are those of the Wayland backend's; what
/// is about dmabufs and their synchronization has nothing to do here.
pub struct Ctx<'a> {
    frames: &'a mut Frames,
    state: &'a mut State,
    exit: &'a mut bool,
    wake: &'a Waker,
    ticker: &'a Ticker,
}

impl Ctx<'_> {
    pub fn request_redraw(&mut self) {
        self.frames.redraw_requested = true;
    }

    /// Requests a redraw after `delay`, for retrying a frame that could not be presented.
    pub fn request_redraw_after(&mut self, delay: Duration) {
        let t = Instant::now() + delay;
        self.frames.redraw_at = Some(self.frames.redraw_at.map_or(t, |r| r.min(t)));
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

    /// Asks for the clipboard, which comes as [`WindowEvent::Paste`].
    pub fn request_paste(&mut self, kind: ClipboardKind) {
        self.state.request_paste(kind);
    }

    /// Something another thread can call to wake up the event loop, which then calls
    /// [`WinHandler::woken`].
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.wake.clone();
        Some(move || wake.wake())
    }

    /// Moves the window with the pointer, from the press going on.
    pub fn start_move(&mut self) {
        self.state.start_nc_drag(HTCAPTION);
    }

    /// Resizes the window with the pointer from an edge, from the press going on.
    pub fn start_resize(&mut self, edge: ResizeEdge) {
        self.state.start_nc_drag(match edge {
            ResizeEdge::Top => HTTOP,
            ResizeEdge::Bottom => HTBOTTOM,
            ResizeEdge::Left => HTLEFT,
            ResizeEdge::Right => HTRIGHT,
            ResizeEdge::TopLeft => HTTOPLEFT,
            ResizeEdge::TopRight => HTTOPRIGHT,
            ResizeEdge::BottomLeft => HTBOTTOMLEFT,
            ResizeEdge::BottomRight => HTBOTTOMRIGHT,
        });
    }

    pub fn minimize(&mut self) {
        self.state.minimize();
    }

    pub fn set_maximized(&mut self, on: bool) {
        self.state.set_maximized(on);
    }

    pub fn show_window_menu(&mut self) {
        self.state.show_window_menu();
    }

    /// Layer surfaces are Wayland's: there are none here.
    pub fn change_layer(&mut self, _change: anti_webui_core::layer::LayerChange) {}

    pub fn set_layer_content_size(&mut self, _size: SizeI) {}

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

    pub fn destroy_buffer(&mut self, _id: BufferId) {}

    pub fn take_buffer_events(&mut self) -> Vec<BufferEvent> {
        Vec::new()
    }

    /// Asks to be told when the next presentation to the window is on the screen, which
    /// [`Ctx::take_presentations`] later returns: when the vertical blank comes after it.
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

    /// The clock of presentation times: the performance counter, which is the only one here.
    pub fn presentation_clock(&self) -> Option<u32> {
        Some(0)
    }

    /// Whether the last presentation to the window isn't on the screen yet.
    pub fn frame_pending(&mut self, surface: SurfaceId) -> bool {
        surface == SurfaceId::Window && self.frames.frame_pending(Instant::now())
    }

    /// Whether there are popups, which have to keep in step with the window.
    pub fn has_popups(&self) -> bool {
        self.frames.has_popups()
    }

    /// When the vertical blank last came after a presentation to the window.
    pub fn last_frame_callback(&mut self, surface: SurfaceId) -> Option<Instant> {
        (surface == SurfaceId::Window)
            .then_some(self.frames.last_frame)
            .flatten()
    }

    pub fn explicit_sync_available(&self) -> bool {
        false
    }

    pub fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Option<TimelineId> {
        fd.never()
    }

    pub fn destroy_timeline(&mut self, _id: TimelineId) {}

    pub fn disable_explicit_sync(&mut self) {}

    pub fn show_popup(&mut self, params: PopupParams) {
        self.frames.add_popup(params.id, params.parent);
        self.state.show_popup(params);
    }

    pub fn update_popup(&mut self, params: PopupParams) {
        self.state.update_popup(params);
    }

    /// Closes a popup and what is attached to it. GPU surfaces on them must go first.
    pub fn close_popup(&mut self, id: PopupId) {
        self.frames.remove_popup(id);
        self.state.close_popup(id);
    }

    /// What closes along with a popup.
    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.frames.popups_attached_to(id)
    }

    /// Drags aren't handed to other programs yet: the core runs them in the window.
    pub fn supports_drag(&self) -> bool {
        false
    }

    pub fn start_drag(&mut self, _offer: DragOffer) {}

    pub fn cancel_drag(&mut self) {}

    pub fn place_drag_icon(&mut self, _logical: SizeI, _hotspot: PointI) {}

    pub fn close_drag_icon(&mut self) {}

    pub fn accept_drag(&mut self, _wanted: Option<(String, DragAction)>) {}

    pub fn receive_drop(&mut self, _mime: String) {}

    pub fn finish_drop(&mut self) {}

    /// Our own buffers aren't presented yet: frames go through a GPU surface.
    pub fn present_buffer(
        &mut self,
        _surface: SurfaceId,
        _id: BufferId,
        _damage: &[RectI],
        _sync: Option<SyncPoints>,
    ) {
    }

    /// To call before something else presents to the surface, like a GPU surface.
    pub fn prepare_commit(&mut self, _surface: SurfaceId) {}

    /// To call after something else presented to the surface.
    pub fn committed(&mut self, surface: SurfaceId) {
        if surface == SurfaceId::Window {
            self.frames.presented();
            // SAFETY: our window.
            let monitor = unsafe { MonitorFromWindow(self.state.hwnd, MONITOR_DEFAULTTONEAREST) };
            self.ticker.want(monitor);
        }
    }

    /// DirectComposition takes the pixels with their alpha either way.
    pub fn set_opaque(&mut self, _surface: SurfaceId, _opaque: bool) {}

    /// Whether the surface came back into view and has to be drawn again in full.
    pub fn take_exposed(&mut self, surface: SurfaceId) -> bool {
        surface == SurfaceId::Window && self.state.take_exposed()
    }

    /// Raw handles for creating a GPU surface on the window or a popup. Valid until it closes.
    pub fn raw_handles(&self, surface: SurfaceId) -> Option<(RawDisplayHandle, RawWindowHandle)> {
        let hwnd = match surface {
            SurfaceId::Window => self.state.hwnd,
            SurfaceId::Popup(id) => self.state.popups.get(&id)?.hwnd,
            SurfaceId::DragIcon => return None,
        };
        let mut window = Win32WindowHandle::new(NonZeroIsize::new(hwnd.0 as isize)?);
        // SAFETY: plain query of this program's module.
        if let Ok(module) = unsafe { GetModuleHandleW(None) } {
            window.hinstance = NonZeroIsize::new(module.0 as isize);
        }
        Some((
            RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
            RawWindowHandle::Win32(window),
        ))
    }
}

/// A window of a [`Session`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey(u64);

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// What a thread needs to open windows like another: nothing, as every thread has its own windows
/// and messages.
pub struct SessionShare;

/// Who a session's windows call: the handler of each.
pub trait Handlers {
    type Handler: WinHandler;
    fn handler(&mut self, key: WindowKey) -> Option<&mut Self::Handler>;
}

struct Instance {
    key: WindowKey,
    state: State,
    frames: Frames,
    /// The deadline the session slept until; asking again now would give the one after it.
    timer_deadline: Option<Instant>,
    /// The vertical blank the touchpad's viewport was last sampled at.
    manipulated: u64,
    woken: bool,
    exit: bool,
}

/// The windows of a thread, which are the thread's in Win32 (see "複数ウィンドウ" in
/// `docs/design/39-platform-windows.md`).
pub struct Session {
    wake: Waker,
    ticker: Ticker,
    /// A timer of high resolution for deadlines; `None` before Windows 10 1803.
    timer: Option<Handle>,
    instances: Vec<Instance>,
}

impl Session {
    /// Windows from `spawn_window` are run on threads of their own.
    pub const OWN_THREADS: bool = true;

    /// The session of this thread.
    pub fn connect() -> Result<Self, Error> {
        wndproc::setup();
        // DirectManipulation is COM, in the thread's apartment. If the thread already has one of
        // another kind, this fails and that one serves.
        // SAFETY: once per thread with windows; never uninitialized, as the thread keeps COM.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let wake = Waker::new()?;
        // SAFETY: an unnamed timer.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .ok()
        .map(Handle);
        Ok(Self {
            ticker: Ticker::new(wake.clone()),
            wake,
            timer,
            instances: Vec::new(),
        })
    }

    /// A session for another thread, which has windows of its own.
    pub fn join(_share: SessionShare) -> Result<Self, Error> {
        Self::connect()
    }

    pub fn share(&self) -> SessionShare {
        SessionShare
    }

    /// Something another thread can call to wake up the session, which then calls
    /// [`WinHandler::woken`] of every window.
    pub fn waker(&self) -> Option<impl Fn() + Send + Sync + use<>> {
        let wake = self.wake.clone();
        Some(move || wake.wake())
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn open(&mut self, options: WindowOptions) -> Result<WindowKey, Error> {
        if options.layer.is_some() {
            return Err(Error("layer surfaces need Wayland".into()));
        }
        if options.lock.is_some() {
            return Err(Error("lock screens need Wayland".into()));
        }
        let key = WindowKey(NEXT_KEY.fetch_add(1, Ordering::Relaxed));
        let mut state = State::new(&options)?;
        if options.fullscreen {
            state.set_fullscreen(true);
        }
        self.instances.push(Instance {
            key,
            state,
            frames: Frames::new(self.ticker.last().0),
            timer_deadline: None,
            manipulated: 0,
            woken: false,
            exit: false,
        });
        Ok(key)
    }

    pub fn parent_link(&self, key: WindowKey) -> Option<ParentLink> {
        let i = self.instances.iter().find(|i| i.key == key)?;
        Some(i.state.parent_link())
    }

    /// Closes a window. GPU surfaces on it must go first.
    pub fn close(&mut self, key: WindowKey) {
        let Some(at) = self.instances.iter().position(|i| i.key == key) else {
            return;
        };
        let mut instance = self.instances.remove(at);
        instance.state.close();
    }

    /// The windows whose handlers asked to close them, through [`Ctx::exit`].
    pub fn exited(&self) -> Vec<WindowKey> {
        self.instances
            .iter()
            .filter(|i| i.exit)
            .map(|i| i.key)
            .collect()
    }

    pub fn request_redraw(&mut self, key: WindowKey) {
        if let Some(i) = self.instances.iter_mut().find(|i| i.key == key) {
            i.frames.redraw_requested = true;
        }
    }

    pub fn redraw_pending(&self, key: WindowKey) -> bool {
        self.instances
            .iter()
            .find(|i| i.key == key)
            .is_some_and(|i| i.frames.redraw_requested)
    }

    /// Hands every window what came for it, and draws those that are due. Doesn't wait.
    pub fn dispatch<M: Handlers>(&mut self, handlers: &mut M) -> Result<(), Error> {
        if self.wake.take_woken() {
            for i in &mut self.instances {
                i.woken = true;
            }
        }
        let tick = self.ticker.last();
        for i in &mut self.instances {
            if i.exit {
                continue;
            }
            if let Some(handler) = handlers.handler(i.key) {
                i.turn(handler, &self.wake, &self.ticker, tick);
            }
        }
        Ok(())
    }

    /// Waits until something comes for a window, or one of them has something to do.
    pub fn wait<M: Handlers>(&mut self, handlers: &mut M) -> Result<(), Error> {
        self.wait_until(handlers, None)
    }

    /// As [`Session::wait`], but waking by `until` at the latest. Windows' own loops, like that of
    /// moving or resizing a window, run [`Session::dispatch`] from inside.
    pub fn wait_until<M: Handlers>(
        &mut self,
        handlers: &mut M,
        until: Option<Instant>,
    ) -> Result<(), Error> {
        let now = Instant::now();
        let mut deadline: Option<Instant> = None;
        let mut earlier = |t: Instant| deadline = Some(deadline.map_or(t, |d| d.min(t)));
        if let Some(t) = until {
            earlier(t);
        }
        let seq = self.ticker.last().0;
        for i in &mut self.instances {
            let f = &i.frames;
            if i.exit || i.woken || i.state.has_pending() || f.has_events() || f.has_tick(seq) {
                return Ok(());
            }
            if i.state.manipulating() {
                if i.manipulated != seq {
                    return Ok(());
                }
                // The viewport following the fingers is sampled every vertical blank.
                self.ticker.want(i.state.monitor());
            }
            i.timer_deadline = handlers.handler(i.key).and_then(|h| h.deadline(now));
            if let Some(t) = i.timer_deadline {
                earlier(t);
            }
            if let Some(t) = f.redraw_at {
                earlier(t);
            }
            if let Some(t) = i.state.cloaked_since() {
                earlier(t + state::CLOAK_AT_MOST);
            }
            if f.redraw_requested && i.state.visible() {
                // The vertical blank wakes the loop; this is for when it doesn't come.
                earlier(f.frame_deadline().unwrap_or(now));
            }
        }
        if self.wake.is_woken() {
            return Ok(());
        }
        if deadline.is_some_and(|d| d <= now) {
            self.pump(handlers);
            return Ok(());
        }
        let mut handles: Vec<HANDLE> = vec![self.wake.event()];
        let mut timeout = INFINITE;
        if let Some(d) = deadline {
            let wait = d - now;
            // Relative, in units of 100 ns.
            let due = -((wait.as_nanos() / 100).max(1) as i64);
            // SAFETY: our timer.
            let timer = self
                .timer
                .as_ref()
                .filter(|t| unsafe { SetWaitableTimer(t.0, &due, 0, None, None, false).is_ok() });
            match timer {
                Some(timer) => handles.push(timer.0),
                None => timeout = wait.as_micros().div_ceil(1000).max(1) as u32,
            }
        }
        // SAFETY: valid handles, for as long as this waits.
        unsafe {
            MsgWaitForMultipleObjectsEx(Some(&handles), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
        self.pump(handlers);
        Ok(())
    }

    /// Dispatches the messages that came, with what Windows' own loops run in place.
    fn pump<M: Handlers>(&mut self, handlers: &mut M) {
        let session: *mut Self = self;
        let handlers: *mut M = handlers;
        let mut pump = move || {
            // SAFETY: nothing else touches either while messages are dispatched, and the pump runs
            // only from inside `DispatchMessageW` below (see `wndproc::with_pump`).
            let _ = unsafe { (*session).dispatch(&mut *handlers) };
        };
        wndproc::with_pump(&mut pump, || {
            let mut msg = MSG::default();
            // SAFETY: valid pointer; this thread's messages.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: a message just taken.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
    }

    /// Waits a little while threads end.
    pub fn idle(&mut self, timeout: Duration) {
        std::thread::sleep(timeout);
    }
}

/// Opens a window alone and runs until its handler exits.
pub fn run<H: WinHandler>(options: WindowOptions, handler: H) -> Result<(), Error> {
    struct One<H>(WindowKey, H);
    impl<H: WinHandler> Handlers for One<H> {
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
    fn turn<H: WinHandler>(
        &mut self,
        handler: &mut H,
        wake: &Waker,
        ticker: &Ticker,
        tick: (u64, Option<anti_webui_core::platform::Shown>),
    ) {
        let Instance {
            state,
            frames,
            timer_deadline,
            manipulated,
            woken,
            exit,
            ..
        } = self;
        if state.manipulating() && *manipulated != tick.0 {
            *manipulated = tick.0;
            state.update_manipulation();
        }
        let events = state.take_events();
        let mut ctx = Ctx {
            frames,
            state,
            exit,
            wake,
            ticker,
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
        let ready = f.take_tick(tick);
        // The first frame reached the screen with this vertical blank: the window can show.
        if ready
            || ctx
                .state
                .cloaked_since()
                .is_some_and(|t| t.elapsed() >= state::CLOAK_AT_MOST)
        {
            ctx.state.uncloak();
        }
        if ctx.state.visible() {
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
    }
}
