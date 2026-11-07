//! A title bar for a window that draws its own frame: where the system draws none (GNOME), or
//! where the program asks for it (see "タイトルバーの部品" in `docs/design/36-multi-window.md`).

use crate::theme::theme;
use crate::theme::{Theme, smooth, themed};
use anti_webui_core::prelude::*;
use anti_webui_core::view::TextSource;
use std::rc::Rc;

/// What a title bar's look can follow.
#[derive(Clone, Copy)]
pub struct TitleBarState {
    _private: (),
}

impl TitleBarState {
    /// Whether the window has the keyboard: title bars of other windows are paler. Reading it
    /// tracks it.
    pub fn active(&self) -> bool {
        window_active()
    }

    /// Reading it tracks it.
    pub fn maximized(&self) -> bool {
        window_maximized()
    }
}

/// One of the buttons at the end of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleBarButton {
    Minimize,
    /// Maximizes, or restores the window when it is maximized.
    Maximize,
    Close,
}

impl TitleBarButton {
    /// What pressing it does. The close button asks first, as the system's does: see
    /// [`on_close_request`].
    pub fn press(self) {
        match self {
            TitleBarButton::Minimize => minimize_window(),
            TitleBarButton::Maximize => toggle_window_maximized(),
            TitleBarButton::Close => request_close_window(),
        }
    }
}

type Part = Rc<dyn Fn(Div, TitleBarState) -> Div>;
type ButtonPart = Rc<dyn Fn(Div, TitleBarButton, TitleBarState) -> Div>;

/// Changes to the parts of the standard title bar. Each gets the part with its standard styles on
/// and returns it; what it says goes above the theme.
#[derive(Clone, Default)]
pub struct TitleBarParts {
    bar: Option<Part>,
    title: Option<Part>,
    button: Option<ButtonPart>,
}

impl TitleBarParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The whole bar, which moves the window when dragged.
    pub fn bar(mut self, f: impl Fn(Div, TitleBarState) -> Div + 'static) -> Self {
        self.bar = Some(Rc::new(f));
        self
    }

    /// The box with the title in it.
    pub fn title(mut self, f: impl Fn(Div, TitleBarState) -> Div + 'static) -> Self {
        self.title = Some(Rc::new(f));
        self
    }

    /// Each button.
    pub fn button(
        mut self,
        f: impl Fn(Div, TitleBarButton, TitleBarState) -> Div + 'static,
    ) -> Self {
        self.button = Some(Rc::new(f));
        self
    }
}

fn bar_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface).border_color(t.border).text_color(t.text))
}

fn button_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| smooth(s).bg(Color::TRANSPARENT).cursor(CursorIcon::Default))
        .hover(|s| s.bg(t.hover))
        .active(|s| s.bg(t.pressed))
}

fn close_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| smooth(s).bg(Color::TRANSPARENT).cursor(CursorIcon::Default))
        .hover(|s| s.bg(t.danger))
        .active(|s| s.bg(t.danger_pressed))
}

/// The glyph of a button, in `color`: drawn with boxes, so it needs no font of icons.
fn glyph(button: TitleBarButton, maximized: bool, color: Color) -> Div {
    let frame = div().relative().w(10.0).h(10.0).flex_none();
    let line = |w: f32, x: f32, y: f32| div().absolute().left(x).top(y).w(w).h(1.0).bg(color);
    let square = |x: f32, y: f32, size: f32| {
        div()
            .absolute()
            .left(x)
            .top(y)
            .w(size)
            .h(size)
            .border(1.0)
            .border_color(color)
    };
    match button {
        TitleBarButton::Minimize => frame.child(line(10.0, 0.0, 5.0)),
        TitleBarButton::Maximize if !maximized => frame.child(square(0.0, 0.0, 10.0)),
        TitleBarButton::Maximize => frame
            .child(square(2.0, 0.0, 8.0))
            .child(square(0.0, 2.0, 8.0)),
        TitleBarButton::Close => frame
            .child(line(13.0, -1.5, 4.5).rotate(45.0))
            .child(line(13.0, -1.5, 4.5).rotate(-45.0)),
    }
}

