//! Platform-independent input events and the events delivered to node handlers.

use crate::geometry::{PointF, RectI, Scale, SizeI};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    /// The platform's primary shortcut modifier (Ctrl on Linux and Windows).
    pub fn primary(&self) -> bool {
        self.ctrl
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
    Other(u32),
}

/// A logical key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Named(NamedKey),
    /// The character produced by the key ignoring Ctrl/Alt, e.g. `"a"` or `"A"`.
    Character(String),
    Unidentified,
}

impl Key {
    /// Case-insensitive comparison with a character key.
    pub fn is_char(&self, c: char) -> bool {
        match self {
            Key::Character(s) => {
                let mut chars = s.chars();
                matches!((chars.next(), chars.next()), (Some(k), None) if k.eq_ignore_ascii_case(&c))
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    Escape,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Shift,
    Control,
    Alt,
    Super,
    CapsLock,
    F(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    /// Text this key press would insert, if any (already excludes control characters).
    pub text: Option<String>,
    pub modifiers: Modifiers,
    pub pressed: bool,
    pub repeat: bool,
}

/// Input-method state changes, delivered atomically (text-input-v3 `done`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImeUpdate {
    /// New pre-edit text and the cursor range inside it (byte offsets). `None` clears it.
    pub preedit: Option<(String, Option<(usize, usize)>)>,
    /// Text to commit at the cursor.
    pub commit: Option<String>,
    /// Bytes to delete before and after the cursor (excluding the selection).
    pub delete_surrounding: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipboardKind {
    /// The regular clipboard (Ctrl+C / Ctrl+V).
    Clipboard,
    /// The primary selection (select to copy, middle-click to paste).
    Primary,
}

/// Logical pixels the content moves per unit of a Wayland touchpad axis. Chosen by feel on real
/// hardware: GTK's and Chromium's newer 2.5 felt slow, and 6 too fast.
pub const AXIS_MOTION_GAIN: f32 = 5.0;

/// What produced a scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollSource {
    /// A wheel with notches. Deltas from the platform are in notches.
    Wheel,
    /// Fingers on a touch surface. Deltas are in physical pixels.
    Touchpad,
    /// Anything else. Deltas are in physical pixels.
    Unknown,
}

/// Where a scroll is in a gesture (see `docs/design/08-scrolling-and-settings.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollPhase {
    /// Input without a beginning or an end, like a wheel.
    Single,
    Began,
    Moved,
    /// The fingers left. With `fling` the platform has no inertia of its own and the UI computes
    /// it; otherwise `Momentum` events follow from the platform.
    Lifted {
        fling: bool,
    },
    /// The fingers rest on the device, which stops inertia. Not delivered to handlers.
    Held,
    /// Motion from inertia.
    Momentum,
    MomentumEnded,
}

/// Events sent by the platform layer to a window. Coordinates are physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowEvent {
    /// The buffer size or the scale changed.
    Resized {
        size: SizeI,
        scale: Scale,
    },
    PointerMoved {
        position: PointF,
        modifiers: Modifiers,
    },
    PointerLeft,
    PointerButton {
        position: PointF,
        button: PointerButton,
        pressed: bool,
        modifiers: Modifiers,
        /// Milliseconds, monotonic, platform-defined origin.
        time: u32,
    },
    /// Positive deltas move the content up / left. See [`ScrollSource`] for their unit.
    Scroll {
        position: PointF,
        delta_x: f32,
        delta_y: f32,
        modifiers: Modifiers,
        source: ScrollSource,
        phase: ScrollPhase,
        /// Milliseconds, monotonic, platform-defined origin.
        time: u32,
    },
    Key(KeyEvent),
    Ime(ImeUpdate),
    /// Text requested with `PlatformRequest::Paste` arrived.
    Paste {
        kind: ClipboardKind,
        text: String,
    },
    Focused(bool),
    CloseRequested,
}

/// Information passed to pointer handlers.
#[derive(Debug)]
pub struct PointerEvent {
    pub position: PointF,
    /// Position relative to the handling node's border box.
    pub local: PointF,
    /// The handling node's border box in window coordinates.
    pub rect: RectI,
    pub button: Option<PointerButton>,
    pub modifiers: Modifiers,
    /// 1 for single click, 2 for double click, ...
    pub click_count: u32,
    pub(crate) stopped: Cell<bool>,
}

impl PointerEvent {
    pub fn stop_propagation(&self) {
        self.stopped.set(true);
    }
}

/// Information passed to keyboard handlers.
#[derive(Debug)]
pub struct KeyboardEvent {
    pub key: Key,
    pub text: Option<String>,
    pub modifiers: Modifiers,
    pub repeat: bool,
    pub(crate) stopped: Cell<bool>,
    pub(crate) default_prevented: Cell<bool>,
}

impl KeyboardEvent {
    pub fn stop_propagation(&self) {
        self.stopped.set(true);
    }

    /// Prevents built-in behavior (text editing, focus navigation) for this key.
    pub fn prevent_default(&self) {
        self.default_prevented.set(true);
    }
}

#[derive(Debug)]
pub struct ScrollEvent {
    /// In physical pixels, with the scroll settings applied.
    pub delta_x: f32,
    pub delta_y: f32,
    pub modifiers: Modifiers,
    pub source: ScrollSource,
    pub phase: ScrollPhase,
    pub(crate) stopped: Cell<bool>,
}

impl ScrollEvent {
    pub fn stop_propagation(&self) {
        self.stopped.set(true);
    }
}
