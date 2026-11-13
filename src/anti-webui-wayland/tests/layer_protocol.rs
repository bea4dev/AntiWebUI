//! Layer surfaces against a small compositor that keeps to the protocol and writes down what it is
//! asked. It shows the requests and what comes of a `configure`, not how a real compositor places
//! them. See `docs/design/37-wayland-shell.md`.

#![cfg(target_os = "linux")]

use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::layer::{
    Anchor, Exclusive, InputRegion, Keyboard, Layer, LayerChange, LayerSize, LayerSpec, Margin,
};
use anti_webui_wayland::{Ctx, WaylandHandler, WindowOptions, run};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wayland_protocols::xdg::shell::server::xdg_wm_base;
use wayland_protocols_wlr::layer_shell::v1::server::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_server::protocol::{
    wl_callback, wl_compositor, wl_output, wl_region, wl_seat, wl_surface,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource, backend::ClientData,
};

#[derive(Default)]
struct Compositor {
    layer: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    /// What was asked, in order, as text.
    asked: Vec<String>,
    size: (u32, u32),
    anchor: u32,
    edge: u32,
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
plain_global!(zwlr_layer_shell_v1::ZwlrLayerShellV1);

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
        _: &wl_surface::WlSurface,
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
                // Zero-sized axes must stretch between opposing anchors at every commit,
                // not just after the last command in a geometry change.
                assert!(state.size.0 != 0 || state.anchor & 12 == 12);
                assert!(state.size.1 != 0 || state.anchor & 3 == 3);
                // The exclusive edge must be one of the anchors.
                assert_eq!(state.anchor & state.edge, state.edge);
                state.asked.push("commit".into());
            }
            wl_surface::Request::SetInputRegion { region } => state
                .asked
                .push(format!("input region {}", region.is_some())),
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface {
            id,
            output,
            layer,
            namespace,
            ..
        } = request
        {
            let output = output
                .and_then(|o| o.data::<&'static str>().copied())
                .unwrap_or("none");
            state.asked.push(format!(
                "layer surface {output} {:?} {namespace}",
                layer.into_result().unwrap()
            ));
            state.layer = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use zwlr_layer_surface_v1::Request as R;
        let said = match request {
            R::SetSize { width, height } => {
                state.size = (width, height);
                format!("size {width} {height}")
            }
            R::SetAnchor { anchor } => {
                state.anchor = anchor.into_result().unwrap().bits();
                format!("anchor {}", state.anchor)
            }
            R::SetExclusiveZone { zone } => format!("zone {zone}"),
            R::SetExclusiveEdge { edge } => {
                state.edge = edge.into_result().unwrap().bits();
                format!("edge {}", state.edge)
            }
            R::SetMargin {
                top,
                right,
                bottom,
                left,
            } => format!("margin {top} {right} {bottom} {left}"),
            R::SetKeyboardInteractivity {
                keyboard_interactivity,
            } => format!(
                "keyboard {:?}",
                keyboard_interactivity.into_result().unwrap()
            ),
            R::SetLayer { layer } => format!("layer {:?}", layer.into_result().unwrap()),
            R::AckConfigure { serial } => format!("ack {serial}"),
            _ => return,
        };
        state.asked.push(said);
    }
}

struct Window {
    asks: mpsc::Receiver<Ask>,
    events: mpsc::Sender<WindowEvent>,
}

enum Ask {
    Change(LayerChange),
    Content(i32, i32),
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
        while let Ok(ask) = self.asks.try_recv() {
            match ask {
                Ask::Change(c) => ctx.change_layer(c),
                Ask::Content(w, h) => ctx.set_layer_content_size(SizeI::new(w, h)),
            }
        }
    }
}

struct World {
    display: Display<Compositor>,
    socket: ListeningSocket,
    state: Compositor,
    client: Option<Client>,
    asks: mpsc::Sender<Ask>,
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

    /// What was asked since the last time.
    fn asked(&mut self) -> Vec<String> {
        std::mem::take(&mut self.state.asked)
    }

    fn until_asked(&mut self, what: &str) -> Vec<String> {
        let mut got = Vec::new();
        self.until(what, |w| {
            got.extend(w.asked());
            got.iter().any(|a| a == what)
        });
        got
    }
}

fn setup(options: WindowOptions) -> World {
    let dir = std::env::temp_dir().join(format!("anti-webui-layer-{}", std::process::id()));
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
    dh.create_global::<Compositor, zwlr_layer_shell_v1::ZwlrLayerShellV1, ()>(5, ());
    dh.create_global::<Compositor, wl_output::WlOutput, &'static str>(4, "DP-1");
    dh.create_global::<Compositor, wl_output::WlOutput, &'static str>(4, "DP-2");
    // SAFETY: only one test here, before the window looks for its compositor.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &path);
        std::env::remove_var("WAYLAND_SOCKET");
    }
    let (asks, ask_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let (exit_tx, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let window = Window {
            asks: ask_rx,
            events: event_tx,
        };
        let _ = run(options, window);
        let _ = exit_tx.send(());
    });
    let mut world = World {
        display,
        socket,
        state: Compositor::default(),
        client: None,
        asks,
        events,
        exited,
    };
    world.until("the layer surface", |w| w.state.layer.is_some());
    world
}

