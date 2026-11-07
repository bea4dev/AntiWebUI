//! The frame of the window: the window manager's or the window's own (`_MOTIF_WM_HINTS`), its
//! states, moving and resizing through `_NET_WM_MOVERESIZE`, and the edges to resize a window with a
//! frame of its own from (see "窓の飾り" in `docs/design/36-multi-window.md`).

use crate::state::State;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::window::{Decorations, ResizeEdge};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xinput::ConnectionExt as _;
use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};

/// How far into the window its edges can be taken hold of, in logical pixels: a child window can't
/// stick out of it, so they are inside.
const BORDER: f64 = 5.0;
/// How far along an edge from a corner taking hold resizes from the corner.
const CORNER: f64 = 16.0;

/// `_NET_WM_MOVERESIZE` directions.
const MOVE: u32 = 8;

fn direction(edge: ResizeEdge) -> u32 {
    match edge {
        ResizeEdge::TopLeft => 0,
        ResizeEdge::Top => 1,
        ResizeEdge::TopRight => 2,
        ResizeEdge::Right => 3,
        ResizeEdge::BottomRight => 4,
        ResizeEdge::Bottom => 5,
        ResizeEdge::BottomLeft => 6,
        ResizeEdge::Left => 7,
    }
}

/// What `_NET_WM_STATE` says, for the core.
pub(crate) fn maximized(states: &[u32], vert: u32, horz: u32) -> bool {
    states.contains(&vert) && states.contains(&horz)
}

