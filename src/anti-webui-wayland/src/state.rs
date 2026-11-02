//! Wayland protocol state and event dispatch.

use crate::keyboard::{Keyboard, Repeat};
use anti_webui_core::event::{ClipboardKind, ImeUpdate, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, ImePurpose, ImeState};
use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::mpsc;
use std::time::Instant;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::{
    wl_callback, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_surface,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{self, Shape},
    wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
    zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3,
    zwp_text_input_v3::{self, ContentHint, ContentPurpose},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

pub const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];

#[derive(Default)]
pub struct PendingIme {
    pub preedit: Option<(String, Option<(usize, usize)>)>,
    pub commit: Option<String>,
    pub delete: Option<(usize, usize)>,
}

#[derive(Default)]
pub struct PointerFrame {
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub wheel_x: i32,
    pub wheel_y: i32,
    pub has_scroll: bool,
}

pub struct State {
    pub qh: QueueHandle<State>,
    pub compositor: wl_compositor::WlCompositor,
    pub surface: wl_surface::WlSurface,
    #[allow(dead_code, reason = "the xdg_surface must outlive the toplevel")]
    pub xdg_surface: xdg_surface::XdgSurface,
    pub toplevel: xdg_toplevel::XdgToplevel,
    pub viewport: Option<wp_viewport::WpViewport>,
    pub fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    pub _decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    pub cursor_shape_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    pub text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    pub data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    pub primary_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,

    pub configured: bool,
    pub logical_size: (i32, i32),
    pending_size: Option<(i32, i32)>,
    pub scale: Scale,
    pub buffer_size: SizeI,
    pub size_announced: Option<(SizeI, Scale)>,
    pub closed: bool,

    pub pointer: Option<wl_pointer::WlPointer>,
    pub cursor_device: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    pub pointer_serial: u32,
    pub pointer_pos: (f64, f64),
    pub cursor: CursorIcon,
    pub frame: PointerFrame,
    pub keyboard: Option<wl_keyboard::WlKeyboard>,
    pub kb: Keyboard,
    pub input_serial: u32,

    pub text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    pub ti_entered: bool,
    pub ti_enabled: bool,
    pub ime_wanted: Option<ImeState>,
    pub pending_ime: PendingIme,

    pub data_device: Option<wl_data_device::WlDataDevice>,
    pub offers: HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    pub selection_offer: Option<wl_data_offer::WlDataOffer>,
    pub data_source: Option<(wl_data_source::WlDataSource, String)>,
    pub primary_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    pub primary_offers: HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    pub primary_offer: Option<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
    pub primary_source: Option<(
        zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        String,
    )>,
    pub paste_tx: mpsc::Sender<(ClipboardKind, String)>,
    pub wake: OwnedFd,

    /// A frame callback is outstanding for a committed frame.
    pub frame_pending: bool,
    /// A frame callback was requested but not committed yet.
    pub frame_callback_queued: bool,
    pub redraw_requested: bool,
    pub redraw_at: Option<Instant>,
    pub events: Vec<WindowEvent>,
    pub(crate) present: crate::present::PresentState,
}

