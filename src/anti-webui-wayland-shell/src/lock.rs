//! Locking the session: a lock screen over every output (`ext_session_lock_v1`), as long as the
//! program wants. Checking a password is the program's business. See "画面ロック" in
//! `docs/design/37-wayland-shell.md`.

use crate::output::{Output, for_each_output, spawn_for_each_output};
use anti_webui_core::reactive::{Owner, create_root, effect, on_cleanup, untrack};
use anti_webui_core::view::IntoView;
use anti_webui_core::window::{Role, WindowOptions, open_window};
pub use anti_webui_wayland::{LockState, lock_state};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// A lock of the session, from [`lock_session`] or [`spawn_lock_session`]. Cheap to clone; the
/// lock stays until [`SessionLock::unlock`], even when every clone has gone.
#[derive(Clone)]
pub struct SessionLock(Rc<RefCell<Option<Owner>>>);

impl std::fmt::Debug for SessionLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SessionLock").field(&lock_state()).finish()
    }
}

impl SessionLock {
    /// Where the lock is. Reading it tracks it.
    pub fn state(&self) -> LockState {
        lock_state()
    }

    /// Whether the compositor says the session is locked: every output shows a lock screen and
    /// nothing else can be seen or used.
    pub fn is_locked(&self) -> bool {
        lock_state() == LockState::Locked
    }

    /// Closes the lock screens, which unlocks the session once the last has gone. Call it when the
    /// user has shown who they are.
    pub fn unlock(&self) {
        if let Some(owner) = self.0.borrow_mut().take() {
            owner.dispose();
        }
    }
}

fn options(output: &Output) -> WindowOptions {
    WindowOptions::new()
        .title("Lock")
        .role(Role::Lock(output.name.clone()))
}

fn opened_here(output: &Output, screen: impl FnOnce() -> anti_webui_core::view::View + 'static) {
    let handle = open_window(options(output), screen);
    on_cleanup(move || handle.close());
}

/// Locks the session, showing `screen` on every output, now and as they are plugged in, all on
/// this thread. It stays locked until [`SessionLock::unlock`]; if this program goes away first,
/// the compositor keeps it locked, as the protocol says. When the compositor won't lock, as when
/// another program has, the screens close by themselves and the state says
/// [`LockState::Refused`].
pub fn lock_session<V: IntoView>(screen: impl Fn(Output) -> V + 'static) -> SessionLock {
    let screen = Rc::new(screen);
    start(|| {
        for_each_output(move |output| {
            let screen = screen.clone();
            let shown = output.clone();
            opened_here(&output, move || screen(shown).into_view());
        });
    })
}

/// Locks the session as [`lock_session`] does, with each output's lock screen on a UI thread of
/// its own.
pub fn spawn_lock_session<V: IntoView>(
    screen: impl Fn(Output) -> V + Send + Sync + 'static,
) -> SessionLock {
    let screen = Arc::new(screen);
    start(|| {
        spawn_for_each_output(move |output| {
            let screen = screen.clone();
            let shown = output.clone();
            opened_here(&output, move || screen(shown).into_view());
        });
    })
}

fn start(open: impl FnOnce()) -> SessionLock {
    let ((), owner) = create_root(open);
    let lock = SessionLock(Rc::new(RefCell::new(Some(owner))));
    // A refusal left over from before doesn't count, until the state has been something else.
    let stale = Cell::new(untrack(lock_state) == LockState::Refused);
    let closing = lock.clone();
    effect(move || match lock_state() {
        LockState::Refused if !stale.get() => closing.unlock(),
        LockState::Refused => {}
        _ => stale.set(false),
    });
    lock
}
