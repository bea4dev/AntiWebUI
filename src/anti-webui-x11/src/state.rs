//! The window and what the server says about it.

use crate::atoms::Atoms;
use crate::cursor::Cursors;
use crate::ime::{Ime, ImeOut};
use crate::input::{self, Button, ScrollDevices};
use crate::keyboard::Keyboard;
use crate::popup::Popup;
use crate::present::{Frames, PresentState};
use crate::selection::{self, Selections};
use crate::settings;
use crate::xdnd::Dnd;
use crate::{Error, WindowOptions, err};
use anti_webui_core::event::{PinchPhase, ScrollPhase, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, ImeState, PopupId, PopupKind, SurfaceId};
use std::collections::{HashMap, HashSet};
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::time::Instant;
use x11rb::NONE;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::properties::WmSizeHints;
use x11rb::protocol::Event;
use x11rb::protocol::sync::{self, ConnectionExt as _};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    self, Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _,
    CreateWindowAux, EventMask, Gravity, PropMode, Window, WindowClass,
};
use x11rb::resource_manager;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

/// How long a popup may hold up the next frame by not getting its presentation done.
const POPUP_FRAME_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

pub(crate) struct State {
    /// Shared with the input method's client.
    pub conn: Rc<XCBConnection>,
    pub screen: usize,
    pub root: Window,
    pub window: Window,
    pub depth: u8,
    pub atoms: Atoms,
    pub xi_opcode: Option<u8>,
    xi_minor: u16,
    /// The master pointer, which menus grab.
    pub pointer_device: u16,
    pub grabbed: bool,
    pub xwayland: bool,
    /// What the window manager says it does.
    wm_supported: Vec<Atom>,

    pub scale: Scale,
    /// Whether `ANTIWEBUI_SCALE` set it.
    scale_fixed: bool,
    xsettings: Option<Window>,
    pub size: SizeI,
    size_announced: Option<(SizeI, Scale)>,
    pub mapped: bool,
    pub closed: bool,
    pub exposed: bool,
    pub(crate) fullscreen: Option<bool>,
    pub(crate) maximized: bool,
    pub(crate) decorations: anti_webui_core::window::Decorations,
    /// The edge the pointer is on, for a window with a frame of its own.
    pub(crate) border: Option<anti_webui_core::window::ResizeEdge>,

    pub kb: Option<Keyboard>,
    ime: Option<Ime>,
    ime_wanted: Option<ImeState>,
    keys_down: HashSet<u8>,
    focused: bool,
    focus_announced: bool,
    /// The time of the last input, for selections and grabs.
    pub last_time: u32,

    scroll: ScrollDevices,
    /// The touchpad properties of libinput and synaptics, if the server knows them.
    touchpad_atoms: Vec<Atom>,
    pinch_scale: f64,
    pub pointer: PointF,
    /// Which of our windows the pointer is on, for the cursor.
    pointer_window: Window,
    pub(crate) cursors: Cursors,
    pub(crate) cursor: CursorIcon,

    /// `_NET_WM_SYNC_REQUEST`: the counter, the value asked for, and the value to set after the
    /// next frame of the new size.
    sync_counter: Option<sync::Counter>,
    sync_asked: Option<i64>,
    sync_after_frame: Option<i64>,

    pub present: PresentState,
    pub window_frames: Frames,
    pub popups: HashMap<PopupId, Popup>,
    pub selections: Selections,
    pub dnd: Dnd,
    /// Where the window is on the screen, to close menus when it moves.
    window_at: Option<(i16, i16)>,

    pub wake: OwnedFd,
    pub redraw_requested: bool,
    pub redraw_at: Option<Instant>,
    pub events: Vec<WindowEvent>,
}

/// The X server's version of `xproto::ConfigureWindow` sizes are `u16`.
fn clamp_u16(v: i32) -> u16 {
    v.clamp(1, u16::MAX as i32) as u16
}

