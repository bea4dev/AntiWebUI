//! Blurs, filters and backdrops through the real renderer (see `docs/design/16-filters.md`).
//! Skipped when no GPU adapter is available.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
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

/// The cumulative distribution of a normal distribution.
fn phi(x: f64) -> f64 {
    // Abramowitz and Stegun 7.1.26.
    let z = x.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * z);
    let y = t
        * (0.254829592
            + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    let erf = 1.0 - y * (-z * z).exp();
    if x >= 0.0 {
        0.5 + erf / 2.0
    } else {
        0.5 - erf / 2.0
    }
}

/// An edge between black and white, blurred: the values across it on a row.
fn blurred_edge(sigma: f32) -> Option<Vec<f64>> {
    let (w, h) = (800, 60);
    let p = render(w, h, 1.0, move || {
        div()
            .size_full()
            .bg(Color::WHITE)
            .child(
                div()
                    .absolute()
                    .left(-200.0)
                    .top(-200.0)
                    .w(600.0)
                    .h(460.0)
                    .bg(Color::BLACK),
            )
            .blur(sigma)
    })?;
    Some(
        (0..w)
            .map(|x| px(&p, w, x, h / 2)[0] as f64 / 255.0)
            .collect(),
    )
}

#[test]
fn a_blur_is_about_as_wide_as_a_css_one_of_that_radius() {
    for sigma in [
        0.5_f32, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 12.0, 20.0, 30.0, 50.0,
    ] {
        let Some(row) = blurred_edge(sigma) else {
            return;
        };
        // Where the edge goes from 16% to 84%, which takes two standard deviations.
        let cross = |v: f64| row.iter().position(|p| *p >= v).unwrap() as f64;
        let width = (cross(0.8413) - cross(0.1587)) / 2.0;
        let middle = cross(0.5);
        eprintln!("sigma {sigma}: width {width:.2}, middle {middle}");
        let tolerance = (sigma as f64 * 0.25).max(0.75);
        assert!(
            (width - sigma as f64).abs() <= tolerance,
            "sigma {sigma}: width {width}"
        );
        assert!(
            (middle - 400.0).abs() <= tolerance + 1.0,
            "sigma {sigma}: {middle}"
        );
        // Against the real thing, at any pixel.
        let error = row
            .iter()
            .enumerate()
            .map(|(x, v)| (v - phi((x as f64 + 0.5 - 400.0) / sigma as f64)).abs())
            .fold(0.0, f64::max);
        eprintln!("  worst difference from a gaussian: {error:.3}");
        assert!(error < 0.15, "sigma {sigma}: {error}");
    }
}

#[test]
fn a_radius_that_changes_a_little_changes_the_picture_a_little() {
    let mut last: Option<Vec<f64>> = None;
    let mut sigma = 0.25_f32;
    while sigma < 40.0 {
        let Some(row) = blurred_edge(sigma) else {
            return;
        };
        if let Some(last) = &last {
            let jump = row
                .iter()
                .zip(last)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max);
            assert!(jump < 0.09, "at sigma {sigma}: {jump}");
        }
        last = Some(row);
        sigma *= 1.08;
    }
}

#[test]
fn a_gentle_edge_stays_smooth_through_all_the_steps() {
    let (w, h) = (800, 60);
    for sigma in [4.0_f32, 15.0, 40.0] {
        // Bigger than the window, or its edges would fade too.
        let Some(p) = render(w, h, 1.0, move || {
            div().size_full().child(
                div()
                    .absolute()
                    .left(-400.0)
                    .top(-400.0)
                    .w(1600.0)
                    .h(860.0)
                    .bg(rgb(0x303030))
                    .child(
                        div()
                            .absolute()
                            .left(0.0)
                            .top(0.0)
                            .w(800.0)
                            .h(860.0)
                            .bg(rgb(0x202020)),
                    )
                    .blur(sigma),
            )
        }) else {
            return;
        };
        let row: Vec<i32> = (0..w).map(|x| px(&p, w, x, h / 2)[0] as i32).collect();
        // It goes up by one at a time, without going back down.
        for pair in row.windows(2) {
            assert!(
                pair[1] - pair[0] >= -1 && pair[1] - pair[0] <= 2,
                "sigma {sigma}: {pair:?}"
            );
        }
        let wrong_way: i32 = row.windows(2).map(|p| (p[0] - p[1]).max(0)).sum();
        eprintln!("sigma {sigma}: goes back by {wrong_way}");
        assert!(wrong_way <= 8, "sigma {sigma}: {wrong_way}");
        assert_eq!(row[0], 0x20);
        assert_eq!(row[row.len() - 1], 0x30);
    }
}

