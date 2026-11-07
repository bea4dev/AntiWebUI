//! Drag and drop with XDND version 5 (see "ドラッグ＆ドロップ" in
//! `docs/design/35-platform-x11.md`): taking drops from others, handing drags to them, and drags
//! over our own window, which go to the core without the protocol.

use crate::present::Frames;
use crate::state::State;
use anti_webui_core::event::{DragAction, DragActions, DragResult, WindowEvent};
use anti_webui_core::geometry::{PointF, PointI, SizeI};
use anti_webui_core::platform::DragOffer;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::shape;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _,
    CreateWindowAux, EventMask, GrabMode, GrabStatus, PropMode, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

/// The version we speak.
pub(crate) const VERSION: u32 = 5;
/// How long a drop waits for the other side to finish.
const FINISH_WAIT: Duration = Duration::from_secs(10);

/// What is dragged over our window by another program.
struct Incoming {
    source: Window,
    version: u32,
    mimes: Vec<String>,
    atoms: Vec<Atom>,
    /// Where it is in window coordinates; `None` before the first position.
    position: Option<PointF>,
    /// What the core said it would take, and how.
    accepted: Option<(String, DragAction)>,
    /// A position wasn't answered yet.
    status_owed: bool,
    dropped: Option<u32>,
}

/// Where what we drag is.
#[derive(Clone, Copy, PartialEq)]
enum Over {
    Nothing,
    /// One of our own windows.
    Us,
    /// Another program's window that takes XDND, with the version it speaks.
    Them(Window, u32),
}

/// What we drag.
struct Outgoing {
    offer: DragOffer,
    atoms: Vec<Atom>,
    over: Over,
    /// What the window under it said: whether it takes it, and how.
    accepted: Option<DragAction>,
    /// A position is on its way and no status came back yet; the latest one waits here.
    waiting: bool,
    pending: Option<(i16, i16, u32)>,
    /// Over our window: what the core said it takes.
    ours: Option<DragAction>,
    dropped_at: Option<Instant>,
    root_at: (i16, i16),
}

pub(crate) struct DragIcon {
    pub window: Window,
    pub depth: u8,
    colormap: Option<u32>,
    pub frames: Frames,
    size: SizeI,
    hotspot: PointI,
}

#[derive(Default)]
pub(crate) struct Dnd {
    incoming: Option<Incoming>,
    outgoing: Option<Outgoing>,
    pub icon: Option<DragIcon>,
    names: HashMap<Atom, String>,
    grabbed: bool,
}

fn action_atom(state: &State, action: DragAction) -> Atom {
    match action {
        DragAction::Copy => state.atoms.XdndActionCopy,
        DragAction::Move => state.atoms.XdndActionMove,
    }
}

impl State {
    fn atom_name(&mut self, atom: Atom) -> String {
        if let Some(n) = self.dnd.names.get(&atom) {
            return n.clone();
        }
        let name = self
            .conn
            .get_atom_name(atom)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .unwrap_or_default();
        self.dnd.names.insert(atom, name.clone());
        name
    }

    fn atom_of(&mut self, name: &str) -> Atom {
        if let Some((&a, _)) = self.dnd.names.iter().find(|(_, n)| *n == name) {
            return a;
        }
        let atom = self
            .conn
            .intern_atom(false, name.as_bytes())
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(NONE, |r| r.atom);
        self.dnd.names.insert(atom, name.to_owned());
        atom
    }

    fn action_of(&self, atom: Atom) -> DragAction {
        if atom == self.atoms.XdndActionMove {
            DragAction::Move
        } else {
            DragAction::Copy
        }
    }

    fn send_client_message(&self, to: Window, kind: Atom, data: [u32; 5]) {
        let event = ClientMessageEvent::new(32, to, kind, data);
        let _ = self.conn.send_event(false, to, EventMask::NO_EVENT, event);
    }

