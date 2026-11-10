//! Drag and drop against a small compositor that keeps to the protocol and writes down what it is
//! asked. It shows the requests are what `wl_data_device` wants, not how a real compositor behaves.
//! See `docs/design/23-drag-and-drop.md`. Pinching goes through the same small compositor.

#![cfg(target_os = "linux")]

use anti_webui_core::event::{
    DragAction, DragActions, DragResult, PinchPhase, PointerButton, WindowEvent,
};
use anti_webui_core::geometry::{PointF, PointI, SizeI};
use anti_webui_core::platform::DragOffer;
use anti_webui_wayland::{Ctx, WaylandHandler, WindowOptions, run};
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wayland_protocols::wp::pointer_gestures::zv1::server::{
    zwp_pointer_gesture_hold_v1, zwp_pointer_gesture_pinch_v1, zwp_pointer_gesture_swipe_v1,
    zwp_pointer_gestures_v1,
};
use wayland_protocols::xdg::shell::server::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::protocol::wl_data_device_manager::DndAction;
use wayland_server::protocol::{
    wl_callback, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_pointer, wl_seat, wl_surface,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource, WEnum, backend::ClientData,
};

#[derive(Default)]
struct Compositor {
    surfaces: Vec<wl_surface::WlSurface>,
    pointer: Option<wl_pointer::WlPointer>,
    device: Option<wl_data_device::WlDataDevice>,
    source_mimes: Vec<String>,
    source_actions: Option<DndAction>,
    /// `source`, `origin`, `icon`, `serial`.
    drag: Option<(
        wl_data_source::WlDataSource,
        wl_surface::WlSurface,
        wl_surface::WlSurface,
        u32,
    )>,
    sources_destroyed: u32,
    surfaces_destroyed: u32,
    offsets: Vec<(i32, i32)>,
    accepted: Vec<(u32, Option<String>)>,
    offer_actions: Vec<(DndAction, DndAction)>,
    received: Vec<(String, OwnedFd)>,
    finished: u32,
    offers_destroyed: u32,
    pinch: Option<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1>,
    holds: u32,
}

struct NoData;
impl ClientData for NoData {}

macro_rules! global {
    ($($ty:ty),*) => {$(
        impl GlobalDispatch<$ty, ()> for Compositor {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$ty>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                init.init(resource, ());
            }
        }
    )*};
}

global!(
    wl_compositor::WlCompositor,
    xdg_wm_base::XdgWmBase,
    wl_data_device_manager::WlDataDeviceManager,
    zwp_pointer_gestures_v1::ZwpPointerGesturesV1
);

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
        seat.capabilities(wl_seat::Capability::Pointer);
    }
}

macro_rules! ignored {
    ($($ty:ty),*) => {$(
        impl Dispatch<$ty, ()> for Compositor {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$ty,
                _: <$ty as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    )*};
}

ignored!(
    xdg_toplevel::XdgToplevel,
    wl_pointer::WlPointer,
    wl_callback::WlCallback,
    zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
    zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1,
    zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1
);

impl Dispatch<zwp_pointer_gestures_v1::ZwpPointerGesturesV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
        request: zwp_pointer_gestures_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use zwp_pointer_gestures_v1::Request;
        match request {
            Request::GetPinchGesture { id, .. } => state.pinch = Some(init.init(id, ())),
            Request::GetHoldGesture { id, .. } => {
                state.holds += 1;
                init.init(id, ());
            }
            Request::GetSwipeGesture { id, .. } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            state.surfaces.push(init.init(id, ()));
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
            wl_surface::Request::Offset { x, y } => state.offsets.push((x, y)),
            wl_surface::Request::Destroy => state.surfaces_destroyed += 1,
            _ => {}
        }
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
        _: &mut Self,
        _: &Client,
        _: &xdg_surface::XdgSurface,
        request: xdg_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetToplevel { id } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_seat::WlSeat,
        request: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            state.pointer = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_data_device_manager::WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                state.source_mimes.clear();
                state.source_actions = None;
                init.init(id, ());
            }
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                state.device = Some(init.init(id, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_data_source::WlDataSource,
        request: wl_data_source::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_source::Request::Offer { mime_type } => state.source_mimes.push(mime_type),
            wl_data_source::Request::SetActions {
                dnd_actions: WEnum::Value(a),
            } => state.source_actions = Some(a),
            wl_data_source::Request::Destroy => state.sources_destroyed += 1,
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_data_device::WlDataDevice,
        request: wl_data_device::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_device::Request::StartDrag {
            source: Some(source),
            origin,
            icon: Some(icon),
            serial,
        } = request
        {
            state.drag = Some((source, origin, icon, serial));
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_data_offer::WlDataOffer,
        request: wl_data_offer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_offer::Request::Accept { serial, mime_type } => {
                state.accepted.push((serial, mime_type))
            }
            wl_data_offer::Request::Receive { mime_type, fd } => {
                state.received.push((mime_type, fd))
            }
            wl_data_offer::Request::SetActions {
                dnd_actions: WEnum::Value(actions),
                preferred_action: WEnum::Value(preferred),
            } => state.offer_actions.push((actions, preferred)),
            wl_data_offer::Request::Finish => state.finished += 1,
            wl_data_offer::Request::Destroy => state.offers_destroyed += 1,
            _ => {}
        }
    }
}