#[test]
fn a_blur_reaches_past_the_box_and_is_seen_there() {
    let (w, h) = (200, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(
            div()
                .absolute()
                .left(80.0)
                .top(80.0)
                .w(40.0)
                .h(40.0)
                .bg(Color::BLACK)
                .blur(8.0),
        )
    }) else {
        return;
    };
    let darkness = |x, y| 255 - px(&p, w, x, y)[0] as i32;
    assert!(darkness(100, 100) > 200, "{}", darkness(100, 100));
    // Half a standard deviation past the edge, and well past it.
    assert!(darkness(124, 100) > 60, "{}", darkness(124, 100));
    assert!(darkness(160, 100) == 0, "{}", darkness(160, 100));
    // Symmetrical.
    assert!((darkness(90, 100) - darkness(109, 100)).abs() <= 4);
    assert!((darkness(100, 72) - darkness(100, 127)).abs() <= 4);
}

#[test]
fn a_blur_keeps_what_is_transparent_transparent() {
    let (w, h) = (200, 100);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::from_rgb(0xff0000)).child(
            div()
                .w(100.0)
                .h(100.0)
                .bg(Color::from_rgb(0x00ff00))
                .blur(4.0),
        )
    }) else {
        return;
    };
    // Across the fading edge, only red and green mix: no darker fringe from black.
    for x in 80..130 {
        let c = px(&p, w, x, 50);
        assert!(c[0] as i32 + c[1] as i32 >= 250, "{x}: {c:?}");
        assert_eq!(c[3], 255);
    }
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

/// Keeps the shape of the input in one color.
static TINT: Shader<Tint> = Shader::new(
    "tint",
    "fn paint(f: Frag) -> vec4<f32> {
    return params.color * read0(f.px).a;
}
",
);

/// The second input over the first.
static OVER: Shader<()> = Shader::new(
    "over",
    "fn paint(f: Frag) -> vec4<f32> {
    let top = read1(f.px);
    return top + read0(f.px) * (1.0 - top.a);
}
",
);

/// Reads from further to the left.
static SHIFT: Shader<()> = Shader::new(
    "shift",
    "fn paint(f: Frag) -> vec4<f32> {
    return read0(f.px - vec2<f32>(10.0 * aw_box.scale, 0.0));
}
",
);

/// Shows the box's own coordinates alongside what it reads.
static WHERE: Shader<()> = Shader::new(
    "where",
    "fn paint(f: Frag) -> vec4<f32> {
    let c = read0(f.px);
    return vec4<f32>(c.r, f.px.x / 255.0, f.pixel, 1.0);
}
",
);

static INVERT: Shader<()> = Shader::new(
    "invert",
    "fn paint(f: Frag) -> vec4<f32> {
    let c = read0(f.px);
    return vec4<f32>(1.0 - c.rgb, 1.0);
}
",
);

static BROKEN: Shader<()> = Shader::new(
    "broken_filter",
    "fn paint(f: Frag) -> vec4<f32> { return nothing_like_this; }
",
);

static PASS: Shader<()> = Shader::new(
    "pass",
    "fn paint(f: Frag) -> vec4<f32> {
    return read0(f.px);
}
",
);

#[test]
fn a_shader_in_the_middle_reads_as_far_as_it_says() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .fill(|| {
                    Image::backdrop()
                        .through(&SHIFT, ())
                        .reach(10.0)
                        .through(&PASS, ())
                }),
        )
    }) else {
        return;
    };
    // 10 to the left of the box: the white stripe outside of it, not the edge of the box.
    assert_eq!(px(&p, w, 105, 100)[0], 255);
    assert_eq!(px(&p, w, 115, 100)[0], 0);
}

#[test]
fn what_a_texture_held_before_does_not_show_around_a_blur() {
    let (w, h) = (400, 200);
    let card = |left: f32, color: u32| {
        div()
            .absolute()
            .left(left)
            .top(50.0)
            .w(40.0)
            .h(40.0)
            .bg(rgb(color))
            .blur(6.0)
    };
    // The first one leaves its picture in a texture that the second one uses next.
    let (Some(alone), Some(after)) = (
        render(w, h, 1.0, move || {
            div()
                .size_full()
                .bg(Color::WHITE)
                .child(card(300.0, 0xff0000))
        }),
        render(w, h, 1.0, move || {
            div()
                .size_full()
                .bg(Color::WHITE)
                .child(card(30.0, 0x000000))
                .child(card(300.0, 0xff0000))
        }),
    ) else {
        return;
    };
    for x in 200..400 {
        for y in 0..h {
            let (a, b) = (px(&alone, w, x, y), px(&after, w, x, y));
            let apart = a.iter().zip(&b).any(|(a, b)| a.abs_diff(*b) > 1);
            assert!(!apart, "{x},{y}: {a:?} {b:?}");
        }
    }
}

