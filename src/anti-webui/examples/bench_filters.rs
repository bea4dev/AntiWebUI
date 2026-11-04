//! What filters and backdrops cost: a window with text and shapes behind and in them (see
//! `docs/design/16-filters.md`).
//!
//! ```text
//! ANTIWEBUI_FRAME_STATS=out.csv cargo run --release -p anti-webui --example bench_filters -- \
//!     --seconds 8 --mode backdrop-moves --radius 20
//! ```
//!
//! - `--mode`:
//!   - `backdrop-moves`: a panel with a backdrop blur moves over what is behind it.
//!   - `behind-moves`: what is behind a panel moves under it.
//!   - `scroll-under`: a list scrolls under a header with a backdrop blur.
//!   - `apart`: something moves far from the panel, which needs nothing done again.
//!   - `filter-radius`: the radius of a blur changes on a card.
//!   - `filter-moves`: a blurred card moves, which needs no blurring again.
//!   - `filter-content`: what is in a blurred card changes.
//!   - `none`: only the same thing moving, as a reference.
//! - `--radius`: logical pixels.
//! - `--count`: how many panels side by side.
//! - `--width`, `--height`: of each panel, in logical pixels.

use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    BackdropMoves,
    BehindMoves,
    ScrollUnder,
    Apart,
    FilterRadius,
    FilterMoves,
    FilterContent,
    None,
}

#[derive(Clone, Copy)]
struct Load {
    seconds: f64,
    mode: Mode,
    radius: f32,
    count: usize,
    width: f32,
    height: f32,
}

fn parse_args() -> Load {
    let mut load = Load {
        seconds: 8.0,
        mode: Mode::None,
        radius: 20.0,
        count: 1,
        width: 400.0,
        height: 240.0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("{name} needs a value"));
        match name.as_str() {
            "--seconds" => load.seconds = value.parse().unwrap(),
            "--radius" => load.radius = value.parse().unwrap(),
            "--count" => load.count = value.parse().unwrap(),
            "--width" => load.width = value.parse().unwrap(),
            "--height" => load.height = value.parse().unwrap(),
            "--mode" => {
                load.mode = match value.as_str() {
                    "backdrop-moves" => Mode::BackdropMoves,
                    "behind-moves" => Mode::BehindMoves,
                    "scroll-under" => Mode::ScrollUnder,
                    "apart" => Mode::Apart,
                    "filter-radius" => Mode::FilterRadius,
                    "filter-moves" => Mode::FilterMoves,
                    "filter-content" => Mode::FilterContent,
                    "none" => Mode::None,
                    other => panic!("unknown mode {other}"),
                }
            }
            other => panic!("unknown option {other}"),
        }
    }
    load
}

fn tick(
    load: Load,
    started: Option<Instant>,
    time: Signal<f32>,
    frame: Signal<u32>,
    scroller: NodeRef,
) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        let elapsed = now.saturating_duration_since(started).as_secs_f64();
        if elapsed >= load.seconds {
            with_doc(|d| d.request(PlatformRequest::Close));
            return;
        }
        time.set(elapsed as f32);
        frame.update(|f| *f += 1);
        if let Some(id) = scroller.get() {
            with_doc(|d| {
                if let Some(n) = d.get_mut(id) {
                    let max = (n.content_size.height - n.padding_abs().height).max(1);
                    n.scroll.y = (elapsed * 1500.0) as i32 % max;
                    d.mark_scrolled(id);
                }
            });
        }
        tick(load, Some(started), time, frame, scroller);
    });
}