    /// Says on our window that it takes drops.
    pub(crate) fn announce_xdnd(&self) {
        let _ = self.conn.change_property32(
            PropMode::REPLACE,
            self.window,
            self.atoms.XdndAware,
            AtomEnum::ATOM,
            &[VERSION],
        );
    }

    fn to_window(&self, root_x: i16, root_y: i16) -> PointF {
        let at = self.window_on_screen();
        PointF::new((root_x as i32 - at.x) as f32, (root_y as i32 - at.y) as f32)
    }

    // ------------------------------------------------------------------------------------------
    // Taking drops from others.

    /// An XDND message to our window.
    pub(crate) fn xdnd_message(&mut self, e: &ClientMessageEvent) -> bool {
        let a = &self.atoms;
        let d = e.data.as_data32();
        let kind = e.type_;
        if kind == a.XdndEnter {
            let source = d[0];
            let version = d[1] >> 24;
            let atoms: Vec<Atom> = if d[1] & 1 != 0 {
                self.conn
                    .get_property(false, source, a.XdndTypeList, AtomEnum::ATOM, 0, 1024)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .and_then(|r| r.value32().map(|v| v.collect()))
                    .unwrap_or_default()
            } else {
                d[2..5].iter().copied().filter(|&t| t != NONE).collect()
            };
            let mimes = atoms.iter().map(|&t| self.atom_name(t)).collect();
            self.dnd.incoming = Some(Incoming {
                source,
                version,
                mimes,
                atoms,
                position: None,
                accepted: None,
                status_owed: false,
                dropped: None,
            });
        } else if kind == a.XdndPosition {
            let (x, y) = ((d[2] >> 16) as i16, d[2] as i16);
            let position = self.to_window(x, y);
            let modifiers = self.kb.as_ref().map(|k| k.modifiers).unwrap_or_default();
            let Some(inc) = &mut self.dnd.incoming else {
                return true;
            };
            let first = inc.position.replace(position).is_none();
            inc.status_owed = true;
            let event = if first {
                WindowEvent::DragEntered {
                    position,
                    mimes: inc.mimes.clone(),
                    modifiers,
                }
            } else {
                WindowEvent::DragMoved {
                    position,
                    modifiers,
                }
            };
            self.events.push(event);
        } else if kind == a.XdndLeave {
            if self
                .dnd
                .incoming
                .as_ref()
                .is_some_and(|i| i.dropped.is_none())
            {
                self.dnd.incoming = None;
                self.events.push(WindowEvent::DragLeft);
            }
        } else if kind == a.XdndDrop {
            let Some(inc) = &mut self.dnd.incoming else {
                return true;
            };
            inc.dropped = Some(d[2]);
            let position = inc.position.unwrap_or(PointF::new(0.0, 0.0));
            match inc.accepted.clone() {
                Some((_, action)) => self
                    .events
                    .push(WindowEvent::DragDropped { position, action }),
                None => {
                    self.finish_incoming(false);
                    self.events.push(WindowEvent::DragLeft);
                }
            }
        } else if kind == a.XdndStatus {
            self.status_came(d);
        } else if kind == a.XdndFinished {
            self.finished_came(d);
        } else {
            return false;
        }
        true
    }

