//! What custom shaders cost: a window of boxes that change every frame, drawn with the standard
//! `bg` or with a shader (see `docs/design/14-shaders.md`).
//!
//! ```text
//! ANTIWEBUI_FRAME_STATS=out.csv cargo run --release -p anti-webui --example bench_shaders -- \
//!     --seconds 8 --kind same --count 1000
//! ```
//!
//! - `--kind plain`: the standard `bg`, changed with `dyn_style`.
//! - `--kind same`: one shader that returns a color.
//! - `--kind alt`: two such shaders in turn, so that the pipeline changes at every box.
//! - `--kind heavy`: one shader that computes something on every pixel.
//! - `--count`, `--size`: how many boxes, and their side in logical pixels.
//! - `--stack 1`: all the boxes in one place, to see what a pixel costs.
//! - `--rows`: that many rows that stay the same around them, drawn in the same layer.
//! - `--first-use`: seconds after which the first shader appears, to see the frame that compiles
//!   it. Nothing is preloaded.
//! - `--offscreen`: no window. That many renders of one frame, to compare what the GPU takes on each
//!   adapter (`VK_LOADER_DRIVERS_SELECT=*radeon*` chooses one). Includes reading the image back.

use anti_webui::core::Ui;
use anti_webui::core::event::WindowEvent;
use anti_webui::core::geometry::{Scale, SizeI};
use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Renderer, wgpu};
use std::time::Instant;

#[derive(ShaderParams, Clone, Copy)]
struct Tint {
    color: Color,
}

#[derive(ShaderParams, Clone, Copy)]
struct Wobble {
    time: f32,
    color: Color,
}

const TINT_CODE: &str = "fn paint(f: Frag) -> vec4<f32> { return params.color; }";
static TINT_A: Shader<Tint> = Shader::new("tint_a", TINT_CODE);
static TINT_B: Shader<Tint> = Shader::new("tint_b", TINT_CODE);
static WOBBLE: Shader<Wobble> = Shader::new(
    "wobble",
    "fn paint(f: Frag) -> vec4<f32> {
        let size = vec2<f32>(aw_box.size);
        let p = f.px / max(size.x, size.y) * 8.0;
        var v = 0.0;
        for (var i = 1; i <= 4; i++) {
            let k = f32(i);
            v += sin(p.x * k + params.time) * cos(p.y * k - params.time * 0.5) / k;
        }
        let d = length(f.px - size * 0.5) - min(size.x, size.y) * (0.35 + 0.05 * v);
        return params.color * (0.25 + 0.75 * aw_coverage(d, f.pixel));
    }",
);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Same,
    Alt,
    Heavy,
}

#[derive(Clone, Copy)]
struct Load {
    seconds: f64,
    kind: Kind,
    count: usize,
    size: f32,
    rows: usize,
    first_use: Option<f64>,
    offscreen: usize,
    stack: bool,
}

fn parse_args() -> Load {
    let mut load = Load {
        seconds: 8.0,
        kind: Kind::Same,
        count: 100,
        size: 20.0,
        rows: 0,
        first_use: None,
        offscreen: 0,
        stack: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("{name} needs a value"));
        match name.as_str() {
            "--seconds" => load.seconds = value.parse().unwrap(),
            "--count" => load.count = value.parse().unwrap(),
            "--size" => load.size = value.parse().unwrap(),
            "--rows" => load.rows = value.parse().unwrap(),
            "--stack" => load.stack = value != "0",
            "--offscreen" => load.offscreen = value.parse().unwrap(),
            "--first-use" => load.first_use = Some(value.parse().unwrap()),
            "--kind" => {
                load.kind = match value.as_str() {
                    "plain" => Kind::Plain,
                    "same" => Kind::Same,
                    "alt" => Kind::Alt,
                    "heavy" => Kind::Heavy,
                    other => panic!("unknown kind {other}"),
                }
            }
            other => panic!("unknown option {other}"),
        }
    }
    load
}