enum Command {
    Start(DragOffer),
    Place(SizeI, PointI),
    Accept(Option<(String, DragAction)>),
    Receive(String),
    Finish,
    Cancel,
    CloseIcon,
    Exit,
}

struct Client_ {
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<WindowEvent>,
    supports: mpsc::Sender<bool>,
}

impl WaylandHandler for Client_ {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
        let _ = self.supports.send(ctx.supports_drag());
        let _ = self.events.send(event);
    }
    fn frame(&mut self, _: &mut Ctx<'_>) {}
    fn deadline(&mut self, now: Instant) -> Option<Instant> {
        Some(now + Duration::from_millis(1))
    }
    fn timer(&mut self, ctx: &mut Ctx<'_>, _: Instant) {
        let _ = self.supports.send(ctx.supports_drag());
        while let Ok(c) = self.commands.try_recv() {
            match c {
                Command::Start(offer) => ctx.start_drag(offer),
                Command::Place(size, hotspot) => ctx.place_drag_icon(size, hotspot),
                Command::Accept(wanted) => ctx.accept_drag(wanted),
                Command::Receive(mime) => ctx.receive_drop(mime),
                Command::Finish => ctx.finish_drop(),
                Command::Cancel => ctx.cancel_drag(),
                Command::CloseIcon => ctx.close_drag_icon(),
                Command::Exit => ctx.exit(),
            }
        }
    }
}

struct World {
    display: Display<Compositor>,
    socket: ListeningSocket,
    state: Compositor,
    client: Option<Client>,
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<WindowEvent>,
    supports: mpsc::Receiver<bool>,
    serial: u32,
}

