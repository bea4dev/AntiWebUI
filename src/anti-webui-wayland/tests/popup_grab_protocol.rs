//! Popup grabs of several windows on one connection, against a small compositor that keeps the
//! rules of xdg-shell the way smithay does: a client has one grab chain, a new grabbing popup must
//! go on its topmost grabbing popup (or on a window when there is none), a popup can't be destroyed
//! while popups are attached to it, and once a popup is destroyed its surface takes no more acks or
//! commits. It ends the connection when a rule is broken.

#![cfg(target_os = "linux")]

use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, SizeI};
use anti_webui_core::platform::{Placement, PopupId, PopupKind, PopupParams};
use anti_webui_wayland::{
    Ctx, Handlers, Session, SurfaceId, WaylandHandler, WindowKey, WindowOptions,
};
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wayland_protocols::xdg::shell::server::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_server::backend::ObjectId;
use wayland_server::protocol::{wl_callback, wl_compositor, wl_region, wl_seat, wl_surface};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource, backend::ClientData,
};

struct ServerPopup {
    /// Its number, in the order the popups were made.
    n: usize,
    popup: xdg_popup::XdgPopup,
    surface: ObjectId,
    /// The xdg surface it is on.
    parent: Option<ObjectId>,
}

#[derive(Default)]
struct Compositor {
    /// How many popups were made.
    made: usize,
    popups: Vec<ServerPopup>,
    /// The grab chain: grabbing popups, the topmost last.
    grabs: Vec<ObjectId>,
    /// What was asked about popups, in order.
    asked: Vec<String>,
    errors: Vec<String>,
    /// The wl_surface of each xdg_surface.
    surfaces: HashMap<ObjectId, ObjectId>,
    /// The xdg_surfaces and wl_surfaces whose popup was destroyed.
    dead: Vec<ObjectId>,
    serial: u32,
}

impl Compositor {
    fn find(&self, popup: &xdg_popup::XdgPopup) -> Option<&ServerPopup> {
        self.popups.iter().find(|p| p.popup.id() == popup.id())
    }

    fn number_of_surface(&self, surface: &ObjectId) -> usize {
        self.popups
            .iter()
            .find(|p| p.surface == *surface)
            .map_or(0, |p| p.n)
    }

    fn fail(&mut self, on: &impl Resource, what: String) {
        on.post_error(xdg_wm_base::Error::NotTheTopmostPopup as u32, what.clone());
        self.errors.push(what);
    }
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

impl GlobalDispatch<wl_seat::WlSeat, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<wl_seat::WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(wl_seat::Capability::empty());
    }
}

macro_rules! ignore {
    ($iface:ty) => {
        impl Dispatch<$iface, ()> for Compositor {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$iface,
                _: <$iface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    };
}

ignore!(wl_seat::WlSeat);
ignore!(wl_callback::WlCallback);
ignore!(wl_region::WlRegion);
ignore!(xdg_toplevel::XdgToplevel);
ignore!(xdg_positioner::XdgPositioner);

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
            wl_surface::Request::Commit if state.dead.contains(&surface.id()) => {
                let what = "a commit on the surface of a destroyed popup".to_string();
                state.fail(surface, what);
            }
            wl_surface::Request::Destroy => state.dead.retain(|d| *d != surface.id()),
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &xdg_wm_base::XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                let xdg = init.init(id, ());
                state.surfaces.insert(xdg.id(), surface.id());
            }
            xdg_wm_base::Request::CreatePositioner { id } => {
                init.init(id, ());
            }
            _ => {}
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
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                init.init(id, ());
            }
            xdg_surface::Request::GetPopup { id, parent, .. } => {
                state.made += 1;
                let n = state.made;
                let parent = parent.map(|p| p.id());
                let on = parent.as_ref().map_or(0, |p| state.number_of_surface(p));
                state.asked.push(format!("popup {n} on {on}"));
                let popup = init.init(id, ());
                popup.configure(0, 0, 50, 50);
                state.serial += 1;
                surface.configure(state.serial);
                state.popups.push(ServerPopup {
                    n,
                    popup,
                    surface: surface.id(),
                    parent,
                });
            }
            xdg_surface::Request::AckConfigure { .. } if state.dead.contains(&surface.id()) => {
                let what = "an ack on the surface of a destroyed popup".to_string();
                state.fail(surface, what);
            }
            xdg_surface::Request::Destroy => {
                if state.popups.iter().any(|p| p.surface == surface.id()) {
                    let what = "an xdg_surface destroyed before its popup".to_string();
                    state.fail(surface, what);
                }
                state.dead.retain(|d| *d != surface.id());
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        popup: &xdg_popup::XdgPopup,
        request: xdg_popup::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(p) = state.find(popup) else {
            return;
        };
        let (n, surface, parent) = (p.n, p.surface.clone(), p.parent.clone());
        match request {
            xdg_popup::Request::Grab { .. } => {
                state.asked.push(format!("grab {n}"));
                let top = state.grabs.last().and_then(|g| {
                    state
                        .popups
                        .iter()
                        .find(|p| p.popup.id() == *g)
                        .map(|p| p.surface.clone())
                });
                if top.is_some() && top != parent {
                    let what = format!("popup {n} grabs but is not on the topmost popup");
                    state.fail(popup, what);
                    return;
                }
                state.grabs.push(popup.id());
            }
            xdg_popup::Request::Destroy => {
                state.asked.push(format!("destroy {n}"));
                if state
                    .popups
                    .iter()
                    .any(|c| c.parent.as_ref() == Some(&surface))
                {
                    let what = format!("popup {n} destroyed under another popup");
                    state.fail(popup, what);
                }
                state.popups.retain(|p| p.popup.id() != popup.id());
                state.grabs.retain(|g| *g != popup.id());
                let wl = state.surfaces.get(&surface).cloned();
                state.dead.extend([Some(surface), wl].into_iter().flatten());
            }
            _ => {}
        }
    }
}

