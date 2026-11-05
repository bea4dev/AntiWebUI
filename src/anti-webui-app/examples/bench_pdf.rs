//! How long a PDF takes on the screen and what scrolling it does, without a window:
//! `bench_pdf <file.pdf> [--dump first.png] [--speed screens-per-second] [--seconds n]
//! [--zoom percent] [--slow-frames] [--search word] [--select]`. The last two are there to look at:
//! what is found and all that is selected show in the picture. What it prints is one line of JSON, for `src/anti-webui-pdf/tools/compare.py`, which gets the same numbers
//! from PDF.js. See `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, ScrollPhase, ScrollSource, WindowEvent};
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_pdf::{PdfState, PdfViewer, PdfZoom, pdf_view, pdf_viewer};
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::rc::Rc;
use std::time::{Duration, Instant};

const WIDTH: u32 = 2340;
const HEIGHT: u32 = 1616;
const SCALE: Scale = Scale(216);
const FRAME: Duration = Duration::from_micros(8333);

fn memory() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find(|l| l.starts_with("VmHWM"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        / 1024
}

/// Which rows of the window show a picture.
fn covered(scene: &Scene, at: i32, rows: &mut [bool]) {
    for p in &scene.primitives {
        match p {
            Primitive::Image(d) => {
                let top = (d.rect.y + at).max(0) as usize;
                let bottom = (d.rect.bottom() + at).max(0) as usize;
                for row in rows.iter_mut().take(bottom).skip(top) {
                    *row = true;
                }
            }
            Primitive::Layer(l) => covered(&l.content, at + l.origin.y, rows),
            _ => {}
        }
    }
}

/// How many rows of the window don't show a picture.
fn blank_rows(scene: &Scene) -> usize {
    let mut rows = vec![false; HEIGHT as usize];
    covered(scene, 0, &mut rows);
    rows.iter().filter(|r| !**r).count()
}

/// How many pixels of the window show a picture one to one.
fn sharp_pixels(scene: &Scene, at: (i32, i32)) -> i64 {
    let window = RectI::new(0, 0, WIDTH as i32, HEIGHT as i32);
    let mut pixels = 0;
    for p in &scene.primitives {
        match p {
            Primitive::Image(d) => {
                if (d.rect.width as u32, d.rect.height as u32)
                    == (d.bitmap.width(), d.bitmap.height())
                {
                    pixels += d.rect.translate(at.0, at.1).intersect(&window).area();
                }
            }
            Primitive::Layer(l) => {
                pixels += sharp_pixels(&l.content, (at.0 + l.origin.x, at.1 + l.origin.y));
            }
            _ => {}
        }
    }
    pixels
}

/// How much of the window is sharp, of the part that shows pages: what is left is the gaps.
fn sharp_share(scene: &Scene) -> f64 {
    sharp_pixels(scene, (0, 0)) as f64 / (WIDTH as f64 * HEIGHT as f64)
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values
        .get(((values.len() as f64 * p) as usize).min(values.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let path = args[1].clone();
    let speed: f32 = value("--speed").map_or(1.0, |v| v.parse().unwrap());
    let seconds: f32 = value("--seconds").map_or(5.0, |v| v.parse().unwrap());
    let zoom: Option<f32> = value("--zoom").map(|v| v.parse::<f32>().unwrap() / 100.0);
    let slow_frames = args.iter().any(|a| a == "--slow-frames");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let gpu = Gpu::headless().unwrap();
    let target = Offscreen::new(&gpu, format, WIDTH, HEIGHT);
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_release_picture_pixels(true);

    let handed: Rc<std::cell::Cell<Option<PdfViewer>>> = Rc::default();
    let start = Instant::now();
    let mut ui = {
        let handed = handed.clone();
        Ui::new(move || {
            let viewer = pdf_viewer();
            handed.set(Some(viewer));
            if let Some(zoom) = zoom {
                viewer.set_zoom(PdfZoom::Percent(zoom));
            }
            pdf_view(path).viewer(viewer).w_full().h_full()
        })
    };
    let viewer = handed.get().unwrap();
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(WIDTH as i32, HEIGHT as i32),
        scale: SCALE,
    });
    if let Some(word) = value("--search") {
        viewer.search(word.clone());
    }

    // Up to when the first picture has been drawn by the GPU, as a window would.
    let shown;
    let mut opened = None;
    loop {
        ui.animate(Instant::now());
        ui.update();
        if opened.is_none() && untrack(|| viewer.state()) == PdfState::Ready {
            opened = Some(start.elapsed());
        }
        if let PdfState::Failed(e) = untrack(|| viewer.state()) {
            panic!("{e}");
        }
        if args.iter().any(|a| a == "--select") {
            viewer.select_all();
        }
        let scene = ui.paint(Instant::now());
        let looking = value("--search").is_some() || args.iter().any(|a| a == "--select");
        // What shows on top comes after the page has been read, a little after its picture.
        if looking && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if blank_rows(&scene) < HEIGHT as usize {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &scene);
            gpu.queue.submit([encoder.finish()]);
            let submitted = start.elapsed();
            let pixels = target.read_rgba(&gpu);
            shown = (submitted, start.elapsed());
            if let Some(out) = value("--dump") {
                anti_webui_app::headless::write_png(out, WIDTH, HEIGHT, &pixels).unwrap();
            }
            break;
        }
        std::thread::sleep(Duration::from_micros(200));
    }

    // Until what shows is sharp, which is when there is a picture of it one to one: most of a
    // window, since the gaps never are.
    let sharp_at;
    loop {
        ui.animate(Instant::now());
        ui.update();
        let scene = ui.paint(Instant::now());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &scene);
        gpu.queue.submit([encoder.finish()]);
        if sharp_share(&scene) >= 0.9 || start.elapsed() > Duration::from_secs(60) {
            sharp_at = start.elapsed();
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // Scrolling at a steady speed, a frame every 8.3 ms, as long as the document lasts.
    let middle = PointF::new(WIDTH as f32 / 2.0, HEIGHT as f32 / 2.0);
    let step = HEIGHT as f32 * speed * FRAME.as_secs_f32();
    let mut frames = Vec::new();
    let mut blank_frames = 0usize;
    let mut blank_row_frames = 0usize;
    let mut sharp_sum = 0.0;
    let began = Instant::now();
    let mut next = began;
    let mut time = 0u32;
    let mut phase = ScrollPhase::Began;
    let mut late = 0usize;
    while began.elapsed().as_secs_f32() < seconds {
        let t = Instant::now();
        ui.handle_event(WindowEvent::Scroll {
            position: middle,
            delta_x: 0.0,
            delta_y: step,
            modifiers: Modifiers::default(),
            source: ScrollSource::Touchpad,
            phase,
            time,
        });
        phase = ScrollPhase::Moved;
        time += 8;
        ui.animate(Instant::now());
        ui.update();
        let updated = t.elapsed();
        let scene = ui.paint(Instant::now());
        let painted = t.elapsed();
        let blank = blank_rows(&scene);
        // The gaps between pages are small: more than that is a page with nothing on it yet.
        if blank > 64 {
            blank_frames += 1;
            blank_row_frames += blank;
        }
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        sharp_sum += sharp_share(&scene);
        let counted = t.elapsed();
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &scene);
        let rendered = t.elapsed();
        gpu.queue.submit([encoder.finish()]);
        frames.push(t.elapsed().as_secs_f64() * 1e3);
        if slow_frames && t.elapsed() > Duration::from_micros(1500) {
            eprintln!(
                "frame {} slow: update {:.2} paint {:.2} render {:.2} submit {:.2}",
                frames.len(),
                updated.as_secs_f64() * 1e3,
                (painted - updated).as_secs_f64() * 1e3,
                (rendered - counted).as_secs_f64() * 1e3,
                (t.elapsed() - rendered).as_secs_f64() * 1e3,
            );
        }
        next += FRAME;
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        } else {
            late += 1;
        }
    }
    let count = frames.len();
    println!(
        "{{\"opened_ms\":{:.1},\"shown_submitted_ms\":{:.1},\"shown_gpu_ms\":{:.1},\"sharp_ms\":{:.1},\"sharp_share\":{:.4},\"pages\":{},\"frames\":{count},\"late\":{late},\"frame_p50_ms\":{:.2},\"frame_p95_ms\":{:.2},\"frame_max_ms\":{:.2},\"blank_frames\":{blank_frames},\"blank_screens_share\":{:.4},\"page_reached\":{},\"peak_mib\":{}}}",
        opened.unwrap_or_default().as_secs_f64() * 1e3,
        shown.0.as_secs_f64() * 1e3,
        shown.1.as_secs_f64() * 1e3,
        sharp_at.as_secs_f64() * 1e3,
        sharp_sum / count.max(1) as f64,
        untrack(|| viewer.page_count()),
        percentile(&mut frames, 0.5),
        percentile(&mut frames, 0.95),
        percentile(&mut frames, 1.0),
        blank_row_frames as f64 / (count.max(1) as f64 * HEIGHT as f64),
        untrack(|| viewer.page()),
        memory(),
    );
}
