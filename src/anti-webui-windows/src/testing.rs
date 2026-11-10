//! Input made by the program, for testing the backend end to end: messages sent to the window
//! procedures of the windows of this thread, as Windows would send them (see "確かめ方" in
//! `docs/design/39-platform-windows.md`). Positions are logical window coordinates.

use crate::wndproc::WindowData;
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, SetKeyboardState, VK_CONTROL, VK_LCONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::*;

thread_local! {
    /// The first window of this thread that is still open.
    static LAST: Cell<isize> = const { Cell::new(0) };
    /// Whether the last window opened here takes a touchpad through DirectManipulation.
    static MANIPULATION: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn manipulation(on: bool) {
    MANIPULATION.with(|m| m.set(on));
}

/// Whether the last window opened takes fingers on a touchpad through DirectManipulation.
pub fn touchpad_manipulated() -> bool {
    MANIPULATION.with(Cell::get)
}

pub(crate) fn opened(hwnd: HWND) {
    // SAFETY: plain query.
    if !unsafe { IsWindow(Some(main())) }.as_bool() {
        LAST.with(|l| l.set(hwnd.0 as isize));
    }
}

fn main() -> HWND {
    HWND(LAST.with(Cell::get) as *mut _)
}

/// The scale of the window, as a factor.
pub fn scale() -> f32 {
    // SAFETY: plain query.
    unsafe { GetDpiForWindow(main()) as f32 / 96.0 }
}

/// The size of the window's client area in physical pixels.
/// Whether the window is hidden from the screen (see "提示" in `docs/design/39-platform-windows.md`).
pub fn cloaked() -> bool {
    let mut v = 0u32;
    // SAFETY: plain query into a `u32`, as the attribute gives.
    unsafe {
        windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            main(),
            windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,
            (&raw mut v).cast(),
            4,
        )
        .is_ok()
            && v != 0
    }
}

pub fn client_size() -> (i32, i32) {
    let mut r = RECT::default();
    // SAFETY: our window.
    unsafe {
        let _ = GetClientRect(main(), &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// Gives the client area a logical size, as dragging an edge would.
pub fn resize(width: f32, height: f32) {
    let hwnd = main();
    let s = scale();
    let (w, h) = ((width * s).round() as i32, (height * s).round() as i32);
    let (cw, ch) = client_size();
    let mut r = RECT::default();
    // SAFETY: our window.
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            r.right - r.left + w - cw,
            r.bottom - r.top + h - ch,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Our windows that show, the window and its popups.
pub fn visible_windows() -> usize {
    let Some(data) = (unsafe { WindowData::of(main()) }) else {
        return 0;
    };
    let windows = data.shared.windows.borrow();
    // SAFETY: plain queries.
    windows
        .iter()
        .filter(|w| unsafe { IsWindowVisible(**w) }.as_bool())
        .count()
}

/// The window, or the popup on top, at a point of window coordinates (physical), with the point
/// in its own client area.
fn window_at(x: i32, y: i32) -> (HWND, i32, i32) {
    let hwnd = main();
    // SAFETY: our windows, on this thread.
    let Some(data) = (unsafe { WindowData::of(hwnd) }) else {
        return (hwnd, x, y);
    };
    let windows = data.shared.windows.borrow().clone();
    for w in windows.iter().rev().filter(|w| **w != hwnd) {
        // SAFETY: as above.
        let Some(d) = (unsafe { WindowData::of(*w) }) else {
            continue;
        };
        let o = d.origin.get();
        let mut r = RECT::default();
        // SAFETY: as above.
        unsafe {
            let _ = GetClientRect(*w, &mut r);
        }
        let (lx, ly) = (x - o.x, y - o.y);
        if lx >= 0 && ly >= 0 && lx < r.right && ly < r.bottom {
            return (*w, lx, ly);
        }
    }
    (hwnd, x, y)
}

fn lparam(x: i32, y: i32) -> LPARAM {
    LPARAM(((y as u16 as u32) << 16 | x as u16 as u32) as isize)
}

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: LPARAM) {
    // SAFETY: our window, on this thread: the window procedure runs now.
    unsafe {
        SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(lparam));
    }
}

fn press(x: f32, y: f32, down: u32, up: u32, mk: usize) {
    let s = scale();
    let (px, py) = ((x * s).round() as i32, (y * s).round() as i32);
    let (hwnd, lx, ly) = window_at(px, py);
    let at = lparam(lx, ly);
    send(hwnd, WM_MOUSEMOVE, 0, at);
    send(hwnd, down, mk, at);
    send(hwnd, up, 0, at);
}

/// A click of the primary button.
pub fn click(x: f32, y: f32) {
    press(x, y, WM_LBUTTONDOWN, WM_LBUTTONUP, 0x1);
}

/// A click of the secondary button.
pub fn right_click(x: f32, y: f32) {
    press(x, y, WM_RBUTTONDOWN, WM_RBUTTONUP, 0x2);
}

/// The wheel turned by `notches` (positive towards the user, scrolling down) at a point.
pub fn wheel(x: f32, y: f32, notches: i32) {
    let s = scale();
    let mut p = POINT {
        x: (x * s).round() as i32,
        y: (y * s).round() as i32,
    };
    // SAFETY: our window.
    unsafe {
        let _ = ClientToScreen(main(), &mut p);
    }
    let delta = (-notches * WHEEL_DELTA as i32) as i16 as u16 as usize;
    send(main(), WM_MOUSEWHEEL, delta << 16, lparam(p.x, p.y));
}

/// Types text, a key and the characters `TranslateMessage` makes of it at a time: posted, so that
/// they come in through the loop like the user's.
pub fn type_text(text: &str) {
    let hwnd = main();
    for c in text.chars() {
        let vk = c.to_ascii_uppercase() as usize;
        // SAFETY: our window.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(vk), LPARAM(1));
            let _ = PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(vk), LPARAM(1 | (3 << 30)));
        }
    }
}

