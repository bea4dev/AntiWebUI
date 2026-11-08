//! The window procedure: messages become `WindowEvent`s in the window's inbox, and what has to be
//! answered at once is answered from what the inbox knows (see "イベントループ" and "入力" in
//! `docs/design/39-platform-windows.md`).

use crate::{ime, keyboard, manipulation};
use anti_webui_core::event::{
    ImeUpdate, KeyEvent, PointerButton, ScrollPhase, ScrollSource, WindowEvent,
};
use anti_webui_core::geometry::{PointF, PointI, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, ImeState, PopupId, PopupKind};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Once;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectManipulation::IDirectManipulationViewport;
use windows::Win32::Graphics::Gdi::{ScreenToClient, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetSystemMetricsForDpi,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_F4, VK_PROCESSKEY,
    VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub(crate) const CLASS: PCWSTR = w!("AntiWebUI");
/// Runs frames while Windows moves or resizes the window in a loop of its own.
const LIVE_TIMER: usize = 1;
/// Starts moving or resizing from the edge in `wParam`, from the loop's `DispatchMessage`.
pub(crate) const WM_APP_NC_DRAG: u32 = WM_APP + 1;
/// Shows the window's menu where the pointer is.
pub(crate) const WM_APP_SYSTEM_MENU: u32 = WM_APP + 2;

/// Sets the process up once: Per-Monitor v2 DPI and the window class.
pub(crate) fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: plain calls; the first fails if a manifest said already, which is as good.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
    });
}

/// What the window procedures of a window and of its popups share with the window's `State`.
pub(crate) struct Shared {
    events: RefCell<Vec<WindowEvent>>,
    pub hwnd: Cell<HWND>,
    /// The window draws its own frame.
    pub client_decor: Cell<bool>,
    /// The size and scale last told to the core.
    told: Cell<Option<(SizeI, Scale)>>,
    pub scale: Cell<Scale>,
    pub maximized: Cell<bool>,
    pub minimized: Cell<bool>,
    pub fullscreen: Cell<bool>,
    pub cursor: Cell<CursorIcon>,
    /// The buttons held, as bits of `MK_*`.
    buttons: Cell<u32>,
    pub ime: RefCell<Option<ImeState>>,
    pub composing: Cell<bool>,
    /// Menus close, as the pointer went to something else.
    pub dismiss_menus: Cell<bool>,
    /// The window moved: menus and tooltips close, and bubbles go where they belong again.
    pub moved: Cell<bool>,
    /// It came back from being minimized, and has to be drawn again in full.
    pub exposed: Cell<bool>,
    /// Windows moves or resizes it in a loop of its own.
    size_move: Cell<bool>,
    /// The window and its popups, to tell where the pointer went.
    pub windows: RefCell<Vec<HWND>>,
    /// The first half of a character that came in two `WM_CHAR`s.
    high_surrogate: Cell<Option<u16>>,
    /// What takes fingers on a touchpad, if DirectManipulation is there (see `manipulation`).
    pub viewport: RefCell<Option<IDirectManipulationViewport>>,
    /// Fingers landed and went to the viewport, which is to be sampled from now.
    pub touched: Cell<bool>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            events: RefCell::new(Vec::new()),
            hwnd: Cell::new(HWND::default()),
            client_decor: Cell::new(false),
            told: Cell::new(None),
            scale: Cell::new(Scale::ONE),
            maximized: Cell::new(false),
            minimized: Cell::new(false),
            fullscreen: Cell::new(false),
            cursor: Cell::new(CursorIcon::Default),
            buttons: Cell::new(0),
            ime: RefCell::new(None),
            composing: Cell::new(false),
            dismiss_menus: Cell::new(false),
            moved: Cell::new(false),
            exposed: Cell::new(false),
            size_move: Cell::new(false),
            windows: RefCell::new(Vec::new()),
            high_surrogate: Cell::new(None),
            viewport: RefCell::new(None),
            touched: Cell::new(false),
        }
    }

    pub fn push(&self, event: WindowEvent) {
        self.events.borrow_mut().push(event);
    }

    pub fn take_events(&self) -> Vec<WindowEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    pub fn has_events(&self) -> bool {
        !self.events.borrow().is_empty()
    }

    /// Tells the core the size and the scale of the window, if they changed.
    pub fn tell_size(&self) {
        let hwnd = self.hwnd.get();
        let mut r = RECT::default();
        // SAFETY: our window.
        let dpi = unsafe {
            let _ = GetClientRect(hwnd, &mut r);
            GetDpiForWindow(hwnd)
        };
        let scale = scale_of(dpi);
        let size = SizeI::new(r.right - r.left, r.bottom - r.top);
        self.scale.set(scale);
        if size.width <= 0 || size.height <= 0 || self.told.get() == Some((size, scale)) {
            return;
        }
        self.told.set(Some((size, scale)));
        self.push(WindowEvent::Resized { size, scale });
    }

    /// Shows the cursor the core asked for.
    pub fn apply_cursor(&self) {
        let id = match self.cursor.get() {
            CursorIcon::Default | CursorIcon::Copy => IDC_ARROW,
            CursorIcon::Pointer | CursorIcon::Grab => IDC_HAND,
            CursorIcon::Text => IDC_IBEAM,
            CursorIcon::NotAllowed => IDC_NO,
            CursorIcon::Grabbing | CursorIcon::Move => IDC_SIZEALL,
            CursorIcon::EwResize => IDC_SIZEWE,
            CursorIcon::NsResize => IDC_SIZENS,
            CursorIcon::Crosshair => IDC_CROSS,
            CursorIcon::None => {
                // SAFETY: plain call.
                unsafe { SetCursor(None) };
                return;
            }
        };
        // SAFETY: a system cursor, which is never freed.
        unsafe {
            if let Ok(cursor) = LoadCursorW(None, id) {
                SetCursor(Some(cursor));
            }
        }
    }

    /// Whether the pointer is over the window or one of its popups.
    pub fn pointer_over_ours(&self) -> bool {
        let mut p = POINT::default();
        // SAFETY: plain queries.
        let at = unsafe {
            if GetCursorPos(&mut p).is_err() {
                return false;
            }
            WindowFromPoint(p)
        };
        self.windows.borrow().contains(&at)
    }
}

