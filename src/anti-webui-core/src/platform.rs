//! Requests from the UI to the platform layer.

use crate::event::{ClipboardKind, DragAction, DragActions};
use crate::geometry::{PointI, RectI, SizeI};
pub use anti_webui_style::CursorIcon;
use std::sync::Arc;

/// A file descriptor, for what is about dmabufs and their synchronization.
#[doc(hidden)]
#[cfg(unix)]
pub use std::os::fd::BorrowedFd;

/// Where there are no file descriptors, like Windows: nothing can make one, so that what is about
/// dmabufs reads the same on every OS and is never called.
#[doc(hidden)]
#[cfg(not(unix))]
#[derive(Clone, Copy, Debug)]
pub struct BorrowedFd<'a> {
    never: std::convert::Infallible,
    _fd: std::marker::PhantomData<&'a ()>,
}

#[cfg(not(unix))]
impl BorrowedFd<'_> {
    pub fn never(self) -> ! {
        match self.never {}
    }
}

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
    /// The system moves the window with the pointer, from the press going on.
    StartMove,
    /// The system resizes the window with the pointer from an edge, from the press going on.
    StartResize(crate::window::ResizeEdge),
    Minimize,
    /// Answered with `WindowEvent::Maximized`, if it happens.
    SetMaximized(bool),
    /// The system's menu for the window, where the pointer is.
    ShowWindowMenu,
    /// For a layer surface of a desktop shell.
    Layer(crate::layer::LayerChange),
}

// ---------------------------------------------------------------------------------------------
// Placing popups.

/// The box of a popup in window coordinates, given the anchor, the size it would like and the
/// area it has to stay in, all physical, for platforms that place popups themselves. Like `xdg_positioner`: the other side if it doesn't fit,
/// then along the edge, and smaller as a last resort.
#[doc(hidden)]
pub fn place_popup(
    placement: Placement,
    anchor: RectI,
    gap: i32,
    size: SizeI,
    area: RectI,
) -> RectI {
    let (w, h) = (size.width.max(1), size.height.max(1));
    let center_x = anchor.x + (anchor.width - w) / 2;
    let below = anchor.bottom() + gap;
    let above = anchor.y - gap - h;
    let (x, y) = match placement {
        Placement::BelowStart => (anchor.x, below),
        Placement::BelowEnd => (anchor.right() - w, below),
        Placement::BelowCenter => (center_x, below),
        Placement::AboveStart => (anchor.x, above),
        Placement::AboveEnd => (anchor.right() - w, above),
        Placement::AboveCenter => (center_x, above),
        Placement::RightStart => (anchor.right() + gap, anchor.y),
        Placement::LeftStart => (anchor.x - gap - w, anchor.y),
    };
    let mut r = RectI::new(x, y, w, h);
    let sideways = matches!(placement, Placement::RightStart | Placement::LeftStart);
    if sideways {
        let flipped = match placement {
            Placement::RightStart => anchor.x - gap - w,
            _ => anchor.right() + gap,
        };
        if (r.x < area.x || r.right() > area.right())
            && flipped >= area.x
            && flipped + w <= area.right()
        {
            r.x = flipped;
        }
        r.y = slide(r.y, h, area.y, area.bottom());
        if r.bottom() > area.bottom() {
            r.height = (area.bottom() - r.y).max(1);
        }
    } else {
        let below_room = area.bottom() - below;
        let above_room = anchor.y - gap - area.y;
        let goes_below = matches!(
            placement,
            Placement::BelowStart | Placement::BelowEnd | Placement::BelowCenter
        );
        let (room, other_room) = if goes_below {
            (below_room, above_room)
        } else {
            (above_room, below_room)
        };
        let flip = room < h && other_room > room;
        let on_below = goes_below != flip;
        let fit = if on_below { below_room } else { above_room };
        r.height = h.min(fit.max(1));
        r.y = if on_below {
            below
        } else {
            anchor.y - gap - r.height
        };
        r.x = slide(r.x, w, area.x, area.right());
    }
    r
}

/// Moves `[at, at + len)` into `[lo, hi)` if it can.
fn slide(at: i32, len: i32, lo: i32, hi: i32) -> i32 {
    if at + len > hi {
        (hi - len).max(lo)
    } else if at < lo {
        lo
    } else {
        at
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: RectI = RectI {
        x: 0,
        y: 0,
        width: 1000,
        height: 800,
    };

    #[test]
    fn below_the_anchor_with_the_gap() {
        let anchor = RectI::new(100, 100, 80, 30);
        let r = place_popup(Placement::BelowStart, anchor, 4, SizeI::new(200, 300), AREA);
        assert_eq!(r, RectI::new(100, 134, 200, 300));
        let r = place_popup(Placement::BelowEnd, anchor, 4, SizeI::new(200, 300), AREA);
        assert_eq!(r.x, 0, "would start left of the area");
        let r = place_popup(Placement::AboveCenter, anchor, 4, SizeI::new(40, 20), AREA);
        assert_eq!(r, RectI::new(120, 76, 40, 20));
    }

    #[test]
    fn flips_when_there_is_more_room_on_the_other_side() {
        let anchor = RectI::new(100, 700, 80, 30);
        let r = place_popup(Placement::BelowStart, anchor, 4, SizeI::new(200, 300), AREA);
        assert_eq!(r, RectI::new(100, 700 - 4 - 300, 200, 300));
    }

    #[test]
    fn slides_along_the_edge_and_shrinks_as_a_last_resort() {
        let anchor = RectI::new(950, 100, 40, 30);
        let r = place_popup(
            Placement::BelowStart,
            anchor,
            0,
            SizeI::new(200, 2000),
            AREA,
        );
        assert_eq!(r.x, 800, "slid in from the right");
        assert_eq!((r.y, r.height), (130, 670), "the longer side, made to fit");
    }

    #[test]
    fn submenus_go_to_the_other_side_at_the_edge() {
        let anchor = RectI::new(850, 200, 100, 30);
        let r = place_popup(Placement::RightStart, anchor, 2, SizeI::new(120, 100), AREA);
        assert_eq!(r, RectI::new(850 - 2 - 120, 200, 120, 100));
        let anchor = RectI::new(300, 750, 100, 30);
        let r = place_popup(Placement::RightStart, anchor, 2, SizeI::new(120, 100), AREA);
        assert_eq!(r.y, 700, "slid up");
    }
}
