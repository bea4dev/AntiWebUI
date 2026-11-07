//! Readings of this machine, which only reads. Run with `--features system`.
#![cfg(feature = "system")]

use anti_webui_wayland_shell::{system, system_every};
use std::time::{Duration, Instant};

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn this_machine_is_read() {
    let sys = system_every(Duration::from_millis(100));
    until("memory", || sys.memory().total > 0);
    let m = sys.memory();
    assert!(m.available <= m.total && m.used == m.total - m.available);
    until("the uptime", || sys.uptime() > Duration::ZERO);
    let cpus = sys.cpus();
    assert!(!cpus.is_empty());
    // After a second reading the share is of the time between.
    std::thread::sleep(Duration::from_millis(250));
    let cpu = sys.cpu();
    assert!((0.0..=1.0).contains(&cpu), "{cpu}");
    assert!(sys.cpus().iter().all(|c| (0.0..=1.0).contains(c)));
    let disks = sys.disks();
    assert!(disks.iter().any(|d| d.mount == "/"), "{disks:?}");
    assert!(disks.iter().all(|d| d.available <= d.total));
    let net = sys.network();
    assert!(net.rx_bytes_per_sec >= 0.0 && net.tx_bytes_per_sec >= 0.0);
    let (one, _, _) = sys.load();
    assert!(one >= 0.0);
    // A second, slower handle shares the readings.
    let slow = system();
    assert_eq!(slow.memory().total, m.total);
    eprintln!(
        "cpu {:.0}% over {} cpus, memory {} of {} MiB, cpu {:?} °C, {} temperatures, disks {:?}, net {:.0}/{:.0} B/s, load {:?}, up {:?}",
        cpu * 100.0,
        cpus.len(),
        m.used >> 20,
        m.total >> 20,
        sys.cpu_temperature(),
        sys.temperatures().len(),
        disks.iter().map(|d| &d.mount).collect::<Vec<_>>(),
        net.rx_bytes_per_sec,
        net.tx_bytes_per_sec,
        sys.load(),
        sys.uptime()
    );
}
