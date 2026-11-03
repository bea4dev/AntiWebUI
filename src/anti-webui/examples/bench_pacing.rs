//! A window that keeps animating with a chosen load, to see what frames take on a real compositor
//! and whether refreshes get skipped (see `docs/design/10-frame-pacing.md`).
//!
//! ```text
//! ANTIWEBUI_FRAME_STATS=out.csv cargo run --release -p anti-webui --example bench_pacing -- \
//!     --seconds 5 --cpu-ms 10 --layers 40 --rows 2000
//! ```
//!
//! - `--cpu-ms`: milliseconds of busy waiting on the UI thread per frame.
//! - `--heavy`: seconds for which `--cpu-ms` applies, from the first frame (default: all of them).
//! - `--layers`: full-window layers with big shadows, repainted every frame. Load on the GPU.
//! - `--bg 0`, `--shadow 0`, `--radius N`: what those layers draw (default: both, radius 12).
//! - `--rows`: a list of that many rows, scrolled every frame. Real work for the CPU and GPU.
//! - `--on`, `--off`: seconds of load and of nothing changing, in turn, to see how frames come
//!   after a pause.
//! - `--linger`: seconds to stay open with nothing changing afterwards, to compare the last frame
//!   on the screen with `ANTIWEBUI_DUMP_FRAME`.

use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Load {
    seconds: f64,
    cpu: Duration,
    heavy: f64,
    layers: usize,
    rows: usize,
    linger: f64,
    bg: bool,
    shadow: bool,
    radius: f32,
    on: f64,
    off: f64,
}

fn parse_args() -> Load {
    let mut load = Load {
        seconds: 5.0,
        cpu: Duration::ZERO,
        heavy: f64::INFINITY,
        layers: 0,
        rows: 0,
        linger: 0.0,
        bg: true,
        shadow: true,
        radius: 12.0,
        on: 1.0,
        off: 0.0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        let value: f64 = args
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{name} needs a number"));
        match name.as_str() {
            "--seconds" => load.seconds = value,
            "--cpu-ms" => load.cpu = Duration::from_secs_f64(value / 1000.0),
            "--heavy" => load.heavy = value,
            "--layers" => load.layers = value as usize,
            "--rows" => load.rows = value as usize,
            "--linger" => load.linger = value,
            "--bg" => load.bg = value != 0.0,
            "--shadow" => load.shadow = value != 0.0,
            "--radius" => load.radius = value as f32,
            "--on" => load.on = value,
            "--off" => load.off = value,
            _ => panic!("unknown option {name}"),
        }
    }
    load
}

/// The same work every frame until the time is up, counted from the first frame.
fn tick(
    load: Load,
    started: Option<Instant>,
    phase: Signal<f32>,
    frame: Signal<u32>,
    scroller: NodeRef,
) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        let elapsed = now.saturating_duration_since(started).as_secs_f64();
        if elapsed >= load.seconds + load.linger {
            with_doc(|d| d.request(PlatformRequest::Close));
            return;
        }
        if elapsed >= load.seconds || elapsed % (load.on + load.off) >= load.on {
            tick(load, Some(started), phase, frame, scroller);
            return;
        }
        let spin = Instant::now();
        while elapsed < load.heavy && spin.elapsed() < load.cpu {
            std::hint::spin_loop();
        }
        phase.set(elapsed as f32);
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
        tick(load, Some(started), phase, frame, scroller);
    });
}

fn main() {
    let load = parse_args();
    App::new()
        .title("bench_pacing")
        .size(900, 900)
        .run(move || {
            let phase = signal(0.0_f32);
            let frame = signal(0_u32);
            let scroller = node_ref();
            tick(load, None, phase, frame, scroller);
            let wave = move || (phase.get() * 3.0).sin() * 0.5 + 0.5;
            let layers = (0..load.layers).map(move |i| {
                div()
                    .absolute()
                    .inset(30.0 + i as f32 % 8.0)
                    .rounded(load.radius)
                    .dyn_style(move |s| {
                        // Different in 8 bits every frame: a browser skips the repaint otherwise.
                        let f = frame.get();
                        let red = (51 + f % 64) as f32 / 255.0;
                        let shadow_red = (f % 64) as f32 / 255.0;
                        let s = if load.bg {
                            s.bg(Color::new(red, 0.4, 0.8, 0.05))
                        } else {
                            s
                        };
                        if !load.shadow {
                            return s;
                        }
                        s.shadow(BoxShadow {
                            offset_y: 4.0,
                            blur: 60.0,
                            color: Color::new(shadow_red, 0.0, 0.0, 0.05),
                            ..Default::default()
                        })
                    })
            });
            let rows = (load.rows > 0).then(|| {
                div()
                    .h_full()
                    .overflow_y_scroll()
                    .node_ref(scroller)
                    .children((0..load.rows).map(|i| {
                        div()
                            .h(40.0)
                            .px(12.0)
                            .flex()
                            .items_center()
                            .gap(8.0)
                            .bg(if i % 2 == 0 {
                                rgb(0xf4f6f8)
                            } else {
                                rgb(0xffffff)
                            })
                            .child(div().w(16.0).h(16.0).rounded(8.0).bg(rgb(0x4477cc)))
                            .child(format!("row {i}: some text to shape and draw"))
                    }))
            });
            div()
                .size_full()
                .relative()
                .bg(Color::WHITE)
                .children(rows)
                .children(layers)
                // Something changes every frame even without other load.
                .child(
                    div()
                        .absolute()
                        .top(8.0)
                        .w(24.0)
                        .h(24.0)
                        .bg(rgb(0xcc4444))
                        .dyn_style(move |s| s.left(8.0 + wave() * 400.0)),
                )
        })
        .unwrap();
}
