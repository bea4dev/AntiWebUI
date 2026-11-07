//! Handing what a service hears to the UI threads that listen for it.

use anti_webui_core::channel::{UiSender, ui_channel};
use anti_webui_core::reactive::on_cleanup;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The UI threads that listen for something, each until the owner it listened under goes.
pub(crate) struct Listeners<T> {
    list: Mutex<Vec<(UiSender<T>, Arc<AtomicBool>)>>,
}

impl<T: Clone + Send + 'static> Listeners<T> {
    pub const fn new() -> Self {
        Self {
            list: Mutex::new(Vec::new()),
        }
    }

    pub fn listen(&self, f: impl Fn(&T) + 'static) {
        let alive = Arc::new(AtomicBool::new(true));
        let sender = ui_channel(move |t: T| f(&t));
        let a = alive.clone();
        on_cleanup(move || a.store(false, Ordering::Relaxed));
        if let Ok(mut list) = self.list.lock() {
            list.push((sender, alive));
        }
    }

    pub fn send(&self, t: &T) {
        if let Ok(mut list) = self.list.lock() {
            list.retain(|(_, alive)| alive.load(Ordering::Relaxed));
            for (sender, _) in list.iter() {
                sender.send(t.clone());
            }
        }
    }
}

impl<T> std::fmt::Debug for Listeners<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Listeners")
    }
}
