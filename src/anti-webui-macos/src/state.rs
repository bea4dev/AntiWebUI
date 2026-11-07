//! One window on the main thread: its `NSWindow` and view, and what the handlers ask of it (see
//! "ウィンドウ" in `docs/design/38-platform-macos.md`). What is drawn when is in `Frames`, on
//! whichever thread draws the window.

use crate::frames::{Frames, content_layer};
use crate::popup::Popup;
use crate::remote::{ThreadWaker, Ticker};
use crate::view::{Shared, View, WindowDelegate};
use crate::{Error, ParentLink, WindowOptions};
use anti_webui_core::event::{ClipboardKind, WindowEvent};
use anti_webui_core::geometry::Scale;
use anti_webui_core::platform::{CursorIcon, ImeState, PopupId};
use anti_webui_core::window::{DecorationMode, Decorations};
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSPasteboard, NSPasteboardTypeString, NSWindow,
    NSWindowButton, NSWindowOrderingMode, NSWindowStyleMask, NSWindowTabbingMode,
    NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

thread_local! {
    /// Where the next window's top left corner goes, after the first one.
    static CASCADE: Cell<Option<NSPoint>> = const { Cell::new(None) };
}

pub(crate) struct State {
    pub mtm: MainThreadMarker,
    pub window: Retained<NSWindow>,
    pub view: Retained<View>,
    _delegate: Retained<WindowDelegate>,
    pub shared: Rc<Shared>,
    pub scale: Scale,
    pub popups: HashMap<PopupId, Popup>,
}

