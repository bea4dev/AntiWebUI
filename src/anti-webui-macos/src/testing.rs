//! Input for tests: events made here and posted to the program's own queue, so that they come in
//! through `sendEvent:` like the user's, without the permissions that posting to the system needs.
//! Coordinates are logical, from the top left of the window's content.

use crate::view::View;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent,
    NSEventModifierFlags, NSEventType, NSPasteboard, NSPasteboardItem, NSPasteboardWriting,
    NSTextInputClient, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::NSArray;
use objc2_foundation::{NSPoint, NSRange, NSSize, NSString};
use objc2_io_surface::IOSurfaceRef;

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("tests drive AppKit from the main thread")
}

thread_local! {
    static TARGET: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Has what follows drive the window of this title, or the first one again with `None`.
pub fn use_window(title: Option<&str>) {
    TARGET.with(|t| *t.borrow_mut() = title.map(str::to_owned));
}

/// The window tests drive: the program's first with a title bar, or the one of [`use_window`].
pub fn window() -> Option<Retained<NSWindow>> {
    let app = NSApplication::sharedApplication(mtm());
    let title = TARGET.with(|t| t.borrow().clone());
    app.windows().iter().find(|w| {
        w.styleMask().contains(NSWindowStyleMask::Titled)
            && w.isVisible()
            && title.as_ref().is_none_or(|t| w.title().to_string() == *t)
    })
}

/// How many windows are on the screen, popups included.
pub fn visible_windows() -> usize {
    let app = NSApplication::sharedApplication(mtm());
    app.windows().iter().filter(|w| w.isVisible()).count()
}

fn view() -> Option<Retained<View>> {
    window()?.contentView()?.downcast::<View>().ok()
}

fn post(event: Option<Retained<NSEvent>>) {
    if let Some(e) = event {
        NSApplication::sharedApplication(mtm()).postEvent_atStart(&e, false);
    }
}

fn mouse(kind: NSEventType, x: f64, y: f64, flags: NSEventModifierFlags) {
    let Some((left, top)) = content_top_left() else {
        return;
    };
    let at = NSPoint::new(left + x, top - y);
    // The front one of the windows there that take input, as AppKit would pick: a popup over the
    // window gets its own events.
    let app = NSApplication::sharedApplication(mtm());
    let target = app.orderedWindows().iter().find(|w| {
        let f = w.frame();
        w.isVisible()
            && !w.ignoresMouseEvents()
            && at.x >= f.origin.x
            && at.y >= f.origin.y
            && at.x < f.origin.x + f.size.width
            && at.y < f.origin.y + f.size.height
    });
    let Some(w) = target.or_else(window) else {
        return;
    };
    let f = w.frame();
    post(NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        NSPoint::new(at.x - f.origin.x, at.y - f.origin.y),
        flags,
        objc2_foundation::NSProcessInfo::processInfo().systemUptime(),
        w.windowNumber(),
        None,
        0,
        1,
        if kind == NSEventType::LeftMouseDown { 1.0 } else { 0.0 },
    ));
}

pub fn move_to(x: f64, y: f64) {
    mouse(NSEventType::MouseMoved, x, y, NSEventModifierFlags::empty());
}

/// Presses and lets go of the primary button, with the modifiers of `flags`.
pub fn click_with(x: f64, y: f64, flags: NSEventModifierFlags) {
    move_to(x, y);
    mouse(NSEventType::LeftMouseDown, x, y, flags);
    mouse(NSEventType::LeftMouseUp, x, y, flags);
}

pub fn click(x: f64, y: f64) {
    click_with(x, y, NSEventModifierFlags::empty());
}

/// Presses and lets go of a key: `code` is its `kVK_*`, `chars` what it types.
pub fn key(code: u16, chars: &str, flags: NSEventModifierFlags) {
    let Some(w) = window() else {
        return;
    };
    let chars = NSString::from_str(chars);
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        post(NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            flags,
            objc2_foundation::NSProcessInfo::processInfo().systemUptime(),
            w.windowNumber(),
            None,
            &chars,
            &chars,
            false,
            code,
        ));
    }
}

