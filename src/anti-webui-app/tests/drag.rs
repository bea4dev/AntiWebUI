//! What is dragged, through the real renderer. Skipped when no GPU adapter is available. See
//! `docs/design/23-drag-and-drop.md`.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::{Modifiers, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::Instant;

#[derive(Clone)]
struct Card;

fn gpu() -> Option<Gpu> {
    match Gpu::headless() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn mount<V: IntoView>(w: i32, h: i32, scale: f64, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w, h),
        scale: Scale::from_f64(scale),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn moved(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

fn pressed(ui: &mut Ui, x: f32, y: f32) {
    moved(ui, x, y);
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

fn page(preview: bool) -> Div {
    let source = div()
        .absolute()
        .left(20.0)
        .top(20.0)
        .w(40.0)
        .h(40.0)
        .bg(rgb(0xff0000))
        .dragging(|s| s.bg(rgb(0x0000ff)))
        .draggable(|| Card);
    let source = if preview {
        source.drag_preview(|_: &Card| div().w(20.0).h(20.0).bg(rgb(0x00ff00)))
    } else {
        source
    };
    div().bg(Color::WHITE).h(pct(100.0)).child(source)
}

#[test]
fn it_shows_what_was_there_with_the_pointer_and_the_change_on_the_node() {
    let Some(gpu) = gpu() else {
        return;
    };
    let (w, h) = (200u32, 200u32);
    let mut ui = mount(w as i32, h as i32, 1.0, || page(false));
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    pressed(&mut ui, 30.0, 30.0);
    moved(&mut ui, 130.0, 130.0);
    let scene = ui.paint(Instant::now());
    let p = gpu.render_to_rgba(&mut renderer, &scene);
    // Where it was: what `dragging` says.
    assert_eq!(px(&p, w, 40, 40), [0, 0, 255, 255]);
    // Moved by 100, in the color it had when it started, at 0.75 over white.
    let shown = px(&p, w, 140, 140);
    assert!((shown[0] as i32 - 255).abs() <= 1, "{shown:?}");
    assert!((shown[1] as i32 - 64).abs() <= 1, "{shown:?}");
    assert!((shown[2] as i32 - 64).abs() <= 1, "{shown:?}");
    // And nowhere else.
    assert_eq!(px(&p, w, 110, 110), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 165, 165), [255, 255, 255, 255]);
}

#[test]
fn a_preview_of_your_own_shows_only_there() {
    let Some(gpu) = gpu() else {
        return;
    };
    let (w, h) = (200u32, 200u32);
    let mut ui = mount(w as i32, h as i32, 1.0, || page(true));
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    pressed(&mut ui, 30.0, 30.0);
    moved(&mut ui, 130.0, 130.0);
    let scene = ui.paint(Instant::now());
    let p = gpu.render_to_rgba(&mut renderer, &scene);
    // 25% across the box, with the pointer 25% across the preview.
    assert_eq!(px(&p, w, 126, 126), [0, 255, 0, 255]);
    assert_eq!(px(&p, w, 144, 144), [0, 255, 0, 255]);
    assert_eq!(px(&p, w, 124, 124), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 146, 146), [255, 255, 255, 255]);
    // Not at the top left of the window, where its portal lays it out.
    assert_eq!(px(&p, w, 5, 5), [255, 255, 255, 255]);
}

#[test]
fn moving_it_repaints_little_and_the_same_as_all_of_it() {
    let Some(gpu) = gpu() else {
        return;
    };
    let (w, h) = (300u32, 300u32);
    let mut ui = mount(w as i32, h as i32, 1.8, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(text("Text underneath ".repeat(200)))
            .child(
                div()
                    .absolute()
                    .left(10.0)
                    .top(10.0)
                    .w(30.0)
                    .h(30.0)
                    .rounded(6.0)
                    .bg(rgb(0xff0000))
                    .child(text("Aa"))
                    .draggable(|| Card),
            )
    });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut partial = Renderer::new(&gpu.device, format);
    let mut full = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(&gpu, format, w, h);
    pressed(&mut ui, 30.0, 30.0);
    moved(&mut ui, 60.0, 60.0);
    let mut prev = ui.paint(Instant::now());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    partial.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    for i in 1..20 {
        let (x, y) = (60.0 + i as f32 * 7.3, 60.0 + i as f32 * 3.1);
        moved(&mut ui, x, y);
        let scene = ui.paint(Instant::now());
        let d = damage::diff(&prev, &scene);
        assert!(!d.is_empty());
        assert!(
            (d.area() as f64) < w as f64 * h as f64 * 0.35,
            "most of the window stays: {} of {}",
            d.area(),
            w * h
        );
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        partial.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        let a = target.read_rgba(&gpu);
        full.clear_layer_caches();
        let b = gpu.render_to_rgba(&mut full, &scene);
        let differing = a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count();
        assert_eq!(differing, 0, "step {i}");
        prev = scene;
    }
}