    pub(crate) fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
        if let Some(out) = &mut self.dnd.outgoing
            && out.over == Over::Us
        {
            out.ours = wanted.map(|(_, action)| action);
            return;
        }
        if let Some(inc) = &mut self.dnd.incoming {
            let changed = inc.accepted != wanted;
            inc.accepted = wanted;
            if changed {
                inc.status_owed = true;
            }
        }
        self.answer_positions();
    }

    /// Answers the positions that came since the last time.
    pub(crate) fn answer_positions(&mut self) {
        let Some(inc) = &mut self.dnd.incoming else {
            return;
        };
        if !std::mem::take(&mut inc.status_owed) || inc.dropped.is_some() {
            return;
        }
        let (source, accepted) = (inc.source, inc.accepted.clone());
        let action = accepted
            .as_ref()
            .map_or(NONE, |(_, a)| action_atom(self, *a));
        // Bit 1: send positions even within the same rectangle.
        let flags = u32::from(accepted.is_some()) | 2;
        self.send_client_message(
            source,
            self.atoms.XdndStatus,
            [self.window, flags, 0, 0, action],
        );
    }

    pub(crate) fn receive_drop(&mut self, mime: String) {
        if let Some(out) = &self.dnd.outgoing
            && out.over == Over::Us
        {
            // Our own: the core has it already.
            self.events.push(WindowEvent::DropData {
                mime,
                bytes: Vec::new(),
            });
            return;
        }
        let Some(inc) = &self.dnd.incoming else {
            self.events.push(WindowEvent::DropData {
                mime,
                bytes: Vec::new(),
            });
            return;
        };
        let time = inc.dropped.unwrap_or(CURRENT_TIME);
        let target = inc
            .mimes
            .iter()
            .position(|m| *m == mime)
            .map(|i| inc.atoms[i]);
        match target {
            Some(t) => self.ask_for_drop(mime, t, time),
            None => {
                self.events.push(WindowEvent::DropData {
                    mime,
                    bytes: Vec::new(),
                });
                self.finish_incoming(false);
            }
        }
    }

    /// What was dropped was read: the other side may let go of it.
    pub(crate) fn drop_read(&mut self) {
        self.finish_incoming(true);
    }

    pub(crate) fn finish_drop(&mut self) {
        if let Some(out) = &self.dnd.outgoing
            && out.over == Over::Us
        {
            let result = match out.ours {
                Some(DragAction::Move) => DragResult::Moved,
                Some(DragAction::Copy) => DragResult::Copied,
                None => DragResult::Cancelled,
            };
            self.end_outgoing(result);
            return;
        }
        self.finish_incoming(true);
    }

    fn finish_incoming(&mut self, ok: bool) {
        let Some(inc) = self.dnd.incoming.take() else {
            return;
        };
        let accepted = ok && inc.accepted.is_some();
        let action = match (&inc.accepted, accepted) {
            (Some((_, a)), true) => action_atom(self, *a),
            _ => NONE,
        };
        let mut data = [self.window, u32::from(accepted), 0, 0, 0];
        if inc.version >= 5 {
            data[2] = action;
        }
        self.send_client_message(inc.source, self.atoms.XdndFinished, data);
    }

    // ------------------------------------------------------------------------------------------
    // Handing drags to others.

    pub(crate) fn supports_drag(&self) -> bool {
        self.xi_opcode.is_some()
    }

    pub(crate) fn start_drag(&mut self, offer: DragOffer) {
        if self.dnd.outgoing.is_some() {
            self.cancel_drag();
        }
        let mut atoms = Vec::new();
        for (mime, _) in &offer.items {
            let atom = self.atom_of(mime);
            if !atoms.contains(&atom) {
                atoms.push(atom);
            }
        }
        let _ = self.conn.change_property32(
            PropMode::REPLACE,
            self.window,
            self.atoms.XdndTypeList,
            AtomEnum::ATOM,
            &atoms,
        );
        let _ =
            self.conn
                .set_selection_owner(self.window, self.atoms.XdndSelection, self.last_time);
        let grabbed = self
            .conn
            .xinput_xi_grab_device(
                self.window,
                CURRENT_TIME,
                NONE,
                self.pointer_device,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                xinput::GrabOwner::NO_OWNER,
                &[u32::from(
                    xinput::XIEventMask::MOTION | xinput::XIEventMask::BUTTON_RELEASE,
                )],
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.status == GrabStatus::SUCCESS);
        self.dnd.grabbed = grabbed;
        self.dnd.outgoing = Some(Outgoing {
            offer,
            atoms,
            over: Over::Nothing,
            accepted: None,
            waiting: false,
            pending: None,
            ours: None,
            dropped_at: None,
            root_at: (0, 0),
        });
    }

    /// Whether a drag of ours is going on, which takes the pointer's motion and release.
    pub(crate) fn dragging(&self) -> bool {
        self.dnd
            .outgoing
            .as_ref()
            .is_some_and(|o| o.dropped_at.is_none())
    }

    /// The pointer moved during our drag, to `root` on the screen.
    pub(crate) fn drag_motion(&mut self, root_x: i16, root_y: i16, time: u32) {
        let over = self.window_under(root_x, root_y);
        let modifiers = self.kb.as_ref().map(|k| k.modifiers).unwrap_or_default();
        let position = self.to_window(root_x, root_y);
        self.move_icon(root_x, root_y);
        let Some(out) = &mut self.dnd.outgoing else {
            return;
        };
        out.root_at = (root_x, root_y);
        let was = out.over;
        if was != over {
            out.over = over;
            out.accepted = None;
            out.waiting = false;
            out.pending = None;
            out.ours = None;
            match was {
                Over::Them(w, _) => {
                    self.send_client_message(w, self.atoms.XdndLeave, [self.window, 0, 0, 0, 0])
                }
                Over::Us => self.events.push(WindowEvent::DragLeft),
                Over::Nothing => {}
            }
            match over {
                Over::Them(w, version) => self.send_enter(w, version),
                Over::Us => {
                    let mimes = self.outgoing_mimes();
                    self.events.push(WindowEvent::DragEntered {
                        position,
                        mimes,
                        modifiers,
                    });
                }
                Over::Nothing => {}
            }
        } else if over == Over::Us {
            self.events.push(WindowEvent::DragMoved {
                position,
                modifiers,
            });
        }
        if let Over::Them(w, _) = over {
            self.send_position(w, root_x, root_y, time);
        }
    }

    fn outgoing_mimes(&self) -> Vec<String> {
        let mut mimes: Vec<String> = Vec::new();
        if let Some(out) = &self.dnd.outgoing {
            for (m, _) in &out.offer.items {
                if !mimes.contains(m) {
                    mimes.push(m.clone());
                }
            }
        }
        mimes
    }

    fn send_enter(&self, to: Window, version: u32) {
        let Some(out) = &self.dnd.outgoing else {
            return;
        };
        let version = version.min(VERSION);
        let many = u32::from(out.atoms.len() > 3);
        let t = |i: usize| out.atoms.get(i).copied().unwrap_or(NONE);
        let data = [self.window, (version << 24) | many, t(0), t(1), t(2)];
        self.send_client_message(to, self.atoms.XdndEnter, data);
    }

    fn send_position(&mut self, to: Window, x: i16, y: i16, time: u32) {
        let Some(out) = &mut self.dnd.outgoing else {
            return;
        };
        if out.waiting {
            out.pending = Some((x, y, time));
            return;
        }
        out.waiting = true;
        let action = match out.offer.actions {
            DragActions::Move => self.atoms.XdndActionMove,
            DragActions::Copy | DragActions::Both => self.atoms.XdndActionCopy,
        };
        let coords = ((x as u16 as u32) << 16) | y as u16 as u32;
        self.send_client_message(
            to,
            self.atoms.XdndPosition,
            [self.window, 0, coords, time, action],
        );
    }

    fn status_came(&mut self, d: [u32; 5]) {
        let accepted = (d[1] & 1 != 0).then(|| self.action_of(d[4]));
        let Some(out) = &mut self.dnd.outgoing else {
            return;
        };
        if !matches!(out.over, Over::Them(w, _) if w == d[0]) {
            return;
        }
        out.accepted = accepted;
        out.waiting = false;
        if let (Some((x, y, time)), Over::Them(w, _)) = (out.pending.take(), out.over) {
            self.send_position(w, x, y, time);
        }
    }

    /// The button was let go during our drag.
    pub(crate) fn drag_release(&mut self, time: u32) {
        let Some(out) = &mut self.dnd.outgoing else {
            return;
        };
        match out.over {
            Over::Them(w, _) if out.accepted.is_some() => {
                out.dropped_at = Some(Instant::now());
                self.ungrab_drag();
                self.send_client_message(w, self.atoms.XdndDrop, [self.window, 0, time, 0, 0]);
            }
            Over::Them(w, _) => {
                self.send_client_message(w, self.atoms.XdndLeave, [self.window, 0, 0, 0, 0]);
                self.end_outgoing(DragResult::Cancelled);
            }
            Over::Us => {
                out.dropped_at = Some(Instant::now());
                let action = out.ours;
                let (x, y) = out.root_at;
                self.ungrab_drag();
                let position = self.to_window(x, y);
                match action {
                    Some(action) => self
                        .events
                        .push(WindowEvent::DragDropped { position, action }),
                    None => {
                        self.events.push(WindowEvent::DragLeft);
                        self.end_outgoing(DragResult::Cancelled);
                    }
                }
            }
            Over::Nothing => self.end_outgoing(DragResult::Cancelled),
        }
    }

    fn finished_came(&mut self, d: [u32; 5]) {
        let Some(out) = &self.dnd.outgoing else {
            return;
        };
        if !matches!(out.over, Over::Them(w, _) if w == d[0]) {
            return;
        }
        let result = if d[1] & 1 == 0 {
            DragResult::Cancelled
        } else {
            // Version 5 says what was done; older ones what the last status said.
            let action = if d[2] != NONE {
                self.action_of(d[2])
            } else {
                out.accepted.unwrap_or(DragAction::Copy)
            };
            match action {
                DragAction::Move => DragResult::Moved,
                DragAction::Copy => DragResult::Copied,
            }
        };
        self.end_outgoing(result);
    }

    /// Drops the other side never finished.
    pub(crate) fn give_up_drops(&mut self, now: Instant) {
        let late = self
            .dnd
            .outgoing
            .as_ref()
            .and_then(|o| o.dropped_at)
            .is_some_and(|t| now - t > FINISH_WAIT);
        if late {
            self.end_outgoing(DragResult::Cancelled);
        }
    }

    pub(crate) fn cancel_drag(&mut self) {
        let Some(out) = &self.dnd.outgoing else {
            return;
        };
        match out.over {
            Over::Them(w, _) => {
                self.send_client_message(w, self.atoms.XdndLeave, [self.window, 0, 0, 0, 0])
            }
            Over::Us => self.events.push(WindowEvent::DragLeft),
            Over::Nothing => {}
        }
        self.end_outgoing(DragResult::Cancelled);
    }

    fn end_outgoing(&mut self, result: DragResult) {
        if self.dnd.outgoing.take().is_some() {
            self.ungrab_drag();
            self.events.push(WindowEvent::DragSourceEnded(result));
        }
    }

    fn ungrab_drag(&mut self) {
        if std::mem::take(&mut self.dnd.grabbed) {
            let _ = self
                .conn
                .xinput_xi_ungrab_device(CURRENT_TIME, self.pointer_device);
        }
    }

    /// The data of what we drag, for whoever asks through `XdndSelection`.
    pub(crate) fn drag_data(&mut self, target: Atom) -> Option<std::sync::Arc<[u8]>> {
        let name = self.atom_name(target);
        let out = self.dnd.outgoing.as_ref()?;
        out.offer
            .items
            .iter()
            .find(|(m, _)| *m == name)
            .map(|(_, d)| d.clone())
    }

    pub(crate) fn drag_targets(&self) -> Vec<Atom> {
        self.dnd
            .outgoing
            .as_ref()
            .map_or(Vec::new(), |o| o.atoms.clone())
    }

    /// The top-level window under a point on the screen that takes XDND, if any.
    fn window_under(&mut self, x: i16, y: i16) -> Over {
        let icon = self.dnd.icon.as_ref().map(|i| i.window);
        let mut window = self.root;
        for _ in 0..32 {
            if window == self.window || self.popups.values().any(|p| p.window == window) {
                return Over::Us;
            }
            if window != self.root
                && let Some(version) = self.xdnd_version(window)
            {
                return Over::Them(window, version);
            }
            let Some(r) = self
                .conn
                .translate_coordinates(self.root, window, x, y)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                return Over::Nothing;
            };
            if r.child == NONE || Some(r.child) == icon {
                return Over::Nothing;
            }
            window = r.child;
        }
        Over::Nothing
    }

    /// The XDND version a window speaks, going through `XdndProxy` if it has one.
    fn xdnd_version(&self, window: Window) -> Option<u32> {
        let a = &self.atoms;
        let aware = self
            .conn
            .get_property(false, window, a.XdndAware, AtomEnum::ATOM, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        aware.value32()?.next().filter(|&v| v >= 3)
    }

    // ------------------------------------------------------------------------------------------
    // The picture that goes with the pointer.

    pub(crate) fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI) {
        let s = self.scale;
        let size = SizeI::new(
            s.px(logical.width as f32).max(1),
            s.px(logical.height as f32).max(1),
        );
        let hotspot = PointI::new(s.px(hotspot.x as f32), s.px(hotspot.y as f32));
        if self.dnd.icon.is_none() {
            let Some(icon) = self.create_icon(size) else {
                return;
            };
            self.dnd.icon = Some(icon);
        }
        let Some(icon) = &mut self.dnd.icon else {
            return;
        };
        let resized = icon.size != size;
        icon.size = size;
        icon.hotspot = hotspot;
        if resized {
            let aux = ConfigureWindowAux::new()
                .width(size.width as u32)
                .height(size.height as u32);
            let _ = self.conn.configure_window(icon.window, &aux);
        }
        if let Some(out) = &self.dnd.outgoing {
            let (x, y) = out.root_at;
            self.move_icon(x, y);
        }
    }

    fn create_icon(&mut self, size: SizeI) -> Option<DragIcon> {
        let conn = &*self.conn;
        let argb = self
            .composited()
            .then(|| crate::popup::argb_visual(conn, self.screen))
            .flatten();
        let (depth, visual) = match argb {
            Some(v) => (32, v),
            None => (self.depth, conn.setup().roots[self.screen].root_visual),
        };
        let window = conn.generate_id().ok()?;
        let mut aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .override_redirect(1);
        let colormap = if depth == 32 {
            let c = conn.generate_id().ok()?;
            conn.create_colormap(ColormapAlloc::NONE, c, self.root, visual)
                .ok()?;
            aux = aux.colormap(c);
            Some(c)
        } else {
            None
        };
        conn.create_window(
            depth,
            window,
            self.root,
            -10000,
            -10000,
            size.width as u16,
            size.height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &aux,
        )
        .ok()?;
        let _ = conn.change_property32(
            PropMode::REPLACE,
            window,
            self.atoms._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[self.atoms._NET_WM_WINDOW_TYPE_DND],
        );
        // Input goes through it, to what it is dragged over.
        if let Ok(region) = conn.generate_id()
            && conn.xfixes_create_region(region, &[]).is_ok()
        {
            let _ = conn.xfixes_set_window_shape_region(window, shape::SK::INPUT, 0, 0, region);
            let _ = conn.xfixes_destroy_region(region);
        }
        self.present.select(conn, window);
        let _ = conn.map_window(window);
        Some(DragIcon {
            window,
            depth,
            colormap,
            frames: Frames::default(),
            size,
            hotspot: PointI::default(),
        })
    }

    fn move_icon(&self, root_x: i16, root_y: i16) {
        if let Some(icon) = &self.dnd.icon {
            let aux = ConfigureWindowAux::new()
                .x(root_x as i32 - icon.hotspot.x)
                .y(root_y as i32 - icon.hotspot.y);
            let _ = self.conn.configure_window(icon.window, &aux);
        }
    }

    pub(crate) fn close_drag_icon(&mut self) {
        if let Some(icon) = self.dnd.icon.take() {
            let _ = self.conn.destroy_window(icon.window);
            if let Some(c) = icon.colormap {
                let _ = self.conn.free_colormap(c);
            }
        }
    }
}
