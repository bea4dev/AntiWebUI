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
                source: ScrollSource::Unknown,
                phase: ScrollPhase::Single,
                time: 0,
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
        full_renderer.clear_layer_caches();
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

/// A scene that scrolls both ways like a map: tall and wide, with text, rounded corners, shadows
/// and a nested scroller, all in a rounded viewport.
fn map_like(view: NodeRef, inner: NodeRef, cell: NodeRef) -> Div {
    div().bg(Color::WHITE).h(pct(100.0)).p(10.0).child(
        div()
            .h(240.0)
            .w(320.0)
            .rounded(14.0)
            .bg(rgb(0xf8f8f0))
            .overflow_scroll()
            .node_ref(view)
            .child(
                div()
                    .w(1500.0)
                    .flex()
                    .flex_col()
                    .gap(4.0)
                    .child(
                        div()
                            .w(160.0)
                            .h(70.0)
                            .overflow_y_scroll()
                            .bg(rgb(0xddeeff))
                            .node_ref(inner)
                            .children(
                                (0..10).map(|i| div().h(22.0).child(text(format!("in {i}")))),
                            ),
                    )
                    .children((0..40).map(move |row| {
                        div().flex().gap(4.0).children((0..14).map(move |col| {
                            let d = div()
                                .w(100.0)
                                .h(34.0)
                                .rounded(6.0)
                                .shadow_sm()
                                .bg(if (row + col) % 2 == 0 {
                                    rgb(0xffe0e0)
                                } else {
                                    rgb(0xe0ffe0)
                                })
                                .child(text(format!("{row}:{col} 地図")));
                            if row == 3 && col == 1 {
                                d.hover(|s| s.bg(rgb(0x3366ff))).node_ref(cell)
                            } else {
                                d
                            }
                        }))
                    })),
            ),
    )
}

