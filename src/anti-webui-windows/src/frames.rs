//! What the thread of a window keeps of what is drawn when: the size, the requests for frames,
//! and the presentations waiting for the vertical blank (see "提示" in
//! `docs/design/39-platform-windows.md`).

use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::platform::{PopupId, Presentation};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a presentation waits for the vertical blank before frames go on without it.
const NO_TICK_WAIT: Duration = Duration::from_millis(100);

pub(crate) struct Frames {
    pub size: SizeI,
    pub scale: Scale,
    pub redraw_requested: bool,
    pub redraw_at: Option<Instant>,
    /// The last presentation the vertical blank hasn't come after yet.
    presented_at: Option<Instant>,
    /// The vertical blanks seen so far, by the ticker's count.
    seen: u64,
    /// When the vertical blank last came after a presentation.
    pub last_frame: Option<Instant>,
    presentation_requests: Vec<u64>,
    pub presentations: Vec<Presentation>,
    /// The popups, with what each is attached to.
    popups: HashMap<PopupId, Option<PopupId>>,
    /// Events made here rather than by the window procedure.
    pub events: Vec<WindowEvent>,
}

impl Frames {
    pub fn new(seen: u64) -> Self {
        Self {
            size: SizeI::new(0, 0),
            scale: Scale::ONE,
            redraw_requested: true,
            redraw_at: None,
            presented_at: None,
            seen,
            last_frame: None,
            presentation_requests: Vec::new(),
            presentations: Vec::new(),
            popups: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Takes in what the window's events say about it, and adds those made here.
    pub fn take_in(&mut self, mut events: Vec<WindowEvent>) -> Vec<WindowEvent> {
        for e in &events {
            match e {
                WindowEvent::Resized { size, scale } => {
                    self.size = *size;
                    self.scale = *scale;
                    self.redraw_requested = true;
                }
                WindowEvent::PopupConfigured { .. } => self.redraw_requested = true,
                _ => {}
            }
        }
        events.append(&mut self.events);
        events
    }

    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    /// Whether the last presentation is still waiting for the vertical blank.
    pub fn frame_pending(&self, now: Instant) -> bool {
        self.presented_at.is_some_and(|t| now < t + NO_TICK_WAIT)
    }

    /// When a frame waiting for the vertical blank goes on without it.
    pub fn frame_deadline(&self) -> Option<Instant> {
        self.presented_at.map(|t| t + NO_TICK_WAIT)
    }

    /// Whether a vertical blank came that this hasn't taken in.
    pub fn has_tick(&self, seq: u64) -> bool {
        seq != self.seen && self.presented_at.is_some()
    }

    /// Takes in the vertical blank, if one came: the frame presented before it is on the screen.
    /// Whether it ended a wait for one.
    pub fn take_tick(
        &mut self,
        (seq, shown): (u64, Option<anti_webui_core::platform::Shown>),
    ) -> bool {
        if seq == self.seen {
            return false;
        }
        self.seen = seq;
        if self.presented_at.take().is_none() {
            return false;
        }
        self.last_frame = Some(Instant::now());
        for token in self.presentation_requests.drain(..) {
            self.presentations.push(Presentation { token, shown });
        }
        true
    }

    pub fn request_presentation(&mut self, token: u64) {
        self.presentation_requests.push(token);
    }

    /// A frame went to the window.
    pub fn presented(&mut self) {
        self.presented_at = Some(Instant::now());
    }

    // --- Popups ---------------------------------------------------------------------------------

    /// A new popup. `false` if it was there.
    pub fn add_popup(&mut self, id: PopupId, parent: Option<PopupId>) -> bool {
        self.popups.insert(id, parent).is_none()
    }

    pub fn has_popups(&self) -> bool {
        !self.popups.is_empty()
    }

    pub fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
        self.popups
            .iter()
            .filter(|(_, parent)| **parent == Some(id))
            .map(|(child, _)| *child)
            .collect()
    }

    pub fn remove_popup(&mut self, id: PopupId) {
        for child in self.popups_attached_to(id) {
            self.remove_popup(child);
        }
        self.popups.remove(&id);
    }
}
