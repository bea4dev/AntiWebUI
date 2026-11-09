//! Material You dynamic color: color schemes from a seed color, and seed colors from an image. See
//! `docs/design/15-md3e.md`.
//!
//! This is a port of the parts of Google's material-color-utilities that do this, from its
//! TypeScript version at commit 194144d (2026-11-06):
//! <https://github.com/material-foundation/material-color-utilities>.
//!
//! Copyright 2021 Google LLC. Licensed under the Apache License, Version 2.0 (the "License"); you
//! may not use these files except in compliance with the License. You may obtain a copy of the
//! License at <http://www.apache.org/licenses/LICENSE-2.0>. Unless required by applicable law or
//! agreed to in writing, software distributed under the License is distributed on an "AS IS"
//! BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the License
//! for the specific language governing permissions and limitations under the License.
//!
//! Changed from upstream: written in Rust; only the 2021 spec of the color roles, under the default
//! viewing conditions, and only the roles that [`ColorScheme`](crate::ColorScheme) has.

mod contrast;
mod dislike;
mod hct;
mod palette;
mod quantize;
pub(crate) mod scheme;
mod score;
mod solver;
mod temperature;
mod utils;

#[cfg(test)]
mod tests;

pub use hct::Hct;
pub use palette::TonalPalette;

use anti_webui::prelude::Color;

/// Google Blue, for images with no color in them.
const FALLBACK_SEED: u32 = 0xff42_85f4;

/// The colors of an image that make good seeds for [`ColorScheme::from_seed`], best first: at most
/// `max` of them, and at least one. `pixels` are RGBA; those that aren't opaque are left out.
///
/// It takes time in proportion to the pixels, so downscale the image first: about 128 pixels on a
/// side is plenty, and is what Android does with wallpapers.
///
/// ```
/// use anti_webui_md3e::{ColorScheme, dynamic::seed_colors};
///
/// let pixels = vec![[0x67, 0x50, 0xa4, 0xff]; 64 * 64];
/// let seed = seed_colors(pixels, 4)[0];
/// let colors = ColorScheme::from_seed(seed, false);
/// ```
///
/// [`ColorScheme::from_seed`]: crate::ColorScheme::from_seed
pub fn seed_colors(pixels: impl IntoIterator<Item = [u8; 4]>, max: usize) -> Vec<Color> {
    let pixels: Vec<u32> = pixels
        .into_iter()
        .filter(|p| p[3] == 255)
        .map(|[r, g, b, _]| utils::argb_from_rgb(r, g, b))
        .collect();
    let quantized = quantize::celebi(&pixels, 128);
    score::score(&quantized, max.max(1), FALLBACK_SEED, true)
        .into_iter()
        .map(hct::color_from_argb)
        .collect()
}