/// Panning in any direction through the cached texture must give exactly the pixels of drawing
/// the content from scratch, including where it wraps around the texture.
#[test]
fn scroll_layers_match_a_fresh_render() {
    use anti_webui_core::Ui;
    use anti_webui_core::damage;
    use anti_webui_core::event::{Modifiers, WindowEvent};
    use anti_webui_core::geometry::{PointF, Scale, SizeI};
    use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
    use std::time::Instant;

    let gpu = match Gpu::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (500u32, 400u32);
    let (view, inner, cell) = (node_ref(), node_ref(), node_ref());
    let mut ui = Ui::new(move || map_like(view, inner, cell));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w as i32, h as i32),
        scale: Scale::from_f64(1.25),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut cached = Renderer::new(&gpu.device, format);
    let mut fresh = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(&gpu, format, w, h);
    let now = Instant::now();
    let mut prev = ui.paint(now);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    cached.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);

    let r = view.rect().unwrap();
    let center = PointF::new(
        r.x as f32 + r.width as f32 * 0.75,
        r.y as f32 + r.height as f32 * 0.75,
    );
    // Small steps both ways, diagonals, reversals, jumps of more than a viewport, and past the
    // texture's size so that everything wraps.
    let mut pans: Vec<(f32, f32)> = vec![
        (0.0, 7.0),
        (9.0, 0.0),
        (13.0, 17.0),
        (-5.0, 3.0),
        (-30.0, -20.0),
        (600.0, 450.0),
        (-200.0, 90.0),
        (333.0, -41.0),
        (-11.0, -11.0),
    ];
    pans.extend((0..40).map(|i| (37.0, if i % 2 == 0 { 53.0 } else { -3.0 })));
    pans.extend((0..30).map(|_| (-41.0, -29.0)));
    for (i, (dx, dy)) in pans.iter().enumerate() {
        ui.handle_event(WindowEvent::Scroll {
            position: center,
            delta_x: *dx,
            delta_y: *dy,
            modifiers: Modifiers::default(),
            source: ScrollSource::Unknown,
            phase: ScrollPhase::Single,
            time: 0,
        });
        // Now and then the content changes too: hover follows whatever is under the pointer.
        if i % 7 == 3 {
            ui.handle_event(WindowEvent::PointerMoved {
                position: PointF::new(center.x - i as f32, center.y),
                modifiers: Modifiers::default(),
            });
        }
        ui.update();
        let scene = ui.paint(now);
        let d = damage::diff(&prev, &scene);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        cached.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        let pixels = target.read_rgba(&gpu);
        fresh.clear_layer_caches();
        let expected = gpu.render_to_rgba(&mut fresh, &scene);
        let differing = pixels
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differing, 0,
            "pan {i} ({dx}, {dy}): {differing} pixels differ"
        );
        prev = scene;
    }

    // Inertia at full speed on a diagonal, which moves further than a screen every few frames.
    let m = Modifiers::default();
    for (phase, time) in [
        (ScrollPhase::Began, 1000),
        (ScrollPhase::Moved, 1010),
        (ScrollPhase::Moved, 1020),
        (ScrollPhase::Lifted { fling: true }, 1025),
    ] {
        let lifted = matches!(phase, ScrollPhase::Lifted { .. });
        ui.handle_event(WindowEvent::Scroll {
            position: center,
            delta_x: if lifted { 0.0 } else { 60.0 },
            delta_y: if lifted { 0.0 } else { 200.0 },
            modifiers: m,
            source: ScrollSource::Touchpad,
            phase,
            time,
        });
    }
    assert!(ui.animating());
    let start = Instant::now();
    let mut frame = 0;
    while ui.animating() {
        frame += 1;
        ui.animate(start + std::time::Duration::from_millis(frame * 16));
        ui.update();
        let scene = ui.paint(now);
        let d = damage::diff(&prev, &scene);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        cached.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        let pixels = target.read_rgba(&gpu);
        fresh.clear_layer_caches();
        let expected = gpu.render_to_rgba(&mut fresh, &scene);
        assert!(pixels == expected, "inertia frame {frame} differs");
        prev = scene;
    }
    assert!(frame > 5, "only {frame} frames of inertia");

    // A nested scroller redraws only itself in the outer layer.
    ui.handle_event(WindowEvent::Scroll {
        position: center,
        delta_x: -5000.0,
        delta_y: -5000.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let at_start = ui.paint(now);
    let d = damage::diff(&prev, &at_start);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    cached.render_damaged(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &target.view,
        &at_start,
        &d,
    );
    gpu.queue.submit([encoder.finish()]);
    let i = inner.rect().unwrap();
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(i.x as f32 + 5.0, i.y as f32 + 5.0),
        delta_x: 0.0,
        delta_y: 15.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let scene = ui.paint(now);
    let d = damage::diff(&at_start, &scene);
    assert!(d.area() <= i.area(), "{d:?} vs {i:?}");
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    cached.render_damaged(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &target.view,
        &scene,
        &d,
    );
    gpu.queue.submit([encoder.finish()]);
    let pixels = target.read_rgba(&gpu);
    fresh.clear_layer_caches();
    assert!(
        pixels == gpu.render_to_rgba(&mut fresh, &scene),
        "nested scroll differs"
    );
}

/// Drawing through a layer looks the same as drawing directly, within 8-bit rounding.
#[test]
fn layers_look_like_direct_drawing() {
    let content = || {
        (0..4).map(|i| {
            div()
                .h(40.0)
                .m(4.0)
                .rounded(8.0)
                .shadow_md()
                .bg(rgba(0x3366ff80))
                .child(text(format!("半透明の行 {i}")))
        })
    };
    let (w, h) = (300u32, 200u32);
    let frame = |inner: Div| {
        div()
            .bg(rgb(0xffeecc))
            .h(pct(100.0))
            .p(10.0)
            .child(inner.w(260.0).h(160.0).rounded(12.0).bg(rgb(0xccffee)))
    };
    let Some(layered) = render(w, h, 1.25, || {
        frame(
            div()
                .overflow_y_scroll()
                .children(content())
                .child(div().h(400.0)),
        )
    }) else {
        return;
    };
    let Some(direct) = render(w, h, 1.25, || {
        frame(
            div()
                .overflow_clip()
                .children(content())
                .child(div().h(400.0)),
        )
    }) else {
        return;
    };
    let mut worst = 0;
    for y in 0..h {
        // The scrollbar is only in the scrolling version.
        for x in 0..w - 40 {
            let (a, b) = (px(&layered, w, x, y), px(&direct, w, x, y));
            for c in 0..4 {
                worst = worst.max(a[c].abs_diff(b[c]));
            }
        }
    }
    assert!(worst <= 2, "layered and direct drawing differ by {worst}");
}

/// Coverage of a rounded rectangle blurred by a Gaussian, by adding up the real thing.
fn true_blur(x: f32, y: f32, rect: (f32, f32, f32, f32), r: f32, sigma: f32) -> f32 {
    let inside = |px: f32, py: f32| {
        let (dx, dy) = (
            (px - (rect.0 + rect.2 / 2.0)).abs() - (rect.2 / 2.0 - r),
            (py - (rect.1 + rect.3 / 2.0)).abs() - (rect.3 / 2.0 - r),
        );
        dx.max(0.0).hypot(dy.max(0.0)) <= r && dx.max(dy) <= r
    };
    let reach = (sigma * 4.0) as i32;
    let (mut sum, mut total) = (0.0, 0.0);
    for j in -reach * 2..=reach * 2 {
        for i in -reach * 2..=reach * 2 {
            let (u, v) = (i as f32 / 2.0, j as f32 / 2.0);
            let g = (-(u * u + v * v) / (2.0 * sigma * sigma)).exp();
            total += g;
            if inside(x + u, y + v) {
                sum += g;
            }
        }
    }
    sum / total
}

#[test]
fn shadows_are_a_real_gaussian_blur_outside_of_the_box() {
    // CSS asks for each pixel to be within 5% of the exact result.
    let (w, h) = (240u32, 200u32);
    for (bw, bh, r, blur) in [
        (100.0, 60.0, 0.0, 20.0),
        (100.0, 60.0, 16.0, 20.0),
        (20.0, 20.0, 4.0, 30.0),
        (100.0, 60.0, 30.0, 8.0),
    ] {
        let Some(p) = render(w, h, 1.0, move || {
            div()
                .bg(Color::WHITE)
                .h(pct(100.0))
                .pl(70.0)
                .pt(70.0)
                .child(div().w(bw).h(bh).rounded(r).shadow(BoxShadow {
                    blur,
                    color: Color::BLACK,
                    ..Default::default()
                }))
        }) else {
            return;
        };
        let rect = (70.0, 70.0, bw, bh);
        let mut worst: f32 = 0.0;
        for y in (0..h).step_by(3) {
            for x in (0..w).step_by(3) {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                // The box itself is left out; only what's clearly outside of it counts here.
                if cx >= rect.0 - 1.0
                    && cx <= rect.0 + bw + 1.0
                    && cy >= rect.1 - 1.0
                    && cy <= rect.1 + bh + 1.0
                {
                    continue;
                }
                let got = 1.0 - px(&p, w, x, y)[0] as f32 / 255.0;
                let want = true_blur(cx, cy, rect, r, blur / 2.0);
                worst = worst.max((got - want).abs());
            }
        }
        assert!(worst < 0.05, "{bw}x{bh} r{r} blur{blur}: off by {worst}");
    }
}

#[test]
fn shadows_stay_out_of_the_box_and_are_ordered_like_css() {
    let (w, h) = (200, 120);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).p(30.0).child(
            div()
                .w(100.0)
                .h(50.0)
                .rounded(8.0)
                .bg(rgba(0x00ff0040))
                .shadows(vec![
                    // The first goes on top.
                    BoxShadow {
                        offset_x: 10.0,
                        color: rgb(0xff0000),
                        ..Default::default()
                    },
                    BoxShadow {
                        offset_x: 20.0,
                        color: rgb(0x0000ff),
                        ..Default::default()
                    },
                ]),
        )
    }) else {
        return;
    };
    // Under the translucent box there is only the page and the box.
    let inside = px(&p, w, 80, 55);
    assert!(
        inside[0] == inside[2] && inside[1] > inside[0],
        "a shadow shows through the box: {inside:?}"
    );
    assert_eq!(px(&p, w, 135, 55), [255, 0, 0, 255]);
    assert_eq!(px(&p, w, 145, 55), [0, 0, 255, 255]);
    assert_eq!(px(&p, w, 155, 55), [255, 255, 255, 255]);
}

