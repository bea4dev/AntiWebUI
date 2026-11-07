//! The view and the window's delegate: what AppKit calls them with becomes `WindowEvent`s in the
//! window's inbox, which the session hands to the window's handler (see "イベントループ", "入力"
//! and "IME" in `docs/design/37-platform-macos.md`).

use crate::app;
use crate::dnd::Dnd;
use crate::frames::Signals;
use crate::input::{button_of, pinch_phase, scroll_of};
use crate::keyboard::{byte_to_utf16, key_of, modifier_down, modifiers_of, text_of, utf16_to_byte};
use anti_webui_core::event::{ImeUpdate, KeyEvent, Modifiers, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, PointI, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, ImeState};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{
    NSCursor, NSDragOperation, NSDraggingContext, NSDraggingDestination, NSDraggingInfo,
    NSDraggingSession, NSDraggingSource, NSEvent, NSResponder, NSTextInputClient, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowDelegate, NSWindowOcclusionState,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSNotification, NSPoint,
    NSRange, NSRect, NSSize, NSString, NSUInteger,
};
use objc2_quartz_core::{CADisplayLink, CALayer, CAMetalLayer, kCAGravityTopLeft};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// What a window's views and delegate share with its `State`. Each borrow ends inside the method
/// that takes it, before anything calls AppKit, which may call back in.
pub(crate) struct Shared {
    events: RefCell<Vec<WindowEvent>>,
    pub scale: Cell<Scale>,
    /// What `Resized` last said, so that it comes once for each change.
    told: Cell<Option<(SizeI, Scale)>>,
    pub modifiers: Cell<Modifiers>,
    /// What the core said about the text being edited, if anything is.
    pub ime: RefCell<Option<ImeState>>,
    /// Text being composed, and the selection in it (UTF-16).
    pub marked: RefCell<String>,
    marked_selection: Cell<NSRange>,
    /// What the input method did during the `keyDown:` going on.
    capture: RefCell<Option<Capture>>,
    /// Keys the input method took: their releases aren't the core's business either.
    ime_keys: RefCell<Vec<u16>>,
    pub cursor: Cell<CursorIcon>,
    cursor_hidden: Cell<bool>,
    /// The pointer is over one of the window's views.
    pub pointer_inside: Cell<bool>,
    /// The last press of a mouse button, to move the window from.
    pub last_press: RefCell<Option<Retained<NSEvent>>>,
    /// The last press or move of the mouse, to start drags from.
    pub last_mouse: RefCell<Option<Retained<NSEvent>>>,
    /// A press of the primary button with Control, which is a press of the secondary one.
    control_click: Cell<bool>,
    /// What whoever draws the window is told: the display link, whether it can be seen.
    pub signals: Arc<Signals>,
    pub zoomed: Cell<bool>,
    /// What closes popups: the window lost the keyboard, or it moved.
    pub dismiss_menus: Cell<bool>,
    pub dismiss_all: Cell<bool>,
    /// The windows of the views, to tell whether the pointer left all of them.
    pub windows: RefCell<Vec<Weak<NSWindow>>>,
    /// Drags over the window, and the drag from it.
    pub dnd: RefCell<Dnd>,
}