/// The scale of a DPI: 96 is 1.
pub(crate) fn scale_of(dpi: u32) -> Scale {
    Scale(dpi.max(1) * 120 / 96)
}

/// What a window procedure knows of its window: the `Shared` of the window it is or belongs to.
pub(crate) struct WindowData {
    pub shared: Rc<Shared>,
    /// For a popup: which, and its origin in window coordinates.
    pub popup: Option<PopupId>,
    pub origin: Cell<PointI>,
    pub kind: Cell<PopupKind>,
    /// `TrackMouseEvent` was asked for since the pointer came.
    tracking: Cell<bool>,
}

impl WindowData {
    pub fn new(shared: Rc<Shared>, popup: Option<(PopupId, PopupKind)>) -> Box<Self> {
        Box::new(Self {
            shared,
            popup: popup.map(|p| p.0),
            origin: Cell::new(PointI::default()),
            kind: Cell::new(popup.map_or(PopupKind::Menu, |p| p.1)),
            tracking: Cell::new(false),
        })
    }

    /// The `WindowData` of one of our windows.
    ///
    /// # Safety
    /// `hwnd` is a window of this class, on this thread.
    pub unsafe fn of(hwnd: HWND) -> Option<&'static WindowData> {
        // SAFETY: what `WM_NCCREATE` put there, until `WM_NCDESTROY`.
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowData;
        // SAFETY: as said.
        NonNull::new(ptr as *mut WindowData).map(|p| unsafe { &*p.as_ptr() })
    }

    /// Where a point of this window's client area is in window coordinates.
    fn position(&self, lparam: LPARAM) -> PointF {
        let (x, y) = point_of(lparam);
        let o = self.origin.get();
        PointF::new((x + o.x) as f32, (y + o.y) as f32)
    }
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    let l = lparam.0 as u32;
    ((l & 0xFFFF) as i16 as i32, (l >> 16) as i16 as i32)
}

// --- The pump ---------------------------------------------------------------------------------

thread_local! {
    /// What runs a pass of the session's loop while Windows runs a loop of its own, like that of
    /// moving or resizing the window. Only there while the session dispatches messages.
    static PUMP: Cell<Option<NonNull<dyn FnMut()>>> = const { Cell::new(None) };
}

/// Runs `f` with `pump` as what Windows' own loops run.
pub(crate) fn with_pump<R>(pump: &mut dyn FnMut(), f: impl FnOnce() -> R) -> R {
    // SAFETY: only the lifetime changes; it is taken out again before `pump` goes.
    let pump: NonNull<dyn FnMut() + 'static> = unsafe { std::mem::transmute(NonNull::from(pump)) };
    let old = PUMP.with(|p| p.replace(Some(pump)));
    struct Restore(Option<NonNull<dyn FnMut()>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PUMP.with(|p| p.set(self.0));
        }
    }
    let _restore = Restore(old);
    f()
}

