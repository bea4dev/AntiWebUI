//! The user's session through logind: suspending, powering off, locking, what logind says of
//! those, and the screen's brightness, which logind sets without root.

use crate::ServiceError;
use crate::dbus;
use crate::listeners::Listeners;
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use futures_lite::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use zbus::zvariant::OwnedObjectPath;

const LOGIN1: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
/// What logind takes for the caller's session.
const AUTO: &str = "/org/freedesktop/login1/session/auto";

/// What the computer can be asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionAction {
    Suspend,
    Hibernate,
    PowerOff,
    Reboot,
}

impl SessionAction {
    const ALL: [SessionAction; 4] = [Self::Suspend, Self::Hibernate, Self::PowerOff, Self::Reboot];

    fn method(self) -> &'static str {
        match self {
            Self::Suspend => "Suspend",
            Self::Hibernate => "Hibernate",
            Self::PowerOff => "PowerOff",
            Self::Reboot => "Reboot",
        }
    }
}

/// The session. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Session(&'static Shared);

#[derive(Debug)]
struct Shared {
    available: SharedSignal<bool>,
    /// What logind said it can do, in the order of [`SessionAction::ALL`].
    can: SharedSignal<[bool; 4]>,
    /// This session's object, once known.
    path: Mutex<Option<String>>,
    brightness: SharedSignal<Option<f64>>,
    lock: Listeners<()>,
    unlock: Listeners<()>,
    sleep: Listeners<bool>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The session. It starts following it the first time.
pub fn session() -> Session {
    Session(dbus::init_then_follow(&SHARED, follow, || {
        let shared = Shared {
            available: shared_signal(false),
            can: shared_signal([false; 4]),
            path: Mutex::new(None),
            brightness: shared_signal(None),
            lock: Listeners::new(),
            unlock: Listeners::new(),
            sleep: Listeners::new(),
        };
        follow_backlight(shared.brightness.clone());
        shared
    }))
}

impl Session {
    /// Whether logind is there.
    pub fn available(&self) -> bool {
        self.0.available.get()
    }

    /// Whether the computer can do it, perhaps after asking for a password.
    pub fn can(&self, action: SessionAction) -> bool {
        let at = SessionAction::ALL.iter().position(|a| *a == action);
        self.0.can.with(|c| at.is_some_and(|i| c[i]))
    }

    fn act(&self, action: SessionAction) -> Task<Result<(), ServiceError>> {
        dbus::run(async move {
            let conn = dbus::system().await?;
            // Interactive: polkit may ask for a password rather than refuse.
            dbus::call(
                &conn,
                LOGIN1,
                MANAGER_PATH,
                MANAGER,
                action.method(),
                &(true,),
            )
            .await
        })
    }

    pub fn suspend(&self) -> Task<Result<(), ServiceError>> {
        self.act(SessionAction::Suspend)
    }

    pub fn hibernate(&self) -> Task<Result<(), ServiceError>> {
        self.act(SessionAction::Hibernate)
    }

    pub fn power_off(&self) -> Task<Result<(), ServiceError>> {
        self.act(SessionAction::PowerOff)
    }

    pub fn reboot(&self) -> Task<Result<(), ServiceError>> {
        self.act(SessionAction::Reboot)
    }

    /// Asks for the session to be locked: whatever locks the screen hears it through
    /// [`Session::on_lock`].
    pub fn lock(&self) -> Task<Result<(), ServiceError>> {
        let path = self.path();
        dbus::run(async move {
            let conn = dbus::system().await?;
            dbus::call(&conn, LOGIN1, &path, SESSION, "Lock", &()).await
        })
    }

    /// Calls `f` on this thread whenever the session is to be locked, as by `loginctl
    /// lock-session`, until the owner this was called under goes.
    pub fn on_lock(&self, f: impl Fn() + 'static) {
        self.0.lock.listen(move |_| f());
    }

    /// Calls `f` on this thread whenever the session is to be unlocked, until the owner this was
    /// called under goes.
    pub fn on_unlock(&self, f: impl Fn() + 'static) {
        self.0.unlock.listen(move |_| f());
    }

    /// Calls `f` on this thread with true just before the computer sleeps and with false after it
    /// wakes, until the owner this was called under goes.
    pub fn on_prepare_for_sleep(&self, f: impl Fn(bool) + 'static) {
        self.0.sleep.listen(move |b| f(*b));
    }

    /// How bright the screen is, from 0 to 1, if it has a backlight.
    pub fn brightness(&self) -> Option<f64> {
        self.0.brightness.get()
    }

    /// Makes the screen this bright, from 0 to 1.
    pub fn set_brightness(&self, brightness: f64) -> Task<Result<(), ServiceError>> {
        let path = self.path();
        let signal = self.0.brightness.clone();
        dbus::run(async move {
            let light = backlight().ok_or(ServiceError::Unavailable)?;
            let max = light.max().ok_or(ServiceError::Unavailable)?;
            let value = (brightness.clamp(0.0, 1.0) * max as f64).round() as u32;
            let conn = dbus::system().await?;
            dbus::call::<_, ()>(
                &conn,
                LOGIN1,
                &path,
                SESSION,
                "SetBrightness",
                &("backlight", light.name.as_str(), value),
            )
            .await?;
            dbus::put(&signal, light.read());
            Ok(())
        })
    }