impl State {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        qh: QueueHandle<State>,
        compositor: wl_compositor::WlCompositor,
        surface: wl_surface::WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        toplevel: xdg_toplevel::XdgToplevel,
        logical_size: (i32, i32),
        paste_tx: mpsc::Sender<(ClipboardKind, String)>,
        wake: OwnedFd,
    ) -> Self {
        Self {
            qh,
            compositor,
            surface,
            xdg_surface,
            toplevel,
            viewport: None,
            fractional: None,
            _decoration: None,
            cursor_shape_manager: None,
            text_input_manager: None,
            data_device_manager: None,
            primary_manager: None,
            configured: false,
            logical_size,
            pending_size: None,
            scale: Scale::ONE,
            buffer_size: SizeI::new(logical_size.0, logical_size.1),
            size_announced: None,
            closed: false,
            pointer: None,
            cursor_device: None,
            pointer_serial: 0,
            pointer_pos: (0.0, 0.0),
            cursor: CursorIcon::Default,
            frame: PointerFrame::default(),
            keyboard: None,
            kb: Keyboard::new(),
            input_serial: 0,
            text_input: None,
            ti_entered: false,
            ti_enabled: false,
            ime_wanted: None,
            pending_ime: PendingIme::default(),
            data_device: None,
            offers: HashMap::new(),
            selection_offer: None,
            data_source: None,
            primary_device: None,
            primary_offers: HashMap::new(),
            primary_offer: None,
            primary_source: None,
            paste_tx,
            wake,
            frame_pending: false,
            frame_callback_queued: false,
            redraw_requested: true,
            redraw_at: None,
            events: Vec::new(),
            present: Default::default(),
        }
    }

    fn physical(&self, x: f64, y: f64) -> PointF {
        let s = self.scale.as_f64();
        PointF::new((x * s) as f32, (y * s) as f32)
    }

    /// Recomputes the buffer size (R3) and announces size/scale changes.
    pub fn update_size(&mut self) {
        let (w, h) = self.logical_size;
        let n = self.scale.0 as f64 / 120.0;
        let size = SizeI::new((w as f64 * n).round() as i32, (h as f64 * n).round() as i32);
        self.buffer_size = size;
        if let Some(vp) = &self.viewport {
            vp.set_destination(w.max(1), h.max(1));
        } else {
            self.surface
                .set_buffer_scale((self.scale.0 / 120).max(1) as i32);
        }
        if self.size_announced != Some((size, self.scale)) {
            self.size_announced = Some((size, self.scale));
            self.events.push(WindowEvent::Resized {
                size,
                scale: self.scale,
            });
            self.redraw_requested = true;
        }
    }

    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.cursor = icon;
        self.apply_cursor();
    }

    fn apply_cursor(&self) {
        let Some(dev) = &self.cursor_device else {
            return;
        };
        let shape = match self.cursor {
            CursorIcon::Default => Shape::Default,
            CursorIcon::Pointer => Shape::Pointer,
            CursorIcon::Text => Shape::Text,
            CursorIcon::NotAllowed => Shape::NotAllowed,
            CursorIcon::Grab => Shape::Grab,
            CursorIcon::Grabbing => Shape::Grabbing,
            CursorIcon::Move => Shape::Move,
            CursorIcon::EwResize => Shape::EwResize,
            CursorIcon::NsResize => Shape::NsResize,
            CursorIcon::Crosshair => Shape::Crosshair,
        };
        dev.set_shape(self.pointer_serial, shape);
    }

    /// Sends the wanted IME state to the compositor if the text-input has focus.
    pub fn apply_ime(&mut self) {
        let Some(ti) = &self.text_input else { return };
        if !self.ti_entered {
            return;
        }
        match &self.ime_wanted {
            Some(state) => {
                if !self.ti_enabled {
                    ti.enable();
                    self.ti_enabled = true;
                }
                ti.set_surrounding_text(
                    state.surrounding.clone(),
                    state.cursor as i32,
                    state.anchor as i32,
                );
                ti.set_text_change_cause(zwp_text_input_v3::ChangeCause::Other);
                let purpose = match state.purpose {
                    ImePurpose::Normal => ContentPurpose::Normal,
                    ImePurpose::Password => ContentPurpose::Password,
                    ImePurpose::Number => ContentPurpose::Number,
                    ImePurpose::Email => ContentPurpose::Email,
                    ImePurpose::Url => ContentPurpose::Url,
                };
                let mut hint = ContentHint::empty();
                if state.multiline {
                    hint |= ContentHint::Multiline;
                }
                if state.purpose == ImePurpose::Password {
                    hint |= ContentHint::SensitiveData | ContentHint::HiddenText;
                }
                ti.set_content_type(hint, purpose);
                let r = state.cursor_rect;
                let s = self.scale;
                let x = s.to_logical(r.x as f32).floor() as i32;
                let y = s.to_logical(r.y as f32).floor() as i32;
                let w = (s.to_logical(r.right() as f32).ceil() as i32 - x).max(1);
                let h = (s.to_logical(r.bottom() as f32).ceil() as i32 - y).max(1);
                ti.set_cursor_rectangle(x, y, w, h);
                ti.commit();
            }
            None => {
                if self.ti_enabled {
                    ti.disable();
                    ti.commit();
                    self.ti_enabled = false;
                }
            }
        }
    }

    pub fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
        match kind {
            ClipboardKind::Clipboard => {
                let (Some(mgr), Some(dev)) = (&self.data_device_manager, &self.data_device) else {
                    return;
                };
                let source = mgr.create_data_source(&self.qh, ());
                for m in TEXT_MIMES {
                    source.offer(m.to_owned());
                }
                dev.set_selection(Some(&source), self.input_serial);
                if let Some((old, _)) = self.data_source.replace((source, text)) {
                    old.destroy();
                }
            }
            ClipboardKind::Primary => {
                let (Some(mgr), Some(dev)) = (&self.primary_manager, &self.primary_device) else {
                    return;
                };
                let source = mgr.create_source(&self.qh, ());
                for m in TEXT_MIMES {
                    source.offer(m.to_owned());
                }
                dev.set_selection(Some(&source), self.input_serial);
                if let Some((old, _)) = self.primary_source.replace((source, text)) {
                    old.destroy();
                }
            }
        }
    }

    /// Starts reading the clipboard; the result arrives through the paste channel.
    pub fn request_paste(&mut self, conn: &Connection, kind: ClipboardKind) {
        let own = match kind {
            ClipboardKind::Clipboard => self.data_source.as_ref().map(|(_, t)| t.clone()),
            ClipboardKind::Primary => self.primary_source.as_ref().map(|(_, t)| t.clone()),
        };
        if let Some(text) = own {
            let _ = self.paste_tx.send((kind, text));
            let _ = rustix::io::write(&self.wake, &[1]);
            return;
        }
        let Ok((read, write)) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC) else {
            return;
        };
        let received = match kind {
            ClipboardKind::Clipboard => self.selection_offer.as_ref().and_then(|offer| {
                let mimes = self.offers.get(&offer.id())?;
                let mime = TEXT_MIMES.iter().find(|m| mimes.iter().any(|x| x == *m))?;
                offer.receive(mime.to_string(), write.as_fd());
                Some(())
            }),
            ClipboardKind::Primary => self.primary_offer.as_ref().and_then(|offer| {
                let mimes = self.primary_offers.get(&offer.id())?;
                let mime = TEXT_MIMES.iter().find(|m| mimes.iter().any(|x| x == *m))?;
                offer.receive(mime.to_string(), write.as_fd());
                Some(())
            }),
        };
        if received.is_none() {
            return;
        }
        let _ = conn.flush();
        drop(write);
        let tx = self.paste_tx.clone();
        let wake = self.wake.try_clone().ok();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut file = std::fs::File::from(read);
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                let text = String::from_utf8_lossy(&buf).replace("\r\n", "\n");
                let _ = tx.send((kind, text));
                if let Some(w) = wake {
                    let _ = rustix::io::write(&w, &[1]);
                }
            }
        });
    }

    pub fn key_repeat_due(&mut self, now: Instant) -> Option<u32> {
        let r = self.kb.repeat.as_mut()?;
        if now < r.next {
            return None;
        }
        let interval = std::time::Duration::from_secs_f64(1.0 / self.kb.repeat_rate.max(1) as f64);
        r.next += interval;
        if r.next < now {
            r.next = now + interval;
        }
        Some(r.keycode)
    }
}

