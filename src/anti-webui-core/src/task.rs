//! Work that ends later, whose result comes back to the UI thread: what takes a while elsewhere,
//! like connecting to a network, without the UI waiting for it (see "時間がかかる操作" in
//! `docs/design/37-wayland-shell.md`).

use crate::channel::{UiSender, ui_channel};
use crate::reactive::{Signal, signal};
use std::cell::RefCell;
use std::rc::Rc;

type Then<T> = Box<dyn FnOnce(T)>;

struct Inner<T> {
    value: RefCell<Option<T>>,
    then: RefCell<Option<Then<T>>>,
    done: Signal<bool>,
}

/// A result to come, on the UI thread and in the window it was made in. If the owner it was made
/// under is gone by then, nothing happens. Dropping it doesn't stop the work.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// let size = signal(0);
/// task(|| std::fs::read("big.bin").map(|b| b.len()).unwrap_or(0))
///     .then(move |len| size.set(len));
/// ```
pub struct Task<T>(Rc<Inner<T>>);

impl<T> Clone for Task<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Ends a [`Task`] from any thread.
pub struct Completer<T>(UiSender<T>);

impl<T: Send + 'static> Completer<T> {
    pub fn complete(self, value: T) {
        self.0.send(value);
    }
}

impl<T: 'static> Task<T> {
    /// A task for something else to end, with what [`Completer::complete`] gives.
    pub fn pending() -> (Task<T>, Completer<T>)
    where
        T: Send,
    {
        let inner = Rc::new(Inner {
            value: RefCell::new(None),
            then: RefCell::new(None),
            done: signal(false),
        });
        // The channel keeps it, so `then` works on a task nobody holds; it goes with the owner.
        let held = inner.clone();
        let sender = ui_channel(move |value: T| {
            let inner = &held;
            let then = inner.then.borrow_mut().take();
            match then {
                Some(f) => f(value),
                None => *inner.value.borrow_mut() = Some(value),
            }
            inner.done.set(true);
        });
        (Task(inner), Completer(sender))
    }

    /// A task that has already ended.
    pub fn ready(value: T) -> Task<T> {
        Task(Rc::new(Inner {
            value: RefCell::new(Some(value)),
            then: RefCell::new(None),
            done: signal(true),
        }))
    }

    /// Calls `f` with the result once it is there, at once if it already is. `f` takes the result:
    /// [`Task::get`] has nothing after that.
    pub fn then(self, f: impl FnOnce(T) + 'static) {
        let value = self.0.value.borrow_mut().take();
        match value {
            Some(value) => f(value),
            None if self.0.done.get_untracked() => {}
            None => *self.0.then.borrow_mut() = Some(Box::new(f)),
        }
    }

    /// Whether it has ended. Reading it tracks it.
    pub fn is_done(&self) -> bool {
        self.0.done.get()
    }

    /// The result, once there and if [`Task::then`] didn't take it. Reading it tracks it.
    pub fn get(&self) -> Option<T>
    where
        T: Clone,
    {
        self.0.done.with(|_| {});
        self.0.value.borrow().clone()
    }

    /// Changes what the result will be.
    pub fn map<U: Send + 'static>(self, f: impl FnOnce(T) -> U + 'static) -> Task<U>
    where
        T: Send,
    {
        let (task, done) = Task::pending();
        let sender = done.0.clone();
        self.then(move |value| sender.send(f(value)));
        drop(done);
        task
    }
}

/// Runs `f` on a thread of its own; the task ends with what it returns.
pub fn task<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Task<T> {
    let (task, done) = Task::pending();
    let work = done.0.work();
    std::thread::spawn(move || {
        let value = f();
        done.complete(value);
        drop(work);
    });
    task
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::{create_root, effect};
    use std::cell::Cell;

    #[test]
    fn the_result_comes_on_this_thread_when_the_channel_is_drained() {
        let got = Rc::new(Cell::new(0));
        let ((), owner) = create_root(|| {
            let got = got.clone();
            let (task, done) = Task::pending();
            task.then(move |v| got.set(v));
            std::thread::spawn(move || done.complete(7)).join().unwrap();
        });
        assert_eq!(got.get(), 0, "not before the UI thread takes it");
        crate::channel::pump();
        assert_eq!(got.get(), 7);
        owner.dispose();
    }

    #[test]
    fn reading_it_tracks_it_and_then_after_the_end_runs_at_once() {
        let seen = Rc::new(Cell::new(None));
        let ((task, done), owner) = create_root(|| {
            let (task, done) = Task::<u32>::pending();
            let (t, seen) = (task.clone(), seen.clone());
            effect(move || seen.set(t.get()));
            (task, done)
        });
        assert_eq!(seen.get(), None);
        done.complete(3);
        crate::channel::pump();
        assert_eq!(seen.get(), Some(3));
        let later = Rc::new(Cell::new(0));
        let l = later.clone();
        owner.with(|| task.then(move |v| l.set(v)));
        assert_eq!(later.get(), 3);
        owner.dispose();
    }

    #[test]
    fn nothing_happens_once_its_owner_is_gone() {
        let got = Rc::new(Cell::new(false));
        let (done, owner) = create_root(|| {
            let (task, done) = Task::pending();
            let got = got.clone();
            task.then(move |()| got.set(true));
            done
        });
        owner.dispose();
        done.complete(());
        crate::channel::pump();
        assert!(!got.get());
    }
}
