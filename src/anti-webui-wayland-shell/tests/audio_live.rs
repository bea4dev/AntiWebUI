//! Reads the PipeWire of the machine this runs on, changing nothing. Not run by default:
//! `cargo test -p anti-webui-wayland-shell --features audio --test audio_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]
#![cfg(feature = "audio")]

use anti_webui_wayland_shell::audio;
use std::time::{Duration, Instant};

#[test]
#[ignore = "reads this machine's PipeWire"]
fn what_this_machine_plays_through() {
    let a = audio();
    let start = Instant::now();
    while (a.sinks().is_empty() || a.default_sink().is_none())
        && start.elapsed() < Duration::from_secs(3)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(a.available());
    eprintln!("default sink: {:#?}", a.default_sink());
    eprintln!("default source: {:#?}", a.default_source());
    eprintln!("sinks: {:#?}", a.sinks());
    eprintln!("sources: {:#?}", a.sources());
    eprintln!("streams: {:#?}", a.streams());
    assert!(!a.sinks().is_empty());
}

/// A null sink of the test's own, removed when this is dropped.
struct NullSink(String);

impl Drop for NullSink {
    fn drop(&mut self) {
        let _ = std::process::Command::new("pactl")
            .args(["unload-module", &self.0])
            .status();
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "adds a null sink to this machine's PipeWire for a moment"]
fn a_sink_of_its_own_is_turned_down_and_muted() {
    until("the default sink", || audio().default_sink().is_some());
    let default_before = audio().default_sink().map(|d| d.name);
    let out = std::process::Command::new("pactl")
        .args([
            "load-module",
            "module-null-sink",
            "sink_name=anti_webui_audio_test",
            "sink_properties=node.description=anti-webui-test",
        ])
        .output()
        .expect("pactl");
    let _sink = NullSink(String::from_utf8_lossy(&out.stdout).trim().to_owned());
    let a = audio();
    let find = || {
        a.sinks()
            .into_iter()
            .find(|d| d.name == "anti_webui_audio_test")
    };
    until("the sink", || find().is_some_and(|d| d.channels > 0));
    // A node just made doesn't take changes until it has settled.
    std::thread::sleep(Duration::from_millis(500));
    let sink = find().unwrap();
    assert_eq!(sink.description, "anti-webui-test");
    sink.set_volume(0.5);
    until("half volume", || {
        find().is_some_and(|d| (d.volume - 0.5).abs() < 0.01)
    });
    // wpctl shows the same scale.
    let shown = std::process::Command::new("wpctl")
        .args(["get-volume", &sink.id.to_string()])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&shown.stdout).contains("0.50"));
    sink.set_muted(true);
    until("muted", || find().is_some_and(|d| d.muted));
    drop(_sink);
    until("the sink to go", || find().is_none());
    assert_eq!(a.default_sink().map(|d| d.name), default_before);
}
