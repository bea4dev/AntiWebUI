//! The colors of the widgets, and what every widget is made of.

use anti_webui_core::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Colors of the widgets. It is just a set of colors, so an app can make its own:
/// `Theme { accent: rgb(0x8250df), ..Theme::light() }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// What everything is on.
    pub page: Color,
    pub surface: Color,
    pub surface_muted: Color,
    /// What is above the page: menus and dialogs.
    pub overlay: Color,
    pub hover: Color,
    pub pressed: Color,
    /// The rows that are selected.
    pub selected: Color,
    pub text: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    pub on_accent: Color,
    pub border: Color,
    pub border_strong: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_pressed: Color,
    /// The accent on the page, for what is only text: links and tabs.
    pub accent_text: Color,
    pub accent_muted: Color,
    pub success: Color,
    pub success_muted: Color,
    pub success_text: Color,
    pub warning: Color,
    pub warning_muted: Color,
    pub warning_text: Color,
    pub danger: Color,
    pub danger_hover: Color,
    pub danger_pressed: Color,
    pub danger_muted: Color,
    pub danger_text: Color,
    pub focus_ring: Color,
    pub field_focus_ring: Color,
    pub tooltip: Color,
    pub on_tooltip: Color,
    pub scrim: Color,
    pub text_selection: Color,
    pub scrollbar: Color,
}

impl Theme {
    pub const fn light() -> Self {
        Self {
            page: rgb(0xf6f8fa),
            surface: rgb(0xffffff),
            surface_muted: rgb(0xf6f8fa),
            overlay: rgb(0xffffff),
            hover: rgb(0xeff2f5),
            pressed: rgb(0xe6eaef),
            selected: rgb(0xddf4ff),
            text: rgb(0x1f2328),
            text_muted: rgb(0x59636e),
            text_disabled: rgb(0x818b98),
            on_accent: rgb(0xffffff),
            border: rgb(0xd1d9e0),
            border_strong: rgb(0x8c959f),
            accent: rgb(0x0969da),
            accent_hover: rgb(0x0860ca),
            accent_pressed: rgb(0x0757ba),
            accent_text: rgb(0x0969da),
            accent_muted: rgb(0xddf4ff),
            success: rgb(0x1f883d),
            success_muted: rgb(0xdafbe1),
            success_text: rgb(0x1a7f37),
            warning: rgb(0xd29922),
            warning_muted: rgb(0xfff8c5),
            warning_text: rgb(0x9a6700),
            danger: rgb(0xcf222e),
            danger_hover: rgb(0xb91c27),
            danger_pressed: rgb(0xa40e26),
            danger_muted: rgb(0xffebe9),
            danger_text: rgb(0xd1242f),
            focus_ring: rgba(0x0969da99),
            field_focus_ring: rgba(0x0969da40),
            tooltip: rgb(0x25292e),
            on_tooltip: rgb(0xffffff),
            scrim: rgba(0x1f232880),
            text_selection: rgba(0x338fff59),
            scrollbar: rgba(0x00000055),
        }
    }

