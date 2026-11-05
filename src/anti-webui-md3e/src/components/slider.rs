//! Sliders. Sizes and colors are those of Jetpack Compose, which doesn't animate them: here the
//! values and handles move with the springs of the motion scheme. See
//! `docs/design/20-md3e-components.md`.

use super::surface::{When, focus_ring, following, when};
use crate::color::color_scheme;
use crate::motion::{fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;

const TOUCH: f32 = 48.0;
const LENGTH: f32 = 240.0;
const HANDLE: f32 = 4.0;
const HANDLE_NARROW: f32 = 2.0;
const INSET: f32 = HANDLE / 2.0;
const LABEL_BOX: f32 = 96.0;

#[derive(ShaderParams, Clone, Copy)]
struct Track {
    filled: Color,
    empty: Color,
    thumb: Color,
    tick_on: Color,
    tick_off: Color,
    start: f32,
    end: f32,
    kind: f32,
    handle_start: f32,
    handle_end: f32,
    handle_height: f32,
    track: f32,
    corner: f32,
    gap: f32,
    stop: f32,
    steps: f32,
    stops: f32,
    vertical: f32,
}

static TRACK: Shader<Track> = Shader::new("slider", include_str!("../../shaders/slider.wgsl"));

pub(super) fn preload_shaders() {
    TRACK.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&TRACK)
}

/// How a slider looks and which values it takes.
#[derive(Clone)]
pub struct SliderStyle {
    range: RangeInclusive<f32>,
    steps: u32,
    track: f32,
    corner: Option<f32>,
    handle_height: f32,
    gap: f32,
    stops: bool,
    centered: bool,
    vertical: bool,
    label: Option<Rc<dyn Fn(f32) -> String>>,
    disabled: Option<When>,
}

impl Default for SliderStyle {
    fn default() -> Self {
        Self {
            range: 0.0..=1.0,
            steps: 0,
            track: 16.0,
            corner: None,
            handle_height: 44.0,
            gap: 6.0,
            stops: true,
            centered: false,
            vertical: false,
            label: None,
            disabled: None,
        }
    }
}

