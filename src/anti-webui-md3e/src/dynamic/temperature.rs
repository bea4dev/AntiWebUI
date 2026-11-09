//! Warm and cool colors, for complements and analogous colors. From
//! `temperature/temperature_cache.ts`.

use super::hct::Hct;
use super::utils::{self, sanitize_degrees, sanitize_degrees_int};
use std::f64::consts::PI;

pub(crate) struct TemperatureCache {
    input: Hct,
    /// The input's chroma and tone at each hue from 0 to 360.
    hcts_by_hue: Vec<Hct>,
    coldest: (Hct, f64),
    warmest: (Hct, f64),
}

impl TemperatureCache {
    pub fn new(input: Hct) -> Self {
        let hcts_by_hue: Vec<Hct> = (0..=360)
            .map(|hue| Hct::new(hue as f64, input.chroma(), input.tone()))
            .collect();
        let mut by_temp: Vec<(Hct, f64)> = hcts_by_hue
            .iter()
            .chain([&input])
            .map(|&hct| (hct, raw_temperature(hct)))
            .collect();
        // Stable, as upstream's is.
        by_temp.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            input,
            hcts_by_hue,
            coldest: by_temp[0],
            warmest: by_temp[by_temp.len() - 1],
        }
    }

    /// From 0 for the coldest color of the input's chroma and tone to 1 for the warmest.
    pub fn relative_temperature(&self, hct: Hct) -> f64 {
        let range = self.warmest.1 - self.coldest.1;
        let difference_from_coldest = raw_temperature(hct) - self.coldest.1;
        if range == 0.0 {
            return 0.5;
        }
        difference_from_coldest / range
    }

    pub fn input_relative_temperature(&self) -> f64 {
        self.relative_temperature(self.input)
    }

    /// The color as far from the input in temperature as the input is from the middle.
    pub fn complement(&self) -> Hct {
        let (coldest_hue, coldest_temp) = (self.coldest.0.hue(), self.coldest.1);
        let warmest_hue = self.warmest.0.hue();
        let range = self.warmest.1 - coldest_temp;
        let start_hue_is_coldest_to_warmest =
            is_between(self.input.hue(), coldest_hue, warmest_hue);
        let (start_hue, end_hue) = if start_hue_is_coldest_to_warmest {
            (warmest_hue, coldest_hue)
        } else {
            (coldest_hue, warmest_hue)
        };
        let direction_of_rotation = 1.0;
        let mut smallest_error = 1000.0;
        let mut answer = self.hcts_by_hue[self.input.hue().round() as usize];
        let complement_relative_temp = 1.0 - self.input_relative_temperature();
        // Find the color in the other half whose relative temperature is nearest the complement's.
        let mut hue_addend = 0.0;
        while hue_addend <= 360.0 {
            let hue = sanitize_degrees(start_hue + direction_of_rotation * hue_addend);
            hue_addend += 1.0;
            if !is_between(hue, start_hue, end_hue) {
                continue;
            }
            let possible_answer = self.hcts_by_hue[hue.round() as usize];
            let relative_temp = (raw_temperature(possible_answer) - coldest_temp) / range;
            let error = (complement_relative_temp - relative_temp).abs();
            if error < smallest_error {
                smallest_error = error;
                answer = possible_answer;
            }
        }
        answer
    }

    /// `count` colors around the input, evenly apart in temperature on a wheel divided in
    /// `divisions`, with the input in the middle.
    pub fn analogous(&self, count: usize, divisions: usize) -> Vec<Hct> {
        let start_hue = self.input.hue().round() as i32;
        let start_hct = self.hcts_by_hue[start_hue as usize];
        let mut last_temp = self.relative_temperature(start_hct);

        let mut all_colors = vec![start_hct];

        let mut absolute_total_temp_delta = 0.0;
        for i in 0..360 {
            let hue = sanitize_degrees_int(start_hue + i);
            let hct = self.hcts_by_hue[hue as usize];
            let temp = self.relative_temperature(hct);
            absolute_total_temp_delta += (temp - last_temp).abs();
            last_temp = temp;
        }

        let mut hue_addend = 1;
        let temp_step = absolute_total_temp_delta / divisions as f64;
        let mut total_temp_delta = 0.0;
        last_temp = self.relative_temperature(start_hct);
        while all_colors.len() < divisions {
            let hue = sanitize_degrees_int(start_hue + hue_addend);
            let hct = self.hcts_by_hue[hue as usize];
            let temp = self.relative_temperature(hct);
            total_temp_delta += (temp - last_temp).abs();

            let mut desired = all_colors.len() as f64 * temp_step;
            let mut index_satisfied = total_temp_delta >= desired;
            let mut index_addend = 1;
            // Keep adding this hue while it satisfies more than one division.
            while index_satisfied && all_colors.len() < divisions {
                all_colors.push(hct);
                desired = (all_colors.len() + index_addend) as f64 * temp_step;
                index_satisfied = total_temp_delta >= desired;
                index_addend += 1;
            }
            last_temp = temp;
            hue_addend += 1;

            if hue_addend > 360 {
                while all_colors.len() < divisions {
                    all_colors.push(hct);
                }
                break;
            }
        }

        let mut answers = vec![self.input];
        let len = all_colors.len() as i64;
        let wrap = |index: i64| all_colors[index.rem_euclid(len) as usize];

        let increase_hue_count = ((count as f64 - 1.0) / 2.0).floor() as i64;
        for i in 1..=increase_hue_count {
            answers.insert(0, wrap(-i));
        }

        let decrease_hue_count = count as i64 - increase_hue_count - 1;
        for i in 1..=decrease_hue_count {
            answers.push(wrap(i));
        }

        answers
    }
}

/// Whether `angle` is on the arc from `a` to `b` going up, in degrees.
fn is_between(angle: f64, a: f64, b: f64) -> bool {
    if a < b {
        a <= angle && angle <= b
    } else {
        a <= angle || angle <= b
    }
}

/// Warmth by hue and chroma in L*a*b*: from about -0.5 to 2.6, cold to warm.
pub(crate) fn raw_temperature(color: Hct) -> f64 {
    let lab = utils::lab_from_argb(color.to_argb());
    let hue = sanitize_degrees(lab[2].atan2(lab[1]) * 180.0 / PI);
    let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    -0.5 + 0.02 * chroma.powf(1.07) * (sanitize_degrees(hue - 50.0) * PI / 180.0).cos()
}
