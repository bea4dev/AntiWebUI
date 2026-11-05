//! Shapes on real pixels. See `docs/design/25-svg.md`.

use anti_webui_app::headless::render_rgba;
use anti_webui_core::prelude::*;

const SVG_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/svg");

/// Writes what the files in `tests/data/svg` come out as, for `tools/svg_compare.py`.
#[test]
#[ignore]
fn dump() {
    let out = std::env::var("SVG_DUMP").unwrap();
    let scale: f64 = std::env::var("SVG_SCALE").map_or(1.0, |s| s.parse().unwrap());
    for entry in std::fs::read_dir(SVG_DIR).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let data = std::fs::read(&path).unwrap();
        let (w, h) = (400u32, 300u32);
        let pixels = std::thread::spawn(move || {
            render_rgba(
                (w as f64 * scale) as u32,
                (h as f64 * scale) as u32,
                scale,
                move || {
                    div()
                        .size_full()
                        .bg(Color::WHITE)
                        .child(svg::parse(&data).unwrap())
                },
            )
            .unwrap()
        })
        .join()
        .unwrap();
        std::fs::write(format!("{out}/{name}.rgba"), pixels).unwrap();
    }
}

const W: u32 = 100;

fn px(p: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    p[i..i + 4].try_into().unwrap()
}

fn draw<V: IntoView>(view: impl FnOnce() -> V) -> Vec<u8> {
    render_rgba(W, W, 1.0, view).unwrap()
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn red() -> Color {
    Color::from_rgb(0xff0000)
}

fn blue() -> Color {
    Color::from_rgb(0x0000ff)
}

#[test]
fn a_shape_shows_on_its_pixels() {
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            svg()
                .w(100.0)
                .h(100.0)
                .child(svg::circle(50.0, 50.0, 30.0).fill(red())),
        )
    });
    assert_eq!(px(&p, 50, 50), RED);
    assert_eq!(px(&p, 5, 5), WHITE);
    assert_eq!(px(&p, 18, 50), WHITE);
    assert_eq!(px(&p, 22, 50), RED);
    // The edge is smooth.
    let edge = px(&p, 71, 71);
    assert!(edge[1] > 0 && edge[1] < 255, "{edge:?}");
}

#[test]
fn later_shapes_go_on_top() {
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            svg()
                .w(100.0)
                .h(100.0)
                .child(svg::rect(0.0, 0.0, 60.0, 60.0).fill(red()))
                .child(svg::rect(30.0, 30.0, 60.0, 60.0).fill(blue())),
        )
    });
    assert_eq!(px(&p, 10, 10), RED);
    assert_eq!(px(&p, 40, 40), BLUE);
    assert_eq!(px(&p, 80, 80), BLUE);
}

#[test]
fn text_and_boxes_stay_in_order_with_shapes() {
    let p = draw(|| {
        div()
            .size_full()
            .bg(Color::WHITE)
            .relative()
            .child(
                svg()
                    .w(100.0)
                    .h(100.0)
                    .child(svg::rect(0.0, 0.0, 100.0, 100.0).fill(red())),
            )
            .child(
                div()
                    .absolute()
                    .left(40.0)
                    .top(40.0)
                    .w(20.0)
                    .h(20.0)
                    .bg(blue()),
            )
    });
    assert_eq!(px(&p, 10, 10), RED);
    assert_eq!(px(&p, 50, 50), BLUE);
}

#[test]
fn a_gradient_goes_from_one_color_to_the_other() {
    use anti_webui_core::style::Gradient;
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            svg().w(100.0).h(100.0).child(
                svg::rect(0.0, 0.0, 100.0, 100.0).fill(
                    Gradient::linear(0.0, 0.0, 1.0, 0.0)
                        .stop(0.0, red())
                        .stop(1.0, blue()),
                ),
            ),
        )
    });
    let (left, middle, right) = (px(&p, 1, 50), px(&p, 50, 50), px(&p, 98, 50));
    assert!(left[0] > 245 && left[2] < 10, "{left:?}");
    assert!(right[2] > 245 && right[0] < 10, "{right:?}");
    assert!(
        (middle[0] as i32 - 127).abs() < 6 && (middle[2] as i32 - 127).abs() < 6,
        "{middle:?}"
    );
}

#[test]
fn clipped_by_rounded_corners_around_it() {
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            div()
                .w(100.0)
                .h(100.0)
                .rounded(50.0)
                .overflow_hidden()
                .child(
                    svg()
                        .w(100.0)
                        .h(100.0)
                        .child(svg::rect(0.0, 0.0, 100.0, 100.0).fill(red())),
                ),
        )
    });
    assert_eq!(px(&p, 50, 50), RED);
    assert_eq!(px(&p, 2, 2), WHITE);
}

