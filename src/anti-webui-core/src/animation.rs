//! Running code once per frame, like `requestAnimationFrame`.

use std::cell::{Cell, RefCell};
use std::time::Instant;

type FrameCallback = Box<dyn FnOnce(Instant)>;

thread_local! {
    static CALLBACKS: RefCell<Vec<FrameCallback>> = const { RefCell::new(Vec::new()) };
    static WANTED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` before the next frame is produced, on the UI thread, with the time of that frame.
/// Asking again from `f` animates: a frame comes as often as the compositor takes them.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use anti_webui_core::animation::request_frame;
///
/// fn spin(angle: Signal<f32>) {
///     request_frame(move |_| {
///         angle.update(|a| *a += 1.0);
///         spin(angle);
///     });
/// }
/// ```
pub fn request_frame(f: impl FnOnce(Instant) + 'static) {
    CALLBACKS.with(|c| c.borrow_mut().push(Box::new(f)));
    WANTED.with(|w| w.set(true));
}

pub(crate) fn frame_requested() -> bool {
    WANTED.with(|w| w.get())
}

/// What was requested from here on waits for the next frame.
pub(crate) fn run_frame_callbacks(now: Instant) {
    WANTED.with(|w| w.set(false));
    for f in CALLBACKS.with(|c| std::mem::take(&mut *c.borrow_mut())) {
        f(now);
    }
}
