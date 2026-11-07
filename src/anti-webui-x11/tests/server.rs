//! The backend against a real X server: Xvfb, started for the tests, or skipped when it isn't
//! installed. Input goes in through XTEST. Xvfb has no DRI3, so nothing is presented here.

use anti_webui_core::event::{
    ClipboardKind, DragAction, DragActions, DragResult, Key, PointerButton, ScrollSource,
    WindowEvent,
};
use anti_webui_core::geometry::{PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{DragOffer, Placement, PopupId, PopupKind, PopupParams};
use anti_webui_core::window::{DecorationMode, Decorations};
use anti_webui_x11::{Ctx, ParentLink, WindowOptions, X11Handler};
use std::process::{Child, Command};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const DISPLAY: &str = ":93";
const DPI: &str = "Xft.dpi:\t144\n";

/// One server for all tests, which take turns with it.
fn server() -> Option<std::sync::MutexGuard<'static, ()>> {
    // The connection stays for as long as the tests run: the server goes with it (`-terminate`).
    static SERVER: OnceLock<Option<(Child, RustConnection)>> = OnceLock::new();
    static TURN: Mutex<()> = Mutex::new(());
    let started = SERVER.get_or_init(|| {
        let child = Command::new("Xvfb")
            .args([
                DISPLAY,
                "-screen",
                "0",
                "1280x800x24",
                "-nolisten",
                "tcp",
                "-terminate",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let socket = format!("/tmp/.X11-unix/X{}", &DISPLAY[1..]);
        let until = Instant::now() + Duration::from_secs(10);
        while !std::path::Path::new(&socket).exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: before any test's threads use the environment.
        unsafe { std::env::set_var("DISPLAY", DISPLAY) };
        let (conn, screen) = RustConnection::connect(Some(DISPLAY)).ok()?;
        let root = conn.setup().roots[screen].root;
        conn.change_property8(
            PropMode::REPLACE,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            DPI.as_bytes(),
        )
        .ok()?;
        conn.sync().ok()?;
        Some((child, conn))
    });
    if started.is_none() {
        eprintln!("Xvfb isn't installed; skipping");
        return None;
    }
    Some(TURN.lock().unwrap_or_else(|e| e.into_inner()))
}

enum Do {
    SetClipboard(ClipboardKind, String),
    Paste(ClipboardKind),
    ShowPopup(PopupParams),
    StartDrag(DragOffer),
    Exit,
}

/// Records what comes, and does what the test says between events.
struct Recorder {
    events: Sender<WindowEvent>,
    todo: Receiver<Do>,
}

impl X11Handler for Recorder {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
        // What the core would do for something that takes the first format it is offered.
        match &event {
            WindowEvent::DragEntered { mimes, .. } => {
                ctx.accept_drag(mimes.first().map(|m| (m.clone(), DragAction::Copy)));
            }
            WindowEvent::DragDropped { .. } => ctx.receive_drop("text/plain;charset=utf-8".into()),
            _ => {}
        }
        let _ = self.events.send(event);
    }
    fn frame(&mut self, _: &mut Ctx<'_>) {}
    fn deadline(&mut self, now: Instant) -> Option<Instant> {
        Some(now + Duration::from_millis(5))
    }
    fn timer(&mut self, ctx: &mut Ctx<'_>, _: Instant) {
        while let Ok(what) = self.todo.try_recv() {
            match what {
                Do::SetClipboard(kind, text) => ctx.set_clipboard(kind, text),
                Do::Paste(kind) => ctx.request_paste(kind),
                Do::ShowPopup(params) => ctx.show_popup(params),
                Do::StartDrag(offer) => ctx.start_drag(offer),
                Do::Exit => ctx.exit(),
            }
        }
    }
}

/// The app's window, running on its own thread.
struct App {
    events: Receiver<WindowEvent>,
    todo: Sender<Do>,
    thread: Option<std::thread::JoinHandle<()>>,
    conn: RustConnection,
    root: Window,
    window: Window,
}

impl App {
    fn start(title: &str) -> App {
        Self::start_with(title, |_| {})
    }

    fn start_with(title: &str, change: impl FnOnce(&mut WindowOptions)) -> App {
        let (events_tx, events) = channel();
        let (todo, todo_rx) = channel();
        let mut options = WindowOptions {
            title: title.into(),
            app_id: "test".into(),
            width: 400,
            height: 300,
            ..Default::default()
        };
        change(&mut options);
        let thread = std::thread::spawn(move || {
            let recorder = Recorder {
                events: events_tx,
                todo: todo_rx,
            };
            anti_webui_x11::run(options, recorder).unwrap();
        });
        let (conn, screen) = RustConnection::connect(Some(DISPLAY)).unwrap();
        let root = conn.setup().roots[screen].root;
        let mut app = App {
            events,
            todo,
            thread: Some(thread),
            conn,
            root,
            window: 0,
        };
        let until = Instant::now() + Duration::from_secs(5);
        while app.window == 0 && Instant::now() < until {
            app.window = app.find(title).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(app.window, 0, "the window opened");
        // Mapped, and with the input focus.
        app.wait_for(|e| matches!(e, WindowEvent::Resized { .. }));
        std::thread::sleep(Duration::from_millis(100));
        app.conn
            .set_input_focus(
                x11rb::protocol::xproto::InputFocus::PARENT,
                app.window,
                x11rb::CURRENT_TIME,
            )
            .unwrap();
        app.conn.sync().unwrap();
        app
    }

    fn find(&self, title: &str) -> Option<Window> {
        let tree = self.conn.query_tree(self.root).ok()?.reply().ok()?;
        tree.children.into_iter().find(|&w| {
            self.conn
                .get_property(false, w, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 64)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| r.value == title.as_bytes())
        })
    }

    /// Waits for an event the test wants, returning it; others before it are dropped.
    fn wait_for(&self, wanted: impl Fn(&WindowEvent) -> bool) -> WindowEvent {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(e) if wanted(&e) => return e,
                Ok(_) => {}
                Err(_) => panic!("the event didn't come"),
            }
        }
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        self.conn
            .xtest_fake_input(kind, detail, 0, self.root, x, y, 0)
            .unwrap();
        self.conn.sync().unwrap();
    }

    fn keycode(&self, sym: u32) -> u8 {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)
            .unwrap()
            .reply()
            .unwrap();
        let per = map.keysyms_per_keycode as usize;
        let i = map.keysyms.iter().position(|&s| s == sym).unwrap();
        min + (i / per) as u8
    }

    fn stop(mut self) {
        let _ = self.todo.send(Do::Exit);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION: u8 = 6;

#[test]
fn the_window_opens_at_the_scale_of_the_dpi() {
    let Some(_turn) = server() else { return };
    let app = App::start("scale");
    let geometry = app.conn.get_geometry(app.window).unwrap().reply().unwrap();
    assert_eq!(
        (geometry.width, geometry.height),
        (600, 450),
        "400 × 300 at 1.5"
    );
    app.stop();
}

#[test]
fn keys_type_and_repeat() {
    let Some(_turn) = server() else { return };
    let app = App::start("keys");
    let a = app.keycode(u32::from(b'a'));
    app.fake(KEY_PRESS, a, 0, 0);
    let WindowEvent::Key(first) = app.wait_for(|e| matches!(e, WindowEvent::Key(k) if k.pressed))
    else {
        unreachable!()
    };
    assert_eq!(first.key, Key::Character("a".into()));
    assert_eq!(first.text.as_deref(), Some("a"));
    assert!(!first.repeat);
    // The server's repeat: pressed again without being let go.
    app.fake(KEY_PRESS, a, 0, 0);
    let WindowEvent::Key(again) = app.wait_for(|e| matches!(e, WindowEvent::Key(k) if k.pressed))
    else {
        unreachable!()
    };
    assert!(again.repeat);
    app.fake(KEY_RELEASE, a, 0, 0);
    app.wait_for(|e| matches!(e, WindowEvent::Key(k) if !k.pressed));
    app.stop();
}

#[test]
fn the_pointer_presses_where_it_is_and_the_wheel_scrolls_in_notches() {
    let Some(_turn) = server() else { return };
    let app = App::start("pointer");
    app.fake(MOTION, 0, 120, 80);
    app.fake(BUTTON_PRESS, 1, 0, 0);
    let WindowEvent::PointerButton {
        position, button, ..
    } = app.wait_for(|e| matches!(e, WindowEvent::PointerButton { pressed: true, .. }))
    else {
        unreachable!()
    };
    assert_eq!(button, PointerButton::Primary);
    assert_eq!((position.x, position.y), (120.0, 80.0));
    app.fake(BUTTON_RELEASE, 1, 0, 0);
    app.fake(BUTTON_PRESS, 5, 0, 0);
    app.fake(BUTTON_RELEASE, 5, 0, 0);
    let WindowEvent::Scroll {
        delta_y, source, ..
    } = app.wait_for(|e| matches!(e, WindowEvent::Scroll { .. }))
    else {
        unreachable!()
    };
    assert_eq!((delta_y, source), (1.0, ScrollSource::Wheel));
    app.stop();
}

#[test]
fn the_clipboard_goes_to_others_in_pieces_and_comes_from_them() {
    let Some(_turn) = server() else { return };
    let app = App::start("clipboard");
    let conn = &app.conn;
    let atom = |name: &str| {
        conn.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let (clipboard, utf8, incr, prop) = (
        atom("CLIPBOARD"),
        atom("UTF8_STRING"),
        atom("INCR"),
        atom("TEST_PROP"),
    );
    // Ours, larger than one request: it comes in pieces.
    let big: String = "あいうえお".repeat(100_000);
    app.todo
        .send(Do::SetClipboard(ClipboardKind::Clipboard, big.clone()))
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let me = conn.generate_id().unwrap();
    conn.create_window(
        0,
        me,
        app.root,
        0,
        0,
        1,
        1,
        0,
        x11rb::protocol::xproto::WindowClass::INPUT_OUTPUT,
        0,
        &x11rb::protocol::xproto::CreateWindowAux::new()
            .event_mask(x11rb::protocol::xproto::EventMask::PROPERTY_CHANGE),
    )
    .unwrap();
    conn.convert_selection(me, clipboard, utf8, prop, x11rb::CURRENT_TIME)
        .unwrap();
    conn.flush().unwrap();
    let mut got = Vec::new();
    let mut incremental = false;
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        let Some(event) = conn.poll_for_event().unwrap() else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        use x11rb::protocol::Event;
        match event {
            Event::SelectionNotify(e) => {
                assert_eq!(e.property, prop);
                let r = conn
                    .get_property(true, me, prop, AtomEnum::ANY, 0, u32::MAX / 4)
                    .unwrap()
                    .reply()
                    .unwrap();
                if r.type_ == incr {
                    incremental = true;
                } else {
                    got = r.value;
                    break;
                }
            }
            Event::PropertyNotify(e)
                if incremental
                    && e.atom == prop
                    && e.state == x11rb::protocol::xproto::Property::NEW_VALUE =>
            {
                let r = conn
                    .get_property(true, me, prop, AtomEnum::ANY, 0, u32::MAX / 4)
                    .unwrap()
                    .reply()
                    .unwrap();
                if r.value.is_empty() {
                    break;
                }
                got.extend(r.value);
            }
            _ => {}
        }
    }
    assert!(incremental, "too big for one request");
    assert_eq!(got.len(), big.len());
    assert!(got == big.as_bytes());

    // Theirs: the app asks, and we answer.
    conn.set_selection_owner(me, clipboard, x11rb::CURRENT_TIME)
        .unwrap();
    conn.flush().unwrap();
    app.todo.send(Do::Paste(ClipboardKind::Clipboard)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        let Some(event) = conn.poll_for_event().unwrap() else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        if let x11rb::protocol::Event::SelectionRequest(e) = event {
            assert_eq!(e.target, utf8);
            conn.change_property8(
                PropMode::REPLACE,
                e.requestor,
                e.property,
                utf8,
                "theirs ✓".as_bytes(),
            )
            .unwrap();
            let notify = x11rb::protocol::xproto::SelectionNotifyEvent {
                response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: e.time,
                requestor: e.requestor,
                selection: e.selection,
                target: e.target,
                property: e.property,
            };
            conn.send_event(
                false,
                e.requestor,
                x11rb::protocol::xproto::EventMask::NO_EVENT,
                notify,
            )
            .unwrap();
            conn.flush().unwrap();
            break;
        }
    }
    let WindowEvent::Paste { text, .. } = app.wait_for(|e| matches!(e, WindowEvent::Paste { .. }))
    else {
        unreachable!()
    };
    assert_eq!(text, "theirs ✓");
    app.stop();
}

#[test]
fn a_menu_is_placed_below_its_anchor_and_closes_on_a_press_outside() {
    let Some(_turn) = server() else { return };
    let app = App::start("menu");
    let s = Scale(180);
    let params = PopupParams {
        id: PopupId(7),
        parent: None,
        kind: PopupKind::Menu,
        placement: Placement::BelowStart,
        anchor: RectI::new(30, 60, 90, 30),
        gap: 4,
        wanted: SizeI::new(100, 80),
        buffer: SizeI::new(s.px(116.0), s.px(96.0)),
        logical: SizeI::new(116, 96),
        geometry: RectI::new(8, 8, 100, 80),
        position: None,
    };
    app.todo.send(Do::ShowPopup(params)).unwrap();
    let WindowEvent::PopupConfigured { id, origin, size } =
        app.wait_for(|e| matches!(e, WindowEvent::PopupConfigured { .. }))
    else {
        unreachable!()
    };
    assert_eq!(id, PopupId(7));
    // The box goes 4 logical (6 physical) below the anchor; the window starts at its shadow.
    assert_eq!(origin, PointI::new(30 - 12, 60 + 30 + 6 - 12));
    assert_eq!(size, SizeI::new(100, 80));
    std::thread::sleep(Duration::from_millis(100));
    // Far from the menu and from the window.
    app.fake(MOTION, 0, 1200, 700);
    app.fake(BUTTON_PRESS, 1, 0, 0);
    app.fake(BUTTON_RELEASE, 1, 0, 0);
    let dismissed = app.wait_for(|e| matches!(e, WindowEvent::PopupDismissed(_)));
    assert_eq!(dismissed, WindowEvent::PopupDismissed(PopupId(7)));
    app.stop();
}

#[test]
fn closing_the_window_is_asked_of_the_app() {
    let Some(_turn) = server() else { return };
    let app = App::start("close");
    let conn = &app.conn;
    let atom = |name: &str| {
        conn.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let event = x11rb::protocol::xproto::ClientMessageEvent::new(
        32,
        app.window,
        atom("WM_PROTOCOLS"),
        [atom("WM_DELETE_WINDOW"), 0, 0, 0, 0],
    );
    conn.send_event(
        false,
        app.window,
        x11rb::protocol::xproto::EventMask::NO_EVENT,
        event,
    )
    .unwrap();
    conn.flush().unwrap();
    app.wait_for(|e| *e == WindowEvent::CloseRequested);
    app.stop();
}

/// A window of the test's own on the other connection, which XDND messages go to.
fn helper_window(app: &App, x: i16, y: i16, size: u16) -> Window {
    let conn = &app.conn;
    let w = conn.generate_id().unwrap();
    conn.create_window(
        0,
        w,
        app.root,
        x,
        y,
        size,
        size,
        0,
        x11rb::protocol::xproto::WindowClass::INPUT_OUTPUT,
        0,
        &x11rb::protocol::xproto::CreateWindowAux::new(),
    )
    .unwrap();
    w
}

fn atom(conn: &RustConnection, name: &str) -> u32 {
    conn.intern_atom(false, name.as_bytes())
        .unwrap()
        .reply()
        .unwrap()
        .atom
}

/// Waits for a message of the kind on the test's connection, answering requests for
/// `XdndSelection` with `data` meanwhile.
fn wait_for_message(app: &App, kind: u32, data: &[u8]) -> [u32; 5] {
    let conn = &app.conn;
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        let Some(event) = conn.poll_for_event().unwrap() else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        match event {
            x11rb::protocol::Event::ClientMessage(e) if e.type_ == kind => {
                return e.data.as_data32();
            }
            x11rb::protocol::Event::SelectionRequest(e) => {
                conn.change_property8(PropMode::REPLACE, e.requestor, e.property, e.target, data)
                    .unwrap();
                let notify = x11rb::protocol::xproto::SelectionNotifyEvent {
                    response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: e.time,
                    requestor: e.requestor,
                    selection: e.selection,
                    target: e.target,
                    property: e.property,
                };
                conn.send_event(
                    false,
                    e.requestor,
                    x11rb::protocol::xproto::EventMask::NO_EVENT,
                    notify,
                )
                .unwrap();
                conn.flush().unwrap();
            }
            _ => {}
        }
    }
    panic!("the message didn't come");
}

fn send_message(app: &App, to: Window, kind: u32, data: [u32; 5]) {
    let event = x11rb::protocol::xproto::ClientMessageEvent::new(32, to, kind, data);
    app.conn
        .send_event(
            false,
            to,
            x11rb::protocol::xproto::EventMask::NO_EVENT,
            event,
        )
        .unwrap();
    app.conn.flush().unwrap();
}

#[test]
fn a_drop_from_another_program_is_read_and_finished() {
    let Some(_turn) = server() else { return };
    let app = App::start("drop");
    let conn = &app.conn;
    let source = helper_window(&app, 900, 600, 10);
    conn.set_selection_owner(source, atom(conn, "XdndSelection"), x11rb::CURRENT_TIME)
        .unwrap();
    let text = atom(conn, "text/plain;charset=utf-8");
    let aware = conn
        .get_property(
            false,
            app.window,
            atom(conn, "XdndAware"),
            AtomEnum::ATOM,
            0,
            1,
        )
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(aware.value32().unwrap().next(), Some(5));
    send_message(
        &app,
        app.window,
        atom(conn, "XdndEnter"),
        [source, 5 << 24, text, 0, 0],
    );
    let position = (100 << 16) | 50;
    let copy = atom(conn, "XdndActionCopy");
    send_message(
        &app,
        app.window,
        atom(conn, "XdndPosition"),
        [source, 0, position, 0, copy],
    );
    let WindowEvent::DragEntered {
        position, mimes, ..
    } = app.wait_for(|e| matches!(e, WindowEvent::DragEntered { .. }))
    else {
        unreachable!()
    };
    assert_eq!((position.x, position.y), (100.0, 50.0));
    assert_eq!(mimes, ["text/plain;charset=utf-8"]);
    let status = wait_for_message(&app, atom(conn, "XdndStatus"), b"");
    assert_eq!((status[0], status[1] & 1, status[4]), (app.window, 1, copy));
    send_message(
        &app,
        app.window,
        atom(conn, "XdndDrop"),
        [source, 0, 0, 0, 0],
    );
    let finished = wait_for_message(&app, atom(conn, "XdndFinished"), "落とした".as_bytes());
    assert_eq!((finished[1] & 1, finished[2]), (1, copy));
    let WindowEvent::DropData { bytes, .. } =
        app.wait_for(|e| matches!(e, WindowEvent::DropData { .. }))
    else {
        unreachable!()
    };
    assert_eq!(bytes, "落とした".as_bytes());
    app.stop();
}

#[test]
fn a_drag_goes_to_another_program_which_reads_it() {
    let Some(_turn) = server() else { return };
    let app = App::start("drag");
    let conn = &app.conn;
    let target = helper_window(&app, 800, 100, 200);
    conn.change_property32(
        PropMode::REPLACE,
        target,
        atom(conn, "XdndAware"),
        AtomEnum::ATOM,
        &[5],
    )
    .unwrap();
    conn.map_window(target).unwrap();
    conn.sync().unwrap();
    app.fake(MOTION, 0, 100, 100);
    app.fake(BUTTON_PRESS, 1, 0, 0);
    let offer = DragOffer {
        items: vec![(
            "text/plain;charset=utf-8".into(),
            b"dragged".to_vec().into(),
        )],
        actions: DragActions::Both,
    };
    app.todo.send(Do::StartDrag(offer)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    app.fake(MOTION, 0, 850, 150);
    let enter = wait_for_message(&app, atom(conn, "XdndEnter"), b"");
    assert_eq!(enter[1] >> 24, 5);
    assert_eq!(enter[2], atom(conn, "text/plain;charset=utf-8"));
    let position = wait_for_message(&app, atom(conn, "XdndPosition"), b"");
    assert_eq!(position[2], (850 << 16) | 150);
    let copy = atom(conn, "XdndActionCopy");
    send_message(
        &app,
        position[0],
        atom(conn, "XdndStatus"),
        [target, 1, 0, 0, copy],
    );
    std::thread::sleep(Duration::from_millis(50));
    app.fake(BUTTON_RELEASE, 1, 0, 0);
    let drop = wait_for_message(&app, atom(conn, "XdndDrop"), b"");
    let selection = atom(conn, "XdndSelection");
    let prop = atom(conn, "TEST_DROP");
    conn.convert_selection(
        target,
        selection,
        atom(conn, "text/plain;charset=utf-8"),
        prop,
        drop[2],
    )
    .unwrap();
    conn.flush().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let mut got = None;
    while got.is_none() && Instant::now() < until {
        match conn.poll_for_event().unwrap() {
            Some(x11rb::protocol::Event::SelectionNotify(e)) => {
                let r = conn
                    .get_property(true, target, e.property, AtomEnum::ANY, 0, 1024)
                    .unwrap()
                    .reply()
                    .unwrap();
                got = Some(r.value);
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert_eq!(got.as_deref(), Some(&b"dragged"[..]));
    send_message(
        &app,
        drop[0],
        atom(conn, "XdndFinished"),
        [target, 1, copy, 0, 0],
    );
    let ended = app.wait_for(|e| matches!(e, WindowEvent::DragSourceEnded(_)));
    assert_eq!(ended, WindowEvent::DragSourceEnded(DragResult::Copied));
    app.stop();
}

#[test]
fn a_window_opened_above_another_says_so_and_can_draw_its_own_frame() {
    let Some(_turn) = server() else { return };
    let parent = App::start("parent");
    let child = App::start_with("child", |o| {
        o.parent = Some(ParentLink::from_window(parent.window));
        o.modal = true;
        o.decorations = DecorationMode::Client;
    });
    let read = |name: &str, kind: u32| {
        let prop = atom(&child.conn, name);
        child
            .conn
            .get_property(false, child.window, prop, kind, 0, 16)
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .map(|v| v.collect::<Vec<u32>>())
            .unwrap_or_default()
    };
    assert_eq!(
        read("WM_TRANSIENT_FOR", u32::from(AtomEnum::WINDOW)),
        [parent.window]
    );
    let motif = atom(&child.conn, "_MOTIF_WM_HINTS");
    assert_eq!(read("_MOTIF_WM_HINTS", motif), [2, 0, 0, 0, 0], "no frame");
    let modal = atom(&child.conn, "_NET_WM_STATE_MODAL");
    assert!(read("_NET_WM_STATE", u32::from(AtomEnum::ATOM)).contains(&modal));
    let dialog = atom(&child.conn, "_NET_WM_WINDOW_TYPE_DIALOG");
    assert_eq!(
        read("_NET_WM_WINDOW_TYPE", u32::from(AtomEnum::ATOM)),
        [dialog]
    );
    child.wait_for(|e| *e == WindowEvent::Decorations(Decorations::Client));
    child.stop();
    parent.stop();
}
