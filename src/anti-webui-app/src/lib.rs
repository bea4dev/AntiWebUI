//! Wires a platform backend, the wgpu renderer and the core together.

pub mod headless;

use anti_webui_core::view::IntoView;

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

    /// Opens the window and runs until it is closed.
    pub fn run<V: IntoView>(
        self,
        root: impl FnOnce() -> V,
    ) -> Result<(), Box<dyn std::error::Error>> {
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

#[cfg(target_os = "linux")]
mod wayland {
    use super::App;
    use anti_webui_core::Ui;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::platform::PlatformRequest;
    use anti_webui_core::view::IntoView;
    use anti_webui_wayland::{Ctx, WaylandHandler, WindowOptions};
    use anti_webui_wgpu::{FrameResult, Gpu, WindowRenderer};
    use std::time::Instant;

    struct Runner {
        ui: Ui,
        gpu: Option<(Gpu, WindowRenderer)>,
        error: Option<Box<dyn std::error::Error>>,
        /// `ANTIWEBUI_DUMP_FRAME=path.png` writes every presented frame to a file (debugging).
        dump: Option<(String, anti_webui_wgpu::Renderer)>,
    }

    impl Runner {
        fn process(&mut self, ctx: &mut Ctx<'_>) {
            for req in self.ui.take_requests() {
                match req {
                    PlatformRequest::SetCursor(c) => ctx.set_cursor(c),
                    PlatformRequest::SetIme(state) => ctx.set_ime(state),
                    PlatformRequest::SetClipboard { kind, text } => ctx.set_clipboard(kind, text),
                    PlatformRequest::Paste(kind) => ctx.request_paste(kind),
                    PlatformRequest::SetTitle(t) => ctx.set_title(&t),
                    PlatformRequest::Close => ctx.exit(),
                }
            }
            if self.ui.needs_redraw() {
                ctx.request_redraw();
            }
        }
    }

    impl WaylandHandler for Runner {
        fn event(&mut self, ctx: &mut Ctx<'_>, event: WindowEvent) {
            self.ui.handle_event(event);
            self.process(ctx);
        }

        fn frame(&mut self, ctx: &mut Ctx<'_>) -> bool {
            if self.gpu.is_none() {
                let (display, window) = ctx.raw_handles();
                let size = ctx.buffer_size();
                // SAFETY: the Wayland connection and surface outlive the runner.
                match unsafe {
                    Gpu::with_window(display, window, size.width as u32, size.height as u32)
                } {
                    Ok(g) => self.gpu = Some(g),
                    Err(e) => {
                        self.error = Some(Box::new(e));
                        ctx.exit();
                        return false;
                    }
                }
            }
            self.ui.update();
            self.process(ctx);
            let scene = self.ui.paint(Instant::now());
            let (gpu, window) = self.gpu.as_mut().unwrap();
            let presented = window.present(gpu, &scene) == FrameResult::Presented;
            if presented {
                if self.dump.is_none()
                    && let Ok(path) = std::env::var("ANTIWEBUI_DUMP_FRAME")
                {
                    let r = anti_webui_wgpu::Renderer::new(
                        &gpu.device,
                        anti_webui_wgpu::wgpu::TextureFormat::Rgba8Unorm,
                    );
                    self.dump = Some((path, r));
                }
                if let Some((path, renderer)) = &mut self.dump {
                    let pixels = gpu.render_to_rgba(renderer, &scene);
                    let _ = crate::headless::write_png(
                        path.as_str(),
                        scene.width,
                        scene.height,
                        &pixels,
                    );
                    eprintln!(
                        "[anti-webui] frame {}x{} scale {:.3} -> {path}",
                        scene.width,
                        scene.height,
                        self.ui.scale().as_f64()
                    );
                }
            }
            if !presented {
                ctx.request_redraw();
            }
            presented
        }

        fn deadline(&mut self, now: Instant) -> Option<Instant> {
            self.ui.next_deadline(now)
        }

        fn timer(&mut self, ctx: &mut Ctx<'_>, now: Instant) {
            self.ui.tick(now);
            self.process(ctx);
        }

        fn exit(&mut self) {
            // The GPU surface must go before the Wayland surface it renders to.
            self.dump = None;
            self.gpu = None;
        }
    }

    pub fn run<V: IntoView>(
        app: App,
        root: impl FnOnce() -> V,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let runner = Runner {
            ui: Ui::new(root),
            gpu: None,
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
