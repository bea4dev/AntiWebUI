//! Whether the user has left the computer alone for a while (`ext_idle_notifier_v1`).

use crate::conn::{State, link};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1 as notification, ext_idle_notifier_v1 as notifier,
};

/// Becomes idle after a while without input, and stops being so at the next. Cheap to clone; it
/// stops watching when the last clone goes.
#[derive(Clone, Debug)]
pub struct Idle(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    object: Option<notification::ExtIdleNotificationV1>,
    idle: SharedSignal<bool>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let (Some(o), Some(link)) = (self.object.take(), link()) {
            o.destroy();
            link.flush();
        }
    }
}

impl Idle {
    /// Whether the compositor can tell.
    pub fn available(&self) -> bool {
        self.0.object.is_some()
    }

    /// Reading it tracks it, on any thread.
    pub fn is_idle(&self) -> bool {
        self.0.idle.get()
    }
}

static NOTIFIER: OnceLock<Option<notifier::ExtIdleNotifierV1>> = OnceLock::new();

/// Watches for `timeout` without input. Programs that keep the screen on, like a video player, hold
/// it off, as they would a screen saver.
pub fn idle(timeout: Duration) -> Idle {
    let idle = shared_signal(false);
    let notifier = NOTIFIER.get_or_init(|| link()?.bind(2, ())).as_ref();
    let object = match (notifier, link()) {
        (Some(n), Some(link)) => link.seat.as_ref().map(|seat| {
            let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            let o = n.get_idle_notification(ms, seat, &link.qh, idle.clone());
            link.flush();
            o
        }),
        _ => None,
    };
    Idle(Arc::new(Inner { object, idle }))
}

impl Dispatch<notification::ExtIdleNotificationV1, SharedSignal<bool>> for State {
    fn event(
        _: &mut Self,
        _: &notification::ExtIdleNotificationV1,
        event: notification::Event,
        idle: &SharedSignal<bool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            notification::Event::Idled => idle.set(true),
            notification::Event::Resumed => idle.set(false),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore notifier::ExtIdleNotifierV1);
