//! niri's IPC against a small server in its place. Run with `--features niri`.
#![cfg(feature = "niri")]

use anti_webui_wayland_shell::serde_json::{Value, json};
use anti_webui_wayland_shell::{WorkspaceSource, niri, workspaces};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        anti_webui_core::channel::pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn niri_is_read_followed_and_told() {
    let path = std::env::temp_dir().join(format!("anti-webui-niri-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    // SAFETY: one test in this binary, before anything reads the environment.
    unsafe {
        std::env::set_var("NIRI_SOCKET", &path);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
    }
    let actions = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (event_tx, event_rx) = mpsc::channel::<Value>();
    let event_rx = Arc::new(Mutex::new(event_rx));
    let a = actions.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let s = stream.unwrap();
            let (a, events) = (a.clone(), event_rx.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut s = s;
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                s.write_all(b"{\"Ok\":\"Handled\"}\n").unwrap();
                if request == json!("EventStream") {
                    let start = [
                        json!({"WorkspacesChanged": {"workspaces": [
                            {"id": 5, "idx": 1, "name": null, "output": "DP-1", "is_active": true},
                            {"id": 6, "idx": 2, "name": "web", "output": "DP-1", "is_active": false}
                        ]}}),
                        json!({"WindowsChanged": {"windows": []}}),
                    ];
                    for e in start.into_iter().chain(events.lock().unwrap().iter()) {
                        let mut l = e.to_string();
                        l.push('\n');
                        if s.write_all(l.as_bytes()).is_err() {
                            return;
                        }
                    }
                } else {
                    a.lock().unwrap().push(request);
                }
            });
        }
    });

    let ws = workspaces();
    assert_eq!(ws.source(), Some(WorkspaceSource::Niri));
    let shown = || {
        ws.all()
            .iter()
            .map(|w| (w.name().to_owned(), w.is_active()))
            .collect::<Vec<_>>()
    };
    // What the stream starts with is there before the first read.
    assert_eq!(shown(), [("1".to_owned(), true), ("web".to_owned(), false)]);
    event_tx
        .send(json!({"WorkspaceActivated": {"id": 6, "focused": true}}))
        .unwrap();
    until("web to show", || shown()[1].1);
    assert!(!shown()[0].1);

    ws.all()[0].activate();
    until("the action", || !actions.lock().unwrap().is_empty());
    assert_eq!(
        actions.lock().unwrap()[0],
        json!({"Action": {"FocusWorkspace": {"reference": {"Id": 5}}}})
    );
    assert!(niri::available());
    let _ = std::fs::remove_file(&path);
}