#[derive(Default)]
struct Capture {
    inserted: String,
    marked_changed: bool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            events: RefCell::default(),
            scale: Cell::new(Scale::ONE),
            told: Cell::new(None),
            modifiers: Cell::default(),
            ime: RefCell::default(),
            marked: RefCell::default(),
            marked_selection: Cell::new(NSRange::new(0, 0)),
            capture: RefCell::default(),
            ime_keys: RefCell::default(),
            cursor: Cell::default(),
            cursor_hidden: Cell::new(false),
            pointer_inside: Cell::new(false),
            last_press: RefCell::default(),
            last_mouse: RefCell::default(),
            control_click: Cell::new(false),
            signals: Arc::new(Signals::new()),
            zoomed: Cell::new(false),
            dismiss_menus: Cell::new(false),
            dismiss_all: Cell::new(false),
            windows: RefCell::default(),
            dnd: RefCell::default(),
        }
    }

    /// Puts an event in the inbox, and makes the loop go around for it: AppKit calls some of
    /// these from timers and notifications, while the loop waits for events.
    pub fn push(&self, event: WindowEvent) {
        self.events.borrow_mut().push(event);
        if let Some(mtm) = MainThreadMarker::new() {
            app::wake(mtm);
        }
    }

    pub fn take_events(&self) -> Vec<WindowEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    pub fn has_events(&self) -> bool {
        !self.events.borrow().is_empty()
    }

    /// Forgets the text being composed, which the core dropped.
    pub fn clear_marked(&self) {
        self.marked.borrow_mut().clear();
        self.marked_selection.set(NSRange::new(0, 0));
    }

    /// Whether the pointer is over one of the windows that take input, going by where it is now.
    fn pointer_over_windows(&self) -> bool {
        let at = NSEvent::mouseLocation();
        self.windows.borrow().iter().any(|w| {
            w.load().is_some_and(|w| {
                let r = w.contentRectForFrameRect(w.frame());
                w.isVisible()
                    && !w.ignoresMouseEvents()
                    && at.x >= r.origin.x
                    && at.y >= r.origin.y
                    && at.x < r.origin.x + r.size.width
                    && at.y < r.origin.y + r.size.height
            })
        })
    }

    /// Shows the cursor the core asked for.
    pub fn apply_cursor(&self) {
        let icon = self.cursor.get();
        if icon == CursorIcon::None {
            if !self.cursor_hidden.replace(true) {
                NSCursor::hide();
            }
            return;
        }
        if self.cursor_hidden.replace(false) {
            NSCursor::unhide();
        }
        cursor_of(icon).set();
    }

    /// Shows the cursor again if the core had hidden it, when the pointer leaves.
    pub fn release_cursor(&self) {
        if self.cursor_hidden.replace(false) {
            NSCursor::unhide();
        }
    }

    /// The selection the core said, in UTF-16 units of the text around it.
    fn selection_utf16(&self) -> Option<NSRange> {
        let ime = self.ime.borrow();
        let ime = ime.as_ref()?;
        let (a, b) = (ime.cursor.min(ime.anchor), ime.cursor.max(ime.anchor));
        let a = byte_to_utf16(&ime.surrounding, a);
        let b = byte_to_utf16(&ime.surrounding, b);
        Some(NSRange::new(a, b - a))
    }
}

pub(crate) fn cursor_of(icon: CursorIcon) -> Retained<NSCursor> {
    match icon {
        CursorIcon::Default | CursorIcon::None => NSCursor::arrowCursor(),
        CursorIcon::Pointer => NSCursor::pointingHandCursor(),
        CursorIcon::Text => NSCursor::IBeamCursor(),
        CursorIcon::NotAllowed => NSCursor::operationNotAllowedCursor(),
        CursorIcon::Grab | CursorIcon::Move => NSCursor::openHandCursor(),
        CursorIcon::Grabbing => NSCursor::closedHandCursor(),
        #[allow(deprecated)]
        CursorIcon::EwResize => NSCursor::resizeLeftRightCursor(),
        #[allow(deprecated)]
        CursorIcon::NsResize => NSCursor::resizeUpDownCursor(),
        CursorIcon::Crosshair => NSCursor::crosshairCursor(),
        CursorIcon::Copy => NSCursor::dragCopyCursor(),
    }
}

/// The text of what AppKit hands the input methods' callbacks: a string, or an attributed one.
fn string_of(object: &AnyObject) -> String {
    if let Some(s) = object.downcast_ref::<NSAttributedString>() {
        s.string().to_string()
    } else if let Some(s) = object.downcast_ref::<NSString>() {
        s.to_string()
    } else {
        String::new()
    }
}

pub(crate) struct ViewIvars {
    pub shared: Rc<Shared>,
    /// Where the view is in window coordinates (physical): the origin of a popup's.
    pub origin: Cell<PointI>,
    /// It shows a popup, not the window.
    pub popup: bool,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // SAFETY: NSView has no requirements for subclasses, and this has no `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AntiWebUIView"]
    #[ivars = ViewIvars]
    pub(crate) struct View;