/// What an input method does as it composes and commits text, straight into the view.
pub fn compose(text: &str) {
    if let Some(v) = view() {
        let s = NSString::from_str(text);
        let len = text.encode_utf16().count();
        unsafe {
            v.setMarkedText_selectedRange_replacementRange(
                &s,
                NSRange::new(len, 0),
                NSRange::new(usize::MAX, 0),
            )
        };
    }
}

pub fn commit(text: &str) {
    if let Some(v) = view() {
        let s = NSString::from_str(text);
        let s: &AnyObject = &s;
        unsafe { v.insertText_replacementRange(s, NSRange::new(usize::MAX, 0)) };
    }
}

/// Where the input method would put its candidates: the caret, in screen coordinates.
pub fn caret_on_screen() -> Option<(f64, f64, f64, f64)> {
    let v = view()?;
    let r = unsafe {
        v.firstRectForCharacterRange_actualRange(NSRange::new(0, 0), std::ptr::null_mut())
    };
    Some((r.origin.x, r.origin.y, r.size.width, r.size.height))
}

/// The top left of the window's content on the screen, in AppKit's coordinates.
pub fn content_top_left() -> Option<(f64, f64)> {
    let w = window()?;
    let c = w.contentRectForFrameRect(w.frame());
    Some((c.origin.x, c.origin.y + c.size.height))
}

/// Resizes the window's content, in logical pixels.
pub fn resize(width: f64, height: f64) {
    if let Some(w) = window() {
        w.setContentSize(NSSize::new(width, height));
    }
}

/// Moves the window by logical pixels, right and down.
pub fn move_by(dx: f64, dy: f64) {
    if let Some(w) = window() {
        let f = w.frame();
        w.setFrameOrigin(NSPoint::new(f.origin.x + dx, f.origin.y - dy));
    }
}

/// What the close button does.
pub fn press_close_button() {
    if let Some(w) = window() {
        w.performClose(None);
    }
}

/// The scale of the window's screen.
pub fn backing_scale() -> f64 {
    window().map_or(1.0, |w| w.backingScaleFactor())
}

/// The size in pixels of what the window shows: the `IOSurface` on the layer above the view's, or
/// the view's own drawable.
pub fn layer_size() -> Option<(f64, f64)> {
    let view = view()?;
    let content = unsafe { view.layer()?.sublayers() }.and_then(|l| l.firstObject());
    if let Some(contents) = content.and_then(|l| unsafe { l.contents() }) {
        let surface = as_iosurface(&contents)?;
        return Some((surface.width() as f64, surface.height() as f64));
    }
    let s = view.metal_layer()?.drawableSize();
    Some((s.width, s.height))
}

/// Whether the window shows `IOSurface`s of ours, and the layer they are on has its origin at the
/// top left of the view.
pub fn shows_iosurfaces() -> bool {
    let Some(backing) = view().and_then(|v| v.layer()) else {
        return false;
    };
    let content = unsafe { backing.sublayers() }.and_then(|l| l.firstObject());
    let has = content
        .and_then(|l| unsafe { l.contents() })
        .is_some_and(|c| as_iosurface(&c).is_some());
    has && backing.isGeometryFlipped()
}

/// The contents of a layer as an `IOSurface`, if that is what they are.
fn as_iosurface(contents: &AnyObject) -> Option<&IOSurfaceRef> {
    let name = contents.class().name().to_str().ok()?;
    // SAFETY: an `IOSurface` object is the `IOSurfaceRef` it is bridged to.
    name.contains("IOSurface")
        .then(|| unsafe { &*(contents as *const AnyObject as *const IOSurfaceRef) })
}

// --- Drags from other programs ------------------------------------------------------------------

pub(crate) struct FakeDragIvars {
    board: Retained<NSPasteboard>,
    at: std::cell::Cell<NSPoint>,
}

