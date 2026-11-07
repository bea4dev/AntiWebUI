//! Sway's IPC against a small server in its place. Run with `--features sway`.
#![cfg(feature = "sway")]

use anti_webui_wayland_shell::sway;
use anti_webui_wayland_shell::{WorkspaceSource, workspaces};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn message(kind: u32, payload: &str) -> Vec<u8> {
    let mut m = b"i3-ipc".to_vec();
    m.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    m.extend_from_slice(&kind.to_ne_bytes());
    m.extend_from_slice(payload.as_bytes());
    m
}

fn read(s: &mut UnixStream) -> Option<(u32, String)> {
    let mut head = [0u8; 14];
    s.read_exact(&mut head).ok()?;
    let len = u32::from_ne_bytes(head[6..10].try_into().unwrap()) as usize;
    let kind = u32::from_ne_bytes(head[10..14].try_into().unwrap());
    let mut p = vec![0u8; len];
    s.read_exact(&mut p).ok()?;
    Some((kind, String::from_utf8(p).unwrap()))
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        anti_webui_core::channel::pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn sway_is_read_followed_and_told() {
    let path = std::env::temp_dir().join(format!("anti-webui-sway-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    // SAFETY: one test in this binary, before anything reads the environment.
    unsafe {
        std::env::set_var("SWAYSOCK", &path);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        std::env::remove_var("NIRI_SOCKET");
    }
    let visible = Arc::new(Mutex::new("1".to_owned()));
    let commands = Arc::new(Mutex::new(Vec::<String>::new()));
    let (event_tx, event_rx) = mpsc::channel::<()>();
    let event_rx = Arc::new(Mutex::new(event_rx));
    let (v, c) = (visible.clone(), commands.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let (v, c, events) = (v.clone(), c.clone(), event_rx.clone());
            std::thread::spawn(move || {
                while let Some((kind, payload)) = read(&mut s) {
                    let answer = match kind {
                        0 => {
                            c.lock().unwrap().push(payload);
                            r#"[{"success":true}]"#.to_owned()
                        }
                        1 => {
                            let v = v.lock().unwrap();
                            format!(
                                r#"[{{"id":10,"num":1,"name":"1","output":"DP-1","visible":{}}},
                                    {{"id":11,"num":2,"name":"2: web","output":"DP-1","visible":{}}}]"#,
                                *v == "1",
                                *v != "1"
                            )
                        }
                        2 => {
                            s.write_all(&message(2, r#"{"success":true}"#)).unwrap();
                            // This connection now carries events.
                            for () in events.lock().unwrap().iter() {
                                let e = message(0x8000_0000, r#"{"change":"focus"}"#);
                                if s.write_all(&e).is_err() {
                                    return;
                                }
                            }
                            return;
                        }
                        _ => "null".to_owned(),
                    };
                    s.write_all(&message(kind, &answer)).unwrap();
                }
            });
        }
    });

    let ws = workspaces();
    assert_eq!(ws.source(), Some(WorkspaceSource::Sway));
    let names = || {
        ws.all()
            .iter()
            .map(|w| (w.name().to_owned(), w.is_active(), w.coordinates().to_vec()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(),
        [
            ("1".to_owned(), true, vec![1]),
            ("2: web".to_owned(), false, vec![2])
        ]
    );
    *visible.lock().unwrap() = "2".into();
    event_tx.send(()).unwrap();
    until("workspace 2 to show", || names()[1].1);

    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let d = done.clone();
    ws.all()[1].activate().then(move |r| {
        assert_eq!(r, Ok(()));
        d.set(true)
    });
    until("the command", || done.get());
    assert_eq!(*commands.lock().unwrap(), [r#"workspace "2: web""#]);
    assert!(sway::available());
    let _ = std::fs::remove_file(&path);
}
