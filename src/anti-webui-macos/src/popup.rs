//! Popups as borderless child windows, placed by us like on X11 (see "ポップアップ" in
//! `docs/design/38-platform-macos.md`).

use crate::frames::Sendable;
use crate::state::State;
use crate::view::View;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{PopupId, PopupKind, PopupParams, place_popup};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;

pub(crate) struct Popup {
    pub window: Retained<NSWindow>,
    pub view: Retained<View>,
    pub params: PopupParams,
    /// The popup's origin in window coordinates (physical), as the core knows it.
    pub origin: PointI,
}

/// The largest logical length within `physical` pixels.
fn logical_floor(physical: i32, s: Scale) -> i32 {
    (physical as f64 / s.as_f64()).floor() as i32
}

impl State {
    /// The top left corner of the window's content on the screen, in points (AppKit's screen
    /// coordinates go up from the bottom of the main screen).
    fn content_top_left(&self) -> NSPoint {
        let frame = self.window.frame();
        let content = self.window.contentRectForFrameRect(frame);
        NSPoint::new(content.origin.x, content.origin.y + content.size.height)
    }

    /// The part of the screen a popup has to stay in, in window coordinates (physical): the screen
    /// with the anchor, but for the menu bar and the Dock.
    fn popup_area(&self, anchor: RectI) -> RectI {
        let s = self.scale.as_f64();
        let top_left = self.content_top_left();
        let center = NSPoint::new(
            top_left.x + (anchor.x + anchor.width / 2) as f64 / s,
            top_left.y - (anchor.y + anchor.height / 2) as f64 / s,
        );
        let screens = objc2_app_kit::NSScreen::screens(self.mtm);
        let screen = screens
            .iter()
            .find(|sc| {
                let f = sc.frame();
                center.x >= f.origin.x
                    && center.y >= f.origin.y
                    && center.x < f.origin.x + f.size.width
                    && center.y < f.origin.y + f.size.height
            })
            .or_else(|| self.window.screen());
        let Some(screen) = screen else {
            return RectI::new(-100_000, -100_000, 200_000, 200_000);
        };
        let v = screen.visibleFrame();
        RectI::new(
            ((v.origin.x - top_left.x) * s).round() as i32,
            ((top_left.y - (v.origin.y + v.size.height)) * s).round() as i32,
            (v.size.width * s).round() as i32,
            (v.size.height * s).round() as i32,
        )
    }

    /// Where a popup's window goes in window coordinates, and the logical size of its box.
    fn popup_placement(&self, params: &PopupParams) -> (PointI, SizeI) {
        let s = self.scale;
        let margin = PointI::new(
            s.px(params.geometry.x as f32),
            s.px(params.geometry.y as f32),
        );
        if let Some(position) = params.position.filter(|_| params.kind == PopupKind::Bubble) {
            let origin = PointI::new(s.px(position.x as f32), s.px(position.y as f32));
            let size = SizeI::new(params.geometry.width, params.geometry.height);
            return (origin, size);
        }
        let anchor = params.anchor;
        let area = self.popup_area(anchor);
        let size = SizeI::new(
            s.px(params.wanted.width as f32),
            s.px(params.wanted.height as f32),
        );
        let gap = s.px(params.gap as f32);
        let b = place_popup(params.placement, anchor, gap, size, area);
        let granted = SizeI::new(
            params.wanted.width.min(logical_floor(b.width, s)).max(1),
            params.wanted.height.min(logical_floor(b.height, s)).max(1),
        );
        (PointI::new(b.x - margin.x, b.y - margin.y), granted)
    }

    /// Opens a popup, with `layer` to show what is drawn for it.
    pub fn show_popup(&mut self, params: PopupParams, layer: Option<Sendable<Retained<CALayer>>>) {
        if self.popups.contains_key(&params.id) {
            return self.update_popup(params);
        }
        let mtm = self.mtm;
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(
                params.logical.width.max(1) as f64,
                params.logical.height.max(1) as f64,
            ),
        );
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        // The core draws the shadow.
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(params.kind == PopupKind::Tooltip);
        window.setAcceptsMouseMovedEvents(true);
        let view = View::new(mtm, View::ivars_new(self.shared.clone(), true), rect);
        window.setContentView(Some(&view));
        if let Some(backing) = view.layer() {
            backing.setContentsScale(self.scale.as_f64());
            if let Some(layer) = &layer {
                backing.addSublayer(layer);
            }
        }
        let parent = params
            .parent
            .and_then(|p| self.popups.get(&p))
            .map_or(&self.window, |p| &p.window);
        unsafe { parent.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        self.shared
            .windows
            .borrow_mut()
            .push(objc2::rc::Weak::from_retained(&window));
        let id = params.id;
        self.popups.insert(
            id,
            Popup {
                window,
                view,
                params,
                origin: PointI::default(),
            },
        );
        self.place_popup(id);
        if let Some(p) = self.popups.get(&id) {
            p.window.orderFront(None);
        }
    }

    /// Puts the popup where it goes and tells the core.
    fn place_popup(&mut self, id: PopupId) {
        let Some(p) = self.popups.get(&id) else {
            return;
        };
        let (origin, size) = self.popup_placement(&p.params);
        let s = self.scale.as_f64();
        let top_left = self.content_top_left();
        let logical = p.params.logical;
        let (w, h) = (logical.width.max(1) as f64, logical.height.max(1) as f64);
        let frame = NSRect::new(
            NSPoint::new(
                top_left.x + origin.x as f64 / s,
                top_left.y - origin.y as f64 / s - h,
            ),
            NSSize::new(w, h),
        );
        p.window.setFrame_display(frame, false);
        p.view.ivars().origin.set(origin);
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
        if old.kind != params.kind {
            p.window
                .setIgnoresMouseEvents(params.kind == PopupKind::Tooltip);
        }
        let moved = old.anchor != params.anchor
            || old.wanted != params.wanted
            || old.placement != params.placement
            || old.gap != params.gap
            || old.position != params.position
            || old.geometry != params.geometry
            || old.buffer != params.buffer
            || old.logical != params.logical;
        if moved {
            self.place_popup(id);
        }
    }

    /// Popups follow the window to a screen of another scale.
    pub(crate) fn rescale_popups(&mut self, scale: Scale) {
        for p in self.popups.values() {
            if let Some(layer) = p.view.layer() {
                layer.setContentsScale(scale.as_f64());
            }
        }
    }

    pub fn close_popup(&mut self, id: PopupId) {
        for child in self.popups_attached_to(id) {
            self.close_popup(child);
        }
        let Some(p) = self.popups.remove(&id) else {
            return;
        };
        if let Some(parent) = p.window.parentWindow() {
            parent.removeChildWindow(&p.window);
        }
        p.window.orderOut(None);
        p.window.close();
        let gone =
            |w: &objc2::rc::Weak<NSWindow>| w.load().is_none_or(|w| std::ptr::eq(&*w, &*p.window));
        self.shared.windows.borrow_mut().retain(|w| !gone(w));
    }

    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.popups
            .iter()
            .filter(|(_, p)| p.params.parent == Some(id))
            .map(|(child, _)| *child)
            .collect()
    }

    /// What tells the core that popups closed, the last ones first, as `popup_done` does on
    /// Wayland.
    pub(crate) fn dismissals(&self, kinds: &[PopupKind]) -> Vec<WindowEvent> {
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