#[test]
fn many_backdrops_share_their_textures() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let textures = |count: usize| {
        let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
        let mut ui = Ui::new(move || {
            div()
                .size_full()
                .bg(Color::WHITE)
                .children((0..count).map(|i| {
                    div()
                        .absolute()
                        .left(10.0 + i as f32 * 30.0)
                        .top(50.0)
                        .w(25.0)
                        .h(25.0)
                        .backdrop_blur(3.0)
                }))
        });
        sized(&mut ui, 400, 200);
        let scene = ui.paint(Instant::now());
        gpu.render_to_rgba(&mut renderer, &scene);
        renderer.effect_counts().1
    };
    assert_eq!(textures(10), textures(1));
}

#[test]
fn a_filter_gets_what_the_node_and_its_children_paint() {
    let (w, h) = (100, 100);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(
            div()
                .w(60.0)
                .h(60.0)
                .bg(Color::from_rgb(0xff0000))
                .child(div().w(20.0).h(20.0).bg(Color::from_rgb(0x00ff00)))
                .filter(|| Image::content().through(&INVERT, ())),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 40, 40), [0, 255, 255, 255]);
    assert_eq!(px(&p, w, 5, 5), [255, 0, 255, 255]);
    assert_eq!(px(&p, w, 80, 80), [255, 255, 255, 255]);
}

#[test]
fn reach_says_how_far_it_reads_and_past_that_is_the_edge() {
    let (w, h) = (100, 40);
    // Reads 10 to the left, with enough reach.
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(
            div()
                .absolute()
                .left(30.0)
                .top(0.0)
                .w(20.0)
                .h(40.0)
                .bg(Color::BLACK)
                .filter(|| Image::content().through(&SHIFT, ()).reach(10.0)),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 35, 20)[0], 255, "moved away");
    assert_eq!(px(&p, w, 45, 20)[0], 0);
    assert_eq!(px(&p, w, 55, 20)[0], 0, "past the box, within reach");
    assert_eq!(px(&p, w, 65, 20)[0], 255);
}

#[test]
fn the_shadow_of_a_shape_comes_from_a_filter() {
    let (w, h) = (200, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(
            div()
                .absolute()
                .left(60.0)
                .top(60.0)
                .w(80.0)
                .h(80.0)
                .bg(Color::from_rgb(0x00ff00))
                .filter(|| {
                    let me = Image::content();
                    let shadow = me.clone().blur(6.0).through(
                        &TINT,
                        Tint {
                            color: Color::from_rgb(0x000000),
                        },
                    );
                    OVER.params(()).inputs([shadow, me]).reach(18.0)
                }),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 100, 100), [0, 255, 0, 255], "it stays on top");
    let shadow = px(&p, w, 145, 100);
    assert!(shadow[0] < 230 && shadow[0] == shadow[2], "{shadow:?}");
    assert_eq!(px(&p, w, 190, 100), [255, 255, 255, 255]);
}

#[test]
fn a_shader_that_does_not_compile_shows_nothing_and_breaks_nothing() {
    let (w, h) = (100, 100);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .size_full()
            .bg(Color::WHITE)
            .child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .bg(Color::BLACK)
                    .filter(|| Image::content().through(&BROKEN, ())),
            )
            .child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .fill(|| Image::backdrop().through(&BROKEN, ())),
            )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 25, 25), [255, 255, 255, 255]);
}

#[test]
fn a_filter_keeps_its_units_in_a_layer_drawn_bigger() {
    let (w, h) = (300, 300);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(
            div()
                .absolute()
                .left(100.0)
                .top(100.0)
                .w(100.0)
                .h(100.0)
                .scale(2.0)
                .bg(Color::from_rgb(0xff0000))
                .filter(|| Image::content().through(&WHERE, ())),
        )
    }) else {
        return;
    };
    // The middle of the box is at 50 in its own pixels, however big it is drawn.
    let c = px(&p, w, 150, 150);
    assert_eq!(c[0], 255);
    assert!((c[1] as i32 - 50).abs() <= 1, "{c:?}");
    assert!((c[2] as f32 / 255.0 - 0.5).abs() < 0.01, "{c:?}");
}

