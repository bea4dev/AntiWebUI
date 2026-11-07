//! What the thread that draws a window keeps of it: the size, when to draw, the display link, and
//! the layers that show our `IOSurface`s (see "提示" and "複数ウィンドウ" in
//! `docs/design/38-platform-macos.md`). This is the main thread for the windows it runs, and the
//! window's own thread for those of `spawn_window`. Core Animation's layers and display links may
//! be used from any thread; AppKit's windows and views stay on the main one, in `State`.

use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{PointI, Scale, SizeI};
use anti_webui_core::platform::{BufferId, DragOffer, PopupId, Presentation, Shown, SurfaceId};
use objc2::rc::Retained;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize, Type};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes};
use objc2_io_surface::IOSurfaceRef;
use objc2_quartz_core::{CADisplayLink, CALayer, CATransaction, kCAGravityTopLeft};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a presentation waits for the display link before frames go on without it: when the
/// window is off the screens, or the system has no display links for views (before macOS 14).
const NO_TICK_WAIT: Duration = Duration::from_millis(100);
/// How often frames come without a display link.
const NO_LINK_INTERVAL: Duration = Duration::from_millis(16);

/// Something to hand to another thread that the type system doesn't know may go there: layers,
/// display links and `IOSurface`s, which Core Animation and IOSurface allow on any thread.
pub(crate) struct Sendable<T>(pub T);

// SAFETY: see above; what is not thread-safe in itself is only used by one thread at a time.
unsafe impl<T> Send for Sendable<T> {}

impl<T> Deref for Sendable<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> Clone for Sendable<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// What the main thread tells whoever draws a window, outside of its events.
pub(crate) struct Signals {
    /// The display link came: its `timestamp` and `targetTimestamp`.
    tick: Mutex<Option<(f64, f64)>>,
    /// The window can be seen (`occlusionState`).
    visible: AtomicBool,
    /// It came back into view, and has to be drawn again in full.
    exposed: AtomicBool,
    /// AppKit resizes the window live: what is shown goes with the frame's transaction.
    live_resize: AtomicBool,
}

impl Signals {
    pub fn new() -> Self {
        Self {
            tick: Mutex::new(None),
            visible: AtomicBool::new(true),
            exposed: AtomicBool::new(false),
            live_resize: AtomicBool::new(false),
        }
    }

    pub fn set_tick(&self, tick: (f64, f64)) {
        *self.tick.lock().unwrap_or_else(|e| e.into_inner()) = Some(tick);
    }

    fn take_tick(&self) -> Option<(f64, f64)> {
        self.tick.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn has_tick(&self) -> bool {
        self.tick
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn set_visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::AcqRel) != visible && visible {
            self.exposed.store(true, Ordering::Release);
        }
    }

