//! Wires a platform backend, the wgpu renderer and the core together.

pub mod headless;
#[cfg(target_os = "linux")]
mod present;

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
            wayland::run(self, root)
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

#[cfg(target_os = "linux")]
mod wayland {
    use super::App;
    use crate::present::{Presented, Presenter};
    use anti_webui_core::Ui;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::platform::{PlatformRequest, PopupId};
    use anti_webui_core::view::IntoView;
    use anti_webui_wayland::{Ctx, SurfaceId, WaylandHandler, WindowOptions};
    use std::time::Instant;

    struct Runner {
        ui: Ui,
        presenter: Option<Presenter>,
        error: Option<Box<dyn std::error::Error>>,
        /// `ANTIWEBUI_DUMP_FRAME=path.png` writes every presented frame to a file (debugging).
        dump: Option<(String, anti_webui_wgpu::Renderer)>,
    }

    impl Runner {
        fn process(&mut self, ctx: &mut Ctx<'_>) {
            self.handle_requests(ctx);
            if self.ui.needs_redraw() {
                ctx.request_redraw();
            }
        }

        fn handle_requests(&mut self, ctx: &mut Ctx<'_>) {
            for req in self.ui.take_requests() {
                match req {
                    PlatformRequest::SetCursor(c) => ctx.set_cursor(c),
                    PlatformRequest::SetIme(state) => ctx.set_ime(state),
                    PlatformRequest::SetClipboard { kind, text } => ctx.set_clipboard(kind, text),
                    PlatformRequest::Paste(kind) => ctx.request_paste(kind),
                    PlatformRequest::SetTitle(t) => ctx.set_title(&t),
                    PlatformRequest::Close => ctx.exit(),
                    PlatformRequest::ShowPopup(params) => ctx.show_popup(params),
                    PlatformRequest::UpdatePopup(params) => ctx.update_popup(params),
                    PlatformRequest::ClosePopup(id) => {
                        self.forget_popup(ctx, id);
                        ctx.close_popup(id);
                    }
                }
            }
        }

        /// What is on a popup and what closes with it has to go before their surfaces.
        fn forget_popup(&mut self, ctx: &mut Ctx<'_>, id: PopupId) {
            for child in ctx.popups_attached_to(id) {
                self.forget_popup(ctx, child);
            }
            if let Some(p) = &mut self.presenter {
                p.forget(ctx, SurfaceId::Popup(id));
            }
        }

        fn dump(&mut self, surface: SurfaceId, suffix: &str) {
            let Some(presenter) = &self.presenter else {
                return;
            };
            let Some(scene) = presenter.last_scene(surface) else {
                return;
            };
            if self.dump.is_none()
                && let Ok(path) = std::env::var("ANTIWEBUI_DUMP_FRAME")
            {
                let r = anti_webui_wgpu::Renderer::new(
                    &presenter.gpu().device,
                    anti_webui_wgpu::wgpu::TextureFormat::Rgba8Unorm,
                );
                self.dump = Some((path, r));
            }
            if let Some((path, renderer)) = &mut self.dump {
                renderer.set_target(match surface {
                    SurfaceId::Window => 0,
                    SurfaceId::Popup(id) => id.0,
                });
                let pixels = presenter.gpu().render_to_rgba(renderer, scene);
                let path = format!("{path}{suffix}");
                let _ =
                    crate::headless::write_png(path.as_str(), scene.width, scene.height, &pixels);
                eprintln!(
                    "[anti-webui] frame {}x{} scale {:.3} -> {path}",
                    scene.width,
                    scene.height,
                    self.ui.scale().as_f64()
                );
            }
        }
    }

    impl WaylandHandler for Runner {
        fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
            self.ui.handle_event(event);
            self.process(ctx);
        }

        fn frame(&mut self, ctx: &mut Ctx<'_>) {
            if self.presenter.is_none() {
                match Presenter::new(ctx) {
                    Ok(p) => self.presenter = Some(p),
                    Err(e) => {
                        self.error = Some(Box::new(e));
                        ctx.exit();
                        return;
                    }
                }
            }
            let now = Instant::now();
            self.ui.animate(now);
            self.ui.update();
            // Not `process`: the frame painted below is the redraw it would ask for.
            self.handle_requests(ctx);
            let frame = self.ui.paint_frame(now);
            let presenter = self.presenter.as_mut().unwrap();
            let window = presenter.present(ctx, SurfaceId::Window, frame.window);
            let mut results = vec![(SurfaceId::Window, window)];
            for (id, scene) in frame.popups {
                let surface = SurfaceId::Popup(id);
                results.push((surface, presenter.present(ctx, surface, scene)));
            }
            let mut committed = false;
            let mut retry = false;
            for (i, (surface, presented)) in results.into_iter().enumerate() {
                committed |= presented == Presented::Yes;
                retry |= presented == Presented::Retry;
                if presented == Presented::Yes {
                    let suffix = if i == 0 {
                        String::new()
                    } else {
                        format!(".popup{i}.png")
                    };
                    self.dump(surface, &suffix);
                }
            }
            // Nothing was committed, so no frame callback will pace the next try.
            if retry || (!committed && self.ui.animating()) {
                ctx.request_redraw_after(std::time::Duration::from_millis(4));
            } else if self.ui.animating() {
                ctx.request_redraw();
            }
        }

        fn deadline(&mut self, now: Instant) -> Option<Instant> {
            self.ui.next_deadline(now)
        }

        fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
            self.ui.tick(now);
            self.process(ctx);
        }

        fn exit(&mut self) {
            // GPU objects must go before the Wayland surface they render to.
            self.dump = None;
            self.presenter = None;
        }
    }

    pub fn run<V: IntoView>(
        app: App,
        root: impl FnOnce() -> V,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let runner = Runner {
            ui: Ui::new(root),
            presenter: None,
            error: None,
            dump: None,
        };
        let options = WindowOptions {
            title: app.title,
            app_id: app.app_id,
            width: app.width,
            height: app.height,
        };
        let mut runner = runner;
        anti_webui_wayland::run(options, &mut runner)?;
        match runner.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