fn stripes() -> Div {
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(400.0)
        .h(200.0)
        .flex()
        .flex_row()
        .children((0..40).map(|i| {
            div().w(10.0).h(200.0).bg(if i % 2 == 0 {
                Color::BLACK
            } else {
                Color::WHITE
            })
        }))
}

#[test]
fn a_backdrop_blur_blurs_what_is_behind_inside_the_box_only() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .backdrop_blur(10.0),
        )
    }) else {
        return;
    };
    // Outside, the stripes are as they are.
    assert_eq!(px(&p, w, 105, 25)[0], 0);
    assert_eq!(px(&p, w, 115, 25)[0], 255);
    // Inside, they are all grey. A sharp edge at the box.
    for x in 120..280 {
        let c = px(&p, w, x, 100)[0] as i32;
        assert!((c - 128).abs() < 10, "{x}: {c}");
    }
    assert_eq!(px(&p, w, 150, 49)[0], 255 * ((150 / 10) % 2) as u8);
    assert!((px(&p, w, 150, 50)[0] as i32 - 128).abs() < 30);
}

#[test]
fn what_is_on_top_of_a_backdrop_and_its_own_background_stay_sharp() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .bg(Color::from_rgba(0xff000080))
                .backdrop_blur(10.0)
                .child(
                    div()
                        .absolute()
                        .left(20.0)
                        .top(20.0)
                        .w(20.0)
                        .h(20.0)
                        .bg(Color::from_rgb(0x00ff00)),
                ),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 130, 80), [0, 255, 0, 255]);
    let c = px(&p, w, 200, 120);
    // Half red over grey.
    assert!((c[0] as i32 - 192).abs() < 12, "{c:?}");
    assert!((c[1] as i32 - 64).abs() < 12, "{c:?}");
}

#[test]
fn a_backdrop_has_the_shape_of_the_box() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .rounded(30.0)
                .backdrop_blur(10.0),
        )
    }) else {
        return;
    };
    // The corner shows the stripes as they are.
    assert_eq!(px(&p, w, 101, 51)[0], 0);
    assert_eq!(px(&p, w, 111, 51)[0], 255);
    assert!((px(&p, w, 200, 100)[0] as i32 - 128).abs() < 10);
}

#[test]
fn a_backdrop_goes_with_the_opacity_and_transform_of_its_box() {
    let (w, h) = (400, 200);
    let Some(faded) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .bg(Color::from_rgba(0xff000080))
                .opacity(0.5)
                .backdrop_blur(10.0),
        )
    }) else {
        return;
    };
    // In between the stripes as they are and grey, on black and on white.
    let dark = px(&faded, w, 205, 100);
    let light = px(&faded, w, 215, 100);
    assert!(
        dark[1] < light[1] && light[1] - dark[1] > 30,
        "{dark:?} {light:?}"
    );
    assert!(dark[1] > 20, "{dark:?}");

    let Some(scaled) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(150.0)
                .top(75.0)
                .w(100.0)
                .h(50.0)
                .scale(2.0)
                .backdrop_blur(10.0),
        )
    }) else {
        return;
    };
    // It covers what its box covers now, from what is behind it there.
    for x in [110, 150, 290] {
        assert!((px(&scaled, w, x, 100)[0] as i32 - 128).abs() < 40, "{x}");
    }
    assert_eq!(px(&scaled, w, 85, 100)[0], 0);

    let Some(rotated) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(150.0)
                .top(50.0)
                .w(100.0)
                .h(100.0)
                .rotate(45.0)
                .backdrop_blur(10.0),
        )
    }) else {
        return;
    };
    assert!((px(&rotated, w, 200, 100)[0] as i32 - 128).abs() < 20);
    // The corner of the box was here, and it turned away.
    assert_eq!(px(&rotated, w, 151, 51)[0], 255);
}

#[test]
fn a_filter_turns_with_its_box() {
    let (w, h) = (300, 300);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(
            div()
                .absolute()
                .left(100.0)
                .top(100.0)
                .w(100.0)
                .h(100.0)
                .bg(Color::BLACK)
                .rotate(45.0)
                .blur(2.0),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 150, 150)[0], 0);
    // It reaches further along the axes now, and not as far toward where the corners were.
    assert!(px(&p, w, 150, 90)[0] < 16, "{:?}", px(&p, w, 150, 90));
    assert_eq!(px(&p, w, 105, 105)[0], 255);
    // Soft across the edge, the way it was before it turned.
    let soft = (0..100).filter(|i| {
        let v = px(&p, w, 150 + i, 150 - i)[0];
        v > 10 && v < 245
    });
    assert!(soft.count() > 2);
}