/// Runs a pass of the session's loop, if this is inside one of Windows' own loops. Never nested.
fn run_pump() {
    let Some(mut pump) = PUMP.with(Cell::take) else {
        return;
    };
    // SAFETY: set by `with_pump`, whose `pump` outlives this; taken out while it runs.
    unsafe { pump.as_mut()() };
    PUMP.with(|p| p.set(Some(pump)));
}

// --- The procedure ----------------------------------------------------------------------------

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: Windows calls this with the window's own messages.
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut WindowData;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(data) = WindowData::of(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match handle(hwnd, data, msg, wparam, lparam) {
            Some(r) => r,
            None => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Handles a message; `None` leaves it to `DefWindowProc`.
fn handle(
    hwnd: HWND,
    data: &WindowData,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    let shared = &*data.shared;
    let main = data.popup.is_none();
    match msg {
        WM_CLOSE => {
            if main {
                shared.push(WindowEvent::CloseRequested);
            }
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            // SAFETY: our window. What shows is DirectComposition's, which keeps it.
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            Some(LRESULT(0))
        }
        WM_SIZE if main => {
            if wparam.0 as u32 == SIZE_MINIMIZED {
                shared.minimized.set(true);
                return Some(LRESULT(0));
            }
            if shared.minimized.replace(false) {
                shared.exposed.set(true);
            }
            let maximized = wparam.0 as u32 == SIZE_MAXIMIZED;
            if shared.maximized.replace(maximized) != maximized && !shared.fullscreen.get() {
                shared.push(WindowEvent::Maximized(maximized));
            }
            shared.tell_size();
            if shared.size_move.get() {
                let keep = data.shared.clone();
                run_pump();
                drop(keep);
            }
            Some(LRESULT(0))
        }
        WM_MOVE if main => {
            if shared.windows.borrow().len() > 1 {
                shared.moved.set(true);
            }
            None
        }
        WM_DPICHANGED if main => {
            // SAFETY: Windows gives the rectangle it suggests.
            let r = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: our window.
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            shared.tell_size();
            Some(LRESULT(0))
        }
        WM_ENTERSIZEMOVE if main => {
            shared.size_move.set(true);
            // SAFETY: our window.
            unsafe { SetTimer(Some(hwnd), LIVE_TIMER, 8, None) };
            None
        }
        WM_EXITSIZEMOVE if main => {
            shared.size_move.set(false);
            // SAFETY: our window.
            unsafe {
                let _ = KillTimer(Some(hwnd), LIVE_TIMER);
            }
            None
        }
        WM_TIMER if wparam.0 == LIVE_TIMER => {
            let keep = data.shared.clone();
            run_pump();
            drop(keep);
            Some(LRESULT(0))
        }
        WM_ACTIVATE if main => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                shared.dismiss_menus.set(true);
            }
            None
        }
        WM_SETFOCUS if main => {
            shared.push(WindowEvent::Focused(true));
            Some(LRESULT(0))
        }
        WM_KILLFOCUS if main => {
            ime::cancel(hwnd, shared);
            shared.push(WindowEvent::Focused(false));
            Some(LRESULT(0))
        }
        WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN if main => {
            shared.dismiss_menus.set(true);
            None
        }
        WM_MOUSEACTIVATE if !main => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_NCHITTEST => {
            if !main {
                return (data.kind.get() == PopupKind::Tooltip)
                    .then_some(LRESULT(HTTRANSPARENT as isize));
            }
            shared
                .client_decor
                .get()
                .then(|| hit_test(hwnd, shared, lparam))
        }
        WM_NCCALCSIZE if main && shared.client_decor.get() && wparam.0 != 0 => {
            // The client area is all of the window, but for what goes off the screen when it is
            // maximized.
            // SAFETY: Windows gives the rectangles.
            let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
            // SAFETY: plain query.
            if unsafe { IsZoomed(hwnd) }.as_bool() && !shared.fullscreen.get() {
                let f = frame_thickness(hwnd);
                let r = &mut params.rgrc[0];
                r.left += f;
                r.top += f;
                r.right -= f;
                r.bottom -= f;
            }
            Some(LRESULT(0))
        }
        WM_SETCURSOR => {
            if (lparam.0 as u32 & 0xFFFF) == HTCLIENT {
                shared.apply_cursor();
                return Some(LRESULT(1));
            }
            None
        }
        WM_MOUSEMOVE => {
            if !data.tracking.replace(true) {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: valid pointer.
                unsafe {
                    let _ = TrackMouseEvent(&mut track);
                }
                shared.apply_cursor();
            }
            shared.push(WindowEvent::PointerMoved {
                position: data.position(lparam),
                modifiers: keyboard::modifiers(),
            });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            data.tracking.set(false);
            // Onto a popup of the window, or back from one.
            if !shared.pointer_over_ours() && shared.buttons.get() == 0 {
                shared.push(WindowEvent::PointerLeft);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_LBUTTONUP
        | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
            button(hwnd, data, msg, wparam, lparam);
            Some(LRESULT(if matches!(msg, WM_XBUTTONDOWN | WM_XBUTTONUP) {
                1
            } else {
                0
            }))
        }
        WM_CAPTURECHANGED => {
            shared.buttons.set(0);
            None
        }
        DM_POINTERHITTEST => {
            let pointer = (wparam.0 & 0xFFFF) as u32;
            let viewport = shared.viewport.borrow().clone();
            match viewport {
                Some(v) if manipulation::hit_test(&v, pointer) => {
                    shared.touched.set(true);
                    Some(LRESULT(0))
                }
                _ => None,
            }
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / WHEEL_DELTA as f32;
            let (x, y) = point_of(lparam);
            let mut p = POINT { x, y };
            // SAFETY: our window. The point is on the screen; pointer events are in the window's.
            unsafe {
                let _ = ScreenToClient(shared.hwnd.get(), &mut p);
            }
            let (delta_x, delta_y) = if msg == WM_MOUSEWHEEL {
                (0.0, -delta)
            } else {
                (delta, 0.0)
            };
            shared.push(WindowEvent::Scroll {
                position: PointF::new(p.x as f32, p.y as f32),
                delta_x,
                delta_y,
                modifiers: keyboard::modifiers(),
                source: ScrollSource::Wheel,
                phase: ScrollPhase::Single,
                time: message_time(),
            });
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            key(hwnd, shared, msg, wparam, lparam)
        }
        WM_CHAR => {
            char_typed(shared, wparam.0 as u16);
            Some(LRESULT(0))
        }
        // Alt and a letter would beep: there is no menu bar.
        WM_SYSCHAR => Some(LRESULT(0)),
        WM_IME_SETCONTEXT
        | WM_IME_STARTCOMPOSITION
        | WM_IME_COMPOSITION
        | WM_IME_ENDCOMPOSITION
            if main =>
        {
            ime::handle(hwnd, shared, msg, wparam, lparam)
        }
        WM_APP_NC_DRAG if main => {
            let mut p = POINT::default();
            // SAFETY: our window; Windows takes it from here in a loop of its own.
            unsafe {
                let _ = GetCursorPos(&mut p);
                let _ = ReleaseCapture();
                let at = LPARAM(((p.y as u16 as u32) << 16 | p.x as u16 as u32) as isize);
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, Some(wparam), Some(at));
            }
            Some(LRESULT(0))
        }
        WM_APP_SYSTEM_MENU if main => {
            system_menu(hwnd);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn message_time() -> u32 {
    // SAFETY: plain query.
    unsafe { GetMessageTime() as u32 }
}

fn button(hwnd: HWND, data: &WindowData, msg: u32, wparam: WPARAM, lparam: LPARAM) {
    let shared = &*data.shared;
    let x = ((wparam.0 >> 16) & 0xFFFF) as u16;
    let (button, bit, pressed) = match msg {
        WM_LBUTTONDOWN => (PointerButton::Primary, 1, true),
        WM_LBUTTONUP => (PointerButton::Primary, 1, false),
        WM_RBUTTONDOWN => (PointerButton::Secondary, 2, true),
        WM_RBUTTONUP => (PointerButton::Secondary, 2, false),
        WM_MBUTTONDOWN => (PointerButton::Middle, 4, true),
        WM_MBUTTONUP => (PointerButton::Middle, 4, false),
        // As evdev's `BTN_SIDE` and `BTN_EXTRA`, like the other backends.
        _ => {
            let (b, bit) = if x == XBUTTON1 {
                (0x113, 8)
            } else {
                (0x114, 16)
            };
            (PointerButton::Other(b), bit, msg == WM_XBUTTONDOWN)
        }
    };
    let held = shared.buttons.get();
    if pressed {
        if held == 0 {
            // SAFETY: our window, which gets what the pointer does until it is let go.
            unsafe { SetCapture(hwnd) };
        }
        shared.buttons.set(held | bit);
    } else {
        shared.buttons.set(held & !bit);
    }
    shared.push(WindowEvent::PointerButton {
        position: data.position(lparam),
        button,
        pressed,
        modifiers: keyboard::modifiers(),
        time: message_time(),
    });
    if !pressed && shared.buttons.get() == 0 {
        // SAFETY: plain call.
        unsafe {
            let _ = ReleaseCapture();
        }
        // The pointer may have been let go of outside.
        if !shared.pointer_over_ours() {
            shared.push(WindowEvent::PointerLeft);
        }
    }
}

fn key(hwnd: HWND, shared: &Shared, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let vk = wparam.0 as u16;
    // The input method took it.
    if vk == VK_PROCESSKEY.0 {
        return Some(LRESULT(0));
    }
    let pressed = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let scan = ((lparam.0 as u32) >> 16) & 0x1FF;
    let mut modifiers = keyboard::modifiers();
    let chars = if pressed {
        take_chars(hwnd)
    } else {
        String::new()
    };
    let text = keyboard::text_of(vk, &chars, &mut modifiers);
    shared.push(WindowEvent::Key(KeyEvent {
        key: keyboard::key_of(vk, scan),
        text,
        modifiers,
        pressed,
        repeat: pressed && (lparam.0 as u32) & (1 << 30) != 0,
    }));
    // Alt+F4 closes, Alt+Space shows the window's menu; Alt alone mustn't go to the menu bar.
    let system = msg == WM_SYSKEYDOWN && (vk == VK_F4.0 || vk == VK_SPACE.0);
    if system { None } else { Some(LRESULT(0)) }
}

/// The characters `TranslateMessage` made of the key being handled, which are next in the queue.
fn take_chars(hwnd: HWND) -> String {
    let mut units = Vec::new();
    let mut msg = MSG::default();
    loop {
        // SAFETY: valid pointer; our window's messages.
        let next =
            unsafe { PeekMessageW(&mut msg, Some(hwnd), WM_KEYFIRST, WM_KEYLAST, PM_NOREMOVE) };
        if !next.as_bool()
            || !matches!(
                msg.message,
                WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR
            )
        {
            break;
        }
        // SAFETY: as above; removes the one just looked at.
        unsafe {
            let _ = PeekMessageW(&mut msg, Some(hwnd), msg.message, msg.message, PM_REMOVE);
        }
        if matches!(msg.message, WM_CHAR | WM_SYSCHAR) {
            units.push(msg.wParam.0 as u16);
        }
    }
    String::from_utf16_lossy(&units)
}

/// A character that came without a key before it: from an input method without composition, or
/// typed with Alt and the number pad.
fn char_typed(shared: &Shared, unit: u16) {
    let text = match unit {
        0xD800..=0xDBFF => {
            shared.high_surrogate.set(Some(unit));
            return;
        }
        0xDC00..=0xDFFF => match shared.high_surrogate.take() {
            Some(high) => String::from_utf16_lossy(&[high, unit]),
            None => return,
        },
        _ => String::from_utf16_lossy(&[unit]),
    };
    if text.chars().any(char::is_control) {
        return;
    }
    shared.push(WindowEvent::Ime(ImeUpdate {
        commit: Some(text),
        ..Default::default()
    }));
}

/// The thickness of the frame Windows puts around a resizable window.
pub(crate) fn frame_thickness(hwnd: HWND) -> i32 {
    // SAFETY: plain queries.
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

/// What is at a point of a window that draws its own frame: the edges resize it, the rest is its.
fn hit_test(hwnd: HWND, shared: &Shared, lparam: LPARAM) -> LRESULT {
    // SAFETY: plain query.
    if unsafe { IsZoomed(hwnd) }.as_bool() || shared.fullscreen.get() {
        return LRESULT(HTCLIENT as isize);
    }
    let (x, y) = point_of(lparam);
    let mut r = RECT::default();
    // SAFETY: our window.
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    let f = frame_thickness(hwnd);
    let left = x < r.left + f;
    let right = x >= r.right - f;
    let top = y < r.top + f;
    let bottom = y >= r.bottom - f;
    let hit = match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (.., true) => HTBOTTOM,
        _ => HTCLIENT,
    };
    LRESULT(hit as isize)
}

/// The window's menu, where the pointer is; what is chosen goes as `WM_SYSCOMMAND`.
fn system_menu(hwnd: HWND) {
    let mut p = POINT::default();
    // SAFETY: our window; the menu is the system's.
    unsafe {
        let _ = GetCursorPos(&mut p);
        let menu = GetSystemMenu(hwnd, false);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            p.x,
            p.y,
            None,
            hwnd,
            None,
        );
        if cmd.0 != 0 {
            let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd.0 as usize), LPARAM(0));
        }
    }
}
