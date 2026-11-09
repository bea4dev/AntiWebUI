//! A hue and a chroma, at any tone. From `palettes/tonal_palette.ts`.

use super::hct::{Hct, color_from_argb};
use anti_webui::prelude::Color;

/// The colors of one hue and chroma from black to white. Where the chroma can't be had at a tone,
/// the color there has as much as it can.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TonalPalette {
    hue: f64,
    chroma: f64,
    key_color: Hct,
}

impl TonalPalette {
    pub fn from_hue_and_chroma(hue: f64, chroma: f64) -> Self {
        Self {
            hue,
            chroma,
            key_color: key_color(hue, chroma),
        }
    }

    /// The hue and chroma of `hct`, which is also its key color.
    pub fn from_hct(hct: Hct) -> Self {
        Self {
            hue: hct.hue(),
            chroma: hct.chroma(),
            key_color: hct,
        }
    }

    pub fn hue(&self) -> f64 {
        self.hue
    }

    pub fn chroma(&self) -> f64 {
        self.chroma
    }

    /// The color of the palette with its chroma at the tone nearest 50.
    pub fn key_color(&self) -> Hct {
        self.key_color
    }

    /// The color at `tone`, from 0 to 100.
    pub fn tone(&self, tone: f64) -> Color {
        color_from_argb(self.argb(tone))
    }

    pub fn hct(&self, tone: f64) -> Hct {
        Hct::from_argb(self.argb(tone))
    }

    pub(crate) fn argb(&self, tone: f64) -> u32 {
        // Yellows at 99 come out too green; halfway to white looks right.
        if tone == 99.0 && Hct::is_yellow(self.hue) {
            average_argb(self.argb(98.0), self.argb(100.0))
        } else {
            Hct::new(self.hue, self.chroma, tone).to_argb()
        }
    }
}

fn average_argb(a: u32, b: u32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = ((a >> shift) & 0xff, (b >> shift) & 0xff);
        (((x + y) as f64 / 2.0).round() as u32) << shift
    };
    0xff00_0000 | channel(16) | channel(8) | channel(0)
}

/// The color of this hue with the chroma asked for, at the tone nearest 50 that has it, or the
/// most chroma there is at this hue.
fn key_color(hue: f64, requested_chroma: f64) -> Hct {
    const PIVOT_TONE: i32 = 50;
    const MAX_CHROMA: f64 = 200.0;
    const EPSILON: f64 = 0.01;
    let mut cache = [f64::NAN; 101];
    let mut max_chroma = |tone: i32| {
        let cached = &mut cache[tone as usize];
        if cached.is_nan() {
            *cached = Hct::new(hue, MAX_CHROMA, tone as f64).chroma();
        }
        *cached
    };
    // Binary search for the tone nearest 50 with enough chroma, assuming chroma goes up and then
    // down with tone.
    let mut lower_tone = 0;
    let mut upper_tone = 100;
    while lower_tone < upper_tone {
        let mid_tone = (lower_tone + upper_tone) / 2;
        let is_ascending = max_chroma(mid_tone) < max_chroma(mid_tone + 1);
        let sufficient_chroma = max_chroma(mid_tone) >= requested_chroma - EPSILON;
        if sufficient_chroma {
            // Either side may be nearer 50: keep the one that is.
            if (lower_tone - PIVOT_TONE).abs() < (upper_tone - PIVOT_TONE).abs() {
                upper_tone = mid_tone;
            } else {
                if lower_tone == mid_tone {
                    return Hct::new(hue, requested_chroma, lower_tone as f64);
                }
                lower_tone = mid_tone;
            }
        } else if is_ascending {
            lower_tone = mid_tone + 1;
        } else {
            upper_tone = mid_tone;
        }
    }
    Hct::new(hue, requested_chroma, lower_tone as f64)
}
