//! The session against a small logind on a bus of the test's own, with backlights in a directory of
//! its own. Run with `--features session`.

#![cfg(target_os = "linux")]
#![cfg(feature = "session")]

mod common;

use anti_webui_wayland_shell::{SessionAction, session};
use common::{private_bus, until};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;

const SESSION: &str = "/org/freedesktop/login1/session/_331";

type Log = Arc<Mutex<Vec<String>>>;

struct Manager {
    log: Log,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    fn suspend(&self, interactive: bool) {
        self.log
            .lock()
            .unwrap()
            .push(format!("suspend {interactive}"));
    }

    fn power_off(&self, interactive: bool) {
        self.log
            .lock()
            .unwrap()
            .push(format!("power off {interactive}"));
    }

    fn can_suspend(&self) -> String {
        "yes".into()
    }

    fn can_hibernate(&self) -> String {
        "na".into()
    }

    fn can_power_off(&self) -> String {
        "challenge".into()
    }

    fn can_reboot(&self) -> String {
        "no".into()
    }

    fn get_session(&self, id: String) -> zbus::fdo::Result<OwnedObjectPath> {
        if id == "31" {
            Ok(OwnedObjectPath::try_from(SESSION).unwrap())
        } else {
            Err(zbus::fdo::Error::Failed(format!("no session {id}")))
        }
    }
}

struct Session {
    log: Log,
    backlights: PathBuf,
}

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl Session {
    #[zbus(name = "Lock")]
    async fn lock(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
        self.log.lock().unwrap().push("lock".into());
        let _ = Self::locked(&e).await;
    }

    fn set_brightness(&self, subsystem: String, name: String, value: u32) {
        self.log
            .lock()
            .unwrap()
            .push(format!("brightness {subsystem} {name} {value}"));
        // As some GPUs do, the hardware gets less than what was asked for.
        let dir = self.backlights.join(name);
        std::fs::write(dir.join("brightness"), format!("{value}\n")).unwrap();
        std::fs::write(
            dir.join("actual_brightness"),
            format!("{}\n", value * 9 / 10),
        )
        .unwrap();
    }

    #[zbus(signal, name = "Lock")]
    async fn locked(e: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn unlock(e: &SignalEmitter<'_>) -> zbus::Result<()>;
}

fn backlight(root: &std::path::Path, name: &str, kind: &str, max: u32, now: u32) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
    std::fs::write(dir.join("max_brightness"), format!("{max}\n")).unwrap();
    std::fs::write(dir.join("brightness"), format!("{now}\n")).unwrap();
    std::fs::write(dir.join("actual_brightness"), format!("{now}\n")).unwrap();
}

fn wait<T: Clone + 'static>(task: anti_webui_core::task::Task<T>) -> T {
    let got = Rc::new(RefCell::new(None));
    let g = got.clone();
    task.then(move |r| *g.borrow_mut() = Some(r));
    until("the reply", || got.borrow().is_some());
    got.borrow().clone().unwrap()
}

