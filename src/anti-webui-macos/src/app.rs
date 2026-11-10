//! The application: AppKit set up once, the menu, waking the loop from other threads, and the way
//! back into our loop from AppKit's own (see "イベントループ" in
//! `docs/design/38-platform-macos.md`).

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSMenu,
    NSMenuItem,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSString};
use std::cell::{Cell, RefCell};
use std::time::Instant;

/// `data1` of the empty events that wake the loop.
const WAKE: isize = 0x4157_5549; // "AWUI"

define_class!(
    // SAFETY: NSObject has no requirements for subclasses, and this has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AntiWebUIAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        /// ⌘Q and the Dock's "Quit": the first window is asked to close instead, through its
        /// handlers.
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            QUIT.with(|q| q.set(true));
            wake(self.mtm());
            NSApplicationTerminateReply::TerminateCancel
        }
    }
);

thread_local! {
    static DELEGATE: RefCell<Option<Retained<AppDelegate>>> = const { RefCell::new(None) };
    static QUIT: Cell<bool> = const { Cell::new(false) };
    /// What runs our loop from inside one of AppKit's; see [`with_pump`].
    static PUMP: Cell<Option<*mut (dyn FnMut() + 'static)>> = const { Cell::new(None) };
}

/// Sets AppKit up, once.
pub(crate) fn setup(mtm: MainThreadMarker) {
    if DELEGATE.with(|d| d.borrow().is_some()) {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate: Retained<AppDelegate> = unsafe { msg_send![AppDelegate::alloc(mtm), init] };
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    app.setMainMenu(Some(&main_menu(mtm)));
    app.finishLaunching();
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

/// The application's menu, with what every Mac app has: hiding and quitting.
fn main_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let name = std::env::current_exe()
        .ok()
        .and_then(|p| Some(p.file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "AntiWebUI".into());
    let item = |title: &str, action, key: &str| unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&item(&format!("Hide {name}"), sel!(hide:), "h"));
    let others = item("Hide Others", sel!(hideOtherApplications:), "h");
    others
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
    app_menu.addItem(&others);
    app_menu.addItem(&item("Show All", sel!(unhideAllApplications:), ""));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&item(&format!("Quit {name}"), sel!(terminate:), "q"));
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&app_menu));
    let bar = NSMenu::new(mtm);
    bar.addItem(&app_item);
    bar
}

/// Whether quitting was asked for since the last time.
pub(crate) fn take_quit() -> bool {
    QUIT.with(|q| q.replace(false))
}

/// Makes the loop go around: posts an empty event for `nextEventMatchingMask` to return.
pub(crate) fn wake(mtm: MainThreadMarker) {
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        WAKE,
        0,
    );
    if let Some(event) = event {
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
}

/// Wakes the main thread's loop, from any thread.
pub(crate) fn wake_main() {
    match MainThreadMarker::new() {
        Some(mtm) => wake(mtm),
        None => dispatch2::DispatchQueue::main().exec_async(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                wake(mtm);
            }
        }),
    }
}

/// Waits for an event until `deadline`, and hands it and those already queued after it to AppKit.
pub(crate) fn pump_events(mtm: MainThreadMarker, deadline: Option<Instant>) {
    let app = NSApplication::sharedApplication(mtm);
    let until = match deadline {
        Some(d) => {
            let secs = d.saturating_duration_since(Instant::now()).as_secs_f64();
            NSDate::dateWithTimeIntervalSinceNow(secs)
        }
        None => NSDate::distantFuture(),
    };
    let mode = unsafe { NSDefaultRunLoopMode };
    let mut wait = Some(until);
    loop {
        let event = autoreleasepool(|_| {
            let until = wait.take().unwrap_or_else(NSDate::distantPast);
            let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                mode,
                true,
            );
            if let Some(e) = &event {
                send(&app, e, mtm);
            }
            event.is_some()
        });
        if !event {
            return;
        }
    }
}

fn send(app: &NSApplication, event: &NSEvent, mtm: MainThreadMarker) {
    let kind = event.r#type();
    if kind == NSEventType::ApplicationDefined && event.data1() == WAKE {
        return;
    }
    // AppKit keeps the release of a key pressed with Command from the window; it goes there
    // straight, as winit and GLFW do.
    if kind == NSEventType::KeyUp
        && event
            .modifierFlags()
            .contains(NSEventModifierFlags::Command)
        && let Some(window) = event.window(mtm)
    {
        window.sendEvent(event);
        return;
    }
    app.sendEvent(event);
}

/// Runs `f` with `pump` as what AppKit's own loops can run ours with, like the one of a live
/// resize, which keeps `f` from returning until the button is let go.
pub(crate) fn with_pump<R>(pump: &mut dyn FnMut(), f: impl FnOnce() -> R) -> R {
    struct Restore(Option<*mut (dyn FnMut() + 'static)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PUMP.with(|p| p.set(self.0));
        }
    }
    let pump: *mut (dyn FnMut() + '_) = pump;
    // SAFETY: only the lifetime is erased. The pointer is taken out again before `pump` goes
    // (`Restore`), and `run_pump` takes it out while it runs, so it is never used twice at once.
    let pump: *mut (dyn FnMut() + 'static) = unsafe { std::mem::transmute(pump) };
    let _restore = Restore(PUMP.with(|p| p.replace(Some(pump))));
    f()
}

/// Runs our loop once from inside one of AppKit's. Whether there was one to run: not outside of
/// [`with_pump`], and not from inside itself.
pub(crate) fn run_pump() -> bool {
    let Some(pump) = PUMP.with(Cell::take) else {
        return false;
    };
    // SAFETY: see `with_pump`; the pointer is out of `PUMP` while it runs.
    autoreleasepool(|_| unsafe { (*pump)() });
    PUMP.with(|p| p.set(Some(pump)));
    true
}