impl World {
    fn pump(&mut self) {
        if let Ok(Some(stream)) = self.socket.accept() {
            let c = self
                .display
                .handle()
                .insert_client(stream, Arc::new(NoData))
                .unwrap();
            self.client = Some(c);
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

    fn next_event(&mut self) -> WindowEvent {
        let mut got = None;
        self.until("an event", |w| {
            got = w.events.try_recv().ok();
            got.is_some()
        });
        got.unwrap()
    }

    fn next_serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }

    /// Presses the button on the window, as the start of everything dragged from it.
    fn press(&mut self) -> u32 {
        self.until("the pointer", |w| {
            w.state.pointer.is_some() && w.state.device.is_some()
        });
        let window = self.state.surfaces[0].clone();
        let (enter, press) = (self.next_serial(), self.next_serial());
        let pointer = self.state.pointer.clone().unwrap();
        pointer.enter(enter, &window, 10.0, 20.0);
        pointer.button(press, 0, 0x110, wl_pointer::ButtonState::Pressed);
        self.pump();
        loop {
            if let WindowEvent::PointerButton { button, .. } = self.next_event() {
                assert_eq!(button, PointerButton::Primary);
                return press;
            }
        }
    }

    /// Offers something to the window, over the window.
    fn enter(&mut self, mimes: &[&str], x: f64, y: f64) -> wl_data_offer::WlDataOffer {
        let dh = self.display.handle();
        let client = self.client.clone().unwrap();
        let device = self.state.device.clone().unwrap();
        let offer = client
            .create_resource::<wl_data_offer::WlDataOffer, (), Compositor>(&dh, 3, ())
            .unwrap();
        device.data_offer(&offer);
        for m in mimes {
            offer.offer(m.to_string());
        }
        let window = self.state.surfaces[0].clone();
        let serial = self.next_serial();
        device.enter(serial, &window, x, y, Some(&offer));
        self.pump();
        offer
    }
}

fn setup() -> World {
    let dir = std::env::temp_dir().join(format!("anti-webui-dnd-{}", std::process::id()));
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
    dh.create_global::<Compositor, wl_data_device_manager::WlDataDeviceManager, ()>(3, ());
    dh.create_global::<Compositor, zwp_pointer_gestures_v1::ZwpPointerGesturesV1, ()>(3, ());
    // SAFETY: the only test here, and nothing runs yet that reads the environment.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &path);
        std::env::remove_var("WAYLAND_SOCKET");
    }
    let (commands, command_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let (supports_tx, supports) = mpsc::channel();
    std::thread::spawn(move || {
        let client = Client_ {
            commands: command_rx,
            events: event_tx,
            supports: supports_tx,
        };
        let options = WindowOptions {
            title: "dnd".into(),
            app_id: "dnd".into(),
            width: 100,
            height: 100,
            fullscreen: false,
            ..Default::default()
        };
        run(options, client).unwrap();
    });
    World {
        display,
        socket,
        state: Compositor::default(),
        client: None,
        commands,
        events,
        supports,
        serial: 100,
    }
}

fn offer_of(mimes: &[&str], actions: DragActions) -> DragOffer {
    DragOffer {
        items: mimes
            .iter()
            .map(|m| {
                (
                    m.to_string(),
                    Arc::from(format!("data of {m}").into_bytes()),
                )
            })
            .collect(),
        actions,
    }
}

#[test]
fn drag_and_drop_goes_by_the_protocol() {
    let mut w = setup();

    // Everything but `wl_data_device_manager` v3 falls back to staying in the window.
    let press = w.press();
    let mut supported = None;
    w.until("the client's say", |w| {
        supported = w.supports.try_recv().ok();
        supported.is_some()
    });
    assert_eq!(supported, Some(true));

    // Starting takes the press of the button.
    w.commands
        .send(Command::Start(offer_of(
            &["text/plain", "text/plain", "text/uri-list"],
            DragActions::Copy,
        )))
        .unwrap();
    w.until("start_drag", |w| w.state.drag.is_some());
    let (source, origin, icon, serial) = w.state.drag.clone().unwrap();
    assert_eq!(serial, press);
    assert_eq!(
        origin, w.state.surfaces[0],
        "it starts from what was pressed"
    );
    assert_ne!(icon, origin);
    assert_eq!(
        w.state.source_mimes,
        ["text/plain", "text/uri-list"],
        "once each"
    );
    assert_eq!(w.state.source_actions, Some(DndAction::Copy));

    // Where the pointer is on the surface is relative, and only sent when it changes.
    w.commands
        .send(Command::Place(SizeI::new(40, 30), PointI::new(5, 6)))
        .unwrap();
    w.commands
        .send(Command::Place(SizeI::new(40, 30), PointI::new(5, 6)))
        .unwrap();
    w.commands
        .send(Command::Place(SizeI::new(40, 30), PointI::new(8, 6)))
        .unwrap();
    w.until("offsets", |w| w.state.offsets.len() >= 2);
    assert_eq!(w.state.offsets, [(-5, -6), (-3, 0)]);

    // Others get the data when they ask.
    let (read, write) = rustix::pipe::pipe().unwrap();
    source.send("text/uri-list".into(), write.as_fd());
    drop(write);
    let mut got = String::new();
    w.pump();
    let mut file = std::fs::File::from(read);
    w.until("the data", |_| {
        file.read_to_string(&mut got).is_ok_and(|n| n > 0)
    });
    assert_eq!(got, "data of text/uri-list");

    // It comes over this window, and is taken here without reading it.
    let offer = w.enter(&["text/plain", "text/uri-list"], 30.0, 40.0);
    let event = w.next_event();
    let WindowEvent::DragEntered {
        position, mimes, ..
    } = event
    else {
        panic!("{event:?}");
    };
    assert_eq!(position, PointF::new(30.0, 40.0));
    assert_eq!(mimes, ["text/plain", "text/uri-list"]);
    let enter_serial = w.serial;
    w.commands
        .send(Command::Accept(Some((
            "text/plain".into(),
            DragAction::Copy,
        ))))
        .unwrap();
    w.until("accept", |w| !w.state.offer_actions.is_empty());
    assert_eq!(
        w.state.accepted,
        [(enter_serial, Some("text/plain".to_string()))]
    );
    assert_eq!(
        w.state.offer_actions,
        [(DndAction::Copy | DndAction::Move, DndAction::Copy)]
    );

    let device = w.state.device.clone().unwrap();
    device.motion(0, 31.5, 41.0);
    w.pump();
    let event = w.next_event();
    assert!(
        matches!(event, WindowEvent::DragMoved { position, .. } if position == PointF::new(31.5, 41.0)),
        "{event:?}"
    );

    offer.action(DndAction::Copy);
    source.action(DndAction::Copy);
    device.drop();
    // Some compositors say this too, which isn't the end of what was dropped.
    device.leave();
    w.pump();
    let event = w.next_event();
    assert_eq!(
        event,
        WindowEvent::DragDropped {
            position: PointF::new(31.5, 41.0),
            action: DragAction::Copy
        }
    );
    w.until("the leave to pass", |w| {
        w.pump();
        true
    });
    assert_eq!(w.state.offers_destroyed, 0, "it is still needed");
    w.commands.send(Command::Finish).unwrap();
    w.until("finish", |w| w.state.finished == 1);
    w.until("the offer to go", |w| w.state.offers_destroyed == 1);

    source.dnd_drop_performed();
    source.dnd_finished();
    w.pump();
    assert_eq!(
        w.next_event(),
        WindowEvent::DragSourceEnded(DragResult::Copied)
    );
    w.until("the source to go", |w| w.state.sources_destroyed == 1);
    w.commands.send(Command::CloseIcon).unwrap();
    w.until("the surface to go", |w| w.state.surfaces_destroyed == 1);

    // From another program: it is read after the drop, and only then finished.
    let offer = w.enter(&["image/png", "text/uri-list"], 1.0, 2.0);
    let event = w.next_event();
    assert!(
        matches!(event, WindowEvent::DragEntered { .. }),
        "{event:?}"
    );
    w.commands
        .send(Command::Accept(Some((
            "text/uri-list".into(),
            DragAction::Copy,
        ))))
        .unwrap();
    let was = w.state.offer_actions.len();
    w.until("accept", |w| w.state.offer_actions.len() > was);
    offer.action(DndAction::Copy);
    device.drop();
    w.pump();
    let event = w.next_event();
    assert!(
        matches!(event, WindowEvent::DragDropped { .. }),
        "{event:?}"
    );
    w.commands
        .send(Command::Receive("text/uri-list".into()))
        .unwrap();
    w.until("receive", |w| !w.state.received.is_empty());
    assert_eq!(w.state.finished, 1, "not before it has been read");
    let (mime, fd) = w.state.received.remove(0);
    assert_eq!(mime, "text/uri-list");
    let mut file = std::fs::File::from(fd);
    file.write_all(b"file:///a\r\n").unwrap();
    drop(file);
    let event = w.next_event();
    assert_eq!(
        event,
        WindowEvent::DropData {
            mime: "text/uri-list".into(),
            bytes: b"file:///a\r\n".to_vec()
        }
    );
    w.until("finish", |w| w.state.finished == 2);
    w.until("the offer to go", |w| w.state.offers_destroyed == 2);

    // Taking back what was accepted.
    let _offer = w.enter(&["text/plain"], 1.0, 2.0);
    let _ = w.next_event();
    let serial = w.serial;
    w.commands.send(Command::Accept(None)).unwrap();
    let was = w.state.accepted.len();
    w.until("accept", |w| w.state.accepted.len() > was);
    assert_eq!(w.state.accepted.last(), Some(&(serial, None)));
    assert_eq!(
        w.state.offer_actions.last(),
        Some(&(DndAction::empty(), DndAction::empty()))
    );
    // It leaves, and the next thing is on its own.
    device.leave();
    w.pump();
    assert_eq!(w.next_event(), WindowEvent::DragLeft);
    w.until("the offer to go", |w| w.state.offers_destroyed == 3);

    // Calling it off destroys what is offered, which is how the protocol says that, and says so.
    w.commands
        .send(Command::Start(offer_of(&["text/plain"], DragActions::Both)))
        .unwrap();
    let previous = w.state.drag.take();
    w.until("start_drag", |w| w.state.drag.is_some());
    assert_ne!(previous.map(|d| d.0), w.state.drag.clone().map(|d| d.0));
    assert_eq!(
        w.state.source_actions,
        Some(DndAction::Move | DndAction::Copy)
    );
    w.commands.send(Command::Cancel).unwrap();
    w.until("the source to go", |w| w.state.sources_destroyed == 2);
    assert_eq!(
        w.next_event(),
        WindowEvent::DragSourceEnded(DragResult::Cancelled)
    );

    // The compositor can also call it off.
    w.commands
        .send(Command::Start(offer_of(&["text/plain"], DragActions::Both)))
        .unwrap();
    w.state.drag = None;
    w.until("start_drag", |w| w.state.drag.is_some());
    let (source, ..) = w.state.drag.clone().unwrap();
    source.cancelled();
    w.pump();
    assert_eq!(
        w.next_event(),
        WindowEvent::DragSourceEnded(DragResult::Cancelled)
    );
    w.until("the source to go", |w| w.state.sources_destroyed == 3);

    pinching_says_how_much_bigger_it_got(&mut w);

    w.commands.send(Command::Exit).unwrap();
}

/// A pinch goes through as how much bigger it got since the last time.
fn pinching_says_how_much_bigger_it_got(w: &mut World) {
    w.until("the gesture", |w| w.state.pinch.is_some());
    assert_eq!(w.state.holds, 1);
    w.press();
    let pinch = w.state.pinch.clone().unwrap();
    let window = w.state.surfaces[0].clone();
    let next = |w: &mut World, want: PinchPhase, scale: f32| loop {
        if let WindowEvent::Pinch {
            phase,
            scale: got,
            position,
            ..
        } = w.next_event()
        {
            assert_eq!(phase, want);
            assert!((got - scale).abs() < 0.001, "{got} and not {scale}");
            assert_eq!(position, PointF::new(10.0, 20.0));
            return;
        }
    };
    let serial = w.next_serial();
    pinch.begin(serial, 0, &window, 2);
    pinch.update(1, 5.0, 5.0, 1.5, 0.0);
    pinch.update(2, 0.0, 0.0, 3.0, 0.0);
    pinch.update(3, 0.0, 0.0, 1.5, 0.0);
    let serial = w.next_serial();
    pinch.end(serial, 4, 0);
    w.pump();
    next(w, PinchPhase::Began, 1.0);
    next(w, PinchPhase::Moved, 1.5);
    next(w, PinchPhase::Moved, 2.0);
    next(w, PinchPhase::Moved, 0.5);
    next(w, PinchPhase::Ended, 1.0);
    // It starts counting again.
    let serial = w.next_serial();
    pinch.begin(serial, 5, &window, 2);
    pinch.update(6, 0.0, 0.0, 2.0, 0.0);
    let serial = w.next_serial();
    pinch.end(serial, 7, 1);
    w.pump();
    next(w, PinchPhase::Began, 1.0);
    next(w, PinchPhase::Moved, 2.0);
    next(w, PinchPhase::Cancelled, 1.0);
}