#[test]
fn a_panel_that_moves_shows_the_same_where_it_still_is() {
    let (w, h) = (400, 200);
    let at = |left: f32| {
        render(w, h, 1.0, move || {
            div().size_full().bg(Color::WHITE).child(stripes()).child(
                div()
                    .absolute()
                    .left(left)
                    .top(50.0)
                    .w(200.0)
                    .h(100.0)
                    .backdrop_blur(3.0),
            )
        })
    };
    let (Some(a), Some(b)) = (at(100.0), at(103.0)) else {
        return;
    };
    // Past the edge of what either reads differently, the blur stays put on the stripes.
    let different = (140..260)
        .flat_map(|x| (60..140).map(move |y| (x, y)))
        .filter(|&(x, y)| (px(&a, w, x, y)[0] as i32 - px(&b, w, x, y)[0] as i32).abs() > 1)
        .count();
    assert_eq!(different, 0);
}

#[test]
fn a_blur_in_a_layer_drawn_bigger_is_as_big_as_the_layer_shows_it() {
    let (w, h) = (400, 100);
    let edge = |scale: f32, sigma: f32| {
        render(w, h, 1.0, move || {
            div().size_full().bg(Color::WHITE).child(
                div()
                    .absolute()
                    .left(150.0)
                    .top(-300.0)
                    .w(100.0)
                    .h(700.0)
                    .bg(Color::BLACK)
                    .blur(sigma)
                    .scale(scale),
            )
        })
    };
    let (Some(direct), Some(scaled)) = (edge(1.0, 8.0), edge(2.0, 4.0)) else {
        return;
    };
    // Twice as big around the middle, so the left edge is further left. Blurred the same on screen.
    for offset in [-12_i32, -6, 0, 6, 12] {
        let d = px(&direct, w, (150 + offset) as u32, 50)[0] as i32;
        let s = px(&scaled, w, (100 + offset) as u32, 50)[0] as i32;
        assert!((d - s).abs() <= 24, "{offset}: {d} {s}");
    }
}

#[test]
fn a_shader_reads_what_is_behind() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .fill(|| Image::backdrop().through(&INVERT, ())),
        )
    }) else {
        return;
    };
    assert_eq!(px(&p, w, 105, 100)[0], 255, "black turned white");
    assert_eq!(px(&p, w, 115, 100)[0], 0, "white turned black");
    assert_eq!(px(&p, w, 115, 25)[0], 255, "not outside of the box");
}

#[test]
fn two_inputs_from_one_picture_and_a_fill_without_a_shader() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .size_full()
            .bg(Color::WHITE)
            .child(stripes())
            .child(
                div()
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .w(100.0)
                    .h(200.0)
                    .fill(|| Image::backdrop().blur(10.0)),
            )
            .child(
                div()
                    .absolute()
                    .left(200.0)
                    .top(0.0)
                    .w(100.0)
                    .h(200.0)
                    .fill(|| {
                        let back = Image::backdrop();
                        OVER.params(()).inputs([back.clone(), back])
                    }),
            )
    }) else {
        return;
    };
    assert!((px(&p, w, 50, 100)[0] as i32 - 128).abs() < 10);
    assert_eq!(px(&p, w, 205, 100)[0], 0);
    assert_eq!(px(&p, w, 215, 100)[0], 255);
}

#[test]
fn inside_a_layer_a_backdrop_sees_the_layer() {
    let (w, h) = (400, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .opacity(0.99)
                .bg(Color::from_rgb(0xff0000))
                .child(div().w(50.0).h(100.0).bg(Color::from_rgb(0x00ff00)))
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(200.0)
                        .h(100.0)
                        .backdrop_blur(4.0),
                ),
        )
    }) else {
        return;
    };
    // Far from where the green and red meet, each one is only itself: no stripes in there.
    let c = px(&p, w, 260, 100);
    assert!(c[0] > 240 && c[1] < 10, "{c:?}");
    let c = px(&p, w, 110, 100);
    assert!(c[1] > 240 && c[0] < 10, "{c:?}");
    // Where they meet, they mix.
    let c = px(&p, w, 150, 100);
    assert!(c[0] > 60 && c[1] > 60, "{c:?}");
}

