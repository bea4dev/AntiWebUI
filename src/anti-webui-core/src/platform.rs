//! Requests from the UI to the platform layer.

use crate::event::{ClipboardKind, DragAction, DragActions};
use crate::geometry::{PointI, RectI, SizeI};
pub use anti_webui_style::CursorIcon;
use std::os::fd::BorrowedFd;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImePurpose {
    #[default]
    Normal,
    Password,
    Number,
    Email,
    Url,
}

/// The input-method state for the focused editable node.
#[derive(Clone, Debug, PartialEq)]
#[doc(hidden)]
pub struct ImeState {
    /// Text around the cursor (at most ~4000 bytes).
    pub surrounding: String,
    /// Cursor and anchor byte offsets inside `surrounding`.
    pub cursor: usize,
    pub anchor: usize,
    /// Caret rectangle in window physical pixels.
    pub cursor_rect: RectI,
    pub multiline: bool,
    pub purpose: ImePurpose,
}

/// Identifies a popup between the core and the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub struct PopupId(pub u64);

/// What a popup is for, which decides how it behaves (see
/// `docs/design/09-shadows-stacking-popups.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PopupKind {
    /// Takes the input: pressing anywhere else closes it. Pull-down lists and menus.
    #[default]
    Menu,
    /// Only shows something: input goes through it to what is below.
    Tooltip,
    /// Takes input where it is, stays until it is removed, and appears at once. The platform
    /// doesn't keep it on the screen; the core keeps it in the window.
    Bubble,
}

/// Where a popup goes relative to its anchor. "Start" and "end" are the anchor's left and right, or
/// top and bottom for the sides. The platform may flip it to keep it on the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Placement {
    #[default]
    BelowStart,
    BelowEnd,
    /// Below, with the middles lined up.
    BelowCenter,
    AboveStart,
    AboveEnd,
    /// Above, with the middles lined up.
    AboveCenter,
    RightStart,
    LeftStart,
}

/// Everything the platform needs to show a popup. Physical pixels unless it says logical.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupParams {
    pub id: PopupId,
    /// The popup it is attached to, or the window.
    pub parent: Option<PopupId>,
    pub kind: PopupKind,
    pub placement: Placement,
    /// In window coordinates, inside of the parent.
    pub anchor: RectI,
    /// Logical pixels between the anchor and the popup.
    pub gap: i32,
    /// The box it would like to have, in logical pixels. The platform may grant less.
    pub wanted: SizeI,
    /// The size of its buffers, the rounded product of `logical` and the scale.
    pub buffer: SizeI,
    /// The size of its surface, in logical pixels.
    pub logical: SizeI,
    /// Its box in the surface, in logical pixels; the rest is shadow.
    pub geometry: RectI,
    /// Where a [`PopupKind::Bubble`] goes: the surface's origin in logical window coordinates.
    pub position: Option<PointI>,
}

/// What a drag offers to whoever it is dragged to.
#[derive(Clone, Debug, PartialEq)]
#[doc(hidden)]
pub struct DragOffer {
    /// The formats, with the data of each.
    pub items: Vec<(String, Arc<[u8]>)>,
    pub actions: DragActions,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlatformRequest {
    SetCursor(CursorIcon),
    /// `None` disables the input method.
    SetIme(Option<ImeState>),
    SetClipboard {
        kind: ClipboardKind,
        text: String,
    },
    /// Ask for the clipboard contents; answered with `WindowEvent::Paste`.
    Paste(ClipboardKind),
    SetTitle(String),
    /// Puts the window on all of the screen or takes it off. `WindowEvent::Fullscreen` says when it
    /// has happened, if it does.
    SetFullscreen(bool),
    Close,
    /// Answered with `WindowEvent::PopupConfigured`.
    ShowPopup(PopupParams),
    UpdatePopup(PopupParams),
    /// Closes it and what is attached to it.
    ClosePopup(PopupId),
    /// Hands what is dragged to the system, with a surface for what shows with the pointer.
    /// Answered with `WindowEvent::DragEntered` and the others, and `DragSourceEnded`.
    StartDrag(DragOffer),
    /// Ends a drag that was started, which is then told as `DragSourceEnded`.
    CancelDrag,
    /// The surface of the drag is not needed any more.
    CloseDragIcon,
    /// The format what is over the window would be taken in here, if it can be.
    AcceptDrag(Option<(String, DragAction)>),
    /// Reads what was dropped; answered with `WindowEvent::DropData`.
    ReceiveDrop(String),
    /// What was dropped has been dealt with without reading it.
    FinishDrop,
}

// ---------------------------------------------------------------------------------------------
// What the app's presenter and the backends hand each other (see `docs/design/35-platform-x11.md`).

/// One of the surfaces that show something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub enum SurfaceId {
    Window,
    Popup(PopupId),
    /// What shows with the pointer during a drag.
    DragIcon,
}

