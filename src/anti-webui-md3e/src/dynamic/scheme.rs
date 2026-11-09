//! Color roles from a seed color, a variant and a contrast level, as the 2021 spec of Material You
//! has them. From `dynamiccolor/dynamic_scheme.ts` (the 2021 palettes),
//! `dynamiccolor/color_spec_2021.ts` and the 2021 part of `dynamiccolor/dynamic_color.ts`.
//!
//! Upstream builds each role from closures; here a role is a variant of [`Role`], and the same
//! functions are `match`es on it. Only the roles [`ColorScheme`] has are here: not the fixed ones,
//! `background`, `shadow` or `surface_tint`.

use super::contrast::{self, ContrastCurve, curve};
use super::dislike;
use super::hct::{Hct, color_from_argb};
use super::palette::TonalPalette;
use super::temperature::TemperatureCache;
use super::utils::{clamp, sanitize_degrees};
use crate::color::{ColorScheme, SchemeVariant};

pub(crate) struct DynamicScheme {
    pub source: Hct,
    pub variant: SchemeVariant,
    pub dark: bool,
    /// From -1 to 1.
    pub contrast: f64,
    pub primary: TonalPalette,
    pub secondary: TonalPalette,
    pub tertiary: TonalPalette,
    pub neutral: TonalPalette,
    pub neutral_variant: TonalPalette,
    pub error: TonalPalette,
}

impl DynamicScheme {
    pub fn new(source: Hct, variant: SchemeVariant, dark: bool, contrast: f64) -> Self {
        use SchemeVariant::*;
        let (hue, chroma) = (source.hue(), source.chroma());
        let palette = TonalPalette::from_hue_and_chroma;
        let primary = match variant {
            Content | Fidelity => palette(hue, chroma),
            FruitSalad => palette(sanitize_degrees(hue - 50.0), 48.0),
            Monochrome => palette(hue, 0.0),
            Neutral => palette(hue, 12.0),
            Rainbow => palette(hue, 48.0),
            TonalSpot => palette(hue, 36.0),
            Expressive => palette(sanitize_degrees(hue + 240.0), 40.0),
            Vibrant => palette(hue, 200.0),
        };
        let secondary = match variant {
            Content | Fidelity => palette(hue, (chroma - 32.0).max(chroma * 0.5)),
            FruitSalad => palette(sanitize_degrees(hue - 50.0), 36.0),
            Monochrome => palette(hue, 0.0),
            Neutral => palette(hue, 8.0),
            Rainbow | TonalSpot => palette(hue, 16.0),
            Expressive => palette(
                rotated_hue(
                    source,
                    &[0.0, 21.0, 51.0, 121.0, 151.0, 191.0, 271.0, 321.0, 360.0],
                    &[45.0, 95.0, 45.0, 20.0, 45.0, 90.0, 45.0, 45.0, 45.0],
                ),
                24.0,
            ),
            Vibrant => palette(
                rotated_hue(
                    source,
                    &[0.0, 41.0, 61.0, 101.0, 131.0, 181.0, 251.0, 301.0, 360.0],
                    &[18.0, 15.0, 10.0, 12.0, 15.0, 18.0, 15.0, 12.0, 12.0],
                ),
                24.0,
            ),
        };
        let tertiary = match variant {
            Content => TonalPalette::from_hct(dislike::fix_if_disliked(
                TemperatureCache::new(source).analogous(3, 6)[2],
            )),
            Fidelity => TonalPalette::from_hct(dislike::fix_if_disliked(
                TemperatureCache::new(source).complement(),
            )),
            FruitSalad => palette(hue, 36.0),
            Monochrome => palette(hue, 0.0),
            Neutral => palette(hue, 16.0),
            Rainbow | TonalSpot => palette(sanitize_degrees(hue + 60.0), 24.0),
            Expressive => palette(
                rotated_hue(
                    source,
                    &[0.0, 21.0, 51.0, 121.0, 151.0, 191.0, 271.0, 321.0, 360.0],
                    &[120.0, 120.0, 20.0, 45.0, 20.0, 15.0, 20.0, 120.0, 120.0],
                ),
                32.0,
            ),
            Vibrant => palette(
                rotated_hue(
                    source,
                    &[0.0, 41.0, 61.0, 101.0, 131.0, 181.0, 251.0, 301.0, 360.0],
                    &[35.0, 30.0, 20.0, 25.0, 30.0, 35.0, 30.0, 25.0, 25.0],
                ),
                32.0,
            ),
        };
        let neutral = match variant {
            Content | Fidelity => palette(hue, chroma / 8.0),
            FruitSalad => palette(hue, 10.0),
            Monochrome | Rainbow => palette(hue, 0.0),
            Neutral => palette(hue, 2.0),
            TonalSpot => palette(hue, 6.0),
            Expressive => palette(sanitize_degrees(hue + 15.0), 8.0),
            Vibrant => palette(hue, 10.0),
        };
        let neutral_variant = match variant {
            Content | Fidelity => palette(hue, chroma / 8.0 + 4.0),
            FruitSalad => palette(hue, 16.0),
            Monochrome | Rainbow => palette(hue, 0.0),
            Neutral => palette(hue, 2.0),
            TonalSpot => palette(hue, 8.0),
            Expressive => palette(sanitize_degrees(hue + 15.0), 12.0),
            Vibrant => palette(hue, 12.0),
        };
        Self {
            source,
            variant,
            dark,
            contrast,
            primary,
            secondary,
            tertiary,
            neutral,
            neutral_variant,
            error: palette(25.0, 84.0),
        }
    }

