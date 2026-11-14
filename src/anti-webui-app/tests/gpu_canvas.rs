//! `gpu_canvas` through the real renderer (see `docs/design/41-gpu-canvas.md`). Skipped when no GPU
//! adapter is available.

use anti_webui_core::Ui;
use anti_webui_core::damage::{self, Damage};
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::Scene;
use anti_webui_wgpu::canvas::{GpuFrame, GpuInit, GpuPaint, gpu_canvas};
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::cell::Cell;
use std::time::Instant;

thread_local! {
    /// How many times a canvas was drawn on this thread.
    static DRAWN: Cell<usize> = const { Cell::new(0) };
    /// The size and the scale of what it was last drawn into.
    static SEEN: Cell<((u32, u32), f32)> = const { Cell::new(((0, 0), 0.0)) };
}

fn drawn() -> usize {
    DRAWN.with(Cell::get)
}

fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

fn color(c: Color) -> wgpu::Color {
    let [r, g, b, a] = c.premultiplied();
    wgpu::Color {
        r: r as f64,
        g: g as f64,
        b: b as f64,
        a: a as f64,
    }
}

/// Fills all of the canvas with one color.
struct Fill;

impl GpuPaint for Fill {
    type Props = Color;

    fn new(_: &GpuInit<'_>) -> Self {
        Fill
    }

    fn render(&mut self, c: &Color, frame: &mut GpuFrame<'_>) {
        DRAWN.with(|d| d.set(d.get() + 1));
        SEEN.with(|s| s.set((frame.size, frame.scale)));
        drop(frame.begin_pass(Some(color(*c))));
    }
}

/// Fills it with a gray given as light, through the sRGB view.
struct Light;

impl GpuPaint for Light {
    type Props = f64;

    fn new(gpu: &GpuInit<'_>) -> Self {
        assert_eq!(gpu.format, wgpu::TextureFormat::Rgba8Unorm);
        Light
    }

    fn render(&mut self, v: &f64, frame: &mut GpuFrame<'_>) {
        drop(frame.begin_srgb_pass(Some(wgpu::Color {
            r: *v,
            g: *v,
            b: *v,
            a: 1.0,
        })));
    }
}

/// Draws without a pipeline, which is wrong.
struct BadDraw;

impl GpuPaint for BadDraw {
    type Props = u32;

    fn new(_: &GpuInit<'_>) -> Self {
        BadDraw
    }

    fn render(&mut self, _: &u32, frame: &mut GpuFrame<'_>) {
        DRAWN.with(|d| d.set(d.get() + 1));
        let mut pass = frame.begin_pass(Some(wgpu::Color::RED));
        pass.draw(0..3, 0..1);
    }
}

/// Makes a buffer that can't be, and so is never drawn.
struct BadMade;

impl GpuPaint for BadMade {
    type Props = ();

    fn new(gpu: &GpuInit<'_>) -> Self {
        let _ = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::MAP_WRITE,
            mapped_at_creation: false,
        });
        BadMade
    }

    fn render(&mut self, _: &(), _: &mut GpuFrame<'_>) {
        DRAWN.with(|d| d.set(d.get() + 1));
    }
}

/// Draws frame after frame, keeping the window's pixels between them as a display buffer does.
struct Frames {
    gpu: Gpu,
    ui: Ui,
    renderer: Renderer,
    target: Offscreen,
    prev: Scene,
    size: (u32, u32),
}

impl Frames {
    fn new<V: IntoView>(w: u32, h: u32, scale: f64, view: impl FnOnce() -> V) -> Option<Self> {
        let gpu = match Gpu::headless() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e}");
                return None;
            }
        };
        let mut ui = Ui::new(view);
        ui.handle_event(WindowEvent::Resized {
            size: SizeI::new(w as i32, h as i32),
            scale: Scale::from_f64(scale),
        });
        ui.update();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = Renderer::new(&gpu.device, format);
        let target = Offscreen::new(&gpu, format, w, h);
        let prev = ui.paint(Instant::now());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
        gpu.queue.submit([encoder.finish()]);
        Some(Self {
            gpu,
            ui,
            renderer,
            target,
            prev,
            size: (w, h),
        })
    }

    fn pixels(&self) -> Vec<u8> {
        self.target.read_rgba(&self.gpu)
    }

    /// The next frame, drawn where it changed, and what changed.
    fn next(&mut self) -> (Vec<u8>, Damage) {
        self.ui.update();
        let scene = self.ui.paint(Instant::now());
        let d = damage::diff(&self.prev, &scene);
        let gpu = &self.gpu;
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &self.target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        self.prev = scene;
        (self.pixels(), d)
    }

    /// What all of the last frame looks like drawn at once, by a renderer of its own.
    fn all_at_once(&self) -> Vec<u8> {
        let mut r = Renderer::new(&self.gpu.device, wgpu::TextureFormat::Rgba8Unorm);
        self.gpu.render_to_rgba(&mut r, &self.prev)
    }
}

