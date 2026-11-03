//! The loading indicator: a shape that turns and keeps changing into the next. See
//! `docs/design/15-md3e.md`.

use crate::color::color_scheme;
use crate::shapes::{SAMPLES, Shapes, morph_spring, shapes};
use anti_webui::prelude::*;
use std::cell::OnceCell;
use std::time::Instant;

const CONTAINER: f32 = 48.0;
const ACTIVE: f32 = 38.0;
const MORPH_SECONDS: f32 = 0.65;
const TURN_SECONDS: f32 = 4.666;

const WORDS: usize = SAMPLES / 4;

#[derive(ShaderParams, Clone, Copy)]
struct Blob {
    color: Color,
    scale: f32,
    rotation: f32,
    table: [[f32; 4]; WORDS],
}

static BLOB: Shader<Blob> = Shader::new("loading", include_str!("../../shaders/loading.wgsl"));

pub(super) fn preload_shaders() {
    BLOB.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&BLOB)
}

thread_local! {
    static SHAPES: OnceCell<Shapes> = const { OnceCell::new() };
}

/// The shape and how it is turned, `seconds` after the start.
fn shape_at(seconds: f32) -> ([[f32; 4]; WORDS], f32) {
    SHAPES.with(|s| {
        let shapes = s.get_or_init(shapes);
        let count = shapes.tables.len();
        let step = (seconds / MORPH_SECONDS).floor();
        let progress = morph_spring(seconds - step * MORPH_SECONDS);
        let (from, to) = (
            &shapes.tables[step as usize % count],
            &shapes.tables[(step as usize + 1) % count],
        );
        let mut table = [[0.0; 4]; WORDS];
        for (i, (a, b)) in from.iter().zip(to).enumerate() {
            // What the spring overshoots by goes on past the next shape, as far as it can.
            table[i / 4][i % 4] = (a + (b - a) * progress).max(0.0);
        }
        let degrees = progress * 90.0 + (step + 1.0) * 90.0 + seconds / TURN_SECONDS * 360.0;
        (table, degrees.to_radians())
    })
}

fn the_fit() -> f32 {
    SHAPES.with(|s| s.get_or_init(shapes).fit)
}

fn keep_going(time: Signal<f32>, start: Instant) {
    request_frame(move |now| {
        if time.is_alive() {
            time.set(now.saturating_duration_since(start).as_secs_f32());
            keep_going(time, start);
        }
    });
}

/// Says that something is loading, for as long as it is there.
pub fn loading_indicator() -> Div {
    crate::ensure_ready();
    let time = signal(0.0f32);
    keep_going(time, Instant::now());
    let scale = the_fit() * ACTIVE / CONTAINER;
    div().w(CONTAINER).h(CONTAINER).flex_none().fill(move || {
        let (table, rotation) = shape_at(time.get());
        BLOB.params(Blob {
            color: color_scheme().primary,
            scale,
            rotation,
            table,
        })
    })
}
