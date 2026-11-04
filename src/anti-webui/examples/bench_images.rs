//! What pictures cost. Without a window: `cargo run --release -p anti-webui --example bench_images`
//!
//! Prints how long big pictures take to arrive, what the frames cost while they do, what they take
//! on the GPU, and what scrolling through small pictures costs.

use anti_webui::core::Ui;
use anti_webui::core::event::WindowEvent;
use anti_webui::core::geometry::{Scale, SizeI};
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Renderer, wgpu::TextureFormat};
use image::ImageEncoder;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCREEN: (u32, u32) = (2340, 1616);

/// Something like a photo: with detail, which is what makes it hard to compress and to decode.
fn photo(seed: u32, width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    let mut noise = seed.wrapping_mul(2654435761).wrapping_add(1);
    for y in 0..height {
        for x in 0..width {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
            let wave = ((u * 40.0 + seed as f32).sin() * (v * 30.0).cos() * 0.5 + 0.5) * 200.0;
            let grain = (noise & 15) as f32;
            pixels.extend([
                (wave + grain) as u8,
                (u * 200.0 + grain) as u8,
                (v * 200.0 + grain) as u8,
            ]);
        }
    }
    pixels
}

fn jpeg(seed: u32, width: u32, height: u32) -> Arc<[u8]> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .write_image(
            &photo(seed, width, height),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .expect("in memory");
    out.into()
}

fn png(seed: u32, width: u32, height: u32) -> Arc<[u8]> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            &photo(seed, width, height),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .expect("in memory");
    out.into()
}

fn percentile(times: &mut [f64], p: f64) -> f64 {
    times.sort_by(f64::total_cmp);
    times[((times.len() - 1) as f64 * p) as usize]
}

fn mount<V: IntoView>(view: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(SCREEN.0 as i32, SCREEN.1 as i32),
        scale: Scale::from_f64(1.8),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui
}

/// Frames as fast as they go until nothing is on its way, and what each took on this thread.
fn frames_until_there(
    ui: &mut Ui,
    gpu: &Gpu,
    renderer: &mut Renderer,
    expected: usize,
) -> Vec<f64> {
    let mut times = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        let begin = Instant::now();
        ui.animate(begin);
        ui.update();
        let scene = ui.paint(begin);
        let target = anti_webui::wgpu::Offscreen::new(gpu, renderer.format(), SCREEN.0, SCREEN.1);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &scene);
        gpu.queue.submit([encoder.finish()]);
        times.push(begin.elapsed().as_secs_f64() * 1000.0);
        if renderer.picture_counts().0 >= expected {
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    times
}

fn arrival(name: &str, files: &[Arc<[u8]>], pixels: u64, side: f32) {
    let gpu = Gpu::headless().expect("a GPU");
    let mut renderer = Renderer::new(&gpu.device, TextureFormat::Bgra8Unorm);
    renderer.set_release_picture_pixels(true);
    let sources: Vec<ImageSource> = files.iter().map(|f| ImageSource::from(f.clone())).collect();
    let n = sources.len();
    let mut ui = mount(move || {
        div()
            .flex()
            .flex_col()
            .children(sources.into_iter().map(move |s| {
                img(s)
                    .w(side)
                    .h(side * 0.75)
                    .flex_none()
                    .object_fit(ObjectFit::Cover)
            }))
    });
    let start = Instant::now();
    let mut times = frames_until_there(&mut ui, &gpu, &mut renderer, n);
    let took = start.elapsed().as_secs_f64() * 1000.0;
    let (count, bytes) = renderer.picture_counts();
    // The first one makes what any frame takes, the rest is what the pictures take.
    times.remove(0);
    let max = times.iter().copied().fold(0.0, f64::max);
    let p50 = percentile(&mut times, 0.5);
    println!(
        "{name}: {count} shown in {took:.0} ms; frames on this thread p50 {p50:.2} max {max:.2} ms; \
         {:.1} MiB on the GPU, {:.1} MiB if kept at full size",
        bytes as f64 / (1 << 20) as f64,
        (pixels * 4 * n as u64 * 4 / 3) as f64 / (1 << 20) as f64,
    );
}