/// Lines and shapes, the same for all modes, drawn once.
fn scenery() -> Div {
    let colors = [0xe45756, 0x4c78a8, 0xf58518, 0x72b7b2, 0x54a24b, 0xeeca3b];
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(900.0)
        .h(900.0)
        .bg(rgb(0x20304a))
        .children((0..30).map(move |i| {
            div()
                .absolute()
                .left(10.0 + (i % 6) as f32 * 150.0)
                .top(10.0 + (i / 6) as f32 * 170.0)
                .w(120.0)
                .h(120.0)
                .rounded(if i % 2 == 0 { 60.0 } else { 10.0 })
                .bg(rgb(colors[i % 6]))
        }))
        .children((0..30).map(|i| {
            div()
                .absolute()
                .left(20.0)
                .top(20.0 + i as f32 * 28.0)
                .text_color(rgb(0xffffff))
                .child(format!(
                    "line {i}: the quick brown fox jumps over the lazy dog"
                ))
        }))
}

fn main() {
    let load = parse_args();
    App::new()
        .title("bench_filters")
        .size(900, 900)
        .run(move || {
            let time = signal(0.0_f32);
            let frame = signal(0_u32);
            let scroller = node_ref();
            tick(load, None, time, frame, scroller);
            let wave = move || (time.get() * 2.0).sin() * 0.5 + 0.5;
            let mode = load.mode;
            let panels = (0..load.count).map(move |i| {
                let at = 20.0 + i as f32 * (load.width + 10.0);
                let panel = div()
                    .absolute()
                    .top(100.0)
                    .w(load.width)
                    .h(load.height)
                    .rounded(16.0);
                match mode {
                    Mode::BackdropMoves => panel
                        .bg(rgba(0xffffff30))
                        .backdrop_blur(load.radius)
                        .dyn_style(move |s| s.left(at + wave() * 400.0)),
                    Mode::BehindMoves | Mode::Apart => panel
                        .left(at)
                        .bg(rgba(0xffffff30))
                        .backdrop_blur(load.radius),
                    Mode::FilterRadius => panel
                        .left(at)
                        .bg(rgb(0x4c78a8))
                        .child("text in the card")
                        .filter(move || Image::content().blur(1.0 + wave() * load.radius)),
                    Mode::FilterMoves => panel
                        .left(at)
                        .bg(rgb(0x4c78a8))
                        .child("text in the card")
                        .blur(load.radius)
                        .dyn_style(move |s| s.translate_x(wave() * 400.0)),
                    Mode::FilterContent => {
                        panel.left(at).bg(rgb(0x4c78a8)).blur(load.radius).child(
                            div()
                                .w(40.0)
                                .h(40.0)
                                .bg(rgb(0xffffff))
                                .dyn_style(move |s| s.ml(wave() * (load.width - 40.0))),
                        )
                    }
                    Mode::ScrollUnder => panel,
                    Mode::None => panel
                        .bg(rgb(0x4c78a8))
                        .dyn_style(move |s| s.left(at + wave() * 400.0)),
                }
            });
            let scroll_list = (mode == Mode::ScrollUnder).then(|| {
                div()
                    .size_full()
                    .overflow_y_scroll()
                    .node_ref(scroller)
                    .children((0..2000).map(|i| {
                        div()
                            .h(40.0)
                            .flex_none()
                            .px(12.0)
                            .flex()
                            .items_center()
                            .bg(if i % 2 == 0 {
                                rgb(0xf4f6f8)
                            } else {
                                rgb(0xffffff)
                            })
                            .child(format!("row {i}: some text to shape and draw"))
                    }))
            });
            let header = (mode == Mode::ScrollUnder).then(|| {
                div()
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .w(900.0)
                    .h(load.height)
                    .bg(rgba(0xffffff60))
                    .backdrop_blur(load.radius)
            });
            let moving = matches!(mode, Mode::BehindMoves | Mode::Apart).then(|| {
                let top = if mode == Mode::Apart { 800.0 } else { 200.0 };
                div()
                    .absolute()
                    .w(120.0)
                    .h(80.0)
                    .bg(rgb(0xffffff))
                    .top(top)
                    .dyn_style(move |s| s.left(wave() * 700.0))
            });
            div()
                .size_full()
                .relative()
                .bg(Color::WHITE)
                .child(scenery())
                .children(moving)
                .children(scroll_list)
                .children(header)
                .children(panels)
        })
        .unwrap();
}
