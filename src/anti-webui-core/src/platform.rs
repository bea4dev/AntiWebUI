//! Requests from the UI to the platform layer.

use crate::event::ClipboardKind;
use crate::geometry::RectI;
pub use anti_webui_style::CursorIcon;

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
    Close,
}
