//! What the D-Bus tests share: a bus of their own, standing in for both the session and the system
//! bus, and waiting while the UI thread's channels are drained.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A `dbus-daemon` of the test's own, gone when this is dropped.
pub struct PrivateBus {
    child: Child,
    pub address: String,
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts a bus and points both `DBUS_SESSION_BUS_ADDRESS` and `DBUS_SYSTEM_BUS_ADDRESS` at it.
/// Call it first, before anything in the test binary connects: one test per binary.
pub fn private_bus() -> PrivateBus {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon to start");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("its output"))
        .read_line(&mut line)
        .expect("its address");
    let address = line.trim().to_owned();
    // SAFETY: before any thread of this binary reads the environment.
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &address);
    }
    PrivateBus { child, address }
}

/// A connection to the private bus for a fake service, owning `name` if given.
pub fn connect(name: Option<&str>) -> zbus::Connection {
    zbus::block_on(async {
        let mut b = zbus::connection::Builder::session().unwrap();
        if let Some(name) = name {
            b = b.name(name.to_owned()).unwrap();
        }
        b.build().await.unwrap()
    })
}

/// Waits for `done`, draining what came for this thread's UI channels meanwhile; at most 5 s.
pub fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        anti_webui_core::channel::pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}
