//! Going on all of the screen against a small compositor that keeps to the protocol and writes down
//! what it is asked. It shows the requests and what comes of a `configure`, not how a real compositor
//! behaves. See `docs/design/27-fullscreen.md`.

use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_wayland::{Ctx, WaylandHandler, WindowOptions, run};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wayland_protocols::xdg::shell::server::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::protocol::{wl_callback, wl_compositor, wl_seat, wl_surface};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    backend::ClientData,
};

#[derive(Default)]
struct Compositor {
    surface: Option<xdg_surface::XdgSurface>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    /// What the window asked to be, in order.
    asked: Vec<bool>,
}

struct NoData;
impl ClientData for NoData {}

impl GlobalDispatch<wl_compositor::WlCompositor, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<wl_compositor::WlCompositor>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl GlobalDispatch<xdg_wm_base::XdgWmBase, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<xdg_wm_base::XdgWmBase>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl GlobalDispatch<wl_seat::WlSeat, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<wl_seat::WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_seat::WlSeat,
        _: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
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
        if let wl_compositor::Request::CreateSurface { id } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_surface::WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_surface::Request::Frame { callback } = request {
            init.init(callback, ());
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_callback::WlCallback,
        _: wl_callback::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &xdg_wm_base::XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_wm_base::Request::GetXdgSurface { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        surface: &xdg_surface::XdgSurface,
        request: xdg_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetToplevel { id } = request {
            state.surface = Some(surface.clone());
            state.toplevel = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &xdg_toplevel::XdgToplevel,
        request: xdg_toplevel::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_toplevel::Request::SetFullscreen { output: None } => state.asked.push(true),
            xdg_toplevel::Request::UnsetFullscreen => state.asked.push(false),
            _ => {}
        }
    }
}

struct Window {
    asks: mpsc::Receiver<bool>,
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
        while let Ok(on) = self.asks.try_recv() {
            ctx.set_fullscreen(on);
        }
    }
}

struct World {
    display: Display<Compositor>,
    socket: ListeningSocket,
    state: Compositor,
    client: Option<Client>,
    asks: mpsc::Sender<bool>,
    events: mpsc::Receiver<WindowEvent>,
    serial: u32,
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

    fn configure(&mut self, width: i32, height: i32, fullscreen: bool) {
        let mut states = Vec::new();
        if fullscreen {
            states.extend((xdg_toplevel::State::Fullscreen as u32).to_ne_bytes());
        }
        states.extend((xdg_toplevel::State::Activated as u32).to_ne_bytes());
        self.serial += 1;
        let serial = self.serial;
        self.state
            .toplevel
            .as_ref()
            .unwrap()
            .configure(width, height, states);
        self.state.surface.as_ref().unwrap().configure(serial);
        self.pump();
    }

    /// All that the window says from now until it has said something and then nothing for a while.
    /// What the window was told, but for who draws its frame: without xdg-decoration it is the
    /// window itself, which it hears once.
    fn events(&mut self) -> Vec<WindowEvent> {
        let told = |e: &WindowEvent| !matches!(e, WindowEvent::Decorations(_));
        let mut got = Vec::new();
        self.until("an event", |w| {
            got.extend(w.events.try_iter().filter(told));
            !got.is_empty()
        });
        let end = Instant::now() + Duration::from_millis(50);
        while Instant::now() < end {
            self.pump();
            got.extend(self.events.try_iter().filter(told));
            std::thread::sleep(Duration::from_millis(1));
        }
        got
    }
}

fn setup(fullscreen: bool) -> World {
    let dir = std::env::temp_dir().join(format!("anti-webui-fullscreen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("wayland-test-{fullscreen}"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("lock"));
    let socket = ListeningSocket::bind_absolute(path.clone()).unwrap();
    let display = Display::<Compositor>::new().unwrap();
    let dh = display.handle();
    dh.create_global::<Compositor, wl_compositor::WlCompositor, ()>(6, ());
    dh.create_global::<Compositor, xdg_wm_base::XdgWmBase, ()>(6, ());
    dh.create_global::<Compositor, wl_seat::WlSeat, ()>(8, ());
    // SAFETY: only one test here, and the window before this one has found its compositor.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &path);
        std::env::remove_var("WAYLAND_SOCKET");
    }
    let (asks, ask_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    std::thread::spawn(move || {
        let window = Window {
            asks: ask_rx,
            events: event_tx,
        };
        let options = WindowOptions {
            title: "fullscreen".into(),
            app_id: "fullscreen".into(),
            width: 300,
            height: 200,
            fullscreen,
            ..Default::default()
        };
        // It ends with an error when the compositor of this test goes away.
        let _ = run(options, window);
    });
    let mut world = World {
        display,
        socket,
        state: Compositor::default(),
        client: None,
        asks,
        events,
        serial: 0,
    };
    world.until("the window", |w| w.state.toplevel.is_some());
    world
}

fn resized(width: i32, height: i32) -> WindowEvent {
    WindowEvent::Resized {
        size: SizeI::new(width, height),
        scale: Scale(120),
    }
}

/// One test, as the window finds the compositor through the environment.
#[test]
fn going_on_all_of_the_screen_goes_by_the_protocol() {
    it_can_start_there();
    the_window_goes_there_and_comes_back();
}

fn it_can_start_there() {
    let mut w = setup(true);
    assert_eq!(w.state.asked, [true]);
    w.configure(1000, 800, true);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(true), resized(1000, 800)]
    );
    // What it asked to be at first is what it goes back to.
    w.configure(0, 0, false);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(false), resized(300, 200)]
    );
}

fn the_window_goes_there_and_comes_back() {
    let mut w = setup(false);
    assert!(w.state.asked.is_empty());

    // The compositor leaves the size up to the window at first.
    w.configure(0, 0, false);
    assert_eq!(w.events(), [resized(300, 200)]);

    w.asks.send(true).unwrap();
    w.until("set_fullscreen", |w| w.state.asked == [true]);
    // It says so together with the size, and the state counts with the `xdg_surface.configure`.
    w.configure(1000, 800, true);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(true), resized(1000, 800)]
    );

    w.asks.send(false).unwrap();
    w.until("unset_fullscreen", |w| w.state.asked == [true, false]);
    // Without a size to go by, it goes back to the one it had.
    w.configure(0, 0, false);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(false), resized(300, 200)]
    );

    // What the compositor does by itself says the same, and a size it gives is the one to take.
    w.configure(1000, 800, true);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(true), resized(1000, 800)]
    );
    w.configure(500, 400, false);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(false), resized(500, 400)]
    );

    // That is what it goes back to the next time.
    w.configure(1000, 800, true);
    w.events();
    w.configure(0, 0, false);
    assert_eq!(
        w.events(),
        [WindowEvent::Fullscreen(false), resized(500, 400)]
    );
}
