//! Schemes from a seed color, seeds from an image, and changing the scheme gradually. See
//! `docs/design/15-md3e.md`. The exact values come from upstream's Swift scheme tests, as do the
//! tests of the port itself in `src/dynamic/tests.rs`.

mod common;

use anti_webui::core::scene::{Primitive, Scene};
use anti_webui::prelude::*;
use anti_webui_md3e::dynamic::{Hct, TonalPalette, seed_colors};
use anti_webui_md3e::{
    ColorScheme, SchemeOptions, SchemeVariant, color_scheme, motion, set_color_scheme,
    set_color_scheme_animated,
};
use common::*;
use std::time::{Duration, Instant};

fn argb(color: Color) -> u32 {
    Hct::from_color(color).to_argb()
}

#[test]
fn a_seed_makes_the_scheme_upstream_makes() {
    // SchemeTonalSpotTests.swift, test3rdPartyLightScheme and test3rdPartyDarkScheme.
    let light = ColorScheme::from_seed(rgb(0x6750a4), false);
    assert_eq!(argb(light.primary), 0xff65558f);
    assert_eq!(argb(light.secondary), 0xff625b71);
    assert_eq!(argb(light.tertiary), 0xff7e5260);
    assert_eq!(argb(light.surface), 0xfffdf7ff);
    assert_eq!(argb(light.on_surface), 0xff1d1b20);
    let dark = ColorScheme::from_seed(rgb(0x6750a4), true);
    assert_eq!(argb(dark.primary), 0xffcfbdfe);
    assert_eq!(argb(dark.secondary), 0xffcbc2db);
    assert_eq!(argb(dark.tertiary), 0xffefb8c8);
    assert_eq!(argb(dark.surface), 0xff141218);
    assert_eq!(argb(dark.on_surface), 0xffe6e0e9);

    // testLightSchemeFromHighChromaColor.
    let c = ColorScheme::from_seed(rgb(0xfa2bec), false);
    let expected = [
        (c.primary, 0xff814c77),
        (c.on_primary, 0xffffffff),
        (c.primary_container, 0xffffd7f3),
        (c.on_primary_container, 0xff67355e),
        (c.secondary_container, 0xfff8daee),
        (c.tertiary_container, 0xffffdbd0),
        (c.error, 0xffba1a1a),
        (c.error_container, 0xffffdad6),
        (c.on_error_container, 0xff93000a),
        (c.surface_variant, 0xffeedee7),
        (c.on_surface_variant, 0xff4e444b),
        (c.outline, 0xff80747b),
        (c.outline_variant, 0xffd2c2cb),
        (c.scrim, 0xff000000),
        (c.inverse_surface, 0xff362e33),
        (c.inverse_on_surface, 0xfffaedf3),
        (c.inverse_primary, 0xfff3b2e4),
    ];
    for (i, (color, value)) in expected.into_iter().enumerate() {
        assert_eq!(argb(color), value, "{i}");
    }
}

#[test]
fn variants_and_contrast_change_the_scheme() {
    let blue = rgb(0x0000ff);
    let with = |variant, dark, contrast| {
        ColorScheme::from_seed_with(
            blue,
            SchemeOptions {
                variant,
                dark,
                contrast,
            },
        )
    };
    // SchemeVibrantTests.swift.
    assert_eq!(
        argb(with(SchemeVariant::Vibrant, false, 0.0).primary),
        0xff343dff
    );
    assert_eq!(
        argb(with(SchemeVariant::Vibrant, false, 1.0).primary),
        0xff00019f
    );
    assert_eq!(
        argb(with(SchemeVariant::Vibrant, true, -1.0).primary),
        0xff7c84ff
    );
    // SchemeMonochromeTests.swift.
    assert_eq!(
        argb(with(SchemeVariant::Monochrome, false, 0.0).primary),
        0xff000000
    );
    // Beyond 1 is 1.
    assert_eq!(
        with(SchemeVariant::Vibrant, false, 5.0),
        with(SchemeVariant::Vibrant, false, 1.0)
    );
    assert_eq!(SchemeVariant::default(), SchemeVariant::TonalSpot);
    assert_eq!(
        ColorScheme::from_seed(blue, true),
        with(SchemeVariant::TonalSpot, true, 0.0)
    );
}

