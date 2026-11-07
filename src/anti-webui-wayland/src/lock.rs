//! Lock screens (`ext_session_lock_v1`): a surface on each output while the session is locked.
//! The lock is one for the whole connection, whichever thread opened its surfaces: the first
//! surface locks, and closing the last one unlocks. See "画面ロック" in
//! `docs/design/37-wayland-shell.md`.

use crate::state::State;
use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::sync::{Mutex, OnceLock};
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1 as manager, ext_session_lock_surface_v1 as lock_surface,
    ext_session_lock_v1 as lock,
};

/// Where the lock of the session is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LockState {
    #[default]
    Unlocked,
    /// Asked for; the compositor says it is locked once every output shows a lock surface.
    Locking,
    Locked,
    /// The compositor won't, as when another program has locked it, or has ended the lock. The
    /// lock surfaces are to be closed.
    Refused,
}

struct Lock {
    object: lock::ExtSessionLockV1,
    /// The lock surfaces there are.
    surfaces: usize,
    locked: bool,
    /// The compositor sent `finished`: the object is gone.
    finished: bool,
}

static LOCK: Mutex<Option<Lock>> = Mutex::new(None);
static STATE: OnceLock<SharedSignal<LockState>> = OnceLock::new();

fn state() -> &'static SharedSignal<LockState> {
    STATE.get_or_init(|| shared_signal(LockState::Unlocked))
}

fn set_state(s: LockState) {
    let state = state();
    if state.with_untracked(|v| *v != s) {
        state.set(s);
    }
}

/// Where the lock of the session is. Reading it tracks it, on any thread.
pub fn lock_state() -> LockState {
    state().get()
}

pub(crate) struct LockRole {
    pub object: lock_surface::ExtSessionLockSurfaceV1,
}

impl LockRole {
    /// Locks the session if it isn't yet, and covers `output` with `surface`.
    pub(crate) fn create(
        manager: &manager::ExtSessionLockManagerV1,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
        qh: &QueueHandle<State>,
    ) -> Self {
        let mut slot = LOCK.lock().expect("no panics in here");
        let lock = slot.get_or_insert_with(|| {
            set_state(LockState::Locking);
            Lock {
                object: manager.lock(qh, ()),
                surfaces: 0,
                locked: false,
                finished: false,
            }
        });
        lock.surfaces += 1;
        Self {
            object: lock.object.get_lock_surface(surface, output, qh, ()),
        }
    }

    /// Takes the surface away, unlocking the session when it was the last.
    pub(crate) fn destroy(&self) {
        self.object.destroy();
        let mut slot = LOCK.lock().expect("no panics in here");
        let Some(lock) = slot.as_mut() else {
            return;
        };
        lock.surfaces = lock.surfaces.saturating_sub(1);
        if lock.surfaces > 0 {
            return;
        }
        if !lock.finished {
            if lock.locked {
                lock.object.unlock_and_destroy();
            } else {
                lock.object.destroy();
            }
        }
        *slot = None;
        set_state(LockState::Unlocked);
    }
}

impl Dispatch<lock::ExtSessionLockV1, ()> for State {
    fn event(
        _: &mut Self,
        object: &lock::ExtSessionLockV1,
        event: lock::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut slot = LOCK.lock().expect("no panics in here");
        let Some(lock) = slot.as_mut().filter(|l| l.object == *object) else {
            return;
        };
        match event {
            lock::Event::Locked => {
                lock.locked = true;
                set_state(LockState::Locked);
            }
            lock::Event::Finished => {
                lock.finished = true;
                object.destroy();
                set_state(LockState::Refused);
            }
            _ => {}
        }
    }
}

impl Dispatch<lock_surface::ExtSessionLockSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        surface: &lock_surface::ExtSessionLockSurfaceV1,
        event: lock_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let lock_surface::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            surface.ack_configure(serial);
            // The compositor decides: the size of the output.
            state.logical_size = (width.max(1) as i32, height.max(1) as i32);
            state.configured = true;
            state.update_size();
            state.redraw_requested = true;
        }
    }
}

wayland_client::delegate_noop!(State: ignore manager::ExtSessionLockManagerV1);
