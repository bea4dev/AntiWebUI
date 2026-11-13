//! The xdg popups of all the windows on one connection, which share one popup grab.
//!
//! The compositor keeps one grab chain per client: a new grabbing popup must go on the topmost
//! grabbing popup, or on a window when no grab is on. Each window keeps its own popups, so before
//! one of them grabs, the grab of another window ends here the way a click outside would end it:
//! its popups are destroyed, top first, and that window gets `PopupDismissed` for each.

use anti_webui_core::platform::PopupId;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, MutexGuard};
use wayland_protocols::xdg::shell::client::xdg_popup;

/// Where a window learns which of its popups another window ended, and what wakes its thread.
pub(crate) struct Mailbox {
    ended: Mutex<Vec<PopupId>>,
    wake: OwnedFd,
}

impl Mailbox {
    pub fn new(wake: OwnedFd) -> Arc<Self> {
        Arc::new(Self {
            ended: Mutex::new(Vec::new()),
            wake,
        })
    }

    /// The popups ended since the last time. Their `xdg_popup` is destroyed already.
    pub fn take(&self) -> Vec<PopupId> {
        std::mem::take(&mut *lock(&self.ended))
    }
}

struct Entry {
    owner: Arc<Mailbox>,
    id: PopupId,
    /// The popups of the same window it is attached to.
    ancestors: Vec<PopupId>,
    popup: xdg_popup::XdgPopup,
    grabbing: bool,
}

/// The xdg popups of a connection, in the order they were made.
#[derive(Clone, Default)]
pub(crate) struct ConnectionPopups(Arc<Mutex<Vec<Entry>>>);

impl ConnectionPopups {
    /// Hold it while making a popup, so that no other thread ends what it goes on meanwhile.
    pub fn lock(&self) -> LockedPopups<'_> {
        LockedPopups(lock(&self.0))
    }
}

pub(crate) struct LockedPopups<'a>(MutexGuard<'a, Vec<Entry>>);

impl LockedPopups<'_> {
    /// Whether the popup of this window is still there.
    pub fn has(&self, owner: &Arc<Mailbox>, id: PopupId) -> bool {
        self.0
            .iter()
            .any(|e| Arc::ptr_eq(&e.owner, owner) && e.id == id)
    }

    /// Ends every grabbing popup that a new grabbing popup with these ancestors can't go on top
    /// of, with the popups attached to them.
    pub fn end_other_grabs(&mut self, owner: &Arc<Mailbox>, ancestors: &[PopupId]) {
        let ended: Vec<(Arc<Mailbox>, PopupId)> = self
            .0
            .iter()
            .filter(|e| e.grabbing && !(Arc::ptr_eq(&e.owner, owner) && ancestors.contains(&e.id)))
            .map(|e| (e.owner.clone(), e.id))
            .collect();
        if ended.is_empty() {
            return;
        }
        let goes = |e: &Entry| {
            ended.iter().any(|(o, id)| {
                Arc::ptr_eq(o, &e.owner) && (e.id == *id || e.ancestors.contains(id))
            })
        };
        let (gone, kept): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut *self.0)
            .into_iter()
            .partition(|e| goes(e));
        *self.0 = kept;
        // A popup comes after what it is attached to, so the topmost goes first.
        for e in gone.iter().rev() {
            e.popup.destroy();
        }
        let mut woken: Vec<&Arc<Mailbox>> = Vec::new();
        for e in &gone {
            lock(&e.owner.ended).push(e.id);
            if !woken.iter().any(|o| Arc::ptr_eq(o, &e.owner)) {
                woken.push(&e.owner);
            }
        }
        for owner in woken {
            let _ = rustix::io::write(&owner.wake, &[1]);
        }
    }

    pub fn add(
        &mut self,
        owner: &Arc<Mailbox>,
        id: PopupId,
        ancestors: Vec<PopupId>,
        popup: xdg_popup::XdgPopup,
        grabbing: bool,
    ) {
        self.0.push(Entry {
            owner: owner.clone(),
            id,
            ancestors,
            popup,
            grabbing,
        });
    }

    /// Takes the popup out and destroys its `xdg_popup`, unless another window ended it already.
    /// Under the lock, so that it can't be destroyed after what it is attached to.
    pub fn destroy(&mut self, owner: &Arc<Mailbox>, id: PopupId) {
        if let Some(at) = self
            .0
            .iter()
            .position(|e| Arc::ptr_eq(&e.owner, owner) && e.id == id)
        {
            self.0.remove(at).popup.destroy();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