    pub fn visible(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    pub fn take_exposed(&self) -> bool {
        self.exposed.swap(false, Ordering::AcqRel)
    }

    pub fn set_live_resize(&self, on: bool) {
        self.live_resize.store(on, Ordering::Release);
    }

    pub fn live_resize(&self) -> bool {
        self.live_resize.load(Ordering::Acquire)
    }
}

/// A layer that shows our buffers, at the top left of a view.
pub(crate) fn content_layer() -> Sendable<Retained<CALayer>> {
    let layer = CALayer::new();
    layer.setAnchorPoint(CGPoint::new(0.0, 0.0));
    layer.setContentsGravity(unsafe { kCAGravityTopLeft });
    Sendable(layer)
}

/// A drag of ours, until the system takes it.
#[derive(Default)]
pub(crate) struct DragStart {
    pub offer: Option<DragOffer>,
    /// The size of its picture in logical pixels, and where the pointer is on it.
    pub place: Option<(SizeI, PointI)>,
    pub icon: Option<Sendable<CFRetained<IOSurfaceRef>>>,
}

pub(crate) struct Frames {
    pub signals: std::sync::Arc<Signals>,
    pub size: SizeI,
    pub scale: Scale,
    pub redraw_requested: bool,
    pub redraw_at: Option<Instant>,
    display_link: Option<Sendable<Retained<CADisplayLink>>>,
    /// The last presentation the display link hasn't come after yet.
    presented_at: Option<Instant>,
    /// When the display link last came after a presentation.
    pub last_frame: Option<Instant>,
    presentation_requests: Vec<u64>,
    pub presentations: Vec<Presentation>,
    /// What shows our buffers on the window and on each popup.
    layers: HashMap<SurfaceId, Sendable<Retained<CALayer>>>,
    buffers: HashMap<BufferId, Sendable<CFRetained<IOSurfaceRef>>>,
    next_buffer: u64,
    /// The popups, with what each is attached to.
    popups: HashMap<PopupId, Option<PopupId>>,
    pub drag: DragStart,
    /// Events made here rather than by AppKit.
    pub events: Vec<WindowEvent>,
}

impl Frames {
    pub fn new(
        signals: std::sync::Arc<Signals>,
        display_link: Option<Retained<CADisplayLink>>,
        layer: Sendable<Retained<CALayer>>,
    ) -> Self {
        Self {
            signals,
            size: SizeI::new(0, 0),
            scale: Scale::ONE,
            redraw_requested: true,
            redraw_at: None,
            display_link: display_link.map(Sendable),
            presented_at: None,
            last_frame: None,
            presentation_requests: Vec::new(),
            presentations: Vec::new(),
            layers: HashMap::from([(SurfaceId::Window, layer)]),
            buffers: HashMap::new(),
            next_buffer: 1,
            popups: HashMap::new(),
            drag: DragStart::default(),
            events: Vec::new(),
        }
    }

    /// Has the display link come on this thread's run loop, for a window drawn on a thread of its
    /// own.
    pub fn run_display_link_here(&self) {
        if let Some(link) = &self.display_link {
            unsafe {
                link.addToRunLoop_forMode(&NSRunLoop::currentRunLoop(), NSRunLoopCommonModes)
            };
        }
    }

    /// Takes in what the window's events say about it, and adds those made here.
    pub fn take_in(&mut self, mut events: Vec<WindowEvent>) -> Vec<WindowEvent> {
        for e in &events {
            match e {
                WindowEvent::Resized { size, scale } => {
                    self.size = *size;
                    self.scale = *scale;
                    self.redraw_requested = true;
                }
                WindowEvent::PopupConfigured { .. } => self.redraw_requested = true,
                _ => {}
            }
        }
        events.append(&mut self.events);
        events
    }

    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    pub fn visible(&self) -> bool {
        self.signals.visible()
    }

    /// Whether the last presentation is still waiting for the display link.
    pub fn frame_pending(&self, now: Instant) -> bool {
        self.presented_at
            .is_some_and(|t| now < t + self.tick_wait())
    }

    /// When a frame waiting for the display link goes on without it.
    pub fn frame_deadline(&self) -> Option<Instant> {
        self.presented_at.map(|t| t + self.tick_wait())
    }

    fn tick_wait(&self) -> Duration {
        match self.display_link {
            Some(_) => NO_TICK_WAIT,
            None => NO_LINK_INTERVAL,
        }
    }

    /// Takes in the display link, if it came: the frame presented before it is on the screen.
    /// Whether it ended a wait for one.
    pub fn take_tick(&mut self) -> bool {
        let Some((timestamp, target)) = self.signals.take_tick() else {
            return false;
        };
        self.last_frame = Some(Instant::now());
        if self.presented_at.take().is_none() {
            return false;
        }
        let shown = Shown {
            time_ns: (timestamp * 1e9) as i64,
            refresh_ns: ((target - timestamp).max(0.0) * 1e9) as u32,
            flags: 0,
        };
        for token in self.presentation_requests.drain(..) {
            self.presentations.push(Presentation {
                token,
                shown: Some(shown),
            });
        }
        true
    }