fn write_async(fd: OwnedFd, text: String) {
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(fd);
        let _ = file.write_all(text.as_bytes());
    });
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: ignore wp_viewport::WpViewport);
delegate_noop!(State: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(State: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(State: ignore zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1);
delegate_noop!(State: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(State: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(State: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);
delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(State: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if let Some(size) = state.pending_size.take() {
                state.logical_size = size;
            }
            state.configured = true;
            state.update_size();
            state.redraw_requested = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    state.pending_size = Some((width, height));
                }
            }
            xdg_toplevel::Event::Close => state.events.push(WindowEvent::CloseRequested),
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.scale = Scale(scale.max(1));
            if state.configured {
                state.update_size();
            }
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event
            && state.fractional.is_none()
        {
            state.scale = Scale(factor.max(1) as u32 * 120);
            if state.configured {
                state.update_size();
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_pending = false;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            if let Some(mgr) = &state.cursor_shape_manager {
                state.cursor_device = Some(mgr.get_pointer(&pointer, qh, ()));
            }
            state.pointer = Some(pointer);
        }
        if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
            if let Some(mgr) = &state.text_input_manager {
                state.text_input = Some(mgr.get_text_input(seat, qh, ()));
            }
        }
        if state.data_device.is_none()
            && let Some(mgr) = &state.data_device_manager
        {
            state.data_device = Some(mgr.get_data_device(seat, qh, ()));
        }
        if state.primary_device.is_none()
            && let Some(mgr) = &state.primary_manager
        {
            state.primary_device = Some(mgr.get_device(seat, qh, ()));
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let modifiers = state.kb.modifiers;
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_serial = serial;
                state.pointer_pos = (surface_x, surface_y);
                state.apply_cursor();
                let position = state.physical(surface_x, surface_y);
                state.events.push(WindowEvent::PointerMoved {
                    position,
                    modifiers,
                });
            }
            wl_pointer::Event::Leave { .. } => state.events.push(WindowEvent::PointerLeft),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_pos = (surface_x, surface_y);
                let position = state.physical(surface_x, surface_y);
                state.events.push(WindowEvent::PointerMoved {
                    position,
                    modifiers,
                });
            }
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: btn_state,
            } => {
                state.input_serial = serial;
                let button = match button {
                    0x110 => PointerButton::Primary,
                    0x111 => PointerButton::Secondary,
                    0x112 => PointerButton::Middle,
                    b => PointerButton::Other(b),
                };
                let pressed = matches!(btn_state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                let (x, y) = state.pointer_pos;
                let position = state.physical(x, y);
                state.events.push(WindowEvent::PointerButton {
                    position,
                    button,
                    pressed,
                    modifiers,
                    time,
                });
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                state.frame.has_scroll = true;
                match axis {
                    WEnum::Value(wl_pointer::Axis::VerticalScroll) => state.frame.scroll_y += value,
                    WEnum::Value(wl_pointer::Axis::HorizontalScroll) => {
                        state.frame.scroll_x += value
                    }
                    _ => {}
                }
            }
            wl_pointer::Event::AxisValue120 { axis, value120 } => {
                state.frame.has_scroll = true;
                match axis {
                    WEnum::Value(wl_pointer::Axis::VerticalScroll) => {
                        state.frame.wheel_y += value120
                    }
                    WEnum::Value(wl_pointer::Axis::HorizontalScroll) => {
                        state.frame.wheel_x += value120
                    }
                    _ => {}
                }
            }
            wl_pointer::Event::Frame => {
                let f = std::mem::take(&mut state.frame);
                if f.has_scroll {
                    // Wheels scroll three 16px lines per notch, like browsers; touchpads move
                    // the content with the fingers.
                    let line = 16.0 * 3.0;
                    let dx = if f.wheel_x != 0 {
                        f.wheel_x as f64 / 120.0 * line
                    } else {
                        f.scroll_x
                    };
                    let dy = if f.wheel_y != 0 {
                        f.wheel_y as f64 / 120.0 * line
                    } else {
                        f.scroll_y
                    };
                    let s = state.scale.as_f64();
                    let (x, y) = state.pointer_pos;
                    let position = state.physical(x, y);
                    state.events.push(WindowEvent::Scroll {
                        position,
                        delta_x: (dx * s) as f32,
                        delta_y: (dy * s) as f32,
                        modifiers,
                    });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { fd, size, .. } => state.kb.set_keymap(fd, size),
            wl_keyboard::Event::Enter { serial, .. } => {
                state.input_serial = serial;
                state.events.push(WindowEvent::Focused(true));
            }
            wl_keyboard::Event::Leave { .. } => {
                state.kb.repeat = None;
                state.events.push(WindowEvent::Focused(false));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state
                    .kb
                    .update_modifiers(mods_depressed, mods_latched, mods_locked, group);
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.kb.repeat_rate = rate.max(0) as u32;
                state.kb.repeat_delay = std::time::Duration::from_millis(delay.max(0) as u64);
            }
            wl_keyboard::Event::Key {
                serial,
                key,
                state: key_state,
                ..
            } => {
                state.input_serial = serial;
                let pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                if pressed {
                    if state.kb.repeat_rate > 0 && state.kb.repeats(key) {
                        state.kb.repeat = Some(Repeat {
                            keycode: key,
                            next: Instant::now() + state.kb.repeat_delay,
                        });
                    }
                } else if state.kb.repeat.as_ref().is_some_and(|r| r.keycode == key) {
                    state.kb.repeat = None;
                }
                if let Some(ev) = state.kb.key_event(key, pressed, false) {
                    state.events.push(WindowEvent::Key(ev));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { .. } => {
                state.ti_entered = true;
                state.ti_enabled = false;
                state.apply_ime();
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                state.ti_entered = false;
                state.ti_enabled = false;
            }
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                let cursor = (cursor_begin >= 0 && cursor_end >= 0)
                    .then_some((cursor_begin as usize, cursor_end as usize));
                state.pending_ime.preedit = text.map(|t| (t, cursor));
            }
            zwp_text_input_v3::Event::CommitString { text } => state.pending_ime.commit = text,
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                state.pending_ime.delete = Some((before_length as usize, after_length as usize));
            }
            zwp_text_input_v3::Event::Done { .. } => {
                let p = std::mem::take(&mut state.pending_ime);
                state.events.push(WindowEvent::Ime(ImeUpdate {
                    preedit: p.preedit,
                    commit: p.commit,
                    delete_surrounding: p.delete,
                }));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = state.selection_offer.take()
                    && Some(&old) != id.as_ref()
                {
                    state.offers.remove(&old.id());
                    old.destroy();
                }
                state.selection_offer = id;
            }
            _ => {}
        }
    }

    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                if let Some((s, text)) = &state.data_source
                    && s == source
                {
                    write_async(fd, text.clone());
                }
            }
            wl_data_source::Event::Cancelled => {
                if state.data_source.as_ref().is_some_and(|(s, _)| s == source) {
                    state.data_source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_device_v1::Event::DataOffer { offer } => {
                state.primary_offers.insert(offer.id(), Vec::new());
            }
            zwp_primary_selection_device_v1::Event::Selection { id } => {
                if let Some(old) = state.primary_offer.take()
                    && Some(&old) != id.as_ref()
                {
                    state.primary_offers.remove(&old.id());
                    old.destroy();
                }
                state.primary_offer = id;
            }
            _ => {}
        }
    }

    event_created_child!(State, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            state
                .primary_offers
                .entry(offer.id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { fd, .. } => {
                if let Some((s, text)) = &state.primary_source
                    && s == source
                {
                    write_async(fd, text.clone());
                }
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                if state
                    .primary_source
                    .as_ref()
                    .is_some_and(|(s, _)| s == source)
                {
                    state.primary_source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}
