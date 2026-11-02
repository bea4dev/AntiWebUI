//! Basic widgets built on the public core API.
//!
//! Every widget returns a regular builder (`Div` or `TextInput`), so it can be styled and
//! extended further with the usual methods.

use anti_webui_core::prelude::*;

/// Colors shared by the default widget styles.
pub mod theme {
    use anti_webui_core::style::{Color, rgb, rgba};

    pub const TEXT: Color = rgb(0x1f2328);
    pub const TEXT_MUTED: Color = rgb(0x59636e);
    pub const BORDER: Color = rgb(0xd1d9e0);
    pub const BORDER_STRONG: Color = rgb(0x8c959f);
    pub const SURFACE: Color = rgb(0xffffff);
    pub const SURFACE_MUTED: Color = rgb(0xf6f8fa);
    pub const HOVER: Color = rgb(0xeff2f5);
    pub const PRESSED: Color = rgb(0xe6eaef);
    pub const ACCENT: Color = rgb(0x0969da);
    pub const ACCENT_HOVER: Color = rgb(0x0860ca);
    pub const ACCENT_PRESSED: Color = rgb(0x0757ba);
    pub const SUCCESS: Color = rgb(0x1f883d);
    pub const DANGER: Color = rgb(0xcf222e);
    pub const FOCUS_RING: Color = rgba(0x0969da99);
}

fn focus_ring(s: Style) -> Style {
    s.outline(2.0)
        .outline_offset(1.0)
        .outline_color(theme::FOCUS_RING)
}

/// A push button with a text label.
pub fn button(label: impl IntoTextSource) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(6.0)
        .px(12.0)
        .py(5.0)
        .rounded(6.0)
        .border(1.0)
        .border_color(theme::BORDER)
        .bg(theme::SURFACE_MUTED)
        .font_weight(FontWeight::MEDIUM)
        .select_none()
        .cursor(CursorIcon::Pointer)
        .focusable()
        .hover(|s| s.bg(theme::HOVER))
        .active(|s| s.bg(theme::PRESSED))
        .focused(focus_ring)
        .child(text(label))
}

/// A filled button for the main action.
pub fn primary_button(label: impl IntoTextSource) -> Div {
    button(label)
        .bg(theme::ACCENT)
        .border_color(theme::ACCENT)
        .text_color(Color::WHITE)
        .hover(|s| s.bg(theme::ACCENT_HOVER))
        .active(|s| s.bg(theme::ACCENT_PRESSED))
}

/// A checkbox bound to `checked`.
pub fn checkbox(checked: Signal<bool>, label: impl IntoTextSource) -> Div {
    let toggle = move || checked.update(|c| *c = !*c);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0)
        .select_none()
        .cursor(CursorIcon::Pointer)
        .focusable()
        .focused(|s| {
            s.outline(2.0)
                .outline_offset(2.0)
                .outline_color(theme::FOCUS_RING)
                .rounded(4.0)
        })
        .on_click(move |_| toggle())
        .child(
            div()
                .w(16.0)
                .h(16.0)
                .rounded(4.0)
                .border(1.0)
                .flex()
                .items_center()
                .justify_center()
                .text_size(12.0)
                .font_weight(FontWeight::BOLD)
                .dyn_style(move |s| {
                    if checked.get() {
                        s.bg(theme::ACCENT)
                            .border_color(theme::ACCENT)
                            .text_color(Color::WHITE)
                    } else {
                        s.bg(theme::SURFACE)
                            .border_color(theme::BORDER_STRONG)
                            .text_color(Color::TRANSPARENT)
                    }
                })
                .child(text("✓").line_height(LineHeight::Relative(1.0))),
        )
        .child(text(label))
}

