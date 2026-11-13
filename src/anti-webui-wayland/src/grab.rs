//! The xdg popups of all the windows on one connection, which share one popup grab.
//!
//! The compositor keeps one grab chain per client: a new grabbing popup must go on the topmost
//! grabbing popup, or on a window when no grab is on. Each window keeps its own popups, so before
//! one of them grabs, the grabs it can't go on top of have to end, the way a click outside would
//! end them.
//!
//! Only the window of a popup ever destroys it. That window may be drawing it on another thread,
//! and the compositor ends the client for a commit or an ack on a surface whose popup is gone. So
//! the window that wants to grab asks the others to dismiss the grabs in its way, keeps its popup
//! waiting, and is woken to try again when one of them goes.

use anti_webui_core::platform::PopupId;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use wayland_protocols::xdg::shell::client::xdg_popup;

/// Where a window learns which of its popups another window needs gone, and what wakes its thread.
pub(crate) struct Mailbox {
    ended: Mutex<Vec<PopupId>>,
    /// A grab went since the last time, which a popup of this window may have waited for.
    released: AtomicBool,
    wake: OwnedFd,
}

impl Mailbox {
    pub fn new(wake: OwnedFd) -> Arc<Self> {
        Arc::new(Self {
            ended: Mutex::new(Vec::new()),
            released: AtomicBool::new(false),
            wake,
        })
    }

    /// The popups to dismiss, asked for since the last time.
    pub fn take_ended(&self) -> Vec<PopupId> {
        std::mem::take(&mut *lock(&self.ended))
    }

    /// Whether a grab went since the last time.
    pub fn take_released(&self) -> bool {
        self.released.swap(false, Ordering::AcqRel)
    }

    fn wake(&self) {
        let _ = rustix::io::write(&self.wake, &[1]);
    }
}

struct Entry {
    owner: Arc<Mailbox>,
    id: PopupId,
    popup: xdg_popup::XdgPopup,
    grabbing: bool,
    /// Its window was asked to dismiss it.
    asked: bool,
}

#[derive(Default)]
struct Shared {
    /// In the order they were made.
    entries: Vec<Entry>,
    /// Windows with a grabbing popup waiting for a grab to go.
    waiting: Vec<Arc<Mailbox>>,
}

/// The xdg popups of a connection.
#[derive(Clone, Default)]
pub(crate) struct ConnectionPopups(Arc<Mutex<Shared>>);

impl ConnectionPopups {
    /// Hold it from deciding to grab until the grab is asked for, so that no other window grabs
    /// in between.
    pub fn lock(&self) -> LockedPopups<'_> {
        LockedPopups(lock(&self.0))
    }
}

pub(crate) struct LockedPopups<'a>(MutexGuard<'a, Shared>);

impl LockedPopups<'_> {
    /// Whether a grabbing popup with these ancestors can go on top of the grab now. If not, the
    /// windows of the grabs in its way are asked to dismiss them, and this one is woken as they go.
    pub fn may_grab(&mut self, owner: &Arc<Mailbox>, ancestors: &[PopupId]) -> bool {
        let mut blocked = false;
        let mut asked: Vec<Arc<Mailbox>> = Vec::new();
        for e in &mut self.0.entries {
            if !e.grabbing || (Arc::ptr_eq(&e.owner, owner) && ancestors.contains(&e.id)) {
                continue;
            }
            blocked = true;
            if !std::mem::replace(&mut e.asked, true) {
                lock(&e.owner.ended).push(e.id);
                if !asked.iter().any(|o| Arc::ptr_eq(o, &e.owner)) {
                    asked.push(e.owner.clone());
                }
            }
        }
        if blocked && !self.0.waiting.iter().any(|w| Arc::ptr_eq(w, owner)) {
            self.0.waiting.push(owner.clone());
        }
        for owner in asked {
            owner.wake();
        }
        !blocked
    }

    pub fn add(
        &mut self,
        owner: &Arc<Mailbox>,
        id: PopupId,
        popup: xdg_popup::XdgPopup,
        grabbing: bool,
    ) {
        self.0.entries.push(Entry {
            owner: owner.clone(),
            id,
            popup,
            grabbing,
            asked: false,
        });
    }

    /// Destroys the `xdg_popup` of the window's popup and takes it out. A grab that goes wakes the
    /// windows waiting to grab, after its destroy, so that their grab comes after it.
    pub fn destroy(&mut self, owner: &Arc<Mailbox>, id: PopupId) {
        let Some(at) = self
            .0
            .entries
            .iter()
            .position(|e| Arc::ptr_eq(&e.owner, owner) && e.id == id)
        else {
            return;
        };
        let e = self.0.entries.remove(at);
        e.popup.destroy();
        if e.grabbing {
            for w in std::mem::take(&mut self.0.waiting) {
                w.released.store(true, Ordering::Release);
                w.wake();
            }
        }
    }

    /// Destroys what is left of the window's popups, topmost first, as when it goes without
    /// closing them.
    pub fn destroy_all(&mut self, owner: &Arc<Mailbox>) {
        let left: Vec<PopupId> = self
            .0
            .entries
            .iter()
            .rev()
            .filter(|e| Arc::ptr_eq(&e.owner, owner))
            .map(|e| e.id)
            .collect();
        for id in left {
            self.destroy(owner, id);
        }
        self.0.waiting.retain(|w| !Arc::ptr_eq(w, owner));
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
