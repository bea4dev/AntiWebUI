//! What P4 of the PDF viewer does with a real renderer, without a window:
//! `bench_pdf_views <file.pdf> [--dump prefix]`. It opens the thumbnails, turns the pages, pinches,
//! and carries what is selected, and says how long that took. With `--dump` each of them leaves a
//! picture to look at. See `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, PinchPhase, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_pdf::{PdfState, PdfZoom, pdf_thumbnails, pdf_view, pdf_viewer};
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::rc::Rc;
use std::time::{Duration, Instant};

const WIDTH: u32 = 2340;
const HEIGHT: u32 = 1616;
const SCALE: Scale = Scale(216);
const FRAME: Duration = Duration::from_micros(8333);

struct Rig {
    ui: Ui,
    gpu: Gpu,
    target: Offscreen,
    renderer: Renderer,
    dump: Option<String>,
}

impl Rig {
    /// A frame as a window would have it, and how long it took on this thread.
    fn frame(&mut self) -> (Scene, f64) {
        let t = Instant::now();
        self.ui.animate(Instant::now());
        self.ui.update();
        let scene = self.ui.paint(Instant::now());
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.render(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &self.target.view,
            &scene,
        );
        self.gpu.queue.submit([encoder.finish()]);
        // A window does this when it gets the next surface, which frees what the GPU is done with.
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
        (scene, t.elapsed().as_secs_f64() * 1e3)
    }

    /// Frames until nothing is going on anymore, and how long that took.
    fn settle(&mut self) -> f64 {
        let t = Instant::now();
        for _ in 0..3 {
            self.ui.wait_for_images(Duration::from_secs(60));
            self.ui.update();
        }
        let done = t.elapsed().as_secs_f64() * 1e3;
        self.frame();
        done
    }

    fn look(&mut self, name: &str) {
        let Some(prefix) = &self.dump else {
            return;
        };
        let path = format!("{prefix}{name}.png");
        self.frame();
        let pixels = self.target.read_rgba(&self.gpu);
        anti_webui_app::headless::write_png(&path, WIDTH, HEIGHT, &pixels).unwrap();
    }

    fn pointer(&mut self, p: PointF) {
        self.ui.handle_event(WindowEvent::PointerMoved {
            position: p,
            modifiers: Modifiers::default(),
        });
    }

    fn button(&mut self, p: PointF, pressed: bool, time: u32) {
        self.ui.handle_event(WindowEvent::PointerButton {
            button: PointerButton::Primary,
            pressed,
            position: p,
            modifiers: Modifiers::default(),
            time,
        });
    }

    fn pinch(&mut self, p: PointF, scale: f32, phase: PinchPhase) {
        self.ui.handle_event(WindowEvent::Pinch {
            position: p,
            scale,
            modifiers: Modifiers::default(),
            phase,
        });
    }
}

fn images(scene: &Scene, out: &mut Vec<ImageDraw>) {
    for p in &scene.primitives {
        match p {
            Primitive::Image(d) => out.push(d.clone()),
            Primitive::Layer(l) => images(&l.content, out),
            _ => {}
        }
    }
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let at = ((values.len() - 1) as f64 * p) as usize;
    values.get(at).copied().unwrap_or(0.0)
}