#[test]
fn every_variant_keeps_text_readable() {
    use SchemeVariant::*;
    let ratio = |a: Color, b: Color| {
        let y = |c: Color| {
            let l = |v: f32| {
                let v = v as f64;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * l(c.r) + 0.7152 * l(c.g) + 0.0722 * l(c.b)
        };
        let (a, b) = (y(a), y(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    };
    for variant in [
        TonalSpot, Neutral, Vibrant, Expressive, Fidelity, Content, Monochrome, Rainbow, FruitSalad,
    ] {
        for dark in [false, true] {
            for seed in [0xff0000, 0xffff00, 0x00ff00, 0x0000ff, 0x6750a4] {
                let c = ColorScheme::from_seed_with(
                    rgb(seed),
                    SchemeOptions {
                        variant,
                        dark,
                        contrast: 0.0,
                    },
                );
                let pairs = [
                    (c.on_primary, c.primary),
                    (c.on_primary_container, c.primary_container),
                    (c.on_secondary_container, c.secondary_container),
                    (c.on_tertiary_container, c.tertiary_container),
                    (c.on_error, c.error),
                    (c.on_surface, c.surface),
                    (c.on_surface_variant, c.surface_container_highest),
                    (c.inverse_on_surface, c.inverse_surface),
                ];
                for (i, (fg, bg)) in pairs.into_iter().enumerate() {
                    // 4.5 by tone; a little less once rounded to 8 bits.
                    let r = ratio(fg, bg);
                    assert!(r > 4.4, "{variant:?} {dark} {seed:06x} pair {i}: {r}");
                }
                let tone = |c: Color| Hct::from_color(c).tone();
                // The containers go from the surface outwards in order.
                let surfaces = [
                    c.surface_container_lowest,
                    c.surface_container_low,
                    c.surface_container,
                    c.surface_container_high,
                    c.surface_container_highest,
                ];
                for w in surfaces.windows(2) {
                    if dark {
                        assert!(tone(w[0]) < tone(w[1]));
                    } else {
                        assert!(tone(w[0]) > tone(w[1]));
                    }
                }
            }
        }
    }
}

#[test]
fn hct_and_palettes() {
    let hct = Hct::from_argb(0xff6750a4);
    assert_eq!(
        Hct::new(hct.hue(), hct.chroma(), hct.tone()).to_argb(),
        0xff6750a4
    );
    assert_eq!(hct.to_color(), rgb(0x6750a4));
    // palettes_test.ts, ofBlue.
    let blue = TonalPalette::from_hct(Hct::from_argb(0xff0000ff));
    assert_eq!(blue.tone(40.0), rgb(0x343dff));
    assert_eq!(blue.tone(100.0), Color::WHITE);
    let palette = TonalPalette::from_hue_and_chroma(270.0, 36.0);
    assert!((palette.hct(50.0).tone() - 50.0).abs() < 0.5);
}

#[test]
fn lerp_goes_from_one_scheme_to_the_other() {
    let (a, b) = (ColorScheme::light(), ColorScheme::dark());
    assert_eq!(a.lerp(&b, 0.0), a);
    assert_eq!(a.lerp(&b, 1.0), b);
    assert_eq!(a.lerp(&b, -1.0), a);
    assert_eq!(a.lerp(&b, 2.0), b);
    let half = a.lerp(&b, 0.5);
    assert_eq!(half.primary, mix_premultiplied(a.primary, b.primary, 0.5));
    assert_eq!(
        half.outline_variant,
        mix_premultiplied(a.outline_variant, b.outline_variant, 0.5)
    );
    // The same way an animated color goes.
    assert_eq!(
        half.surface,
        Color::from_components(std::array::from_fn(|i| {
            let (x, y) = (a.surface.to_components()[i], b.surface.to_components()[i]);
            x + (y - x) * 0.5
        }))
    );
}

#[test]
fn seeds_come_from_the_hue_most_of_an_image_has() {
    // Mostly green with some noise, a little red, and some gray.
    let mut pixels = Vec::new();
    for i in 0..128 * 128u32 {
        let pixel = match i % 10 {
            0 => [0xd0, 0x30, 0x30, 0xff],
            1 => [0x80, 0x80, 0x80, 0xff],
            2 => [0, 0, 0, 0],
            _ => {
                let n = (i % 7) as u8 * 4;
                [0x30 + n, 0x90 + n, 0x40, 0xff]
            }
        };
        pixels.push(pixel);
    }
    let seeds = seed_colors(pixels.iter().copied(), 4);
    assert!(!seeds.is_empty() && seeds.len() <= 4);
    let hue = |c: Color| Hct::from_color(c).hue();
    let green = hue(rgb(0x3c9c40));
    let first = hue(seeds[0]);
    assert!((first - green).abs() < 10.0, "{first} isn't near {green}");
    assert_eq!(seed_colors(pixels.iter().copied(), 4), seeds);

    // Nothing colorful: Google Blue.
    assert_eq!(
        seed_colors(vec![[0x80, 0x80, 0x80, 0xff]; 100], 4),
        [rgb(0x4285f4)]
    );
    assert_eq!(seed_colors(Vec::new(), 4), [rgb(0x4285f4)]);
}

fn shown(scene: &Scene) -> Color {
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Quad(q) if q.rect.width > 20 => Some(q.background),
            _ => None,
        })
        .expect("a box")
}

fn start() -> anti_webui::core::Ui {
    start_ui(|| {
        div()
            .w(100.0)
            .h(100.0)
            .dyn_style(|s| s.bg(color_scheme().primary))
    })
}

/// Plays a frame `at` this long after `start`.
fn frame(ui: &mut anti_webui::core::Ui, start: Instant, at: u64) -> Scene {
    let now = start + Duration::from_millis(at);
    ui.animate(now);
    ui.update();
    ui.paint(now)
}

#[test]
fn an_animated_change_goes_there_frame_by_frame_and_stops() {
    let mut ui = start();
    let (a, b) = (
        ColorScheme::light(),
        ColorScheme::from_seed(rgb(0x386a20), true),
    );
    set_color_scheme(a);
    assert_eq!(shown(&settle(&mut ui)), a.primary.snapped());
    assert!(!ui.animating());

    let start = Instant::now();
    set_color_scheme_animated(b, motion::default_effects());
    assert!(ui.animating());
    let mut seen = Vec::new();
    for t in (8..=400).step_by(8) {
        seen.push(shown(&frame(&mut ui, start, t)));
    }
    assert!(
        seen.iter()
            .any(|&c| c != a.primary.snapped() && c != b.primary.snapped()),
        "something in between"
    );
    assert_eq!(*seen.last().unwrap(), b.primary.snapped());
    assert_eq!(color_scheme(), b);
    frame(&mut ui, start, 408);
    assert!(!ui.animating(), "no frames once there");
    set_color_scheme(ColorScheme::light());
}

#[test]
fn an_animated_change_goes_on_from_where_it_is() {
    let mut ui = start();
    let (a, b, c) = (
        ColorScheme::light(),
        ColorScheme::dark(),
        ColorScheme::from_seed(rgb(0xb3261e), false),
    );
    set_color_scheme(a);
    settle(&mut ui);
    let start = Instant::now();
    set_color_scheme_animated(b, motion::slow_effects());
    for t in (8..=40).step_by(8) {
        frame(&mut ui, start, t);
    }
    let midway = color_scheme();
    assert!(midway != a && midway != b);

    // Springs count time from when they start, which is now, not 40ms after `start`.
    let start = Instant::now();
    set_color_scheme_animated(c, motion::slow_effects());
    // Nothing jumps: the first frame is next to where it was.
    let first = shown(&frame(&mut ui, start, 8));
    let d = |x: Color, y: Color| (x.r - y.r).abs() + (x.g - y.g).abs() + (x.b - y.b).abs();
    assert!(d(first, midway.primary) < 0.1);
    for t in (16..=1500).step_by(8) {
        frame(&mut ui, start, t);
    }
    assert_eq!(color_scheme(), c);
    assert!(!ui.animating());
    set_color_scheme(ColorScheme::light());
}

#[test]
fn setting_the_scheme_stops_a_change_going_on() {
    let mut ui = start();
    set_color_scheme(ColorScheme::light());
    settle(&mut ui);
    let start = Instant::now();
    set_color_scheme_animated(ColorScheme::dark(), motion::slow_effects());
    frame(&mut ui, start, 8);
    frame(&mut ui, start, 16);
    let c = ColorScheme::from_seed(rgb(0x00639b), false);
    set_color_scheme(c);
    for t in (24..=600).step_by(8) {
        assert_eq!(shown(&frame(&mut ui, start, t)), c.primary.snapped());
    }
    assert_eq!(color_scheme(), c);
    assert!(!ui.animating());
    set_color_scheme(ColorScheme::light());
}