#[test]
fn what_is_transparent_stays_so_on_a_canvas() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let mut ui = Ui::new(|| {
        div()
            .w(50.0)
            .h(50.0)
            .fill(|| Image::backdrop().through(&SHIFT, ()))
    });
    sized(&mut ui, 100, 100);
    let mut scene = ui.paint(Instant::now());
    scene.clear = Color::TRANSPARENT;
    let p = gpu.render_to_rgba(&mut renderer, &scene);
    assert_eq!(px(&p, 100, 80, 80), [0, 0, 0, 0]);
    assert_eq!(renderer.effect_counts().0, 1);
}

fn sized(ui: &mut Ui, w: i32, h: i32) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w, h),
        scale: Scale::ONE,
    });
    ui.update();
}

#[test]
fn nothing_is_kept_for_a_scene_that_makes_no_pictures() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let on = signal(true);
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .bg(Color::WHITE)
            .dyn_style(move |s| if on.get() { s.backdrop_blur(4.0) } else { s })
    });
    sized(&mut ui, 100, 100);
    let scene = ui.paint(Instant::now());
    gpu.render_to_rgba(&mut renderer, &scene);
    let (canvases, textures, _) = renderer.effect_counts();
    assert_eq!(canvases, 1);
    assert!(textures > 0);

    on.set(false);
    ui.update();
    for _ in 0..12 {
        let scene = ui.paint(Instant::now());
        gpu.render_to_rgba(&mut renderer, &scene);
    }
    assert_eq!(renderer.effect_counts().0, 0, "no canvas");
    assert_eq!(renderer.effect_counts().1, 0, "no textures left");
}

#[test]
fn a_filter_is_not_run_again_for_what_goes_on_around_it() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let color = signal(rgb(0x0000ff));
    let radius = signal(3.0_f32);
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .bg(Color::WHITE)
            .child(div().w(50.0).h(50.0).dyn_style(move |s| s.bg(color.get())))
            .child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .bg(Color::BLACK)
                    .filter(move || Image::content().blur(radius.get())),
            )
    });
    sized(&mut ui, 200, 200);
    let target = Offscreen::new(&gpu, format, 200, 200);
    let mut prev = ui.paint(Instant::now());
    let draw = |renderer: &mut Renderer,
                scene: &anti_webui_core::scene::Scene,
                damage: Option<&damage::Damage>| {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        match damage {
            Some(d) => renderer.render_damaged(
                &gpu.device,
                &gpu.queue,
                &mut encoder,
                &target.view,
                scene,
                d,
            ),
            None => renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, scene),
        }
        gpu.queue.submit([encoder.finish()]);
    };
    draw(&mut renderer, &prev, None);
    let runs = renderer.effect_counts().2;
    assert_eq!(runs, 1);

    color.set(rgb(0xff0000));
    ui.update();
    let next = ui.paint(Instant::now());
    let d = damage::diff(&prev, &next);
    draw(&mut renderer, &next, Some(&d));
    assert_eq!(renderer.effect_counts().2, runs, "something else changed");
    prev = next;

    radius.set(5.0);
    ui.update();
    let next = ui.paint(Instant::now());
    let d = damage::diff(&prev, &next);
    assert!(!d.is_empty());
    draw(&mut renderer, &next, Some(&d));
    assert_eq!(renderer.effect_counts().2, runs + 1, "the filter changed");
}

/// Goes through states in one go and in parts, each time comparing all the pixels.
fn repaints_the_same_in_parts_as_at_once(
    w: u32,
    h: u32,
    view: impl FnOnce() -> Div + 'static,
    steps: &[(&str, &dyn Fn())],
) {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut ui = Ui::new(view);
    sized(&mut ui, w as i32, h as i32);
    let mut prev = ui.paint(Instant::now());
    let target = Offscreen::new(&gpu, format, w, h);
    let mut partial = Renderer::new(&gpu.device, format);
    let mut whole = Renderer::new(&gpu.device, format);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    partial.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    for (what, step) in steps {
        step();
        ui.update();
        let next = ui.paint(Instant::now());
        let d = damage::diff(&prev, &next);
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
        let wrong = different_pixels(&got, &want, 0);
        assert_eq!(wrong, 0, "{what}: damage {:?}", d.rects());
        prev = next;
    }
}