    pub fn argb(&self, role: Role) -> u32 {
        role.palette(self).argb(role.tone_of(self))
    }

    pub fn color_scheme(&self) -> ColorScheme {
        use Role::*;
        let c = |role| color_from_argb(self.argb(role));
        ColorScheme {
            primary: c(Primary),
            on_primary: c(OnPrimary),
            primary_container: c(PrimaryContainer),
            on_primary_container: c(OnPrimaryContainer),
            secondary: c(Secondary),
            on_secondary: c(OnSecondary),
            secondary_container: c(SecondaryContainer),
            on_secondary_container: c(OnSecondaryContainer),
            tertiary: c(Tertiary),
            on_tertiary: c(OnTertiary),
            tertiary_container: c(TertiaryContainer),
            on_tertiary_container: c(OnTertiaryContainer),
            error: c(Error),
            on_error: c(OnError),
            error_container: c(ErrorContainer),
            on_error_container: c(OnErrorContainer),
            surface: c(Surface),
            surface_dim: c(SurfaceDim),
            surface_bright: c(SurfaceBright),
            surface_variant: c(SurfaceVariant),
            on_surface: c(OnSurface),
            on_surface_variant: c(OnSurfaceVariant),
            inverse_surface: c(InverseSurface),
            inverse_on_surface: c(InverseOnSurface),
            inverse_primary: c(InversePrimary),
            scrim: c(Scrim),
            surface_container_lowest: c(SurfaceContainerLowest),
            surface_container_low: c(SurfaceContainerLow),
            surface_container: c(SurfaceContainer),
            surface_container_high: c(SurfaceContainerHigh),
            surface_container_highest: c(SurfaceContainerHighest),
            outline: c(Outline),
            outline_variant: c(OutlineVariant),
        }
    }

    fn is_fidelity(&self) -> bool {
        matches!(
            self.variant,
            SchemeVariant::Fidelity | SchemeVariant::Content
        )
    }

    fn is_monochrome(&self) -> bool {
        self.variant == SchemeVariant::Monochrome
    }

    /// What the colors on surfaces are measured against: the surface furthest from them in tone.
    fn highest_surface(&self) -> Role {
        if self.dark {
            Role::SurfaceBright
        } else {
            Role::SurfaceDim
        }
    }
}