#[test]
fn in_a_scroller_it_stays_in_its_place() {
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            div().w(100.0).h(100.0).overflow_y_scroll().child(
                svg()
                    .w(100.0)
                    .h(1000.0)
                    .child(svg::rect(0.0, 10.0, 50.0, 50.0).fill(red())),
            ),
        )
    });
    assert_eq!(px(&p, 10, 30), RED);
    assert_eq!(px(&p, 60, 30), WHITE);
}

#[test]
fn rotated_with_a_transform_of_its_box() {
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            svg()
                .w(100.0)
                .h(100.0)
                .rotate(45.0)
                .child(svg::rect(45.0, 0.0, 10.0, 100.0).fill(red())),
        )
    });
    assert_eq!(px(&p, 50, 50), RED);
    assert_eq!(px(&p, 75, 25), RED);
    assert_eq!(px(&p, 25, 25), WHITE);
}

/// A shape far bigger than the window: only what shows of it is made, and it still fills what it
/// should.
#[test]
fn a_huge_shape_comes_out_right() {
    let p = draw(|| {
        div().size_full().bg(Color::WHITE).child(
            svg()
                .w(100.0)
                .h(100.0)
                .view_box(0.0, 0.0, 1.0, 1.0)
                .child(svg::circle(0.5, -9999.5, 10000.0).fill(red())),
        )
    });
    assert_eq!(px(&p, 50, 40), RED);
    assert_eq!(px(&p, 1, 2), RED);
    assert_eq!(px(&p, 50, 60), WHITE);
}

/// What is painted in parts comes out as if painted at once.
#[test]
fn damage_comes_out_the_same() {
    use anti_webui_core::Ui;
    use anti_webui_core::damage;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::geometry::{Scale, SizeI};
    use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
    use std::time::Instant;
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let color = signal(red());
    let at = signal(10.0);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut ui = Ui::new(move || {
        div().size_full().bg(Color::WHITE).child(
            svg()
                .w(100.0)
                .h(100.0)
                .child(svg::circle(30.0, 30.0, 20.0).dyn_style(move |s| s.fill(color.get())))
                .child(
                    svg::rect(0.0, 0.0, 20.0, 20.0)
                        .fill(blue())
                        .dyn_transform(move || svg::Transform::translation(at.get(), 60.0)),
                ),
        )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(W as i32, W as i32),
        scale: Scale(120),
    });
    ui.update();
    let mut prev = ui.paint(Instant::now());
    let target = Offscreen::new(&gpu, format, W, W);
    let mut partial = Renderer::new(&gpu.device, format);
    let mut whole = Renderer::new(&gpu.device, format);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    partial.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    let mut steps: Vec<Box<dyn Fn()>> = vec![
        Box::new(move || color.set(blue())),
        Box::new(move || at.set(40.5)),
        Box::new(move || at.set(70.0)),
    ];
    for step in steps.drain(..) {
        step();
        ui.update();
        let next = ui.paint(Instant::now());
        let d = damage::diff(&prev, &next);
        assert!(d.rects().iter().all(|r| r.area() < (W * W) as i64));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        partial.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &next,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        let got = target.read_rgba(&gpu);
        let want = gpu.render_to_rgba(&mut whole, &next);
        assert!(got == want, "damage {:?}", d.rects());
        prev = next;
    }
}

/// Many shapes at many sizes: what fills the store up starts over, and what is drawn stays right.
#[test]
fn a_store_that_fills_up_starts_over() {
    use anti_webui_core::Ui;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::geometry::{Scale, SizeI};
    use anti_webui_wgpu::{Gpu, Renderer, wgpu};
    use std::time::Instant;
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let zoom = signal(1.0);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut ui = Ui::new(move || {
        div().size_full().bg(Color::WHITE).child(
            svg().w(100.0).h(100.0).child(
                svg::group()
                    .dyn_transform(move || {
                        let z = zoom.get();
                        svg::Transform::scaling(z, z)
                    })
                    .children((0..40).map(|i| {
                        svg::circle(i as f64 * 5.0, 50.0, 12.0)
                            .fill(red())
                            .stroke(blue())
                            .stroke_width(0.5)
                    })),
            ),
        )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(W as i32, W as i32),
        scale: Scale(120),
    });
    let mut renderer = Renderer::new(&gpu.device, format);
    for i in 0..600 {
        zoom.set(1.0 + i as f64 * 0.01);
        ui.update();
        let scene = ui.paint(Instant::now());
        let pixels = gpu.render_to_rgba(&mut renderer, &scene);
        if i % 50 == 49 {
            let mut fresh = Renderer::new(&gpu.device, format);
            assert!(pixels == gpu.render_to_rgba(&mut fresh, &scene), "{i}");
        }
    }
}
