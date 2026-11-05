//! How long a big diagram takes, without a window: `bench_svg <boxes>`. The boxes and arrows of the
//! same diagram for Chromium come from `tools/svg_bench.py`.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::Instant;

fn resident() -> u64 {
    let s = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap();
    s.lines()
        .find(|l| l.starts_with("Pss:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        / 1024
}

fn path_of(a: (f64, f64), b: (f64, f64)) -> String {
    let mid = (a.1 + b.1) * 0.5;
    format!(
        "M{} {} C{} {} {} {} {} {}",
        a.0, a.1, a.0, mid, b.0, mid, b.0, b.1
    )
}

fn main() {
    let boxes: usize = std::env::args().nth(1).map_or(300, |a| a.parse().unwrap());
    let gpu = Gpu::headless().unwrap();
    let (w, h) = (1728u32, 1616u32);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let hovered = signal(usize::MAX);
    let start = Instant::now();
    let columns = (boxes as f64).sqrt().ceil() as usize;
    let place = |i: usize| {
        (
            20.0 + (i % columns) as f64 * 180.0,
            20.0 + (i / columns) as f64 * 110.0,
        )
    };
    let rows = boxes.div_ceil(columns);
    let (full_w, full_h) = (columns as f64 * 180.0 + 40.0, rows as f64 * 110.0 + 40.0);
    let mut ui = Ui::new(move || {
        let (bw, bh) = (140.0, 56.0);
        // Scaled so all of it is in the window: everything is painted.
        let fit = (w as f64 / 1.8 / full_w)
            .min(h as f64 / 1.8 / full_h)
            .min(1.0);
        div().size_full().bg(Color::WHITE).child(
            svg()
                .w((full_w * fit) as f32)
                .h((full_h * fit) as f32)
                .view_box(0.0, 0.0, full_w as f32, full_h as f32)
                .child(
                    svg::group()
                        .fill_none()
                        .stroke(Color::from_rgb(0x333333))
                        .stroke_width(1.5)
                        .children((1..boxes).map(|i| {
                            let (ax, ay) = place((i * 7919) % i.max(1));
                            let (bx, by) = place(i);
                            svg::path(path_of((ax + bw * 0.5, ay + bh), (bx + bw * 0.5, by)))
                        })),
                )
                .child(svg::group().children((0..boxes).map(|i| {
                    let (x, y) = place(i);
                    svg::group()
                        .child(
                            svg::rounded_rect(x, y, bw, bh, 8.0)
                                .dyn_style(move |s| {
                                    s.fill(if hovered.get() == i {
                                        Color::from_rgb(0xffd0a0)
                                    } else {
                                        Color::from_rgb(0xdde8f8)
                                    })
                                })
                                .stroke(Color::from_rgb(0x3366aa)),
                        )
                        .child(
                            svg::text(format!("Node number {i}"))
                                .at((x + bw * 0.5) as f32, (y + bh * 0.5) as f32)
                                .anchor(svg::TextAnchor::Middle)
                                .baseline(svg::TextBaseline::Middle)
                                .text_size(14.0)
                                .fill(Color::from_rgb(0x222222)),
                        )
                }))),
        )
    });
    let built = start.elapsed();
    let t = Instant::now();
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w as i32, h as i32),
        scale: Scale(216),
    });
    ui.update();
    let laid = t.elapsed();
    let t = Instant::now();
    let mut scene = ui.paint(Instant::now());
    let painted = t.elapsed();
    let target = Offscreen::new(&gpu, format, w, h);
    let mut renderer = Renderer::new(&gpu.device, format);
    let frame = |scene: &anti_webui_core::scene::Scene, renderer: &mut Renderer, whole: bool| {
        let t = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if whole {
            renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, scene);
        }
        gpu.queue.submit([encoder.finish()]);
        let cpu = t.elapsed();
        target.read_rgba(&gpu);
        (cpu, t.elapsed())
    };
    let first = frame(&scene, &mut renderer, true);
    let again = frame(&scene, &mut renderer, true);
    let t = Instant::now();
    hovered.set(boxes / 2);
    ui.update();
    let next = ui.paint(Instant::now());
    let d = damage::diff(&scene, &next);
    let changed = t.elapsed();
    let t = Instant::now();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render_damaged(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &target.view,
        &next,
        &d,
    );
    gpu.queue.submit([encoder.finish()]);
    let hover_render = t.elapsed();
    target.read_rgba(&gpu);
    let hover_all = t.elapsed();
    scene = next;
    let prims = scene.primitives.len();
    println!(
        "boxes {boxes}: build {:.1} ms, layout {:.1} ms, paint {:.1} ms, first frame cpu {:.1} ms (with GPU {:.1} ms), again cpu {:.1} ms (with GPU {:.1} ms), hover: tree {:.2} ms, cpu {:.2} ms, GPU {:.2} ms; {prims} primitives, edges {} KiB, {} MiB PSS",
        built.as_secs_f64() * 1e3,
        laid.as_secs_f64() * 1e3,
        painted.as_secs_f64() * 1e3,
        first.0.as_secs_f64() * 1e3,
        first.1.as_secs_f64() * 1e3,
        again.0.as_secs_f64() * 1e3,
        again.1.as_secs_f64() * 1e3,
        changed.as_secs_f64() * 1e3,
        hover_render.as_secs_f64() * 1e3,
        hover_all.as_secs_f64() * 1e3,
        renderer.path_store_used() / 1024,
        resident(),
    );
}