#[test]
fn it_fills_its_content_box_cut_to_its_shape() {
    let Some(f) = Frames::new(200, 120, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).p(10.0).child(
            gpu_canvas::<Fill>(|| rgb(0x2060c0))
                .w(100.0)
                .h(60.0)
                .p(5.0)
                .bg(rgb(0x00ff00))
                .rounded(20.0),
        )
    }) else {
        return;
    };
    let p = f.pixels();
    let w = f.size.0;
    assert_eq!(px(&p, w, 60, 40), [0x20, 0x60, 0xc0, 255], "the middle");
    assert_eq!(
        px(&p, w, 12, 40),
        [0, 255, 0, 255],
        "the padding is the background's"
    );
    assert_eq!(
        px(&p, w, 11, 11),
        [255, 255, 255, 255],
        "the rounded corner"
    );
    assert_eq!(px(&p, w, 150, 40), [255, 255, 255, 255], "beside it");
    assert_eq!(drawn(), 1);
}

#[test]
fn it_is_as_big_as_a_canvas_and_draws_every_pixel_it_shows_on() {
    let Some(_f) = Frames::new(500, 300, 1.5, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(gpu_canvas::<Fill>(|| rgb(0x123456)))
    }) else {
        return;
    };
    assert_eq!(
        SEEN.with(Cell::get),
        ((450, 225), 1.5),
        "300 by 150 logical"
    );

    let r = node_ref();
    let Some(_f) = Frames::new(300, 300, 1.0, move || {
        div().child(gpu_canvas::<Fill>(|| rgb(0x123456)).w(120.0).node_ref(r))
    }) else {
        return;
    };
    assert_eq!(
        r.rect().map(|r| (r.width, r.height)),
        Some((120, 60)),
        "2:1 kept"
    );
}

#[test]
fn it_draws_sharp_when_scaled_up() {
    let Some(_f) = Frames::new(400, 300, 1.0, || {
        div().p(100.0).child(
            gpu_canvas::<Fill>(|| rgb(0x123456))
                .w(50.0)
                .h(40.0)
                .scale(2.0),
        )
    }) else {
        return;
    };
    assert_eq!(SEEN.with(Cell::get), ((100, 80), 2.0));
}

#[test]
fn it_draws_again_only_when_what_it_is_drawn_from_changes() {
    let c = signal(rgb(0x2060c0));
    let other = signal(0);
    let Some(mut f) = Frames::new(200, 120, 1.0, move || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(
                gpu_canvas::<Fill>(move || c.get())
                    .w(80.0)
                    .h(40.0)
                    .rounded(8.0)
                    .child(text("on top")),
            )
            .child(text(move || format!("{}", other.get())))
    }) else {
        return;
    };
    assert_eq!(drawn(), 1);

    other.set(1);
    f.next();
    assert_eq!(drawn(), 1, "something else changed");

    c.set(rgb(0x2060c0));
    let (_, d) = f.next();
    assert_eq!((drawn(), d.rects().len()), (1, 0), "the same color");

    c.set(rgb(0xc06020));
    let (p, d) = f.next();
    assert_eq!(drawn(), 2);
    assert_eq!(d.rects(), &[RectI::new(0, 0, 80, 40)], "only its box");
    assert_eq!(p, f.all_at_once(), "in parts as at once");
    assert_eq!(px(&p, 200, 70, 35), [0xc0, 0x60, 0x20, 255]);
}

#[test]
fn light_given_through_the_srgb_view_is_stored_as_colors_are() {
    let Some(f) = Frames::new(100, 100, 1.0, || {
        gpu_canvas::<Light>(|| 0.5).w(100.0).h(100.0)
    }) else {
        return;
    };
    // sRGB's 0.5 of light is 188 of 255.
    let [r, g, b, a] = px(&f.pixels(), 100, 50, 50);
    assert!(
        (187..=189).contains(&r) && r == g && g == b && a == 255,
        "{r} {g} {b} {a}"
    );
}

#[test]
fn what_goes_wrong_in_it_draws_nothing_and_breaks_nothing_else() {
    let n = signal(0u32);
    let Some(mut f) = Frames::new(200, 100, 1.0, move || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .flex()
            .child(gpu_canvas::<BadDraw>(move || n.get()).w(50.0).h(50.0))
            .child(gpu_canvas::<BadMade>(|| ()).w(50.0).h(50.0))
            .child(gpu_canvas::<Fill>(|| rgb(0x2060c0)).w(50.0).h(50.0))
    }) else {
        return;
    };
    let check = |p: &[u8]| {
        assert_eq!(px(p, 200, 25, 25), [255, 255, 255, 255], "a bad pass");
        assert_eq!(px(p, 200, 75, 25), [255, 255, 255, 255], "made badly");
        assert_eq!(px(p, 200, 125, 25), [0x20, 0x60, 0xc0, 255], "the good one");
    };
    check(&f.pixels());
    // `BadMade` was never asked to draw; `BadDraw` and `Fill` were.
    assert_eq!(drawn(), 2);
    n.set(1);
    let (p, _) = f.next();
    check(&p);
    assert_eq!(drawn(), 3, "tried again with what changed");
}

#[test]
fn its_texture_goes_with_it() {
    let here = signal(true);
    let Some(mut f) = Frames::new(100, 100, 1.0, move || {
        div().child(show(
            move || here.get(),
            || gpu_canvas::<Fill>(|| rgb(0x2060c0)).w(50.0).h(50.0),
        ))
    }) else {
        return;
    };
    assert_eq!(f.renderer.canvas_textures(), 1);
    here.set(false);
    f.next();
    f.next();
    assert_eq!(f.renderer.canvas_textures(), 0);
}
