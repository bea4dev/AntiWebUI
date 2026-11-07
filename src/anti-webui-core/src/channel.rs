//! Getting things from other threads onto the UI thread, for crates that do their work elsewhere.
//! The closure of a [`ui_channel`] runs on the UI thread, in a batch, so it can set signals; the
//! other threads never see a signal. See `docs/design/05-rendering.md` on why there is one UI thread.

use crate::reactive::on_cleanup;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type Waker = Arc<dyn Fn() + Send + Sync>;

/// What the senders of a UI thread share with it.
#[derive(Default)]
struct Thread {
    waker: Mutex<Option<Waker>>,
    /// How many [`UiWork`] there are.
    working: AtomicUsize,
}

trait Inbox {
    fn drain(&self);
}

struct Queue<T, F> {
    queue: Arc<Mutex<Vec<T>>>,
    on_message: F,
}

impl<T, F: Fn(T)> Inbox for Queue<T, F> {
    fn drain(&self) {
        let messages = std::mem::take(&mut *self.queue.lock().expect("no panics in here"));
        for message in messages {
            (self.on_message)(message);
        }
    }
}

/// A channel's id, the window it was made in, and where its messages go.
type Registered = (u64, u64, Rc<dyn Inbox>);

thread_local! {
    static THREAD: Arc<Thread> = Arc::default();
    /// With the window each was made in, where its closure runs (see `crate::window`).
    static INBOXES: RefCell<Vec<Registered>> = const { RefCell::new(Vec::new()) };
    static NEXT_INBOX: Cell<u64> = const { Cell::new(0) };
}

/// Sends to the UI thread from any thread. See [`ui_channel`].
pub struct UiSender<T> {
    queue: Arc<Mutex<Vec<T>>>,
    thread: Arc<Thread>,
}

impl<T> Clone for UiSender<T> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            thread: self.thread.clone(),
        }
    }
}

impl<T> UiSender<T> {
    /// Wakes the UI thread, which gets to it before its next frame. Nothing happens if the owner of
    /// the channel is gone.
    pub fn send(&self, message: T) {
        self.queue.lock().expect("no panics in here").push(message);
        let waker = self.thread.waker.lock().expect("no panics in here").clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Says that more is coming, until this is dropped. A window doesn't care, but tests and
    /// screenshots wait for it with [`Ui::wait_for_images`](crate::Ui::wait_for_images).
    pub fn work(&self) -> UiWork {
        self.thread.working.fetch_add(1, Ordering::Relaxed);
        UiWork(self.thread.clone())
    }
}

/// See [`UiSender::work`].
pub struct UiWork(Arc<Thread>);

impl Drop for UiWork {
    fn drop(&mut self) {
        self.0.working.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Makes a way to the UI thread: `on_message` is called there with what was sent. It stops with the
/// owner it was made in.
pub fn ui_channel<T: Send + 'static>(on_message: impl Fn(T) + 'static) -> UiSender<T> {
    let queue = Arc::new(Mutex::new(Vec::new()));
    let id = NEXT_INBOX.with(|n| n.replace(n.get() + 1));
    let inbox: Rc<dyn Inbox> = Rc::new(Queue {
        queue: queue.clone(),
        on_message,
    });
    let window = crate::window::active_id();
    INBOXES.with(|i| i.borrow_mut().push((id, window, inbox)));
    on_cleanup(move || INBOXES.with(|i| i.borrow_mut().retain(|(other, _, _)| *other != id)));
    UiSender {
        queue,
        thread: THREAD.with(Clone::clone),
    }
}

/// Calls the closures of the channels with what has come. They can make more channels.
pub(crate) fn drain() {
    let inboxes: Vec<(u64, Rc<dyn Inbox>)> = INBOXES.with(|i| {
        i.borrow()
            .iter()
            .map(|(_, window, inbox)| (*window, inbox.clone()))
            .collect()
    });
    for (window, inbox) in inboxes {
        crate::window::within(window, || inbox.drain());
    }
}

/// Says how to wake the UI loop of this thread, before any window does.
#[doc(hidden)]
pub fn set_thread_waker(waker: Waker) {
    set_waker(waker);
}

pub(crate) fn set_waker(waker: Waker) {
    THREAD.with(|t| *t.waker.lock().expect("no panics in here") = Some(waker));
}

/// Whether something said more is coming.
pub(crate) fn working() -> bool {
    THREAD.with(|t| t.working.load(Ordering::Relaxed) > 0)
}

/// Calls the closures of the channels with what has come, for tests that have no `Ui`.
#[doc(hidden)]
pub fn pump() {
    drain();
}
