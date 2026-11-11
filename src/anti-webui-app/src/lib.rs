//! Wires a platform backend, the wgpu renderer and the core together.

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod clock;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod frame_stats;
pub mod headless;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod host;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod pacing;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod perf_overlay;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod present;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod runner;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod vsync;

use anti_webui_core::settings::{Settings, update_settings};
use anti_webui_core::view::IntoView;
pub use anti_webui_core::window::DecorationMode;

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
    decorations: DecorationMode,
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
            decorations: DecorationMode::Auto,
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

    /// Who draws the frame of the window: the system's where there is one by default. With
    /// [`DecorationMode::Client`], or where the system has none (GNOME), the window has no title bar
    /// unless it shows one, like `title_bar` of the standard widgets. See
    /// [`window_decorations`](anti_webui_core::window::window_decorations).
    pub fn decorations(mut self, mode: DecorationMode) -> Self {
        self.decorations = mode;
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

    /// Runs what `root` opens without a window of its own, until
    /// [`quit`](anti_webui_core::window::quit): for a desktop shell, through
    /// `anti-webui-wayland-shell` (see `docs/design/37-wayland-shell.md`).
    #[doc(hidden)]
    pub fn run_shell(mut self, root: impl FnOnce()) -> Result<(), Box<dyn std::error::Error>> {
        self.apply_settings();
        #[cfg(target_os = "linux")]
        {
            runner::run_shell(self, root)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = root;
            Err("a shell needs Wayland".into())
        }
    }

    /// Opens the window and runs until it is closed.
    pub fn run<V: IntoView>(
        mut self,
        root: impl FnOnce() -> V,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // The GPU is made while the first window is (Windows; see `Gpu::start_early`).
        #[cfg(windows)]
        anti_webui_wgpu::Gpu::start_early();
        self.apply_settings();
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            runner::run(self, root)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
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