/// Which edge a point of a window of this size is on, if any.
pub(crate) fn edge_at(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Option<ResizeEdge> {
    let (border, corner) = (BORDER * scale, CORNER * scale);
    let (left, right) = (x < border, x >= width - border);
    let (top, bottom) = (y < border, y >= height - border);
    let (near_left, near_right) = (x < corner, x >= width - corner);
    let (near_top, near_bottom) = (y < corner, y >= height - corner);
    Some(match () {
        _ if (top && near_left) || (left && near_top) => ResizeEdge::TopLeft,
        _ if (top && near_right) || (right && near_top) => ResizeEdge::TopRight,
        _ if (bottom && near_left) || (left && near_bottom) => ResizeEdge::BottomLeft,
        _ if (bottom && near_right) || (right && near_bottom) => ResizeEdge::BottomRight,
        _ if top => ResizeEdge::Top,
        _ if bottom => ResizeEdge::Bottom,
        _ if left => ResizeEdge::Left,
        _ if right => ResizeEdge::Right,
        _ => return None,
    })
}

impl State {
    /// Whether the window needs edges of its own to resize it from.
    fn wants_borders(&self) -> bool {
        self.decorations == Decorations::Client && !self.maximized && self.fullscreen != Some(true)
    }

    /// The pointer moved on the window: whether it is on an edge, which the core doesn't hear of.
    pub(crate) fn pointer_on_border(&mut self, x: f64, y: f64) -> bool {
        let edge = self
            .wants_borders()
            .then(|| {
                let (w, h) = (self.size.width as f64, self.size.height as f64);
                edge_at(x, y, w, h, self.scale.as_f64())
            })
            .flatten();
        match edge {
            Some(edge) => {
                if self.border != Some(edge) {
                    if self.border.is_none() {
                        self.events.push(WindowEvent::PointerLeft);
                    }
                    self.border = Some(edge);
                    self.cursors.set_edge(&*self.conn, self.window, edge);
                }
                true
            }
            None => {
                if self.border.take().is_some() {
                    let cursor = self.cursor;
                    self.cursors.set(&*self.conn, self.window, cursor);
                }
                false
            }
        }
    }

    /// Hands the pointer to the window manager to move or resize the window from where it is.
    fn move_resize(&mut self, direction: u32, root_x: i32, root_y: i32, button: u32) {
        // The press grabbed the pointer for us; the window manager needs it.
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        if self.xi_opcode.is_some() {
            let _ = self
                .conn
                .xinput_xi_ungrab_device(CURRENT_TIME, self.pointer_device);
        }
        let event = ClientMessageEvent::new(
            32,
            self.window,
            self.atoms._NET_WM_MOVERESIZE,
            [root_x as u32, root_y as u32, direction, button, 1],
        );
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let _ = self.conn.send_event(false, self.root, mask, event);
        let _ = self.conn.flush();
    }

    /// Where the pointer is on the screen.
    fn pointer_on_screen(&self) -> Option<(i32, i32)> {
        let reply = self.conn.query_pointer(self.window).ok()?.reply().ok()?;
        Some((reply.root_x as i32, reply.root_y as i32))
    }

    /// A press on an edge of the window: the window manager resizes it from there.
    pub(crate) fn press_on_border(&mut self, root_x: f64, root_y: f64, button: u32) -> bool {
        let Some(edge) = self.border else {
            return false;
        };
        self.border = None;
        self.move_resize(direction(edge), root_x as i32, root_y as i32, button);
        true
    }

    pub(crate) fn start_move(&mut self) {
        if let Some((x, y)) = self.pointer_on_screen() {
            self.move_resize(MOVE, x, y, 1);
        }
    }

    pub(crate) fn start_resize(&mut self, edge: ResizeEdge) {
        if let Some((x, y)) = self.pointer_on_screen() {
            self.move_resize(direction(edge), x, y, 1);
        }
    }

    pub(crate) fn minimize(&mut self) {
        const ICONIC_STATE: u32 = 3;
        let event = ClientMessageEvent::new(
            32,
            self.window,
            self.atoms.WM_CHANGE_STATE,
            [ICONIC_STATE, 0, 0, 0, 0],
        );
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let _ = self.conn.send_event(false, self.root, mask, event);
    }

    pub(crate) fn set_maximized(&mut self, on: bool) {
        let a = &self.atoms;
        let event = ClientMessageEvent::new(
            32,
            self.window,
            a._NET_WM_STATE,
            [
                on as u32,
                a._NET_WM_STATE_MAXIMIZED_VERT,
                a._NET_WM_STATE_MAXIMIZED_HORZ,
                1,
                0,
            ],
        );
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let _ = self.conn.send_event(false, self.root, mask, event);
    }

    /// The window manager's menu, where the pointer is, if it has one: Mutter and KWin take
    /// GTK's message for it.
    pub(crate) fn show_window_menu(&mut self) {
        if !self.wm_supports(self.atoms._GTK_SHOW_WINDOW_MENU) {
            return;
        }
        let Some((x, y)) = self.pointer_on_screen() else {
            return;
        };
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        if self.xi_opcode.is_some() {
            let _ = self
                .conn
                .xinput_xi_ungrab_device(CURRENT_TIME, self.pointer_device);
        }
        let event = ClientMessageEvent::new(
            32,
            self.window,
            self.atoms._GTK_SHOW_WINDOW_MENU,
            [self.pointer_device as u32, x as u32, y as u32, 0, 0],
        );
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let _ = self.conn.send_event(false, self.root, mask, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_inside_and_corners_reach_along_them() {
        let at = |x, y| edge_at(x, y, 200.0, 100.0, 2.0);
        assert_eq!(at(100.0, 50.0), None);
        assert_eq!(at(100.0, 3.0), Some(ResizeEdge::Top));
        assert_eq!(at(199.0, 50.0), Some(ResizeEdge::Right));
        assert_eq!(at(100.0, 95.0), Some(ResizeEdge::Bottom));
        assert_eq!(at(1.0, 50.0), Some(ResizeEdge::Left));
        assert_eq!(at(20.0, 2.0), Some(ResizeEdge::TopLeft), "along the top");
        assert_eq!(at(2.0, 20.0), Some(ResizeEdge::TopLeft), "along the side");
        assert_eq!(at(198.0, 98.0), Some(ResizeEdge::BottomRight));
        assert_eq!(at(40.0, 2.0), Some(ResizeEdge::Top), "past the corner");
    }

    #[test]
    fn both_directions_make_it_maximized() {
        assert!(maximized(&[1, 2, 3], 2, 3));
        assert!(!maximized(&[1, 2], 2, 3));
    }
}
