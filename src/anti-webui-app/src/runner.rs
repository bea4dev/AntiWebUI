//! Runs the UI on whichever backend there is: Wayland or X11 (see "バックエンドの選び方" in
//! `docs/design/35-platform-x11.md`).

use super::App;
use crate::frame_stats::FrameStats;
use crate::host::Host;
use crate::pacing::Pacing;
use crate::perf_overlay::{LAST_WORD, OverlayFeed};
use crate::present::{Presented, Presenter};
use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::platform::{PlatformRequest, PopupId, SurfaceId};
use anti_webui_core::video::VideoTiming;
use anti_webui_core::view::IntoView;
use std::time::Instant;

struct Runner {
    ui: Ui,
    presenter: Option<Presenter>,
    error: Option<Box<dyn std::error::Error>>,
    /// `ANTIWEBUI_DUMP_FRAME=path.png` writes every presented frame to a file (debugging).
    dump: Option<(String, anti_webui_wgpu::Renderer)>,
    /// `ANTIWEBUI_FRAME_STATS=path.csv` records what every frame took (see `frame_stats`).
    stats: Option<FrameStats>,
    /// What frames take, over the window (see `perf_overlay`).
    overlay: OverlayFeed,
    pacing: Pacing,
    /// Decoded pictures say so through the event loop.
    waker_set: bool,
}

impl Runner {
    fn set_waker(&mut self, ctx: &mut impl Host) {
        if !std::mem::replace(&mut self.waker_set, true)
            && let Some(waker) = ctx.waker()
        {
            self.ui.set_image_waker(waker);
            self.ui.set_platform_drag(ctx.supports_drag());
        }
    }

    fn process(&mut self, ctx: &mut impl Host) {
        self.set_waker(ctx);
        self.handle_requests(ctx);
        if self.ui.needs_redraw() {
            ctx.request_redraw();
        }
    }

    fn handle_requests(&mut self, ctx: &mut impl Host) {
        for req in self.ui.take_requests() {
            match req {
                PlatformRequest::SetCursor(c) => ctx.set_cursor(c),
                PlatformRequest::SetIme(state) => ctx.set_ime(state),
                PlatformRequest::SetClipboard { kind, text } => ctx.set_clipboard(kind, text),
                PlatformRequest::Paste(kind) => ctx.request_paste(kind),
                PlatformRequest::SetTitle(t) => ctx.set_title(&t),
                PlatformRequest::SetFullscreen(on) => ctx.set_fullscreen(on),
                PlatformRequest::Close => ctx.exit(),
                PlatformRequest::ShowPopup(params) => ctx.show_popup(params),
                PlatformRequest::UpdatePopup(params) => ctx.update_popup(params),
                PlatformRequest::ClosePopup(id) => {
                    self.forget_popup(ctx, id);
                    ctx.close_popup(id);
                }
                PlatformRequest::StartDrag(offer) => ctx.start_drag(offer),
                PlatformRequest::CancelDrag => ctx.cancel_drag(),
                PlatformRequest::CloseDragIcon => {
                    if let Some(p) = &mut self.presenter {
                        p.forget(ctx, SurfaceId::DragIcon);
                    }
                    ctx.close_drag_icon();
                }
                PlatformRequest::AcceptDrag(wanted) => ctx.accept_drag(wanted),
                PlatformRequest::ReceiveDrop(mime) => ctx.receive_drop(mime),
                PlatformRequest::FinishDrop => ctx.finish_drop(),
            }
        }
    }

