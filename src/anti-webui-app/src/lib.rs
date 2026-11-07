//! Wires a platform backend, the wgpu renderer and the core together.

#[cfg(target_os = "linux")]
mod frame_stats;
pub mod headless;
#[cfg(target_os = "linux")]
mod host;
#[cfg(target_os = "linux")]
mod pacing;
#[cfg(target_os = "linux")]
mod perf_overlay;
#[cfg(target_os = "linux")]
mod present;
#[cfg(target_os = "linux")]
mod runner;
#[cfg(target_os = "linux")]
mod vsync;

use anti_webui_core::settings::{Settings, update_settings};
use anti_webui_core::view::IntoView;

type SettingsChange = Box<dyn FnOnce(&mut Settings)>;

/// Application entry point.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// anti_webui_app::App::new().title("Hello").run(|| div().child("Hello")).unwrap();
/// ```
pub struct App {
    title: String,
    app_id: String,
    width: i32,
    height: i32,
    fullscreen: bool,
    settings: Vec<SettingsChange>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            title: "AntiWebUI".into(),
            app_id: "dev.antiwebui.app".into(),
            width: 800,
            height: 600,
            fullscreen: false,
            settings: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = app_id.into();
        self
    }

    /// Initial window size in logical pixels.
    pub fn size(mut self, width: i32, height: i32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Whether the window opens on all of the screen. See also [`set_window_fullscreen`] for
    /// changes while running.
    ///
    /// [`set_window_fullscreen`]: anti_webui_core::view::set_window_fullscreen
    pub fn fullscreen(mut self, on: bool) -> Self {
        self.fullscreen = on;
        self
    }

    /// Changes the framework settings before the window opens. See also
    /// [`update_settings`] for changes while running.
    ///
    /// ```no_run
    /// anti_webui_app::App::new().settings(|s| s.scroll.touchpad_factor = 1.5);
    /// ```
    pub fn settings(mut self, f: impl FnOnce(&mut Settings) + 'static) -> Self {
        self.settings.push(Box::new(f));
        self
    }

    fn apply_settings(&mut self) {
        for f in self.settings.drain(..) {
            update_settings(f);
        }
    }

    /// Opens the window and runs until it is closed.
    pub fn run<V: IntoView>(
        mut self,
        root: impl FnOnce() -> V,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.apply_settings();
        #[cfg(target_os = "linux")]
        {
            runner::run(self, root)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = root;
            Err("no platform backend is available for this OS yet".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anti_webui_core::settings::settings;

    #[test]
    fn settings_apply_in_order_and_keep_earlier_changes() {
        update_settings(|s| s.scroll.wheel_step = 10.0);
        let mut app = App::new()
            .settings(|s| s.scroll.touchpad_factor = 2.0)
            .settings(|s| s.scroll.touchpad_factor *= 1.5);
        assert_eq!(settings().scroll.touchpad_factor, 1.0);
        app.apply_settings();
        assert_eq!(settings().scroll.touchpad_factor, 3.0);
        assert_eq!(settings().scroll.wheel_step, 10.0);
    }
}