objc2::define_class!(
    // SAFETY: NSObject has no requirements for subclasses, and this has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = objc2::MainThreadOnly]
    #[name = "AntiWebUITestDraggingInfo"]
    #[ivars = FakeDragIvars]
    /// What AppKit tells a dragging destination about a drag from another program.
    struct FakeDrag;

    unsafe impl NSObjectProtocol for FakeDrag {}

    // What the view asks of it; it doesn't say it conforms, which would take all of the rest.
    impl FakeDrag {
        #[unsafe(method_id(draggingPasteboard))]
        fn dragging_pasteboard(&self) -> Retained<NSPasteboard> {
            self.ivars().board.clone()
        }

        #[unsafe(method(draggingLocation))]
        fn dragging_location(&self) -> NSPoint {
            self.ivars().at.get()
        }

        #[unsafe(method(draggingSourceOperationMask))]
        fn dragging_source_operation_mask(&self) -> NSDragOperation {
            NSDragOperation::Copy | NSDragOperation::Move | NSDragOperation::Generic
        }
    }
);

// SAFETY: the methods the view calls are there (above).
unsafe impl NSDraggingInfo for FakeDrag {}

thread_local! {
    static DRAG: std::cell::RefCell<Option<Retained<FakeDrag>>> = const { std::cell::RefCell::new(None) };
}

/// Where a point of the window's content (logical, from the top left) is in the window's
/// coordinates, as AppKit gives them.
fn in_window(x: f64, y: f64) -> Option<NSPoint> {
    let w = window()?;
    let c = w.contentRectForFrameRect(w.frame());
    Some(NSPoint::new(x, c.size.height - y))
}

/// A drag of text, files or both from another program comes over the window, at `(x, y)`.
pub fn drag_enter(text: Option<&str>, files: &[&str], x: f64, y: f64) {
    let (Some(v), Some(at)) = (view(), in_window(x, y)) else {
        return;
    };
    let board = NSPasteboard::pasteboardWithUniqueName();
    board.clearContents();
    let mut items = Vec::new();
    for f in files {
        let item = NSPasteboardItem::new();
        item.setString_forType(
            &NSString::from_str(f),
            &NSString::from_str("public.file-url"),
        );
        items.push(item);
    }
    if let Some(text) = text {
        let item = NSPasteboardItem::new();
        item.setString_forType(&NSString::from_str(text), unsafe {
            objc2_app_kit::NSPasteboardTypeString
        });
        items.push(item);
    }
    let writers: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
        .into_iter()
        .map(ProtocolObject::from_retained)
        .collect();
    board.writeObjects(&NSArray::from_retained_slice(&writers));
    let this = FakeDrag::alloc(mtm()).set_ivars(FakeDragIvars {
        board,
        at: std::cell::Cell::new(at),
    });
    let info: Retained<FakeDrag> = unsafe { objc2::msg_send![super(this), init] };
    v.draggingEntered(ProtocolObject::from_ref(&*info));
    DRAG.with(|d| *d.borrow_mut() = Some(info));
}

/// The drag moves to `(x, y)`. What the window would do with it: `"copy"`, `"move"` or `"none"`.
pub fn drag_move(x: f64, y: f64) -> &'static str {
    let (Some(v), Some(info), Some(at)) =
        (view(), DRAG.with(|d| d.borrow().clone()), in_window(x, y))
    else {
        return "none";
    };
    info.ivars().at.set(at);
    let op = v.draggingUpdated(ProtocolObject::from_ref(&*info));
    if op.contains(NSDragOperation::Copy) {
        "copy"
    } else if op.intersects(NSDragOperation::Move | NSDragOperation::Generic) {
        "move"
    } else {
        "none"
    }
}

/// The drag is let go of. Whether the window took it.
pub fn drag_drop() -> bool {
    let (Some(v), Some(info)) = (view(), DRAG.with(|d| d.borrow_mut().take())) else {
        return false;
    };
    let info = ProtocolObject::from_ref(&*info);
    v.prepareForDragOperation(info) && v.performDragOperation(info)
}

/// The drag leaves the window.
pub fn drag_exit() {
    if let (Some(v), Some(info)) = (view(), DRAG.with(|d| d.borrow_mut().take())) {
        v.draggingExited(Some(ProtocolObject::from_ref(&*info)));
    }
}
