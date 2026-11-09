//! Sliders. Sizes and colors are those of Jetpack Compose, which doesn't animate them: here the
//! values and handles move with the springs of the motion scheme. See
//! `docs/design/20-md3e-components.md`.

use super::progress::{AMPLITUDE, STROKE, WAVELENGTH};
use super::surface::{When, focus_ring, when};
use crate::color::color_scheme;
use crate::motion::{fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::time::Instant;

const TOUCH: f32 = 48.0;
const LENGTH: f32 = 240.0;
const HANDLE: f32 = 4.0;
const HANDLE_NARROW: f32 = 2.0;
const INSET: f32 = HANDLE / 2.0;
const LABEL_BOX: f32 = 96.0;
/// The handle of [`SliderStyle::wavy`].
const WAVY_HANDLE: f32 = 24.0;

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
    stroke: f32,
    amplitude: f32,
    wavelength: f32,
    phase: f32,
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
    wave: Option<Wave>,
}

/// The wavy line of [`SliderStyle::wavy`].
#[derive(Clone, Copy)]
struct Wave {
    amplitude: f32,
    wavelength: f32,
    stroke: f32,
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
            wave: None,
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

    /// Looks like the wavy progress bar: what is covered is its wavy line, and the rest of the track
    /// is as thin as the line, with a smaller handle to go with it. The wave flows while the pointer
    /// is on the slider or it is held, and lies still otherwise, asking for no frames. Only for a
    /// slider from the start: a range or a centered slider stays straight. `track` and
    /// `handle_height` after this change the track and the handle.
    pub fn wavy(self) -> Self {
        self.wave(AMPLITUDE, WAVELENGTH, STROKE)
            .track(STROKE)
            .handle_height(WAVY_HANDLE)
    }