    pub const fn dark() -> Self {
        Self {
            page: rgb(0x0d1117),
            surface: rgb(0x151b23),
            surface_muted: rgb(0x212830),
            overlay: rgb(0x2a313c),
            hover: rgb(0x262c36),
            pressed: rgb(0x2a313c),
            selected: rgb(0x1a2f4d),
            text: rgb(0xf0f6fc),
            text_muted: rgb(0x9198a1),
            text_disabled: rgb(0x656c76),
            on_accent: rgb(0xffffff),
            border: rgb(0x3d444d),
            border_strong: rgb(0x656c76),
            accent: rgb(0x1f6feb),
            accent_hover: rgb(0x388bfd),
            accent_pressed: rgb(0x1158c7),
            accent_text: rgb(0x4493f8),
            accent_muted: rgb(0x172f4f),
            success: rgb(0x238636),
            success_muted: rgb(0x12261e),
            success_text: rgb(0x3fb950),
            warning: rgb(0xd29922),
            warning_muted: rgb(0x272115),
            warning_text: rgb(0xd29922),
            danger: rgb(0xda3633),
            danger_hover: rgb(0xe5534b),
            danger_pressed: rgb(0xb62324),
            danger_muted: rgb(0x25171c),
            danger_text: rgb(0xf85149),
            focus_ring: rgba(0x388bfdb3),
            field_focus_ring: rgba(0x388bfd59),
            tooltip: rgb(0x3d444d),
            on_tooltip: rgb(0xf0f6fc),
            scrim: rgba(0x010409a6),
            text_selection: rgba(0x388bfd66),
            scrollbar: rgba(0xffffff55),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::light()
    }
}

thread_local! {
    static THEME: Cell<Option<Signal<Theme>>> = const { Cell::new(None) };
    static SHEETS: RefCell<HashMap<usize, Rc<Sheet>>> = RefCell::default();
}

fn theme_signal() -> Signal<Theme> {
    THEME.with(|t| {
        t.get().unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever asks for it first.
            let (signal, _) = create_root(|| signal(Theme::default()));
            t.set(Some(signal));
            signal
        })
    })
}

/// The current theme. Reading it tracks it, so what is built from it follows [`set_theme`].
pub fn theme() -> Theme {
    theme_signal().get()
}

pub fn set_theme(theme: Theme) {
    SHEETS.with(|s| s.borrow_mut().clear());
    theme_signal().set(theme);
}

/// The sheet of a kind of element under the current theme. There is one for all elements of that
/// kind. Reading it tracks the theme.
pub(crate) fn shared(build: fn(&Theme) -> Sheet) -> Rc<Sheet> {
    let t = theme();
    SHEETS.with(|s| {
        s.borrow_mut()
            .entry(build as usize)
            .or_insert_with(|| Rc::new(build(&t)))
            .clone()
    })
}

/// How long a control takes to change its colors.
pub(crate) const QUICK: Transition = Transition::ms(120).ease_out();

/// Makes the colors of a control change smoothly.
pub(crate) fn smooth(s: Style) -> Style {
    s.transition_of(
        [Prop::Background, Prop::BorderColor, Prop::TextColor],
        QUICK,
    )
}

/// What is around a control that has the focus from the keyboard.
pub(crate) fn ring(s: Style, t: &Theme) -> Style {
    s.outline(2.0)
        .outline_offset(1.0)
        .outline_color(t.focus_ring)
}

/// Goes from 0 to 1 and back with `open`, for as long as `QUICK`.
pub(crate) fn fade(open: impl Fn() -> bool + 'static) -> Animated<f32> {
    let shown = animated(if untrack(&open) { 1.0f32 } else { 0.0 });
    let last = Cell::new(untrack(&open));
    effect(move || {
        let now = open();
        if last.replace(now) != now {
            untrack(|| shown.to(if now { 1.0 } else { 0.0 }, QUICK));
        }
    });
    shown
}

/// Keeps `time` at the seconds since now, from frame to frame, for as long as it exists.
pub(crate) fn keep_time(time: Signal<f32>) {
    fn tick(time: Signal<f32>, start: std::time::Instant) {
        request_frame(move |now| {
            if time.is_alive() {
                time.set(now.saturating_duration_since(start).as_secs_f32());
                tick(time, start);
            }
        });
    }
    tick(time, std::time::Instant::now());
}

fn page_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.page)
            .text_color(t.text)
            .caret_color(t.text)
            .selection_color(t.text_selection)
            .scrollbar_color(t.scrollbar)
    })
}

/// The root of an app: fills the window, and gives what is in it the colors of the theme, popups
/// included. It also shows what [`toast`](crate::toast) says.
pub fn page() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .sheet(|| shared(page_sheet))
        .child(crate::toast_host())
}
