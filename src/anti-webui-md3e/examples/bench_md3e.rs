//! What the components cost at run time and when they first appear.
//!
//! ```text
//! ANTIWEBUI_FRAME_STATS=out.csv cargo run --release -p anti-webui-md3e --example bench_md3e -- \
//!     --seconds 8 --loaders 1
//! cargo run --release -p anti-webui-md3e --example bench_md3e -- --compile 1
//! ```
//!
//! - `--loaders`, `--waves`, `--buttons`: how many of each. Only buttons: no frame is asked for and
//!   nothing closes the window, so whoever runs it ends it.
//! - `--compile 1`: no window. Prints how long the first frames take without and with the
//!   components, which is when their shaders compile, and the frames of a press on a page of 30
//!   buttons. Run it in a new process for each number.

use anti_webui::core::Ui;
use anti_webui::core::event::WindowEvent;
use anti_webui::core::event::{Modifiers, PointerButton};
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Renderer, wgpu};
use anti_webui_md3e::components::{button, loading_indicator, wavy_progress};
use std::time::Instant;

#[derive(Clone, Copy)]
struct Load {
    seconds: f64,
    loaders: usize,
    waves: usize,
    buttons: usize,
    compile: bool,
}

fn parse_args() -> Load {
    let mut load = Load {
        seconds: 8.0,
        loaders: 0,
        waves: 0,
        buttons: 0,
        compile: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        let value: f64 = args
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{name} needs a number"));
        match name.as_str() {
            "--seconds" => load.seconds = value,
            "--loaders" => load.loaders = value as usize,
            "--waves" => load.waves = value as usize,
            "--buttons" => load.buttons = value as usize,
            "--compile" => load.compile = value != 0.0,
            other => panic!("unknown option {other}"),
        }
    }
    load
}

fn page(load: Load) -> Div {
    let row = || div().flex().flex_row().items_center().gap(12.0);
    let rows = |count: usize, per_row: usize, item: fn(usize) -> Div| {
        (0..count.div_ceil(per_row))
            .map(move |r| row().children((r * per_row..((r + 1) * per_row).min(count)).map(item)))
    };
    div()
        .flex()
        .flex_col()
        .gap(12.0)
        .p(12.0)
        .size_full()
        .bg(Color::WHITE)
        .children(rows(load.buttons, 7, |i| button(format!("Button {i}"))))
        .children(rows(load.waves, 3, |_| wavy_progress(|| 0.5)))
        .children(rows(load.loaders, 14, |_| loading_indicator()))
}

fn close_after(load: Load, started: Option<Instant>) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        if now.saturating_duration_since(started).as_secs_f64() >= load.seconds {
            with_doc(|d| d.request(PlatformRequest::Close));
            return;
        }
        close_after(load, Some(started));
    });
}

fn timed_render(ui: &mut Ui, gpu: &Gpu, renderer: &mut Renderer) -> f64 {
    let now = Instant::now();
    ui.animate(now);
    ui.update();
    let scene = ui.paint(now);
    let start = Instant::now();
    let _ = gpu.render_to_rgba(renderer, &scene);
    start.elapsed().as_secs_f64() * 1e3
}

fn compile_times() {
    let (w, h) = (1620, 1296);
    let start = Instant::now();
    let gpu = Gpu::headless().unwrap();
    let device = start.elapsed().as_secs_f64() * 1e3;
    let start = Instant::now();
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let renderer_new = start.elapsed().as_secs_f64() * 1e3;
    let size = |ui: &mut Ui| {
        ui.handle_event(WindowEvent::Resized {
            size: SizeI::new(w, h),
            scale: Scale::from_f64(1.8),
        });
        ui.update();
    };
    let mut plain = Ui::new(|| div().size_full().bg(Color::WHITE).child("Plain page"));
    size(&mut plain);
    let plain_first = timed_render(&mut plain, &gpu, &mut renderer);
    let plain_second = timed_render(&mut plain, &gpu, &mut renderer);

    let start = Instant::now();
    let mut ui = Ui::new(|| {
        page(Load {
            seconds: 0.0,
            loaders: 1,
            waves: 1,
            buttons: 5,
            compile: true,
        })
    });
    size(&mut ui);
    let build = start.elapsed().as_secs_f64() * 1e3;
    let first = timed_render(&mut ui, &gpu, &mut renderer);
    let second = timed_render(&mut ui, &gpu, &mut renderer);
    let third = timed_render(&mut ui, &gpu, &mut renderer);
    println!(
        "{}: device {device:.1}, renderer {renderer_new:.1}, plain {plain_first:.1} then \
         {plain_second:.1}, build {build:.1}, components {first:.1} then {second:.1}, {third:.1} ms",
        gpu.adapter.get_info().name,
    );
    press_times(&gpu, &mut renderer);
}

fn press_times(gpu: &Gpu, renderer: &mut Renderer) {
    let mut ui = Ui::new(|| {
        page(Load {
            seconds: 0.0,
            loaders: 0,
            waves: 0,
            buttons: 30,
            compile: true,
        })
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1620, 1296),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    let at_rest = timed_render(&mut ui, gpu, renderer);
    let at_rest_again = timed_render(&mut ui, gpu, renderer);
    let position = PointF::new(60.0, 60.0);
    let start = Instant::now();
    ui.handle_event(WindowEvent::PointerMoved {
        position,
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::PointerButton {
        position,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time: 0,
    });
    let input = start.elapsed().as_secs_f64() * 1e3;
    let mut ui_ms = Vec::new();
    let mut render_ms = Vec::new();
    let mut frames = 0;
    let began = Instant::now();
    while began.elapsed().as_millis() < 500 {
        let now = Instant::now();
        let start = Instant::now();
        ui.animate(now);
        ui.update();
        let scene = ui.paint(now);
        ui_ms.push(start.elapsed().as_secs_f64() * 1e3);
        let start = Instant::now();
        let _ = gpu.render_to_rgba(renderer, &scene);
        render_ms.push(start.elapsed().as_secs_f64() * 1e3);
        frames += 1;
    }
    let first = (ui_ms[0], render_ms[0]);
    ui_ms.sort_by(f64::total_cmp);
    render_ms.sort_by(f64::total_cmp);
    println!(
        "press on 30 buttons: at rest {at_rest:.2}, {at_rest_again:.2}; input {input:.3}; first \
         frame ui {:.3} render {:.2}; {frames} frames: ui p50 {:.3} max {:.3}, render p50 {:.2} \
         max {:.2} ms",
        first.0,
        first.1,
        ui_ms[frames / 2],
        ui_ms[frames - 1],
        render_ms[frames / 2],
        render_ms[frames - 1],
    );
}

fn main() {
    let load = parse_args();
    if load.compile {
        compile_times();
        return;
    }
    App::new()
        .title("bench_md3e")
        .size(900, 720)
        .run(move || {
            if load.loaders + load.waves > 0 {
                close_after(load, None);
            }
            page(load)
        })
        .unwrap();
}