    impl View {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            !self.ivars().popup
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<CALayer> {
            let layer = CAMetalLayer::new();
            // What is left of the last frame stays where it was while a new size is drawn.
            layer.setContentsGravity(unsafe { kCAGravityTopLeft });
            layer.setNeedsDisplayOnBoundsChange(false);
            // Says so while it has frames: those come on a layer above it, if they are ours.
            layer.setOpaque(false);
            Retained::into_super(layer)
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::CursorUpdate
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().tracking.borrow_mut() = Some(area);
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.tell_size();
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidChangeBackingProperties] };
            if let Some(layer) = self.layer() {
                layer.setContentsScale(self.backing_factor());
            }
            self.tell_size();
        }

        #[unsafe(method(viewWillStartLiveResize))]
        fn view_will_start_live_resize(&self) {
            let _: () = unsafe { msg_send![super(self), viewWillStartLiveResize] };
            // The frame of the window and what is in it change together.
            self.ivars().shared.signals.set_live_resize(true);
            if let Some(layer) = self.metal_layer() {
                layer.setPresentsWithTransaction(true);
            }
        }

        #[unsafe(method(viewDidEndLiveResize))]
        fn view_did_end_live_resize(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidEndLiveResize] };
            self.ivars().shared.signals.set_live_resize(false);
            if let Some(layer) = self.metal_layer() {
                layer.setPresentsWithTransaction(false);
            }
        }

        // --- The pointer --------------------------------------------------------------------

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.moved(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.moved(event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.moved(event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.moved(event);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.ivars().shared.pointer_inside.set(true);
            self.ivars().shared.apply_cursor();
            self.moved(event);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let shared = &self.ivars().shared;
            // Onto a popup of the window, or back from one.
            if shared.pointer_over_windows() {
                return;
            }
            shared.pointer_inside.set(false);
            shared.release_cursor();
            shared.push(WindowEvent::PointerLeft);
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            self.ivars().shared.apply_cursor();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.button(event, true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.button(event, false);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.button(event, true);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.button(event, false);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.button(event, true);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.button(event, false);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let shared = &self.ivars().shared;
            let modifiers = self.take_modifiers(event);
            let scroll = scroll_of(
                event.hasPreciseScrollingDeltas(),
                event.phase(),
                event.momentumPhase(),
                (event.scrollingDeltaX(), event.scrollingDeltaY()),
                shared.scale.get().as_f64(),
            );
            if let Some((delta_x, delta_y, source, phase)) = scroll {
                shared.push(WindowEvent::Scroll {
                    position: self.position(event),
                    delta_x,
                    delta_y,
                    modifiers,
                    source,
                    phase,
                    time: millis(event),
                });
            }
        }

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify_with_event(&self, event: &NSEvent) {
            let modifiers = self.take_modifiers(event);
            if let Some(phase) = pinch_phase(event.phase()) {
                self.ivars().shared.push(WindowEvent::Pinch {
                    position: self.position(event),
                    scale: (1.0 + event.magnification()) as f32,
                    modifiers,
                    phase,
                });
            }
        }

        // --- Keys ---------------------------------------------------------------------------

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.key_pressed(event);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            let shared = &self.ivars().shared;
            let modifiers = self.take_modifiers(event);
            let code = event.keyCode();
            let mut taken = shared.ime_keys.borrow_mut();
            if let Some(i) = taken.iter().position(|k| *k == code) {
                taken.swap_remove(i);
                return;
            }
            drop(taken);
            let ignoring = event.charactersIgnoringModifiers().map(|s| s.to_string());
            shared.push(WindowEvent::Key(KeyEvent {
                key: key_of(code, ignoring.as_deref().unwrap_or("")),
                text: None,
                modifiers,
                pressed: false,
                repeat: false,
            }));
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            let modifiers = self.take_modifiers(event);
            let code = event.keyCode();
            if let Some(pressed) = modifier_down(code, event.modifierFlags()) {
                self.ivars().shared.push(WindowEvent::Key(KeyEvent {
                    key: key_of(code, ""),
                    text: None,
                    modifiers,
                    pressed,
                    repeat: false,
                }));
            }
        }

        // --- Frames -------------------------------------------------------------------------

        #[unsafe(method(displayLinkFired:))]
        fn display_link_fired(&self, link: &CADisplayLink) {
            let shared = &self.ivars().shared;
            shared.signals.set_tick((link.timestamp(), link.targetTimestamp()));
            // In a live resize or a drag of ours, AppKit's loop runs instead of ours.
            let modal = self.inLiveResize() || shared.dnd.borrow().dragging();
            if !(modal && app::run_pump()) {
                app::wake(self.mtm());
            }
        }
    }

    unsafe impl NSObjectProtocol for View {}

    unsafe impl NSDraggingDestination for View {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.drag_entered(info)
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.drag_updated(info)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.drag_exited();
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag_operation(&self, _info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            true
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.drag_performed(info)
        }
    }

    unsafe impl NSDraggingSource for View {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn dragging_session_source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            self.drag_source_mask()
        }

        #[unsafe(method(draggingSession:movedToPoint:))]
        fn dragging_session_moved_to_point(&self, _session: &NSDraggingSession, _at: NSPoint) {
            app::run_pump();
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn dragging_session_ended(
            &self,
            _session: &NSDraggingSession,
            _at: NSPoint,
            operation: NSDragOperation,
        ) {
            self.drag_ended(operation);
        }
    }

    unsafe impl NSTextInputClient for View {
        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            !self.ivars().shared.marked.borrow().is_empty()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let shared = &self.ivars().shared;
            let marked = shared.marked.borrow();
            if marked.is_empty() {
                return NSRange::new(NSNotFound as NSUInteger, 0);
            }
            let start = shared.selection_utf16().map_or(0, |r| r.location);
            NSRange::new(start, marked.encode_utf16().count())
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            let shared = &self.ivars().shared;
            let Some(selection) = shared.selection_utf16() else {
                return NSRange::new(NSNotFound as NSUInteger, 0);
            };
            if shared.marked.borrow().is_empty() {
                return selection;
            }
            let inside = shared.marked_selection.get();
            NSRange::new(selection.location + inside.location, inside.length)
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, _replacement: NSRange) {
            let shared = &self.ivars().shared;
            let text = string_of(string);
            *shared.marked.borrow_mut() = text;
            shared.marked_selection.set(selected);
            let mut capture = shared.capture.borrow_mut();
            match capture.as_mut() {
                Some(c) => c.marked_changed = true,
                None => {
                    drop(capture);
                    shared.push(WindowEvent::Ime(ImeUpdate {
                        preedit: preedit(shared),
                        ..Default::default()
                    }));
                }
            }
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            let shared = &self.ivars().shared;
            let text = std::mem::take(&mut *shared.marked.borrow_mut());
            if !text.is_empty() {
                self.commit(text, true);
            }
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring_for_proposed_range(
            &self,
            _range: NSRange,
            _actual: *mut NSRange,
        ) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement: NSRange) {
            let shared = &self.ivars().shared;
            let was_marked = !std::mem::take(&mut *shared.marked.borrow_mut()).is_empty();
            self.commit(string_of(string), was_marked);
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            NSNotFound as NSUInteger
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect_for_character_range(&self, _range: NSRange, _actual: *mut NSRange) -> NSRect {
            let shared = &self.ivars().shared;
            let Some(r) = shared.ime.borrow().as_ref().map(|i| i.cursor_rect) else {
                return NSRect::ZERO;
            };
            let s = shared.scale.get().as_f64();
            let o = self.ivars().origin.get();
            let local = NSRect::new(
                NSPoint::new((r.x - o.x) as f64 / s, (r.y - o.y) as f64 / s),
                NSSize::new(r.width as f64 / s, r.height as f64 / s),
            );
            let in_window = self.convertRect_toView(local, None);
            match self.window() {
                Some(w) => w.convertRectToScreen(in_window),
                None => in_window,
            }
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            // The key goes to the core as it is, which knows what it does.
        }
    }
);