fn scrolling() {
    use anti_webui::core::event::{Modifiers, ScrollPhase, ScrollSource};
    use anti_webui::core::geometry::PointF;
    let gpu = Gpu::headless().expect("a GPU");
    let mut renderer = Renderer::new(&gpu.device, TextureFormat::Bgra8Unorm);
    renderer.set_release_picture_pixels(true);
    let files: Vec<ImageSource> = (0..2000)
        .map(|i| {
            let side = 64;
            ImageSource::rgba(
                side,
                side,
                (0..side * side)
                    .flat_map(|p| [(p + i) as u8, (p * 3) as u8, i as u8, 255])
                    .collect(),
            )
        })
        .collect();
    let mut ui = mount(move || {
        div().h_full().overflow_y_scroll().children(
            files
                .into_iter()
                .map(|f| img(f).w(40.0).h(40.0).flex_none().rounded(8.0)),
        )
    });
    let target = anti_webui::wgpu::Offscreen::new(&gpu, renderer.format(), SCREEN.0, SCREEN.1);
    let mut times = Vec::new();
    ui.update();
    let mut last = ui.paint(Instant::now());
    let mut draw = |scene: &anti_webui::core::scene::Scene,
                    last: &anti_webui::core::scene::Scene,
                    first: bool| {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if first {
            renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, scene);
        } else {
            let damage = anti_webui::core::damage::diff(last, scene);
            renderer.render_damaged(
                &gpu.device,
                &gpu.queue,
                &mut encoder,
                &target.view,
                scene,
                &damage,
            );
        }
        gpu.queue.submit([encoder.finish()]);
    };
    draw(&last, &last.clone(), true);
    for i in 0..600 {
        let begin = Instant::now();
        ui.handle_event(WindowEvent::Scroll {
            position: PointF::new(100.0, 100.0),
            delta_x: 0.0,
            delta_y: 24.0,
            phase: ScrollPhase::Moved,
            source: ScrollSource::Touchpad,
            modifiers: Modifiers::default(),
            time: i,
        });
        ui.animate(begin);
        ui.update();
        let scene = ui.paint(begin);
        draw(&scene, &last, false);
        last = scene;
        times.push(begin.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_millis(2));
    }
    let max = times.iter().copied().fold(0.0, f64::max);
    let p50 = percentile(&mut times, 0.5);
    let p99 = percentile(&mut times, 0.99);
    let (shown, bytes) = renderer.picture_counts();
    println!(
        "scrolling 2000 small pictures: p50 {p50:.2} p99 {p99:.2} max {max:.2} ms; {shown} on the GPU, {:.1} MiB",
        bytes as f64 / (1 << 20) as f64
    );
}

fn main() {
    let start = Instant::now();
    let jpegs: Vec<Arc<[u8]>> = (0..8).map(|i| jpeg(i, 4000, 3000)).collect();
    let pngs: Vec<Arc<[u8]>> = (0..4).map(|i| png(i, 4000, 3000)).collect();
    eprintln!("made the files in {:.1} s", start.elapsed().as_secs_f32());
    println!(
        "a JPEG is {:.1} MiB, a PNG {:.1} MiB",
        jpegs[0].len() as f64 / (1 << 20) as f64,
        pngs[0].len() as f64 / (1 << 20) as f64
    );
    let twelve = 4000 * 3000;
    // A thread of its own for each, which comes with a cache of its own.
    let apart = |f: &(dyn Fn() + Sync)| {
        std::thread::scope(|s| {
            s.spawn(f);
        });
    };
    apart(&|| arrival("1 JPEG of 12 MP at 300x225", &jpegs[..1], twelve, 300.0));
    apart(&|| arrival("8 JPEG of 12 MP at 150x112", &jpegs, twelve, 150.0));
    apart(&|| arrival("1 JPEG of 12 MP at 1000x750", &jpegs[..1], twelve, 1000.0));
    apart(&|| arrival("1 PNG of 12 MP at 300x225", &pngs[..1], twelve, 300.0));
    apart(&|| arrival("4 PNG of 12 MP at 150x112", &pngs, twelve, 150.0));
    apart(&scrolling);
}
