//! Power profiles through power-profiles-daemon (or tuned's stand-in for it): power saver, balanced
//! and performance.

use crate::ServiceError;
use crate::dbus::{self, Props, get, put, string};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use futures_lite::StreamExt;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use zbus::zvariant::{OwnedValue, Value};

/// The name it has had since 0.20, and the one before.
const NAMES: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];

/// The power profiles. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct PowerProfiles(&'static Shared);

#[derive(Debug)]
struct Shared {
    available: SharedSignal<bool>,
    active: SharedSignal<String>,
    profiles: SharedSignal<Vec<String>>,
    degraded: SharedSignal<Option<String>>,
    /// The name and path in use.
    at: Mutex<Option<(&'static str, &'static str)>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The power profiles. It starts following them the first time.
pub fn power_profiles() -> PowerProfiles {
    PowerProfiles(SHARED.get_or_init(|| {
        dbus::spawn(follow());
        Shared {
            available: shared_signal(false),
            active: shared_signal(String::new()),
            profiles: shared_signal(Vec::new()),
            degraded: shared_signal(None),
            at: Mutex::new(None),
        }
    }))
}

impl PowerProfiles {
    pub fn available(&self) -> bool {
        self.0.available.get()
    }

    /// The profile in use, like `power-saver`, `balanced` or `performance`.
    pub fn active(&self) -> String {
        self.0.active.get()
    }

    /// The profiles there are, in the order the daemon gives them.
    pub fn profiles(&self) -> Vec<String> {
        self.0.profiles.get()
    }

    /// Why `performance` is held back, like `lap-detected` or `high-operating-temperature`.
    pub fn degraded(&self) -> Option<String> {
        self.0.degraded.get()
    }

    pub fn set_active(&self, profile: &str) -> Task<Result<(), ServiceError>> {
        let at = self.0.at.lock().ok().and_then(|a| *a);
        let profile = profile.to_owned();
        dbus::run(async move {
            let (name, path) = at.ok_or(ServiceError::Unavailable)?;
            let conn = dbus::system().await?;
            dbus::set_property(
                &conn,
                name,
                path,
                name,
                "ActiveProfile",
                Value::from(profile),
            )
            .await
        })
    }
}

fn publish(p: &Props) {
    let Some(s) = SHARED.get() else {
        return;
    };
    put(&s.available, !p.is_empty());
    put(&s.active, string(p, "ActiveProfile"));
    let profiles: Vec<HashMap<String, OwnedValue>> = get(p, "Profiles").unwrap_or_default();
    put(
        &s.profiles,
        profiles
            .iter()
            .filter_map(|m| get::<String>(m, "Profile"))
            .collect(),
    );
    put(
        &s.degraded,
        Some(string(p, "PerformanceDegraded")).filter(|d| !d.is_empty()),
    );
}

async fn follow() {
    let Ok(conn) = dbus::system().await else {
        return;
    };
    // The new name, unless only the old one is there.
    let mut at = NAMES[0];
    if let Ok(bus) = zbus::fdo::DBusProxy::new(&conn).await {
        for (name, path) in NAMES {
            if let Ok(n) = name.try_into()
                && bus.name_has_owner(n).await.unwrap_or(false)
            {
                at = (name, path);
                break;
            }
        }
    }
    if let Some(s) = SHARED.get()
        && let Ok(mut a) = s.at.lock()
    {
        *a = Some(at);
    }
    let (name, path) = at;
    dbus::while_owned(
        &conn,
        name,
        || serve(&conn, name, path),
        || publish(&Props::new()),
    )
    .await;
}

async fn serve(conn: &zbus::Connection, name: &'static str, path: &'static str) {
    let Some(mut signals) = dbus::signals(conn, path, None).await else {
        return;
    };
    let mut props = dbus::get_all(conn, name, path, name)
        .await
        .unwrap_or_default();
    publish(&props);
    while let Some(Ok(msg)) = signals.next().await {
        if let Some((_, changed, _)) = dbus::properties_changed(&msg) {
            props.extend(changed);
            publish(&props);
        }
    }
}