    /// This session's object, or the one logind takes for the caller's until that is known.
    fn path(&self) -> String {
        self.0
            .path
            .lock()
            .ok()
            .and_then(|p| p.clone())
            .unwrap_or_else(|| AUTO.to_owned())
    }
}

/// Finds this session's object: by `$XDG_SESSION_ID`, then the one logind takes for the
/// caller's, then by this process.
async fn find_session(conn: &zbus::Connection) -> Option<String> {
    let get = |id: String| async move {
        dbus::call::<_, OwnedObjectPath>(conn, LOGIN1, MANAGER_PATH, MANAGER, "GetSession", &(id,))
            .await
            .ok()
    };
    if let Ok(id) = std::env::var("XDG_SESSION_ID")
        && let Some(path) = get(id).await
    {
        return Some(path.to_string());
    }
    if let Some(path) = get("auto".to_owned()).await {
        return Some(path.to_string());
    }
    dbus::call::<_, OwnedObjectPath>(
        conn,
        LOGIN1,
        MANAGER_PATH,
        MANAGER,
        "GetSessionByPID",
        &(std::process::id(),),
    )
    .await
    .ok()
    .map(|p| p.to_string())
}

async fn follow() {
    let Ok(conn) = dbus::system().await else {
        return;
    };
    dbus::while_owned(
        &conn,
        LOGIN1,
        || serve(&conn),
        || {
            if let Some(s) = SHARED.get() {
                dbus::put(&s.available, false);
            }
        },
    )
    .await;
}

async fn serve(conn: &zbus::Connection) {
    let Some(s) = SHARED.get() else {
        return;
    };
    let Some(mut signals) = dbus::signals(conn, MANAGER_PATH, None).await else {
        return;
    };
    let path = find_session(conn).await;
    if let Ok(mut p) = s.path.lock() {
        p.clone_from(&path);
    }
    let mut can = [false; 4];
    for (i, action) in SessionAction::ALL.iter().enumerate() {
        let method = format!("Can{}", action.method());
        let answer: String = dbus::call(conn, LOGIN1, MANAGER_PATH, MANAGER, &method, &())
            .await
            .unwrap_or_default();
        can[i] = answer == "yes" || answer == "challenge";
    }
    dbus::put(&s.can, can);
    dbus::put(&s.available, true);
    while let Some(Ok(msg)) = signals.next().await {
        let Some((from, interface, member)) = dbus::signal_of(&msg) else {
            continue;
        };
        match (interface.as_str(), member.as_str()) {
            (MANAGER, "PrepareForSleep") => {
                if let Ok(sleeping) = msg.body().deserialize::<bool>() {
                    s.sleep.send(&sleeping);
                }
            }
            (SESSION, "Lock") if path.as_deref() == Some(&from) => s.lock.send(&()),
            (SESSION, "Unlock") if path.as_deref() == Some(&from) => s.unlock.send(&()),
            _ => {}
        }
    }
}

/// A backlight in `/sys/class/backlight`.
struct Backlight {
    name: String,
    dir: PathBuf,
}

impl Backlight {
    fn number(&self, file: &str) -> Option<u64> {
        std::fs::read_to_string(self.dir.join(file))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn max(&self) -> Option<u64> {
        self.number("max_brightness").filter(|m| *m > 0)
    }

    /// How bright it is, from 0 to 1.
    fn read(&self) -> Option<f64> {
        let now = self
            .number("actual_brightness")
            .or_else(|| self.number("brightness"))?;
        Some((now as f64 / self.max()? as f64).clamp(0.0, 1.0))
    }

    /// The file the kernel tells of changes to.
    fn watched(&self) -> PathBuf {
        let actual = self.dir.join("actual_brightness");
        if actual.exists() {
            actual
        } else {
            self.dir.join("brightness")
        }
    }
}

/// The backlight of the screen: one the firmware drives before one of the platform's, before one
/// of the GPU's own, as GNOME picks. `ANTIWEBUI_BACKLIGHT_DIR` looks elsewhere, as tests do.
fn backlight() -> Option<Backlight> {
    let root = std::env::var_os("ANTIWEBUI_BACKLIGHT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/sys/class/backlight"));
    let mut found: Vec<(u8, String, PathBuf)> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| {
            let dir = e.path();
            let kind = std::fs::read_to_string(dir.join("type")).unwrap_or_default();
            let rank = match kind.trim() {
                "firmware" => 0,
                "platform" => 1,
                "raw" => 2,
                _ => 3,
            };
            (rank, e.file_name().to_string_lossy().into_owned(), dir)
        })
        .collect();
    found.sort();
    let (_, name, dir) = found.into_iter().next()?;
    Some(Backlight { name, dir })
}

/// Reads the brightness now and whenever the kernel says it changed, on a thread of its own.
fn follow_backlight(signal: SharedSignal<Option<f64>>) {
    let Some(light) = backlight() else {
        return;
    };
    dbus::put(&signal, light.read());
    let Some(fd) = watch(&light.watched()) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("anti-webui backlight".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                // SAFETY: reading into a buffer of this size from the inotify fd this owns.
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    return;
                }
                dbus::put(&signal, light.read());
            }
        });
}

/// An inotify fd that wakes when `path` is written to.
fn watch(path: &Path) -> Option<i32> {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: plain calls; the fd stays open for the life of the process.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 {
            return None;
        }
        if libc::inotify_add_watch(fd, name.as_ptr(), libc::IN_MODIFY) < 0 {
            libc::close(fd);
            return None;
        }
        Some(fd)
    }
}