impl SliderStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// The values it goes between. By default from 0 to 1.
    pub fn range(mut self, range: RangeInclusive<f32>) -> Self {
        self.range = range;
        self
    }

    /// How many values there are between the ends: it snaps to them and shows ticks.
    pub fn steps(mut self, steps: u32) -> Self {
        self.steps = steps;
        self
    }

    /// How thick the track is, and how round its ends are unless `corner` says otherwise.
    pub fn track(mut self, height: f32) -> Self {
        self.track = height;
        self
    }

    pub fn corner(mut self, radius: f32) -> Self {
        self.corner = Some(radius);
        self
    }

    pub fn handle_height(mut self, height: f32) -> Self {
        self.handle_height = height;
        self
    }

    /// Without the dot at the end of the track.
    pub fn no_stops(mut self) -> Self {
        self.stops = false;
        self
    }

    /// Goes both ways from the middle.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// Goes up rather than right.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// Shows the value above the handle while it is held, under the pointer or focused.
    pub fn label(mut self, f: impl Fn(f32) -> String + 'static) -> Self {
        self.label = Some(Rc::new(f));
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// A slider that shows `value`; `on_change` gets what it should become.
pub fn slider(value: impl Fn() -> f32 + 'static, on_change: impl Fn(f32) + 'static) -> Div {
    styled_slider(value, on_change, SliderStyle::new())
}

pub fn styled_slider(
    value: impl Fn() -> f32 + 'static,
    on_change: impl Fn(f32) + 'static,
    style: SliderStyle,
) -> Div {
    build(
        Rc::new(move || (f32::NAN, value())),
        Rc::new(move |(_, end)| on_change(end)),
        false,
        style,
    )
}

/// A slider with two handles, that shows `(start, end)`; `on_change` gets what they should become.
pub fn range_slider(
    values: impl Fn() -> (f32, f32) + 'static,
    on_change: impl Fn((f32, f32)) + 'static,
    style: SliderStyle,
) -> Div {
    build(Rc::new(values), Rc::new(on_change), true, style)
}

fn build(
    values: Rc<dyn Fn() -> (f32, f32)>,
    on_change: Rc<dyn Fn((f32, f32))>,
    two: bool,
    style: SliderStyle,
) -> Div {
    crate::ensure_ready();
    let SliderStyle {
        range,
        steps,
        track,
        corner,
        handle_height,
        gap,
        stops,
        centered,
        vertical,
        label,
        disabled,
    } = style;
    let (min, max) = (*range.start(), *range.end());
    let span = (max - min).max(f32::EPSILON);
    let disabled = when(disabled);
    let corner = corner.unwrap_or(track / 2.0);

    let to_fraction = move |v: f32| ((v - min) / span).clamp(0.0, 1.0);
    let from_fraction = move |t: f32| {
        let t = t.clamp(0.0, 1.0);
        let t = if steps > 0 {
            let n = (steps + 1) as f32;
            (t * n).round() / n
        } else {
            t
        };
        min + span * t
    };

    // Which handle is being dragged. While it is, it stays with the pointer.
    let dragging = signal(None::<usize>);
    let hover = signal(false);
    let keyed = [signal(false), signal(false)];
    let thumbs = [node_ref(), node_ref()];

    let fractions_now = {
        let values = values.clone();
        move || {
            let (start, end) = values();
            [
                if start.is_nan() {
                    0.0
                } else {
                    to_fraction(start)
                },
                to_fraction(end),
            ]
        }
    };
    let first = untrack(&fractions_now);
    let fractions = first.map(animated);
    let last = Cell::new(first);
    effect(move || {
        let now = fractions_now();
        let before = last.replace(now);
        let held = dragging.get_untracked();
        untrack(|| {
            for i in 0..2 {
                if now[i] != before[i] {
                    if held == Some(i) && steps == 0 {
                        fractions[i].set(now[i]);
                    } else {
                        fractions[i].spring_to(now[i], fast_spatial());
                    }
                }
            }
        });
    });
    let handles = [0, 1].map(|i| {
        following(
            move || {
                if dragging.get() == Some(i) || keyed[i].get() {
                    HANDLE_NARROW
                } else {
                    HANDLE
                }
            },
            fast_spatial(),
        )
    });

    let change = {
        let values = values.clone();
        let on_change = on_change.clone();
        move |i: usize, value: f32| {
            let (start, end) = untrack(|| values());
            let next = match i {
                0 => (value.min(end), end),
                _ => (start, if two { value.max(start) } else { value }),
            };
            if next != (start, end) {
                on_change(next);
            }
        }
    };
    let press_off = disabled.clone();
    let press_values = values.clone();
    let press_change = change.clone();
    let along = move |g: &GrabEvent| {
        let scale = with_scale();
        let inset = INSET * scale;
        if vertical {
            let h = g.rect.height as f32;
            (h - g.local.y - inset) / (h - inset * 2.0).max(1.0)
        } else {
            let w = g.rect.width as f32;
            (g.local.x - inset) / (w - inset * 2.0).max(1.0)
        }
    };
    let move_change = change.clone();

    let paint_off = disabled.clone();
    let thumb =
        move |i: usize, change: Rc<dyn Fn(usize, f32)>, values: Rc<dyn Fn() -> (f32, f32)>| {
            let off = disabled.clone();
            let value_of = move || {
                let (start, end) = values();
                if i == 0 { start } else { end }
            };
            let key_value = value_of.clone();
            let label_value = value_of.clone();
            let label = label.clone();
            let fraction = fractions[i];
            let is_keyed = keyed[i];
            let shown = following(
                move || {
                    if dragging.get() == Some(i) || is_keyed.get() || (hover.get() && !two) {
                        1.0f32
                    } else {
                        0.0
                    }
                },
                fast_effects(),
            );
            let size = following(
                move || {
                    if dragging.get() == Some(i) || is_keyed.get() || (hover.get() && !two) {
                        1.0f32
                    } else {
                        0.8
                    }
                },
                fast_spatial(),
            );
            let bubble = label.map(|label| {
                show(
                    move || shown.get() > 0.0,
                    move || {
                        let label = label.clone();
                        let value = label_value.clone();
                        let text_of = move || label(value());
                        let bubble = TextStyle::BODY_SMALL.apply(
                            div()
                                .flex_none()
                                .min_w(40.0)
                                .h(24.0)
                                .px(8.0)
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(4.0)
                                .transform_origin(0.5, 1.0)
                                .dyn_style(move |s| {
                                    let c = color_scheme();
                                    s.bg(c.inverse_surface)
                                        .text_color(c.inverse_on_surface)
                                        .opacity(shown.get().clamp(0.0, 1.0))
                                        .scale(size.get())
                                })
                                .child(text(text_of)),
                        );
                        div()
                            .absolute()
                            .flex()
                            .justify_center()
                            .items_end()
                            .w(LABEL_BOX)
                            .pointer_events_none()
                            .left((HANDLE - LABEL_BOX) / 2.0)
                            .bottom(handle_height + 4.0)
                            .child(bubble)
                    },
                )
            });
            div()
                .absolute()
                .focusable()
                .node_ref(thumbs[i])
                .rounded(HANDLE / 2.0)
                .dyn_style(move |s| {
                    let t = fraction.get();
                    let across = (TOUCH - handle_height) / 2.0;
                    // The track starts and ends half a handle in from the box.
                    let s = if vertical {
                        s.w(handle_height)
                            .h(HANDLE)
                            .left(across)
                            .bottom(pct(t * 100.0))
                            .mb(INSET - INSET * 2.0 * t - HANDLE / 2.0)
                    } else {
                        s.h(handle_height)
                            .w(HANDLE)
                            .top(across)
                            .left(pct(t * 100.0))
                            .ml(INSET - INSET * 2.0 * t - HANDLE / 2.0)
                    };
                    focus_ring(s, is_keyed.get(), &color_scheme())
                })
                .on_focus_visible(move |on| is_keyed.set(on))
                .on_key_down(move |e| {
                    if off() {
                        return;
                    }
                    let step = if steps > 0 {
                        span / (steps + 1) as f32
                    } else {
                        span / 20.0
                    };
                    let now = untrack(&key_value);
                    let to = match e.key {
                        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => now - step,
                        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => now + step,
                        Key::Named(NamedKey::Home) => min,
                        Key::Named(NamedKey::End) => max,
                        _ => return,
                    };
                    e.prevent_default();
                    change(i, from_fraction(to_fraction(to)));
                })
                .children(bubble)
        };
    let change_rc: Rc<dyn Fn(usize, f32)> = Rc::new(change);

    let kind = if two {
        1.0
    } else if centered {
        2.0
    } else {
        0.0
    };
    let root = div()
        .relative()
        .flex_none()
        .select_none()
        .cursor(CursorIcon::Pointer);
    let root = if vertical {
        root.w(TOUCH).h(LENGTH)
    } else {
        root.w(LENGTH).h(TOUCH)
    };
    let cursor_off = paint_off.clone();
    let root = root
        .dyn_style(move |s| {
            if cursor_off() {
                s.cursor(CursorIcon::Default)
            } else {
                s
            }
        })
        .fill(move || {
            let c = color_scheme();
            let off = paint_off();
            let (filled, empty, thumb, tick_on, tick_off) = if off {
                let strong = c.on_surface.with_alpha(0.38);
                let weak = c.on_surface.with_alpha(0.12);
                (
                    strong,
                    weak,
                    mix_premultiplied(c.surface, c.on_surface, 0.38),
                    weak,
                    strong,
                )
            } else {
                (
                    c.primary,
                    c.secondary_container,
                    c.primary,
                    c.secondary_container,
                    c.primary,
                )
            };
            TRACK.params(Track {
                filled,
                empty,
                thumb,
                tick_on,
                tick_off,
                start: fractions[0].get().clamp(0.0, 1.0),
                end: fractions[1].get().clamp(0.0, 1.0),
                kind,
                handle_start: handles[0].get().max(1.0),
                handle_end: handles[1].get().max(1.0),
                handle_height,
                track,
                corner,
                gap,
                stop: 4.0,
                steps: steps as f32,
                stops: if stops { 1.0 } else { 0.0 },
                vertical: if vertical { 1.0 } else { 0.0 },
            })
        })
        .on_hover(move |on| hover.set(on))
        .grab_threshold(0.0)
        .on_grab(move |g| match g.phase {
            GrabPhase::Start => {
                if press_off() {
                    return;
                }
                let t = along(g).clamp(0.0, 1.0);
                let i = if two {
                    let (start, end) = untrack(|| press_values());
                    let (a, b) = (to_fraction(start), to_fraction(end));
                    if (t - a).abs() < (t - b).abs() || (t < a && a == b) {
                        0
                    } else {
                        1
                    }
                } else {
                    1
                };
                dragging.set(Some(i));
                thumbs[i].focus();
                press_change(i, from_fraction(t));
            }
            GrabPhase::Move => {
                if let Some(i) = dragging.get_untracked() {
                    move_change(i, from_fraction(along(g)));
                }
            }
            GrabPhase::End | GrabPhase::Cancel => dragging.set(None),
        });
    let root = if two {
        root.child(thumb(0, change_rc.clone(), values.clone()))
    } else {
        root
    };
    root.child(thumb(1, change_rc, values))
}

/// The scale of the window, which pointer events are measured in.
fn with_scale() -> f32 {
    anti_webui::core::tree::with_doc(|d| d.scale.as_f64() as f32)
}
