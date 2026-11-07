//! Buttons.

use crate::theme::{Theme, focus_ring, smooth, themed};
use anti_webui_core::prelude::*;

fn disabled_look(s: Style, t: &Theme) -> Style {
    s.bg(t.surface_muted)
        .border_color(t.border)
        .text_color(t.text_disabled)
        .cursor(CursorIcon::Default)
}

fn button_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.surface_muted)
            .border_color(t.border)
            .text_color(t.text)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.hover).border_color(t.border_strong))
    .active(|s| s.bg(t.pressed))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| disabled_look(s, t))
}

fn primary_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.accent)
            .border_color(t.accent)
            .text_color(t.on_accent)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.accent_hover).border_color(t.accent_hover))
    .active(|s| s.bg(t.accent_pressed).border_color(t.accent_pressed))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| disabled_look(s, t))
}

fn danger_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.surface_muted)
            .border_color(t.border)
            .text_color(t.danger_text)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| {
        s.bg(t.danger_hover)
            .border_color(t.danger_hover)
            .text_color(t.on_accent)
    })
    .active(|s| {
        s.bg(t.danger_pressed)
            .border_color(t.danger_pressed)
            .text_color(t.on_accent)
    })
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| disabled_look(s, t))
}

fn quiet_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .text_color(t.text)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.hover))
    .active(|s| s.bg(t.pressed))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| {
        s.bg(Color::TRANSPARENT)
            .text_color(t.text_disabled)
            .cursor(CursorIcon::Default)
    })
}

fn toggled_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.accent_muted)
            .border_color(t.accent)
            .text_color(t.accent_text)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.accent_muted).border_color(t.accent_hover))
    .active(|s| s.bg(t.pressed))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| disabled_look(s, t))
    .accent_border()
}

fn frame() -> Div {
    div()
        .access(|a| a.role(Role::Button))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(6.0)
        .px(12.0)
        .py(5.0)
        .rounded(6.0)
        .border(1.0)
        .font_weight(FontWeight::MEDIUM)
        .select_none()
        .focusable()
}

/// A push button with a text label.
pub fn button(label: impl IntoTextSource) -> Div {
    frame().sheet(|| themed(button_sheet)).child(text(label))
}

/// A filled button for the main action.
pub fn primary_button(label: impl IntoTextSource) -> Div {
    frame().sheet(|| themed(primary_sheet)).child(text(label))
}

/// A button for what can't be undone.
pub fn danger_button(label: impl IntoTextSource) -> Div {
    frame().sheet(|| themed(danger_sheet)).child(text(label))
}

/// A button that only shows when the pointer is on it.
pub fn quiet_button(label: impl IntoTextSource) -> Div {
    frame().sheet(|| themed(quiet_sheet)).child(text(label))
}

/// A button that stays pressed, bound to `on`. Its behavior is [`toggle_with`](crate::toggle_with)'s.
pub fn toggle_button(on: impl Into<Binding<bool>>, label: impl IntoTextSource) -> Div {
    crate::toggle_with(on, move |t| {
        frame()
            .sheet(move || themed(if t.on() { toggled_sheet } else { button_sheet }))
            .child(text(label))
    })
}
