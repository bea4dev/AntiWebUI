//! Platform-independent input events and the events delivered to node handlers.

use crate::geometry::{PointF, PointI, RectI, Scale, SizeI};
use crate::platform::PopupId;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        logo: false,
    };
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Self::NONE
    };
    const ALT: Modifiers = Modifiers {
        alt: true,
        ..Self::NONE
    };
    const LOGO: Modifiers = Modifiers {
        logo: true,
        ..Self::NONE
    };

    /// The modifier of shortcuts alone, as [`Modifiers::primary`] takes it: for tests and input
    /// made by programs.
    pub const PRIMARY: Modifiers = if cfg!(target_os = "macos") {
        Self::LOGO
    } else {
        Self::CTRL
    };

    /// The modifier of moving by words alone, as [`Modifiers::word`] takes it.
    pub const WORD: Modifiers = if cfg!(target_os = "macos") {
        Self::ALT
    } else {
        Self::CTRL
    };

    /// The modifier that makes drags copy alone, as [`Modifiers::copies_drag`] takes it.
    pub const COPY_DRAG: Modifiers = Self::WORD;

    /// These with Shift as well.
    pub const fn with_shift(self) -> Modifiers {
        Modifiers {
            shift: true,
            ..self
        }
    }

    /// The platform's primary shortcut modifier (Ctrl on Linux and Windows, Command on macOS).
    pub fn primary(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.logo
        } else {
            self.ctrl
        }
    }

    /// What moves and deletes by words (Ctrl, or Option on macOS).
    pub fn word(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.alt
        } else {
            self.ctrl
        }
    }

    /// What moves and deletes to the ends of lines and of the text with the arrows (Command on
    /// macOS; nothing elsewhere, where Home and End do).
    pub fn line(&self) -> bool {
        cfg!(target_os = "macos") && self.logo
    }

    /// Whether a key pressed with these types its text: not with the modifiers of shortcuts. Option
    /// types characters on macOS.
    pub fn types_text(&self) -> bool {
        if cfg!(target_os = "macos") {
            !self.ctrl && !self.logo
        } else {
            !self.ctrl && !self.alt
        }
    }

    /// What makes a drag copy instead of move (Ctrl, or Option on macOS).
    pub fn copies_drag(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.alt
        } else {
            self.ctrl
        }
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
#[doc(hidden)]
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
#[doc(hidden)]
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

/// Where a pinch is in a gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinchPhase {
    Began,
    Moved,
    Ended,
    /// The system took it for something else: what it did so far should be taken back, if that
    /// matters.
    Cancelled,
}

/// What is done with what is dragged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DragAction {
    #[default]
    Move,
    Copy,
}

/// What may be done with what is dragged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DragActions {
    Move,
    Copy,
    #[default]
    Both,
}

/// What became of what was dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DragResult {
    Moved,
    Copied,
    Cancelled,
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
    /// Fingers moved apart or together on a touch surface. `scale` is how many times bigger it got
    /// since the last one.
    Pinch {
        position: PointF,
        scale: f32,
        modifiers: Modifiers,
        phase: PinchPhase,
    },
    Key(KeyEvent),
    Ime(ImeUpdate),
    /// Text requested with `PlatformRequest::Paste` arrived.
    Paste {
        kind: ClipboardKind,
        text: String,
    },
    Focused(bool),
    /// The window went on all of the screen or came off it, for whatever reason.
    Fullscreen(bool),
    /// The window came to fill the screen but for panels, or stopped.
    Maximized(bool),
    /// Who draws the frame of the window, as agreed with the system.
    Decorations(crate::window::Decorations),
    CloseRequested,
    /// The platform placed a popup. `origin` is its surface's origin in window coordinates, the
    /// coordinates of all pointer events; `size` the logical size granted to its box.
    PopupConfigured {
        id: PopupId,
        origin: PointI,
        size: SizeI,
    },
    /// The platform closed a popup, because of something happening outside of it.
    PopupDismissed(PopupId),
    /// Something being dragged came over the window: from here, or from another program.
    DragEntered {
        position: PointF,
        mimes: Vec<String>,
        modifiers: Modifiers,
    },
    DragMoved {
        position: PointF,
        modifiers: Modifiers,
    },
    DragLeft,
    DragDropped {
        position: PointF,
        action: DragAction,
    },
    /// What `PlatformRequest::ReceiveDrop` asked for. Empty if it couldn't be read.
    DropData {
        mime: String,
        bytes: Vec<u8>,
    },
    /// What `PlatformRequest::StartDrag` started is over.
    DragSourceEnded(DragResult),
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

    /// Whether a handler before this one has prevented the built-in behavior.
    pub fn is_default_prevented(&self) -> bool {
        self.default_prevented.get()
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

/// Fingers moved apart or together. See [`Div::on_pinch`](crate::view::Div::on_pinch).
#[derive(Debug)]
pub struct PinchEvent {
    /// How many times bigger it got since the last one, which multiplies up over the gesture.
    pub scale: f32,
    pub phase: PinchPhase,
    /// Where the pointer is. It stays there during a gesture.
    pub position: PointF,
    /// The same relative to the handling node's border box.
    pub local: PointF,
    /// The handling node's border box in window coordinates.
    pub rect: RectI,
    pub modifiers: Modifiers,
    pub(crate) stopped: Cell<bool>,
}

impl PinchEvent {
    pub fn stop_propagation(&self) {
        self.stopped.set(true);
    }
}
