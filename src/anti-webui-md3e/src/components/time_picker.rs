//! Time pickers. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::surface::{Corners, Surface, When, focus_ring, following, round, surface};
use crate::color::color_scheme;
use crate::motion::{default_effects, default_spatial};
use crate::typography::TextStyle;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;
use std::time::Duration;

const DIAL: f32 = 256.0;
const OUTER: f32 = 101.0;
const INNER: f32 = 69.0;
const NUMBER: f32 = 48.0;
const HANDLE: f32 = 48.0;
const INNER_SPLIT: f32 = 74.0;

#[derive(ShaderParams, Clone, Copy)]
struct Dial {
    face: Color,
    hand: Color,
    angle: f32,
    length: f32,
    handle: f32,
}

static DIAL_FACE: Shader<Dial> =
    Shader::new("time_dial", include_str!("../../shaders/time_dial.wgsl"));

pub(super) fn preload_shaders() {
    DIAL_FACE.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&DIAL_FACE)
}

/// How a time picker shows the hours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimePickerStyle {
    twenty_four: bool,
}

impl TimePickerStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// From 0 to 23, without AM and PM.
    pub fn twenty_four_hours(mut self) -> Self {
        self.twenty_four = true;
        self
    }
}

/// The angle of an hour or a minute on the dial.
fn hour_angle(hour: u32) -> f32 {
    TAU / 12.0 * (hour % 12) as f32 - FRAC_PI_2
}

fn minute_angle(minute: u32) -> f32 {
    TAU / 60.0 * minute as f32 - FRAC_PI_2
}

/// Where a number is on the dial: the offset of its box from the center.
fn number_at(i: usize, count: usize, radius: f32) -> (f32, f32) {
    let angle = TAU / count as f32 * i as f32 - FRAC_PI_2;
    (radius * angle.cos(), radius * angle.sin())
}

