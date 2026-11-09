//! Dark yellow-greens, which people commonly dislike. From `dislike/dislike_analyzer.ts`.

use super::hct::Hct;

pub(crate) fn is_disliked(hct: Hct) -> bool {
    let hue_passes = (90.0..=111.0).contains(&hct.hue().round());
    let chroma_passes = hct.chroma().round() > 16.0;
    let tone_passes = hct.tone().round() < 65.0;
    hue_passes && chroma_passes && tone_passes
}

/// Lightens a disliked color to tone 70, where it no longer is.
pub(crate) fn fix_if_disliked(hct: Hct) -> Hct {
    if is_disliked(hct) {
        Hct::new(hct.hue(), hct.chroma(), 70.0)
    } else {
        hct
    }
}