#[test]
fn an_opaque_box_has_no_light_seam_around_its_corners() {
    let (w, h) = (120, 120);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).p(30.0).child(
            div()
                .w(60.0)
                .h(60.0)
                .rounded(20.0)
                .bg(Color::BLACK)
                .shadow(BoxShadow {
                    color: Color::BLACK,
                    spread: 4.0,
                    ..Default::default()
                }),
        )
    }) else {
        return;
    };
    // Black on black: a leak of the white page would show along the edge of the box.
    for i in 0..24u32 {
        let t = i as f32 / 23.0 * std::f32::consts::FRAC_PI_2;
        let x = 50.0 - 20.0 * t.cos();
        let y = 50.0 - 20.0 * t.sin();
        let c = px(&p, w, x as u32, y as u32);
        assert!(c[0] < 3, "seam at {x},{y}: {c:?}");
    }
}

#[test]
fn inset_shadows_darken_the_inside_edge_only() {
    let (w, h) = (160, 120);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).p(20.0).child(
            div()
                .w(120.0)
                .h(80.0)
                .bg(Color::WHITE)
                .border(2.0)
                .border_color(rgb(0x00ff00))
                .child(div().w(10.0).h(10.0).ml(50.0).mt(30.0).bg(rgb(0xff0000)))
                .shadow(BoxShadow {
                    blur: 10.0,
                    color: Color::BLACK,
                    inset: true,
                    ..Default::default()
                }),
        )
    }) else {
        return;
    };
    assert_eq!(
        px(&p, w, 10, 60),
        [255, 255, 255, 255],
        "outside stays clean"
    );
    assert_eq!(
        px(&p, w, 20, 60),
        [0, 255, 0, 255],
        "the border stays clean"
    );
    let edge = px(&p, w, 22, 60)[0];
    assert!(
        (110..=145).contains(&edge),
        "half covered at the edge: {edge}"
    );
    assert_eq!(
        px(&p, w, 110, 60),
        [255, 255, 255, 255],
        "the middle is clean"
    );
    assert_eq!(
        px(&p, w, 75, 55),
        [255, 0, 0, 255],
        "the children are above"
    );
}