#[test]
fn the_renderer_takes_all_of_what_is_read_whatever_it_is_given() {
    let Ok(gpu) = Gpu::headless() else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut ui = Ui::new(|| {
        div().size_full().bg(Color::WHITE).child(stripes()).child(
            div()
                .absolute()
                .left(100.0)
                .top(50.0)
                .w(200.0)
                .h(100.0)
                .backdrop_blur(4.0),
        )
    });
    sized(&mut ui, 400, 200);
    let scene = ui.paint(Instant::now());
    let target = Offscreen::new(&gpu, format, 400, 200);
    let mut renderer = Renderer::new(&gpu.device, format);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &scene);
    gpu.queue.submit([encoder.finish()]);
    let before = target.read_rgba(&gpu);

    // A small part of the box: what is around it holds the blur already, and must not be read.
    let mut small = damage::Damage::new();
    small.add(anti_webui_core::geometry::RectI::new(190, 90, 10, 10));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render_damaged(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &target.view,
        &scene,
        &small,
    );
    gpu.queue.submit([encoder.finish()]);
    assert_eq!(different_pixels(&before, &target.read_rgba(&gpu), 0), 0);
}

#[test]
fn a_backdrop_repaints_the_same_in_parts_as_at_once() {
    let x = signal(10.0_f32);
    let panel = signal(100.0_f32);
    let color = signal(rgb(0x3366cc));
    let far = signal(rgb(0x808080));
    let on_top = signal(rgb(0xffcc00));
    repaints_the_same_in_parts_as_at_once(
        400,
        300,
        move || {
            div()
                .size_full()
                .bg(Color::WHITE)
                // Far from the panel, but within what it reads.
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(120.0)
                        .w(50.0)
                        .h(60.0)
                        .dyn_style(move |s| s.bg(far.get())),
                )
                .child(
                    div()
                        .absolute()
                        .top(40.0)
                        .w(60.0)
                        .h(200.0)
                        .dyn_style(move |s| s.left(x.get()).bg(color.get())),
                )
                .child(
                    div()
                        .absolute()
                        .top(90.0)
                        .w(150.0)
                        .h(100.0)
                        .rounded(12.0)
                        .bg(Color::from_rgba(0xffffff40))
                        .backdrop_blur(6.0)
                        .dyn_style(move |s| s.left(panel.get())),
                )
                .child(
                    div()
                        .absolute()
                        .left(150.0)
                        .top(120.0)
                        .w(30.0)
                        .h(30.0)
                        .dyn_style(move |s| s.bg(on_top.get())),
                )
        },
        &[
            ("nothing", &|| {}),
            ("something moves behind", &move || x.set(40.0)),
            ("under the panel", &move || x.set(120.0)),
            ("changes color behind", &move || color.set(rgb(0xcc3366))),
            ("past the box, what it reads", &move || {
                far.set(rgb(0x000000))
            }),
            ("on top of the panel", &move || on_top.set(rgb(0x00ccff))),
            ("the panel moves", &move || panel.set(130.0)),
            ("out of what it reads", &move || x.set(320.0)),
            ("the panel moves back", &move || panel.set(100.0)),
        ],
    );
}

#[test]
fn backdrops_over_each_other_repaint_the_same_in_parts_as_at_once() {
    let color = signal(rgb(0x3366cc));
    repaints_the_same_in_parts_as_at_once(
        400,
        300,
        move || {
            div()
                .size_full()
                .bg(Color::WHITE)
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(100.0)
                        .w(20.0)
                        .h(40.0)
                        .dyn_style(move |s| s.bg(color.get())),
                )
                // Each reads what the one before shows, further and further from the change.
                .child(
                    div()
                        .absolute()
                        .left(20.0)
                        .top(80.0)
                        .w(80.0)
                        .h(80.0)
                        .backdrop_blur(4.0),
                )
                .child(
                    div()
                        .absolute()
                        .left(110.0)
                        .top(80.0)
                        .w(80.0)
                        .h(80.0)
                        .backdrop_blur(4.0),
                )
                .child(
                    div()
                        .absolute()
                        .left(200.0)
                        .top(80.0)
                        .w(80.0)
                        .h(80.0)
                        .fill(|| Image::backdrop().blur(4.0)),
                )
        },
        &[
            ("the start of the chain", &move || color.set(rgb(0xcc3366))),
            ("again", &move || color.set(rgb(0x33cc66))),
        ],
    );
}

