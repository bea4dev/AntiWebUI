//! Settings for the whole framework (see `docs/design/08-scrolling-and-settings.md`).

use crate::reactive::{Signal, create_root, signal};

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Settings {
    pub scroll: ScrollSettings,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScrollSettings {
    /// Multiplies the motion of fingers on a touchpad, and inertia that follows from it.
    pub touchpad_factor: f32,
    /// Logical pixels per wheel notch.
    pub wheel_step: f32,
    pub kinetic: KineticSettings,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct KineticSettings {
    /// Whether scrolling goes on after the fingers leave a touchpad.
    pub enabled: bool,
    /// Multiplies the speed inertia starts with. Only where the framework computes inertia, which
    /// macOS and Windows do themselves. So do the settings below.
    pub velocity_factor: f32,
    /// In logical pixels per second squared; larger stops sooner. The distance is the speed
    /// squared over twice this.
    pub deceleration: f32,
    /// The furthest one fling goes, in window sizes.
    pub max_screens: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scroll: ScrollSettings {
                touchpad_factor: 1.0,
                wheel_step: 48.0,
                kinetic: KineticSettings {
                    enabled: true,
                    velocity_factor: 1.0,
                    // 96 pixels per inch at Chromium's 2.7559e-5 inches per millisecond squared.
                    deceleration: 2645.7,
                    max_screens: 3.0,
                },
            },
        }
    }
}

thread_local! {
    // In its own root: it outlives whichever owner happens to read it first.
    static SETTINGS: Signal<Settings> = create_root(|| signal(Settings::default())).0;
}

/// The current settings. Reading them in an effect or a view subscribes to changes.
pub fn settings() -> Settings {
    SETTINGS.with(|s| s.get())
}

/// Changes the settings. They apply from the next event on.
pub fn update_settings(f: impl FnOnce(&mut Settings)) {
    SETTINGS.with(|s| s.update(f));
}

pub(crate) fn with_settings<R>(f: impl FnOnce(&Settings) -> R) -> R {
    SETTINGS.with(|s| s.with_untracked(f))
}
