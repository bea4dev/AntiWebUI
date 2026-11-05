//! How long pages take to turn when presenting, with a real renderer and without a window:
//! `bench_pdf_slides <file.pdf> [ms between turns]`. It goes through the pages once slowly, so that the next ones have
//! been drawn, and once at a fast pace. See `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Key, KeyEvent, Modifiers, WindowEvent};
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_pdf::{PdfState, pdf_view, pdf_viewer};
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WIDTH: u32 = 2880;
const HEIGHT: u32 = 1800;
const SCALE: Scale = Scale(216);
const FRAME: Duration = Duration::from_micros(8333);

struct Rig {
    ui: Ui,
    gpu: Gpu,
    target: Offscreen,
    renderer: Renderer,
}

impl Rig {
    /// A frame as a window would have it, and how long it took on this thread.
    fn frame(&mut self) -> (Scene, f64) {
        let t = Instant::now();
        self.ui.tick(Instant::now());
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
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
        (scene, t.elapsed().as_secs_f64() * 1e3)
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.ui.wait_for_images(Duration::from_secs(60));
            self.ui.update();
        }
        self.frame();
    }

    fn shown(&mut self) -> Option<ImageDraw> {
        first_image(&self.frame().0)
    }

    fn turn(&mut self) {
        self.ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::ArrowRight),
            text: None,
            modifiers: Modifiers::default(),
            pressed: true,
            repeat: false,
        }));
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

fn first_image(scene: &Scene) -> Option<ImageDraw> {
    let mut all = Vec::new();
    images(scene, &mut all);
    all.into_iter().next()
}

fn same_picture(a: &Option<ImageDraw>, b: &Option<ImageDraw>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(&a.bitmap, &b.bitmap),
        (None, None) => true,
        _ => false,
    }
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let i = ((values.len() - 1) as f64 * p).round() as usize;
    values.get(i).copied().unwrap_or(0.0)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("bench_pdf_slides <file.pdf> [ms between turns]");
    let pace: u64 = std::env::args().nth(2).map_or(100, |p| p.parse().unwrap());
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
            handed.set(Some(viewer));
            pdf_view(path).viewer(viewer).w_full().h_full()
        })
    };
    let viewer = handed.get().unwrap();
    let mut rig = Rig {
        ui,
        gpu,
        target,
        renderer,
    };
    let size = SizeI::new(WIDTH as i32, HEIGHT as i32);
    rig.ui
        .handle_event(WindowEvent::Resized { size, scale: SCALE });
    rig.settle();
    if let PdfState::Failed(e) = untrack(|| viewer.state()) {
        panic!("{e}");
    }
    let count = untrack(|| viewer.page_count());
    let t = Instant::now();
    viewer.present();
    rig.ui.handle_event(WindowEvent::Fullscreen(true));
    rig.settle();
    let started_ms = t.elapsed().as_secs_f64() * 1e3;

    // Slowly, with all that comes next drawn: what a person going through slides gets.
    let mut slow = Vec::new();
    let mut waited = 0;
    for _ in 1..count.min(60) {
        rig.settle();
        let before = rig.shown();
        rig.turn();
        let (scene, ms) = rig.frame();
        slow.push(ms);
        if same_picture(&before, &first_image(&scene)) {
            waited += 1;
        }
    }

    // As fast as a key repeats: how long each page took to show after it was asked for.
    viewer.go_to_page(0);
    rig.settle();
    let mut frames = Vec::new();
    let mut shows = Vec::new();
    let mut asked = None;
    let mut on_screen = rig.shown();
    let mut last_turn = Instant::now();
    let mut turns = 0;
    let t = Instant::now();
    while turns + 1 < count.min(60) || asked.is_some() {
        if last_turn.elapsed() >= Duration::from_millis(pace) && turns + 1 < count.min(60) {
            rig.turn();
            turns += 1;
            last_turn = Instant::now();
            asked.get_or_insert(last_turn);
        }
        rig.ui.poll_images();
        let (scene, ms) = rig.frame();
        frames.push(ms);
        let now = first_image(&scene);
        if !same_picture(&on_screen, &now) {
            on_screen = now;
            if let Some(at) = asked.take() {
                shows.push(at.elapsed().as_secs_f64() * 1e3);
            }
        }
        std::thread::sleep(FRAME.saturating_sub(Duration::from_secs_f64(ms / 1e3)));
    }
    let fast_s = t.elapsed().as_secs_f64();
    println!(
        "{{\"pages\":{count},\"start_ms\":{started_ms:.1},\"turn_p50_ms\":{:.2},\"turn_p95_ms\":{:.2},\"turn_max_ms\":{:.2},\"turns_not_ready\":{waited},\"pace_ms\":{pace},\"fast_seconds\":{fast_s:.1},\"fast_show_p50_ms\":{:.1},\"fast_show_max_ms\":{:.1},\"fast_frame_p95_ms\":{:.2},\"fast_frame_max_ms\":{:.2}}}",
        percentile(&mut slow, 0.5),
        percentile(&mut slow, 0.95),
        percentile(&mut slow, 1.0),
        percentile(&mut shows, 0.5),
        percentile(&mut shows, 1.0),
        percentile(&mut frames, 0.95),
        percentile(&mut frames, 1.0),
    );
}