#[test]
fn the_surface_of_a_drag_has_it_at_its_corner() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut ui = mount(200, 200, 2.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            div()
                .absolute()
                .left(50.0)
                .top(50.0)
                .w(20.0)
                .h(10.0)
                .bg(rgb(0xff0000))
                .draggable(|| DragData::new(Card).text("x")),
        )
    });
    ui.set_platform_drag(true);
    pressed(&mut ui, 110.0, 105.0);
    moved(&mut ui, 150.0, 150.0);
    let frame = ui.paint_frame(Instant::now());
    let icon = frame.drag_icon.expect("it has its own surface");
    assert_eq!((icon.scene.width, icon.scene.height), (40, 20));
    assert_eq!((icon.logical.width, icon.logical.height), (20, 10));
    assert_eq!((icon.hotspot.x, icon.hotspot.y), (5, 3));
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let p = gpu.render_to_rgba(&mut renderer, &icon.scene);
    let corner = px(&p, 40, 0, 0);
    let end = px(&p, 40, 39, 19);
    assert_eq!(corner, end);
    assert_eq!(corner[3], 191, "see-through as it shows: {corner:?}");
}

fn selected_words() -> Div {
    div()
        .w(pct(100.0))
        .h(pct(100.0))
        .bg(rgb(0x00ff00))
        .selection_color(rgb(0x0000ff))
        .text_color(rgb(0xff0000))
        .child(text("Selected words"))
}

fn select_all(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Key(anti_webui_core::event::KeyEvent {
        key: anti_webui_core::event::Key::Character("a".into()),
        text: None,
        modifiers: Modifiers::PRIMARY,
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

#[test]
fn the_surface_of_selected_text_has_only_the_text_on_nothing() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut ui = mount(400, 200, 1.8, selected_words);
    ui.set_platform_drag(true);
    select_all(&mut ui);
    pressed(&mut ui, 20.0, 15.0);
    moved(&mut ui, 60.0, 15.0);
    let frame = ui.paint_frame(Instant::now());
    let icon = frame.drag_icon.expect("a surface");
    let (w, h) = (icon.scene.width, icon.scene.height);
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let p = gpu.render_to_rgba(&mut renderer, &icon.scene);

    let (mut empty, mut strongest) = (0, 0u8);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b, a] = px(&p, w, x, y);
            // Premultiplied red at most: no page, and no highlight.
            assert_eq!((g, b), (0, 0), "{x},{y}");
            assert!(r <= a, "{x},{y}");
            empty += (a == 0) as u32;
            strongest = strongest.max(a);
        }
    }
    assert!(empty > w * h / 4, "{empty} of {}", w * h);
    // The inside of a stroke, at the opacity of the picture.
    assert!((strongest as i32 - 191).abs() <= 2, "{strongest}");
    assert_eq!(px(&p, w, 0, 0)[3], 0);
}

#[test]
fn a_caret_where_it_would_land_repaints_the_same_as_all_of_it() {
    let Some(gpu) = gpu() else {
        return;
    };
    let (w, h) = (300u32, 300u32);
    let value = signal("0123456789 0123456789".to_string());
    let mut ui = mount(w as i32, h as i32, 1.8, move || {
        selected_words().child(
            text_input()
                .absolute()
                .left(10.0)
                .top(60.0)
                .w(140.0)
                .h(30.0)
                .bg(Color::WHITE)
                .caret_color(rgb(0x000000))
                .value(value),
        )
    });
    select_all(&mut ui);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut partial = Renderer::new(&gpu.device, format);
    let mut full = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(&gpu, format, w, h);
    pressed(&mut ui, 20.0, 15.0);
    moved(&mut ui, 40.0, 40.0);
    let mut prev = ui.paint(Instant::now());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    partial.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    for i in 0..30 {
        let (x, y) = (30.0 + i as f32 * 7.3, 120.0 + (i % 3) as f32 * 40.0);
        moved(&mut ui, x, y);
        let scene = ui.paint(Instant::now());
        let dmg = damage::diff(&prev, &scene);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        partial.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &dmg,
        );
        gpu.queue.submit([encoder.finish()]);
        let got = target.read_rgba(&gpu);
        full.clear_layer_caches();
        let want = gpu.render_to_rgba(&mut full, &scene);
        let differing = got
            .chunks(4)
            .zip(want.chunks(4))
            .filter(|(x, y)| x != y)
            .count();
        assert_eq!(differing, 0, "step {i}");
        prev = scene;
    }
}
