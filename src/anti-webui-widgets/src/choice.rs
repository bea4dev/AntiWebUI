//! Checkboxes, radio buttons and switches.

use crate::theme::{QUICK, Theme, ring, shared, smooth};
use anti_webui_core::prelude::*;
use std::rc::Rc;

fn row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).rounded(4.0))
        .focus_visible(|s| ring(s, t).outline_offset(2.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn row() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0)
        .select_none()
        .sheet(|| shared(row_sheet))
}

fn off_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.surface)
            .border_color(t.border_strong)
            .text_color(Color::TRANSPARENT)
    })
    .disabled(|s| s.bg(t.surface_muted).border_color(t.border))
}

fn on_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.accent)
            .border_color(t.accent)
            .text_color(t.on_accent)
    })
    .disabled(|s| {
        s.bg(t.text_disabled)
            .border_color(t.text_disabled)
            .text_color(t.surface)
    })
    .accent_background()
    .accent_border()
}

/// The state of a checkbox that can also stand for a mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Off,
    On,
    Mixed,
}

fn check_box(state: impl Fn() -> CheckState + 'static) -> Div {
    let state = Rc::new(state);
    let (on, mark, bar) = (state.clone(), state.clone(), state);
    div()
        .flex_none()
        .w(16.0)
        .h(16.0)
        .rounded(4.0)
        .border(1.0)
        .flex()
        .items_center()
        .justify_center()
        .text_size(12.0)
        .font_weight(FontWeight::BOLD)
        .sheet(move || {
            shared(if on() == CheckState::Off {
                off_box_sheet
            } else {
                on_box_sheet
            })
        })
        .child(
            text(move || if mark() == CheckState::On { "✓" } else { "" }.to_owned())
                .line_height(LineHeight::Relative(1.0)),
        )
        .child(show(
            move || bar() == CheckState::Mixed,
            // The color of the mark, whatever gives it that.
            || {
                div()
                    .w(8.0)
                    .h(2.0)
                    .rounded(1.0)
                    .dyn_style(|s| s.bg(crate::theme().on_accent))
            },
        ))
}

/// A checkbox bound to `checked`.
pub fn checkbox(checked: Signal<bool>, label: impl IntoTextSource) -> Div {
    row()
        .focusable()
        .on_click(move |_| checked.update(|c| *c = !*c))
        .child(check_box(move || {
            if checked.get() {
                CheckState::On
            } else {
                CheckState::Off
            }
        }))
        .child(text(label))
}

/// A checkbox that can also stand for a mix, as one above a group of checkboxes does. Pressing it
/// makes a mix on, and on off.
pub fn tri_checkbox(state: Signal<CheckState>, label: impl IntoTextSource) -> Div {
    row()
        .focusable()
        .on_click(move |_| {
            state.update(|s| {
                *s = if *s == CheckState::On {
                    CheckState::Off
                } else {
                    CheckState::On
                }
            })
        })
        .child(check_box(move || state.get()))
        .child(text(label))
}

fn radio_circle(on: impl Fn() -> bool + 'static) -> Div {
    let on = Rc::new(on);
    let dot = on.clone();
    div()
        .flex_none()
        .w(16.0)
        .h(16.0)
        .rounded(8.0)
        .border(1.0)
        .flex()
        .items_center()
        .justify_center()
        .sheet(move || shared(if on() { on_box_sheet } else { off_box_sheet }))
        .child(
            div()
                .w(6.0)
                .h(6.0)
                .rounded(3.0)
                .transition_of([Prop::Background], QUICK)
                .dyn_style(move |s| {
                    s.bg(if dot() {
                        crate::theme().on_accent
                    } else {
                        Color::TRANSPARENT
                    })
                }),
        )
}

/// A radio button that sets `selected` to `value`. See also [`radio_group`], which is one stop for
/// Tab and goes through its options with the arrow keys.
pub fn radio<T: Clone + PartialEq + 'static>(
    selected: Signal<T>,
    value: T,
    label: impl IntoTextSource,
) -> Div {
    let mine = value.clone();
    row()
        .focusable()
        .on_click(move |_| selected.set(value.clone()))
        .child(radio_circle(move || selected.with(|s| *s == mine)))
        .child(text(label))
}

fn ringed_row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| ring(s.cursor(CursorIcon::Pointer).rounded(4.0), t).outline_offset(2.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

/// Radio buttons for `options`, one under the other. It is one stop for Tab, and the arrow keys go
/// through the options.
pub fn radio_group<T: Clone + PartialEq + 'static>(
    selected: Signal<T>,
    options: Vec<(T, String)>,
) -> Div {
    let options = Rc::new(options);
    let keyed = signal(false);
    let keys = options.clone();
    let rows = options.iter().map(|(value, name)| {
        let (mine, this) = (value.clone(), value.clone());
        let picked = value.clone();
        row()
            .sheet(move || {
                shared(if keyed.get() && selected.with(|s| *s == this) {
                    ringed_row_sheet
                } else {
                    row_sheet
                })
            })
            .on_click(move |_| selected.set(picked.clone()))
            .child(radio_circle(move || selected.with(|s| *s == mine)))
            .child(text(name.clone()))
    });
    div()
        .flex()
        .flex_col()
        .gap(6.0)
        .focusable()
        .on_focus_visible(move |v| keyed.set(v))
        .on_key_down(move |e| {
            let count = keys.len();
            let step = match e.key {
                Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight) => 1,
                Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => count.saturating_sub(1),
                _ => return,
            };
            e.prevent_default();
            let at = selected.with_untracked(|s| keys.iter().position(|(v, _)| v == s));
            let next = at.map_or(0, |i| (i + step) % count.max(1));
            if let Some((v, _)) = keys.get(next) {
                selected.set(v.clone());
            }
        })
        .children(rows.collect::<Vec<_>>())
}

fn switch_off_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.border_strong)
            .rounded(10.0)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.text_muted))
    .focus_visible(|s| ring(s, t))
    .disabled(|s| s.bg(t.border).cursor(CursorIcon::Default))
}

fn switch_on_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.success)
            .rounded(10.0)
            .cursor(CursorIcon::Pointer)
    })
    .focus_visible(|s| ring(s, t))
    .disabled(|s| s.bg(t.border).cursor(CursorIcon::Default))
    .accent_background()
}

/// An on/off switch bound to `on`.
pub fn switch(on: Signal<bool>) -> Div {
    div()
        .flex_none()
        .w(36.0)
        .h(20.0)
        .p(2.0)
        .flex()
        .flex_row()
        .items_center()
        .select_none()
        .focusable()
        .on_click(move |_| on.update(|v| *v = !*v))
        .sheet(move || {
            shared(if on.get() {
                switch_on_sheet
            } else {
                switch_off_sheet
            })
        })
        .child(
            div()
                .flex_none()
                .w(16.0)
                .h(16.0)
                .rounded(8.0)
                .bg(Color::WHITE)
                .shadow_sm()
                .transition_of([Prop::Margin], QUICK)
                .dyn_style(move |s| s.ml(if on.get() { 16.0 } else { 0.0 })),
        )
}