/// What a window of the test is asked to do.
enum Ask {
    /// Show a popup and make it ready for popups on it.
    Show(PopupParams),
    /// Keep dismissed popups open until told otherwise, as a thread busy with something else.
    Hold(bool),
}

struct Window {
    name: &'static str,
    asks: mpsc::Receiver<Ask>,
    events: mpsc::Sender<(&'static str, WindowEvent)>,
    mapped: bool,
    hold: bool,
    /// Dismissed popups it was told to keep open.
    held: Vec<PopupId>,
}

impl WaylandHandler for Window {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
        // A dismissed popup closes, as the core closes it.
        if let WindowEvent::PopupDismissed(id) = event {
            if self.hold {
                self.held.push(id);
            } else {
                ctx.close_popup(id);
            }
        }
        let _ = self.events.send((self.name, event));
    }
    fn frame(&mut self, _: &mut Ctx<'_>) {}
    fn deadline(&mut self, now: Instant) -> Option<Instant> {
        Some(now + Duration::from_millis(1))
    }
    fn timer(&mut self, ctx: &mut Ctx<'_>, _: Instant) {
        if !std::mem::replace(&mut self.mapped, true) {
            ctx.committed(SurfaceId::Window);
        }
        while let Ok(ask) = self.asks.try_recv() {
            match ask {
                Ask::Show(params) => {
                    let id = params.id;
                    ctx.show_popup(params);
                    ctx.committed(SurfaceId::Popup(id));
                }
                Ask::Hold(hold) => {
                    self.hold = hold;
                    if !hold {
                        for id in std::mem::take(&mut self.held) {
                            ctx.close_popup(id);
                        }
                    }
                }
            }
        }
    }
}

struct Windows(Vec<(WindowKey, Window)>);

impl Handlers for Windows {
    type Handler = Window;
    fn handler(&mut self, key: WindowKey) -> Option<&mut Window> {
        self.0.iter_mut().find(|(k, _)| *k == key).map(|(_, w)| w)
    }
}

/// Runs the windows of a session until the connection fails, and says why.
fn run_session(mut session: Session, mut windows: Windows, ended: mpsc::Sender<String>) {
    let error = loop {
        if let Err(e) = session.dispatch(&mut windows) {
            break e;
        }
        if let Err(e) = session.wait(&mut windows) {
            break e;
        }
    };
    let _ = ended.send(error.to_string());
}

fn popup(id: u64, parent: Option<u64>, kind: PopupKind) -> Ask {
    Ask::Show(PopupParams {
        id: PopupId(id),
        parent: parent.map(PopupId),
        kind,
        placement: Placement::BelowStart,
        anchor: RectI::new(0, 0, 10, 10),
        gap: 0,
        wanted: SizeI::new(50, 50),
        buffer: SizeI::new(50, 50),
        logical: SizeI::new(50, 50),
        geometry: RectI::new(0, 0, 50, 50),
        position: None,
    })
}

struct World {
    display: Display<Compositor>,
    socket: ListeningSocket,
    state: Compositor,
    clients: Vec<Client>,
}

impl World {
    fn pump(&mut self) {
        while let Ok(Some(stream)) = self.socket.accept() {
            let client = self
                .display
                .handle()
                .insert_client(stream, Arc::new(NoData))
                .unwrap();
            self.clients.push(client);
        }
        self.display.dispatch_clients(&mut self.state).unwrap();
        self.display.flush_clients().unwrap();
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut World) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(
                self.state.errors.is_empty(),
                "the compositor ended the client: {:?}",
                self.state.errors
            );
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}; asked {:?}",
                self.state.asked
            );
            self.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The next `n` popups dismissed, by window, in order.
    fn dismissed(
        &mut self,
        events: &mpsc::Receiver<(&'static str, WindowEvent)>,
        n: usize,
    ) -> Vec<(&'static str, u64)> {
        let mut dismissed = Vec::new();
        self.until("popups to be dismissed", |_| {
            dismissed.extend(events.try_iter().filter_map(|(name, e)| match e {
                WindowEvent::PopupDismissed(id) => Some((name, id.0)),
                _ => None,
            }));
            dismissed.len() >= n
        });
        dismissed.sort();
        dismissed
    }

    /// What was asked about popups, up to and with `what`.
    fn until_asked(&mut self, what: &str) -> Vec<String> {
        self.until(what, |w| w.state.asked.iter().any(|a| a == what));
        let at = self.state.asked.iter().position(|a| a == what).unwrap();
        self.state.asked.drain(..=at).collect()
    }
}

