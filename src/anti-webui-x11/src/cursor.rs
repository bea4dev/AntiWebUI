//! Pointer cursors from the Xcursor theme.

use anti_webui_core::platform::CursorIcon;
use std::collections::HashMap;
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::cursor::Handle;
use x11rb::protocol::xproto::{ConnectionExt as _, Cursor, Window};
use x11rb::resource_manager::Database;

/// Names to look for, the CSS one first and then older ones of themes that don't have it.
pub(crate) fn names(icon: CursorIcon) -> &'static [&'static str] {
    match icon {
        CursorIcon::Default => &["default", "left_ptr"],
        CursorIcon::Pointer => &["pointer", "hand2", "hand1"],
        CursorIcon::Text => &["text", "xterm"],
        CursorIcon::NotAllowed => &["not-allowed", "crossed_circle", "circle"],
        CursorIcon::Grab => &["grab", "openhand", "hand1"],
        CursorIcon::Grabbing => &["grabbing", "closedhand", "fleur"],
        CursorIcon::Move => &["move", "fleur"],
        CursorIcon::EwResize => &["ew-resize", "sb_h_double_arrow", "h_double_arrow"],
        CursorIcon::NsResize => &["ns-resize", "sb_v_double_arrow", "v_double_arrow"],
        CursorIcon::Crosshair => &["crosshair", "cross"],
        CursorIcon::Copy => &["copy", "dnd-copy"],
        CursorIcon::None => &[],
    }
}

/// Cursors made so far, which stay until the connection closes.
pub(crate) struct Cursors {
    handle: Option<Handle>,
    made: HashMap<CursorIcon, Cursor>,
}

impl Cursors {
    pub fn new(conn: &impl Connection, screen: usize, db: &Database) -> Self {
        let handle = Handle::new(conn, screen, db)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        Self {
            handle,
            made: HashMap::new(),
        }
    }

    /// Shows `icon` over `window`.
    pub fn set(&mut self, conn: &impl Connection, window: Window, icon: CursorIcon) {
        let cursor = match self.made.get(&icon) {
            Some(&c) => c,
            None => {
                let c = self.make(conn, window, icon);
                self.made.insert(icon, c);
                c
            }
        };
        let aux = x11rb::protocol::xproto::ChangeWindowAttributesAux::new().cursor(cursor);
        let _ = conn.change_window_attributes(window, &aux);
    }

    fn make(&self, conn: &impl Connection, window: Window, icon: CursorIcon) -> Cursor {
        if icon == CursorIcon::None {
            return invisible(conn, window).unwrap_or(NONE);
        }
        let Some(handle) = &self.handle else {
            return NONE;
        };
        names(icon)
            .iter()
            .find_map(|name| handle.load_cursor(conn, name).ok().filter(|&c| c != NONE))
            .unwrap_or(NONE)
    }
}

/// A cursor of one clear pixel.
fn invisible(conn: &impl Connection, window: Window) -> Option<Cursor> {
    let pixmap = conn.generate_id().ok()?;
    conn.create_pixmap(1, pixmap, window, 1, 1).ok()?;
    let cursor = conn.generate_id().ok()?;
    conn.create_cursor(cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0)
        .ok()?;
    conn.free_pixmap(pixmap).ok()?;
    Some(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_names_come_first_and_every_shown_icon_has_some() {
        for icon in [
            CursorIcon::Default,
            CursorIcon::Pointer,
            CursorIcon::Text,
            CursorIcon::NotAllowed,
            CursorIcon::Grab,
            CursorIcon::Grabbing,
            CursorIcon::Move,
            CursorIcon::EwResize,
            CursorIcon::NsResize,
            CursorIcon::Crosshair,
            CursorIcon::Copy,
        ] {
            assert!(names(icon).len() >= 2, "{icon:?} has a fallback");
        }
        assert_eq!(names(CursorIcon::Pointer)[0], "pointer");
    }
}