/// An on/off switch bound to `on`.
pub fn switch(on: Signal<bool>) -> Div {
    div()
        .w(36.0)
        .h(20.0)
        .p(2.0)
        .rounded(10.0)
        .flex()
        .flex_row()
        .items_center()
        .select_none()
        .cursor(CursorIcon::Pointer)
        .focusable()
        .focused(focus_ring)
        .on_click(move |_| on.update(|v| *v = !*v))
        .dyn_style(move |s| {
            if on.get() {
                s.bg(theme::SUCCESS).justify_end()
            } else {
                s.bg(theme::BORDER_STRONG).justify_start()
            }
        })
        .child(
            div()
                .w(16.0)
                .h(16.0)
                .rounded(8.0)
                .bg(Color::WHITE)
                .shadow_sm(),
        )
}

/// A horizontal slider bound to `value` in `min..=max`.
pub fn slider(value: Signal<f32>, min: f32, max: f32) -> Div {
    let dragging = signal(false);
    let set_from = move |e: &PointerEvent| {
        let t = (e.local.x / e.rect.width.max(1) as f32).clamp(0.0, 1.0);
        value.set(min + (max - min) * t);
    };
    let fraction = move || ((value.get() - min) / (max - min)).clamp(0.0, 1.0);
    div()
        .relative()
        .h(20.0)
        .min_w(80.0)
        .flex()
        .items_center()
        .select_none()
        .cursor(CursorIcon::Pointer)
        .focusable()
        .focused(|s| {
            s.outline(2.0)
                .outline_offset(2.0)
                .outline_color(theme::FOCUS_RING)
                .rounded(10.0)
        })
        .on_pointer_down(move |e| {
            dragging.set(true);
            set_from(e);
        })
        .on_pointer_move(move |e| {
            if dragging.get_untracked() {
                set_from(e);
            }
        })
        .on_pointer_up(move |_| dragging.set(false))
        .on_key_down(move |e| {
            let step = (max - min) / 20.0;
            match e.key {
                Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => {
                    value.update(|v| *v = (*v - step).max(min))
                }
                Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => {
                    value.update(|v| *v = (*v + step).min(max))
                }
                _ => {}
            }
        })
        .child(
            div().w_full().h(4.0).rounded(2.0).bg(theme::BORDER).child(
                div()
                    .h(4.0)
                    .rounded(2.0)
                    .bg(theme::ACCENT)
                    .dyn_style(move |s| s.w(pct(fraction() * 100.0))),
            ),
        )
        .child(
            div()
                .absolute()
                .top(2.0)
                .w(16.0)
                .h(16.0)
                .rounded(8.0)
                .bg(Color::WHITE)
                .border(1.0)
                .border_color(theme::BORDER_STRONG)
                .shadow_sm()
                // Keep the thumb inside the track: left = fraction × (width − thumb).
                .dyn_style(move |s| s.left(pct(fraction() * 100.0)).ml(-16.0 * fraction())),
        )
}

fn field_style(input: TextInput) -> TextInput {
    input
        .px(8.0)
        .py(5.0)
        .rounded(6.0)
        .border(1.0)
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .hover(|s| s.border_color(theme::BORDER_STRONG))
        .focused(|s| {
            s.border_color(theme::ACCENT)
                .outline(2.0)
                .outline_color(rgba(0x0969da40))
        })
}

/// A single-line text field bound to `value`.
pub fn text_field(value: Signal<String>) -> TextInput {
    field_style(text_input().value(value))
}

/// A multi-line text field bound to `value`.
pub fn text_area_field(value: Signal<String>) -> TextInput {
    field_style(text_area().value(value))
}

/// A vertically scrolling container.
pub fn scroll_view() -> Div {
    div().overflow_y_scroll()
}

/// A bordered surface.
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(12.0)
        .p(16.0)
        .rounded(8.0)
        .border(1.0)
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
}

pub fn heading(label: impl IntoTextSource) -> Text {
    text(label)
        .text_size(20.0)
        .font_weight(FontWeight::SEMIBOLD)
}

pub fn label(label: impl IntoTextSource) -> Text {
    text(label).text_color(theme::TEXT_MUTED).text_size(12.0)
}

/// A thin horizontal line.
pub fn divider() -> Div {
    div().h(1.0).bg(theme::BORDER)
}

pub use anti_webui_core::view::IntoTextSource;
