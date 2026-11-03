//! Custom shaders through the real renderer (see `docs/design/14-shaders.md`). Skipped when no GPU
//! adapter is available.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::shader::{ParamField, ParamType, ParamValue, ParamWriter};
use anti_webui_wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::Instant;

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

fn different_pixels(a: &[u8], b: &[u8], tolerance: i32) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| {
            a.iter()
                .zip(b.iter())
                .any(|(a, b)| (*a as i32 - *b as i32).abs() > tolerance)
        })
        .count()
}

struct Tint {
    color: Color,
}

impl ShaderParams for Tint {
    const FIELDS: &'static [ParamField] = &[ParamField {
        name: "color",
        ty: ParamType::Vec4,
    }];

    fn write(&self, out: &mut ParamWriter) {
        self.color.write(out);
    }
}

static TINT: Shader<Tint> = Shader::new(
    "tint",
    "fn paint(f: Frag) -> vec4<f32> {
    return params.color;
}
",
);

static OTHER_TINT: Shader<Tint> = Shader::new(
    "other_tint",
    "fn paint(f: Frag) -> vec4<f32> {
    return params.color;
}
",
);

fn tint(color: Color) -> ShaderPaint {
    TINT.params(Tint { color })
}

#[test]
fn a_shader_gets_the_whole_pixels_layout_decided() {
    static VALUES: Shader<()> = Shader::new(
        "values",
        "fn paint(f: Frag) -> vec4<f32> {
    let b = aw_box;
    return vec4<f32>(
        f32(b.size.x) / 255.0,
        f32(b.border.x * 10 + b.border.z) / 255.0,
        f32(b.padding.w * 10 + b.padding.y) / 255.0,
        1.0,
    );
}
",
    );
    let (w, h) = (240, 120);
    let Some(p) = render(w, h, 1.5, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            shader(|| VALUES.params(()))
                .w(100.0)
                .h(50.0)
                .border(2.0)
                .border_color(Color::TRANSPARENT)
                .border_b(1.0)
                .pl(4.0)
                .pr(2.0),
        )
    }) else {
        return;
    };
    // At 1.5: 150 wide, a border of 3 on top and 2 at the bottom, padding of 6 and 3.
    assert_eq!(px(&p, w, 75, 37), [150, 32, 63, 255]);
}

#[test]
fn every_pixel_gets_its_own_position_at_a_fractional_scale() {
    static CHECKER: Shader<()> = Shader::new(
        "checker",
        "fn paint(f: Frag) -> vec4<f32> {
    let on = (f.ipx.x + f.ipx.y) % 2 == 0 && abs(f.px.x - f32(f.ipx.x) - 0.5) < 0.001;
    return vec4<f32>(select(0.0, 1.0, on), 0.0, 0.0, 1.0);
}
",
    );
    let (w, h) = (100, 60);
    let Some(p) = render(w, h, 1.25, || {
        div().h(pct(100.0)).child(
            shader(|| CHECKER.params(()))
                .ml(5.0)
                .mt(3.0)
                .w(40.0)
                .h(20.0),
        )
    }) else {
        return;
    };
    // 5 and 3 logical pixels are 6 and 4 physical ones.
    let (ox, oy) = (6, 4);
    for y in 0..25 {
        for x in 0..50 {
            let expected = if (x + y) % 2 == 0 { 255 } else { 0 };
            assert_eq!(px(&p, w, ox + x, oy + y)[0], expected, "at {x}, {y}");
        }
    }
    // Nothing outside of the box.
    assert_eq!(px(&p, w, ox - 1, oy)[3], 255);
    assert_eq!(px(&p, w, ox + 50, oy)[0], 255);
    assert_eq!(px(&p, w, ox, oy + 25)[0], 255);
}

#[test]
fn a_shader_is_cut_like_a_background() {
    let (w, h) = (200, 140);
    let color = rgb(0x336699);
    let with = |view: Div| {
        move || {
            div().bg(Color::WHITE).h(pct(100.0)).child(
                view.m(13.0)
                    .w(120.0)
                    .h(90.0)
                    .rounded_t(30.0)
                    .rounded_b(11.0),
            )
        }
    };
    let Some(shaded) = render(w, h, 1.0, with(shader(move || tint(color)))) else {
        return;
    };
    let plain = render(w, h, 1.0, with(div().bg(color))).unwrap();
    assert_eq!(different_pixels(&shaded, &plain, 1), 0);
}

#[test]
fn it_stays_in_a_rounded_clip_around_it() {
    let (w, h) = (160, 160);
    let with = |view: fn() -> Div| {
        move || {
            div().bg(Color::WHITE).h(pct(100.0)).p(20.0).child(
                div()
                    .w(100.0)
                    .h(100.0)
                    .rounded(40.0)
                    .overflow_hidden()
                    .child(view()),
            )
        }
    };
    let Some(shaded) = render(
        w,
        h,
        1.0,
        with(|| shader(|| tint(rgb(0xcc3344))).w(100.0).h(100.0)),
    ) else {
        return;
    };
    let plain = render(
        w,
        h,
        1.0,
        with(|| div().bg(rgb(0xcc3344)).w(100.0).h(100.0)),
    )
    .unwrap();
    assert_eq!(different_pixels(&shaded, &plain, 1), 0);
}

