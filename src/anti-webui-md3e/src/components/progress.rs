//! The wavy progress bar. See `docs/design/15-md3e.md`.

use crate::color::color_scheme;
use crate::motion::default_effects;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const STROKE: f32 = 4.0;
const AMPLITUDE: f32 = 3.0;
const WAVELENGTH: f32 = 40.0;
const GAP: f32 = 4.0;
const STOP: f32 = 4.0;
const HEIGHT: f32 = 10.0;
const WIDTH: f32 = 240.0;

#[derive(ShaderParams, Clone, Copy)]
struct Wave {
    line: Color,
    track: Color,
    progress: f32,
    /// From 0 for a straight line to 1 for a full wave.
    wave: f32,
    phase: f32,
    stroke: f32,
    amplitude: f32,
    wavelength: f32,
    gap: f32,
    stop: f32,
}

static WAVE: Shader<Wave> = Shader::new(
    "wavy_progress",
    include_str!("../../shaders/wavy_progress.wgsl"),
);

pub(super) fn preload_shaders() {
    WAVE.preload();
    preload_more_shaders();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&WAVE)?;
    check_more_shaders()
}

/// Whether a progress shows as a wave: not at the very start and end, as in Compose.
fn waves_at(progress: f32) -> bool {
    progress > 0.1 && progress < 0.95
}

/// Keeps the phase moving, one wavelength per second, for as long as there is a wave to move.
fn keep_moving(phase: Signal<f32>, wave: Animated<f32>, ticking: Signal<bool>, start: Instant) {
    request_frame(move |now| {
        if !phase.is_alive() {
            return;
        }
        if wave.get_untracked() <= 0.0 {
            ticking.set(false);
            return;
        }
        let seconds = now.saturating_duration_since(start).as_secs_f32();
        phase.set(seconds.fract() * std::f32::consts::TAU);
        keep_moving(phase, wave, ticking, start);
    });
}

/// Keeps the seconds of a circle's wave going while it has one.
fn keep_turning(time: Signal<f32>, wave: Animated<f32>, ticking: Signal<bool>, start: Instant) {
    request_frame(move |now| {
        if !time.is_alive() {
            return;
        }
        if wave.get_untracked() <= 0.0 {
            ticking.set(false);
            return;
        }
        time.set(now.saturating_duration_since(start).as_secs_f32());
        keep_turning(time, wave, ticking, start);
    });
}

/// A progress bar with a wavy line, for a value from 0 to 1. It takes the whole width it is given
/// when styled with `.w_full()`.
pub fn wavy_progress(progress: impl Fn() -> f32 + 'static) -> Div {
    crate::ensure_ready();
    let first = progress().clamp(0.0, 1.0);
    let value = animated(first);
    let wave = animated(if waves_at(first) { 1.0f32 } else { 0.0 });
    let phase = signal(0.0f32);
    let ticking = signal(false);
    let start = Instant::now();
    let last = Cell::new(first);
    effect(move || {
        let target = progress().clamp(0.0, 1.0);
        let before = last.replace(target);
        if target == before {
            return;
        }
        untrack(|| {
            value.to(target, Transition::ms(500).linear());
            if waves_at(target) != waves_at(before) {
                wave.to(
                    if waves_at(target) { 1.0 } else { 0.0 },
                    Transition::ms(500).ease_in_out(),
                );
            }
        });
    });
    effect(move || {
        if wave.get() > 0.0 && !ticking.get_untracked() {
            ticking.set(true);
            keep_moving(phase, wave, ticking, start);
        }
    });
    div()
        .access(move |a| told_progress(a, value.get()))
        .w(WIDTH)
        .h(HEIGHT)
        .flex_none()
        .fill(move || {
            let c = color_scheme();
            WAVE.params(Wave {
                line: c.primary,
                track: c.secondary_container,
                progress: value.get(),
                wave: wave.get(),
                phase: phase.get(),
                stroke: STROKE,
                amplitude: AMPLITUDE,
                wavelength: WAVELENGTH,
                gap: GAP,
                stop: STOP,
            })
        })
}

const FLAT_HEIGHT: f32 = 4.0;
const CIRCLE: f32 = 40.0;
const WAVY_CIRCLE: f32 = 48.0;
const CIRCLE_WAVELENGTH: f32 = 15.0;
const CIRCLE_AMPLITUDE: f32 = 1.6;
const INDETERMINATE_WAVELENGTH: f32 = 20.0;
/// How a value moves when nothing says otherwise: `ProgressAnimationSpec` of Compose.
fn slow() -> Spring {
    Spring::from_ratio(50.0, 1.0)
}

#[derive(ShaderParams, Clone, Copy)]
struct Line {
    line: Color,
    track: Color,
    tail0: f32,
    head0: f32,
    tail1: f32,
    head1: f32,
    piece0_a: f32,
    piece0_b: f32,
    piece1_a: f32,
    piece1_b: f32,
    piece2_a: f32,
    piece2_b: f32,
    stroke: f32,
    amplitude: f32,
    wavelength: f32,
    phase: f32,
    stop: f32,
}

