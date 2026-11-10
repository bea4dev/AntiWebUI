//! Lock surfaces against a small compositor that keeps to the protocol and writes down what it is
//! asked: the lock, the surface on the output named, `configure`, `locked`, and unlocking when the
//! surface closes. See "画面ロック" in `docs/design/37-wayland-shell.md`.

#![cfg(target_os = "linux")]

use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_wayland::{Ctx, LockState, WaylandHandler, WindowOptions, lock_state, run};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wayland_protocols::ext::session_lock::v1::server::{
    ext_session_lock_manager_v1 as manager, ext_session_lock_surface_v1 as lock_surface,
    ext_session_lock_v1 as lock,
};
use wayland_protocols::xdg::shell::server::xdg_wm_base;
use wayland_server::protocol::{
    wl_callback, wl_compositor, wl_output, wl_region, wl_seat, wl_surface,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource, backend::ClientData,
};

#[derive(Default)]
struct Compositor {
    lock: Option<lock::ExtSessionLockV1>,
    surface: Option<lock_surface::ExtSessionLockSurfaceV1>,
    /// The `wl_surface` under it, and whether a configure has been acked.
    lock_wl_surface: Option<wl_surface::WlSurface>,
    acked: bool,
    /// What was asked, in order, as text.
    asked: Vec<String>,
}

struct NoData;
impl ClientData for NoData {}

macro_rules! plain_global {
    ($iface:ty) => {
        impl GlobalDispatch<$iface, ()> for Compositor {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$iface>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                init.init(resource, ());
            }
        }
    };
}

plain_global!(wl_compositor::WlCompositor);
plain_global!(xdg_wm_base::XdgWmBase);
plain_global!(wl_seat::WlSeat);
plain_global!(manager::ExtSessionLockManagerV1);

/// Outputs with their names.
impl GlobalDispatch<wl_output::WlOutput, &'static str> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<wl_output::WlOutput>,
        name: &&'static str,
        init: &mut DataInit<'_, Self>,
    ) {
        let output = init.init(resource, name);
        if output.version() >= 4 {
            output.name(name.to_string());
        }
        output.done();
    }
}

macro_rules! ignore {
    ($iface:ty, $data:ty) => {
        impl Dispatch<$iface, $data> for Compositor {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$iface,
                _: <$iface as Resource>::Request,
                _: &$data,
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    };
}

ignore!(wl_seat::WlSeat, ());
ignore!(wl_callback::WlCallback, ());
ignore!(xdg_wm_base::XdgWmBase, ());
ignore!(wl_output::WlOutput, &'static str);

impl Dispatch<wl_region::WlRegion, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_region::WlRegion,
        request: wl_region::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_region::Request::Add {
            x,
            y,
            width,
            height,
        } = request
        {
            state.asked.push(format!("region {x} {y} {width} {height}"));
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                init.init(id, ());
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        surface: &wl_surface::WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_surface::Request::Frame { callback } => {
                init.init(callback, ());
            }
            wl_surface::Request::Commit => {
                // As smithay does, which is what ShojiWM and niri are built on.
                if state.lock_wl_surface.as_ref() == Some(surface) && !state.acked {
                    state.asked.push("commit before ack".into());
                    if let Some(lock_surface) = &state.surface {
                        lock_surface.post_error(
                            lock_surface::Error::CommitBeforeFirstAck,
                            "Committed before the first ack_configure.",
                        );
                    }
                    return;
                }
                state.asked.push("commit".into())
            }
            wl_surface::Request::SetInputRegion { region } => state
                .asked
                .push(format!("input region {}", region.is_some())),
            _ => {}
        }
    }
}

impl Dispatch<manager::ExtSessionLockManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &manager::ExtSessionLockManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let manager::Request::Lock { id } = request {
            state.asked.push("lock".into());
            state.lock = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<lock::ExtSessionLockV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &lock::ExtSessionLockV1,
        request: lock::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            lock::Request::GetLockSurface {
                id,
                surface,
                output,
            } => {
                let output = output.data::<&'static str>().copied().unwrap_or("none");
                state.asked.push(format!("lock surface {output}"));
                state.lock_wl_surface = Some(surface);
                state.surface = Some(init.init(id, ()));
            }
            lock::Request::UnlockAndDestroy => state.asked.push("unlock".into()),
            lock::Request::Destroy => state.asked.push("destroy lock".into()),
            _ => {}
        }
    }
}

impl Dispatch<lock_surface::ExtSessionLockSurfaceV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &lock_surface::ExtSessionLockSurfaceV1,
        request: lock_surface::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            lock_surface::Request::AckConfigure { serial } => {
                state.acked = true;
                state.asked.push(format!("ack {serial}"))
            }
            lock_surface::Request::Destroy => state.asked.push("destroy surface".into()),
            _ => {}
        }
    }
}