#[test]
fn windows_of_one_connection_share_one_popup_grab() {
    let dir = std::env::temp_dir().join(format!("anti-webui-grab-{}", std::process::id()));
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
    // SAFETY: only one test here, before the windows look for their compositor.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &path);
        std::env::remove_var("WAYLAND_SOCKET");
    }
    let mut w = World {
        display,
        socket,
        state: Compositor::default(),
        clients: Vec::new(),
    };

    // Windows `a` and `b` share a thread; `c` is on another thread, on the same connection.
    let (events_tx, events) = mpsc::channel();
    let (ended_tx, ended) = mpsc::channel();
    let (share_tx, share_rx) = mpsc::channel();
    let (to_a, a_asks) = mpsc::channel();
    let (to_b, b_asks) = mpsc::channel();
    let (to_c, c_asks) = mpsc::channel();
    let window = |name, asks| Window {
        name,
        asks,
        events: events_tx.clone(),
        mapped: false,
        hold: false,
        held: Vec::new(),
    };
    let (a, b, c) = (
        window("a", a_asks),
        window("b", b_asks),
        window("c", c_asks),
    );
    let first_ended = ended_tx.clone();
    std::thread::spawn(move || {
        let mut session = Session::connect().unwrap();
        let ka = session.open(WindowOptions::default()).unwrap();
        let kb = session.open(WindowOptions::default()).unwrap();
        share_tx.send(session.share()).unwrap();
        run_session(session, Windows(vec![(ka, a), (kb, b)]), first_ended);
    });
    let mut share = None;
    w.until("the first session", |_| {
        share = share_rx.try_recv().ok();
        share.is_some()
    });
    let share = share.unwrap();
    std::thread::spawn(move || {
        let mut session = Session::join(share).unwrap();
        let kc = session.open(WindowOptions::default()).unwrap();
        run_session(session, Windows(vec![(kc, c)]), ended_tx);
    });

    // A menu of `a`, a tooltip on it, and a submenu on it: one window nests as before.
    to_a.send(popup(1, None, PopupKind::Menu)).unwrap();
    assert_eq!(w.until_asked("grab 1"), ["popup 1 on 0", "grab 1"]);
    to_a.send(popup(2, Some(1), PopupKind::Tooltip)).unwrap();
    assert_eq!(w.until_asked("popup 2 on 1"), ["popup 2 on 1"]);
    to_a.send(popup(3, Some(1), PopupKind::Menu)).unwrap();
    assert_eq!(w.until_asked("grab 3"), ["popup 3 on 1", "grab 3"]);

    // `b` opens a menu, as on a click that came to it before `a` was told its menu ended. `a` is
    // asked to dismiss the grabs in the way, and `b` waits: nothing is destroyed and nothing made
    // while `a` keeps them, as a busy thread would.
    to_a.send(Ask::Hold(true)).unwrap();
    to_b.send(popup(1, None, PopupKind::Menu)).unwrap();
    let dismissed = w.dismissed(&events, 2);
    assert_eq!(dismissed, [("a", 1), ("a", 3)]);
    let end = Instant::now() + Duration::from_millis(100);
    w.until("a while", |_| Instant::now() >= end);
    assert!(w.state.asked.is_empty(), "{:?}", w.state.asked);
    // `a` closes them, top first, and then `b` makes its menu.
    to_a.send(Ask::Hold(false)).unwrap();
    let mut asked = w.until_asked("grab 4");
    asked[..2].sort();
    assert_eq!(
        asked,
        [
            "destroy 2",
            "destroy 3",
            "destroy 1",
            "popup 4 on 0",
            "grab 4"
        ]
    );

    // The same from another thread.
    to_c.send(popup(1, None, PopupKind::Menu)).unwrap();
    assert_eq!(
        w.until_asked("grab 5"),
        ["destroy 4", "popup 5 on 0", "grab 5"]
    );
    assert_eq!(w.dismissed(&events, 1), [("b", 1)]);

    // A second menu of one window that isn't on the first: the first goes.
    to_c.send(popup(2, None, PopupKind::Menu)).unwrap();
    assert_eq!(
        w.until_asked("grab 6"),
        ["destroy 5", "popup 6 on 0", "grab 6"]
    );
    assert_eq!(w.dismissed(&events, 1), [("c", 1)]);

    // The connection is still there.
    w.pump();
    assert!(w.state.errors.is_empty(), "{:?}", w.state.errors);
    assert!(ended.try_recv().is_err());
}
