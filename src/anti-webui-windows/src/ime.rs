//! The input method through IMM32: the composition is drawn inline by the core, and the candidates
//! go under the caret (see "IME" in `docs/design/39-platform-windows.md`).

use crate::keyboard::utf16_to_byte;
use crate::wndproc::Shared;
use anti_webui_core::event::{ImeUpdate, WindowEvent};
use anti_webui_core::platform::ImeState;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::Ime::{
    ATTR_TARGET_CONVERTED, ATTR_TARGET_NOTCONVERTED, CANDIDATEFORM, CFS_EXCLUDE, CFS_POINT,
    COMPOSITIONFORM, CPS_CANCEL, GCS_COMPATTR, GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, HIMC,
    IACE_DEFAULT, IME_COMPOSITION_STRING, ISC_SHOWUICOMPOSITIONWINDOW, ImmAssociateContextEx,
    ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME, ImmReleaseContext,
    ImmSetCandidateWindow, ImmSetCompositionWindow, NI_COMPOSITIONSTR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT,
    WM_IME_STARTCOMPOSITION,
};

/// The window's input context, for as long as this lives.
struct Context(HWND, HIMC);

impl Context {
    fn of(hwnd: HWND) -> Option<Self> {
        // SAFETY: our window.
        let himc = unsafe { ImmGetContext(hwnd) };
        (!himc.is_invalid()).then_some(Self(hwnd, himc))
    }

    /// A string of the composition, as UTF-16.
    fn string(&self, kind: IME_COMPOSITION_STRING) -> Vec<u16> {
        // SAFETY: asks for the size first, then fills a buffer of that size.
        unsafe {
            let bytes = ImmGetCompositionStringW(self.1, kind, None, 0);
            if bytes <= 0 {
                return Vec::new();
            }
            let mut buf = vec![0u16; bytes as usize / 2];
            ImmGetCompositionStringW(self.1, kind, Some(buf.as_mut_ptr().cast()), bytes as u32);
            buf
        }
    }

    /// The attribute of each UTF-16 unit of the composition.
    fn attributes(&self) -> Vec<u8> {
        // SAFETY: as above.
        unsafe {
            let bytes = ImmGetCompositionStringW(self.1, GCS_COMPATTR, None, 0);
            if bytes <= 0 {
                return Vec::new();
            }
            let mut buf = vec![0u8; bytes as usize];
            ImmGetCompositionStringW(
                self.1,
                GCS_COMPATTR,
                Some(buf.as_mut_ptr().cast()),
                bytes as u32,
            );
            buf
        }
    }

    fn cursor(&self) -> Option<usize> {
        // SAFETY: the cursor comes as the return value.
        let at = unsafe { ImmGetCompositionStringW(self.1, GCS_CURSORPOS, None, 0) };
        (at >= 0).then_some(at as usize)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: from `ImmGetContext` of the same window.
        unsafe {
            let _ = ImmReleaseContext(self.0, self.1);
        }
    }
}

/// The input method's messages to the window.
pub(crate) fn handle(
    hwnd: HWND,
    shared: &Shared,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match msg {
        WM_IME_SETCONTEXT => {
            // The core shows the composition itself.
            let lparam = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
            // SAFETY: our window.
            Some(unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
        }
        WM_IME_STARTCOMPOSITION => {
            shared.composing.set(true);
            place_windows(hwnd, shared);
            Some(LRESULT(0))
        }
        WM_IME_COMPOSITION => {
            let flags = lparam.0 as u32;
            let Some(context) = Context::of(hwnd) else {
                return Some(LRESULT(0));
            };
            let mut update = ImeUpdate::default();
            if flags & GCS_RESULTSTR.0 != 0 {
                let result = String::from_utf16_lossy(&context.string(GCS_RESULTSTR));
                update.commit = (!result.is_empty()).then_some(result);
                update.preedit = None;
            }
            if flags & GCS_COMPSTR.0 != 0 {
                let units = context.string(GCS_COMPSTR);
                let text = String::from_utf16_lossy(&units);
                if !text.is_empty() {
                    let range = target_range(&context.attributes())
                        .or_else(|| context.cursor().map(|c| (c, c)))
                        .map(|(a, b)| (utf16_to_byte(&text, a), utf16_to_byte(&text, b)));
                    update.preedit = Some((text, range));
                }
            }
            shared.push(WindowEvent::Ime(update));
            place_windows(hwnd, shared);
            Some(LRESULT(0))
        }
        WM_IME_ENDCOMPOSITION => {
            shared.composing.set(false);
            shared.push(WindowEvent::Ime(ImeUpdate::default()));
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// The clause being converted, in UTF-16 units.
fn target_range(attributes: &[u8]) -> Option<(usize, usize)> {
    let target = |a: &u8| {
        u32::from(*a) == ATTR_TARGET_CONVERTED || u32::from(*a) == ATTR_TARGET_NOTCONVERTED
    };
    let start = attributes.iter().position(target)?;
    let len = attributes[start..].iter().take_while(|a| target(a)).count();
    Some((start, start + len))
}

/// The core's state of the input method for the window: what is edited, and where the caret is.
pub(crate) fn set(hwnd: HWND, shared: &Shared, ime: Option<ImeState>) {
    let was = shared.ime.replace(ime.clone()).is_some();
    match (was, ime.is_some()) {
        (false, true) => {
            // SAFETY: our window; the default context comes back.
            unsafe {
                let _ = ImmAssociateContextEx(hwnd, HIMC::default(), IACE_DEFAULT);
            }
        }
        (true, false) => {
            cancel(hwnd, shared);
            // SAFETY: our window; the input method takes nothing while nothing is edited.
            unsafe {
                let _ = ImmAssociateContextEx(hwnd, HIMC::default(), 0);
            }
            return;
        }
        (false, false) => return,
        (true, true) => {}
    }
    place_windows(hwnd, shared);
}

/// Ends a composition, whose text goes.
pub(crate) fn cancel(hwnd: HWND, shared: &Shared) {
    if !shared.composing.replace(false) {
        return;
    }
    if let Some(context) = Context::of(hwnd) {
        // SAFETY: a valid context.
        unsafe {
            let _ = ImmNotifyIME(context.1, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
        }
    }
    shared.push(WindowEvent::Ime(ImeUpdate::default()));
}

/// Puts the input method's windows at the caret: the candidates below it, out of its way.
fn place_windows(hwnd: HWND, shared: &Shared) {
    let Some(r) = shared.ime.borrow().as_ref().map(|i| i.cursor_rect) else {
        return;
    };
    let Some(context) = Context::of(hwnd) else {
        return;
    };
    let caret = RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    };
    let composition = COMPOSITIONFORM {
        dwStyle: CFS_POINT,
        ptCurrentPos: POINT { x: r.x, y: r.y },
        rcArea: caret,
    };
    let candidates = CANDIDATEFORM {
        dwIndex: 0,
        dwStyle: CFS_EXCLUDE,
        ptCurrentPos: POINT {
            x: r.x,
            y: r.bottom(),
        },
        rcArea: caret,
    };
    // SAFETY: a valid context and forms.
    unsafe {
        let _ = ImmSetCompositionWindow(context.1, &composition);
        let _ = ImmSetCandidateWindow(context.1, &candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_clause_is_where_the_attributes_say() {
        let a = [0u8, 0, 1, 1, 1, 2, 2];
        assert_eq!(target_range(&a), Some((2, 5)));
        assert_eq!(target_range(&[0, 0, 0]), None);
    }
}