#[test]
fn a_filter_repaints_the_same_in_parts_as_at_once() {
    let radius = signal(3.0_f32);
    let inner = signal(rgb(0x3366cc));
    let around = signal(rgb(0xcccccc));
    let opacity = signal(1.0_f32);
    let turned = signal(0.0_f32);
    repaints_the_same_in_parts_as_at_once(
        300,
        300,
        move || {
            div()
                .size_full()
                .bg(Color::WHITE)
                .child(
                    div()
                        .absolute()
                        .left(100.0)
                        .top(100.0)
                        .w(100.0)
                        .h(100.0)
                        .dyn_style(move |s| s.bg(around.get())),
                )
                .child(
                    div()
                        .absolute()
                        .left(120.0)
                        .top(120.0)
                        .w(60.0)
                        .h(60.0)
                        .bg(Color::BLACK)
                        .dyn_style(move |s| s.opacity(opacity.get()).rotate(turned.get()))
                        .filter(move || Image::content().blur(radius.get()))
                        .child(div().w(20.0).h(20.0).dyn_style(move |s| s.bg(inner.get()))),
                )
        },
        &[
            ("nothing", &|| {}),
            ("the radius", &move || radius.set(8.0)),
            ("a bigger radius", &move || radius.set(30.0)),
            ("what is inside", &move || inner.set(rgb(0xcc3366))),
            ("what is around", &move || around.set(rgb(0x66cc33))),
            ("opacity", &move || opacity.set(0.5)),
            ("a smaller radius", &move || radius.set(1.0)),
            ("turned", &move || turned.set(30.0)),
            ("turned, with the radius", &move || radius.set(6.0)),
            ("turned, with what is inside", &move || {
                inner.set(rgb(0x33cc66))
            }),
            ("back", &move || turned.set(0.0)),
        ],
    );
}

#[test]
fn a_backdrop_over_scrolled_content_repaints_the_same_in_parts_as_at_once() {
    let scroller = node_ref();
    let at = signal(0_i32);
    repaints_the_same_in_parts_as_at_once(
        300,
        300,
        move || {
            effect(move || {
                let y = at.get();
                if let Some(id) = scroller.get() {
                    anti_webui_core::tree::with_doc(|d| {
                        if let Some(n) = d.get_mut(id) {
                            n.scroll.y = y;
                            d.mark_scrolled(id);
                        }
                    });
                }
            });
            div()
                .size_full()
                .relative()
                .bg(Color::WHITE)
                .child(
                    div()
                        .size_full()
                        .overflow_y_scroll()
                        .node_ref(scroller)
                        .children((0..60).map(|i| {
                            div().h(30.0).flex_none().bg(if i % 2 == 0 {
                                rgb(0x3366cc)
                            } else {
                                rgb(0xcccc33)
                            })
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(300.0)
                        .h(60.0)
                        .bg(Color::from_rgba(0xffffff40))
                        .backdrop_blur(8.0),
                )
        },
        &[
            ("scrolls", &move || at.set(17)),
            ("scrolls more", &move || at.set(100)),
            ("far", &move || at.set(700)),
            ("back", &move || at.set(650)),
        ],
    );
}

#[test]
fn a_backdrop_inside_a_scrolled_layer_repaints_the_same_in_parts_as_at_once() {
    let scroller = node_ref();
    let at = signal(0_i32);
    let color = signal(rgb(0x3366cc));
    repaints_the_same_in_parts_as_at_once(
        300,
        300,
        move || {
            effect(move || {
                let y = at.get();
                if let Some(id) = scroller.get() {
                    anti_webui_core::tree::with_doc(|d| {
                        if let Some(n) = d.get_mut(id) {
                            n.scroll.y = y;
                            d.mark_scrolled(id);
                        }
                    });
                }
            });
            div().size_full().bg(Color::WHITE).child(
                div()
                    .size_full()
                    .overflow_y_scroll()
                    .node_ref(scroller)
                    .children((0..40).map(move |i| {
                        div()
                            .h(60.0)
                            .flex_none()
                            .relative()
                            .dyn_style(move |s| {
                                s.bg(if i == 3 {
                                    color.get()
                                } else if i % 2 == 0 {
                                    rgb(0x88cc33)
                                } else {
                                    rgb(0xcccc33)
                                })
                            })
                            .child(
                                div()
                                    .absolute()
                                    .left(40.0)
                                    .top(-10.0)
                                    .w(120.0)
                                    .h(80.0)
                                    .bg(Color::from_rgba(0xffffff30))
                                    .backdrop_blur(6.0),
                            )
                    })),
            )
        },
        &[
            ("scrolls", &move || at.set(23)),
            ("something inside changes", &move || {
                color.set(rgb(0xcc3366))
            }),
            ("past the size of a texture", &move || at.set(901)),
            ("back", &move || at.set(40)),
        ],
    );
}