/// Characters that come without a key, as from an input method that doesn't compose.
pub fn commit(text: &str) {
    for unit in text.encode_utf16() {
        send(main(), WM_CHAR, unit as usize, LPARAM(1));
    }
}

/// A key pressed and let go with Control or without.
pub fn key(vk: u16, control: bool) {
    let mut state = [0u8; 256];
    // SAFETY: valid buffers; the state of this thread's keyboard is put back after.
    unsafe {
        let _ = GetKeyboardState(&mut state);
        let mut held = state;
        if control {
            held[VK_CONTROL.0 as usize] = 0x80;
            held[VK_LCONTROL.0 as usize] = 0x80;
        }
        let _ = SetKeyboardState(&held);
        send(main(), WM_KEYDOWN, vk as usize, LPARAM(1));
        send(main(), WM_KEYUP, vk as usize, LPARAM(1 | (3 << 30)));
        let _ = SetKeyboardState(&state);
    }
}

/// The close button, as the title bar's.
pub fn press_close_button() {
    // SAFETY: our window; it is only asked.
    unsafe {
        let _ = PostMessageW(Some(main()), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Whether the window covers its screen, and whether Windows says it is maximized.
pub fn fullscreen_and_maximized() -> (bool, bool) {
    let hwnd = main();
    let mut r = RECT::default();
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: plain queries.
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
        let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
        (r == info.rcMonitor, IsZoomed(hwnd).as_bool())
    }
}

/// The windows of this program on the screen, popups aside, and the threads they are on.
pub fn top_level_windows() -> (usize, usize) {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> windows::core::BOOL {
        // SAFETY: what `top_level_windows` passed; plain queries.
        unsafe {
            let found = &mut *(found.0 as *mut Vec<u32>);
            let mut pid = 0;
            let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut class = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if pid == std::process::id()
                && String::from_utf16_lossy(&class[..n]) == "AntiWebUI"
                && IsWindowVisible(hwnd).as_bool()
                && ex & WS_EX_NOACTIVATE.0 == 0
            {
                found.push(thread);
            }
        }
        true.into()
    }
    let mut found: Vec<u32> = Vec::new();
    // SAFETY: the callback only reads, and `found` outlives the call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut Vec<u32> as isize));
    }
    let windows = found.len();
    found.sort_unstable();
    found.dedup();
    (windows, found.len())
}

/// The colors on the screen just inside the top left corner of the first popup, where its shadow
/// fades out and what is behind shows, and at its middle: `None` if something else is on top.
pub fn popup_colors() -> Option<[(u8, u8, u8); 2]> {
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    let hwnd = main();
    // SAFETY: our windows, on this thread; the screen's device context is let go of.
    unsafe {
        let data = WindowData::of(hwnd)?;
        let popup = *data.shared.windows.borrow().iter().find(|w| **w != hwnd)?;
        let mut r = RECT::default();
        GetWindowRect(popup, &mut r).ok()?;
        let corner = POINT {
            x: r.left + 1,
            y: r.top + 1,
        };
        let middle = POINT {
            x: (r.left + r.right) / 2,
            y: (r.top + r.bottom) / 2,
        };
        if WindowFromPoint(corner) != popup || WindowFromPoint(middle) != popup {
            return None;
        }
        let dc = GetDC(None);
        let at = |p: POINT| {
            let c = GetPixel(dc, p.x, p.y).0;
            (c as u8, (c >> 8) as u8, (c >> 16) as u8)
        };
        let colors = [at(corner), at(middle)];
        ReleaseDC(None, dc);
        Some(colors)
    }
}
