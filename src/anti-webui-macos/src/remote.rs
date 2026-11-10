//! Windows of `spawn_window`, drawn on threads of their own (see "複数ウィンドウ" in
//! `docs/design/38-platform-macos.md`). AppKit's window and views stay on the main thread, which
//! hosts them here and hands their events over; the thread draws to its layers, waits on its own
//! run loop, and has the display link come there. What it asks of the window goes to the main
//! thread as a function to run on it.

use crate::WindowKey;
use crate::frames::{Frames, Sendable, Signals};
use crate::state::State;
use anti_webui_core::event::WindowEvent;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopDefaultMode,
};
use objc2_quartz_core::CADisplayLink;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Wakes a thread waiting on its run loop: a source that does nothing, signalled.
pub(crate) struct ThreadWaker {
    run_loop: CFRetained<CFRunLoop>,
    source: CFRetained<CFRunLoopSource>,
}

// SAFETY: signalling a source and waking a run loop are what other threads are meant to do; the
// run loop runs only on its own thread (`wait`).
unsafe impl Send for ThreadWaker {}
unsafe impl Sync for ThreadWaker {}

unsafe extern "C-unwind" fn perform(_info: *mut c_void) {}

impl ThreadWaker {
    /// For the run loop of this thread.
    pub fn here() -> Arc<Self> {
        let run_loop = CFRunLoop::current().expect("every thread has a run loop");
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: std::ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform),
        };
        // SAFETY: the context is valid for the call, and copied by it.
        let source =
            unsafe { CFRunLoopSource::new(None, 0, &mut context) }.expect("a run loop source");
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
        Arc::new(Self { run_loop, source })
    }

    pub fn wake(&self) {
        self.source.signal();
        self.run_loop.wake_up();
    }

    /// Runs the run loop until something comes, or `deadline`. On the thread it is for.
    pub fn wait(&self, deadline: Option<Instant>) {
        let secs = deadline.map_or(1e9, |d| {
            d.saturating_duration_since(Instant::now()).as_secs_f64()
        });
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, secs, true);
    }
}

pub(crate) struct TickerIvars {
    signals: Arc<Signals>,
    wake: Arc<ThreadWaker>,
}

define_class!(
    // SAFETY: NSObject has no requirements for subclasses, and this has no `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "AntiWebUITicker"]
    #[ivars = TickerIvars]
    /// What the display link of a window drawn on its own thread calls, on that thread.
    pub(crate) struct Ticker;

    impl Ticker {
        #[unsafe(method(displayLinkFired:))]
        fn display_link_fired(&self, link: &CADisplayLink) {
            let ivars = self.ivars();
            ivars.signals.set_tick((link.timestamp(), link.targetTimestamp()));
            ivars.wake.wake();
        }
    }
);

impl Ticker {
    pub fn new(signals: Arc<Signals>, wake: Arc<ThreadWaker>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(TickerIvars { signals, wake });
        unsafe { msg_send![super(this), init] }
    }
}

/// What a window's thread and the main thread share.
pub(crate) struct RemoteLink {
    pub key: WindowKey,
    /// What came for the window, handed over by the main thread.
    events: Mutex<Vec<WindowEvent>>,
    pub wake: Arc<ThreadWaker>,
}

impl RemoteLink {
    pub fn new(key: WindowKey, wake: Arc<ThreadWaker>) -> Arc<Self> {
        Arc::new(Self {
            key,
            events: Mutex::new(Vec::new()),
            wake,
        })
    }

    fn push(&self, mut events: Vec<WindowEvent>) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(&mut events);
        self.wake.wake();
    }

    pub fn take_events(&self) -> Vec<WindowEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn has_events(&self) -> bool {
        !self
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// Runs `f` on the main thread with the window, after what was asked before it.
    pub fn run(&self, f: impl FnOnce(&mut State) + Send + 'static) {
        let key = self.key;
        dispatch2::DispatchQueue::main().exec_async(move || {
            with_hosted(key, |h| f(&mut h.state));
        });
    }
}

struct Hosted {
    state: State,
    link: Arc<RemoteLink>,
}

thread_local! {
    /// The windows of other threads, on the main thread.
    static HOSTED: RefCell<HashMap<WindowKey, Hosted>> = RefCell::default();
    /// Windows closed while they were out of `HOSTED`, being used.
    static CLOSED: RefCell<HashSet<WindowKey>> = RefCell::default();
}

/// Opens a window for another thread, on the main thread. What draws it goes to that thread.
pub(crate) fn host(
    options: &crate::WindowOptions,
    link: Arc<RemoteLink>,
) -> Result<(Sendable<Frames>, crate::ParentLink), crate::Error> {
    let mtm = objc2::MainThreadMarker::new().expect("on the main thread");
    let (mut state, frames) = State::new(mtm, options, Some(link.wake.clone()))?;
    let parent = state.parent_link();
    // The size, before the first frame.
    link.push(state.take_events());
    HOSTED.with(|h| h.borrow_mut().insert(link.key, Hosted { state, link }));
    Ok((Sendable(frames), parent))
}

/// Closes a window of another thread, on the main thread.
pub(crate) fn unhost(key: WindowKey) {
    match HOSTED.with(|h| h.borrow_mut().remove(&key)) {
        Some(mut hosted) => hosted.state.close(),
        None => {
            CLOSED.with(|c| c.borrow_mut().insert(key));
        }
    }
}

/// Runs `f` with a hosted window. It is out of `HOSTED` meanwhile, so that what AppKit runs from
/// inside `f`, like our loop in a drag of the window, can use the others.
fn with_hosted<R>(key: WindowKey, f: impl FnOnce(&mut Hosted) -> R) -> Option<R> {
    let mut hosted = HOSTED.with(|h| h.borrow_mut().remove(&key))?;
    let r = f(&mut hosted);
    if CLOSED.with(|c| c.borrow_mut().remove(&key)) {
        hosted.state.close();
    } else {
        HOSTED.with(|h| h.borrow_mut().insert(key, hosted));
    }
    Some(r)
}

/// Hands the hosted windows what came for them.
pub(crate) fn forward_events() {
    let keys: Vec<WindowKey> = HOSTED.with(|h| h.borrow().keys().copied().collect());
    for key in keys {
        with_hosted(key, |h| {
            let events = h.state.take_events();
            if !events.is_empty() {
                h.link.push(events);
            }
        });
    }
}

/// Whether a hosted window has something to hand over.
pub(crate) fn has_pending() -> bool {
    HOSTED.with(|h| h.borrow().values().any(|w| w.state.has_pending()))
}