/// The hue of `source` turned by the rotation for the range it is in.
pub(crate) fn rotated_hue(source: Hct, breakpoints: &[f64], rotations: &[f64]) -> f64 {
    let size = breakpoints.len().saturating_sub(1).min(rotations.len());
    let hue = source.hue();
    let rotation = if size == 0 {
        0.0
    } else {
        // Upstream's `getPiecewiseHue` gives the hue itself where no range has it.
        (0..size)
            .find(|&i| hue >= breakpoints[i] && hue < breakpoints[i + 1])
            .map_or(hue, |i| sanitize_degrees(rotations[i]))
    };
    sanitize_degrees(hue + rotation)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Surface,
    SurfaceDim,
    SurfaceBright,
    SurfaceContainerLowest,
    SurfaceContainerLow,
    SurfaceContainer,
    SurfaceContainerHigh,
    SurfaceContainerHighest,
    OnSurface,
    SurfaceVariant,
    OnSurfaceVariant,
    InverseSurface,
    InverseOnSurface,
    Outline,
    OutlineVariant,
    Scrim,
    Primary,
    OnPrimary,
    PrimaryContainer,
    OnPrimaryContainer,
    InversePrimary,
    Secondary,
    OnSecondary,
    SecondaryContainer,
    OnSecondaryContainer,
    Tertiary,
    OnTertiary,
    TertiaryContainer,
    OnTertiaryContainer,
    Error,
    OnError,
    ErrorContainer,
    OnErrorContainer,
}

/// Two roles that keep `delta` apart in tone, `nearer` being nearer the surfaces. Upstream also has
/// pairs that are lighter or darker than each other and pairs that stay together; only the fixed
/// roles use them.
struct ToneDeltaPair {
    nearer: Role,
    farther: Role,
    delta: f64,
}

impl Role {
    fn palette(self, s: &DynamicScheme) -> &TonalPalette {
        use Role::*;
        match self {
            Surface
            | SurfaceDim
            | SurfaceBright
            | SurfaceContainerLowest
            | SurfaceContainerLow
            | SurfaceContainer
            | SurfaceContainerHigh
            | SurfaceContainerHighest
            | OnSurface
            | InverseSurface
            | InverseOnSurface
            | Scrim => &s.neutral,
            SurfaceVariant | OnSurfaceVariant | Outline | OutlineVariant => &s.neutral_variant,
            Primary | OnPrimary | PrimaryContainer | OnPrimaryContainer | InversePrimary => {
                &s.primary
            }
            Secondary | OnSecondary | SecondaryContainer | OnSecondaryContainer => &s.secondary,
            Tertiary | OnTertiary | TertiaryContainer | OnTertiaryContainer => &s.tertiary,
            Error | OnError | ErrorContainer | OnErrorContainer => &s.error,
        }
    }