fn memory() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches(" kB").parse::<u64>().ok())
        .map_or(0, |kb| kb / 1024)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args[1].clone();
    let dump = args
        .iter()
        .position(|a| a == "--dump")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let gpu = Gpu::headless().unwrap();
    let target = Offscreen::new(&gpu, format, WIDTH, HEIGHT);
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_release_picture_pixels(true);

    let handed = Rc::new(std::cell::Cell::new(None));
    let ui = {
        let handed = handed.clone();
        Ui::new(move || {
            let viewer = pdf_viewer();
            let open = signal(false);
            handed.set(Some((viewer, open)));
            div()
                .flex()
                .w_full()
                .h_full()
                .child(show(
                    move || open.get(),
                    move || pdf_thumbnails(viewer).w(140.0).h_full().flex_none(),
                ))
                .child(pdf_view(path).viewer(viewer).flex_1().h_full())
        })
    };
    let (viewer, open) = handed.get().unwrap();
    let mut rig = Rig {
        ui,
        gpu,
        target,
        renderer,
        dump,
    };
    rig.ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(WIDTH as i32, HEIGHT as i32),
        scale: SCALE,
    });
    rig.settle();
    if let PdfState::Failed(e) = untrack(|| viewer.state()) {
        panic!("{e}");
    }

    open.set(true);
    let thumbnails_ms = rig.settle();
    rig.look("thumbnails");
    let thumbnails_shown = {
        let (scene, _) = rig.frame();
        let mut all = Vec::new();
        images(&scene, &mut all);
        all.iter()
            .filter(|d| d.bitmap.width() == SCALE.px(108.0) as u32)
            .count()
    };
    open.set(false);
    rig.settle();

    viewer.rotate_clockwise();
    let turned_ms = rig.settle();
    rig.look("turned");
    viewer.set_rotation(0);
    rig.settle();

    let before_pinch_mib = memory();
    // Pinching out to three times the size and back, a step every frame, and then coming to rest.
    viewer.set_zoom(PdfZoom::Percent(1.0));
    rig.settle();
    let middle = PointF::new(WIDTH as f32 / 2.0, HEIGHT as f32 / 2.0);
    rig.pointer(middle);
    let steps = 240;
    let step = 3.0f32.powf(1.0 / (steps as f32 / 2.0));
    let mut frames = Vec::new();
    rig.pinch(middle, 1.0, PinchPhase::Began);
    let began = Instant::now();
    for i in 0..steps {
        let t = Instant::now();
        let scale = if i < steps / 2 { step } else { 1.0 / step };
        rig.pinch(middle, scale, PinchPhase::Moved);
        let handled = t.elapsed().as_secs_f64() * 1e3;
        frames.push(handled + rig.frame().1);
        let due = began + FRAME * (i + 1);
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        }
    }
    rig.pinch(middle, 1.0, PinchPhase::Ended);
    let pinch_rest_ms = rig.settle();
    let after_pinch_mib = memory();
    rig.look("pinched");

    // Zooming as with a wheel, which can't say when it is over, as the pinch did.
    let mut wheel = Vec::new();
    let began = Instant::now();
    for i in 0..steps {
        let t = Instant::now();
        let by = if i < steps / 2 { step } else { 1.0 / step };
        viewer.zoom_by(by);
        wheel.push(t.elapsed().as_secs_f64() * 1e3 + rig.frame().1);
        let due = began + FRAME * (i + 1);
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        }
    }
    rig.settle();
    let after_wheel_mib = memory();

    // Everything is selected, and the first press on it that is on some text carries it.
    viewer.set_zoom(PdfZoom::FitWidth);
    // The first pages of a book can be without text.
    viewer.go_to_page(untrack(|| viewer.page_count()).min(20) / 2);
    rig.settle();
    let (mut carried, mut picture_ms, mut text_ms) = (false, 0.0, 0.0);
    'found: for y in (200..HEIGHT as i32 - 200).step_by(40) {
        for x in [
            WIDTH as i32 / 2 - 300,
            WIDTH as i32 / 2 + 300,
            WIDTH as i32 / 2,
        ] {
            let p = PointF::new(x as f32, y as f32);
            viewer.select_all();
            rig.pointer(p);
            rig.settle();
            let t = Instant::now();
            rig.button(p, true, 10_000);
            rig.ui.wait_for_images(Duration::from_secs(5));
            // It wasn't on any text, which ended the selection.
            if !untrack(|| viewer.has_selection()) {
                rig.button(p, false, 10_000);
                rig.settle();
                continue;
            }
            rig.pointer(PointF::new(p.x + 40.0, p.y + 40.0));
            rig.frame();
            let until = Instant::now() + Duration::from_secs(10);
            while untrack(dragged::<DraggedText>).is_none() && Instant::now() < until {
                rig.ui.wait_for_images(Duration::from_secs(1));
                rig.ui.update();
            }
            text_ms = t.elapsed().as_secs_f64() * 1e3;
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                let (scene, _) = rig.frame();
                let mut all = Vec::new();
                images(&scene, &mut all);
                if all.iter().any(|d| !d.bitmap.is_opaque()) {
                    picture_ms = t.elapsed().as_secs_f64() * 1e3;
                    break;
                }
            }
            carried = true;
            rig.pointer(PointF::new(p.x + 200.0, p.y + 200.0));
            rig.look("carried");
            rig.button(PointF::new(p.x + 200.0, p.y + 200.0), false, 10_001);
            break 'found;
        }
    }
    println!(
        "{{\"thumbnails_ms\":{thumbnails_ms:.1},\"thumbnails_shown\":{thumbnails_shown},\"turned_ms\":{turned_ms:.1},\"pinch_p50_ms\":{:.2},\"pinch_p95_ms\":{:.2},\"pinch_max_ms\":{:.2},\"pinch_rest_ms\":{pinch_rest_ms:.1},\"carried\":{carried},\"carry_text_ms\":{text_ms:.1},\"carry_picture_ms\":{picture_ms:.1},\"before_pinch_mib\":{before_pinch_mib},\"after_pinch_mib\":{after_pinch_mib},\"wheel_p95_ms\":{:.2},\"after_wheel_mib\":{after_wheel_mib},\"peak_mib\":{}}}",
        percentile(&mut frames, 0.5),
        percentile(&mut frames, 0.95),
        percentile(&mut frames, 1.0),
        percentile(&mut wheel, 0.95),
        memory(),
    );
}
