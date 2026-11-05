//! Requests from the UI to the platform layer.

use crate::event::{ClipboardKind, DragAction, DragActions};
use crate::geometry::{PointI, RectI, SizeI};
pub use anti_webui_style::CursorIcon;
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