    /// What is on a popup and what closes with it has to go before their surfaces.
    fn forget_popup(&mut self, ctx: &mut impl Host, id: PopupId) {
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
            let mut r = anti_webui_wgpu::Renderer::new(
                &presenter.gpu().device,
                anti_webui_wgpu::wgpu::TextureFormat::Rgba8Unorm,
            );
            if let Some(importer) = presenter.video_importer() {
                r.set_video_importer(importer);
            }
            self.dump = Some((path, r));
        }
        if let Some((path, renderer)) = &mut self.dump {
            renderer.set_target(match surface {
                SurfaceId::Window => 0,
                SurfaceId::Popup(id) => id.0,
                SurfaceId::DragIcon => u64::MAX,
            });
            let pixels = presenter.gpu().render_to_rgba(renderer, scene);
            let path = format!("{path}{suffix}");
            let _ = crate::headless::write_png(path.as_str(), scene.width, scene.height, &pixels);
            eprintln!(
                "[anti-webui] frame {}x{} scale {:.3} -> {path}",
                scene.width,
                scene.height,
                self.ui.scale().as_f64()
            );
        }
    }

    fn on_event(&mut self, ctx: &mut impl Host, event: WindowEvent) {
        if let (WindowEvent::Resized { .. }, Some(p)) = (&event, &mut self.presenter) {
            p.drop_held(SurfaceId::Window);
        }
        self.ui.handle_event(event);
        self.process(ctx);
    }

    fn on_woken(&mut self, ctx: &mut impl Host) {
        self.ui.poll_images();
        self.process(ctx);
    }

    fn on_frame(&mut self, ctx: &mut impl Host) {
        self.set_waker(ctx);
        if self.presenter.is_none() {
            match Presenter::new(ctx) {
                Ok(mut p) => {
                    // What writes frames to files draws them again, with its own renderer.
                    p.keep_picture_pixels(std::env::var_os("ANTIWEBUI_DUMP_FRAME").is_some());
                    self.ui.set_video_render_node(p.video_render_node());
                    self.presenter = Some(p);
                }
                Err(e) => {
                    self.error = Some(Box::new(e));
                    ctx.exit();
                    return;
                }
            }
        }
        self.overlay.update();
        let own = self.overlay.is_own_frame(self.ui.needs_redraw());
        let (stats, overlay) = (&mut self.stats, &mut self.overlay);
        self.pacing.begin_frame(ctx, |frame| {
            if let Some(s) = stats {
                s.add(frame);
            }
            overlay.add(frame);
        });
        let now = self.pacing.frame_time();
        self.ui
            .set_video_timing(self.pacing.refresh().map(|refresh| VideoTiming {
                refresh,
                vsync: self.pacing.last_refresh(),
            }));
        self.ui.animate(now);
        self.ui.update();
        // Not `process`: the frame painted below is the redraw it would ask for.
        self.handle_requests(ctx);
        let mut frame = self.ui.paint_frame(now);
        let trace = self.pacing.trace();
        self.overlay.paint(
            &mut frame.window,
            self.ui.scale(),
            self.pacing.advance_words(),
            trace.token,
            own,
        );
        let presenter = self.presenter.as_mut().unwrap();
        presenter.trace_next(trace);
        let window = presenter.present(ctx, SurfaceId::Window, frame.window);
        if presenter.take_video_import_failed() {
            self.ui.video_use_memory();
        }
        let mut results = vec![(SurfaceId::Window, window)];
        for (id, scene) in frame.popups {
            let surface = SurfaceId::Popup(id);
            results.push((surface, presenter.present(ctx, surface, scene)));
        }
        if let Some(icon) = frame.drag_icon {
            ctx.place_drag_icon(icon.logical, icon.hotspot);
            let surface = SurfaceId::DragIcon;
            results.push((surface, presenter.present(ctx, surface, icon.scene)));
        }
        if let Some(trace) = presenter.take_trace() {
            let feedback = ctx.presentation_clock().is_some();
            self.pacing
                .end_frame(trace, results[0].1 == Presented::Yes, feedback);
        }
        if let Some(stats) = &mut self.stats {
            stats.describe(presenter.gpu(), presenter.path());
        }
        self.overlay.describe(presenter.gpu(), presenter.path());
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
        let soon = std::time::Duration::from_millis(4);
        if retry {
            ctx.request_redraw_after(soon);
        } else if !committed && self.ui.animating() {
            // Nothing to show this time: it may have something at the next refresh.
            let wait = self.pacing.until_next_refresh(ctx, Instant::now());
            ctx.request_redraw_after(wait.unwrap_or(soon));
        } else if self.ui.animating() {
            ctx.request_redraw();
        } else if committed && self.overlay.wants_last_word(own) {
            // The last frames are still on their way to the screen.
            ctx.request_redraw_after(LAST_WORD);
        }
    }

    fn on_surfaces_ready(&mut self, ctx: &mut impl Host) {
        if let Some(p) = &mut self.presenter
            && let Some((token, time)) = p.commit_held(ctx, SurfaceId::Window)
        {
            self.pacing.committed(token, time);
        }
    }

    fn on_may_render_ahead(&mut self, ctx: &mut impl Host) -> bool {
        // Popups show what was just asked for, and keep in step with the window.
        !ctx.has_popups()
            && ctx.frame_pending(SurfaceId::Window)
            && self
                .presenter
                .as_mut()
                .is_some_and(|p| p.can_hold_back(SurfaceId::Window, self.pacing.max_advance()))
    }

    fn on_timer(&mut self, ctx: &mut impl Host, now: Instant) {
        self.ui.tick(now);
        self.process(ctx);
    }

    fn on_exit(&mut self) {
        if let Some(mut stats) = self.stats.take() {
            self.pacing.drain(&mut |frame| stats.add(frame));
            stats.finish();
        }
        // GPU objects must go before the surface they render to.
        self.dump = None;
        self.presenter = None;
    }
}

