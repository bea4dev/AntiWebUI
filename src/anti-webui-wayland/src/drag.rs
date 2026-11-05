//! Drag and drop with `wl_data_device`. See `docs/design/23-drag-and-drop.md`.

use crate::popup::SurfaceFrame;
use crate::state::{State, SurfaceId};
use anti_webui_core::event::{DragAction, DragActions, DragResult, WindowEvent};
use anti_webui_core::geometry::{PointI, SizeI};
use anti_webui_core::platform::DragOffer;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, mpsc};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::{wl_data_offer, wl_data_source, wl_surface};
use wayland_client::{Connection, Proxy};
use wayland_protocols::wp::viewporter::client::wp_viewport;

/// `wl_data_source.set_actions` and the rest of it came in this version.
const ACTIONS_VERSION: u32 = 3;
/// `wl_surface.offset` came in this version.
const OFFSET_VERSION: u32 = 5;

pub(crate) struct DragIcon {
    pub surface: wl_surface::WlSurface,
    viewport: Option<wp_viewport::WpViewport>,
    pub frame: SurfaceFrame,
    size: SizeI,
    hotspot: PointI,
}

pub(crate) struct DragState {
    /// The last press of a button: what `start_drag` needs, which keys don't change.
    pub button_serial: u32,
    /// What this program offers while it drags something.
    source: Option<(wl_data_source::WlDataSource, DragOffer)>,
    pub icon: Option<DragIcon>,
    pub source_action: DndAction,

    /// What is over one of our surfaces.
    offer: Option<wl_data_offer::WlDataOffer>,
    serial: u32,
    surface: SurfaceId,
    position: (f64, f64),
    pub offer_action: DndAction,
    /// It was dropped; the offer stays until it has been dealt with.
    dropped: bool,

    received_tx: mpsc::Sender<(String, Vec<u8>)>,
    received_rx: mpsc::Receiver<(String, Vec<u8>)>,
}

impl Default for DragState {
    fn default() -> Self {
        let (received_tx, received_rx) = mpsc::channel();
        Self {
            button_serial: 0,
            source: None,
            icon: None,
            source_action: DndAction::empty(),
            offer: None,
            serial: 0,
            surface: SurfaceId::Window,
            position: (0.0, 0.0),
            offer_action: DndAction::empty(),
            dropped: false,
            received_tx,
            received_rx,
        }
    }
}

fn wire_actions(actions: DragActions) -> DndAction {
    match actions {
        DragActions::Move => DndAction::Move,
        DragActions::Copy => DndAction::Copy,
        DragActions::Both => DndAction::Move | DndAction::Copy,
    }
}

fn wire_action(action: DragAction) -> DndAction {
    match action {
        DragAction::Move => DndAction::Move,
        DragAction::Copy => DndAction::Copy,
    }
}

fn action_of(action: DndAction) -> DragAction {
    if action.contains(DndAction::Copy) {
        DragAction::Copy
    } else {
        DragAction::Move
    }
}

impl State {
    pub(crate) fn supports_drag(&self) -> bool {
        self.data_device.is_some()
            && self
                .data_device_manager
                .as_ref()
                .is_some_and(|m| m.version() >= ACTIONS_VERSION)
    }