/// The text being composed, as the core takes it.
fn preedit(shared: &Shared) -> Option<(String, Option<(usize, usize)>)> {
    let marked = shared.marked.borrow();
    if marked.is_empty() {
        return None;
    }
    let r = shared.marked_selection.get();
    let a = utf16_to_byte(&marked, r.location);
    let b = utf16_to_byte(&marked, r.location + r.length);
    Some((marked.clone(), Some((a, b))))
}

fn millis(event: &NSEvent) -> u32 {
    (event.timestamp() * 1000.0) as u64 as u32
}

impl View {
    pub fn new(mtm: MainThreadMarker, ivars: ViewIvars, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);
        view
    }

    pub fn ivars_new(shared: Rc<Shared>, popup: bool) -> ViewIvars {
        ViewIvars {
            shared,
            origin: Cell::new(PointI::default()),
            popup,
            tracking: RefCell::default(),
        }
    }

    pub fn metal_layer(&self) -> Option<Retained<CAMetalLayer>> {
        self.layer()?.downcast::<CAMetalLayer>().ok()
    }

    fn backing_factor(&self) -> f64 {
        self.window().map_or(1.0, |w| w.backingScaleFactor())
    }

    /// Says the size of the window's view and its scale, when they change. In a live resize, the
    /// new size is drawn there and then.
    pub fn tell_size(&self) {
        let ivars = self.ivars();
        if ivars.popup {
            return;
        }
        let factor = self.backing_factor();
        let scale = Scale::from_f64(factor);
        let b = self.bounds().size;
        let size = SizeI::new(
            (b.width * factor).round() as i32,
            (b.height * factor).round() as i32,
        );
        let shared = &ivars.shared;
        shared.scale.set(scale);
        if shared.told.get() == Some((size, scale)) {
            return;
        }
        shared.told.set(Some((size, scale)));
        shared.push(WindowEvent::Resized { size, scale });
        if self.inLiveResize() {
            app::run_pump();
        }
    }

    /// Where an event is, in window coordinates.
    fn position(&self, event: &NSEvent) -> PointF {
        self.position_in_window(event.locationInWindow())
    }

    /// A point of the `NSWindow`'s coordinates, in the window's (physical, from the top left).
    pub(crate) fn position_in_window(&self, at: NSPoint) -> PointF {
        let p = self.convertPoint_fromView(at, None);
        let s = self.ivars().shared.scale.get().as_f64();
        let o = self.ivars().origin.get();
        PointF::new((p.x * s) as f32 + o.x as f32, (p.y * s) as f32 + o.y as f32)
    }

    fn take_modifiers(&self, event: &NSEvent) -> Modifiers {
        let m = modifiers_of(event.modifierFlags());
        self.ivars().shared.modifiers.set(m);
        m
    }

    fn moved(&self, event: &NSEvent) {
        let modifiers = self.take_modifiers(event);
        let shared = &self.ivars().shared;
        shared.pointer_inside.set(true);
        *shared.last_mouse.borrow_mut() = Some(event.retain());
        shared.push(WindowEvent::PointerMoved {
            position: self.position(event),
            modifiers,
        });
    }

    fn button(&self, event: &NSEvent, pressed: bool) {
        let modifiers = self.take_modifiers(event);
        let shared = &self.ivars().shared;
        let mut button = button_of(event.buttonNumber());
        if button == PointerButton::Primary {
            if pressed && modifiers.ctrl {
                shared.control_click.set(true);
            }
            if shared.control_click.get() {
                button = PointerButton::Secondary;
                if !pressed {
                    shared.control_click.set(false);
                }
            }
        }
        if pressed {
            *shared.last_press.borrow_mut() = Some(event.retain());
        }
        *shared.last_mouse.borrow_mut() = Some(event.retain());
        shared.push(WindowEvent::PointerButton {
            position: self.position(event),
            button,
            pressed,
            modifiers,
            time: millis(event),
        });
    }

    /// A key went down: to the input method first while text is edited, and to the core as a key
    /// unless the input method took it (see "IME" in `docs/design/37-platform-macos.md`).
    fn key_pressed(&self, event: &NSEvent) {
        let shared = &self.ivars().shared;
        let modifiers = self.take_modifiers(event);
        let code = event.keyCode();
        let ignoring = event.charactersIgnoringModifiers().map(|s| s.to_string());
        let key = key_of(code, ignoring.as_deref().unwrap_or(""));
        let repeat = event.isARepeat();
        let editing = shared.ime.borrow().is_some();
        let text = if editing && !modifiers.logo {
            let had_marked = !shared.marked.borrow().is_empty();
            *shared.capture.borrow_mut() = Some(Capture::default());
            self.interpretKeyEvents(&NSArray::from_slice(&[event]));
            let capture = shared.capture.borrow_mut().take().unwrap_or_default();
            let has_marked = !shared.marked.borrow().is_empty();
            if had_marked || has_marked || capture.marked_changed {
                if !repeat {
                    shared.ime_keys.borrow_mut().push(code);
                }
                shared.push(WindowEvent::Ime(ImeUpdate {
                    preedit: preedit(shared),
                    commit: (!capture.inserted.is_empty()).then_some(capture.inserted),
                    delete_surrounding: None,
                }));
                return;
            }
            (!capture.inserted.is_empty()).then_some(capture.inserted)
        } else {
            let characters = event.characters().map(|s| s.to_string());
            text_of(code, characters.as_deref().unwrap_or(""), modifiers)
        };
        shared.push(WindowEvent::Key(KeyEvent {
            key,
            text,
            modifiers,
            pressed: true,
            repeat,
        }));
    }

    /// Text the input method finished: part of the key going on, or on its own, like a candidate
    /// picked with the mouse.
    fn commit(&self, text: String, was_marked: bool) {
        let shared = &self.ivars().shared;
        shared.marked_selection.set(NSRange::new(0, 0));
        let mut capture = shared.capture.borrow_mut();
        match capture.as_mut() {
            Some(c) => {
                c.inserted.push_str(&text);
                c.marked_changed |= was_marked;
            }
            None => {
                drop(capture);
                shared.push(WindowEvent::Ime(ImeUpdate {
                    preedit: None,
                    commit: Some(text),
                    delete_surrounding: None,
                }));
            }
        }
    }
}