    pub fn request_presentation(&mut self, token: u64) {
        self.presentation_requests.push(token);
    }

    /// A frame went to the window.
    pub fn presented(&mut self) {
        self.presented_at = Some(Instant::now());
        if let Some(link) = &self.display_link {
            link.setPaused(false);
        }
    }

    /// Stops the display link while nothing waits for it.
    pub fn settle_display_link(&self) {
        if let Some(link) = &self.display_link
            && self.presented_at.is_none()
            && !link.isPaused()
        {
            link.setPaused(true);
        }
    }

    /// Lets go of what is on this thread, before the window goes.
    pub fn close(&mut self) {
        if let Some(link) = self.display_link.take() {
            link.invalidate();
        }
        self.layers.clear();
        self.buffers.clear();
    }

    // --- Popups -----------------------------------------------------------------------------

    /// A new popup, and the layer that is to show it.
    pub fn add_popup(
        &mut self,
        id: PopupId,
        parent: Option<PopupId>,
    ) -> Option<Sendable<Retained<CALayer>>> {
        if self.popups.insert(id, parent).is_some() {
            return None;
        }
        let layer = content_layer();
        self.layers.insert(SurfaceId::Popup(id), layer.clone());
        Some(layer)
    }

    pub fn has_popups(&self) -> bool {
        !self.popups.is_empty()
    }

    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.popups
            .iter()
            .filter(|(_, parent)| **parent == Some(id))
            .map(|(child, _)| *child)
            .collect()
    }

    pub fn remove_popup(&mut self, id: PopupId) {
        for child in self.popups_attached_to(id) {
            self.remove_popup(child);
        }
        self.popups.remove(&id);
        self.layers.remove(&SurfaceId::Popup(id));
    }

    // --- Our buffers ------------------------------------------------------------------------

    /// Whether frames come as our `IOSurface`s, not through a GPU surface on the view's layer.
    pub fn has_buffers(&self) -> bool {
        !self.buffers.is_empty()
    }

    pub fn add_buffer(&mut self, surface: &IOSurfaceRef) -> BufferId {
        let id = BufferId(self.next_buffer);
        self.next_buffer += 1;
        self.buffers.insert(id, Sendable(surface.retain()));
        id
    }

    pub fn remove_buffer(&mut self, id: BufferId) {
        self.buffers.remove(&id);
    }

    /// Whether the surface is shown with a layer of ours, which is told whether it is opaque.
    pub fn set_opaque(&self, surface: SurfaceId, opaque: bool) -> bool {
        let Some(layer) = self.layers.get(&surface).filter(|_| self.has_buffers()) else {
            return false;
        };
        if layer.isOpaque() != opaque {
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            layer.setOpaque(opaque);
            CATransaction::commit();
        }
        true
    }

    /// Shows a buffer on a surface. Core Animation has no use for what changed.
    pub fn present(&mut self, surface: SurfaceId, id: BufferId) {
        let Some(buffer) = self.buffers.get(&id) else {
            return;
        };
        if surface == SurfaceId::DragIcon {
            self.drag.icon = Some(buffer.clone());
            return;
        }
        let Some(layer) = self.layers.get(&surface) else {
            return;
        };
        let s = self.scale.as_f64();
        let size = CGSize::new(buffer.width() as f64 / s, buffer.height() as f64 / s);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let contents: &IOSurfaceRef = &buffer.0;
        unsafe { layer.setContents(Some(contents.as_ref())) };
        layer.setContentsScale(s);
        layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), size));
        CATransaction::commit();
        // In a live resize, it goes with AppKit's transaction of the window's new frame.
        if !self.signals.live_resize() {
            CATransaction::flush();
        }
        if surface == SurfaceId::Window {
            self.presented();
        }
    }
}
