//! Fields to type in.

use crate::theme::{Theme, shared, smooth};
use anti_webui_core::prelude::*;

fn field_look(s: Style, t: &Theme) -> Style {
    smooth(s)
        .bg(t.surface)
        .border_color(t.border)
        .text_color(t.text)
}

fn focused_look(s: Style, t: &Theme) -> Style {
    s.border_color(t.accent)
        .outline(2.0)
        .outline_color(t.field_focus_ring)
}

fn disabled_look(s: Style, t: &Theme) -> Style {
    s.bg(t.surface_muted)
        .border_color(t.border)
        .text_color(t.text_disabled)
        .cursor(CursorIcon::Default)
}

fn field_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| field_look(s, t))
        .hover(|s| s.border_color(t.border_strong))
        .focus(|s| focused_look(s, t))
        .disabled(|s| disabled_look(s, t))
}

fn field_style(input: TextInput) -> TextInput {
    input
        .px(8.0)
        .py(5.0)
        .rounded(6.0)
        .border(1.0)
        .sheet(|| shared(field_sheet))
}

/// A single-line text field bound to `value`.
pub fn text_field(value: Signal<String>) -> TextInput {
    field_style(text_input().value(value))
}

/// A multi-line text field bound to `value`.
pub fn text_area_field(value: Signal<String>) -> TextInput {
    field_style(text_area().value(value))
}

fn box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| field_look(s, t))
        .hover(|s| s.border_color(t.border_strong))
        .disabled(|s| disabled_look(s, t))
}

fn focused_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| focused_look(field_look(s, t), t)).disabled(|s| disabled_look(s, t))
}

/// The box of a field with more than the text in it: what is styled, with an input that only has
/// the text. That tells it when it has the focus.
fn field_box(focused: Signal<bool>) -> Div {
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded(6.0)
        .border(1.0)
        .sheet(move || {
            shared(if focused.get() {
                focused_box_sheet
            } else {
                box_sheet
            })
        })
}

fn bare_input(focused: Signal<bool>) -> TextInput {
    text_input()
        .flex_1()
        .min_w(0.0)
        .px(8.0)
        .py(5.0)
        .on_focus_change(move |v| focused.set(v))
}

fn inner_button_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(Color::TRANSPARENT)
            .text_color(t.text_muted)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.hover).text_color(t.text))
    .active(|s| s.bg(t.pressed))
    .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn inner_button(label: &str) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .select_none()
        .sheet(|| shared(inner_button_sheet))
        .child(text(label.to_owned()).line_height(LineHeight::Relative(1.0)))
}

/// How many decimals to show a value of steps of `step` with.
fn decimals(step: f64) -> usize {
    let shown = format!("{step}");
    shown.split_once('.').map_or(0, |(_, d)| d.len())
}

/// How a value shows: with the decimals of its steps, unless that would hide that it is between
/// them.
fn formatted(v: f64, step: f64) -> String {
    let shown = format!("{:.*}", decimals(step), v);
    if shown.parse::<f64>().is_ok_and(|s| (s - v).abs() < 1e-9) {
        shown
    } else {
        v.to_string()
    }
}

/// A field for a number in `min..=max`, with buttons and arrow keys that go by `step`. What is
/// typed counts on Enter and when the focus goes, and goes back if it isn't a number.
pub fn number_field(value: Signal<f64>, min: f64, max: f64, step: f64) -> Div {
    let focused = signal(false);
    let typed = signal(formatted(value.get_untracked(), step));
    // Follows the value, but not under the fingers of who is typing.
    effect(move || {
        let v = value.get();
        if !untrack(|| focused.get()) {
            typed.set_if_changed(formatted(v, step));
        }
    });
    // To the next step, from what may be between steps. It counts steps from `min`, as what adds
    // up steps of a tenth doesn't end up on a tenth by itself.
    let go = move |up: bool| {
        let at = (value.get_untracked() - min) / step;
        let on_a_step = (at - at.round()).abs() < 1e-9;
        let steps = match (up, on_a_step) {
            (true, true) => at.round() + 1.0,
            (false, true) => at.round() - 1.0,
            (true, false) => at.floor() + 1.0,
            (false, false) => at.ceil() - 1.0,
        };
        let next = (min + steps * step).clamp(min, max);
        value.set(next);
        typed.set_if_changed(formatted(next, step));
    };
    let commit = move || {
        match typed.get_untracked().trim().parse::<f64>() {
            Ok(v) if v.is_finite() => value.set(v.clamp(min, max)),
            _ => {}
        }
        typed.set_if_changed(formatted(value.get_untracked(), step));
    };
    let stepper = |label: &str, up: bool| {
        inner_button(label)
            .flex_1()
            .w(18.0)
            .text_size(8.0)
            .on_click(move |_| go(up))
    };
    field_box(focused)
        .min_w(80.0)
        // The buttons reach the corners of the box, and what they show on hover stays inside them.
        .overflow_clip()
        .child(
            bare_input(focused)
                .value(typed)
                .on_focus_change(move |v| {
                    if !v {
                        commit();
                    }
                })
                .on_submit(move |_| commit())
                .on_key_down(move |e| {
                    let up = match e.key {
                        Key::Named(NamedKey::ArrowUp) => true,
                        Key::Named(NamedKey::ArrowDown) => false,
                        _ => return,
                    };
                    e.prevent_default();
                    commit();
                    go(up);
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .self_stretch()
                .flex_none()
                .border_l(1.0)
                .dyn_style(|s| s.border_color(crate::theme().border))
                .child(stepper("▲", true))
                .child(stepper("▼", false)),
        )
}

/// A field to search with: `placeholder` while it is empty, and a button that empties it, as
/// Escape does.
pub fn search_field(value: Signal<String>, placeholder: impl Into<String>) -> Div {
    let focused = signal(false);
    let input = node_ref();
    field_box(focused)
        .min_w(120.0)
        .child(
            bare_input(focused)
                .value(value)
                .placeholder(placeholder)
                .node_ref(input)
                .on_key_down(move |e| {
                    if matches!(e.key, Key::Named(NamedKey::Escape))
                        && value.with_untracked(|v| !v.is_empty())
                    {
                        e.prevent_default();
                        value.set(String::new());
                    }
                }),
        )
        .child(show(
            move || value.with(|v| !v.is_empty()),
            move || {
                inner_button("×")
                    .flex_none()
                    .w(20.0)
                    .h(20.0)
                    .mr(4.0)
                    .rounded(10.0)
                    .text_size(14.0)
                    .on_click(move |_| {
                        value.set(String::new());
                        input.focus();
                    })
            },
        ))
}
