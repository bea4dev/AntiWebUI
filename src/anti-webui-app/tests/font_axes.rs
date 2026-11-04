//! The axes of variable fonts through the real renderer (see `docs/design/17-font-axes.md`).
//! Skipped when no GPU adapter is available.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_wgpu::{Gpu, Renderer, wgpu};
use std::time::Instant;

static FONT: &[u8] = include_bytes!("../../anti-webui-md3e/fonts/RobotoFlex.ttf");
const FAMILY: &str = "Axes";

fn page(view: Text) -> Div {
    div()
        .size_full()
        .bg(Color::WHITE)
        .text_color(Color::BLACK)
        .font_family(FAMILY)
        .child(view)
}

fn render(w: u32, h: u32, view: impl FnOnce() -> Text) -> Option<Vec<u8>> {
    register_font_as(FONT, FAMILY).unwrap();
    match anti_webui_app::headless::render_rgba(w, h, 1.0, || page(view())) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

/// How much is painted on white.
fn ink(p: &[u8]) -> u64 {
    p.chunks_exact(4).map(|p| 255 - p[0] as u64).sum()
}

fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().unwrap()
}

#[test]
fn axes_change_the_picture_as_they_should() {
    let draw = |view: fn() -> Text| on_thread(move || render(400, 100, view));
    let Some(plain) = draw(|| text("Handgloves").text_size(40.0)) else {
        return;
    };
    let bold = draw(|| text("Handgloves").text_size(40.0).font_axis(b"wght", 800.0)).unwrap();
    let graded = draw(|| text("Handgloves").text_size(40.0).font_axis(b"GRAD", 150.0)).unwrap();
    let thin = draw(|| text("Handgloves").text_size(40.0).font_axis(b"wght", 100.0)).unwrap();
    assert!(ink(&bold) > ink(&plain), "{} {}", ink(&bold), ink(&plain));
    assert!(ink(&graded) > ink(&plain));
    assert!(ink(&thin) < ink(&plain));
    assert_ne!(graded, bold);
}

#[test]
fn optical_size_changes_big_text_as_chrome_does() {
    let draw = |view: fn() -> Text| on_thread(move || render(800, 200, view));
    let Some(auto) = draw(|| text("Hamburg").text_size(100.0)) else {
        return;
    };
    let same = draw(|| text("Hamburg").text_size(100.0).font_axis(b"opsz", 100.0)).unwrap();
    assert_eq!(auto, same);
    let fixed = draw(|| text("Hamburg").text_size(100.0).font_axis(b"opsz", 14.0)).unwrap();
    assert_ne!(auto, fixed);
}

fn sized(ui: &mut Ui, w: i32, h: i32) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w, h),
        scale: Scale::ONE,
    });
    ui.update();
}

#[test]
fn text_that_keeps_moving_along_its_axes_keeps_the_atlas_in_check() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    register_font_as(FONT, FAMILY).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let grade = signal(0.0_f32);
    let width = signal(100.0_f32);
    let (w, h) = (1000, 140);
    let mut ui = Ui::new(move || {
        page(
            text("Sphinx of black quartz")
                .text_size(70.0)
                .dyn_style(move |s| {
                    s.font_axis(b"GRAD", grade.get())
                        .font_axis(b"wdth", width.get())
                }),
        )
    });
    sized(&mut ui, w, h);
    let mut pixels = Vec::new();
    // Two axes that don't meet in the same place again for a long time.
    for i in 0..400 {
        grade.set((i * 7 % 64) as f32 * 350.0 / 64.0 - 200.0);
        width.set(25.0 + (i * 11 % 61) as f32 * 2.0);
        ui.update();
        let scene = ui.paint(Instant::now());
        pixels = gpu.render_to_rgba(&mut renderer, &scene);
    }
    assert!(renderer.atlas_size() <= 2048, "{}", renderer.atlas_size());

    // What was cleared comes back as it should.
    let again = {
        let mut fresh = Renderer::new(&gpu.device, format);
        let scene = ui.paint(Instant::now());
        gpu.render_to_rgba(&mut fresh, &scene)
    };
    assert_eq!(pixels, again);
}

#[test]
fn more_than_fits_in_one_frame_makes_the_atlas_grow() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    register_font_as(FONT, FAMILY).unwrap();
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let mut ui = Ui::new(|| {
        div()
            .size_full()
            .bg(Color::WHITE)
            .font_family(FAMILY)
            .text_size(150.0)
            .children((0..60).map(|i| {
                div()
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .font_axis(b"XTRA", 323.0 + i as f32 * 4.5)
                    .child("abcdefghijklmnopqrstuvwxyz")
            }))
    });
    sized(&mut ui, 2600, 220);
    let scene = ui.paint(Instant::now());
    let p = gpu.render_to_rgba(&mut renderer, &scene);
    assert!(renderer.atlas_size() >= 4096, "{}", renderer.atlas_size());
    assert!(ink(&p) > 0);
}