impl State {
    pub fn new(options: &WindowOptions, wake: OwnedFd) -> Result<Self, Error> {
        let (conn, screen) = XCBConnection::connect(None).map_err(err)?;
        let conn = Rc::new(conn);
        let setup = conn.setup();
        let root = setup.roots[screen].root;
        let root_visual = setup.roots[screen].root_visual;
        let depth = setup.roots[screen].root_depth;
        let atoms = Atoms::new(&conn).map_err(err)?.reply().map_err(err)?;
        let db = resource_manager::new_from_default(&*conn).map_err(err)?;
        let xwayland = conn
            .extension_information("XWAYLAND")
            .ok()
            .flatten()
            .is_some();

        let (scale, scale_fixed) = match settings::scale_from_env() {
            Some(s) => (s, true),
            None => (Scale::ONE, false),
        };
        let window = conn.generate_id().map_err(err)?;
        let mut state = Self {
            cursors: Cursors::new(&*conn, screen, &db),
            conn,
            screen,
            root,
            window,
            depth,
            atoms,
            xi_opcode: None,
            xi_minor: 0,
            pointer_device: 2,
            grabbed: false,
            xwayland,
            wm_supported: Vec::new(),
            scale,
            scale_fixed,
            xsettings: None,
            size: SizeI::new(1, 1),
            size_announced: None,
            mapped: false,
            closed: false,
            exposed: false,
            fullscreen: None,
            maximized: false,
            decorations: match options.decorations {
                anti_webui_core::window::DecorationMode::Client => {
                    anti_webui_core::window::Decorations::Client
                }
                _ => anti_webui_core::window::Decorations::Server,
            },
            border: None,
            kb: None,
            ime: None,
            ime_wanted: None,
            keys_down: HashSet::new(),
            focused: false,
            focus_announced: false,
            last_time: 0,
            scroll: ScrollDevices::new(if xwayland { 10.0 } else { 15.0 }),
            touchpad_atoms: Vec::new(),
            pinch_scale: 1.0,
            pointer: PointF::new(0.0, 0.0),
            pointer_window: window,
            cursor: CursorIcon::Default,
            sync_counter: None,
            sync_asked: None,
            sync_after_frame: None,
            present: PresentState::default(),
            window_frames: Frames::default(),
            popups: HashMap::new(),
            selections: Selections::default(),
            dnd: Dnd::default(),
            window_at: None,
            wake,
            redraw_requested: true,
            redraw_at: None,
            events: Vec::new(),
        };
        if !scale_fixed {
            state.scale = state.read_scale(&db);
        }
        state.wm_supported = state.read_atoms(root, state.atoms._NET_SUPPORTED);
        state.create_window(options, root_visual)?;
        state.present = PresentState::new(&state.conn, root, window, depth);
        state.announce_xdnd();
        state.setup_input();
        state.ime = Ime::new(state.conn.clone(), screen, window);
        state.conn.map_window(window).map_err(err)?;
        state.conn.flush().map_err(err)?;
        Ok(state)
    }