/// The bar of a window that draws its own frame, with `title` and the buttons to minimize,
/// maximize and close it. It is there only while the window draws its own frame (see
/// [`window_decorations`]), so it can stay in place whatever the system does.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use anti_webui_widgets::title_bar;
///
/// let view = div().flex().flex_col().size_full()
///     .child(title_bar("Editor"))
///     .child(div().flex_1().child("…"));
/// ```
pub fn title_bar(title: impl IntoTextSource) -> Show {
    styled_title_bar(title, TitleBarParts::new())
}

/// A title bar with the standard look, with its parts changed by `parts`.
pub fn styled_title_bar(title: impl IntoTextSource, parts: TitleBarParts) -> Show {
    let title: Rc<dyn Fn() -> String> = match title.into_text_source() {
        TextSource::Static(t) => Rc::new(move || t.clone()),
        TextSource::Dynamic(f) => Rc::from(f),
    };
    title_bar_with(move |state| {
        let label = {
            let title = title.clone();
            div()
                .flex_1()
                .min_w(0.0)
                .px(12.0)
                .text_size(13.0)
                .font_weight(FontWeight::MEDIUM)
                .overflow_hidden()
                .whitespace_nowrap()
                .child(text(move || title()))
        };
        let label = match &parts.title {
            Some(f) => f(label, state),
            None => label,
        };
        let buttons = [
            TitleBarButton::Minimize,
            TitleBarButton::Maximize,
            TitleBarButton::Close,
        ]
        .map(|which| {
            let button = title_bar_button(which, move || {
                let close = which == TitleBarButton::Close;
                let ui = Interaction::new();
                let color = move || {
                    let t = theme();
                    match (close, ui.hovered()) {
                        (true, true) => t.on_accent,
                        (_, true) => t.text,
                        _ => t.text_muted,
                    }
                };
                div()
                    .w(46.0)
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .interaction(ui)
                    .sheet(move || themed(if close { close_sheet } else { button_sheet }))
                    .child(dyn_view(move || glyph(which, state.maximized(), color())))
            });
            match &parts.button {
                Some(f) => f(button, which, state),
                None => button,
            }
        });
        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .h(34.0)
            .flex_none()
            .border_b(1.0)
            .select_none()
            .sheet(|| themed(bar_sheet))
            .dyn_style(move |s| if state.active() { s } else { s.opacity(0.75) })
            .child(label)
            .children(buttons);
        match &parts.bar {
            Some(f) => f(bar, state),
            None => bar,
        }
    })
}

/// A title bar that looks like whatever `look` builds. Dragging the box that comes back moves the
/// window, double-clicking it maximizes or restores it, and the secondary button shows the system's
/// menu for the window. Put [`title_bar_button`]s in it. It is there only while the window draws its
/// own frame.
pub fn title_bar_with(look: impl Fn(TitleBarState) -> Div + 'static) -> Show {
    let look = Rc::new(look);
    show(
        move || window_decorations() == Decorations::Client && !window_fullscreen(),
        move || {
            let state = TitleBarState { _private: () };
            look(state)
                .access(|a| a.role(Role::Toolbar).label("Title bar"))
                .on_pointer_down(|e| match e.button {
                    Some(PointerButton::Primary) if e.click_count == 2 => toggle_window_maximized(),
                    Some(PointerButton::Primary) => start_window_move(),
                    Some(PointerButton::Secondary) => show_window_menu(),
                    _ => {}
                })
        },
    )
}

/// A button of a title bar that looks like whatever `look` builds: pressing it does what
/// [`TitleBarButton::press`] says, and doesn't move the window.
pub fn title_bar_button(which: TitleBarButton, look: impl FnOnce() -> Div) -> Div {
    let name = match which {
        TitleBarButton::Minimize => "Minimize",
        TitleBarButton::Maximize => "Maximize",
        TitleBarButton::Close => "Close",
    };
    look()
        .access(move |a| a.role(Role::Button).label(name))
        .on_pointer_down(|e| e.stop_propagation())
        .on_click(move |_| which.press())
}
