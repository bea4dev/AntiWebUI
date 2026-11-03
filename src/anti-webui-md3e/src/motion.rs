//! The springs of the Expressive motion scheme. "Spatial" ones are for what moves or changes shape
//! and may overshoot; "effects" ones are for colors and opacity and don't.

use anti_webui::prelude::Spring;

pub fn fast_spatial() -> Spring {
    Spring::from_ratio(800.0, 0.6)
}

pub fn default_spatial() -> Spring {
    Spring::from_ratio(380.0, 0.8)
}

pub fn slow_spatial() -> Spring {
    Spring::from_ratio(200.0, 0.8)
}

pub fn fast_effects() -> Spring {
    Spring::from_ratio(3800.0, 1.0)
}

pub fn default_effects() -> Spring {
    Spring::from_ratio(1600.0, 1.0)
}

pub fn slow_effects() -> Spring {
    Spring::from_ratio(800.0, 1.0)
}
