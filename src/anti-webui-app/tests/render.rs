//! Pixel tests through the real renderer. Skipped when no GPU adapter is available.

use anti_webui_core::prelude::*;

fn render(w: u32, h: u32, scale: f64, view: impl FnOnce() -> Div) -> Option<Vec<u8>> {
    match anti_webui_app::headless::render_rgba(w, h, scale, view) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

#[test]
fn rounded_overflow_clips_children() {
    let (w, h) = (200, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).p(20.0).child(
            div()
                .w(120.0)
                .h(120.0)
                .rounded(30.0)
                .overflow_y_scroll()
                .child(div().h(400.0).bg(rgb(0xff0000))),
        )
    }) else {
        return;
    };
    // The corner of the scroll view's bounding box lies outside its rounded shape.
    assert_eq!(
        px(&p, w, 22, 22),
        [255, 255, 255, 255],
        "child leaked out of the rounded corner"
    );
    // The middle is covered by the child.
    assert_eq!(px(&p, w, 80, 80), [255, 0, 0, 255]);
    // Along the straight edge the child reaches the box edge exactly.
    assert_eq!(px(&p, w, 20, 80), [255, 0, 0, 255]);
    assert_eq!(px(&p, w, 19, 80), [255, 255, 255, 255]);
}

#[test]
fn borders_are_crisp_at_fractional_scale() {
    let (w, h) = (180, 90);
    let Some(p) = render(w, h, 1.5, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            div()
                .ml(10.0)
                .mt(10.0)
                .w(33.0)
                .h(20.0)
                .border(1.0)
                .border_color(Color::BLACK),
        )
    }) else {
        return;
    };
    // At 1.5x: box at (15,15) size 50x30, 1px border → 2 physical pixels, fully opaque.
    for x in [15, 16, 63, 64] {
        assert_eq!(px(&p, w, x, 30), [0, 0, 0, 255], "x={x}");
    }
    assert_eq!(px(&p, w, 14, 30), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 17, 30), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 65, 30), [255, 255, 255, 255]);
}

/// Partial repaints must produce exactly the pixels of a full repaint.
#[test]
fn damaged_repaint_matches_full_repaint() {
    use anti_webui_core::Ui;
    use anti_webui_core::damage;
    use anti_webui_core::event::{Modifiers, PointerButton, WindowEvent};
    use anti_webui_core::geometry::{PointF, Scale, SizeI};
    use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
    use std::time::{Duration, Instant};

    let gpu = match Gpu::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (360u32, 300u32);
    let field = node_ref();
    let hover = node_ref();
    let scroller = node_ref();
    let label = signal("AAAA".to_string());
    let shift = signal(0.0f32);
    let mut ui = Ui::new(move || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .p(10.0)
            .gap(6.0)
            .flex_col()
            .child(text(
                "Damage tracking keeps everything else untouched. 選択も部分再描画。",
            ))
            .child(
                div()
                    .flex()
                    .gap(4.0)
                    .child(text(move || label.get()))
                    .child(
                        div()
                            .w(16.0)
                            .h(16.0)
                            .bg(rgb(0x3366ff))
                            .dyn_style(move |s| s.ml(shift.get())),
                    ),
            )
            .child(
                div()
                    .h(28.0)
                    .rounded(6.0)
                    .bg(rgb(0xeeeeee))
                    .shadow_md()
                    .hover(|s| s.bg(rgb(0xcce0ff)).rounded(10.0))
                    .node_ref(hover)
                    .child(text("hover me")),
            )
            .child(
                text_input()
                    .w(200.0)
                    .node_ref(field)
                    .value(signal("edit".to_string())),
            )
            .child(
                div()
                    .h(80.0)
                    .rounded(12.0)
                    .overflow_y_scroll()
                    .node_ref(scroller)
                    .children((0..12).map(|i| {
                        div()
                            .h(20.0)
                            .bg(if i % 2 == 0 {
                                rgb(0xf4f4f4)
                            } else {
                                rgb(0xe0e0e0)
                            })
                            .child(text(format!("row {i}")))
                    })),
            )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w as i32, h as i32),
        scale: Scale::from_f64(1.25),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();

    let center = |r: anti_webui_core::geometry::RectI| {
        PointF::new(
            r.x as f32 + r.width as f32 / 2.0,
            r.y as f32 + r.height as f32 / 2.0,
        )
    };
    let click = |ui: &mut Ui, p: PointF| {
        for pressed in [true, false] {
            ui.handle_event(WindowEvent::PointerButton {
                position: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::default(),
                time: 0,
            });
        }
    };

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut partial_renderer = Renderer::new(&gpu.device, format);
    let mut full_renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(&gpu, format, w, h);
    let start = Instant::now();
    let mut prev = ui.paint(start);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    partial_renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);

    type Step = Box<dyn Fn(&mut Ui)>;
    let steps: Vec<Step> = vec![
        Box::new(move |ui| {
            ui.handle_event(WindowEvent::PointerMoved {
                position: center(hover.rect().unwrap()),
                modifiers: Modifiers::default(),
            })
        }),
        // Glyphs alone change (with descenders), then a box jumps away from where it was.
        Box::new(move |_| label.set("gjpq".to_string())),
        Box::new(move |_| shift.set(150.0)),
        Box::new(move |ui| click(ui, center(field.rect().unwrap()))),
        Box::new(|_| {}), // caret blinks off
        Box::new(move |ui| {
            ui.handle_event(WindowEvent::Scroll {
                position: center(scroller.rect().unwrap()),
                delta_x: 0.0,
                delta_y: 37.0,
                modifiers: Modifiers::default(),
            })
        }),
        Box::new(move |ui| {
            let r = scroller.rect().unwrap();
            for _ in 0..2 {
                click(ui, PointF::new(r.x as f32 + 8.0, r.y as f32 + 30.0));
            }
        }),
    ];
    for (i, step) in steps.iter().enumerate() {
        step(&mut ui);
        ui.update();
        let now = start + Duration::from_millis(600 * (i as u64 + 1));
        let scene = ui.paint(now);
        let d = damage::diff(&prev, &scene);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        partial_renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        let partial = target.read_rgba(&gpu);
        let full = gpu.render_to_rgba(&mut full_renderer, &scene);
        let differing = partial
            .chunks_exact(4)
            .zip(full.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differing, 0,
            "step {i}: {differing} pixels differ (damage {d:?})"
        );
        assert!(
            d.area() < (w * h) as i64,
            "step {i}: damage should not cover the whole window: {d:?}"
        );
        prev = scene;
    }
}
