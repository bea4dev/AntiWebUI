//! Checkboxes and radio buttons. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::surface::{Ink, When, focus_ring, halo, when};
use crate::color::color_scheme;
use crate::motion::{default_effects, default_spatial, fast_effects, fast_spatial};
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const TOUCH: f32 = 48.0;
const HALO: f32 = 40.0;
const BOX: f32 = 20.0;
const CORNER: f32 = 2.0;
const STROKE: f32 = 2.0;
const DOT: f32 = 10.0;

#[derive(ShaderParams, Clone, Copy)]
struct Check {
    color: Color,
    fraction: f32,
    gravity: f32,
    stroke: f32,
}

static CHECK: Shader<Check> = Shader::new("check", include_str!("../../shaders/check.wgsl"));

pub(super) fn preload_shaders() {
    CHECK.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&CHECK)
}

/// What a checkbox shows: the standard one, where a bar shows `Mixed`.
pub use anti_webui::widgets::CheckState;

/// How a checkbox looks.
#[derive(Clone, Default)]
pub struct CheckboxStyle {
    error: Option<When>,
    disabled: Option<When>,
}

impl CheckboxStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// In the error colors while `f` returns true.
    pub fn error_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.error = Some(Rc::new(f));
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// A checkbox bound to `checked`, which it flips when it is clicked. The behavior is the
/// standard checkbox's (`checkbox_with`).
pub fn checkbox(checked: impl Into<Binding<bool>>) -> Div {
    styled_checkbox(checked, CheckboxStyle::new())
}

pub fn styled_checkbox(checked: impl Into<Binding<bool>>, style: CheckboxStyle) -> Div {
    checkbox_with(checked, move |s| checkbox_look(s, style))
}

/// A checkbox that can also show that it is partly on. Pressing it makes a mix on, and on off.
pub fn tri_state_checkbox(state: impl Into<Binding<CheckState>>) -> Div {
    styled_tri_state_checkbox(state, CheckboxStyle::new())
}

pub fn styled_tri_state_checkbox(
    state: impl Into<Binding<CheckState>>,
    style: CheckboxStyle,
) -> Div {
    tri_checkbox_with(state, move |s| checkbox_look(s, style))
}

fn checkbox_look(box_state: CheckboxState, style: CheckboxStyle) -> Div {
    let state = Rc::new(move || box_state.state());
    let error = when(style.error);
    let disabled = when(style.disabled);

    // Off, the mark stays as it is while its color goes; the next time it draws itself from nothing.
    let first = untrack(|| state());
    let fraction = animated(if first == CheckState::Off {
        0.0f32
    } else {
        1.0
    });
    let gravity = animated(if first == CheckState::Mixed {
        1.0f32
    } else {
        0.0
    });
    let last = Cell::new(first);
    let moved = state.clone();
    effect(move || {
        let now = moved();
        let before = last.replace(now);
        if before == now {
            return;
        }
        untrack(|| match (before, now) {
            (CheckState::Off, _) => {
                fraction.set(0.0);
                gravity.set(if now == CheckState::Mixed { 1.0 } else { 0.0 });
                fraction.spring_to(1.0, default_spatial());
            }
            (_, CheckState::Off) => {}
            (_, CheckState::Mixed) => gravity.spring_to(1.0, default_spatial()),
            (_, CheckState::On) => gravity.spring_to(0.0, default_spatial()),
        });
    });

    let on_now = state.clone();
    let looks_off = disabled.clone();
    let looks_error = error.clone();
    let colors = move || {
        let c = color_scheme();
        let on = on_now() != CheckState::Off;
        if looks_off() {
            let faded = c.on_surface.with_alpha(0.38);
            let mark = if on { c.surface } else { Color::TRANSPARENT };
            (if on { faded } else { Color::TRANSPARENT }, faded, mark)
        } else if looks_error() {
            let mark = if on { c.on_error } else { Color::TRANSPARENT };
            (if on { c.error } else { Color::TRANSPARENT }, c.error, mark)
        } else if on {
            (c.primary, c.primary, c.on_primary)
        } else {
            (Color::TRANSPARENT, c.on_surface_variant, Color::TRANSPARENT)
        }
    };
    // Out faster than in, as in `Checkbox.kt`.
    let in_or_out = {
        let state = state.clone();
        move || {
            if state() == CheckState::Off {
                fast_effects()
            } else {
                default_effects()
            }
        }
    };
    let (background, border, mark) = untrack(&colors);
    let background = animated(background);
    let border = animated(border);
    let mark = animated(mark);
    let last_colors = Cell::new(untrack(&colors));
    effect(move || {
        let now = colors();
        if last_colors.replace(now) != now {
            let spring = untrack(&in_or_out);
            untrack(|| {
                background.spring_to(now.0, spring);
                border.spring_to(now.1, spring);
                mark.spring_to(now.2, spring);
            });
        }
    });

    let layer_state = state.clone();
    let layer_error = error;
    let ink = Ink::new_when(
        move |c| {
            if layer_error() {
                c.error
            } else if layer_state() != CheckState::Off {
                c.primary
            } else {
                c.on_surface
            }
        },
        disabled.clone(),
    );
    let focused = ink.focused;

    let root_off = disabled.clone();
    ink.listen_centered(
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(TOUCH)
            .h(TOUCH)
            .select_none()
            .dyn_style(move |s| {
                s.cursor(if root_off() {
                    CursorIcon::Default
                } else {
                    CursorIcon::Pointer
                })
            }),
    )
    .disabled(move || disabled())
    .child(halo(&ink, HALO).left(4.0).top(4.0))
    .child(
        div()
            .w(BOX)
            .h(BOX)
            .rounded(CORNER)
            .border(STROKE)
            .dyn_style(move |s| {
                focus_ring(
                    s.bg(background.get()).border_color(border.get()),
                    focused.get(),
                    &color_scheme(),
                )
            })
            .overlay(move || {
                let color = mark.get();
                (!color.is_transparent() && fraction.get() > 0.0).then(|| {
                    CHECK.params(Check {
                        color,
                        fraction: fraction.get(),
                        gravity: gravity.get(),
                        stroke: STROKE,
                    })
                })
            }),
    )
}

