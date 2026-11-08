//! One window: its `HWND`, and what the handlers ask of it (see "ウィンドウ" in
//! `docs/design/39-platform-windows.md`). What is drawn when is in `Frames`.

use crate::manipulation::Manipulation;
use crate::wndproc::{self, CLASS, Shared, WindowData};
use crate::{Error, ParentLink, WindowOptions, clipboard, ime};
use anti_webui_core::event::{ClipboardKind, WindowEvent};
use anti_webui_core::geometry::{PointI, RectI, SizeI};
use anti_webui_core::platform::{
    CursorIcon, ImeState, PopupId, PopupKind, PopupParams, place_popup,
};
use anti_webui_core::window::{DecorationMode, Decorations};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAK, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;

pub(crate) struct Popup {
    pub hwnd: HWND,
    pub params: PopupParams,
    /// Its origin in window coordinates (physical), as the core knows it.
    pub origin: PointI,
}

/// What a window that went on all of the screen comes back to.
struct Restore {
    style: WINDOW_STYLE,
    placement: WINDOWPLACEMENT,
}

pub(crate) struct State {
    pub hwnd: HWND,
    pub shared: Rc<Shared>,
    pub popups: HashMap<PopupId, Popup>,
    restore: Option<Restore>,
    /// Fingers on a touchpad; `None` without DirectManipulation.
    manipulation: Option<Manipulation>,
    /// Since when the window is hidden from the screen while its first frame comes.
    cloaked: Option<Instant>,
}

/// How long a window stays hidden waiting for its first frame, if that never comes.
pub(crate) const CLOAK_AT_MOST: Duration = Duration::from_secs(2);

fn cloak(hwnd: HWND, on: bool) -> bool {
    let value = windows::core::BOOL::from(on);
    // SAFETY: our window; a `BOOL`, as the attribute takes.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            (&raw const value).cast(),
            size_of_val(&value) as u32,
        )
    }
    .is_ok()
}

fn instance() -> windows::Win32::Foundation::HINSTANCE {
    // SAFETY: plain query of this program's module.
    unsafe { GetModuleHandleW(None) }.unwrap_or_default().into()
}