/// The backends' handler traits have the same methods; each is answered by the runner's.
macro_rules! handler_for {
    ($handler:path, $ctx:ty) => {
        impl $handler for Runner {
            fn event(&mut self, ctx: &mut $ctx, event: WindowEvent) {
                self.on_event(ctx, event)
            }
            fn frame(&mut self, ctx: &mut $ctx) {
                self.on_frame(ctx)
            }
            fn deadline(&mut self, now: Instant) -> Option<Instant> {
                self.ui.next_deadline(now)
            }
            fn timer(&mut self, ctx: &mut $ctx, now: Instant) {
                self.on_timer(ctx, now)
            }
            fn exit(&mut self) {
                self.on_exit()
            }
            fn surfaces_ready(&mut self, ctx: &mut $ctx) {
                self.on_surfaces_ready(ctx)
            }
            fn may_render_ahead(&mut self, ctx: &mut $ctx) -> bool {
                self.on_may_render_ahead(ctx)
            }
            fn woken(&mut self, ctx: &mut $ctx) {
                self.on_woken(ctx)
            }
        }
    };
}

#[cfg(feature = "wayland")]
handler_for!(
    anti_webui_wayland::WaylandHandler,
    anti_webui_wayland::Ctx<'_>
);
#[cfg(feature = "x11")]
handler_for!(anti_webui_x11::X11Handler, anti_webui_x11::Ctx<'_>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    #[cfg(feature = "wayland")]
    Wayland,
    #[cfg(feature = "x11")]
    X11,
}

/// `ANTIWEBUI_BACKEND` if given, otherwise Wayland if it can be reached, and X11 after it.
fn choose_backend() -> Result<Backend, Box<dyn std::error::Error>> {
    let wanted = std::env::var("ANTIWEBUI_BACKEND").ok();
    match wanted.as_deref() {
        #[cfg(feature = "wayland")]
        Some("wayland") => return Ok(Backend::Wayland),
        #[cfg(feature = "x11")]
        Some("x11") => return Ok(Backend::X11),
        Some(other) if !other.is_empty() => {
            return Err(format!("ANTIWEBUI_BACKEND={other} isn't built in").into());
        }
        _ => {}
    }
    #[cfg(feature = "wayland")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && anti_webui_wayland::can_connect() {
        return Ok(Backend::Wayland);
    }
    #[cfg(feature = "x11")]
    if std::env::var_os("DISPLAY").is_some() {
        return Ok(Backend::X11);
    }
    #[cfg(feature = "wayland")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        // `WAYLAND_DISPLAY` may be left out for the default socket.
        return Ok(Backend::Wayland);
    }
    Err("found neither a Wayland compositor nor an X server".into())
}

/// Before any thread of the app's: it sets the environment of the process.
fn limit_vulkan_drivers(device: Option<u64>) {
    if let Some(device) = device {
        // SAFETY: the app's views, and with them its threads, aren't made yet.
        let drivers = unsafe { anti_webui_wgpu::limit_vulkan_drivers(device) };
        if let Some(d) = drivers {
            log::debug!("Vulkan drivers of the display server's GPU only: {d}");
        }
    }
}

pub fn run<V: IntoView>(
    app: App,
    root: impl FnOnce() -> V,
) -> Result<(), Box<dyn std::error::Error>> {
    let backend = choose_backend()?;
    limit_vulkan_drivers(match backend {
        #[cfg(feature = "wayland")]
        Backend::Wayland => anti_webui_wayland::compositor_main_device(),
        #[cfg(feature = "x11")]
        Backend::X11 => anti_webui_x11::server_main_device(),
    });
    #[cfg(feature = "video")]
    anti_webui_video::install();
    let mut runner = Runner {
        ui: Ui::new(root),
        presenter: None,
        error: None,
        dump: None,
        stats: FrameStats::from_env(),
        overlay: OverlayFeed::new(),
        pacing: Pacing::new(),
        waker_set: false,
    };
    match backend {
        #[cfg(feature = "wayland")]
        Backend::Wayland => {
            let options = anti_webui_wayland::WindowOptions {
                title: app.title,
                app_id: app.app_id,
                width: app.width,
                height: app.height,
                fullscreen: app.fullscreen,
            };
            anti_webui_wayland::run(options, &mut runner)?;
        }
        #[cfg(feature = "x11")]
        Backend::X11 => {
            let options = anti_webui_x11::WindowOptions {
                title: app.title,
                app_id: app.app_id,
                width: app.width,
                height: app.height,
                fullscreen: app.fullscreen,
            };
            anti_webui_x11::run(options, &mut runner)?;
        }
    }
    match runner.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
