//! What moving along the axes of a font costs (see `docs/design/17-font-axes.md`).
//!
//! ```text
//! ANTIWEBUI_FRAME_STATS=out.csv cargo run --release -p anti-webui --example bench_font_axes -- \
//!     --seconds 8 --axis wght --lines 1 --chars 20
//! ```
//!
//! - `--axis`: `wght` makes the text wider, `GRAD` doesn't, `none` moves a box instead, and
//!   `random` jumps around two axes, which keeps making new glyph images.
//! - `--lines`, `--chars`: how much text moves.
//! - `--still`: how many lines that don't move are around it.

use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::time::Instant;

static FONT: &[u8] = include_bytes!("../../anti-webui-md3e/fonts/RobotoFlex.ttf");

#[derive(Clone, Copy)]
struct Load {
    seconds: f64,
    axis: &'static str,
    lines: usize,
    chars: usize,
    still: usize,
}

fn parse_args() -> Load {
    let mut load = Load {
        seconds: 8.0,
        axis: "wght",
        lines: 1,
        chars: 20,
        still: 40,
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("{name} needs a value"));
        match name.as_str() {
            "--seconds" => load.seconds = value.parse().unwrap(),
            "--axis" => load.axis = value.leak(),
            "--lines" => load.lines = value.parse().unwrap(),
            "--chars" => load.chars = value.parse().unwrap(),
            "--still" => load.still = value.parse().unwrap(),
            other => panic!("unknown option {other}"),
        }
    }
    load
}

fn tick(load: Load, started: Option<Instant>, time: Signal<f32>) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        let elapsed = now.saturating_duration_since(started).as_secs_f64();
        if elapsed >= load.seconds {
            with_doc(|d| d.request(PlatformRequest::Close));
            return;
        }
        time.set(elapsed as f32);
        tick(load, Some(started), time);
    });
}

fn text_of(chars: usize) -> String {
    "The quick brown fox jumps over the lazy dog. "
        .chars()
        .cycle()
        .take(chars)
        .collect()
}

fn main() {
    let load = parse_args();
    App::new()
        .title("bench_font_axes")
        .size(900, 900)
        .run(move || {
            register_font(FONT).unwrap();
            let time = signal(0.0_f32);
            tick(load, None, time);
            let moving = move |s: Style| {
                let t = time.get();
                let wave = 0.5 + 0.5 * (t * 3.0).sin();
                match load.axis {
                    "wght" => s.font_axis(b"wght", 300.0 + wave * 500.0),
                    "GRAD" => s.font_axis(b"GRAD", wave * 150.0),
                    "random" => s
                        .font_axis(b"GRAD", (t * 37.0).sin() * 150.0)
                        .font_axis(b"XTRA", 468.0 + (t * 53.0).sin() * 130.0),
                    _ => s.translate_x(wave * 100.0),
                }
            };
            div()
                .size_full()
                .bg(rgb(0xfafafa))
                .text_color(rgb(0x1a1c1e))
                .font_family("Roboto Flex")
                .text_size(16.0)
                .children(
                    (0..load.lines)
                        .map(move |_| div().flex().dyn_style(moving).child(text_of(load.chars))),
                )
                .children(
                    (0..load.still)
                        .map(|i| div().child(format!("A line that stays where it is, number {i}"))),
                )
        })
        .unwrap();
}