    fn create_window(&mut self, options: &WindowOptions, visual: u32) -> Result<(), Error> {
        let (conn, a, window) = (&self.conn, &self.atoms, self.window);
        let n = self.scale.as_f64();
        let w = (options.width as f64 * n).round() as i32;
        let h = (options.height as f64 * n).round() as i32;
        self.size = SizeI::new(w.max(1), h.max(1));
        let mask = EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::FOCUS_CHANGE
            | EventMask::PROPERTY_CHANGE
            | EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE;
        let aux = CreateWindowAux::new()
            .background_pixmap(NONE)
            .border_pixel(0)
            .bit_gravity(Gravity::NORTH_WEST)
            .event_mask(mask);
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            self.root,
            0,
            0,
            clamp_u16(w),
            clamp_u16(h),
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &aux,
        )
        .map_err(err)?;
        let utf8 = a.UTF8_STRING;
        let set8 = |prop: Atom, ty: Atom, data: &[u8]| {
            conn.change_property8(PropMode::REPLACE, window, prop, ty, data)
        };
        set8(a._NET_WM_NAME, utf8, options.title.as_bytes()).map_err(err)?;
        set8(AtomEnum::WM_NAME.into(), utf8, options.title.as_bytes()).map_err(err)?;
        let class = format!("{0}\0{0}\0", options.app_id);
        set8(
            AtomEnum::WM_CLASS.into(),
            AtomEnum::STRING.into(),
            class.as_bytes(),
        )
        .map_err(err)?;
        if let Ok(host) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            let host = host.trim().as_bytes();
            set8(a.WM_CLIENT_MACHINE, AtomEnum::STRING.into(), host).map_err(err)?;
        }
        let cardinal = u32::from(AtomEnum::CARDINAL);
        conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_PID,
            cardinal,
            &[std::process::id()],
        )
        .map_err(err)?;
        let mut protocols = vec![a.WM_DELETE_WINDOW, a._NET_WM_PING];
        if self.wm_supported.contains(&a._NET_WM_SYNC_REQUEST)
            && let Some(counter) = create_counter(conn)
        {
            protocols.push(a._NET_WM_SYNC_REQUEST);
            conn.change_property32(
                PropMode::REPLACE,
                window,
                a._NET_WM_SYNC_REQUEST_COUNTER,
                cardinal,
                &[counter],
            )
            .map_err(err)?;
            self.sync_counter = Some(counter);
        }
        conn.change_property32(
            PropMode::REPLACE,
            window,
            a.WM_PROTOCOLS,
            u32::from(AtomEnum::ATOM),
            &protocols,
        )
        .map_err(err)?;
        let kind = if options.modal {
            a._NET_WM_WINDOW_TYPE_DIALOG
        } else {
            a._NET_WM_WINDOW_TYPE_NORMAL
        };
        conn.change_property32(
            PropMode::REPLACE,
            window,
            a._NET_WM_WINDOW_TYPE,
            u32::from(AtomEnum::ATOM),
            &[kind],
        )
        .map_err(err)?;
        if let Some(parent) = &options.parent {
            conn.change_property32(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                &[parent.0],
            )
            .map_err(err)?;
        }
        if self.decorations == anti_webui_core::window::Decorations::Client {
            // Motif's hints: the decorations field (flag 2) is set, to none.
            conn.change_property32(
                PropMode::REPLACE,
                window,
                a._MOTIF_WM_HINTS,
                a._MOTIF_WM_HINTS,
                &[2, 0, 0, 0, 0],
            )
            .map_err(err)?;
        }
        let hints = WmSizeHints {
            size: Some((
                x11rb::properties::WmSizeHintsSpecification::ProgramSpecified,
                w,
                h,
            )),
            ..Default::default()
        };
        hints.set_normal_hints(conn, window).map_err(err)?;
        let mut states = Vec::new();
        if options.fullscreen {
            states.push(a._NET_WM_STATE_FULLSCREEN);
        }
        if options.modal && options.parent.is_some() {
            states.push(a._NET_WM_STATE_MODAL);
        }
        if !states.is_empty() {
            conn.change_property32(
                PropMode::REPLACE,
                window,
                a._NET_WM_STATE,
                u32::from(AtomEnum::ATOM),
                &states,
            )
            .map_err(err)?;
        }
        let root_mask = EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
        conn.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(root_mask),
        )
        .map_err(err)?;
        self.events.push(WindowEvent::Resized {
            size: self.size,
            scale: self.scale,
        });
        self.size_announced = Some((self.size, self.scale));
        self.events.push(WindowEvent::Decorations(self.decorations));
        Ok(())
    }

    /// XInput 2 for the pointer, and XKB for the keyboard.
    fn setup_input(&mut self) {
        let conn = &self.conn;
        let xi = conn
            .extension_information(xinput::X11_EXTENSION_NAME)
            .ok()
            .flatten();
        if let Some(info) = xi
            && let Ok(Ok(v)) = conn.xinput_xi_query_version(2, 4).map(|c| c.reply())
            && (v.major_version, v.minor_version) >= (2, 2)
        {
            self.xi_opcode = Some(info.major_opcode);
            self.xi_minor = v.minor_version;
            self.select_pointer(self.window);
            let conn = &self.conn;
            let _ = conn.xinput_xi_select_events(
                self.window,
                &[
                    xinput::EventMask {
                        deviceid: xinput::Device::ALL_MASTER.into(),
                        mask: vec![self.pointer_mask()],
                    },
                    xinput::EventMask {
                        deviceid: xinput::Device::ALL.into(),
                        mask: vec![xinput::XIEventMask::DEVICE_CHANGED],
                    },
                ],
            );
            let _ = conn.xinput_xi_select_events(
                self.root,
                &[xinput::EventMask {
                    deviceid: xinput::Device::ALL.into(),
                    mask: vec![xinput::XIEventMask::HIERARCHY],
                }],
            );
            let names = ["libinput Tapping Enabled", "Synaptics Off"];
            self.touchpad_atoms = names
                .iter()
                .filter_map(|n| conn.intern_atom(true, n.as_bytes()).ok()?.reply().ok())
                .map(|r| r.atom)
                .filter(|&a| a != NONE)
                .collect();
            self.read_devices();
        } else {
            // Core events then, with the wheel as buttons 4 to 7.
            let mask = EventMask::EXPOSURE
                | EventMask::STRUCTURE_NOTIFY
                | EventMask::FOCUS_CHANGE
                | EventMask::PROPERTY_CHANGE
                | EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::POINTER_MOTION
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::LEAVE_WINDOW;
            let aux = ChangeWindowAttributesAux::new().event_mask(mask);
            let _ = conn.change_window_attributes(self.window, &aux);
        }
        self.kb = Keyboard::new(&self.conn);
        if let Some(kb) = &self.kb {
            let device = kb.device() as u16;
            let events = xkb::EventType::NEW_KEYBOARD_NOTIFY
                | xkb::EventType::MAP_NOTIFY
                | xkb::EventType::STATE_NOTIFY;
            let all = xkb::MapPart::from(0xffu16);
            let _ = self.conn.xkb_select_events(
                device,
                xkb::EventType::from(0u16),
                events,
                all,
                all,
                &xkb::SelectEventsAux::new(),
            );
            let repeat = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
            let none = xkb::BoolCtrl::from(0u32);
            let _ = self
                .conn
                .xkb_per_client_flags(device, repeat, repeat, none, none, none);
        }
    }

    /// What XInput 2 events pointers send to our windows.
    pub fn pointer_mask(&self) -> xinput::XIEventMask {
        let mut mask = xinput::XIEventMask::MOTION
            | xinput::XIEventMask::BUTTON_PRESS
            | xinput::XIEventMask::BUTTON_RELEASE
            | xinput::XIEventMask::ENTER
            | xinput::XIEventMask::LEAVE;
        if self.xi_minor >= 4 {
            // `XI_GesturePinchBegin`, `Update` and `End`, which x11rb has no names for.
            mask |= xinput::XIEventMask::from((1u32 << 27) | (1 << 28) | (1 << 29));
        }
        mask
    }

    /// Asks for the pointer's events on one of our windows.
    pub fn select_pointer(&self, window: Window) {
        if self.xi_opcode.is_some() {
            let _ = self.conn.xinput_xi_select_events(
                window,
                &[xinput::EventMask {
                    deviceid: xinput::Device::ALL_MASTER.into(),
                    mask: vec![self.pointer_mask()],
                }],
            );
        } else {
            let mask = EventMask::POINTER_MOTION
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::LEAVE_WINDOW
                | EventMask::EXPOSURE
                | EventMask::STRUCTURE_NOTIFY;
            let aux = ChangeWindowAttributesAux::new().event_mask(mask);
            let _ = self.conn.change_window_attributes(window, &aux);
        }
    }

    /// The scroll axes of every device.
    fn read_devices(&mut self) {
        let Ok(Ok(reply)) = self
            .conn
            .xinput_xi_query_device(xinput::Device::ALL)
            .map(|c| c.reply())
        else {
            return;
        };
        if let Some(master) = reply
            .infos
            .iter()
            .find(|i| i.type_ == xinput::DeviceType::MASTER_POINTER)
        {
            self.pointer_device = master.deviceid;
        }
        let touchpads: HashSet<u16> = reply
            .infos
            .iter()
            .filter(|i| i.type_ == xinput::DeviceType::SLAVE_POINTER)
            .filter(|i| self.is_touchpad(i.deviceid))
            .map(|i| i.deviceid)
            .collect();
        self.scroll.set(&reply.infos, |id| touchpads.contains(&id));
    }

    fn is_touchpad(&self, device: u16) -> bool {
        if self.touchpad_atoms.is_empty() {
            return false;
        }
        let Ok(Ok(props)) = self
            .conn
            .xinput_xi_list_properties(device)
            .map(|c| c.reply())
        else {
            return false;
        };
        props
            .properties
            .iter()
            .any(|p| self.touchpad_atoms.contains(p))
    }

    /// The scale from the settings of the desktop.
    fn read_scale(&mut self, db: &resource_manager::Database) -> Scale {
        let owner = format!("_XSETTINGS_S{}", self.screen);
        self.xsettings = self
            .conn
            .intern_atom(false, owner.as_bytes())
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| self.conn.get_selection_owner(r.atom).ok()?.reply().ok())
            .map(|r| r.owner)
            .filter(|&w| w != NONE);
        if let Some(w) = self.xsettings {
            let aux = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
            let _ = self.conn.change_window_attributes(w, &aux);
            let setting = self.atoms._XSETTINGS_SETTINGS;
            let data = self
                .conn
                .get_property(false, w, setting, setting, 0, u32::MAX / 4)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.value)
                .unwrap_or_default();
            if let Some(dpi) = settings::dpi_of_xsettings(&settings::parse_xsettings(&data)) {
                return settings::scale_of_dpi(dpi);
            }
        }
        let dpi = db.get_value::<f64>("Xft.dpi", "").ok().flatten();
        settings::scale_of_dpi(dpi.unwrap_or(96.0))
    }

    fn rescale(&mut self) {
        if self.scale_fixed {
            return;
        }
        let Ok(db) = resource_manager::new_from_default(&*self.conn) else {
            return;
        };
        let scale = self.read_scale(&db);
        if scale != self.scale {
            self.scale = scale;
            self.announce_size();
        }
    }

    fn announce_size(&mut self) {
        if self.size_announced != Some((self.size, self.scale)) {
            self.size_announced = Some((self.size, self.scale));
            self.events.push(WindowEvent::Resized {
                size: self.size,
                scale: self.scale,
            });
            self.redraw_requested = true;
        }
    }

    fn read_atoms(&self, window: Window, property: Atom) -> Vec<Atom> {
        self.conn
            .get_property(false, window, property, AtomEnum::ATOM, 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        if icon != self.cursor {
            self.cursor = icon;
            self.cursors.set(&*self.conn, self.pointer_window, icon);
        }
    }

    pub fn set_title(&mut self, title: &str) {
        let (a, w) = (&self.atoms, self.window);
        let _ = self.conn.change_property8(
            PropMode::REPLACE,
            w,
            a._NET_WM_NAME,
            a.UTF8_STRING,
            title.as_bytes(),
        );
        let _ = self.conn.change_property8(
            PropMode::REPLACE,
            w,
            AtomEnum::WM_NAME,
            a.UTF8_STRING,
            title.as_bytes(),
        );
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        let a = &self.atoms;
        let event = ClientMessageEvent::new(
            32,
            self.window,
            a._NET_WM_STATE,
            [on as u32, a._NET_WM_STATE_FULLSCREEN, 0, 1, 0],
        );
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let _ = self.conn.send_event(false, self.root, mask, event);
    }

    /// A frame was presented to the window: what the window manager waits for during a resize.
    pub fn window_presented(&mut self, size: SizeI) {
        if size == self.size
            && let (Some(counter), Some(value)) = (self.sync_counter, self.sync_after_frame.take())
        {
            set_counter(&self.conn, counter, value);
        }
    }

    pub fn set_ime(&mut self, ime: Option<ImeState>) {
        self.ime_wanted = ime;
        if let Some(i) = &mut self.ime {
            i.set(self.ime_wanted.as_ref());
        }
        self.take_ime_out();
    }

    /// Passes on what the input method said.
    fn take_ime_out(&mut self) {
        let Some(ime) = &mut self.ime else { return };
        // What it was told before it was ready applies once it is.
        ime.set(self.ime_wanted.as_ref());
        for out in ime.take_out() {
            match out {
                ImeOut::Update(update) => self.events.push(WindowEvent::Ime(update)),
                ImeOut::Key(e) => {
                    let pressed = e.response_type & 0x7f == xproto::KEY_PRESS_EVENT;
                    self.key_from_server(e.detail, pressed);
                }
            }
        }
    }

    fn key_from_server(&mut self, keycode: u8, pressed: bool) {
        if pressed {
            let repeat = !self.keys_down.insert(keycode);
            self.key(keycode, true, repeat);
        } else {
            self.keys_down.remove(&keycode);
            self.key(keycode, false, false);
        }
    }

    /// Handles one event from the server.
    pub fn handle(&mut self, event: Event) {
        if let Some(ime) = &mut self.ime
            && ime.filter(&event)
        {
            self.take_ime_out();
            return;
        }
        match event {
            Event::Expose(e) if e.count == 0 => {
                if e.window == self.window {
                    self.exposed = true;
                } else if let Some(p) = self.popups.values_mut().find(|p| p.window == e.window) {
                    p.exposed = true;
                }
                self.redraw_requested = true;
            }
            Event::ConfigureNotify(e) if e.window == self.window => {
                // Window managers say where it went on the screen with what they send.
                if e.response_type & 0x80 != 0 {
                    let at = (e.x, e.y);
                    if self.window_at.replace(at).is_some_and(|was| was != at) {
                        self.dismiss(&[PopupKind::Menu, PopupKind::Tooltip]);
                    }
                }
                let size = SizeI::new(e.width as i32, e.height as i32);
                if size != self.size {
                    self.size = size;
                    self.sync_after_frame = self.sync_asked.take();
                    self.announce_size();
                } else if let (Some(counter), Some(v)) = (self.sync_counter, self.sync_asked.take())
                {
                    // Nothing to draw for it.
                    set_counter(&self.conn, counter, v);
                }
            }
            Event::MapNotify(e) if e.window == self.window => {
                self.mapped = true;
                self.redraw_requested = true;
            }
            Event::UnmapNotify(e) if e.window == self.window => self.mapped = false,
            Event::FocusIn(e) if e.event == self.window => {
                if e.detail != xproto::NotifyDetail::POINTER {
                    self.focused = true;
                }
            }
            Event::FocusOut(e) if e.event == self.window => {
                let grab = e.mode == xproto::NotifyMode::GRAB;
                if e.detail != xproto::NotifyDetail::POINTER && !grab {
                    self.focused = false;
                    self.keys_down.clear();
                    // As on Chromium: another program took over, which closes menus.
                    if e.mode == xproto::NotifyMode::NORMAL {
                        self.dismiss(&[PopupKind::Menu]);
                    }
                }
            }
            Event::KeyPress(e) | Event::KeyRelease(e) => {
                self.last_time = e.time;
                match &mut self.ime {
                    // It sends back what it doesn't use.
                    Some(ime) if ime.takes_keys() => ime.forward(&e),
                    _ => {
                        let pressed = e.response_type & 0x7f == xproto::KEY_PRESS_EVENT;
                        self.key_from_server(e.detail, pressed);
                    }
                }
            }
            Event::XkbStateNotify(e) => {
                if let Some(kb) = &mut self.kb {
                    kb.update(
                        [
                            u16::from(e.base_mods) as u32,
                            u16::from(e.latched_mods) as u32,
                            u16::from(e.locked_mods) as u32,
                        ],
                        [
                            e.base_group as i32,
                            e.latched_group as i32,
                            u8::from(e.locked_group) as i32,
                        ],
                    );
                }
            }
            Event::XkbNewKeyboardNotify(_) | Event::XkbMapNotify(_) => {
                if let Some(kb) = &mut self.kb {
                    kb.reload(&self.conn);
                }
            }
            Event::ClientMessage(e) if e.format == 32 && self.xdnd_message(&e) => {}
            Event::ClientMessage(e) if e.type_ == self.atoms.WM_PROTOCOLS && e.format == 32 => {
                let data = e.data.as_data32();
                if data[0] == self.atoms.WM_DELETE_WINDOW {
                    self.events.push(WindowEvent::CloseRequested);
                } else if data[0] == self.atoms._NET_WM_PING {
                    let mut reply = e;
                    reply.window = self.root;
                    let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
                    let _ = self.conn.send_event(false, self.root, mask, reply);
                } else if data[0] == self.atoms._NET_WM_SYNC_REQUEST {
                    self.sync_asked = Some(data[2] as i64 | ((data[3] as i32 as i64) << 32));
                }
            }
            Event::SelectionNotify(e) => self.selection_notify(&e),
            Event::SelectionRequest(e) if e.owner == self.window => self.selection_request(&e),
            Event::SelectionClear(e) if e.owner == self.window => self.selection_clear(e.selection),
            Event::PropertyNotify(e) => {
                self.selection_property(e.window, e.atom, selection::deleted(e.state));
                if e.window == self.window && e.atom == self.atoms._NET_WM_STATE {
                    self.read_states();
                } else if (e.window == self.root && e.atom == u32::from(AtomEnum::RESOURCE_MANAGER))
                    || (Some(e.window) == self.xsettings
                        && e.atom == self.atoms._XSETTINGS_SETTINGS)
                {
                    self.rescale();
                }
            }
            Event::XinputMotion(e) if self.dragging() => {
                let (x, y) = (input::fp1616(e.root_x), input::fp1616(e.root_y));
                self.drag_motion(x as i16, y as i16, e.time);
            }
            Event::XinputButtonRelease(e) if self.dragging() => {
                self.last_time = e.time;
                self.drag_release(e.time);
            }
            Event::XinputMotion(e)
                if e.event == self.window
                    && self
                        .pointer_on_border(input::fp1616(e.event_x), input::fp1616(e.event_y)) => {}
            Event::XinputMotion(e) if self.origin_of(e.event).is_some() => {
                let position = self.position(e.event, e.event_x, e.event_y);
                self.pointer = position;
                if let Some(s) =
                    self.scroll
                        .motion(e.sourceid, &e.valuator_mask, &e.axisvalues, Instant::now())
                {
                    self.scrolled(position, s.dx, s.dy, s.source, e.time);
                }
                let modifiers = self.modifiers();
                self.events.push(WindowEvent::PointerMoved {
                    position,
                    modifiers,
                });
            }
            Event::XinputButtonPress(e) | Event::XinputButtonRelease(e)
                if self.origin_of(e.event).is_some() =>
            {
                let pressed = e.event_type == xinput::BUTTON_PRESS_EVENT;
                self.last_time = e.time;
                if e.event == self.window && self.border.is_some() {
                    if pressed {
                        let (x, y) = (input::fp1616(e.root_x), input::fp1616(e.root_y));
                        self.press_on_border(x, y, e.detail);
                    }
                    return;
                }
                let (x, y) = (input::fp1616(e.event_x), input::fp1616(e.event_y));
                if pressed && self.press_outside(e.event, x, y) {
                    self.dismiss(&[PopupKind::Menu]);
                    return;
                }
                let position = self.position(e.event, e.event_x, e.event_y);
                let emulated = e
                    .flags
                    .contains(xinput::PointerEventFlags::POINTER_EMULATED);
                match input::button(e.detail) {
                    Button::Pointer(button) => {
                        let modifiers = self.modifiers();
                        self.events.push(WindowEvent::PointerButton {
                            position,
                            button,
                            pressed,
                            modifiers,
                            time: e.time,
                        });
                    }
                    Button::Wheel { dx, dy } => {
                        if pressed && !emulated && !self.scroll.scrolls_smoothly(e.sourceid) {
                            let wheel = anti_webui_core::event::ScrollSource::Wheel;
                            self.scrolled(position, dx, dy, wheel, e.time);
                        }
                    }
                }
            }
            Event::XinputEnter(e) if self.origin_of(e.event).is_some() => {
                // Moving between our windows isn't leaving.
                if self.events.last() == Some(&WindowEvent::PointerLeft) {
                    self.events.pop();
                }
                self.scroll.forget_positions();
                self.border = None;
                self.pointer = self.position(e.event, e.event_x, e.event_y);
                self.pointer_window = e.event;
                let cursor = self.cursor;
                self.cursors.set(&*self.conn, e.event, cursor);
            }
            Event::XinputLeave(e) if self.origin_of(e.event).is_some() => {
                // A grab that keeps the pointer on us isn't leaving.
                if e.mode != xinput::NotifyMode::GRAB
                    && e.mode != xinput::NotifyMode::UNGRAB
                    && self.border.take().is_none()
                {
                    self.events.push(WindowEvent::PointerLeft);
                }
            }
            Event::XinputGesturePinchBegin(e) if self.origin_of(e.event).is_some() => {
                self.pinch_scale = 1.0;
                let position = self.position(e.event, e.event_x, e.event_y);
                self.pinch(position, 1.0, PinchPhase::Began);
            }
            Event::XinputGesturePinchUpdate(e) if self.origin_of(e.event).is_some() => {
                let now = input::fp1616(e.scale).max(0.01);
                let by = now / self.pinch_scale;
                self.pinch_scale = now;
                let position = self.position(e.event, e.event_x, e.event_y);
                self.pinch(position, by as f32, PinchPhase::Moved);
            }
            Event::XinputGesturePinchEnd(e) if self.origin_of(e.event).is_some() => {
                let cancelled = e
                    .flags
                    .contains(xinput::GesturePinchEventFlags::GESTURE_PINCH_CANCELLED);
                let phase = if cancelled {
                    PinchPhase::Cancelled
                } else {
                    PinchPhase::Ended
                };
                let position = self.position(e.event, e.event_x, e.event_y);
                self.pinch(position, 1.0, phase);
            }
            Event::XinputDeviceChanged(e) => {
                let touchpad = self.is_touchpad(e.sourceid);
                self.scroll.update(e.sourceid, &e.classes, touchpad);
            }
            Event::XinputHierarchy(_) => self.read_devices(),
            // Without XInput 2.
            Event::MotionNotify(e) if self.origin_of(e.event).is_some() => {
                let position = self.core_position(e.event, e.event_x, e.event_y);
                self.pointer = position;
                let modifiers = self.modifiers();
                self.events.push(WindowEvent::PointerMoved {
                    position,
                    modifiers,
                });
            }
            Event::ButtonPress(e) | Event::ButtonRelease(e)
                if self.origin_of(e.event).is_some() =>
            {
                let pressed = e.response_type & 0x7f == xproto::BUTTON_PRESS_EVENT;
                self.last_time = e.time;
                let position = self.core_position(e.event, e.event_x, e.event_y);
                let modifiers = self.modifiers();
                match input::button(e.detail as u32) {
                    Button::Pointer(button) => self.events.push(WindowEvent::PointerButton {
                        position,
                        button,
                        pressed,
                        modifiers,
                        time: e.time,
                    }),
                    Button::Wheel { dx, dy } if pressed => {
                        let wheel = anti_webui_core::event::ScrollSource::Wheel;
                        self.scrolled(position, dx, dy, wheel, e.time);
                    }
                    Button::Wheel { .. } => {}
                }
            }
            Event::LeaveNotify(e) if self.origin_of(e.event).is_some() => {
                self.events.push(WindowEvent::PointerLeft);
            }
            Event::PresentCompleteNotify(e) => {
                if e.window == self.window {
                    self.present.complete(&mut self.window_frames, &e);
                } else if let Some(p) = self.popups.values_mut().find(|p| p.window == e.window) {
                    self.present.complete(&mut p.frames, &e);
                } else if let Some(i) = self.dnd.icon.as_mut().filter(|i| i.window == e.window) {
                    self.present.complete(&mut i.frames, &e);
                }
            }
            Event::PresentIdleNotify(e) => self.present.idle(e.pixmap),
            _ => {}
        }
    }

    /// A position on one of our windows in window coordinates.
    fn position(&self, window: Window, x: i32, y: i32) -> PointF {
        let o = self.origin_of(window).unwrap_or_default();
        PointF::new(
            input::fp1616(x) as f32 + o.x as f32,
            input::fp1616(y) as f32 + o.y as f32,
        )
    }

    fn core_position(&self, window: Window, x: i16, y: i16) -> PointF {
        let o = self.origin_of(window).unwrap_or_default();
        PointF::new((x as i32 + o.x) as f32, (y as i32 + o.y) as f32)
    }

    fn modifiers(&self) -> anti_webui_core::event::Modifiers {
        self.kb.as_ref().map(|k| k.modifiers).unwrap_or_default()
    }

    fn key(&mut self, keycode: u8, pressed: bool, repeat: bool) {
        if let Some(ev) = self
            .kb
            .as_ref()
            .and_then(|kb| kb.key_event(keycode, pressed, repeat))
        {
            self.events.push(WindowEvent::Key(ev));
        }
    }

    fn scrolled(
        &mut self,
        position: PointF,
        dx: f32,
        dy: f32,
        source: anti_webui_core::event::ScrollSource,
        time: u32,
    ) {
        let modifiers = self.modifiers();
        self.events.push(WindowEvent::Scroll {
            position,
            delta_x: dx,
            delta_y: dy,
            modifiers,
            source,
            phase: ScrollPhase::Single,
            time,
        });
    }

    fn pinch(&mut self, position: PointF, scale: f32, phase: PinchPhase) {
        let modifiers = self.modifiers();
        self.events.push(WindowEvent::Pinch {
            position,
            scale,
            modifiers,
            phase,
        });
    }

    pub(crate) fn wm_supports(&self, atom: Atom) -> bool {
        self.wm_supported.contains(&atom)
    }

    fn read_states(&mut self) {
        let states = self.read_atoms(self.window, self.atoms._NET_WM_STATE);
        let maximized = crate::decor::maximized(
            &states,
            self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
            self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
        );
        if maximized != self.maximized {
            self.maximized = maximized;
            self.events.push(WindowEvent::Maximized(maximized));
        }
        let on = states.contains(&self.atoms._NET_WM_STATE_FULLSCREEN);
        if self.fullscreen != Some(on) {
            let was = self.fullscreen.replace(on);
            let _ = self.conn.change_property32(
                PropMode::REPLACE,
                self.window,
                self.atoms._NET_WM_BYPASS_COMPOSITOR,
                AtomEnum::CARDINAL,
                &[on as u32],
            );
            if was.is_some() || on {
                self.events.push(WindowEvent::Fullscreen(on));
            }
        }
    }

    /// Whether the core should hear of a change of focus.
    pub fn take_focus_change(&mut self) -> Option<bool> {
        (self.focused != self.focus_announced).then(|| {
            self.focus_announced = self.focused;
            self.focused
        })
    }

    pub fn frames(&mut self, surface: SurfaceId) -> Option<&mut Frames> {
        match surface {
            SurfaceId::Window => Some(&mut self.window_frames),
            SurfaceId::Popup(id) => self.popups.get_mut(&id).map(|p| &mut p.frames),
            SurfaceId::DragIcon => self.dnd.icon.as_mut().map(|i| &mut i.frames),
        }
    }

    pub fn window_of(&self, surface: SurfaceId) -> Option<(Window, u8)> {
        match surface {
            SurfaceId::Window => Some((self.window, self.depth)),
            SurfaceId::Popup(id) => self.popups.get(&id).map(|p| (p.window, p.depth)),
            SurfaceId::DragIcon => self.dnd.icon.as_ref().map(|i| (i.window, i.depth)),
        }
    }

    /// Whether frames allow a frame now. If not, the time when a popup stops holding it up, if
    /// that's all it is waiting for.
    pub fn frame_wait(&mut self, now: Instant) -> Result<(), Option<Instant>> {
        if self.window_frames.pending(now) {
            return Err(None);
        }
        let latest = self
            .popups
            .values_mut()
            .filter_map(|p| p.frames.pending(now).then(|| p.frames.oldest()).flatten())
            .map(|t| t + POPUP_FRAME_WAIT)
            .max();
        match latest {
            Some(t) if t > now => Err(Some(t)),
            _ => Ok(()),
        }
    }
}

fn create_counter(conn: &XCBConnection) -> Option<sync::Counter> {
    conn.extension_information(sync::X11_EXTENSION_NAME)
        .ok()
        .flatten()?;
    conn.sync_initialize(3, 1).ok()?.reply().ok()?;
    let counter = conn.generate_id().ok()?;
    conn.sync_create_counter(counter, sync::Int64 { hi: 0, lo: 0 })
        .ok()?;
    Some(counter)
}

fn set_counter(conn: &XCBConnection, counter: sync::Counter, value: i64) {
    let value = sync::Int64 {
        hi: (value >> 32) as i32,
        lo: value as u32,
    };
    let _ = conn.sync_set_counter(counter, value);
}
