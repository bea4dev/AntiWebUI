//! Upstream's tests, with the values they expect. Where a value comes from is noted by each group:
//! the TypeScript tests (`typescript/**/*_test.ts`), the Swift scheme tests
//! (`swift/Tests/MaterialColorUtilitiesTests/SchemeTests`) and the Dart tests (`dart/test`), all
//! at the commit in `mod.rs`.

use super::contrast::{self, ContrastCurve, curve};
use super::hct::{Cam16, DEFAULT_VIEWING_CONDITIONS, Hct};
use super::palette::TonalPalette;
use super::scheme::{DynamicScheme, Role, rotated_hue};
use super::temperature::{TemperatureCache, raw_temperature};
use super::{dislike, quantize, score, utils};
use crate::color::SchemeVariant;

/// Jasmine's `toBeCloseTo(expected, digits)`.
#[track_caller]
fn close(actual: f64, expected: f64, digits: i32) {
    assert!(
        (actual - expected).abs() < 10f64.powi(-digits) / 2.0,
        "{actual} isn't {expected} to {digits} digits"
    );
}

// hct/hct_test.ts

#[test]
fn cam16_of_primaries() {
    let cases = [
        (
            0xffff0000,
            [27.408, 113.358, 46.445, 89.494, 91.890, 105.989],
        ),
        (
            0xff00ff00,
            [142.140, 108.410, 79.332, 85.588, 78.605, 138.520],
        ),
        (
            0xff0000ff,
            [282.788, 87.231, 25.466, 68.867, 93.675, 78.481],
        ),
        (0xffffffff, [209.492, 2.869, 100.0, 2.265, 12.068, 155.521]),
        (0xff000000, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ];
    for (argb, [hue, chroma, j, m, s, q]) in cases {
        let cam = Cam16::from_argb(argb);
        close(cam.hue, hue, 3);
        close(cam.chroma, chroma, 3);
        close(cam.j, j, 3);
        close(cam.m, m, 3);
        close(cam.s, s, 3);
        close(cam.q, q, 3);
    }
}

#[test]
fn cam16_round_trips() {
    for argb in [0xffff0000, 0xff00ff00, 0xff0000ff] {
        assert_eq!(Cam16::from_argb(argb).to_argb(), argb);
    }
}

#[test]
fn argb_to_hct() {
    let green = Hct::from_argb(0xff00ff00);
    close(green.hue(), 142.139, 2);
    close(green.chroma(), 108.410, 2);
    close(green.tone(), 87.737, 2);

    let blue = Hct::from_argb(0xff0000ff);
    close(blue.hue(), 282.788, 2);
    close(blue.chroma(), 87.230, 2);
    close(blue.tone(), 32.302, 2);

    let blue_tone_90 = Hct::new(282.788, 87.230, 90.0);
    close(blue_tone_90.hue(), 282.239, 2);
    close(blue_tone_90.chroma(), 19.144, 2);
    close(blue_tone_90.tone(), 90.035, 2);
}

#[test]
fn default_viewing_conditions() {
    let vc = &*DEFAULT_VIEWING_CONDITIONS;
    close(vc.n, 0.184, 3);
    close(vc.aw, 29.981, 3);
    close(vc.nbb, 1.017, 3);
    close(vc.ncb, 1.017, 3);
    close(vc.c, 0.69, 3);
    close(vc.nc, 1.0, 3);
    close(vc.rgb_d[0], 1.021, 3);
    close(vc.rgb_d[1], 0.986, 3);
    close(vc.rgb_d[2], 0.934, 3);
    close(vc.fl, 0.388, 3);
    close(vc.fl_root, 0.789, 3);
    close(vc.z, 1.909, 3);
}

#[test]
fn the_solver_returns_a_sufficiently_close_color() {
    let on_boundary = |argb: u32| {
        [
            utils::red_from_argb(argb),
            utils::green_from_argb(argb),
            utils::blue_from_argb(argb),
        ]
        .iter()
        .any(|&c| c == 0 || c == 255)
    };
    for hue in (15..360).step_by(30) {
        for chroma in (0..=100).step_by(10) {
            for tone in (20..=80).step_by(10) {
                let (hue, chroma, tone) = (hue as f64, chroma as f64, tone as f64);
                let hct = Hct::new(hue, chroma, tone);
                if chroma > 0.0 {
                    assert!((hct.hue() - hue).abs() <= 4.0);
                }
                assert!(hct.chroma() >= 0.0 && hct.chroma() <= chroma + 2.5);
                if hct.chroma() < chroma - 2.5 {
                    assert!(on_boundary(hct.to_argb()));
                }
                assert!((hct.tone() - tone).abs() <= 0.5);
            }
        }
    }
}

// hct/hct_round_trip_test.ts

#[test]
fn hct_round_trips() {
    for r in (0..296).step_by(37) {
        for g in (0..296).step_by(37) {
            for b in (0..296).step_by(37) {
                let c = |v: u32| v.min(255) as u8;
                let argb = utils::argb_from_rgb(c(r), c(g), c(b));
                let hct = Hct::from_argb(argb);
                let back = Hct::new(hct.hue(), hct.chroma(), hct.tone()).to_argb();
                assert_eq!(back, argb, "{argb:08x}");
            }
        }
    }
}

// palettes/palettes_test.ts

#[test]
fn tonal_palette_of_blue() {
    let blue = TonalPalette::from_hct(Hct::from_argb(0xff0000ff));
    let expected = [
        (100.0, 0xffffffff),
        (95.0, 0xfff1efff),
        (90.0, 0xffe0e0ff),
        (80.0, 0xffbec2ff),
        (70.0, 0xff9da3ff),
        (60.0, 0xff7c84ff),
        (50.0, 0xff5a64ff),
        (40.0, 0xff343dff),
        (30.0, 0xff0000ef),
        (20.0, 0xff0001ac),
        (10.0, 0xff00006e),
        (0.0, 0xff000000),
    ];
    for (tone, argb) in expected {
        assert_eq!(blue.argb(tone), argb, "tone {tone}");
    }
}

#[test]
fn key_colors() {
    let exact = TonalPalette::from_hue_and_chroma(50.0, 60.0).key_color();
    assert!((exact.hue() - 50.0).abs() < 10.0);
    assert!((exact.chroma() - 60.0).abs() < 0.5);
    assert!(exact.tone() > 0.0 && exact.tone() < 100.0);

    let high = TonalPalette::from_hue_and_chroma(149.0, 200.0).key_color();
    assert!((high.hue() - 149.0).abs() < 10.0);
    assert!(high.chroma() > 89.0);
    assert!(high.tone() > 0.0 && high.tone() < 100.0);

    let low = TonalPalette::from_hue_and_chroma(50.0, 3.0).key_color();
    assert!((low.hue() - 50.0).abs() < 10.0);
    assert!((low.chroma() - 3.0).abs() < 0.5);
    assert!((low.tone() - 50.0).abs() < 0.5);
}

// contrast/contrast_test.ts

#[test]
fn contrast_ratios() {
    close(contrast::ratio_of_tones(100.0, 0.0), 21.0, 3);
    close(contrast::ratio_of_tones(0.0, 100.0), 21.0, 3);
    close(contrast::ratio_of_tones(50.0, 50.0), 1.0, 3);
    close(contrast::lighter_unsafe(0.0, 21.0), 100.0, 3);
    close(contrast::darker_unsafe(100.0, 21.0), 0.0, 3);
    assert_eq!(contrast::lighter(-10.0, 2.0), None);
    assert_eq!(contrast::darker(110.0, 2.0), None);
    assert_eq!(contrast::lighter(100.0, 1.5), None);
}

// dislike/dislike_analyzer_test.ts

#[test]
fn dislike_analyzer() {
    let monk_skin_tone_scale = [
        0xfff6ede4, 0xfff3e7db, 0xfff7ead0, 0xffeadaba, 0xffd7bd96, 0xffa07e56, 0xff825c43,
        0xff604134, 0xff3a312a, 0xff292420,
    ];
    for argb in monk_skin_tone_scale {
        assert!(!dislike::is_disliked(Hct::from_argb(argb)));
    }
    let bile = [0xff95884b, 0xff716b40, 0xffb08e00, 0xff4c4308, 0xff464521];
    for argb in bile {
        let hct = Hct::from_argb(argb);
        assert!(dislike::is_disliked(hct));
        assert!(!dislike::is_disliked(dislike::fix_if_disliked(hct)));
    }
    let tone_67 = Hct::new(100.0, 50.0, 67.0);
    assert!(!dislike::is_disliked(tone_67));
    assert_eq!(
        dislike::fix_if_disliked(tone_67).to_argb(),
        tone_67.to_argb()
    );
}

// temperature/temperature_cache_test.ts

#[test]
fn raw_temperatures() {
    close(raw_temperature(Hct::from_argb(0xff0000ff)), -1.393, 3);
    close(raw_temperature(Hct::from_argb(0xffff0000)), 2.351, 3);
    close(raw_temperature(Hct::from_argb(0xff00ff00)), -0.267, 3);
    close(raw_temperature(Hct::from_argb(0xffffffff)), -0.5, 3);
    close(raw_temperature(Hct::from_argb(0xff000000)), -0.5, 3);
}

#[test]
fn relative_temperatures() {
    let relative = |argb| TemperatureCache::new(Hct::from_argb(argb)).input_relative_temperature();
    close(relative(0xff0000ff), 0.0, 3);
    close(relative(0xffff0000), 1.0, 3);
    close(relative(0xff00ff00), 0.467, 3);
    close(relative(0xffffffff), 0.5, 3);
    close(relative(0xff000000), 0.5, 3);
}

#[test]
fn complements() {
    let complement = |argb| {
        TemperatureCache::new(Hct::from_argb(argb))
            .complement()
            .to_argb()
    };
    assert_eq!(complement(0xff0000ff), 0xff9d0002);
    assert_eq!(complement(0xffff0000), 0xff007bfc);
    assert_eq!(complement(0xff00ff00), 0xffffd2c9);
    assert_eq!(complement(0xffffffff), 0xffffffff);
    assert_eq!(complement(0xff000000), 0xff000000);
}

#[test]
fn analogous_colors() {
    let analogous = |argb| -> Vec<u32> {
        TemperatureCache::new(Hct::from_argb(argb))
            .analogous(5, 12)
            .iter()
            .map(Hct::to_argb)
            .collect()
    };
    assert_eq!(
        analogous(0xff0000ff),
        [0xff00590c, 0xff00564e, 0xff0000ff, 0xff6700cc, 0xff81009f]
    );
    assert_eq!(
        analogous(0xffff0000),
        [0xfff60082, 0xfffc004c, 0xffff0000, 0xffd95500, 0xffaf7200]
    );
    assert_eq!(
        analogous(0xff00ff00),
        [0xffcee900, 0xff92f500, 0xff00ff00, 0xff00fd6f, 0xff00fab3]
    );
    assert_eq!(analogous(0xff000000), [0xff000000; 5]);
    assert_eq!(analogous(0xffffffff), [0xffffffff; 5]);
}

// score/score_test.ts

#[test]
fn scoring() {
    type Case = (&'static [(u32, u32)], usize, u32, bool, &'static [u32]);
    let blue = 0xff4285f4;
    let cases: [Case; 15] = [
        // Prioritizes chroma.
        (
            &[(0xff000000, 1), (0xffffffff, 1), (0xff0000ff, 1)],
            4,
            blue,
            true,
            &[0xff0000ff],
        ),
        // Prioritizes chroma when proportions are equal.
        (
            &[(0xffff0000, 1), (0xff00ff00, 1), (0xff0000ff, 1)],
            4,
            blue,
            true,
            &[0xffff0000, 0xff00ff00, 0xff0000ff],
        ),
        // Google Blue when no colors are available.
        (&[(0xff000000, 1)], 4, blue, true, &[0xff4285f4]),
        // Dedupes nearby hues.
        (
            &[(0xff008772, 1), (0xff318477, 1)],
            4,
            blue,
            true,
            &[0xff008772],
        ),
        // Maximizes hue distance.
        (
            &[(0xff008772, 1), (0xff008587, 1), (0xff007ebc, 1)],
            2,
            blue,
            true,
            &[0xff007ebc, 0xff008772],
        ),
        // Generated scenarios one to ten.
        (
            &[(0xff7ea16d, 67), (0xffd8ccae, 67), (0xff835c0d, 49)],
            3,
            0xff8d3819,
            false,
            &[0xff7ea16d, 0xffd8ccae, 0xff835c0d],
        ),
        (
            &[
                (0xffd33881, 14),
                (0xff3205cc, 77),
                (0xff0b48cf, 36),
                (0xffa08f5d, 81),
            ],
            4,
            0xff7d772b,
            true,
            &[0xff3205cc, 0xffa08f5d, 0xffd33881],
        ),
        (
            &[
                (0xffbe94a6, 23),
                (0xffc33fd7, 42),
                (0xff899f36, 90),
                (0xff94c574, 82),
            ],
            3,
            0xffaa79a4,
            true,
            &[0xff94c574, 0xffc33fd7, 0xffbe94a6],
        ),
        (
            &[
                (0xffdf241c, 85),
                (0xff685859, 44),
                (0xffd06d5f, 34),
                (0xff561c54, 27),
                (0xff713090, 88),
            ],
            5,
            0xff58c19c,
            false,
            &[0xffdf241c, 0xff561c54],
        ),
        (
            &[
                (0xffbe66f8, 41),
                (0xff4bbda9, 88),
                (0xff80f6f9, 44),
                (0xffab8017, 43),
                (0xffe89307, 65),
            ],
            3,
            0xff916691,
            false,
            &[0xffab8017, 0xff4bbda9, 0xffbe66f8],
        ),
        (
            &[
                (0xff18ea8f, 93),
                (0xff327593, 18),
                (0xff066a18, 53),
                (0xfffa8a23, 74),
                (0xff04ca1f, 62),
            ],
            2,
            0xff4c377a,
            false,
            &[0xff18ea8f, 0xfffa8a23],
        ),
        (
            &[
                (0xff2e05ed, 23),
                (0xff153e55, 90),
                (0xff9ab220, 23),
                (0xff153379, 66),
                (0xff68bcc3, 81),
            ],
            2,
            0xfff588dc,
            true,
            &[0xff2e05ed, 0xff9ab220],
        ),
        (
            &[
                (0xff816ec5, 24),
                (0xff6dcb94, 19),
                (0xff3cae91, 98),
                (0xff5b542f, 25),
            ],
            1,
            0xff84b0fd,
            false,
            &[0xff3cae91],
        ),
        (
            &[
                (0xff206f86, 52),
                (0xff4a620d, 96),
                (0xfff51401, 85),
                (0xff2b8ebf, 3),
                (0xff277766, 59),
            ],
            3,
            0xff02b415,
            true,
            &[0xfff51401, 0xff4a620d, 0xff2b8ebf],
        ),
        (
            &[
                (0xff8b1d99, 54),
                (0xff27effe, 43),
                (0xff6f558d, 2),
                (0xff77fdf2, 78),
            ],
            4,
            0xff5e7a10,
            true,
            &[0xff27effe, 0xff8b1d99, 0xff6f558d],
        ),
    ];
    for (i, (colors, desired, fallback, filter, expected)) in cases.into_iter().enumerate() {
        assert_eq!(
            score::score(colors, desired, fallback, filter),
            expected,
            "case {i}"
        );
    }
}

// quantize/quantizer_celebi_test.ts

#[test]
fn celebi_keeps_few_colors_as_they_are() {
    const RED: u32 = 0xffff0000;
    const GREEN: u32 = 0xff00ff00;
    const BLUE: u32 = 0xff0000ff;
    let sorted = |mut v: Vec<(u32, u32)>| {
        v.sort();
        v
    };
    assert_eq!(quantize::celebi(&[RED], 128), [(RED, 1)]);
    assert_eq!(quantize::celebi(&[GREEN], 128), [(GREEN, 1)]);
    assert_eq!(quantize::celebi(&[BLUE], 128), [(BLUE, 1)]);
    assert_eq!(quantize::celebi(&[BLUE; 5], 128), [(BLUE, 5)]);
    assert_eq!(
        sorted(quantize::celebi(&[RED, RED, GREEN, GREEN, GREEN], 128)),
        sorted(vec![(RED, 2), (GREEN, 3)])
    );
    assert_eq!(
        sorted(quantize::celebi(&[RED, GREEN, BLUE], 128)),
        sorted(vec![(RED, 1), (GREEN, 1), (BLUE, 1)])
    );
}

#[test]
fn quantizers_are_deterministic() {
    let mut pixels = Vec::new();
    for r in (0..256u32).step_by(16) {
        for g in (0..256u32).step_by(16) {
            let b = (r * 3 + g * 5) & 0xff;
            pixels.push(0xff00_0000 | r << 16 | g << 8 | b);
        }
    }
    assert_eq!(quantize::celebi(&pixels, 32), quantize::celebi(&pixels, 32));
    assert_eq!(
        quantize::wsmeans(&pixels, &[], 16),
        quantize::wsmeans(&pixels, &[], 16)
    );
    assert!(!quantize::wsmeans(&pixels, &[], 16).is_empty());
}

// scheme/dynamic_scheme_test.ts

#[test]
fn rotated_hues() {
    let source = Hct::new(43.0, 16.0, 16.0);
    let rotated =
        |breakpoints: &[f64], rotations: &[f64]| rotated_hue(source, breakpoints, rotations);
    // `toBeCloseTo(x, 0.4)` allows about 0.2.
    let near = |a: f64, b: f64| assert!((a - b).abs() < 10f64.powf(-0.4) / 2.0, "{a} {b}");
    near(rotated(&[], &[]), 43.0);
    near(rotated(&[0.0], &[0.0]), 43.0);
    near(rotated(&[0.0], &[0.0, 1.0]), 43.0);
    near(rotated(&[0.0, 42.0, 360.0], &[0.0, 15.0, 0.0]), 43.0 + 15.0);
    near(rotated(&[0.0, 42.0, 360.0], &[0.0, 480.0, 0.0]), 163.0);
}

// The Swift scheme tests.

enum Expect {
    /// A field of `ColorScheme`.
    Field(&'static str),
    /// The key color of a palette.
    Key(&'static str),
}

fn field(s: &DynamicScheme, name: &str) -> u32 {
    use Role::*;
    let role = match name {
        "primary" => Primary,
        "on_primary" => OnPrimary,
        "primary_container" => PrimaryContainer,
        "on_primary_container" => OnPrimaryContainer,
        "secondary" => Secondary,
        "on_secondary" => OnSecondary,
        "secondary_container" => SecondaryContainer,
        "on_secondary_container" => OnSecondaryContainer,
        "tertiary" => Tertiary,
        "on_tertiary" => OnTertiary,
        "tertiary_container" => TertiaryContainer,
        "on_tertiary_container" => OnTertiaryContainer,
        "error" => Error,
        "on_error" => OnError,
        "error_container" => ErrorContainer,
        "on_error_container" => OnErrorContainer,
        "surface" => Surface,
        "surface_dim" => SurfaceDim,
        "surface_bright" => SurfaceBright,
        "surface_variant" => SurfaceVariant,
        "on_surface" => OnSurface,
        "on_surface_variant" => OnSurfaceVariant,
        "inverse_surface" => InverseSurface,
        "inverse_on_surface" => InverseOnSurface,
        "inverse_primary" => InversePrimary,
        "scrim" => Scrim,
        "surface_container_lowest" => SurfaceContainerLowest,
        "surface_container_low" => SurfaceContainerLow,
        "surface_container" => SurfaceContainer,
        "surface_container_high" => SurfaceContainerHigh,
        "surface_container_highest" => SurfaceContainerHighest,
        "outline" => Outline,
        "outline_variant" => OutlineVariant,
        _ => panic!("no role {name}"),
    };
    s.argb(role)
}

fn key(s: &DynamicScheme, name: &str) -> u32 {
    let palette = match name {
        "primary" => &s.primary,
        "secondary" => &s.secondary,
        "tertiary" => &s.tertiary,
        "neutral" => &s.neutral,
        "neutral_variant" => &s.neutral_variant,
        _ => panic!("no palette {name}"),
    };
    // Upstream's `primary_palette_key_color` and the others: the palette at the key color's tone.
    palette.argb(palette.key_color().tone())
}

#[test]
fn schemes_match_upstream() {
    use Expect::*;
    use SchemeVariant::*;
    #[rustfmt::skip]
    let cases = [
    (Content, 0xff0000ff, false, 0.0, Key("primary"), 0xff080cff), // SchemeContentTests.swift testKeyColors
    (Content, 0xff0000ff, false, 0.0, Key("secondary"), 0xff656dd3), // SchemeContentTests.swift testKeyColors
    (Content, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff81009f), // SchemeContentTests.swift testKeyColors
    (Content, 0xff0000ff, false, 0.0, Key("neutral"), 0xff767684), // SchemeContentTests.swift testKeyColors
    (Content, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff757589), // SchemeContentTests.swift testKeyColors
    (Content, 0xff0000ff, false, -1.0, Field("primary"), 0xff5660ff), // SchemeContentTests.swift testLightTheme_MinContrast_Primary
    (Content, 0xff0000ff, false, 0.0, Field("primary"), 0xff0001bb), // SchemeContentTests.swift testLightTheme_StandardContrast_Primary
    (Content, 0xff0000ff, false, 1.0, Field("primary"), 0xff00019f), // SchemeContentTests.swift testLightTheme_MaxContrast_Primary
    (Content, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffd5d6ff), // SchemeContentTests.swift testLightTheme_MinContrast_PrimaryContainer
    (Content, 0xff0000ff, false, 0.0, Field("primary_container"), 0xff0000ff), // SchemeContentTests.swift testLightTheme_StandardContrast_PrimaryContainer
    (Content, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff0000f6), // SchemeContentTests.swift testLightTheme_MaxContrast_PrimaryContainer
    (Content, 0xff0000ff, false, -1.0, Field("tertiary_container"), 0xfffac9ff), // SchemeContentTests.swift testLightTheme_MinContrast_TertiaryContainer
    (Content, 0xff0000ff, false, 0.0, Field("tertiary_container"), 0xff81009f), // SchemeContentTests.swift testLightTheme_StandardContrast_TertiaryContainer
    (Content, 0xff0000ff, false, 1.0, Field("tertiary_container"), 0xff7d009a), // SchemeContentTests.swift testLightTheme_MaxContrast_TertiaryContainer
    (Content, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff5e68ff), // SchemeContentTests.swift testLightTheme_MinContrast_OnPrimaryContainer
    (Content, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xffb3b7ff), // SchemeContentTests.swift testLightTheme_StandardContrast_OnPrimaryContainer
    (Content, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeContentTests.swift testLightTheme_MaxContrast_OnPrimaryContainer
    (Content, 0xff0000ff, false, -1.0, Field("surface"), 0xfffbf8ff), // SchemeContentTests.swift testLightTheme_MinContrast_Surface
    (Content, 0xff0000ff, false, 0.0, Field("surface"), 0xfffbf8ff), // SchemeContentTests.swift testLightTheme_StandardContrast_Surface
    (Content, 0xff0000ff, false, 1.0, Field("surface"), 0xfffbf8ff), // SchemeContentTests.swift testLightTheme_MaxContrast_Surface
    (Content, 0xff0000ff, true, -1.0, Field("primary"), 0xff7c84ff), // SchemeContentTests.swift testDarkTheme_MinContrast_Primary
    (Content, 0xff0000ff, true, 0.0, Field("primary"), 0xffbec2ff), // SchemeContentTests.swift testDarkTheme_StandardContrast_Primary
    (Content, 0xff0000ff, true, 1.0, Field("primary"), 0xfff0eeff), // SchemeContentTests.swift testDarkTheme_MaxContrast_Primary
    (Content, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff0001c9), // SchemeContentTests.swift testDarkTheme_MinContrast_PrimaryContainer
    (Content, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff0000ff), // SchemeContentTests.swift testDarkTheme_StandardContrast_PrimaryContainer
    (Content, 0xff0000ff, true, 1.0, Field("primary_container"), 0xffbabdff), // SchemeContentTests.swift testDarkTheme_MaxContrast_PrimaryContainer
    (Content, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff6b75ff), // SchemeContentTests.swift testDarkTheme_MinContrast_OnPrimaryContainer
    (Content, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffb3b7ff), // SchemeContentTests.swift testDarkTheme_StandardContrast_OnPrimaryContainer
    (Content, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff00003d), // SchemeContentTests.swift testDarkTheme_MaxContrast_OnPrimaryContainer
    (Content, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xffc254de), // SchemeContentTests.swift testDarkTheme_MinContrast_OnTertiaryContainer
    (Content, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xfff09fff), // SchemeContentTests.swift testDarkTheme_StandardContrast_OnTertiaryContainer
    (Content, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff1a0022), // SchemeContentTests.swift testDarkTheme_MaxContrast_OnTertiaryContainer
    (Content, 0xff0000ff, true, -1.0, Field("surface"), 0xff12121d), // SchemeContentTests.swift testDarkTheme_MinContrast_Surface
    (Content, 0xff0000ff, true, 0.0, Field("surface"), 0xff12121d), // SchemeContentTests.swift testDarkTheme_StandardContrast_Surface
    (Content, 0xff0000ff, true, 1.0, Field("surface"), 0xff12121d), // SchemeContentTests.swift testDarkTheme_MaxContrast_Surface
    (Expressive, 0xff0000ff, false, 0.0, Key("primary"), 0xff35855f), // SchemeExpressiveTests.swift testKeyColors
    (Expressive, 0xff0000ff, false, 0.0, Key("secondary"), 0xff8c6d8c), // SchemeExpressiveTests.swift testKeyColors
    (Expressive, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff806ea1), // SchemeExpressiveTests.swift testKeyColors
    (Expressive, 0xff0000ff, false, 0.0, Key("neutral"), 0xff79757f), // SchemeExpressiveTests.swift testKeyColors
    (Expressive, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff7a7585), // SchemeExpressiveTests.swift testKeyColors
    (Expressive, 0xff0000ff, false, -1.0, Field("primary"), 0xff32835d), // SchemeExpressiveTests.swift testLightTheme_minContrast_primary
    (Expressive, 0xff0000ff, false, 0.0, Field("primary"), 0xff146c48), // SchemeExpressiveTests.swift testLightTheme_standardContrast_primary
    (Expressive, 0xff0000ff, false, 1.0, Field("primary"), 0xff00341f), // SchemeExpressiveTests.swift testLightTheme_maxContrast_primary
    (Expressive, 0xff0000ff, false, -1.0, Field("primary_container"), 0xff99eabd), // SchemeExpressiveTests.swift testLightTheme_minContrast_primaryContainer
    (Expressive, 0xff0000ff, false, 0.0, Field("primary_container"), 0xffa2f4c6), // SchemeExpressiveTests.swift testLightTheme_standardContrast_primaryContainer
    (Expressive, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff005436), // SchemeExpressiveTests.swift testLightTheme_maxContrast_primaryContainer
    (Expressive, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff388862), // SchemeExpressiveTests.swift testLightTheme_minContrast_onPrimaryContainer
    (Expressive, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xff005234), // SchemeExpressiveTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (Expressive, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeExpressiveTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (Expressive, 0xff0000ff, false, -1.0, Field("surface"), 0xfffdf7ff), // SchemeExpressiveTests.swift testLightTheme_minContrast_surface
    (Expressive, 0xff0000ff, false, 0.0, Field("surface"), 0xfffdf7ff), // SchemeExpressiveTests.swift testLightTheme_standardContrast_surface
    (Expressive, 0xff0000ff, false, 1.0, Field("surface"), 0xfffdf7ff), // SchemeExpressiveTests.swift testLightTheme_maxContrast_surface
    (Expressive, 0xff0000ff, true, -1.0, Field("primary"), 0xff51a078), // SchemeExpressiveTests.swift testDarkTheme_minContrast_primary
    (Expressive, 0xff0000ff, true, 0.0, Field("primary"), 0xff87d7ab), // SchemeExpressiveTests.swift testDarkTheme_standardContrast_primary
    (Expressive, 0xff0000ff, true, 1.0, Field("primary"), 0xffbbffd7), // SchemeExpressiveTests.swift testDarkTheme_maxContrast_primary
    (Expressive, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff00432a), // SchemeExpressiveTests.swift testDarkTheme_minContrast_primaryContainer
    (Expressive, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff005234), // SchemeExpressiveTests.swift testDarkTheme_standardContrast_primaryContainer
    (Expressive, 0xff0000ff, true, 1.0, Field("primary_container"), 0xff83d3a8), // SchemeExpressiveTests.swift testDarkTheme_maxContrast_primaryContainer
    (Expressive, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff43936c), // SchemeExpressiveTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (Expressive, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffa2f4c6), // SchemeExpressiveTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (Expressive, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff000e06), // SchemeExpressiveTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (Expressive, 0xff0000ff, true, -1.0, Field("surface"), 0xff14121a), // SchemeExpressiveTests.swift testDarkTheme_minContrast_surface
    (Expressive, 0xff0000ff, true, 0.0, Field("surface"), 0xff14121a), // SchemeExpressiveTests.swift testDarkTheme_standardContrast_surface
    (Expressive, 0xff0000ff, true, 1.0, Field("surface"), 0xff14121a), // SchemeExpressiveTests.swift testDarkTheme_maxContrast_surface
    (Fidelity, 0xff0000ff, false, 0.0, Key("primary"), 0xff080cff), // SchemeFidelityTests.swift testKeyColors
    (Fidelity, 0xff0000ff, false, 0.0, Key("secondary"), 0xff656dd3), // SchemeFidelityTests.swift testKeyColors
    (Fidelity, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff9d0002), // SchemeFidelityTests.swift testKeyColors
    (Fidelity, 0xff0000ff, false, 0.0, Key("neutral"), 0xff767684), // SchemeFidelityTests.swift testKeyColors
    (Fidelity, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff757589), // SchemeFidelityTests.swift testKeyColors
    (Fidelity, 0xff0000ff, false, -1.0, Field("primary"), 0xff5660ff), // SchemeFidelityTests.swift testLightTheme_minContrast_primary
    (Fidelity, 0xff0000ff, false, 0.0, Field("primary"), 0xff0001bb), // SchemeFidelityTests.swift testLightTheme_standardContrast_primary
    (Fidelity, 0xff0000ff, false, 1.0, Field("primary"), 0xff00019f), // SchemeFidelityTests.swift testLightTheme_maxContrast_primary
    (Fidelity, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffd5d6ff), // SchemeFidelityTests.swift testLightTheme_minContrast_primaryContainer
    (Fidelity, 0xff0000ff, false, 0.0, Field("primary_container"), 0xff0000ff), // SchemeFidelityTests.swift testLightTheme_standardContrast_primaryContainer
    (Fidelity, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff0000f6), // SchemeFidelityTests.swift testLightTheme_maxContrast_primaryContainer
    (Fidelity, 0xff0000ff, false, -1.0, Field("tertiary_container"), 0xffffcdc6), // SchemeFidelityTests.swift testLightTheme_minContrast_tertiaryContainer
    (Fidelity, 0xff0000ff, false, 0.0, Field("tertiary_container"), 0xff9d0002), // SchemeFidelityTests.swift testLightTheme_standardContrast_tertiaryContainer
    (Fidelity, 0xff0000ff, false, 1.0, Field("tertiary_container"), 0xff980002), // SchemeFidelityTests.swift testLightTheme_maxContrast_tertiaryContainer
    (Fidelity, 0xff850096, false, -1.0, Field("tertiary_container"), 0xffebd982), // SchemeFidelityTests.swift testLightTheme_minContrast_objectionableTertiaryContainerLightens
    (Fidelity, 0xff850096, false, 0.0, Field("tertiary_container"), 0xffbcac5a), // SchemeFidelityTests.swift testLightTheme_standardContrast_objectionableTertiaryContainerLightens
    (Fidelity, 0xff850096, false, 1.0, Field("tertiary_container"), 0xff544900), // SchemeFidelityTests.swift testLightTheme_maxContrast_objectionableTertiaryContainerDarkens
    (Fidelity, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff5e68ff), // SchemeFidelityTests.swift testLightTheme_minContrast_onPrimaryContainer
    (Fidelity, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xffb3b7ff), // SchemeFidelityTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (Fidelity, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeFidelityTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (Fidelity, 0xff0000ff, false, -1.0, Field("surface"), 0xfffbf8ff), // SchemeFidelityTests.swift testLightTheme_minContrast_surface
    (Fidelity, 0xff0000ff, false, 0.0, Field("surface"), 0xfffbf8ff), // SchemeFidelityTests.swift testLightTheme_standardContrast_surface
    (Fidelity, 0xff0000ff, false, 1.0, Field("surface"), 0xfffbf8ff), // SchemeFidelityTests.swift testLightTheme_maxContrast_surface
    (Fidelity, 0xff0000ff, true, -1.0, Field("primary"), 0xff7c84ff), // SchemeFidelityTests.swift testDarkTheme_minContrast_primary
    (Fidelity, 0xff0000ff, true, 0.0, Field("primary"), 0xffbec2ff), // SchemeFidelityTests.swift testDarkTheme_standardContrast_primary
    (Fidelity, 0xff0000ff, true, 1.0, Field("primary"), 0xfff0eeff), // SchemeFidelityTests.swift testDarkTheme_maxContrast_primary
    (Fidelity, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff0001c9), // SchemeFidelityTests.swift testDarkTheme_minContrast_primaryContainer
    (Fidelity, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff0000ff), // SchemeFidelityTests.swift testDarkTheme_standardContrast_primaryContainer
    (Fidelity, 0xff0000ff, true, 1.0, Field("primary_container"), 0xffbabdff), // SchemeFidelityTests.swift testDarkTheme_maxContrast_primaryContainer
    (Fidelity, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff6b75ff), // SchemeFidelityTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (Fidelity, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffb3b7ff), // SchemeFidelityTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (Fidelity, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff00003d), // SchemeFidelityTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (Fidelity, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xffef4635), // SchemeFidelityTests.swift testDarkTheme_minContrast_onTertiaryContainer
    (Fidelity, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xffffa598), // SchemeFidelityTests.swift testDarkTheme_standardContrast_onTertiaryContainer
    (Fidelity, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff220000), // SchemeFidelityTests.swift testDarkTheme_maxContrast_onTertiaryContainer
    (Fidelity, 0xff0000ff, true, -1.0, Field("surface"), 0xff12121d), // SchemeFidelityTests.swift testDarkTheme_minContrast_surface
    (Fidelity, 0xff0000ff, true, 0.0, Field("surface"), 0xff12121d), // SchemeFidelityTests.swift testDarkTheme_standardContrast_surface
    (Fidelity, 0xff0000ff, true, 1.0, Field("surface"), 0xff12121d), // SchemeFidelityTests.swift testDarkTheme_maxContrast_surface
    (FruitSalad, 0xff0000ff, false, 0.0, Key("primary"), 0xff0393c3), // SchemeFruitSaladTests.swift testKeyColors
    (FruitSalad, 0xff0000ff, false, 0.0, Key("secondary"), 0xff3a7e9e), // SchemeFruitSaladTests.swift testKeyColors
    (FruitSalad, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff6e72ac), // SchemeFruitSaladTests.swift testKeyColors
    (FruitSalad, 0xff0000ff, false, 0.0, Key("neutral"), 0xff777682), // SchemeFruitSaladTests.swift testKeyColors
    (FruitSalad, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff75758b), // SchemeFruitSaladTests.swift testKeyColors
    (FruitSalad, 0xff0000ff, false, -1.0, Field("primary"), 0xff007ea7), // SchemeFruitSaladTests.swift testLightTheme_minContrast_primary
    (FruitSalad, 0xff0000ff, false, 0.0, Field("primary"), 0xff006688), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_primary
    (FruitSalad, 0xff0000ff, false, 1.0, Field("primary"), 0xff003042), // SchemeFruitSaladTests.swift testLightTheme_maxContrast_primary
    (FruitSalad, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffaae0ff), // SchemeFruitSaladTests.swift testLightTheme_minContrast_primaryContainer
    (FruitSalad, 0xff0000ff, false, 0.0, Field("primary_container"), 0xffc2e8ff), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_primaryContainer
    (FruitSalad, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff004f6b), // SchemeFruitSaladTests.swift testLightTheme_maxContrast_primaryContainer
    (FruitSalad, 0xff0000ff, false, -1.0, Field("tertiary_container"), 0xffd5d6ff), // SchemeFruitSaladTests.swift testLightTheme_minContrast_tertiaryContainer
    (FruitSalad, 0xff0000ff, false, 0.0, Field("tertiary_container"), 0xffe0e0ff), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_tertiaryContainer
    (FruitSalad, 0xff0000ff, false, 1.0, Field("tertiary_container"), 0xff40447b), // SchemeFruitSaladTests.swift testLightTheme_maxContrast_tertiaryContainer
    (FruitSalad, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff0083ae), // SchemeFruitSaladTests.swift testLightTheme_minContrast_onPrimaryContainer
    (FruitSalad, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xff004d67), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (FruitSalad, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeFruitSaladTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (FruitSalad, 0xff0000ff, false, -1.0, Field("surface"), 0xfffbf8ff), // SchemeFruitSaladTests.swift testLightTheme_minContrast_surface
    (FruitSalad, 0xff0000ff, false, 0.0, Field("surface"), 0xfffbf8ff), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_surface
    (FruitSalad, 0xff0000ff, false, 1.0, Field("surface"), 0xfffbf8ff), // SchemeFruitSaladTests.swift testLightTheme_maxContrast_surface
    (FruitSalad, 0xff0000ff, false, 0.0, Field("secondary"), 0xff196584), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_secondary
    (FruitSalad, 0xff0000ff, false, 0.0, Field("secondary_container"), 0xffc2e8ff), // SchemeFruitSaladTests.swift testLightTheme_standardContrast_secondaryContainer
    (FruitSalad, 0xff0000ff, true, -1.0, Field("primary"), 0xff1e9bcb), // SchemeFruitSaladTests.swift testDarkTheme_minContrast_primary
    (FruitSalad, 0xff0000ff, true, 0.0, Field("primary"), 0xff76d1ff), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_primary
    (FruitSalad, 0xff0000ff, true, 1.0, Field("primary"), 0xffe0f3ff), // SchemeFruitSaladTests.swift testDarkTheme_maxContrast_primary
    (FruitSalad, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff003f56), // SchemeFruitSaladTests.swift testDarkTheme_minContrast_primaryContainer
    (FruitSalad, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff004d67), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_primaryContainer
    (FruitSalad, 0xff0000ff, true, 1.0, Field("primary_container"), 0xff68ceff), // SchemeFruitSaladTests.swift testDarkTheme_maxContrast_primaryContainer
    (FruitSalad, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff008ebc), // SchemeFruitSaladTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (FruitSalad, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffc2e8ff), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (FruitSalad, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff000d15), // SchemeFruitSaladTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (FruitSalad, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xff7b7fbb), // SchemeFruitSaladTests.swift testDarkTheme_minContrast_onTertiaryContainer
    (FruitSalad, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xffe0e0ff), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_onTertiaryContainer
    (FruitSalad, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff00003c), // SchemeFruitSaladTests.swift testDarkTheme_maxContrast_onTertiaryContainer
    (FruitSalad, 0xff0000ff, true, -1.0, Field("surface"), 0xff12131c), // SchemeFruitSaladTests.swift testDarkTheme_minContrast_surface
    (FruitSalad, 0xff0000ff, true, 0.0, Field("surface"), 0xff12131c), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_surface
    (FruitSalad, 0xff0000ff, true, 1.0, Field("surface"), 0xff12131c), // SchemeFruitSaladTests.swift testDarkTheme_maxContrast_surface
    (FruitSalad, 0xff0000ff, true, 0.0, Field("secondary"), 0xff8ecff2), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_secondary
    (FruitSalad, 0xff0000ff, true, 0.0, Field("secondary_container"), 0xff004d67), // SchemeFruitSaladTests.swift testDarkTheme_standardContrast_secondaryContainer
    (Monochrome, 0xff0000ff, false, 0.0, Key("primary"), 0xff777777), // SchemeMonochromeTests.swift testKeyColors
    (Monochrome, 0xff0000ff, false, 0.0, Key("secondary"), 0xff777777), // SchemeMonochromeTests.swift testKeyColors
    (Monochrome, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff777777), // SchemeMonochromeTests.swift testKeyColors
    (Monochrome, 0xff0000ff, false, 0.0, Key("neutral"), 0xff777777), // SchemeMonochromeTests.swift testKeyColors
    (Monochrome, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff777777), // SchemeMonochromeTests.swift testKeyColors
    (Monochrome, 0xff0000ff, false, -1.0, Field("primary"), 0xff747474), // SchemeMonochromeTests.swift testLightTheme_minContrast_primary
    (Monochrome, 0xff0000ff, false, 0.0, Field("primary"), 0xff000000), // SchemeMonochromeTests.swift testLightTheme_standardContrast_primary
    (Monochrome, 0xff0000ff, false, 1.0, Field("primary"), 0xff000000), // SchemeMonochromeTests.swift testLightTheme_maxContrast_primary
    (Monochrome, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffd9d9d9), // SchemeMonochromeTests.swift testLightTheme_minContrast_primaryContainer
    (Monochrome, 0xff0000ff, false, 0.0, Field("primary_container"), 0xff3b3b3b), // SchemeMonochromeTests.swift testLightTheme_standardContrast_primaryContainer
    (Monochrome, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff3b3b3b), // SchemeMonochromeTests.swift testLightTheme_maxContrast_primaryContainer
    (Monochrome, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff7a7a7a), // SchemeMonochromeTests.swift testLightTheme_minContrast_onPrimaryContainer
    (Monochrome, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xffffffff), // SchemeMonochromeTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (Monochrome, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeMonochromeTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (Monochrome, 0xff0000ff, false, -1.0, Field("surface"), 0xfff9f9f9), // SchemeMonochromeTests.swift testLightTheme_minContrast_surface
    (Monochrome, 0xff0000ff, false, 0.0, Field("surface"), 0xfff9f9f9), // SchemeMonochromeTests.swift testLightTheme_standardContrast_surface
    (Monochrome, 0xff0000ff, false, 1.0, Field("surface"), 0xfff9f9f9), // SchemeMonochromeTests.swift testLightTheme_maxContrast_surface
    (Monochrome, 0xff0000ff, true, -1.0, Field("primary"), 0xff919191), // SchemeMonochromeTests.swift testDarkTheme_minContrast_primary
    (Monochrome, 0xff0000ff, true, 0.0, Field("primary"), 0xffffffff), // SchemeMonochromeTests.swift testDarkTheme_standardContrast_primary
    (Monochrome, 0xff0000ff, true, 1.0, Field("primary"), 0xffffffff), // SchemeMonochromeTests.swift testDarkTheme_maxContrast_primary
    (Monochrome, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff3a3a3a), // SchemeMonochromeTests.swift testDarkTheme_minContrast_primaryContainer
    (Monochrome, 0xff0000ff, true, 0.0, Field("primary_container"), 0xffd4d4d4), // SchemeMonochromeTests.swift testDarkTheme_standardContrast_primaryContainer
    (Monochrome, 0xff0000ff, true, 1.0, Field("primary_container"), 0xffd4d4d4), // SchemeMonochromeTests.swift testDarkTheme_maxContrast_primaryContainer
    (Monochrome, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff848484), // SchemeMonochromeTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (Monochrome, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xff000000), // SchemeMonochromeTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (Monochrome, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff000000), // SchemeMonochromeTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (Monochrome, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xff848484), // SchemeMonochromeTests.swift testDarkTheme_minContrast_onTertiaryContainer
    (Monochrome, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xff000000), // SchemeMonochromeTests.swift testDarkTheme_standardContrast_onTertiaryContainer
    (Monochrome, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff000000), // SchemeMonochromeTests.swift testDarkTheme_maxContrast_onTertiaryContainer
    (Monochrome, 0xff0000ff, true, -1.0, Field("surface"), 0xff131313), // SchemeMonochromeTests.swift testDarkTheme_minContrast_surface
    (Monochrome, 0xff0000ff, true, 0.0, Field("surface"), 0xff131313), // SchemeMonochromeTests.swift testDarkTheme_standardContrast_surface
    (Monochrome, 0xff0000ff, true, 1.0, Field("surface"), 0xff131313), // SchemeMonochromeTests.swift testDarkTheme_maxContrast_surface
    (Neutral, 0xff0000ff, false, 0.0, Key("primary"), 0xff767685), // SchemeNeutralTests.swift testKeyColors
    (Neutral, 0xff0000ff, false, 0.0, Key("secondary"), 0xff777680), // SchemeNeutralTests.swift testKeyColors
    (Neutral, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff75758b), // SchemeNeutralTests.swift testKeyColors
    (Neutral, 0xff0000ff, false, 0.0, Key("neutral"), 0xff787678), // SchemeNeutralTests.swift testKeyColors
    (Neutral, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff787678), // SchemeNeutralTests.swift testKeyColors
    (Neutral, 0xff0000ff, false, -1.0, Field("primary"), 0xff737383), // SchemeNeutralTests.swift testLightTheme_minContrast_primary
    (Neutral, 0xff0000ff, false, 0.0, Field("primary"), 0xff5d5d6c), // SchemeNeutralTests.swift testLightTheme_standardContrast_primary
    (Neutral, 0xff0000ff, false, 1.0, Field("primary"), 0xff2b2b38), // SchemeNeutralTests.swift testLightTheme_maxContrast_primary
    (Neutral, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffd9d7e9), // SchemeNeutralTests.swift testLightTheme_minContrast_primaryContainer
    (Neutral, 0xff0000ff, false, 0.0, Field("primary_container"), 0xffe2e1f3), // SchemeNeutralTests.swift testLightTheme_standardContrast_primaryContainer
    (Neutral, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff484856), // SchemeNeutralTests.swift testLightTheme_maxContrast_primaryContainer
    (Neutral, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff797888), // SchemeNeutralTests.swift testLightTheme_minContrast_onPrimaryContainer
    (Neutral, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xff454654), // SchemeNeutralTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (Neutral, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeNeutralTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (Neutral, 0xff0000ff, false, -1.0, Field("surface"), 0xfffcf8fa), // SchemeNeutralTests.swift testLightTheme_minContrast_surface
    (Neutral, 0xff0000ff, false, 0.0, Field("surface"), 0xfffcf8fa), // SchemeNeutralTests.swift testLightTheme_standardContrast_surface
    (Neutral, 0xff0000ff, false, 1.0, Field("surface"), 0xfffcf8fa), // SchemeNeutralTests.swift testLightTheme_maxContrast_surface
    (Neutral, 0xff0000ff, true, -1.0, Field("primary"), 0xff908f9f), // SchemeNeutralTests.swift testDarkTheme_minContrast_primary
    (Neutral, 0xff0000ff, true, 0.0, Field("primary"), 0xffc6c5d6), // SchemeNeutralTests.swift testDarkTheme_standardContrast_primary
    (Neutral, 0xff0000ff, true, 1.0, Field("primary"), 0xfff0eeff), // SchemeNeutralTests.swift testDarkTheme_maxContrast_primary
    (Neutral, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff393947), // SchemeNeutralTests.swift testDarkTheme_minContrast_primaryContainer
    (Neutral, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff454654), // SchemeNeutralTests.swift testDarkTheme_standardContrast_primaryContainer
    (Neutral, 0xff0000ff, true, 1.0, Field("primary_container"), 0xffc2c1d2), // SchemeNeutralTests.swift testDarkTheme_maxContrast_primaryContainer
    (Neutral, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff838393), // SchemeNeutralTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (Neutral, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffe2e1f3), // SchemeNeutralTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (Neutral, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff090a16), // SchemeNeutralTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (Neutral, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xff828299), // SchemeNeutralTests.swift testDarkTheme_minContrast_onTertiaryContainer
    (Neutral, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xffe1e0f9), // SchemeNeutralTests.swift testDarkTheme_standardContrast_onTertiaryContainer
    (Neutral, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff080a1b), // SchemeNeutralTests.swift testDarkTheme_maxContrast_onTertiaryContainer
    (Neutral, 0xff0000ff, true, -1.0, Field("surface"), 0xff131315), // SchemeNeutralTests.swift testDarkTheme_minContrast_surface
    (Neutral, 0xff0000ff, true, 0.0, Field("surface"), 0xff131315), // SchemeNeutralTests.swift testDarkTheme_standardContrast_surface
    (Neutral, 0xff0000ff, true, 1.0, Field("surface"), 0xff131315), // SchemeNeutralTests.swift testDarkTheme_maxContrast_surface
    (Rainbow, 0xff0000ff, false, 0.0, Key("primary"), 0xff696fc4), // SchemeRainbowTests.swift testKeyColors
    (Rainbow, 0xff0000ff, false, 0.0, Key("secondary"), 0xff75758b), // SchemeRainbowTests.swift testKeyColors
    (Rainbow, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff936b84), // SchemeRainbowTests.swift testKeyColors
    (Rainbow, 0xff0000ff, false, 0.0, Key("neutral"), 0xff777777), // SchemeRainbowTests.swift testKeyColors
    (Rainbow, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff777777), // SchemeRainbowTests.swift testKeyColors
    (Rainbow, 0xff0000ff, false, -1.0, Field("primary"), 0xff676dc1), // SchemeRainbowTests.swift testLightTheme_minContrast_primary
    (Rainbow, 0xff0000ff, false, 0.0, Field("primary"), 0xff5056a9), // SchemeRainbowTests.swift testLightTheme_standardContrast_primary
    (Rainbow, 0xff0000ff, false, 1.0, Field("primary"), 0xff1b2074), // SchemeRainbowTests.swift testLightTheme_maxContrast_primary
    (Rainbow, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffd5d6ff), // SchemeRainbowTests.swift testLightTheme_minContrast_primaryContainer
    (Rainbow, 0xff0000ff, false, 0.0, Field("primary_container"), 0xffe0e0ff), // SchemeRainbowTests.swift testLightTheme_standardContrast_primaryContainer
    (Rainbow, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff3a4092), // SchemeRainbowTests.swift testLightTheme_maxContrast_primaryContainer
    (Rainbow, 0xff0000ff, false, -1.0, Field("tertiary_container"), 0xfffbcbe7), // SchemeRainbowTests.swift testLightTheme_minContrast_tertiaryContainer
    (Rainbow, 0xff0000ff, false, 0.0, Field("tertiary_container"), 0xffffd8ee), // SchemeRainbowTests.swift testLightTheme_standardContrast_tertiaryContainer
    (Rainbow, 0xff0000ff, false, 1.0, Field("tertiary_container"), 0xff613e55), // SchemeRainbowTests.swift testLightTheme_maxContrast_tertiaryContainer
    (Rainbow, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff6c72c7), // SchemeRainbowTests.swift testLightTheme_minContrast_onPrimaryContainer
    (Rainbow, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xff383e8f), // SchemeRainbowTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (Rainbow, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeRainbowTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (Rainbow, 0xff0000ff, false, -1.0, Field("surface"), 0xfff9f9f9), // SchemeRainbowTests.swift testLightTheme_minContrast_surface
    (Rainbow, 0xff0000ff, false, 0.0, Field("surface"), 0xfff9f9f9), // SchemeRainbowTests.swift testLightTheme_standardContrast_surface
    (Rainbow, 0xff0000ff, false, 1.0, Field("surface"), 0xfff9f9f9), // SchemeRainbowTests.swift testLightTheme_maxContrast_surface
    (Rainbow, 0xff0000ff, false, 0.0, Field("secondary"), 0xff5c5d72), // SchemeRainbowTests.swift testLightTheme_standardContrast_secondary
    (Rainbow, 0xff0000ff, false, 0.0, Field("secondary_container"), 0xffe1e0f9), // SchemeRainbowTests.swift testLightTheme_standardContrast_secondaryContainer
    (Rainbow, 0xff0000ff, true, -1.0, Field("primary"), 0xff8389e0), // SchemeRainbowTests.swift testDarkTheme_minContrast_primary
    (Rainbow, 0xff0000ff, true, 0.0, Field("primary"), 0xffbec2ff), // SchemeRainbowTests.swift testDarkTheme_standardContrast_primary
    (Rainbow, 0xff0000ff, true, 1.0, Field("primary"), 0xfff0eeff), // SchemeRainbowTests.swift testDarkTheme_maxContrast_primary
    (Rainbow, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff2a3082), // SchemeRainbowTests.swift testDarkTheme_minContrast_primaryContainer
    (Rainbow, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff383e8f), // SchemeRainbowTests.swift testDarkTheme_standardContrast_primaryContainer
    (Rainbow, 0xff0000ff, true, 1.0, Field("primary_container"), 0xffbabdff), // SchemeRainbowTests.swift testDarkTheme_maxContrast_primaryContainer
    (Rainbow, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff767cd2), // SchemeRainbowTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (Rainbow, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffe0e0ff), // SchemeRainbowTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (Rainbow, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff00003d), // SchemeRainbowTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (Rainbow, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xffa17891), // SchemeRainbowTests.swift testDarkTheme_minContrast_onTertiaryContainer
    (Rainbow, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xffffd8ee), // SchemeRainbowTests.swift testDarkTheme_standardContrast_onTertiaryContainer
    (Rainbow, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff1b0315), // SchemeRainbowTests.swift testDarkTheme_maxContrast_onTertiaryContainer
    (Rainbow, 0xff0000ff, true, -1.0, Field("surface"), 0xff131313), // SchemeRainbowTests.swift testDarkTheme_minContrast_surface
    (Rainbow, 0xff0000ff, true, 0.0, Field("surface"), 0xff131313), // SchemeRainbowTests.swift testDarkTheme_standardContrast_surface
    (Rainbow, 0xff0000ff, true, 1.0, Field("surface"), 0xff131313), // SchemeRainbowTests.swift testDarkTheme_maxContrast_surface
    (Rainbow, 0xff0000ff, true, 0.0, Field("secondary"), 0xffc5c4dd), // SchemeRainbowTests.swift testDarkTheme_standardContrast_secondary
    (Rainbow, 0xff0000ff, true, 0.0, Field("secondary_container"), 0xff444559), // SchemeRainbowTests.swift testDarkTheme_standardContrast_secondaryContainer
    (TonalSpot, 0xff0000ff, false, 0.0, Field("primary"), 0xff555992), // SchemeTonalSpotTests.swift testBlueLightScheme
    (TonalSpot, 0xff0000ff, true, 0.0, Field("primary"), 0xffbec2ff), // SchemeTonalSpotTests.swift testBlueDarkScheme
    (TonalSpot, 0xff6750a4, false, 0.0, Field("primary"), 0xff65558f), // SchemeTonalSpotTests.swift test3rdPartyLightScheme
    (TonalSpot, 0xff6750a4, false, 0.0, Field("secondary"), 0xff625b71), // SchemeTonalSpotTests.swift test3rdPartyLightScheme
    (TonalSpot, 0xff6750a4, false, 0.0, Field("tertiary"), 0xff7e5260), // SchemeTonalSpotTests.swift test3rdPartyLightScheme
    (TonalSpot, 0xff6750a4, false, 0.0, Field("surface"), 0xfffdf7ff), // SchemeTonalSpotTests.swift test3rdPartyLightScheme
    (TonalSpot, 0xff6750a4, false, 0.0, Field("on_surface"), 0xff1d1b20), // SchemeTonalSpotTests.swift test3rdPartyLightScheme
    (TonalSpot, 0xff6750a4, true, 0.0, Field("primary"), 0xffcfbdfe), // SchemeTonalSpotTests.swift test3rdPartyDarkScheme
    (TonalSpot, 0xff6750a4, true, 0.0, Field("secondary"), 0xffcbc2db), // SchemeTonalSpotTests.swift test3rdPartyDarkScheme
    (TonalSpot, 0xff6750a4, true, 0.0, Field("tertiary"), 0xffefb8c8), // SchemeTonalSpotTests.swift test3rdPartyDarkScheme
    (TonalSpot, 0xff6750a4, true, 0.0, Field("surface"), 0xff141218), // SchemeTonalSpotTests.swift test3rdPartyDarkScheme
    (TonalSpot, 0xff6750a4, true, 0.0, Field("on_surface"), 0xffe6e0e9), // SchemeTonalSpotTests.swift test3rdPartyDarkScheme
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("primary"), 0xff814c77), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_primary"), 0xffffffff), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("primary_container"), 0xffffd7f3), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_primary_container"), 0xff67355e), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("secondary"), 0xff6e5868), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_secondary"), 0xffffffff), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("secondary_container"), 0xfff8daee), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_secondary_container"), 0xff554050), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("tertiary"), 0xff815343), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_tertiary"), 0xffffffff), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("tertiary_container"), 0xffffdbd0), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_tertiary_container"), 0xff663c2d), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("error"), 0xffba1a1a), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_error"), 0xffffffff), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("error_container"), 0xffffdad6), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_error_container"), 0xff93000a), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("surface"), 0xfffff7f9), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_surface"), 0xff201a1e), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("surface_variant"), 0xffeedee7), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("on_surface_variant"), 0xff4e444b), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("outline"), 0xff80747b), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("outline_variant"), 0xffd2c2cb), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("scrim"), 0xff000000), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("inverse_surface"), 0xff362e33), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("inverse_on_surface"), 0xfffaedf3), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, false, 0.0, Field("inverse_primary"), 0xfff3b2e4), // SchemeTonalSpotTests.swift testLightSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("primary"), 0xfff3b2e4), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_primary"), 0xff4d1f47), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("primary_container"), 0xff67355e), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_primary_container"), 0xffffd7f3), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("secondary"), 0xffdbbed1), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_secondary"), 0xff3e2a39), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("secondary_container"), 0xff554050), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_secondary_container"), 0xfff8daee), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("tertiary"), 0xfff5b9a5), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_tertiary"), 0xff4c2619), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("tertiary_container"), 0xff663c2d), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_tertiary_container"), 0xffffdbd0), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("error"), 0xffffb4ab), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_error"), 0xff690005), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("error_container"), 0xff93000a), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_error_container"), 0xffffdad6), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("surface"), 0xff181216), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_surface"), 0xffecdfe5), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("surface_variant"), 0xff4e444b), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("on_surface_variant"), 0xffd2c2cb), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("outline"), 0xff9a8d95), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("outline_variant"), 0xff4e444b), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("scrim"), 0xff000000), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("inverse_surface"), 0xffecdfe5), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("inverse_on_surface"), 0xff362e33), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (TonalSpot, 0xfffa2bec, true, 0.0, Field("inverse_primary"), 0xff814c77), // SchemeTonalSpotTests.swift testDarkSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("primary"), 0xffa7009e), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_primary"), 0xffffffff), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("primary_container"), 0xffd100c6), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_primary_container"), 0xfffffbff), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("secondary"), 0xff9e2d93), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_secondary"), 0xffffffff), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("secondary_container"), 0xffff83ec), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_secondary_container"), 0xff7c0276), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("tertiary"), 0xffb51830), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_tertiary"), 0xffffffff), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("tertiary_container"), 0xffd83546), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_tertiary_container"), 0xfffffbff), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("error"), 0xffba1a1a), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_error"), 0xffffffff), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("error_container"), 0xffffdad6), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_error_container"), 0xff93000a), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("surface"), 0xfffff7f9), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_surface"), 0xff251722), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("surface_variant"), 0xfff9daee), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("on_surface_variant"), 0xff564050), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("outline"), 0xff897081), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("outline_variant"), 0xffdcbed2), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("scrim"), 0xff000000), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("inverse_surface"), 0xff3b2c37), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("inverse_on_surface"), 0xffffebf6), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, false, 0.0, Field("inverse_primary"), 0xffffabee), // SchemeTonalSpotTests.swift testLightContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("primary"), 0xffffabee), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_primary"), 0xff5c0057), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("primary_container"), 0xfffa2bec), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_primary_container"), 0xff51004c), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("secondary"), 0xffffabee), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_secondary"), 0xff5c0057), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("secondary_container"), 0xff840f7c), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_secondary_container"), 0xffff93ec), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("tertiary"), 0xffffb3b2), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_tertiary"), 0xff680014), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("tertiary_container"), 0xffff525f), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_tertiary_container"), 0xff5b0010), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("error"), 0xffffb4ab), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_error"), 0xff690005), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("error_container"), 0xff93000a), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_error_container"), 0xffffdad6), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("surface"), 0xff1c0f19), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_surface"), 0xfff4dceb), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("surface_variant"), 0xff564050), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("on_surface_variant"), 0xffdcbed2), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("outline"), 0xffa4899b), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("outline_variant"), 0xff564050), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("scrim"), 0xff000000), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("inverse_surface"), 0xfff4dceb), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("inverse_on_surface"), 0xff3b2c37), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Content, 0xfffa2bec, true, 0.0, Field("inverse_primary"), 0xffab00a2), // SchemeTonalSpotTests.swift testDarkContentSchemeFromHighChromaColor
    (Vibrant, 0xff0000ff, false, 0.0, Key("primary"), 0xff080cff), // SchemeVibrantTests.swift testKeyColors
    (Vibrant, 0xff0000ff, false, 0.0, Key("secondary"), 0xff7b7296), // SchemeVibrantTests.swift testKeyColors
    (Vibrant, 0xff0000ff, false, 0.0, Key("tertiary"), 0xff886c9d), // SchemeVibrantTests.swift testKeyColors
    (Vibrant, 0xff0000ff, false, 0.0, Key("neutral"), 0xff777682), // SchemeVibrantTests.swift testKeyColors
    (Vibrant, 0xff0000ff, false, 0.0, Key("neutral_variant"), 0xff767685), // SchemeVibrantTests.swift testKeyColors
    (Vibrant, 0xff0000ff, false, -1.0, Field("primary"), 0xff5660ff), // SchemeVibrantTests.swift testLightTheme_minContrast_primary
    (Vibrant, 0xff0000ff, false, 0.0, Field("primary"), 0xff343dff), // SchemeVibrantTests.swift testLightTheme_standardContrast_primary
    (Vibrant, 0xff0000ff, false, 1.0, Field("primary"), 0xff00019f), // SchemeVibrantTests.swift testLightTheme_maxContrast_primary
    (Vibrant, 0xff0000ff, false, -1.0, Field("primary_container"), 0xffd5d6ff), // SchemeVibrantTests.swift testLightTheme_minContrast_primaryContainer
    (Vibrant, 0xff0000ff, false, 0.0, Field("primary_container"), 0xffe0e0ff), // SchemeVibrantTests.swift testLightTheme_standardContrast_primaryContainer
    (Vibrant, 0xff0000ff, false, 1.0, Field("primary_container"), 0xff0000f6), // SchemeVibrantTests.swift testLightTheme_maxContrast_primaryContainer
    (Vibrant, 0xff0000ff, false, -1.0, Field("on_primary_container"), 0xff5e68ff), // SchemeVibrantTests.swift testLightTheme_minContrast_onPrimaryContainer
    (Vibrant, 0xff0000ff, false, 0.0, Field("on_primary_container"), 0xff0000ef), // SchemeVibrantTests.swift testLightTheme_standardContrast_onPrimaryContainer
    (Vibrant, 0xff0000ff, false, 1.0, Field("on_primary_container"), 0xffffffff), // SchemeVibrantTests.swift testLightTheme_maxContrast_onPrimaryContainer
    (Vibrant, 0xff0000ff, false, -1.0, Field("surface"), 0xfffbf8ff), // SchemeVibrantTests.swift testLightTheme_minContrast_surface
    (Vibrant, 0xff0000ff, false, 0.0, Field("surface"), 0xfffbf8ff), // SchemeVibrantTests.swift testLightTheme_standardContrast_surface
    (Vibrant, 0xff0000ff, false, 1.0, Field("surface"), 0xfffbf8ff), // SchemeVibrantTests.swift testLightTheme_maxContrast_surface
    (Vibrant, 0xff0000ff, true, -1.0, Field("primary"), 0xff7c84ff), // SchemeVibrantTests.swift testDarkTheme_minContrast_primary
    (Vibrant, 0xff0000ff, true, 0.0, Field("primary"), 0xffbec2ff), // SchemeVibrantTests.swift testDarkTheme_standardContrast_primary
    (Vibrant, 0xff0000ff, true, 1.0, Field("primary"), 0xfff0eeff), // SchemeVibrantTests.swift testDarkTheme_maxContrast_primary
    (Vibrant, 0xff0000ff, true, -1.0, Field("primary_container"), 0xff0001c9), // SchemeVibrantTests.swift testDarkTheme_minContrast_primaryContainer
    (Vibrant, 0xff0000ff, true, 0.0, Field("primary_container"), 0xff0000ef), // SchemeVibrantTests.swift testDarkTheme_standardContrast_primaryContainer
    (Vibrant, 0xff0000ff, true, 1.0, Field("primary_container"), 0xffbabdff), // SchemeVibrantTests.swift testDarkTheme_maxContrast_primaryContainer
    (Vibrant, 0xff0000ff, true, -1.0, Field("on_primary_container"), 0xff6b75ff), // SchemeVibrantTests.swift testDarkTheme_minContrast_onPrimaryContainer
    (Vibrant, 0xff0000ff, true, 0.0, Field("on_primary_container"), 0xffe0e0ff), // SchemeVibrantTests.swift testDarkTheme_standardContrast_onPrimaryContainer
    (Vibrant, 0xff0000ff, true, 1.0, Field("on_primary_container"), 0xff00003d), // SchemeVibrantTests.swift testDarkTheme_maxContrast_onPrimaryContainer
    (Vibrant, 0xff0000ff, true, -1.0, Field("on_tertiary_container"), 0xff9679ab), // SchemeVibrantTests.swift testDarkTheme_minContrast_onTertiaryContainer
    (Vibrant, 0xff0000ff, true, 0.0, Field("on_tertiary_container"), 0xfff2daff), // SchemeVibrantTests.swift testDarkTheme_standardContrast_onTertiaryContainer
    (Vibrant, 0xff0000ff, true, 1.0, Field("on_tertiary_container"), 0xff16002a), // SchemeVibrantTests.swift testDarkTheme_maxContrast_onTertiaryContainer
    (Vibrant, 0xff0000ff, true, -1.0, Field("surface"), 0xff12131c), // SchemeVibrantTests.swift testDarkTheme_minContrast_surface
    (Vibrant, 0xff0000ff, true, 0.0, Field("surface"), 0xff12131c), // SchemeVibrantTests.swift testDarkTheme_standardContrast_surface
    (Vibrant, 0xff0000ff, true, 1.0, Field("surface"), 0xff12131c), // SchemeVibrantTests.swift testDarkTheme_maxContrast_surface
    ];
    let mut wrong = Vec::new();
    for (variant, seed, dark, contrast, expect, expected) in cases {
        let s = DynamicScheme::new(Hct::from_argb(seed), variant, dark, contrast);
        let (name, actual) = match expect {
            Field(name) => (name, field(&s, name)),
            Key(name) => (name, key(&s, name)),
        };
        if actual != expected {
            wrong.push(format!(
                "{variant:?} {seed:08x} dark {dark} contrast {contrast} {name}: \
                 {actual:08x}, not {expected:08x}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// dart/test/dynamic_color_test.dart, "Values are correct"

#[test]
fn values_from_the_dart_tests() {
    use SchemeVariant::*;
    let argb = |variant, seed, dark, contrast, role| {
        DynamicScheme::new(Hct::from_argb(seed), variant, dark, contrast).argb(role)
    };
    assert_eq!(
        argb(Fidelity, 0xffff0000, false, 0.5, Role::OnPrimaryContainer),
        0xffffffff
    );
    assert_eq!(
        argb(Content, 0xff0000ff, false, 0.5, Role::OnSecondaryContainer),
        0xffffffff
    );
    assert_eq!(
        argb(Content, 0xffffff00, true, -0.5, Role::OnTertiaryContainer),
        0xff959b1a
    );
    assert_eq!(
        argb(Content, 0xff0000ff, false, 0.0, Role::InverseSurface),
        0xff2f2f3b
    );
    assert_eq!(
        argb(Content, 0xffff0000, false, -0.5, Role::InversePrimary),
        0xffff422f
    );
    assert_eq!(
        argb(Content, 0xffffff00, true, 0.0, Role::OutlineVariant),
        0xff484831
    );
}

// dynamiccolor/dynamic_color_test.ts, for the roles here.

fn all_schemes() -> Vec<DynamicScheme> {
    use SchemeVariant::*;
    let mut schemes = Vec::new();
    for seed in [0xffff0000, 0xffffff00, 0xff00ff00, 0xff0000ff] {
        for contrast in [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0] {
            for dark in [false, true] {
                for variant in [
                    Content, Expressive, Fidelity, Monochrome, Neutral, TonalSpot, Vibrant,
                ] {
                    schemes.push(DynamicScheme::new(
                        Hct::from_argb(seed),
                        variant,
                        dark,
                        contrast,
                    ));
                }
            }
        }
    }
    schemes
}

#[test]
fn colors_respect_contrast() {
    use Role::*;
    let pairs = [
        (OnPrimary, Primary),
        (OnPrimaryContainer, PrimaryContainer),
        (OnSecondary, Secondary),
        (OnSecondaryContainer, SecondaryContainer),
        (OnTertiary, Tertiary),
        (OnTertiaryContainer, TertiaryContainer),
        (OnError, Error),
        (OnErrorContainer, ErrorContainer),
        (OnSurfaceVariant, SurfaceBright),
        (OnSurfaceVariant, SurfaceDim),
    ];
    for s in all_schemes() {
        for (fg, bg) in pairs {
            let tone = |role| utils::lstar_from_argb(s.argb(role));
            let ratio = contrast::ratio_of_tones(tone(fg), tone(bg));
            let minimum = if s.contrast >= 0.0 { 4.5 } else { 3.0 };
            assert!(ratio >= minimum, "{fg:?} on {bg:?} is {ratio}");
        }
    }
}

#[test]
fn constraints_are_met() {
    use Role::*;
    fn min_requirement(c: ContrastCurve, level: f64) -> f64 {
        if level >= 1.0 {
            c.high
        } else if level >= 0.5 {
            c.medium
        } else if level >= 0.0 {
            c.normal
        } else {
            c.low
        }
    }
    let surfaces: &[Role] = &[SurfaceDim, SurfaceBright];
    let contrasts: &[(ContrastCurve, &[Role], &[Role], bool)] = &[
        (curve(4.5, 7.0, 11.0, 21.0), &[OnSurface], surfaces, false),
        (
            curve(3.0, 4.5, 7.0, 11.0),
            &[OnSurfaceVariant],
            surfaces,
            false,
        ),
        (
            curve(3.0, 4.5, 7.0, 7.0),
            &[Primary, Secondary, Tertiary, Error],
            surfaces,
            false,
        ),
        (curve(1.5, 3.0, 4.5, 7.0), &[Outline], surfaces, false),
        (
            curve(0.0, 0.0, 3.0, 4.5),
            &[
                PrimaryContainer,
                SecondaryContainer,
                TertiaryContainer,
                ErrorContainer,
                OutlineVariant,
            ],
            surfaces,
            false,
        ),
        (
            curve(4.5, 7.0, 11.0, 21.0),
            &[InverseOnSurface],
            &[InverseSurface],
            false,
        ),
        (
            curve(3.0, 4.5, 7.0, 7.0),
            &[InversePrimary],
            &[InverseSurface],
            false,
        ),
        (
            curve(4.5, 7.0, 11.0, 21.0),
            &[OnPrimary, OnSecondary, OnTertiary, OnError],
            &[Primary, Secondary, Tertiary, Error],
            true,
        ),
        (
            curve(3.0, 4.5, 7.0, 11.0),
            &[
                OnPrimaryContainer,
                OnSecondaryContainer,
                OnTertiaryContainer,
                OnErrorContainer,
            ],
            &[
                PrimaryContainer,
                SecondaryContainer,
                TertiaryContainer,
                ErrorContainer,
            ],
            true,
        ),
    ];
    let backgrounds = [
        Error,
        ErrorContainer,
        Primary,
        PrimaryContainer,
        Secondary,
        SecondaryContainer,
        Surface,
        SurfaceBright,
        SurfaceContainer,
        SurfaceContainerHigh,
        SurfaceContainerHighest,
        SurfaceContainerLow,
        SurfaceContainerLowest,
        SurfaceDim,
        SurfaceVariant,
        Tertiary,
        TertiaryContainer,
    ];
    for s in all_schemes() {
        let tone = |role| utils::lstar_from_argb(s.argb(role));
        for &(c, fore, back, respectively) in contrasts {
            let required = min_requirement(c, s.contrast);
            let pairs: Vec<(Role, Role)> = if respectively {
                fore.iter().copied().zip(back.iter().copied()).collect()
            } else {
                fore.iter()
                    .flat_map(|&f| back.iter().map(move |&b| (f, b)))
                    .collect()
            };
            for (f, b) in pairs {
                let (ft, bt) = (tone(f), tone(b));
                let ratio = contrast::ratio_of_tones(ft, bt);
                // Above 4.5 it is a goal, which black or white may not reach.
                let failing = if required <= 4.5 {
                    ratio < required - 0.05
                } else {
                    ft != 0.0 && bt != 0.0 && ft != 100.0 && bt != 100.0 && ratio < required - 0.05
                };
                assert!(
                    !failing,
                    "{f:?} {ft} on {b:?} {bt}: {ratio}, not {required}"
                );
            }
        }
        // Accents are 10 farther from the surfaces than their containers.
        for (accent, container) in [
            (Primary, PrimaryContainer),
            (Secondary, SecondaryContainer),
            (Tertiary, TertiaryContainer),
            (Error, ErrorContainer),
        ] {
            let (ft, bt) = (tone(accent), tone(container));
            let delta = if s.dark { ft - bt } else { bt - ft };
            assert!(delta >= 10.0 - 0.5, "{accent:?} {ft} {container:?} {bt}");
        }
        // No background in the band from 50 to 60.
        for bg in backgrounds {
            let t = tone(bg);
            assert!(!(50.5..59.5).contains(&t), "{bg:?} {t}");
        }
    }
}