    pub(crate) fn start_drag(&mut self, offer: DragOffer) {
        let (Some(mgr), Some(dev)) = (&self.data_device_manager, &self.data_device) else {
            self.events
                .push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
            return;
        };
        let Some(origin) = self.wl_surface(self.pointer_surface).cloned() else {
            self.events
                .push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
            return;
        };
        let source = mgr.create_data_source(&self.qh, ());
        let mut seen = Vec::new();
        for (mime, _) in &offer.items {
            if !seen.contains(mime) {
                source.offer(mime.clone());
                seen.push(mime.clone());
            }
        }
        source.set_actions(wire_actions(offer.actions));
        let surface = self
            .compositor
            .create_surface(&self.qh, SurfaceId::DragIcon);
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, &self.qh, ()));
        if viewport.is_none() {
            surface.set_buffer_scale((self.scale.0 / 120).max(1) as i32);
        }
        dev.start_drag(
            Some(&source),
            &origin,
            Some(&surface),
            self.drag.button_serial,
        );
        self.drag.source_action = DndAction::empty();
        self.drag.source = Some((source, offer));
        self.drag.icon = Some(DragIcon {
            surface,
            viewport,
            frame: SurfaceFrame::default(),
            size: SizeI::new(0, 0),
            hotspot: PointI::new(0, 0),
        });
    }

    pub(crate) fn cancel_drag(&mut self) {
        if let Some((source, _)) = self.drag.source.take() {
            source.destroy();
            self.events
                .push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
        }
    }

    pub(crate) fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI) {
        let Some(icon) = &mut self.drag.icon else {
            return;
        };
        if icon.size != logical {
            icon.size = logical;
            if let Some(v) = &icon.viewport {
                v.set_destination(logical.width.max(1), logical.height.max(1));
            }
        }
        if icon.hotspot != hotspot {
            // Relative to where it was, and it starts with its corner on the pointer.
            let (dx, dy) = (icon.hotspot.x - hotspot.x, icon.hotspot.y - hotspot.y);
            icon.hotspot = hotspot;
            if icon.surface.version() >= OFFSET_VERSION {
                icon.surface.offset(dx, dy);
            }
        }
    }

    pub(crate) fn close_drag_icon(&mut self) {
        if let Some(icon) = self.drag.icon.take() {
            if let Some(v) = icon.viewport {
                v.destroy();
            }
            icon.surface.destroy();
        }
    }

    /// Someone wants what is dragged in a format.
    pub(crate) fn drag_data_wanted(
        &mut self,
        source: &wl_data_source::WlDataSource,
        mime: &str,
        fd: OwnedFd,
    ) {
        let data: Option<Arc<[u8]>> = self
            .drag
            .source
            .as_ref()
            .filter(|(s, _)| s == source)
            .and_then(|(_, offer)| offer.items.iter().find(|(m, _)| m == mime))
            .map(|(_, data)| data.clone());
        if let Some(data) = data {
            std::thread::spawn(move || {
                let _ = std::fs::File::from(fd).write_all(&data);
            });
        }
    }

    /// The source is over, unless it is another one.
    pub(crate) fn drag_source_cancelled(&mut self, source: &wl_data_source::WlDataSource) {
        if self.drag.source.as_ref().is_some_and(|(s, _)| s == source) {
            self.drag.source = None;
            self.events
                .push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
        }
    }

    pub(crate) fn drag_source_finished(&mut self, source: &wl_data_source::WlDataSource) {
        if self.drag.source.as_ref().is_some_and(|(s, _)| s == source) {
            self.drag.source = None;
            let result = match action_of(self.drag.source_action) {
                DragAction::Copy => DragResult::Copied,
                DragAction::Move => DragResult::Moved,
            };
            self.events.push(WindowEvent::DragSourceEnded(result));
        }
    }

    fn forget_offer(&mut self) {
        if let Some(offer) = self.drag.offer.take() {
            self.offers.remove(&offer.id());
            offer.destroy();
        }
        self.drag.dropped = false;
        self.drag.offer_action = DndAction::empty();
    }

    pub(crate) fn drag_entered(
        &mut self,
        serial: u32,
        surface: SurfaceId,
        x: f64,
        y: f64,
        offer: Option<wl_data_offer::WlDataOffer>,
    ) {
        // What was dropped may still be read when the next one comes.
        if !self.drag.dropped {
            self.forget_offer();
        }
        self.drag.serial = serial;
        self.drag.surface = surface;
        self.drag.position = (x, y);
        let mimes = offer
            .as_ref()
            .and_then(|o| self.offers.get(&o.id()).cloned())
            .unwrap_or_default();
        self.drag.offer = offer;
        self.events.push(WindowEvent::DragEntered {
            position: self.physical_on(surface, x, y),
            mimes,
            modifiers: self.kb.modifiers,
        });
    }

    pub(crate) fn drag_moved(&mut self, x: f64, y: f64) {
        self.drag.position = (x, y);
        self.events.push(WindowEvent::DragMoved {
            position: self.physical_on(self.drag.surface, x, y),
            modifiers: self.kb.modifiers,
        });
    }

    pub(crate) fn drag_left(&mut self) {
        // Some compositors say this after a drop too, which isn't the end of it for us.
        if self.drag.dropped {
            return;
        }
        self.forget_offer();
        self.events.push(WindowEvent::DragLeft);
    }

    pub(crate) fn drag_dropped(&mut self) {
        self.drag.dropped = true;
        let (x, y) = self.drag.position;
        self.events.push(WindowEvent::DragDropped {
            position: self.physical_on(self.drag.surface, x, y),
            action: action_of(self.drag.offer_action),
        });
    }

    pub(crate) fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
        let Some(offer) = &self.drag.offer else {
            return;
        };
        let serial = self.drag.serial;
        match wanted {
            Some((mime, action)) => {
                offer.accept(serial, Some(mime));
                if offer.version() >= ACTIONS_VERSION {
                    offer.set_actions(DndAction::Move | DndAction::Copy, wire_action(action));
                }
            }
            None => {
                offer.accept(serial, None);
                if offer.version() >= ACTIONS_VERSION {
                    offer.set_actions(DndAction::empty(), DndAction::empty());
                }
            }
        }
    }

    pub(crate) fn receive_drop(&mut self, conn: &Connection, mime: String) {
        let send_empty = |tx: &mpsc::Sender<(String, Vec<u8>)>, mime: String| {
            let _ = tx.send((mime, Vec::new()));
        };
        let tx = self.drag.received_tx.clone();
        let wake = self.wake.try_clone().ok();
        let (Some(offer), Ok((read, write))) = (
            &self.drag.offer,
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC),
        ) else {
            send_empty(&tx, mime);
            if let Some(w) = wake {
                let _ = rustix::io::write(&w, &[1]);
            }
            return;
        };
        offer.receive(mime.clone(), write.as_fd());
        let _ = conn.flush();
        drop(write);
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if std::fs::File::from(read).read_to_end(&mut bytes).is_err() {
                bytes.clear();
            }
            let _ = tx.send((mime, bytes));
            if let Some(w) = wake {
                let _ = rustix::io::write(&w, &[1]);
            }
        });
    }

    /// Passes on what was read of what was dropped, and ends that.
    pub(crate) fn take_dropped_data(&mut self) {
        while let Ok((mime, bytes)) = self.drag.received_rx.try_recv() {
            self.events.push(WindowEvent::DropData { mime, bytes });
            self.finish_drop();
        }
    }

    pub(crate) fn finish_drop(&mut self) {
        if let Some(offer) = &self.drag.offer
            && self.drag.dropped
            && offer.version() >= ACTIONS_VERSION
        {
            offer.finish();
        }
        self.forget_offer();
    }
}
