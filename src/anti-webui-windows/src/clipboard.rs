//! The clipboard as `CF_UNICODETEXT` (see "クリップボード" in
//! `docs/design/39-platform-windows.md`).

use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// The clipboard, open for as long as this lives.
struct Open;

impl Open {
    /// Opens it; another program may have it open for a moment.
    fn new(hwnd: HWND) -> Option<Self> {
        for _ in 0..5 {
            // SAFETY: our window.
            if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                return Some(Self);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: opened by `Open::new`.
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Puts text on the clipboard, with Windows' line ends.
pub(crate) fn set(hwnd: HWND, text: &str) {
    let mut units: Vec<u16> = text
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .encode_utf16()
        .collect();
    units.push(0);
    let Some(_open) = Open::new(hwnd) else {
        return;
    };
    // SAFETY: the memory is filled within its size; the clipboard owns it once set.
    unsafe {
        if EmptyClipboard().is_err() {
            return;
        }
        let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) else {
            return;
        };
        let ptr = GlobalLock(memory) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(memory));
            return;
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
        let _ = GlobalUnlock(memory);
        if SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0))).is_err() {
            let _ = GlobalFree(Some(memory));
        }
    }
}

/// The text on the clipboard, with `\n` for line ends.
pub(crate) fn get(hwnd: HWND) -> Option<String> {
    let format = u32::from(CF_UNICODETEXT.0);
    // SAFETY: plain query.
    unsafe { IsClipboardFormatAvailable(format) }.ok()?;
    let _open = Open::new(hwnd)?;
    // SAFETY: the clipboard's memory, read while it is locked and up to its terminating zero.
    unsafe {
        let data = GetClipboardData(format).ok()?;
        let memory = HGLOBAL(data.0);
        let ptr = GlobalLock(memory) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(memory);
        Some(text.replace("\r\n", "\n"))
    }
}
