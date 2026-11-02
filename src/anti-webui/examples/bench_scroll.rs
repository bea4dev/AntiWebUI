//! Measures the cost of scrolling: one wheel step followed by a frame rendered on the GPU.
//!
//! `cargo run --release -p anti-webui --example bench_scroll`

#[path = "showcase.rs"]
mod showcase;

use anti_webui::core::Ui;
use anti_webui::core::damage;
use anti_webui::core::event::*;
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::{Duration, Instant};

const W: u32 = 1620;
const H: u32 = 1616;

fn long_list() -> Div {
    div()
        .h_full()
        .overflow_y_scroll()
        .children((0..2000).map(|i| {
            div()
                .flex()
                .items_center()
                .h(28.0)
                .px(8.0)
                .rounded(4.0)
                .bg(if i % 2 == 0 {
                    rgb(0xf4f4f4)
                } else {
                    rgb(0xffffff)
                })
                .child(text(format!(
                    "Row {i}: the quick brown fox jumps over the lazy dog"
                )))
        }))
}

fn bench(name: &str, gpu: &Gpu, root: impl FnOnce() -> Div) {
    let mut ui = Ui::new(root);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(W as i32, H as i32),
        scale: Scale(216),
    });
    ui.update();
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(gpu, format, W, H);
    let mut prev = ui.paint(Instant::now());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());

    let steps = 240;
    let position = PointF::new(W as f32 / 2.0, H as f32 / 2.0);
    let (mut t_update, mut t_paint, mut t_diff, mut t_record, mut t_gpu) = (
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
    );
    let mut repaint = 0i64;
    for i in 0..steps {
        let delta_y = if i % 120 < 60 { 24.0 } else { -24.0 };
        let t0 = Instant::now();
        ui.handle_event(WindowEvent::Scroll {
            position,
            delta_x: 0.0,
            delta_y,
            modifiers: Modifiers::default(),
            source: ScrollSource::Unknown,
            phase: ScrollPhase::Single,
            time: 0,
        });
        ui.update();
        let t1 = Instant::now();
        let scene = ui.paint(Instant::now());
        let t2 = Instant::now();
        let d = damage::diff(&prev, &scene);
        let t3 = Instant::now();
        repaint += d.area();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        let t4 = Instant::now();
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let t5 = Instant::now();
        t_update += t1 - t0;
        t_paint += t2 - t1;
        t_diff += t3 - t2;
        t_record += t4 - t3;
        t_gpu += t5 - t4;
        prev = scene;
    }
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / steps as f64;
    println!(
        "{name}: {} primitives; per step: update {:.2} ms, paint {:.2} ms, diff {:.2} ms, \
         record {:.2} ms, GPU wait {:.2} ms; damage {:.0}%",
        prev.primitives.len(),
        per(t_update),
        per(t_paint),
        per(t_diff),
        per(t_record),
        per(t_gpu),
        repaint as f64 * 100.0 / (steps as f64 * W as f64 * H as f64),
    );
}

fn main() {
    let gpu = Gpu::headless().expect("GPU");
    println!("{:?}", gpu.adapter.get_info().name);
    bench("showcase", &gpu, showcase::root);
    bench("2000 rows", &gpu, long_list);
}