/// What the compositor or X server can use: `zwp_linux_dmabuf_feedback_v1`, or DRI3's
/// `GetSupportedModifiers`.
#[derive(Clone, Debug, Default)]
#[doc(hidden)]
pub struct DmabufFeedback {
    /// `dev_t` of the device the compositor composites with.
    pub main_device: u64,
    /// In order of preference.
    pub tranches: Vec<DmabufTranche>,
    /// Increases whenever the compositor sends new feedback.
    pub generation: u64,
}

#[derive(Clone, Debug, Default)]
#[doc(hidden)]
pub struct DmabufTranche {
    pub device: u64,
    /// `(DRM fourcc, modifier)` pairs.
    pub formats: Vec<(u32, u64)>,
    /// Buffers in this tranche can be scanned out directly.
    pub scanout: bool,
}

impl DmabufFeedback {
    /// Modifiers for `format` on the main device, most preferred tranche first.
    pub fn modifiers(&self, format: u32) -> Vec<u64> {
        let mut out = Vec::new();
        for t in self
            .tranches
            .iter()
            .filter(|t| t.device == self.main_device)
        {
            for &(f, m) in &t.formats {
                if f == format && !out.contains(&m) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Modifiers for `format` that the compositor can put on the screen as they are, if that is what
    /// it would like most.
    pub fn scanout_modifiers(&self, format: u32) -> Vec<u64> {
        let first = self
            .tranches
            .iter()
            .find(|t| t.device == self.main_device && t.formats.iter().any(|&(f, _)| f == format));
        first.filter(|t| t.scanout).map_or(Vec::new(), |t| {
            let of_format = t.formats.iter().filter(|&&(f, _)| f == format);
            of_format.map(|&(_, m)| m).collect()
        })
    }
}

/// One plane of a dmabuf to wrap in a `wl_buffer` or a pixmap.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct DmabufPlaneDesc<'a> {
    pub fd: BorrowedFd<'a>,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub struct BufferId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub enum BufferEvent {
    /// The compositor no longer reads the buffer (implicit sync only).
    Released(BufferId),
    /// The compositor rejected the buffer.
    Failed(BufferId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub struct TimelineId(pub u64);

/// Explicit synchronization points for one commit.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct SyncPoints {
    pub acquire: (TimelineId, u64),
    pub release: (TimelineId, u64),
}

/// What the compositor says became of a commit (`wp_presentation`, or Present's
/// `CompleteNotify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct Presentation {
    /// What the platform's `request_presentation` was given.
    pub token: u64,
    /// `None` if the content never reached the screen.
    pub shown: Option<Shown>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct Shown {
    /// In the clock of the platform's `presentation_clock`.
    pub time_ns: i64,
    /// The time until the next refresh; 0 if the output has no constant rate.
    pub refresh_ns: u32,
    /// `wp_presentation_feedback.kind`.
    pub flags: u32,
}