#[test]
fn a_layer_surface_goes_by_the_protocol() {
    let options = WindowOptions {
        width: 300,
        height: 32,
        layer: Some(LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            width: LayerSize::Stretch,
            height: LayerSize::Content,
            exclusive: Exclusive::Auto,
            margin: Margin {
                top: 4,
                ..Default::default()
            },
            keyboard: Keyboard::OnDemand,
            input_region: InputRegion::Rects(vec![anti_webui_core::geometry::RectI::new(
                0, 0, 100, 32,
            )]),
            output: Some("DP-2".into()),
            namespace: "bar".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut w = setup(options);
    // Everything is asked for before the first commit, on the output named.
    let first = w.until_asked("commit");
    assert_eq!(
        first,
        [
            "layer surface DP-2 Top bar",
            "size 0 32",
            "anchor 13",
            "margin 4 0 0 0",
            "keyboard OnDemand",
            "zone 32",
            "region 0 0 100 32",
            "input region true",
            "commit",
        ]
    );

    // The compositor stretches it; it takes that size, and the height it asked for.
    w.state.layer.as_ref().unwrap().configure(1, 1920, 0);
    w.pump();
    let mut events = Vec::new();
    w.until("resized", |w| {
        events.extend(w.events.try_iter());
        !events.is_empty()
    });
    assert_eq!(
        events,
        [WindowEvent::Resized {
            size: SizeI::new(1920, 32),
            scale: Scale(120)
        }]
    );
    assert_eq!(w.until_asked("ack 1"), ["ack 1"]);

    // What is in it grows: the new height goes with a commit, and so does the zone it keeps.
    w.asks.send(Ask::Content(1920, 48)).unwrap();
    assert_eq!(w.until_asked("commit"), ["size 0 48", "commit"]);
    w.state.layer.as_ref().unwrap().configure(2, 1920, 48);
    w.pump();
    assert_eq!(w.until_asked("zone 48"), ["ack 2", "zone 48"]);

    // Changes after it opened.
    w.asks
        .send(Ask::Change(LayerChange::Keyboard(Keyboard::Exclusive)))
        .unwrap();
    assert_eq!(w.until_asked("commit"), ["keyboard Exclusive", "commit"]);
    w.asks
        .send(Ask::Change(LayerChange::Layer(Layer::Overlay)))
        .unwrap();
    assert_eq!(w.until_asked("commit"), ["layer Overlay", "commit"]);
    w.asks
        .send(Ask::Change(LayerChange::Exclusive(Exclusive::None)))
        .unwrap();
    assert_eq!(w.until_asked("commit"), ["zone 0", "commit"]);

    // Closed by the compositor, as when its output goes: the window ends.
    w.state.layer.as_ref().unwrap().closed();
    w.pump();
    w.until("the window to end", |w| w.exited.try_recv().is_ok());

    // Keep scenarios in this single test: the client reads process-wide Wayland variables.
    drop(w);
    fixed_bars_reserve_their_thickness_before_configure();
    rotating_a_bar_commits_only_the_target_geometry();
    what_the_protocol_would_not_take_is_put_right();
}

fn fixed_bars_reserve_their_thickness_before_configure() {
    for edge in [Anchor::LEFT, Anchor::RIGHT, Anchor::TOP, Anchor::BOTTOM] {
        let vertical = edge == Anchor::LEFT || edge == Anchor::RIGHT;
        let mut w = setup(WindowOptions {
            layer: Some(LayerSpec {
                anchor: edge
                    | if vertical {
                        Anchor::TOP | Anchor::BOTTOM
                    } else {
                        Anchor::LEFT | Anchor::RIGHT
                    },
                width: if vertical {
                    LayerSize::Fixed(52)
                } else {
                    LayerSize::Stretch
                },
                height: if vertical {
                    LayerSize::Stretch
                } else {
                    LayerSize::Fixed(52)
                },
                exclusive: Exclusive::Auto,
                ..Default::default()
            }),
            ..Default::default()
        });
        let requests = w.until_asked("commit");
        let zones: Vec<_> = requests.iter().filter(|r| r.starts_with("zone ")).collect();
        assert_eq!(zones, ["zone 52"], "startup requests: {requests:?}");
        assert_eq!(requests.last().unwrap(), "commit");
        w.state.layer.as_ref().unwrap().closed();
        w.pump();
        w.until("the window to end", |w| w.exited.try_recv().is_ok());
    }
}

fn rotating_a_bar_commits_only_the_target_geometry() {
    let mut w = setup(WindowOptions {
        layer: Some(LayerSpec {
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            width: LayerSize::Stretch,
            height: LayerSize::Fixed(52),
            exclusive: Exclusive::Auto,
            ..Default::default()
        }),
        ..Default::default()
    });
    w.until_asked("commit");
    w.state.layer.as_ref().unwrap().configure(1, 2194, 52);
    assert_eq!(w.until_asked("ack 1"), ["ack 1"]);

    // No intermediate commit may reserve the previous orientation's long dimension.
    for (serial, anchor, width, height, size, zone, configured) in [
        (
            2,
            Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM,
            LayerSize::Fixed(52),
            LayerSize::Stretch,
            "size 52 0",
            None,
            (52, 1080),
        ),
        (
            3,
            Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            LayerSize::Stretch,
            LayerSize::Fixed(36),
            "size 0 36",
            Some("zone 36"),
            (2194, 36),
        ),
        (
            4,
            Anchor::RIGHT | Anchor::TOP | Anchor::BOTTOM,
            LayerSize::Fixed(44),
            LayerSize::Stretch,
            "size 44 0",
            Some("zone 44"),
            (44, 1080),
        ),
        (
            5,
            Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            LayerSize::Stretch,
            LayerSize::Fixed(56),
            "size 0 56",
            Some("zone 56"),
            (2194, 56),
        ),
    ] {
        w.asks
            .send(Ask::Change(LayerChange::Geometry {
                anchor,
                width,
                height,
            }))
            .unwrap();
        let mut expected = vec![size.to_string(), format!("anchor {}", anchor.bits())];
        if let Some(zone) = zone {
            expected.push(zone.into());
        }
        expected.push("commit".into());
        assert_eq!(w.until_asked("commit"), expected);
        w.state
            .layer
            .as_ref()
            .unwrap()
            .configure(serial, configured.0, configured.1);
        let ack = format!("ack {serial}");
        assert_eq!(w.until_asked(&ack), [ack]);
    }

    // A size-only change must also use the new thickness before the next configure.
    w.asks
        .send(Ask::Change(LayerChange::Size(
            LayerSize::Stretch,
            LayerSize::Fixed(64),
        )))
        .unwrap();
    assert_eq!(w.until_asked("commit"), ["size 0 64", "zone 64", "commit"]);
    // An older configure cannot put the old thickness back.
    w.state.layer.as_ref().unwrap().configure(6, 2194, 56);
    assert_eq!(w.until_asked("ack 6"), ["ack 6"]);

    for (exclusive, zone) in [
        (Exclusive::Fixed(77), "zone 77"),
        (Exclusive::None, "zone 0"),
        (Exclusive::Ignore, "zone -1"),
        (Exclusive::Auto, "zone 64"),
    ] {
        w.asks
            .send(Ask::Change(LayerChange::Exclusive(exclusive)))
            .unwrap();
        assert_eq!(w.until_asked("commit"), [zone, "commit"]);
    }
    w.state.layer.as_ref().unwrap().closed();
    w.pump();
    w.until("the window to end", |w| w.exited.try_recv().is_ok());
}

fn what_the_protocol_would_not_take_is_put_right() {
    let mut w = setup(WindowOptions {
        width: 300,
        layer: Some(LayerSpec {
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            width: LayerSize::Stretch,
            height: LayerSize::Fixed(32),
            exclusive: Exclusive::Auto,
            exclusive_edge: Some(Anchor::TOP),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert!(w.until_asked("commit").contains(&"edge 1".to_string()));
    w.state.layer.as_ref().unwrap().configure(1, 2194, 32);
    assert_eq!(w.until_asked("ack 1"), ["ack 1"]);

    // The top is no longer an edge to keep clear at the bottom, and is again back on top.
    w.asks
        .send(Ask::Change(LayerChange::Anchor(
            Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        )))
        .unwrap();
    assert_eq!(w.until_asked("commit"), ["anchor 14", "edge 0", "commit"]);
    let top = Anchor::TOP | Anchor::LEFT | Anchor::RIGHT;
    w.asks.send(Ask::Change(LayerChange::Anchor(top))).unwrap();
    assert_eq!(w.until_asked("commit"), ["anchor 13", "edge 1", "commit"]);

    // Turned to the left the old way, a call at a time: the width can't stretch half way. Every
    // commit passes the checks above.
    w.asks
        .send(Ask::Change(LayerChange::Anchor(
            Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM,
        )))
        .unwrap();
    assert_eq!(
        w.until_asked("commit"),
        ["size 300 32", "anchor 7", "commit"]
    );
    w.asks
        .send(Ask::Change(LayerChange::Size(
            LayerSize::Fixed(52),
            LayerSize::Stretch,
        )))
        .unwrap();
    assert_eq!(w.until_asked("commit"), ["size 52 0", "commit"]);

    // Back on top in one go, it stretches again.
    w.asks
        .send(Ask::Change(LayerChange::Geometry {
            anchor: top,
            width: LayerSize::Stretch,
            height: LayerSize::Fixed(32),
        }))
        .unwrap();
    assert_eq!(
        w.until_asked("commit"),
        ["size 0 32", "anchor 13", "commit"]
    );
    w.state.layer.as_ref().unwrap().closed();
    w.pump();
    w.until("the window to end", |w| w.exited.try_recv().is_ok());
}