    /// The tone it would like, before contrast is taken into account.
    fn tone(self, s: &DynamicScheme) -> f64 {
        use Role::*;
        let dark = s.dark;
        let pick = |d: f64, l: f64| if dark { d } else { l };
        let level = |c: ContrastCurve| c.get(s.contrast);
        let mono = s.is_monochrome();
        let fidelity = s.is_fidelity();
        match self {
            Surface => pick(6.0, 98.0),
            SurfaceDim => pick(6.0, level(curve(87.0, 87.0, 80.0, 75.0))),
            SurfaceBright => pick(level(curve(24.0, 24.0, 29.0, 34.0)), 98.0),
            SurfaceContainerLowest => pick(level(curve(4.0, 4.0, 2.0, 0.0)), 100.0),
            SurfaceContainerLow => pick(
                level(curve(10.0, 10.0, 11.0, 12.0)),
                level(curve(96.0, 96.0, 96.0, 95.0)),
            ),
            SurfaceContainer => pick(
                level(curve(12.0, 12.0, 16.0, 20.0)),
                level(curve(94.0, 94.0, 92.0, 90.0)),
            ),
            SurfaceContainerHigh => pick(
                level(curve(17.0, 17.0, 21.0, 25.0)),
                level(curve(92.0, 92.0, 88.0, 85.0)),
            ),
            SurfaceContainerHighest => pick(
                level(curve(22.0, 22.0, 26.0, 30.0)),
                level(curve(90.0, 90.0, 84.0, 80.0)),
            ),
            OnSurface => pick(90.0, 10.0),
            SurfaceVariant => pick(30.0, 90.0),
            OnSurfaceVariant => pick(80.0, 30.0),
            InverseSurface => pick(90.0, 20.0),
            InverseOnSurface => pick(20.0, 95.0),
            Outline => pick(60.0, 50.0),
            OutlineVariant => pick(30.0, 80.0),
            Scrim => 0.0,
            Primary if mono => pick(100.0, 0.0),
            Primary => pick(80.0, 40.0),
            OnPrimary if mono => pick(10.0, 90.0),
            OnPrimary => pick(20.0, 100.0),
            PrimaryContainer if fidelity => s.source.tone(),
            PrimaryContainer if mono => pick(85.0, 25.0),
            PrimaryContainer => pick(30.0, 90.0),
            OnPrimaryContainer if fidelity => foreground_tone(PrimaryContainer.tone(s), 4.5),
            OnPrimaryContainer if mono => pick(0.0, 100.0),
            OnPrimaryContainer => pick(90.0, 30.0),
            InversePrimary => pick(40.0, 80.0),
            Secondary => pick(80.0, 40.0),
            OnSecondary if mono => pick(10.0, 100.0),
            OnSecondary => pick(20.0, 100.0),
            SecondaryContainer if mono => pick(30.0, 85.0),
            SecondaryContainer if !fidelity => pick(30.0, 90.0),
            SecondaryContainer => find_desired_chroma_by_tone(
                s.secondary.hue(),
                s.secondary.chroma(),
                pick(30.0, 90.0),
                !dark,
            ),
            OnSecondaryContainer if mono => pick(90.0, 10.0),
            OnSecondaryContainer if !fidelity => pick(90.0, 30.0),
            OnSecondaryContainer => foreground_tone(SecondaryContainer.tone(s), 4.5),
            Tertiary if mono => pick(90.0, 25.0),
            Tertiary => pick(80.0, 40.0),
            OnTertiary if mono => pick(10.0, 90.0),
            OnTertiary => pick(20.0, 100.0),
            TertiaryContainer if mono => pick(60.0, 49.0),
            TertiaryContainer if !fidelity => pick(30.0, 90.0),
            TertiaryContainer => dislike::fix_if_disliked(s.tertiary.hct(s.source.tone())).tone(),
            OnTertiaryContainer if mono => pick(0.0, 100.0),
            OnTertiaryContainer if !fidelity => pick(90.0, 30.0),
            OnTertiaryContainer => foreground_tone(TertiaryContainer.tone(s), 4.5),
            Error => pick(80.0, 40.0),
            OnError => pick(20.0, 100.0),
            ErrorContainer => pick(30.0, 90.0),
            OnErrorContainer if mono => pick(90.0, 10.0),
            OnErrorContainer => pick(90.0, 30.0),
        }
    }

    fn is_background(self) -> bool {
        use Role::*;
        matches!(
            self,
            Surface
                | SurfaceDim
                | SurfaceBright
                | SurfaceContainerLowest
                | SurfaceContainerLow
                | SurfaceContainer
                | SurfaceContainerHigh
                | SurfaceContainerHighest
                | SurfaceVariant
                | InverseSurface
                | Primary
                | PrimaryContainer
                | Secondary
                | SecondaryContainer
                | Tertiary
                | TertiaryContainer
                | Error
                | ErrorContainer
        )
    }