static LINE: Shader<Line> = Shader::new(
    "line_progress",
    include_str!("../../shaders/line_progress.wgsl"),
);

#[derive(ShaderParams, Clone, Copy)]
struct Circular {
    line: Color,
    track: Color,
    start: f32,
    sweep: f32,
    track_start: f32,
    track_sweep: f32,
    stroke: f32,
    amplitude: f32,
    waves: f32,
    phase: f32,
}

static CIRCULAR: Shader<Circular> = Shader::new(
    "circular_progress",
    include_str!("../../shaders/circular_progress.wgsl"),
);

pub(super) fn preload_more_shaders() {
    LINE.preload();
    CIRCULAR.preload();
}

pub(super) fn check_more_shaders() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&LINE)?;
    anti_webui::wgpu::check_shader(&CIRCULAR)
}

/// A straight progress bar, for a value from 0 to 1.
pub fn linear_progress(progress: impl Fn() -> f32 + 'static) -> Div {
    crate::ensure_ready();
    let progress = Rc::new(progress);
    let told = progress.clone();
    let value = follow(move || progress().clamp(0.0, 1.0), slow());
    div()
        .access(move |a| told_progress(a, told()))
        .w(WIDTH)
        .h(FLAT_HEIGHT)
        .flex_none()
        .fill(move || {
            let c = color_scheme();
            let p = value.get().clamp(0.0, 1.0);
            // The gap goes with the ends, which are round: 8 of the 240 by default.
            let gap = 8.0 / WIDTH;
            LINE.params(Line {
                line: c.primary,
                track: c.secondary_container,
                tail0: 0.0,
                head0: p,
                tail1: 0.0,
                head1: 0.0,
                piece0_a: p + p.min(gap),
                piece0_b: 1.0,
                piece1_a: 0.0,
                piece1_b: 0.0,
                piece2_a: 0.0,
                piece2_b: 0.0,
                stroke: FLAT_HEIGHT,
                amplitude: 0.0,
                wavelength: WAVELENGTH,
                phase: 0.0,
                stop: FLAT_HEIGHT,
            })
        })
}

/// How far a value that moves from `delay` for `duration`, both in milliseconds, has got at `ms`.
fn eased_from(ms: f32, delay: f32, duration: f32) -> f32 {
    Easing::CubicBezier(0.3, 0.0, 0.8, 0.15).apply(((ms - delay) / duration).clamp(0.0, 1.0))
}

fn indeterminate_line(height: f32, amplitude: f32, wavelength: f32) -> Div {
    crate::ensure_ready();
    let time = clock();
    div().w(WIDTH).h(height).flex_none().fill(move || {
        let c = color_scheme();
        let seconds = time.get();
        let ms = (seconds * 1000.0) % 1750.0;
        let head0 = eased_from(ms, 0.0, 1000.0);
        let tail0 = eased_from(ms, 250.0, 1000.0);
        let head1 = eased_from(ms, 650.0, 850.0);
        let tail1 = eased_from(ms, 900.0, 850.0);
        let gap = 8.0 / WIDTH;
        let (a0, b0) = if head0 < 1.0 - gap {
            (if head0 > 0.0 { head0 + gap } else { 0.0 }, 1.0)
        } else {
            (0.0, 0.0)
        };
        let (a1, b1) = if tail0 > gap {
            (
                if head1 > 0.0 { head1 + gap } else { 0.0 },
                if tail0 < 1.0 { tail0 - gap } else { 1.0 },
            )
        } else {
            (0.0, 0.0)
        };
        let (a2, b2) = if tail1 > gap {
            (0.0, if tail1 < 1.0 { tail1 - gap } else { 1.0 })
        } else {
            (0.0, 0.0)
        };
        LINE.params(Line {
            line: c.primary,
            track: c.secondary_container,
            tail0,
            head0,
            tail1,
            head1,
            piece0_a: a0,
            piece0_b: b0,
            piece1_a: a1,
            piece1_b: b1,
            piece2_a: a2,
            piece2_b: b2,
            stroke: FLAT_HEIGHT,
            amplitude,
            wavelength,
            phase: seconds.fract() * std::f32::consts::TAU,
            stop: 0.0,
        })
    })
}

/// A straight progress bar for something that takes as long as it takes.
pub fn indeterminate_linear_progress() -> Div {
    indeterminate_line(FLAT_HEIGHT, 0.0, WAVELENGTH)
}

/// A wavy progress bar for something that takes as long as it takes.
pub fn indeterminate_wavy_progress() -> Div {
    indeterminate_line(HEIGHT, AMPLITUDE, INDETERMINATE_WAVELENGTH)
}

const TO_RADIANS: f32 = std::f32::consts::PI / 180.0;
const TOP: f32 = 270.0 * TO_RADIANS;

/// The radians of the gap on each side of what is shown, with round ends.
fn gap_sweep(size: f32) -> f32 {
    8.0 / (std::f32::consts::PI * size) * std::f32::consts::TAU
}