impl State {
    pub fn new(options: &WindowOptions) -> Result<Self, Error> {
        wndproc::setup();
        let client = options.decorations == DecorationMode::Client;
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let ex_style = WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW;
        let shared = Rc::new(Shared::new());
        shared.client_decor.set(client);
        let data = Box::into_raw(WindowData::new(shared.clone(), None));
        let owner = options.parent.map(|p| HWND(p.0 as *mut _));
        // SAFETY: the class is registered; `data` goes to the window, which frees it.
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                CLASS,
                &HSTRING::from(options.title.as_str()),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                owner,
                None,
                Some(instance()),
                Some(data.cast()),
            )
        };
        let hwnd = match hwnd {
            Ok(h) => h,
            Err(e) => {
                // SAFETY: the window didn't take it.
                drop(unsafe { Box::from_raw(data) });
                return Err(Error(format!("could not create a window: {e}")));
            }
        };
        shared.hwnd.set(hwnd);
        shared.windows.borrow_mut().push(hwnd);
        crate::testing::opened(hwnd);
        let manipulation = Manipulation::new(hwnd, shared.clone());
        crate::testing::manipulation(manipulation.is_some());
        *shared.viewport.borrow_mut() = manipulation.as_ref().map(Manipulation::viewport);
        let mut state = Self {
            hwnd,
            shared,
            popups: HashMap::new(),
            restore: None,
            manipulation,
            cloaked: None,
        };
        if client {
            // The system's shadow and corners stay with a frame of a pixel in the client area.
            let margins = MARGINS {
                cxLeftWidth: 0,
                cxRightWidth: 0,
                cyTopHeight: 1,
                cyBottomHeight: 0,
            };
            // SAFETY: our window.
            unsafe {
                let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
                // Takes in the frame `WM_NCCALCSIZE` now gives.
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        state.set_initial_size(options.width, options.height, owner);
        // Nothing is drawn yet, and without a redirection bitmap the window would show its frame
        // around nothing until the first frame. It is shown, so it is active and has its size,
        // but hidden from the screen until then.
        if cloak(hwnd, true) {
            state.cloaked = Some(Instant::now());
        }
        // SAFETY: our window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        state.shared.push(WindowEvent::Decorations(if client {
            Decorations::Client
        } else {
            Decorations::Server
        }));
        state.shared.tell_size();
        Ok(state)
    }

    /// Gives the client area the logical size asked for, at the scale of the screen it opened on,
    /// and puts a dialog over its owner.
    fn set_initial_size(&self, width: i32, height: i32, owner: Option<HWND>) {
        let hwnd = self.hwnd;
        // SAFETY: our windows.
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            let scale = wndproc::scale_of(dpi);
            let mut r = RECT {
                left: 0,
                top: 0,
                right: scale.px(width.max(1) as f32),
                bottom: scale.px(height.max(1) as f32),
            };
            if !self.shared.client_decor.get() {
                let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
                let ex = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
                let _ = AdjustWindowRectExForDpi(&mut r, style, false, ex, dpi);
            }
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let mut at = RECT::default();
            let _ = GetWindowRect(hwnd, &mut at);
            let (mut x, mut y) = (at.left, at.top);
            if let Some(owner) = owner {
                let mut o = RECT::default();
                if GetWindowRect(owner, &mut o).is_ok() {
                    x = o.left + (o.right - o.left - w) / 2;
                    y = o.top + (o.bottom - o.top - h) / 2;
                }
            }
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    pub fn parent_link(&self) -> ParentLink {
        ParentLink(self.hwnd.0 as isize)
    }

    /// What came for the window since the last time.
    pub fn take_events(&mut self) -> Vec<WindowEvent> {
        let mut events = self.shared.take_events();
        // Only the last of consecutive pointer moves matters.
        events.dedup_by(|next, prev| {
            let both = matches!(
                (&*prev, &*next),
                (
                    WindowEvent::PointerMoved { .. },
                    WindowEvent::PointerMoved { .. }
                )
            );
            if both {
                *prev = next.clone();
            }
            both
        });
        if self.shared.moved.take() {
            self.shared.dismiss_menus.set(false);
            events.extend(self.dismissals(&[PopupKind::Menu, PopupKind::Tooltip]));
            let bubbles: Vec<PopupId> = self
                .popups
                .iter()
                .filter(|(_, p)| p.params.kind == PopupKind::Bubble)
                .map(|(id, _)| *id)
                .collect();
            for id in bubbles {
                self.place_popup(id);
                events.extend(self.shared.take_events());
            }
        } else if self.shared.dismiss_menus.take() {
            events.extend(self.dismissals(&[PopupKind::Menu]));
        }
        events
    }

    /// Whether there is something to hand to the window's handler.
    pub fn has_pending(&self) -> bool {
        self.shared.has_events() || self.shared.dismiss_menus.get() || self.shared.moved.get()
    }

    pub fn visible(&self) -> bool {
        !self.shared.minimized.get()
    }

    /// Since when the window waits hidden for its first frame.
    pub fn cloaked_since(&self) -> Option<Instant> {
        self.cloaked
    }

    /// Puts the window on the screen: its first frame is there, or isn't coming.
    pub fn uncloak(&mut self) {
        if self.cloaked.take().is_some() {
            cloak(self.hwnd, false);
        }
    }

    /// Whether fingers are on the touchpad, or their inertia goes on: the viewport following them
    /// is to be sampled every vertical blank.
    pub fn manipulating(&self) -> bool {
        let Some(m) = &self.manipulation else {
            return false;
        };
        if self.shared.touched.take() {
            m.touched();
        }
        m.active()
    }

    /// Samples where the fingers went; what they did comes as events.
    pub fn update_manipulation(&self) {
        if let Some(m) = &self.manipulation {
            m.update();
        }
    }

    /// The screen the window is on.
    pub fn monitor(&self) -> windows::Win32::Graphics::Gdi::HMONITOR {
        // SAFETY: our window.
        unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) }
    }

    pub fn take_exposed(&self) -> bool {
        self.shared.exposed.take()
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        if self.shared.cursor.replace(icon) != icon && self.shared.pointer_over_ours() {
            self.shared.apply_cursor();
        }
    }

    pub fn set_ime(&mut self, state: Option<ImeState>) {
        ime::set(self.hwnd, &self.shared, state);
    }

    pub fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
        // There is no primary selection on Windows.
        if kind == ClipboardKind::Clipboard {
            clipboard::set(self.hwnd, &text);
        }
    }

    pub fn request_paste(&mut self, kind: ClipboardKind) {
        if kind != ClipboardKind::Clipboard {
            return;
        }
        if let Some(text) = clipboard::get(self.hwnd) {
            self.shared.push(WindowEvent::Paste { kind, text });
        }
    }

    pub fn set_title(&mut self, title: &str) {
        // SAFETY: our window.
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
    }

    /// Moves or resizes the window with the pointer, from the loop that dispatches messages.
    pub fn start_nc_drag(&mut self, hit: u32) {
        // SAFETY: our window.
        unsafe {
            let _ = PostMessageW(
                Some(self.hwnd),
                wndproc::WM_APP_NC_DRAG,
                WPARAM(hit as usize),
                LPARAM(0),
            );
        }
    }

    pub fn show_window_menu(&mut self) {
        // SAFETY: our window.
        unsafe {
            let _ = PostMessageW(
                Some(self.hwnd),
                wndproc::WM_APP_SYSTEM_MENU,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    pub fn minimize(&mut self) {
        // SAFETY: our window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
        }
    }

    pub fn set_maximized(&mut self, on: bool) {
        if self.shared.fullscreen.get() {
            return;
        }
        // SAFETY: our window.
        unsafe {
            let _ = ShowWindow(self.hwnd, if on { SW_MAXIMIZE } else { SW_RESTORE });
        }
    }

    /// Borderless on all of the screen the window is on, or back to where it was.
    pub fn set_fullscreen(&mut self, on: bool) {
        if self.shared.fullscreen.get() == on {
            return;
        }
        let hwnd = self.hwnd;
        // SAFETY: our window.
        unsafe {
            if on {
                let mut placement = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                let _ = GetWindowPlacement(hwnd, &mut placement);
                let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
                self.restore = Some(Restore { style, placement });
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(monitor, &mut info);
                let r = info.rcMonitor;
                self.shared.fullscreen.set(true);
                SetWindowLongW(
                    hwnd,
                    GWL_STYLE,
                    ((style & !WS_OVERLAPPEDWINDOW) | WS_POPUP).0 as i32,
                );
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
                );
            } else {
                self.shared.fullscreen.set(false);
                if let Some(Restore { style, placement }) = self.restore.take() {
                    SetWindowLongW(hwnd, GWL_STYLE, style.0 as i32);
                    let _ = SetWindowPlacement(hwnd, &placement);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        0,
                        0,
                        SWP_FRAMECHANGED
                            | SWP_NOMOVE
                            | SWP_NOSIZE
                            | SWP_NOZORDER
                            | SWP_NOOWNERZORDER,
                    );
                }
            }
        }
        self.shared.push(WindowEvent::Fullscreen(on));
    }

    /// Lets go of the window and its popups. GPU surfaces on them must go first.
    pub fn close(&mut self) {
        let ids: Vec<PopupId> = self.popups.keys().copied().collect();
        for id in ids {
            self.close_popup(id);
        }
        ime::set(self.hwnd, &self.shared, None);
        self.shared.viewport.borrow_mut().take();
        self.manipulation = None;
        // SAFETY: our window; the window procedure frees what it holds.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        self.shared.windows.borrow_mut().clear();
    }

    // --- Popups -----------------------------------------------------------------------------

    /// The origin of the client area on the screen.
    fn client_on_screen(&self) -> PointI {
        let mut p = POINT::default();
        // SAFETY: our window.
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut p);
        }
        PointI::new(p.x, p.y)
    }

    /// The part of the screen at `at` (screen coordinates) that popups stay in.
    fn area_at(at: PointI) -> RectI {
        // SAFETY: plain queries.
        unsafe {
            let monitor = MonitorFromPoint(POINT { x: at.x, y: at.y }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let r = info.rcWork;
            RectI::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
        }
    }

    /// Where a popup's window goes in window coordinates, and the logical size of its box.
    fn popup_placement(&self, params: &PopupParams) -> (PointI, SizeI) {
        let s = self.shared.scale.get();
        let margin = PointI::new(
            s.px(params.geometry.x as f32),
            s.px(params.geometry.y as f32),
        );
        if let Some(position) = params.position.filter(|_| params.kind == PopupKind::Bubble) {
            let origin = PointI::new(s.px(position.x as f32), s.px(position.y as f32));
            let size = SizeI::new(params.geometry.width, params.geometry.height);
            return (origin, size);
        }
        let on_screen = self.client_on_screen();
        let anchor = params.anchor;
        let center = PointI::new(
            on_screen.x + anchor.x + anchor.width / 2,
            on_screen.y + anchor.y + anchor.height / 2,
        );
        let area = Self::area_at(center).translate(-on_screen.x, -on_screen.y);
        let size = SizeI::new(
            s.px(params.wanted.width as f32),
            s.px(params.wanted.height as f32),
        );
        let gap = s.px(params.gap as f32);
        let b = place_popup(params.placement, anchor, gap, size, area);
        let granted = SizeI::new(
            params.wanted.width.min(s.logical_floor(b.width)).max(1),
            params.wanted.height.min(s.logical_floor(b.height)).max(1),
        );
        (PointI::new(b.x - margin.x, b.y - margin.y), granted)
    }

    pub fn show_popup(&mut self, params: PopupParams) {
        if self.popups.contains_key(&params.id) {
            return self.update_popup(params);
        }
        let id = params.id;
        let owner = params
            .parent
            .and_then(|p| self.popups.get(&p))
            .map_or(self.hwnd, |p| p.hwnd);
        let data = Box::into_raw(WindowData::new(
            self.shared.clone(),
            Some((id, params.kind)),
        ));
        let ex_style = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP;
        // SAFETY: the class is registered; `data` goes to the window, which frees it.
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                CLASS,
                &HSTRING::new(),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                params.buffer.width.max(1),
                params.buffer.height.max(1),
                Some(owner),
                None,
                Some(instance()),
                Some(data.cast()),
            )
        };
        let Ok(hwnd) = hwnd else {
            // SAFETY: the window didn't take it.
            drop(unsafe { Box::from_raw(data) });
            self.shared.push(WindowEvent::PopupDismissed(id));
            return;
        };
        self.shared.windows.borrow_mut().push(hwnd);
        self.popups.insert(
            id,
            Popup {
                hwnd,
                params,
                origin: PointI::default(),
            },
        );
        self.place_popup(id);
        // SAFETY: our window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }

    /// Puts the popup where it goes and tells the core.
    fn place_popup(&mut self, id: PopupId) {
        let Some(p) = self.popups.get(&id) else {
            return;
        };
        let (origin, size) = self.popup_placement(&p.params);
        let on_screen = self.client_on_screen();
        let buffer = p.params.buffer;
        let hwnd = p.hwnd;
        // SAFETY: our windows.
        unsafe {
            if let Some(data) = WindowData::of(hwnd) {
                data.origin.set(origin);
                data.kind.set(p.params.kind);
            }
            let _ = SetWindowPos(
                hwnd,
                None,
                on_screen.x + origin.x,
                on_screen.y + origin.y,
                buffer.width.max(1),
                buffer.height.max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if let Some(p) = self.popups.get_mut(&id) {
            p.origin = origin;
        }
        self.shared
            .push(WindowEvent::PopupConfigured { id, origin, size });
    }

    pub fn update_popup(&mut self, params: PopupParams) {
        let id = params.id;
        let Some(p) = self.popups.get_mut(&id) else {
            return;
        };
        let old = std::mem::replace(&mut p.params, params.clone());
        let moved = old.anchor != params.anchor
            || old.wanted != params.wanted
            || old.placement != params.placement
            || old.gap != params.gap
            || old.position != params.position
            || old.geometry != params.geometry
            || old.buffer != params.buffer
            || old.logical != params.logical
            || old.kind != params.kind;
        if moved {
            self.place_popup(id);
        }
    }

    pub fn close_popup(&mut self, id: PopupId) {
        for child in self.popups_attached_to(id) {
            self.close_popup(child);
        }
        let Some(p) = self.popups.remove(&id) else {
            return;
        };
        self.shared.windows.borrow_mut().retain(|w| *w != p.hwnd);
        // SAFETY: our window; the window procedure frees what it holds.
        unsafe {
            let _ = DestroyWindow(p.hwnd);
        }
    }

    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.popups
            .iter()
            .filter(|(_, p)| p.params.parent == Some(id))
            .map(|(child, _)| *child)
            .collect()
    }

    /// What tells the core that popups closed, the last ones first.
    fn dismissals(&self, kinds: &[PopupKind]) -> Vec<WindowEvent> {
        let mut ids: Vec<(usize, PopupId)> = self
            .popups
            .iter()
            .filter(|(_, p)| kinds.contains(&p.params.kind))
            .map(|(&id, _)| (self.popup_depth(id), id))
            .collect();
        ids.sort_by_key(|(depth, id)| (std::cmp::Reverse(*depth), id.0));
        ids.into_iter()
            .map(|(_, id)| WindowEvent::PopupDismissed(id))
            .collect()
    }

    fn popup_depth(&self, mut id: PopupId) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.popups.get(&id).and_then(|p| p.params.parent) {
            depth += 1;
            id = parent;
        }
        depth
    }
}
