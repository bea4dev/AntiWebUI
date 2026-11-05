//! Showing a document one page at a time on all of the screen: what comes in from the keyboard and
//! the pointer. See "プレゼンテーションモード" in `docs/design/26-pdf.md`.

use crate::PdfViewer;
use crate::plan::Direction;
use crate::reading::Reader;
use anti_webui_core::event::{Key, KeyboardEvent, NamedKey, PointerButton, PointerEvent};
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// How long the pointer stays when it doesn't move, as in PDF.js.
const POINTER_STAYS: Duration = Duration::from_secs(3);

/// See the top of this file. A [`crate::PdfView`] has one.
pub(crate) struct Slides {
    viewer: PdfViewer,
    reader: Rc<Reader>,
    direction: Rc<Cell<Direction>>,
    /// What shows instead of the page, after `B` and `W`.
    pub blank: Signal<Option<Color>>,
    pub pointer_hidden: Signal<bool>,
    hide_pointer: Cell<Option<Timeout>>,
    /// The number being typed, to go to that page.
    typed: Cell<Option<usize>>,
    /// How far a wheel or fingers have come towards the next page.
    scrolled: Cell<f32>,
    /// These fingers have turned a page, which is all they do until they lift.
    swiped: Cell<bool>,
}

impl Slides {
    pub fn new(viewer: PdfViewer, reader: Rc<Reader>, direction: Rc<Cell<Direction>>) -> Rc<Self> {
        Rc::new(Self {
            viewer,
            reader,
            direction,
            blank: signal(None),
            pointer_hidden: signal(false),
            hide_pointer: Cell::new(None),
            typed: Cell::new(None),
            scrolled: Cell::new(0.0),
            swiped: Cell::new(false),
        })
    }

    fn presenting(&self) -> bool {
        self.viewer.presenting.get_untracked()
    }

    pub fn started(self: &Rc<Self>) {
        self.keep_pointer();
    }

    pub fn stopped(&self) {
        if let Some(timer) = self.hide_pointer.take() {
            timer.cancel();
        }
        self.pointer_hidden.set_if_changed(false);
        self.blank.set_if_changed(None);
        self.typed.set(None);
        self.scrolled.set(0.0);
    }

    fn turn_to(&self, page: usize) {
        let count = self.viewer.count.get_untracked();
        if count == 0 {
            return;
        }
        let page = page.min(count - 1);
        let now = self.viewer.slide.get_untracked();
        if page != now {
            self.direction.set(if page > now {
                Direction::Down
            } else {
                Direction::Up
            });
            self.viewer.slide.set(page);
        }
    }

    fn next(&self) {
        self.turn_to(self.viewer.slide.get_untracked() + 1);
    }

    fn previous(&self) {
        self.turn_to(self.viewer.slide.get_untracked().saturating_sub(1));
    }

    /// Takes a key while presenting, and says whether it is presenting. Escape goes on to end it.
    pub fn key(&self, e: &KeyboardEvent) -> bool {
        if !self.presenting() {
            return false;
        }
        if e.modifiers.ctrl || e.modifiers.alt || matches!(e.key, Key::Named(NamedKey::Escape)) {
            return true;
        }
        let typed = self.typed.take();
        if self.blank.get_untracked().is_some() {
            self.blank.set(None);
        } else {
            match &e.key {
                Key::Named(
                    NamedKey::ArrowRight
                    | NamedKey::ArrowDown
                    | NamedKey::PageDown
                    | NamedKey::Space,
                ) => self.next(),
                Key::Named(NamedKey::Enter) => match typed {
                    Some(number) => self.turn_to(number.saturating_sub(1)),
                    None => self.next(),
                },
                Key::Named(
                    NamedKey::ArrowLeft
                    | NamedKey::ArrowUp
                    | NamedKey::PageUp
                    | NamedKey::Backspace,
                ) => self.previous(),
                Key::Named(NamedKey::Home) => self.turn_to(0),
                Key::Named(NamedKey::End) => self.turn_to(usize::MAX),
                Key::Character(c) => match c.to_ascii_lowercase().as_str() {
                    "n" => self.next(),
                    "p" => self.previous(),
                    "b" | "." => self.blank.set(Some(Color::BLACK)),
                    "w" | "," => self.blank.set(Some(Color::WHITE)),
                    c => match c.parse::<usize>() {
                        Ok(digit) if digit < 10 => {
                            let number = typed.unwrap_or(0).saturating_mul(10);
                            self.typed.set(Some(number.saturating_add(digit)));
                        }
                        _ => return true,
                    },
                },
                _ => return true,
            }
        }
        e.stop_propagation();
        e.prevent_default();
        true
    }

    /// Shows the pointer, and hides it again if it stays where it is.
    fn keep_pointer(self: &Rc<Self>) {
        self.pointer_hidden.set_if_changed(false);
        if let Some(timer) = self.hide_pointer.take() {
            timer.cancel();
        }
        let hidden = self.pointer_hidden;
        self.hide_pointer
            .set(Some(set_timeout(POINTER_STAYS, move || {
                hidden.set(true);
            })));
    }

    pub fn pointer_moved(self: &Rc<Self>, e: &PointerEvent) {
        self.keep_pointer();
        let cursor = if self.reader.has_link_at(e.local) {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        };
        self.reader.cursor.set_if_changed(cursor);
    }

    pub fn pointer_down(self: &Rc<Self>, e: &PointerEvent) {
        self.keep_pointer();
        if self.blank.get_untracked().is_some() {
            self.blank.set(None);
            return;
        }
        match e.button {
            Some(PointerButton::Primary) if e.modifiers.shift => self.previous(),
            Some(PointerButton::Primary) => {
                if !self.reader.follow_link(e.local) {
                    self.next();
                }
            }
            Some(PointerButton::Secondary) => self.previous(),
            _ => {}
        }
    }

    pub fn scroll(&self, e: &ScrollEvent) {
        e.stop_propagation();
        match e.phase {
            ScrollPhase::Began => {
                self.scrolled.set(0.0);
                self.swiped.set(false);
            }
            // Whatever keeps going after the fingers are gone isn't asking for anything.
            ScrollPhase::Single | ScrollPhase::Moved => {}
            _ => return,
        }
        if self.swiped.get() && e.phase != ScrollPhase::Single {
            return;
        }
        let delta = if e.delta_y.abs() >= e.delta_x.abs() {
            e.delta_y
        } else {
            e.delta_x
        };
        // Turning around starts over.
        let before = self.scrolled.get();
        let scrolled = if before * delta < 0.0 {
            delta
        } else {
            before + delta
        };
        // A notch of a wheel is enough, and half of one for a wheel with finer notches.
        let enough = settings().scroll.wheel_step * untrack(window_scale).as_f32() / 2.0;
        if scrolled.abs() < enough {
            self.scrolled.set(scrolled);
            return;
        }
        self.scrolled.set(0.0);
        self.swiped.set(e.phase != ScrollPhase::Single);
        if self.blank.get_untracked().is_some() {
            return;
        }
        if scrolled > 0.0 {
            self.next();
        } else {
            self.previous();
        }
    }
}