struct Window {
    close: mpsc::Receiver<()>,
    events: mpsc::Sender<WindowEvent>,
}

impl WaylandHandler for Window {
    fn event(&mut self, _: &mut Ctx<'_>, event: WindowEvent) {
        let _ = self.events.send(event);
    }
    fn frame(&mut self, _: &mut Ctx<'_>) {}
    fn deadline(&mut self, now: Instant) -> Option<Instant> {
        Some(now + Duration::from_millis(1))
    }
    fn timer(&mut self, ctx: &mut Ctx<'_>, _: Instant) {
        if self.close.try_recv().is_ok() {
            ctx.exit();
        }
    }
}

struct World {
    display: Display<Compositor>,
    socket: ListeningSocket,
    state: Compositor,
    client: Option<Client>,
    close: mpsc::Sender<()>,
    events: mpsc::Receiver<WindowEvent>,
    exited: mpsc::Receiver<()>,
}

impl World {
    fn pump(&mut self) {
        if let Ok(Some(stream)) = self.socket.accept() {
            let client = self
                .display
                .handle()
                .insert_client(stream, Arc::new(NoData))
                .unwrap();
            self.client = Some(client);
        }
        self.display.dispatch_clients(&mut self.state).unwrap();
        self.display.flush_clients().unwrap();
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut World) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            self.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn until_asked(&mut self, what: &str) -> Vec<String> {
        let mut got = Vec::new();
        self.until(what, |w| {
            got.extend(std::mem::take(&mut w.state.asked));
            got.iter().any(|a| a == what)
        });
        got
    }
}

#[test]
fn a_lock_surface_goes_by_the_protocol() {
    let dir = std::env::temp_dir().join(format!("anti-webui-lock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wayland-test");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("lock"));
    let socket = ListeningSocket::bind_absolute(path.clone()).unwrap();
    let display = Display::<Compositor>::new().unwrap();
    let dh = display.handle();
    dh.create_global::<Compositor, wl_compositor::WlCompositor, ()>(6, ());
    dh.create_global::<Compositor, xdg_wm_base::XdgWmBase, ()>(6, ());
    dh.create_global::<Compositor, wl_seat::WlSeat, ()>(8, ());
    dh.create_global::<Compositor, manager::ExtSessionLockManagerV1, ()>(1, ());
    dh.create_global::<Compositor, wl_output::WlOutput, &'static str>(4, "DP-1");
    dh.create_global::<Compositor, wl_output::WlOutput, &'static str>(4, "DP-2");
    // SAFETY: only one test here, before the window looks for its compositor.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &path);
        std::env::remove_var("WAYLAND_SOCKET");
    }
    let (close, close_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let (exit_tx, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let window = Window {
            close: close_rx,
            events: event_tx,
        };
        let options = WindowOptions {
            lock: Some("DP-2".into()),
            ..Default::default()
        };
        let _ = run(options, window);
        let _ = exit_tx.send(());
    });
    let mut w = World {
        display,
        socket,
        state: Compositor::default(),
        client: None,
        close,
        events,
        exited,
    };

    // It locks and asks for a surface on the output named, and doesn't commit until it has acked
    // a configure.
    let mut asked = w.until_asked("lock surface DP-2");
    assert_eq!(asked, ["lock", "lock surface DP-2"]);
    assert_eq!(lock_state(), LockState::Locking);
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        w.pump();
        asked.extend(std::mem::take(&mut w.state.asked));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!asked.iter().any(|a| a.starts_with("commit")), "{asked:?}");

    // The compositor gives it the size of the output.
    w.state.surface.as_ref().unwrap().configure(1, 1920, 1080);
    w.pump();
    let mut events = Vec::new();
    w.until("resized", |w| {
        events.extend(w.events.try_iter());
        !events.is_empty()
    });
    assert_eq!(
        events,
        [WindowEvent::Resized {
            size: SizeI::new(1920, 1080),
            scale: Scale(120)
        }]
    );
    let asked = w.until_asked("ack 1");
    assert!(!asked.iter().any(|a| a.starts_with("commit")), "{asked:?}");

    // Locked.
    w.state.lock.as_ref().unwrap().locked();
    w.pump();
    w.until("locked", |_| lock_state() == LockState::Locked);

    // Closing the last lock surface unlocks.
    w.close.send(()).unwrap();
    let asked = w.until_asked("unlock");
    assert!(asked.contains(&"destroy surface".to_owned()), "{asked:?}");
    w.until("the window to end", |w| w.exited.try_recv().is_ok());
    assert_eq!(lock_state(), LockState::Unlocked);
}