    /// What it is shown on, with the contrast it needs there.
    fn background(self, s: &DynamicScheme) -> Option<(Role, ContrastCurve)> {
        use Role::*;
        let text = curve(4.5, 7.0, 11.0, 21.0);
        let accent = curve(3.0, 4.5, 7.0, 7.0);
        let container = curve(1.0, 1.0, 3.0, 4.5);
        let on_container = curve(3.0, 4.5, 7.0, 11.0);
        Some(match self {
            Surface
            | SurfaceDim
            | SurfaceBright
            | SurfaceContainerLowest
            | SurfaceContainerLow
            | SurfaceContainer
            | SurfaceContainerHigh
            | SurfaceContainerHighest
            | SurfaceVariant
            | InverseSurface
            | Scrim => return None,
            OnSurface => (s.highest_surface(), text),
            OnSurfaceVariant => (s.highest_surface(), on_container),
            InverseOnSurface => (InverseSurface, text),
            Outline => (s.highest_surface(), curve(1.5, 3.0, 4.5, 7.0)),
            OutlineVariant => (s.highest_surface(), container),
            Primary | Secondary | Tertiary | Error => (s.highest_surface(), accent),
            PrimaryContainer | SecondaryContainer | TertiaryContainer | ErrorContainer => {
                (s.highest_surface(), container)
            }
            OnPrimary => (Primary, text),
            OnSecondary => (Secondary, text),
            OnTertiary => (Tertiary, text),
            OnError => (Error, text),
            OnPrimaryContainer => (PrimaryContainer, on_container),
            OnSecondaryContainer => (SecondaryContainer, on_container),
            OnTertiaryContainer => (TertiaryContainer, on_container),
            OnErrorContainer => (ErrorContainer, on_container),
            InversePrimary => (InverseSurface, accent),
        })
    }

    fn tone_delta_pair(self) -> Option<ToneDeltaPair> {
        use Role::*;
        let (container, accent) = match self {
            Primary | PrimaryContainer => (PrimaryContainer, Primary),
            Secondary | SecondaryContainer => (SecondaryContainer, Secondary),
            Tertiary | TertiaryContainer => (TertiaryContainer, Tertiary),
            Error | ErrorContainer => (ErrorContainer, Error),
            _ => return None,
        };
        Some(ToneDeltaPair {
            nearer: container,
            farther: accent,
            delta: 10.0,
        })
    }

    /// The tone it gets in `s`: the one it would like, moved for contrast with what it is on and
    /// for the distance it keeps from its pair.
    fn tone_of(self, s: &DynamicScheme) -> f64 {
        let decreasing_contrast = s.contrast < 0.0;

        if let Some(pair) = self.tone_delta_pair() {
            let (nearer, farther) = (pair.nearer, pair.farther);
            let am_nearer = self == nearer;
            let expansion_dir = if s.dark { 1.0 } else { -1.0 };
            let delta = pair.delta;

            let mut n_tone = nearer.tone(s);
            let mut f_tone = farther.tone(s);

            // 1st round: solve to the minimum contrast each needs with the background.
            if let (Some((bg, _)), Some((_, n_curve)), Some((_, f_curve))) = (
                self.background(s),
                nearer.background(s),
                farther.background(s),
            ) {
                let bg_tone = bg.tone_of(s);
                let n_contrast = n_curve.get(s.contrast);
                let f_contrast = f_curve.get(s.contrast);
                if contrast::ratio_of_tones(bg_tone, n_tone) < n_contrast {
                    n_tone = foreground_tone(bg_tone, n_contrast);
                }
                if contrast::ratio_of_tones(bg_tone, f_tone) < f_contrast {
                    f_tone = foreground_tone(bg_tone, f_contrast);
                }
                if decreasing_contrast {
                    // Less contrast is asked for: the tones that just give it.
                    n_tone = foreground_tone(bg_tone, n_contrast);
                    f_tone = foreground_tone(bg_tone, f_contrast);
                }
            }

            // 2nd round: keep them `delta` apart, moving the farther one first.
            if (f_tone - n_tone) * expansion_dir < delta {
                f_tone = clamp(0.0, 100.0, n_tone + delta * expansion_dir);
                if (f_tone - n_tone) * expansion_dir < delta {
                    n_tone = clamp(0.0, 100.0, f_tone - delta * expansion_dir);
                }
            }

            // 3rd round: out of the 50 to 60 band, where neither black nor white text reads well.
            if (50.0..60.0).contains(&n_tone) {
                if expansion_dir > 0.0 {
                    n_tone = 60.0;
                    f_tone = f_tone.max(n_tone + delta * expansion_dir);
                } else {
                    n_tone = 49.0;
                    f_tone = f_tone.min(n_tone + delta * expansion_dir);
                }
            } else if (50.0..60.0).contains(&f_tone) {
                f_tone = if expansion_dir > 0.0 { 60.0 } else { 49.0 };
            }

            return if am_nearer { n_tone } else { f_tone };
        }

        let mut answer = self.tone(s);
        let Some((bg, contrast_curve)) = self.background(s) else {
            return answer;
        };
        let bg_tone = bg.tone_of(s);
        let desired_ratio = contrast_curve.get(s.contrast);
        if contrast::ratio_of_tones(bg_tone, answer) < desired_ratio {
            answer = foreground_tone(bg_tone, desired_ratio);
        }
        if decreasing_contrast {
            answer = foreground_tone(bg_tone, desired_ratio);
        }
        if self.is_background() && (50.0..60.0).contains(&answer) {
            answer = if contrast::ratio_of_tones(49.0, bg_tone) >= desired_ratio {
                49.0
            } else {
                60.0
            };
        }
        // Upstream checks a second background here; only the fixed roles have one.
        answer
    }
}