#[test]
fn fill_goes_below_the_box_and_overlay_above_it_and_below_the_children() {
    let (w, h) = (120, 120);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            div()
                .w(100.0)
                .h(100.0)
                .border(10.0)
                .border_color(rgba(0x0000ff80))
                .bg(rgba(0x00ff0080))
                .fill(|| tint(rgb(0xff0000)))
                .overlay(|| tint(rgba(0x00000080)))
                .child(div().w(20.0).h(20.0).bg(rgb(0xffffff))),
        )
    }) else {
        return;
    };
    let close = |a: [u8; 4], b: [u8; 3]| {
        for i in 0..3 {
            assert!(
                (a[i] as i32 - b[i] as i32).abs() <= 3,
                "{a:?} is not close to {b:?}"
            );
        }
    };
    // Green over the fill's red, darkened by half.
    close(px(&p, w, 60, 60), [64, 64, 0]);
    // The child isn't touched by the overlay.
    close(px(&p, w, 15, 15), [255, 255, 255]);
    // The border is over the background and below the overlay: blue over the green over red.
    let border = px(&p, w, 5, 60);
    close(border, [32, 32, 64]);
}

#[test]
fn nothing_is_painted_while_there_is_no_paint() {
    let (w, h) = (60, 60);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            div()
                .w(40.0)
                .h(40.0)
                .overlay(|| None::<ShaderPaint>)
                .fill(|| None::<ShaderPaint>),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 20, 20), [255, 255, 255, 255]);
}

#[test]
fn opacity_applies_to_what_it_paints() {
    let (w, h) = (80, 80);
    let color = rgb(0x224466);
    // On its own, nothing overlaps; with a box on top it goes through a layer. Either way the
    // result is the same as fading a box of that color.
    let reference = render(w, h, 1.0, move || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(div().w(50.0).h(50.0).rounded(9.0).bg(color).opacity(0.5))
    });
    let Some(reference) = reference else {
        return;
    };
    let alone = render(w, h, 1.0, move || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            shader(move || tint(color))
                .w(50.0)
                .h(50.0)
                .rounded(9.0)
                .opacity(0.5),
        )
    })
    .unwrap();
    assert_eq!(different_pixels(&alone, &reference, 1), 0);
    let layered = render(w, h, 1.0, move || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            shader(move || tint(color))
                // Two coats would cover the edges of the corners more, so the second is clear.
                .overlay(|| tint(Color::TRANSPARENT))
                .w(50.0)
                .h(50.0)
                .rounded(9.0)
                .opacity(0.5),
        )
    })
    .unwrap();
    assert_eq!(different_pixels(&layered, &reference, 2), 0);
}

#[test]
fn it_keeps_its_units_in_a_layer_drawn_bigger() {
    static MEASURES: Shader<()> = Shader::new(
        "measures",
        "fn paint(f: Frag) -> vec4<f32> {
    // Red tells how far along the box the pixel is, green how big a pixel of the output is.
    return vec4<f32>(f.px.x / f32(aw_box.size.x), f.pixel, 0.0, 1.0);
}
",
    );
    let (w, h) = (200, 200);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .h(pct(100.0))
            .pl(50.0)
            .pt(50.0)
            .child(shader(|| MEASURES.params(())).w(100.0).h(100.0).scale(2.0))
    }) else {
        return;
    };
    // Scaled around its center it covers from 0 to 200. A quarter of the way along the box:
    let at = px(&p, w, 50, 100);
    assert!((at[0] as i32 - 64).abs() <= 2, "{at:?}");
    assert!((at[1] as i32 - 128).abs() <= 2, "{at:?}");
}

#[test]
fn a_shader_that_does_not_compile_paints_nothing_and_breaks_nothing() {
    static BROKEN: Shader<()> = Shader::new("broken", "fn paint(f: Frag) -> vec4<f32> { nope }");
    let (w, h) = (100, 50);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .flex()
            .flex_row()
            .child(shader(|| BROKEN.params(())).w(50.0).h(50.0))
            .child(shader(|| tint(rgb(0x00ff00))).w(50.0).h(50.0))
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 25, 25), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 75, 25), [0, 255, 0, 255]);
}

#[test]
fn compiling_ahead_is_enough() {
    static AHEAD: Shader<()> = Shader::new(
        "ahead",
        "fn paint(f: Frag) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 1.0, 1.0); }",
    );
    AHEAD.preload();
    let (w, h) = (20, 20);
    let Some(p) = render(w, h, 1.0, || shader(|| AHEAD.params(())).w(20.0).h(20.0)) else {
        return;
    };
    assert_eq!(px(&p, w, 10, 10), [0, 0, 255, 255]);
}