/// A time picker that shows `time` as hour and minute, the hour from 0 to 23; `on_change` gets what
/// it should become.
pub fn time_picker(
    time: impl Fn() -> (u32, u32) + 'static,
    on_change: impl Fn((u32, u32)) + 'static,
    style: TimePickerStyle,
) -> Div {
    crate::ensure_ready();
    let time: Rc<dyn Fn() -> (u32, u32)> = Rc::new(time);
    let on_change: Rc<dyn Fn((u32, u32))> = Rc::new(on_change);
    let twenty_four = style.twenty_four;
    let minutes_mode = signal(false);
    let hand = {
        let time = time.clone();
        move || {
            let (hour, minute) = time();
            if minutes_mode.get() {
                minute_angle(minute)
            } else {
                hour_angle(hour)
            }
        }
    };
    let first = untrack(&hand);
    let angle = animated(first);
    let dragging = signal(false);
    let last = Cell::new(first);
    effect(move || {
        let now = hand();
        if last.replace(now) == now {
            return;
        }
        // The short way round.
        let here = angle.get_untracked();
        let mut diff = (here - now).rem_euclid(TAU);
        if diff > PI {
            diff -= TAU;
        }
        let to = here - diff;
        untrack(|| {
            if dragging.get() {
                angle.set(to);
            } else {
                angle.spring_to(to, default_spatial());
            }
        });
    });
    let inner_now = {
        let time = time.clone();
        move || {
            let (hour, _) = time();
            twenty_four && !minutes_mode.get() && (hour == 0 || hour > 12)
        }
    };
    let inner_for_hand = inner_now.clone();
    let hand_length = following(
        move || if inner_for_hand() { INNER } else { OUTER },
        default_spatial(),
    );

    // From a point on the dial, to a value, snapped or not.
    let from_pointer = {
        let time = time.clone();
        let on_change = on_change.clone();
        Rc::new(move |e: &PointerEvent, snap: bool| {
            let scale = with_doc(|d| d.scale.as_f64() as f32);
            let x = e.local.x - e.rect.width as f32 / 2.0;
            let y = e.local.y - e.rect.height as f32 / 2.0;
            let to_top = (y.atan2(x) + FRAC_PI_2).rem_euclid(TAU);
            let (hour, minute) = untrack(|| time());
            if minutes_mode.get_untracked() {
                let exact = to_top / TAU * 60.0;
                let m = if snap {
                    ((exact / 5.0).round() * 5.0) as u32
                } else {
                    exact.round() as u32
                } % 60;
                on_change((hour, m));
            } else {
                let h = (to_top / TAU * 12.0).round() as u32 % 12;
                let inner = (x * x + y * y).sqrt() / scale < INNER_SPLIT;
                let h = if twenty_four {
                    match (inner, h) {
                        (true, 0) => 0,
                        (true, h) => h + 12,
                        (false, 0) => 12,
                        (false, h) => h,
                    }
                } else if hour >= 12 {
                    h + 12
                } else {
                    h
                };
                on_change((h, minute));
            }
        })
    };
    let (press, moving) = (from_pointer.clone(), from_pointer);
    let moved_while_down = Rc::new(Cell::new(false));
    let (down_moved, move_moved, up_moved) = (
        moved_while_down.clone(),
        moved_while_down.clone(),
        moved_while_down,
    );

    let number = move |label: String, offset: (f32, f32), shown: Rc<dyn Fn() -> bool>| {
        let (x, y) = offset;
        let visible = following(
            move || if shown() { 1.0f32 } else { 0.0 },
            default_effects(),
        );
        TextStyle::BODY_LARGE.apply(
            div()
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .w(NUMBER)
                .h(NUMBER)
                .left(DIAL / 2.0 + x - NUMBER / 2.0)
                .top(DIAL / 2.0 + y - NUMBER / 2.0)
                .pointer_events_none()
                .dyn_style(move |s| {
                    let c = color_scheme();
                    let a = angle.get();
                    let r = hand_length.get();
                    // Under the handle it is on the color of the handle.
                    let (hx, hy) = (r * a.cos(), r * a.sin());
                    let distance = ((x - hx).powi(2) + (y - hy).powi(2)).sqrt();
                    let over = ((HANDLE / 2.0 + 6.0 - distance) / 12.0).clamp(0.0, 1.0);
                    s.text_color(mix_premultiplied(c.on_surface, c.on_primary, over))
                        .opacity(visible.get().clamp(0.0, 1.0))
                })
                .child(text(label)),
        )
    };
    let mut numbers: Vec<Div> = Vec::new();
    let hours_on: Rc<dyn Fn() -> bool> = Rc::new(move || !minutes_mode.get());
    let minutes_on: Rc<dyn Fn() -> bool> = Rc::new(move || minutes_mode.get());
    for i in 0..12 {
        let hour = if i == 0 { 12 } else { i };
        numbers.push(number(
            hour.to_string(),
            number_at(i, 12, OUTER),
            hours_on.clone(),
        ));
        if twenty_four {
            let inner = if i == 0 { 0 } else { i + 12 };
            numbers.push(number(
                format!("{inner:02}"),
                number_at(i, 12, INNER),
                hours_on.clone(),
            ));
        }
        numbers.push(number(
            format!("{:02}", i * 5),
            number_at(i, 12, OUTER),
            minutes_on.clone(),
        ));
    }
    let key_change = on_change.clone();
    let key_time = time.clone();
    let tap_mode = minutes_mode;
    let dial_focused = signal(false);
    let dial = div()
        .relative()
        .flex_none()
        .w(DIAL)
        .h(DIAL)
        .rounded(DIAL / 2.0)
        .select_none()
        .focusable()
        .cursor(CursorIcon::Pointer)
        .dyn_style(move |s| focus_ring(s, dial_focused.get(), &color_scheme()))
        .fill(move || {
            let c = color_scheme();
            DIAL_FACE.params(Dial {
                face: c.surface_container_highest,
                hand: c.primary,
                angle: angle.get(),
                length: hand_length.get(),
                handle: HANDLE,
            })
        })
        .on_focus_visible(move |on| dial_focused.set(on))
        .on_pointer_down(move |e| {
            dragging.set(true);
            down_moved.set(false);
            press(e, true);
        })
        .on_pointer_move(move |e| {
            if dragging.get_untracked() {
                move_moved.set(true);
                moving(e, false);
            }
        })
        .on_active(move |on| {
            if on {
                return;
            }
            dragging.set(false);
            let dragged = up_moved.get();
            if !tap_mode.get_untracked() {
                // After a tap the numbers change a moment later, after a drag at once.
                let wait = if dragged { 0 } else { 100 };
                set_timeout(Duration::from_millis(wait), move || tap_mode.set(true));
            }
        })
        .on_key_down(move |e| {
            let (hour, minute) = untrack(|| key_time());
            let delta: i32 = match e.key {
                Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => 1,
                Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => -1,
                _ => return,
            };
            e.prevent_default();
            if minutes_mode.get_untracked() {
                key_change((hour, (minute as i32 + delta).rem_euclid(60) as u32));
            } else {
                key_change(((hour as i32 + delta).rem_euclid(24) as u32, minute));
            }
        })
        .children(numbers);

    let time_box = {
        let time = time.clone();
        move |is_minutes: bool| {
            let time = time.clone();
            let label = move || {
                let (hour, minute) = time();
                if is_minutes {
                    format!("{minute:02}")
                } else if twenty_four {
                    format!("{hour:02}")
                } else {
                    let h = hour % 12;
                    format!("{:02}", if h == 0 { 12 } else { h })
                }
            };
            let on_it: When = Rc::new(move || minutes_mode.get() == is_minutes);
            let colors_on = on_it.clone();
            surface(Surface {
                colors: Rc::new(move |c, _| {
                    if colors_on() {
                        (
                            c.primary_container,
                            c.on_primary_container,
                            Color::TRANSPARENT,
                        )
                    } else {
                        (
                            c.surface_container_highest,
                            c.on_surface,
                            Color::TRANSPARENT,
                        )
                    }
                }),
                disabled_content: |c| c.on_surface.with_alpha(0.38),
                rest: round(8.0),
                pressed: round(8.0),
                checked_shape: round(8.0),
                spring: default_effects(),
                levels: (0.0, 0.0),
                outline: 0.0,
                disabled: None,
                checked: None,
            })
            .w(96.0)
            .h(80.0)
            .on_click(move |_| minutes_mode.set(is_minutes))
            .child(TextStyle::DISPLAY_LARGE.apply(text(label)))
        }
    };

    let period = (!twenty_four).then(|| {
        let period_button = {
            let time = time.clone();
            let on_change = on_change.clone();
            move |label: &'static str, is_pm: bool| {
                let time_now = time.clone();
                let on: When = Rc::new(move || (time_now().0 >= 12) == is_pm);
                let time = time.clone();
                let on_change = on_change.clone();
                let corners: Corners = round(26.0);
                let colors_on = on.clone();
                surface(Surface {
                    colors: Rc::new(move |c, _| {
                        if colors_on() {
                            (
                                c.primary_container,
                                c.on_primary_container,
                                Color::TRANSPARENT,
                            )
                        } else {
                            (
                                c.surface_container_lowest,
                                c.on_surface_variant,
                                Color::TRANSPARENT,
                            )
                        }
                    }),
                    disabled_content: |c| c.on_surface.with_alpha(0.38),
                    rest: corners,
                    pressed: round(12.0),
                    checked_shape: round(12.0),
                    spring: crate::motion::fast_spatial(),
                    levels: (0.0, 0.0),
                    outline: 0.0,
                    disabled: None,
                    checked: Some(on),
                })
                .w(52.0)
                .flex_grow(1.0)
                .on_click(move |_| {
                    let (hour, minute) = untrack(|| time());
                    let hour = hour % 12 + if is_pm { 12 } else { 0 };
                    on_change((hour, minute));
                })
                .child(TextStyle::TITLE_MEDIUM.apply(text(label)))
            }
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(80.0)
            .ml(12.0)
            .gap(4.0)
            .child(period_button("AM", false))
            .child(period_button("PM", true))
    });

    div()
        .flex()
        .flex_col()
        .flex_none()
        .items_center()
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .child(time_box(false))
                .child(
                    TextStyle::DISPLAY_LARGE.apply(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(24.0)
                            .h(80.0)
                            .dyn_style(|s| s.text_color(color_scheme().on_surface))
                            .child(text(":")),
                    ),
                )
                .child(time_box(true))
                .children(period),
        )
        .child(div().h(36.0).flex_none())
        .child(dial)
}

/// A time picker in a dialog. Buttons are made each time it opens.
pub fn time_picker_dialog(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    time: impl Fn() -> (u32, u32) + 'static,
    on_change: impl Fn((u32, u32)) + 'static,
    style: TimePickerStyle,
    buttons: impl Fn() -> Vec<Div> + 'static,
) -> Show {
    let time: Rc<dyn Fn() -> (u32, u32)> = Rc::new(time);
    let on_change: Rc<dyn Fn((u32, u32))> = Rc::new(on_change);
    super::modal::dialog(open, on_dismiss, move || {
        let (time, on_change) = (time.clone(), on_change.clone());
        div()
            .flex()
            .flex_col()
            .px(24.0)
            .pt(24.0)
            .pb(24.0)
            .child(
                TextStyle::LABEL_MEDIUM
                    .apply(div().mb(20.0).child(text("Select time")))
                    .dyn_style(|s| s.text_color(color_scheme().on_surface_variant)),
            )
            .child(time_picker(move || time(), move |t| on_change(t), style))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(8.0)
                    .pt(24.0)
                    .children(buttons()),
            )
    })
}