/// The tone with `ratio` against `bg_tone`, lighter or darker, whichever reads better.
fn foreground_tone(bg_tone: f64, ratio: f64) -> f64 {
    let lighter_tone = contrast::lighter_unsafe(bg_tone, ratio);
    let darker_tone = contrast::darker_unsafe(bg_tone, ratio);
    let lighter_ratio = contrast::ratio_of_tones(lighter_tone, bg_tone);
    let darker_ratio = contrast::ratio_of_tones(darker_tone, bg_tone);
    if tone_prefers_light_foreground(bg_tone) {
        // Light text on a middle tone looks better, even with a little less contrast.
        let negligible_difference = (lighter_ratio - darker_ratio).abs() < 0.1
            && lighter_ratio < ratio
            && darker_ratio < ratio;
        if lighter_ratio >= ratio || lighter_ratio >= darker_ratio || negligible_difference {
            lighter_tone
        } else {
            darker_tone
        }
    } else if darker_ratio >= ratio || darker_ratio >= lighter_ratio {
        darker_tone
    } else {
        lighter_tone
    }
}

/// Tones below 60 take light text, though by contrast alone dark text would be chosen up to about
/// 50.
fn tone_prefers_light_foreground(tone: f64) -> bool {
    tone.round() < 60.0
}

/// The tone near `tone` where `hue` has `chroma`, searching up or down one by one.
fn find_desired_chroma_by_tone(hue: f64, chroma: f64, tone: f64, by_decreasing_tone: bool) -> f64 {
    let mut answer = tone;
    let mut closest_to_chroma = Hct::new(hue, chroma, tone);
    if closest_to_chroma.chroma() < chroma {
        let mut chroma_peak = closest_to_chroma.chroma();
        while closest_to_chroma.chroma() < chroma {
            answer += if by_decreasing_tone { -1.0 } else { 1.0 };
            let potential_solution = Hct::new(hue, chroma, answer);
            if chroma_peak > potential_solution.chroma() {
                break;
            }
            if (potential_solution.chroma() - chroma).abs() < 0.4 {
                break;
            }
            let potential_delta = (potential_solution.chroma() - chroma).abs();
            let current_delta = (closest_to_chroma.chroma() - chroma).abs();
            if potential_delta < current_delta {
                closest_to_chroma = potential_solution;
            }
            chroma_peak = chroma_peak.max(potential_solution.chroma());
        }
    }
    answer
}