    /// A wavy line with the height of the wave from its middle, its length and the thickness of its
    /// line, on the track as it is otherwise.
    pub fn wave(mut self, amplitude: f32, wavelength: f32, stroke: f32) -> Self {
        self.wave = Some(Wave {
            amplitude,
            wavelength: wavelength.max(1.0),
            stroke,
        });
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

impl SliderStyle {
    /// The range and the options of the standard slider's behavior.
    fn behavior(&self) -> (f32, f32, SliderOptions) {
        let (min, max) = (*self.range.start(), *self.range.end());
        let mut options = SliderOptions::new().inset(INSET);
        if self.steps > 0 {
            options = options.step((max - min) / (self.steps + 1) as f32);
        }
        if self.vertical {
            options = options.vertical();
        }
        (min, max, options)
    }
}

/// A slider bound to `value`. The behavior is the standard slider's (`slider_with`).
pub fn slider(value: impl Into<Binding<f32>>) -> Div {
    styled_slider(value, SliderStyle::new())
}

pub fn styled_slider(value: impl Into<Binding<f32>>, style: SliderStyle) -> Div {
    let (min, max, options) = style.behavior();
    slider_with(value, min, max, options, move |s| look(s, style))
}

/// A slider with two handles for a range from `start` to `end`, which don't pass each other. The
/// behavior is the standard range slider's (`range_slider_with`).
pub fn range_slider(
    start: impl Into<Binding<f32>>,
    end: impl Into<Binding<f32>>,
    style: SliderStyle,
) -> Div {
    let (min, max, options) = style.behavior();
    range_slider_with(start, end, min, max, options, move |s| look(s, style))
}

/// Moves the wave along, a wavelength a second, while `flowing`, from where it was.
fn flow(
    phase: Signal<f32>,
    flowing: Memo<bool>,
    start: Instant,
    from: f32,
    latest: Rc<Cell<u64>>,
    me: u64,
) {
    request_frame(move |now| {
        if !phase.is_alive() || latest.get() != me || !untrack(|| flowing.get()) {
            return;
        }
        let seconds = now.saturating_duration_since(start).as_secs_f32();
        phase.set((from + seconds * std::f32::consts::TAU) % std::f32::consts::TAU);
        flow(phase, flowing, start, from, latest, me);
    });
}

fn look(state: SliderState, style: SliderStyle) -> Div {
    crate::ensure_ready();
    let SliderStyle {
        range: _,
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
        wave,
    } = style;
    let two = state.count() == 2;
    let disabled = when(disabled);
    let corner = corner.unwrap_or(track / 2.0);
    // Here the handles are 0 and 1, and one alone is 1, as in the shader.
    let held = move || state.held().map(|i| if two { i } else { 1 });
    let keyed = move |i: usize| state.focus_visible_of(i);

    let fractions_now = move || {
        if two {
            [state.fraction_of(0), state.fraction_of(1)]
        } else {
            [0.0, state.fraction()]
        }
    };
    let first = untrack(fractions_now);
    let fractions = first.map(animated);
    let last = Cell::new(first);
    effect(move || {
        let now = fractions_now();
        let before = last.replace(now);
        let holding = untrack(held);
        untrack(|| {
            for i in 0..2 {
                if now[i] != before[i] {
                    if holding == Some(i) && steps == 0 {
                        fractions[i].set(now[i]);
                    } else {
                        fractions[i].spring_to(now[i], fast_spatial());
                    }
                }
            }
        });
    });
    let handles = [0, 1].map(|i| {
        follow(
            move || {
                if held() == Some(i) || keyed(i) {
                    HANDLE_NARROW
                } else {
                    HANDLE
                }
            },
            fast_spatial(),
        )
    });

    // Where the wave is along its length.
    let wavy = wave.is_some();
    let lively = move || wavy && (state.hovered() || state.held().is_some());
    let phase = signal(0.0f32);
    let flowing = memo(lively);
    let latest = Rc::new(Cell::new(0u64));
    effect(move || {
        if flowing.get() {
            // One that went off and on again before its next frame stops for this one.
            let me = latest.get() + 1;
            latest.set(me);
            let from = untrack(|| phase.get());
            flow(phase, flowing, Instant::now(), from, latest.clone(), me);
        }
    });

    let paint_off = disabled.clone();
    let thumb = move |i: usize| {
        let value_of = move || {
            if two {
                state.value_of(i)
            } else {
                state.value()
            }
        };
        let label = label.clone();
        let fraction = fractions[i];
        let up = move || held() == Some(i) || keyed(i) || (state.hovered() && !two);
        let shown = follow(move || if up() { 1.0f32 } else { 0.0 }, fast_effects());
        let size = follow(move || if up() { 1.0f32 } else { 0.8 }, fast_spatial());
        let bubble = label.map(|label| {
            show(
                move || shown.get() > 0.0,
                move || {
                    let label = label.clone();
                    let text_of = move || label(value_of());
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
        let handle = div()
            .absolute()
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
                focus_ring(s, keyed(i), &color_scheme())
            })
            .children(bubble);
        state.thumb(if two { i } else { 0 }, handle)
    };

    let kind = if two {
        1.0
    } else if centered {
        2.0
    } else {
        0.0
    };
    let root = div().relative().flex_none().cursor(CursorIcon::Pointer);
    let root = if vertical {
        root.w(TOUCH).h(LENGTH)
    } else {
        root.w(LENGTH).h(TOUCH)
    };
    let cursor_off = paint_off.clone();
    let root_off = paint_off.clone();
    let root = root
        .dyn_style(move |s| {
            if cursor_off() {
                s.cursor(CursorIcon::Default)
            } else {
                s
            }
        })
        .disabled(move || root_off())
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
                stroke: wave.map_or(0.0, |w| w.stroke),
                amplitude: wave.map_or(0.0, |w| w.amplitude),
                wavelength: wave.map_or(1.0, |w| w.wavelength),
                phase: phase.get(),
            })
        });
    let root = if two { root.child(thumb(0)) } else { root };
    root.child(thumb(1))
}
