//! Measures what shadows cost on the GPU: a shadow around almost the whole window, and many small
//! ones.
//!
//! `cargo run --release -p anti-webui --example bench_shadow`

use anti_webui::core::Ui;
use anti_webui::core::event::*;
use anti_webui::core::geometry::{Scale, SizeI};
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::{Duration, Instant};

const W: u32 = 1620;
const H: u32 = 1616;
const FRAMES: u32 = 200;

fn bench<V: IntoView>(gpu: &Gpu, name: &str, view: impl FnOnce() -> V) {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(W as i32, H as i32),
        scale: Scale(216),
    });
    ui.update();
    let scene = ui.paint(Instant::now());
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let target = Offscreen::new(gpu, wgpu::TextureFormat::Rgba8Unorm, W, H);
    let (mut record, mut wait) = (Duration::ZERO, Duration::ZERO);
    for _ in 0..FRAMES {
        let t0 = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &scene);
        gpu.queue.submit([encoder.finish()]);
        let t1 = Instant::now();
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        record += t1 - t0;
        wait += t1.elapsed();
    }
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / FRAMES as f64;
    println!(
        "  {name:<24} record {:>6.3} ms | gpu {:>6.3} ms",
        per(record),
        per(wait)
    );
}

fn shadow(blur: f32) -> BoxShadow {
    BoxShadow {
        offset_y: 4.0,
        blur,
        color: rgba(0x00000040),
        ..Default::default()
    }
}

fn main() {
    let gpu = Gpu::headless().unwrap();
    println!("{:?}", gpu.adapter.get_info().name);
    bench(&gpu, "no shadow", || {
        div()
            .size_full()
            .p(40.0)
            .child(div().size_full().rounded(10.0).bg(Color::WHITE))
    });
    bench(&gpu, "window-sized shadow", || {
        div().size_full().p(40.0).child(
            div()
                .size_full()
                .rounded(10.0)
                .bg(Color::WHITE)
                .shadow(shadow(30.0)),
        )
    });
    bench(&gpu, "200 small shadows", || {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(4.0)
            .children((0..40).map(|_| {
                div().flex().flex_row().gap(4.0).children((0..5).map(|_| {
                    div()
                        .w(80.0)
                        .h(14.0)
                        .rounded(4.0)
                        .bg(Color::WHITE)
                        .shadow(shadow(6.0))
                }))
            }))
    });
}