fn tick(load: Load, started: Option<Instant>, frame: Signal<u32>, shown: Signal<bool>) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        let elapsed = now.saturating_duration_since(started).as_secs_f64();
        if elapsed >= load.seconds {
            with_doc(|d| d.request(PlatformRequest::Close));
            return;
        }
        match load.first_use {
            Some(at) => {
                if elapsed >= at && !shown.get_untracked() {
                    shown.set(true);
                }
            }
            None => shown.set(true),
        }
        frame.update(|f| *f += 1);
        tick(load, Some(started), frame, shown);
    });
}

/// Different in 8 bits every frame and for every box: a repaint is never skipped.
fn color_of(frame: u32, i: usize) -> Color {
    let red = ((frame as usize + i) % 128 + 64) as f32 / 255.0;
    Color::new(red, 0.4, 0.8, 1.0)
}

fn offscreen(load: Load) {
    let gpu = Gpu::headless().unwrap();
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let mut ui = Ui::new(move || page(load));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1620, 1620),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    let scene = ui.paint(Instant::now());
    let image = gpu.render_to_rgba(&mut renderer, &scene);
    // Something that was not drawn takes no time, so make sure that it was.
    let painted = image
        .chunks(4)
        .filter(|p| p[..3] != [255, 255, 255])
        .count();
    let mut times: Vec<f64> = (0..load.offscreen)
        .map(|_| {
            let start = Instant::now();
            let _ = gpu.render_to_rgba(&mut renderer, &scene);
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(f64::total_cmp);
    println!(
        "{}: {painted} pixels painted, p50 {:.2} ms, p95 {:.2} ms",
        gpu.adapter.get_info().name,
        times[times.len() / 2],
        times[times.len() * 95 / 100],
    );
}

fn main() {
    let load = parse_args();
    if load.offscreen > 0 {
        offscreen(load);
        return;
    }
    App::new()
        .title("bench_shaders")
        .size(900, 900)
        .run(move || page(load))
        .unwrap();
}

fn page(load: Load) -> Div {
    let frame = signal(0_u32);
    let shown = signal(load.first_use.is_none());
    if load.offscreen == 0 {
        tick(load, None, frame, shown);
    }
    let boxes = (0..load.count).map(move |i| {
        // The window is 900 wide; the rows go on below it if there are too many.
        let per_row = ((880.0 / (load.size + 1.0)) as usize).max(1);
        let b = div()
            .absolute()
            .left(if load.stack {
                0.0
            } else {
                (i % per_row) as f32 * (load.size + 1.0)
            })
            .top(if load.stack {
                0.0
            } else {
                (i / per_row) as f32 * (load.size + 1.0)
            })
            .w(load.size)
            .h(load.size)
            .rounded(4.0);
        match load.kind {
            Kind::Plain => b.dyn_style(move |s| s.bg(color_of(frame.get(), i))),
            Kind::Same => b.fill(move || {
                shown.get().then(|| {
                    TINT_A.params(Tint {
                        color: color_of(frame.get(), i),
                    })
                })
            }),
            Kind::Alt => b.fill(move || {
                shown.get().then(|| {
                    let color = color_of(frame.get(), i);
                    if i % 2 == 0 {
                        TINT_A.params(Tint { color })
                    } else {
                        TINT_B.params(Tint { color })
                    }
                })
            }),
            Kind::Heavy => b.fill(move || {
                shown.get().then(|| {
                    let f = frame.get();
                    WOBBLE.params(Wobble {
                        time: f as f32 * 0.02,
                        color: color_of(f, i),
                    })
                })
            }),
        }
    });
    let rows = (0..load.rows).map(|i| {
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
    });
    div()
        .size_full()
        .relative()
        .bg(Color::WHITE)
        .child(div().h_full().overflow_y_scroll().children(rows))
        .children(boxes)
}
