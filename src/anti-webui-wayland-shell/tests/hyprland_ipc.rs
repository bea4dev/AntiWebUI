//! Hyprland's IPC against a small server in its place: what is read, what follows events, and what
//! is sent. Run with `--features hyprland`.
#![cfg(feature = "hyprland")]

use anti_webui_wayland_shell::hyprland::{self, Event};
use anti_webui_wayland_shell::{WorkspaceSource, workspaces};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Hypr {
    active: i64,
    asked: Vec<String>,
}

fn reply(h: &Hypr, request: &str) -> String {
    match request {
        "j/monitors" => format!(
            r#"[{{"id":0,"name":"DP-1","scale":1.0,"focused":true,
                "activeWorkspace":{{"id":{0},"name":"{0}"}},"specialWorkspace":{{"id":0,"name":""}}}}]"#,
            h.active
        ),
        "j/workspaces" => r#"[{"id":2,"name":"2","monitor":"DP-1","windows":0},
                              {"id":1,"name":"1","monitor":"DP-1","windows":1},
                              {"id":-98,"name":"special:magic","monitor":"DP-1","windows":0}]"#
            .into(),
        "j/clients" => "[]".into(),
        "j/activewindow" => "{}".into(),
        r if r.starts_with("dispatch ") => "ok".into(),
        _ => "unknown request".into(),
    }
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
fn hyprland_is_read_followed_and_told() {
    let dir = std::env::temp_dir().join(format!("anti-webui-hypr-{}", std::process::id()));
    let sockets = dir.join("hypr").join("test");
    std::fs::create_dir_all(&sockets).unwrap();
    let requests = UnixListener::bind(sockets.join(".socket.sock")).unwrap();
    let events = UnixListener::bind(sockets.join(".socket2.sock")).unwrap();
    // SAFETY: one test in this binary, before anything reads the environment.
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "test");
    }
    let hypr = Arc::new(Mutex::new(Hypr {
        active: 1,
        ..Default::default()
    }));
    let h = hypr.clone();
    std::thread::spawn(move || {
        for stream in requests.incoming() {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let mut h = h.lock().unwrap();
            let answer = reply(&h, &request);
            h.asked.push(request);
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    let (send_event, event_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let (mut stream, _) = events.accept().unwrap();
        for line in event_rx {
            stream.write_all(line.as_bytes()).unwrap();
        }
    });

    assert!(hyprland::available());
    let ws = workspaces();
    assert_eq!(ws.source(), Some(WorkspaceSource::Hyprland));
    let shown = |ws: &anti_webui_wayland_shell::Workspaces| {
        ws.all()
            .iter()
            .map(|w| (w.name().to_owned(), w.is_active(), w.is_hidden()))
            .collect::<Vec<_>>()
    };
    // In order, with the special one last and hidden.
    assert_eq!(
        shown(&ws),
        [
            ("1".to_owned(), true, false),
            ("2".to_owned(), false, false),
            ("special:magic".to_owned(), false, true)
        ]
    );

    let heard = Rc::new(RefCell::new(Vec::new()));
    let h2 = heard.clone();
    hyprland::on_event(move |e| h2.borrow_mut().push(e.clone()));

    // Hyprland goes to workspace 2 and says so.
    hypr.lock().unwrap().active = 2;
    send_event
        .send("workspace>>2\nworkspacev2>>2,2\n".into())
        .unwrap();
    until("workspace 2 to be active", || shown(&ws)[1].1);
    until("the events", || heard.borrow().len() == 2);
    assert_eq!(
        heard.borrow()[1],
        Event::Workspace {
            id: 2,
            name: "2".into()
        }
    );

    // Activating one sends the dispatcher.
    let done = Rc::new(RefCell::new(None));
    let d = done.clone();
    ws.all()[0]
        .activate()
        .then(move |r| *d.borrow_mut() = Some(r));
    until("the reply", || done.borrow().is_some());
    assert_eq!(*done.borrow(), Some(Ok(())));
    assert!(
        hypr.lock()
            .unwrap()
            .asked
            .contains(&"dispatch workspace 1".to_owned())
    );
    let d = done.clone();
    ws.all()[2]
        .activate()
        .then(move |r| *d.borrow_mut() = Some(r));
    until("the reply", || {
        hypr.lock()
            .unwrap()
            .asked
            .contains(&"dispatch togglespecialworkspace magic".to_owned())
    });
    let _ = std::fs::remove_dir_all(&dir);
}