#[test]
fn the_area_a_shadow_skips_is_transparent_anyway() {
    use anti_webui_core::geometry::RectI;
    use anti_webui_core::scene::{Clip, Primitive, Scene, Shadow};
    use anti_webui_wgpu::{Gpu, Renderer, wgpu};
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let (w, h) = (320u32, 240u32);
    let mut draw = |s: Shadow| {
        let scene = Scene {
            width: w,
            height: h,
            clear: Color::TRANSPARENT,
            primitives: vec![Primitive::Shadow(s)],
            chunks: Vec::new(),
        };
        gpu.render_to_rgba(&mut renderer, &scene)
    };
    let rect = RectI::new(60, 50, 200, 140);
    for (blur, radii) in [
        (0.0, [0.0; 4]),
        (12.0, [0.0; 4]),
        (12.0, [30.0; 4]),
        (40.0, [10.0, 30.0, 0.0, 50.0]),
        (6.0, [70.0; 4]),
    ] {
        let shadow = Shadow {
            rect,
            radii,
            blur,
            color: Color::BLACK,
            clip: Clip::NONE,
            cut: RectI::default(),
            cut_radii: [0.0; 4],
            inset: false,
        };
        // With nothing to cut, it draws everywhere: the reference.
        let all = draw(shadow.clone());
        let outer = draw(Shadow {
            cut: rect,
            cut_radii: radii,
            ..shadow.clone()
        });
        let window = RectI::new(0, 0, w as i32, h as i32);
        let inset = draw(Shadow {
            cut: window,
            inset: true,
            ..shadow.clone()
        });
        for y in 0..h {
            for x in 0..w {
                let (a, o, i) = (
                    px(&all, w, x, y)[3] as i32,
                    px(&outer, w, x, y)[3] as i32,
                    px(&inset, w, x, y)[3] as i32,
                );
                // Inside of the rectangle an outer shadow fades out over the antialiased edge.
                let beyond = !rect
                    .outset(1)
                    .contains(anti_webui_core::geometry::PointF::new(x as f32, y as f32));
                if beyond {
                    assert!(
                        (a - o).abs() <= 1,
                        "outer at {x},{y} blur {blur}: {o} vs {a}"
                    );
                }
                assert!(o <= a + 1, "outer at {x},{y} blur {blur}: {o} vs {a}");
                assert!(
                    (i - (255 - a)).abs() <= 2,
                    "inset at {x},{y} blur {blur} radii {radii:?}: {i} vs {}",
                    255 - a
                );
            }
        }
        // And deep in the box nothing is drawn at all.
        let hole = Shadow {
            cut: rect,
            cut_radii: radii,
            ..shadow.clone()
        }
        .hole();
        assert!(!hole.is_empty());
        let (cx, cy) = (hole.x as u32 + 1, hole.y as u32 + 1);
        assert_eq!(px(&outer, w, cx, cy)[3], 0);
        // Nor in the box outside of the hole, which only the shader keeps clean.
        let edge = px(
            &outer,
            w,
            rect.x as u32 + 3,
            (rect.y + rect.height / 2) as u32,
        );
        assert_eq!(edge[3], 0, "blur {blur} radii {radii:?}");
    }
}

