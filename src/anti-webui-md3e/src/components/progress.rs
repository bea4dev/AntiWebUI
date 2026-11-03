//! The wavy progress bar. See `docs/design/15-md3e.md`.

use crate::color::color_scheme;
use anti_webui::prelude::*;
use std::cell::Cell;
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
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&WAVE)
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
    div().w(WIDTH).h(HEIGHT).flex_none().fill(move || {
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