impl State {
    /// Opens a window, and what its frames are kept with. `thread` is the run loop of the thread
    /// that draws it, if not this one: the display link comes there.
    pub fn new(
        mtm: MainThreadMarker,
        options: &WindowOptions,
        thread: Option<Arc<ThreadWaker>>,
    ) -> Result<(Self, Frames), Error> {
        let client = options.decorations == DecorationMode::Client;
        let mut style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        if client {
            style |= NSWindowStyleMask::FullSizeContentView;
        }
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(options.width.max(1) as f64, options.height.max(1) as f64),
        );
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Only `Session::close` closes it, and it stays ours until then.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&options.title));
        window.setTabbingMode(NSWindowTabbingMode::Disallowed);
        window.setAcceptsMouseMovedEvents(true);
        if client {
            window.setTitlebarAppearsTransparent(true);
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            for b in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(button) = window.standardWindowButton(b) {
                    button.setHidden(true);
                }
            }
        }

        let shared = Rc::new(Shared::new());
        shared
            .windows
            .borrow_mut()
            .push(objc2::rc::Weak::from_retained(&window));
        let view = View::new(mtm, View::ivars_new(shared.clone(), false), rect);
        window.setContentView(Some(&view));
        window.makeFirstResponder(Some(&view));
        let delegate = WindowDelegate::new(mtm, shared.clone());
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let parent = options
            .parent
            .and_then(|p| NSApplication::sharedApplication(mtm).windowWithWindowNumber(p.0));
        match CASCADE.with(Cell::get) {
            None => {
                window.center();
                CASCADE.with(|c| c.set(Some(window.cascadeTopLeftFromPoint(NSPoint::ZERO))));
            }
            Some(at) => CASCADE.with(|c| c.set(Some(window.cascadeTopLeftFromPoint(at)))),
        }
        if let Some(parent) = &parent {
            unsafe { parent.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        }
        window.makeKeyAndOrderFront(None);
        if options.fullscreen {
            window.toggleFullScreen(None);
        }

        let display_link = view
            .respondsToSelector(sel!(displayLinkWithTarget:selector:))
            .then(|| {
                let link = match &thread {
                    Some(wake) => {
                        let ticker = Ticker::new(shared.signals.clone(), wake.clone());
                        unsafe {
                            view.displayLinkWithTarget_selector(&ticker, sel!(displayLinkFired:))
                        }
                    }
                    None => {
                        let link = unsafe {
                            view.displayLinkWithTarget_selector(&view, sel!(displayLinkFired:))
                        };
                        unsafe {
                            link.addToRunLoop_forMode(
                                &NSRunLoop::mainRunLoop(),
                                NSRunLoopCommonModes,
                            )
                        };
                        link
                    }
                };
                link.setPaused(true);
                link
            });
        let layer = content_layer();
        if let Some(backing) = view.layer() {
            backing.addSublayer(&layer);
        }

        let decorations = if client {
            Decorations::Client
        } else {
            Decorations::Server
        };
        shared.push(WindowEvent::Decorations(decorations));
        let frames = Frames::new(shared.signals.clone(), display_link, layer);
        let state = Self {
            mtm,
            window,
            view,
            _delegate: delegate,
            shared,
            scale: Scale::ONE,
            popups: HashMap::new(),
        };
        state.shared.zoomed.set(state.window.isZoomed());
        state.view.tell_size();
        state.register_drags();
        Ok((state, frames))
    }

    pub fn parent_link(&self) -> ParentLink {
        ParentLink(self.window.windowNumber())
    }

    /// What came for the window since the last time, with what it says about the window taken in.
    pub fn take_events(&mut self) -> Vec<WindowEvent> {
        let mut events = self.shared.take_events();
        for e in &events {
            if let WindowEvent::Resized { scale, .. } = e {
                if *scale != self.scale {
                    self.rescale_popups(*scale);
                }
                self.scale = *scale;
            }
        }
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
        if self.shared.dismiss_all.take() {
            self.shared.dismiss_menus.set(false);
            events.extend(self.dismissals(&[
                anti_webui_core::platform::PopupKind::Menu,
                anti_webui_core::platform::PopupKind::Tooltip,
            ]));
        } else if self.shared.dismiss_menus.take() {
            events.extend(self.dismissals(&[anti_webui_core::platform::PopupKind::Menu]));
        }
        events
    }

    /// Whether there is something to hand to the window's handler.
    pub fn has_pending(&self) -> bool {
        self.shared.has_events() || self.shared.dismiss_menus.get() || self.shared.dismiss_all.get()
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        if self.shared.cursor.replace(icon) != icon && self.shared.pointer_inside.get() {
            self.shared.apply_cursor();
        }
    }

    pub fn set_ime(&mut self, ime: Option<ImeState>) {
        let old = self.shared.ime.replace(ime.clone());
        let context = self.view.inputContext();
        match (&old, &ime) {
            (Some(_), None) => {
                if !self.shared.marked.borrow().is_empty() {
                    self.shared.clear_marked();
                    if let Some(c) = &context {
                        c.discardMarkedText();
                    }
                }
            }
            (Some(a), Some(b)) if a.cursor_rect == b.cursor_rect => return,
            _ => {}
        }
        if let Some(c) = &context {
            c.invalidateCharacterCoordinates();
        }
    }

    pub fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
        // There is no primary selection on macOS.
        if kind != ClipboardKind::Clipboard {
            return;
        }
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        board.setString_forType(&NSString::from_str(&text), unsafe {
            NSPasteboardTypeString
        });
    }

    pub fn request_paste(&mut self, kind: ClipboardKind) {
        if kind != ClipboardKind::Clipboard {
            return;
        }
        let board = NSPasteboard::generalPasteboard();
        let text = board.stringForType(unsafe { NSPasteboardTypeString });
        if let Some(text) = text {
            self.shared.push(WindowEvent::Paste {
                kind,
                text: text.to_string(),
            });
        }
    }

    pub fn set_title(&mut self, title: &str) {
        self.window.setTitle(&NSString::from_str(title));
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        let is = self
            .window
            .styleMask()
            .contains(NSWindowStyleMask::FullScreen);
        if is != on {
            self.window.toggleFullScreen(None);
        }
    }

    pub fn start_move(&mut self) {
        let press = self.shared.last_press.borrow().clone();
        if let Some(event) = press {
            self.window.performWindowDragWithEvent(&event);
        }
    }

    pub fn minimize(&mut self) {
        self.window.miniaturize(None);
    }

    pub fn set_maximized(&mut self, on: bool) {
        if self.window.isZoomed() != on {
            self.window.zoom(None);
        }
    }

    /// Lets go of what holds the window, before it goes.
    pub fn close(&mut self) {
        let ids: Vec<PopupId> = self.popups.keys().copied().collect();
        for id in ids {
            self.close_popup(id);
        }
        self.shared.release_cursor();
        if let Some(parent) = self.window.parentWindow() {
            parent.removeChildWindow(&self.window);
        }
        self.window.setDelegate(None);
        self.window.orderOut(None);
        self.window.close();
        self.shared.windows.borrow_mut().clear();
    }
}