/// How a radio button looks.
#[derive(Clone, Default)]
pub struct RadioStyle {
    disabled: Option<When>,
}

impl RadioStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// A radio button that sets `selected` to `value` when it is clicked. The behavior is the
/// standard radio button's (`radio_with`).
pub fn radio_button<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    value: T,
) -> Div {
    styled_radio_button(selected, value, RadioStyle::new())
}

pub fn styled_radio_button<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    value: T,
    style: RadioStyle,
) -> Div {
    radio_with(selected, value, move |s| radio_look(s, style))
}

fn radio_look(radio: RadioState, style: RadioStyle) -> Div {
    let selected: When = Rc::new(move || radio.selected());
    let disabled = when(style.disabled);
    let dot_selected = selected.clone();
    let dot = follow(
        move || if dot_selected() { 1.0f32 } else { 0.0 },
        fast_spatial(),
    );
    let color_selected = selected.clone();
    let color_off = disabled.clone();
    let color = follow(
        move || {
            let c = color_scheme();
            if color_off() {
                c.on_surface.with_alpha(0.38)
            } else if color_selected() {
                c.primary
            } else {
                c.on_surface_variant
            }
        },
        default_effects(),
    );
    let layer_selected = selected;
    let ink = Ink::new_when(
        move |c| {
            if layer_selected() {
                c.primary
            } else {
                c.on_surface
            }
        },
        disabled.clone(),
    );
    let focused = ink.focused;
    let root_off = disabled.clone();
    ink.listen_centered(
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(TOUCH)
            .h(TOUCH)
            .select_none()
            .dyn_style(move |s| {
                s.cursor(if root_off() {
                    CursorIcon::Default
                } else {
                    CursorIcon::Pointer
                })
            }),
    )
    .disabled(move || disabled())
    .child(halo(&ink, HALO).left(4.0).top(4.0))
    .child(
        div()
            .relative()
            .w(BOX)
            .h(BOX)
            .rounded(BOX / 2.0)
            .border(STROKE)
            .dyn_style(move |s| {
                focus_ring(s.border_color(color.get()), focused.get(), &color_scheme())
            })
            .child(
                div()
                    .absolute()
                    .left(3.0)
                    .top(3.0)
                    .w(DOT)
                    .h(DOT)
                    .rounded(DOT / 2.0)
                    // Scaled rather than resized, so that it stays round and costs no layout.
                    .dyn_style(move |s| {
                        let v = dot.get();
                        if v <= 0.01 {
                            s
                        } else {
                            s.bg(color.get()).scale(v)
                        }
                    }),
            ),
    )
}