#[test]
fn many_boxes_get_their_own_parameters_past_a_chunk() {
    // 16 bytes each, so that 1024 fit in a chunk, and the order on the screen is kept by turning
    // to another shader on some of them.
    let (w, h) = (640, 640);
    let color = |i: u32| rgb(i * 0x0301 % 0xffff + 0x10000 * (i % 200));
    let with = |make: fn(u32, Color) -> Div| {
        move || {
            div()
                .bg(Color::WHITE)
                .h(pct(100.0))
                .children((0..2500).map(move |i| {
                    make(i, color(i))
                        .absolute()
                        .left((i % 50 * 12) as f32)
                        .top((i / 50 * 12) as f32)
                        .w(14.0)
                        .h(14.0)
                }))
        }
    };
    let Some(shaded) = render(
        w,
        h,
        1.0,
        with(|i, c| {
            if i % 701 == 0 {
                shader(move || OTHER_TINT.params(Tint { color: c }))
            } else {
                shader(move || tint(c))
            }
        }),
    ) else {
        return;
    };
    let plain = render(w, h, 1.0, with(|_, c| div().bg(c))).unwrap();
    assert_eq!(different_pixels(&shaded, &plain, 1), 0);
}

/// Goes through changes of what shaders paint, comparing drawing only what changed to drawing all.
#[test]
fn it_repaints_the_same_in_parts_as_at_once() {
    let gpu = match Gpu::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (300u32, 300u32);
    let a = signal(rgb(0x204060));
    let shown = signal(false);
    let moved = signal(0.0f32);
    let faded = signal(0.5f32);
    let mut ui = Ui::new(move || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(
                shader(move || tint(a.get()))
                    .w(80.0)
                    .h(40.0)
                    .rounded(12.0)
                    .shadow_md()
                    .overlay(move || shown.get().then(|| tint(rgba(0xffffff80))))
                    .child(text("on top")),
            )
            // In a layer that scrolls.
            .child(
                div().h(80.0).overflow_y_scroll().child(
                    div().h(300.0).child(
                        shader(move || tint(a.get()))
                            .w(50.0)
                            .h(50.0)
                            .mt(10.0)
                            .rounded(25.0),
                    ),
                ),
            )
            // In a layer that moves.
            .child(
                div()
                    .h(60.0)
                    .dyn_style(move |s| s.translate_x(moved.get()))
                    .child(shader(move || tint(a.get())).w(40.0).h(40.0)),
            )
            // In a layer that fades.
            .child(
                div()
                    .h(60.0)
                    .bg(rgb(0xdddddd))
                    .dyn_style(move |s| s.opacity(faded.get()))
                    .child(shader(move || tint(a.get())).w(40.0).h(40.0)),
            )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w as i32, h as i32),
        scale: Scale::ONE,
    });
    ui.update();

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut partial_renderer = Renderer::new(&gpu.device, format);
    let mut full_renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(&gpu, format, w, h);
    let now = Instant::now();
    let mut prev = ui.paint(now);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    partial_renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);

    let mut check = |ui: &mut Ui, what: &str| {
        ui.update();
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
        assert_eq!(different_pixels(&partial, &full, 0), 0, "{what}");
        prev = scene;
        d
    };

    a.set(rgb(0x806020));
    check(&mut ui, "the color of all of them");
    shown.set(true);
    let d = check(&mut ui, "an overlay coming");
    assert_eq!(d.rects(), &[RectI::new(0, 0, 80, 40)], "only its box");
    shown.set(false);
    let d = check(&mut ui, "an overlay going");
    assert_eq!(d.rects(), &[RectI::new(0, 0, 80, 40)], "only its box");
    moved.set(7.0);
    check(&mut ui, "a layer moved");
    a.set(rgb(0x208060));
    check(&mut ui, "the color while moved");
    faded.set(0.8);
    check(&mut ui, "a layer faded");
    a.set(rgb(0x602080));
    check(&mut ui, "the color while faded");
}

/// A shader that takes its color from a position in its parameters, to see that those of each box
/// reach it in the order drawn, when they overlap and when other things come between them.
#[test]
fn what_overlaps_stays_in_order() {
    let (w, h) = (100, 100);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(
                shader(|| tint(rgb(0xff0000)))
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .w(60.0)
                    .h(60.0),
            )
            .child(
                div()
                    .absolute()
                    .left(10.0)
                    .top(10.0)
                    .w(60.0)
                    .h(60.0)
                    .bg(rgb(0x00ff00)),
            )
            .child(
                shader(|| {
                    OTHER_TINT.params(Tint {
                        color: rgb(0x0000ff),
                    })
                })
                .absolute()
                .left(20.0)
                .top(20.0)
                .w(60.0)
                .h(60.0),
            )
            .child(
                shader(|| tint(rgb(0xffff00)))
                    .absolute()
                    .left(30.0)
                    .top(30.0)
                    .w(60.0)
                    .h(60.0),
            )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 5, 5), [255, 0, 0, 255]);
    assert_eq!(px(&p, w, 15, 15), [0, 255, 0, 255]);
    assert_eq!(px(&p, w, 25, 25), [0, 0, 255, 255]);
    assert_eq!(px(&p, w, 35, 35), [255, 255, 0, 255]);
}