#[test]
fn logind_is_asked_and_heard_and_the_backlight_followed() {
    let _bus = private_bus();
    let backlights =
        std::env::temp_dir().join(format!("anti-webui-backlight-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&backlights);
    // The firmware's goes before the GPU's.
    backlight(&backlights, "intel_backlight", "raw", 96000, 1000);
    backlight(&backlights, "acpi_video0", "firmware", 1000, 500);
    // SAFETY: before anything in this binary reads the environment.
    unsafe {
        std::env::set_var("ANTIWEBUI_BACKLIGHT_DIR", &backlights);
        std::env::set_var("XDG_SESSION_ID", "31");
    }
    let log = Log::default();
    let logind = zbus::block_on({
        let (log, backlights) = (log.clone(), backlights.clone());
        async move {
            zbus::connection::Builder::session()
                .unwrap()
                .serve_at("/org/freedesktop/login1", Manager { log: log.clone() })
                .unwrap()
                .serve_at(SESSION, Session { log, backlights })
                .unwrap()
                .name("org.freedesktop.login1")
                .unwrap()
                .build()
                .await
                .unwrap()
        }
    });

    let s = session();
    until("logind", || s.available());
    assert_eq!(s.brightness(), Some(0.5));
    assert!(s.can(SessionAction::Suspend));
    assert!(!s.can(SessionAction::Hibernate));
    assert!(s.can(SessionAction::PowerOff));
    assert!(!s.can(SessionAction::Reboot));

    assert_eq!(wait(s.suspend()), Ok(()));
    assert_eq!(wait(s.power_off()), Ok(()));
    // Hibernating isn't there: the error says so.
    assert_eq!(
        wait(s.hibernate()),
        Err(anti_webui_wayland_shell::ServiceError::Unsupported)
    );

    // Locking goes to this session, and what logind says of it comes back.
    let heard = Rc::new(RefCell::new(Vec::new()));
    let h = heard.clone();
    s.on_lock(move || h.borrow_mut().push("lock".to_owned()));
    let h = heard.clone();
    s.on_unlock(move || h.borrow_mut().push("unlock".to_owned()));
    let h = heard.clone();
    s.on_prepare_for_sleep(move |b| h.borrow_mut().push(format!("sleep {b}")));
    assert_eq!(wait(s.lock()), Ok(()));
    until("the lock", || heard.borrow().len() == 1);
    zbus::block_on(async {
        logind
            .emit_signal(
                None::<zbus::names::BusName<'_>>,
                SESSION,
                "org.freedesktop.login1.Session",
                "Unlock",
                &(),
            )
            .await
            .unwrap();
        logind
            .emit_signal(
                None::<zbus::names::BusName<'_>>,
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                "PrepareForSleep",
                &(true,),
            )
            .await
            .unwrap();
        // Another session's locking isn't this one's.
        logind
            .emit_signal(
                None::<zbus::names::BusName<'_>>,
                "/org/freedesktop/login1/session/_332",
                "org.freedesktop.login1.Session",
                "Lock",
                &(),
            )
            .await
            .unwrap();
    });
    until("unlock and sleep", || heard.borrow().len() == 3);
    std::thread::sleep(std::time::Duration::from_millis(50));
    anti_webui_core::channel::pump();
    assert_eq!(*heard.borrow(), ["lock", "unlock", "sleep true"]);

    // Brightness is set through logind, on the firmware's backlight, and reads as what was asked
    // for even where the hardware gets less.
    assert_eq!(wait(s.set_brightness(0.8)), Ok(()));
    until("brighter", || s.brightness() == Some(0.8));
    // And followed when something else changes it: the kernel tells of `actual_brightness`.
    let light = backlights.join("acpi_video0");
    std::fs::write(light.join("brightness"), "250\n").unwrap();
    std::fs::write(light.join("actual_brightness"), "225\n").unwrap();
    until("dimmer", || s.brightness() == Some(0.25));

    let log = log.lock().unwrap().clone();
    assert_eq!(
        log,
        [
            "suspend true",
            "power off true",
            "lock",
            "brightness backlight acpi_video0 800"
        ]
    );
    let _ = std::fs::remove_dir_all(&backlights);
}

/// Reads this machine's logind and backlight, changing nothing:
/// `cargo test -p anti-webui-wayland-shell --features session --test session_bus -- --ignored --nocapture`.
#[test]
#[ignore = "reads this machine's logind"]
fn this_machine() {
    let s = session();
    std::thread::sleep(std::time::Duration::from_millis(800));
    eprintln!(
        "{} brightness {:?} suspend {} hibernate {} power off {} reboot {}",
        s.available(),
        s.brightness(),
        s.can(SessionAction::Suspend),
        s.can(SessionAction::Hibernate),
        s.can(SessionAction::PowerOff),
        s.can(SessionAction::Reboot)
    );
}