fn determinate_circle(progress: impl Fn() -> f32 + 'static, size: f32, wavy: bool) -> Div {
    crate::ensure_ready();
    let progress = Rc::new(progress);
    let target = progress.clone();
    let value = if wavy {
        let first = untrack(|| target().clamp(0.0, 1.0));
        let value = animated(first);
        let last = Cell::new(first);
        effect(move || {
            let now = target().clamp(0.0, 1.0);
            if last.replace(now) != now {
                untrack(|| value.to(now, Transition::ms(500).linear()));
            }
        });
        value
    } else {
        follow(move || target().clamp(0.0, 1.0), slow())
    };
    let wave = follow(
        move || {
            if wavy && waves_at(value.get()) {
                1.0f32
            } else {
                0.0
            }
        },
        default_effects(),
    );
    // Moves only while there is a wave to move, like the wavy bar.
    let time = signal(0.0f32);
    if wavy {
        let ticking = signal(false);
        let start = Instant::now();
        effect(move || {
            if wave.get() > 0.0 && !ticking.get_untracked() {
                ticking.set(true);
                keep_turning(time, wave, ticking, start);
            }
        });
    }
    let waves = (std::f32::consts::TAU * (size / 2.0 - 2.0) / CIRCLE_WAVELENGTH)
        .round()
        .max(5.0);
    div()
        .access(move |a| told_progress(a, value.get()))
        .w(size)
        .h(size)
        .flex_none()
        .fill(move || {
            let c = color_scheme();
            let sweep = value.get().clamp(0.0, 1.0) * std::f32::consts::TAU;
            let gap = gap_sweep(size).min(sweep);
            CIRCULAR.params(Circular {
                line: c.primary,
                track: c.secondary_container,
                start: TOP,
                sweep,
                track_start: TOP + sweep + gap,
                track_sweep: (std::f32::consts::TAU - sweep - gap * 2.0).max(0.0),
                stroke: 4.0,
                amplitude: CIRCLE_AMPLITUDE * wave.get().clamp(0.0, 1.0),
                waves,
                phase: time.get() * std::f32::consts::TAU / waves,
            })
        })
}

/// A circular progress indicator, for a value from 0 to 1.
pub fn circular_progress(progress: impl Fn() -> f32 + 'static) -> Div {
    determinate_circle(progress, CIRCLE, false)
}

/// A circular progress indicator with a wavy line, for a value from 0 to 1.
pub fn circular_wavy_progress(progress: impl Fn() -> f32 + 'static) -> Div {
    determinate_circle(progress, WAVY_CIRCLE, true)
}

/// Where an arc that comes and goes starts and how long it is, in radians, in its 6 second cycle.
fn spinning(seconds: f32) -> (f32, f32) {
    let ms = (seconds * 1000.0) % 6000.0;
    let period = (ms / 1500.0).floor();
    let extra = 90.0 * period + 90.0 * ((ms - 1500.0 * period) / 300.0).min(1.0);
    let rotation = 1080.0 * ms / 6000.0 + extra;
    let length = if ms < 3000.0 {
        0.1 + 0.77 * ms / 3000.0
    } else {
        0.87 - 0.77 * Easing::CubicBezier(0.2, 0.0, 0.0, 1.0).apply((ms - 3000.0) / 3000.0)
    };
    (rotation * TO_RADIANS, length * std::f32::consts::TAU)
}

fn indeterminate_circle(size: f32, wavy: bool) -> Div {
    crate::ensure_ready();
    let time = clock();
    let waves = (std::f32::consts::TAU * (size / 2.0 - 2.0) / CIRCLE_WAVELENGTH)
        .round()
        .max(5.0);
    div().w(size).h(size).flex_none().fill(move || {
        let c = color_scheme();
        let seconds = time.get();
        let (start, sweep) = spinning(seconds);
        let gap = gap_sweep(size).min(sweep);
        CIRCULAR.params(Circular {
            line: c.primary,
            track: if wavy {
                c.secondary_container
            } else {
                Color::TRANSPARENT
            },
            start,
            sweep,
            track_start: start + sweep + gap,
            track_sweep: (std::f32::consts::TAU - sweep - gap * 2.0).max(0.0),
            stroke: 4.0,
            amplitude: if wavy { CIRCLE_AMPLITUDE } else { 0.0 },
            waves,
            phase: seconds * std::f32::consts::TAU / waves,
        })
    })
}

/// A circular progress indicator for something that takes as long as it takes.
pub fn indeterminate_circular_progress() -> Div {
    indeterminate_circle(CIRCLE, false)
}

/// A wavy circular progress indicator for something that takes as long as it takes.
pub fn indeterminate_circular_wavy_progress() -> Div {
    indeterminate_circle(WAVY_CIRCLE, true)
}

/// What a progress indicator says to assistive technology.
fn told_progress(a: Access, value: f32) -> Access {
    a.role(Role::ProgressBar)
        .value(value.clamp(0.0, 1.0) as f64, 0.0, 1.0)
}