#[test]
fn a_popup_has_its_own_transparent_scene() {
    use anti_webui_core::Ui;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::geometry::{PointI, Scale, SizeI};
    use anti_webui_core::platform::PlatformRequest;
    use anti_webui_wgpu::{Gpu, Renderer, wgpu};
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let mut ui = Ui::new(|| {
        div().size_full().bg(rgb(0x0000ff)).child(
            popup().child(
                div()
                    .w(60.0)
                    .h(40.0)
                    .rounded(10.0)
                    .bg(rgb(0xff0000))
                    .shadow(BoxShadow {
                        blur: 10.0,
                        color: Color::BLACK,
                        ..Default::default()
                    }),
            ),
        )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(200, 200),
        scale: Scale::ONE,
    });
    ui.update();
    let params = ui
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::ShowPopup(p) => Some(p),
            _ => None,
        })
        .unwrap();
    ui.handle_event(WindowEvent::PopupConfigured {
        id: params.id,
        origin: PointI::new(50, 50),
        size: params.wanted,
    });
    ui.update();
    let frame = ui.paint_frame(std::time::Instant::now());
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let scene = &frame.popups[0].1;
    let p = gpu.render_to_rgba(&mut renderer, scene);
    let g = params.geometry;
    let (cx, cy) = ((g.x + 30) as u32, (g.y + 20) as u32);
    assert_eq!(px(&p, scene.width, cx, cy), [255, 0, 0, 255]);
    assert_eq!(
        px(&p, scene.width, 0, 0),
        [0, 0, 0, 0],
        "transparent around it"
    );
    let below = px(&p, scene.width, cx, (g.bottom() + 2) as u32);
    assert!(
        below[3] > 40 && below[3] < 200,
        "the shadow fades: {below:?}"
    );
    assert_eq!(below[0], 0, "and is premultiplied black");
    // The window below is untouched by it.
    let window = gpu.render_to_rgba(&mut renderer, &frame.window);
    assert_eq!(px(&window, 200, 100, 100), [0, 0, 255, 255]);
}
