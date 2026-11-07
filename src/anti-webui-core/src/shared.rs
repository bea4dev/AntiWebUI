//! A signal that windows on different threads can share (see `docs/design/36-multi-window.md`).
//! The value sits behind a mutex; each UI thread that reads it in an effect or a view keeps a signal
//! of its own that a write wakes, so the reactions run on their own thread.

use crate::channel::{UiSender, ui_channel};
use crate::reactive::{Signal, create_root, is_tracking, signal};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::ThreadId;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A thread that reads the value and wants to hear of writes.
struct Listener {
    thread: ThreadId,
    sender: UiSender<()>,
    /// Gone with the thread.
    alive: Weak<()>,
}

struct Inner<T> {
    id: u64,
    value: Mutex<T>,
    listeners: Mutex<Vec<Listener>>,
}

/// The signal a shared signal has on a thread, and what keeps its listener alive.
type Local = (Signal<()>, Rc<Arc<()>>);

thread_local! {
    /// What each shared signal read on this thread has here.
    static LOCAL: RefCell<HashMap<u64, Local>> = RefCell::default();
}

/// A value that windows on any thread can read and write, like a [`Signal`] that is `Send` and
/// `Sync`. Reading it in an effect or a view tracks it on that thread. Cheap to clone.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// let done = shared_signal(0u32);
/// let worker = done.clone();
/// std::thread::spawn(move || worker.update(|d| *d += 1));
/// let view = text(move || done.get().to_string());
/// ```
pub struct SharedSignal<T>(Arc<Inner<T>>);

impl<T> Clone for SharedSignal<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for SharedSignal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedSignal")
            .field(&*self.0.value.lock().expect("no panics in here"))
            .finish()
    }
}

pub fn shared_signal<T: Send + 'static>(value: T) -> SharedSignal<T> {
    SharedSignal(Arc::new(Inner {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        value: Mutex::new(value),
        listeners: Mutex::new(Vec::new()),
    }))
}

impl<T: Send + 'static> SharedSignal<T> {
    /// Subscribes the running effect, on this thread.
    fn track(&self) {
        if !is_tracking() {
            return;
        }
        let id = self.0.id;
        let local = LOCAL.with(|l| l.borrow().get(&id).map(|(s, _)| *s));
        let local = local.unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever reads it first.
            let ((local, sender), _) = create_root(|| {
                let local = signal(());
                (local, ui_channel(move |()| local.notify()))
            });
            let alive = Rc::new(Arc::new(()));
            self.0
                .listeners
                .lock()
                .expect("no panics in here")
                .push(Listener {
                    thread: std::thread::current().id(),
                    sender,
                    alive: Arc::downgrade(&alive),
                });
            LOCAL.with(|l| l.borrow_mut().insert(id, (local, alive)));
            local
        });
        local.with(|_| {});
    }

    /// Wakes the threads that read it; this one at once.
    fn notify(&self) {
        let here = std::thread::current().id();
        let mut local = None;
        self.0
            .listeners
            .lock()
            .expect("no panics in here")
            .retain(|l| {
                if l.alive.strong_count() == 0 {
                    return false;
                }
                if l.thread == here {
                    local = LOCAL.with(|m| m.borrow().get(&self.0.id).map(|(s, _)| *s));
                } else {
                    l.sender.send(());
                }
                true
            });
        if let Some(local) = local {
            local.notify();
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.track();
        f(&self.0.value.lock().expect("no panics in here"))
    }

    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0.value.lock().expect("no panics in here"))
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    pub fn set(&self, value: T) {
        *self.0.value.lock().expect("no panics in here") = value;
        self.notify();
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let r = f(&mut self.0.value.lock().expect("no panics in here"));
        self.notify();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::{create_root, effect};
    use std::cell::Cell;

    #[test]
    fn a_write_on_this_thread_reruns_at_once_and_one_elsewhere_after_the_channel() {
        let shared = shared_signal(1);
        let seen = Rc::new(Cell::new(0));
        let (_, _owner) = create_root(|| {
            let (shared, seen) = (shared.clone(), seen.clone());
            effect(move || seen.set(shared.get()));
        });
        assert_eq!(seen.get(), 1);
        shared.set(2);
        assert_eq!(seen.get(), 2);
        let other = shared.clone();
        std::thread::spawn(move || other.set(3)).join().unwrap();
        assert_eq!(seen.get(), 2, "not before the UI thread takes the message");
        crate::channel::pump();
        assert_eq!(seen.get(), 3);
    }

    #[test]
    fn reading_outside_an_effect_listens_to_nothing() {
        let shared = shared_signal(String::from("a"));
        assert_eq!(shared.get(), "a");
        assert!(shared.0.listeners.lock().unwrap().is_empty());
    }
}
