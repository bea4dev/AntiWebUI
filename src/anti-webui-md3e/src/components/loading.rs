//! The loading indicator: a shape that turns and keeps changing into the next. See
//! `docs/design/15-md3e.md`.

use super::shape::paint_table;
use crate::color::color_scheme;
use crate::shapes::{MaterialShape, fit, morph_spring, table};
use anti_webui::prelude::*;
use std::cell::OnceCell;
use std::time::Instant;

const CONTAINER: f32 = 48.0;
const ACTIVE: f32 = 38.0;
const MORPH_SECONDS: f32 = 0.65;
const TURN_SECONDS: f32 = 4.666;

/// The shapes it goes through, in order.
const SHAPES: [MaterialShape; 7] = [
    MaterialShape::SoftBurst,
    MaterialShape::Cookie9Sided,
    MaterialShape::Pentagon,
    MaterialShape::Pill,
    MaterialShape::Sunny,
    MaterialShape::Cookie4Sided,
    MaterialShape::Oval,
];

thread_local! {
    static FIT: OnceCell<f32> = const { OnceCell::new() };
}

/// The shape and how it is turned, `seconds` after the start.
fn paint_at(seconds: f32, scale: f32) -> ShaderPaint {
    let step = (seconds / MORPH_SECONDS).floor();
    let progress = morph_spring(seconds - step * MORPH_SECONDS);
    let from = table(SHAPES[step as usize % SHAPES.len()]);
    let to = table(SHAPES[(step as usize + 1) % SHAPES.len()]);
    // What the spring overshoots by goes on past the next shape, as far as it can.
    let shape = from.mix(&to, progress);
    let degrees = progress * 90.0 + (step + 1.0) * 90.0 + seconds / TURN_SECONDS * 360.0;
    paint_table(
        &shape,
        color_scheme().primary,
        scale,
        degrees.to_radians(),
        0.0,
    )
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
    let scale = FIT.with(|f| *f.get_or_init(|| fit(&SHAPES))) * ACTIVE / CONTAINER;
    div()
        .w(CONTAINER)
        .h(CONTAINER)
        .flex_none()
        .fill(move || paint_at(time.get(), scale))
}
