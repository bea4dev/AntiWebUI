//! Contrast ratios between tones, as WCAG defines them. From `contrast/contrast.ts` and
//! `dynamiccolor/contrast_curve.ts`.

use super::utils::{self, lerp};

/// The contrast ratio of two tones, from 1 to 21.
pub(crate) fn ratio_of_tones(a: f64, b: f64) -> f64 {
    let a = a.clamp(0.0, 100.0);
    let b = b.clamp(0.0, 100.0);
    ratio_of_ys(utils::y_from_lstar(a), utils::y_from_lstar(b))
}

fn ratio_of_ys(y1: f64, y2: f64) -> f64 {
    let lighter = if y1 > y2 { y1 } else { y2 };
    let darker = if lighter == y2 { y1 } else { y2 };
    (lighter + 5.0) / (darker + 5.0)
}

/// The tone that has `ratio` with `tone` and is lighter, or `None` if there is none.
pub(crate) fn lighter(tone: f64, ratio: f64) -> Option<f64> {
    if !(0.0..=100.0).contains(&tone) {
        return None;
    }
    let dark_y = utils::y_from_lstar(tone);
    let light_y = ratio * (dark_y + 5.0) - 5.0;
    let real_contrast = ratio_of_ys(light_y, dark_y);
    let delta = (real_contrast - ratio).abs();
    if real_contrast < ratio && delta > 0.04 {
        return None;
    }
    // A little more than needed, since tones are rounded on the way to colors.
    let value = utils::lstar_from_y(light_y) + 0.4;
    (0.0..=100.0).contains(&value).then_some(value)
}

/// The tone that has `ratio` with `tone` and is darker, or `None` if there is none.
pub(crate) fn darker(tone: f64, ratio: f64) -> Option<f64> {
    if !(0.0..=100.0).contains(&tone) {
        return None;
    }
    let light_y = utils::y_from_lstar(tone);
    let dark_y = (light_y + 5.0) / ratio - 5.0;
    let real_contrast = ratio_of_ys(light_y, dark_y);
    let delta = (real_contrast - ratio).abs();
    if real_contrast < ratio && delta > 0.04 {
        return None;
    }
    let value = utils::lstar_from_y(dark_y) - 0.4;
    (0.0..=100.0).contains(&value).then_some(value)
}

/// [`lighter`], or white.
pub(crate) fn lighter_unsafe(tone: f64, ratio: f64) -> f64 {
    lighter(tone, ratio).unwrap_or(100.0)
}

/// [`darker`], or black.
pub(crate) fn darker_unsafe(tone: f64, ratio: f64) -> f64 {
    darker(tone, ratio).unwrap_or(0.0)
}

/// A contrast ratio for each contrast level: -1, 0, 0.5 and 1, with what is between interpolated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContrastCurve {
    pub low: f64,
    pub normal: f64,
    pub medium: f64,
    pub high: f64,
}

pub(crate) const fn curve(low: f64, normal: f64, medium: f64, high: f64) -> ContrastCurve {
    ContrastCurve {
        low,
        normal,
        medium,
        high,
    }
}

impl ContrastCurve {
    pub fn get(&self, level: f64) -> f64 {
        if level <= -1.0 {
            self.low
        } else if level < 0.0 {
            lerp(self.low, self.normal, level + 1.0)
        } else if level < 0.5 {
            lerp(self.normal, self.medium, level / 0.5)
        } else if level < 1.0 {
            lerp(self.medium, self.high, (level - 0.5) / 0.5)
        } else {
            self.high
        }
    }
}