// --- The window's delegate ---------------------------------------------------------------------

define_class!(
    // SAFETY: NSObject has no requirements for subclasses, and this has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AntiWebUIWindowDelegate"]
    #[ivars = Rc<Shared>]
    pub(crate) struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        /// Whether to close is the core's to say, after the handlers of the program.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            self.ivars().push(WindowEvent::CloseRequested);
            false
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _n: &NSNotification) {
            self.ivars().push(WindowEvent::Focused(true));
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _n: &NSNotification) {
            let shared = self.ivars();
            shared.dismiss_menus.set(true);
            shared.push(WindowEvent::Focused(false));
        }

        #[unsafe(method(windowDidMove:))]
        fn window_did_move(&self, _n: &NSNotification) {
            let shared = self.ivars();
            shared.dismiss_all.set(true);
            if let Some(mtm) = MainThreadMarker::new() {
                app::wake(mtm);
            }
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, n: &NSNotification) {
            let Some(window) = window_of(n) else {
                return;
            };
            let shared = self.ivars();
            let zoomed = window.isZoomed();
            if shared.zoomed.replace(zoomed) != zoomed {
                shared.push(WindowEvent::Maximized(zoomed));
            }
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn window_did_enter_full_screen(&self, _n: &NSNotification) {
            self.ivars().push(WindowEvent::Fullscreen(true));
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn window_did_exit_full_screen(&self, _n: &NSNotification) {
            self.ivars().push(WindowEvent::Fullscreen(false));
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, n: &NSNotification) {
            let Some(window) = window_of(n) else {
                return;
            };
            let shared = self.ivars();
            let visible = window
                .occlusionState()
                .contains(NSWindowOcclusionState::Visible);
            shared.signals.set_visible(visible);
            if let Some(mtm) = MainThreadMarker::new() {
                app::wake(mtm);
            }
        }
    }
);

fn window_of(n: &NSNotification) -> Option<Retained<NSWindow>> {
    n.object()?.downcast::<NSWindow>().ok()
}

impl WindowDelegate {
    pub fn new(mtm: MainThreadMarker, shared: Rc<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}
