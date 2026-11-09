//! Ranking the colors of an image as seed colors: by how much of the image is near their hue and
//! by chroma, keeping hues apart. From `score/score.ts`.

use super::hct::Hct;
use super::utils::{difference_degrees, sanitize_degrees_int};

const TARGET_CHROMA: f64 = 48.0;
const WEIGHT_PROPORTION: f64 = 0.7;
const WEIGHT_CHROMA_ABOVE: f64 = 0.3;
const WEIGHT_CHROMA_BELOW: f64 = 0.1;
const CUTOFF_CHROMA: f64 = 5.0;
const CUTOFF_EXCITED_PROPORTION: f64 = 0.01;

/// At most `desired` colors of `colors_to_population` (ARGB, pixel count), best first, or
/// `fallback` if none is suitable. `filter` leaves out grays and hues that are barely there.
pub(crate) fn score(
    colors_to_population: &[(u32, u32)],
    desired: usize,
    fallback: u32,
    filter: bool,
) -> Vec<u32> {
    let mut colors_hct = Vec::with_capacity(colors_to_population.len());
    let mut hue_population = [0.0f64; 360];
    let mut population_sum = 0.0;
    for &(argb, population) in colors_to_population {
        let hct = Hct::from_argb(argb);
        colors_hct.push(hct);
        if let Some(p) = hue_population.get_mut(hct.hue().floor() as usize) {
            *p += population as f64;
        }
        population_sum += population as f64;
    }

    // How much of the image is within 15 degrees of each hue.
    let mut hue_excited_proportions = [0.0f64; 360];
    for hue in 0..360 {
        let proportion = hue_population[hue as usize] / population_sum;
        for i in hue - 14..hue + 16 {
            hue_excited_proportions[sanitize_degrees_int(i) as usize] += proportion;
        }
    }

    let mut scored_hct = Vec::new();
    for hct in colors_hct {
        let hue = sanitize_degrees_int(hct.hue().round() as i32);
        let proportion = hue_excited_proportions[hue as usize];
        if filter && (hct.chroma() < CUTOFF_CHROMA || proportion <= CUTOFF_EXCITED_PROPORTION) {
            continue;
        }
        let proportion_score = proportion * 100.0 * WEIGHT_PROPORTION;
        let chroma_weight = if hct.chroma() < TARGET_CHROMA {
            WEIGHT_CHROMA_BELOW
        } else {
            WEIGHT_CHROMA_ABOVE
        };
        let chroma_score = (hct.chroma() - TARGET_CHROMA) * chroma_weight;
        scored_hct.push((hct, proportion_score + chroma_score));
    }
    // Stable, as upstream's is.
    scored_hct.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // As far apart in hue as allows `desired` colors, from 90 degrees down to 15.
    let mut chosen_colors: Vec<Hct> = Vec::new();
    for difference in (15..=90).rev() {
        chosen_colors.clear();
        for &(hct, _) in &scored_hct {
            let duplicate_hue = chosen_colors
                .iter()
                .any(|chosen| difference_degrees(hct.hue(), chosen.hue()) < difference as f64);
            if !duplicate_hue {
                chosen_colors.push(hct);
            }
            if chosen_colors.len() >= desired {
                break;
            }
        }
        if chosen_colors.len() >= desired {
            break;
        }
    }

    if chosen_colors.is_empty() {
        return vec![fallback];
    }
    chosen_colors.iter().map(Hct::to_argb).collect()
}
