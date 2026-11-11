//! Presentation: our own buffers with damage tracking, or a wgpu surface as the fallback (see
//! `docs/design/05-rendering.md`). Our own buffers are dmabufs on Linux (`06-platform-wayland.md`
//! and `35-platform-x11.md`) and `IOSurface`s on macOS (`38-platform-macos.md`).

// Where there are no buffers of our own, what they would do can't be reached.
#![cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    allow(unreachable_code)
)]

#[cfg(target_os = "linux")]
mod dmabuf;
#[cfg(target_os = "macos")]
mod iosurface;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "present/no_dmabuf.rs"]
mod none;

#[cfg(target_os = "linux")]
use dmabuf::{DmabufPresenter as OwnPresenter, DmabufTarget as OwnTarget};
#[cfg(target_os = "macos")]
use iosurface::{IoSurfacePresenter as OwnPresenter, IoSurfaceTarget as OwnTarget};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use none::{DmabufPresenter as OwnPresenter, DmabufTarget as OwnTarget};

use crate::clock;
use crate::host::Host;
use crate::pacing::PresentTrace;
use anti_webui_core::damage::{self, Damage};
use anti_webui_core::platform::SurfaceId;
use anti_webui_core::scene::Scene;
use anti_webui_wgpu::{FrameResult, Gpu, GpuError, WindowRenderer};
use raw_window_handle::RawWindowHandle;
use std::collections::HashMap;
use std::rc::Rc;

/// Presents to the window and the popups. They share the GPU, and all but the fallback also the
/// renderer with its glyph atlas. The windows of a thread share the GPU and what makes our own
/// buffers (see `docs/design/36-multi-window.md`).
pub(crate) struct Presenter {
    // Dropped before `gpu`.
    targets: HashMap<SurfaceId, Target>,
    /// Surfaces of popup windows that the platform keeps for the next popups, by window. Making
    /// one takes 50 ms on Windows (see "ポップアップ" in `docs/design/39-platform-windows.md`).
    #[cfg(windows)]
    spares: Vec<(RawWindowHandle, WindowRenderer)>,
    /// One spare was made ahead of time, or tried; after that, popups that close leave theirs.
    #[cfg(windows)]
    spare_prepared: bool,
    own: Option<OwnPresenter>,
    gpu: Rc<Gpu>,
    debug: bool,
    /// Filled in while presenting to the window, if asked for.
    trace: Option<PresentTrace>,
}

/// What is kept per surface.
struct Target {
    kind: TargetKind,
    /// The window a GPU surface is on.
    #[cfg_attr(not(windows), allow(dead_code))]
    window: Option<RawWindowHandle>,
    /// The last presented scene, to diff the next one against.
    prev: Option<Scene>,
}

// There are few of them.
#[allow(clippy::large_enum_variant)]
enum TargetKind {
    Surface(WindowRenderer),
    Own(OwnTarget),
}

/// How long after the window's first frame a spare popup surface is made, if nothing else is drawn
/// before.
#[cfg(windows)]
const SPARE_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

fn debug_enabled() -> bool {
    std::env::var_os("ANTIWEBUI_DEBUG_DAMAGE").is_some_and(|v| v != "0")
}

fn renderer_target(surface: SurfaceId) -> u64 {
    match surface {
        SurfaceId::Window => 0,
        SurfaceId::Popup(id) => id.0,
        SurfaceId::DragIcon => u64::MAX,
    }
}

impl Presenter {
    pub fn new(ctx: &mut impl Host) -> Result<Self, GpuError> {
        let debug = debug_enabled();
        // Not where the platform can't make a surface, like for a window on a thread of its own.
        let force_surface = std::env::var("ANTIWEBUI_PRESENT").is_ok_and(|v| v == "surface")
            && ctx.raw_handles(SurfaceId::Window).is_some();
        if !force_surface {
            match OwnPresenter::new(ctx) {
                Ok((gpu, d)) => {
                    if debug {
                        eprintln!("[anti-webui] presenting our own buffers ({})", d.name());
                    }
                    return Ok(Self {
                        targets: HashMap::new(),
                        #[cfg(windows)]
                        spares: Vec::new(),
                        #[cfg(windows)]
                        spare_prepared: false,
                        own: Some(d),
                        gpu,
                        debug,
                        trace: None,
                    });
                }
                Err(e) => {
                    if debug {
                        eprintln!("[anti-webui] {e}; falling back to a wgpu surface");
                    }
                }
            }
        }
        let (gpu, window) = surface_gpu(ctx)?;
        if debug {
            let info = gpu.adapter.get_info();
            eprintln!(
                "[anti-webui] {} on {} ({:?}, {})",
                window.path(),
                info.name,
                info.backend,
                info.driver
            );
        }
        Ok(Self {
            targets: HashMap::from([(
                SurfaceId::Window,
                Target {
                    kind: TargetKind::Surface(window),
                    window: None,
                    prev: None,
                },
            )]),
            #[cfg(windows)]
            spares: Vec::new(),
            #[cfg(windows)]
            spare_prepared: false,
            own: None,
            gpu: Rc::new(gpu),
            debug,
            trace: None,
        })
    }

    /// How buffers get to the compositor, for reports.
    pub fn path(&self) -> &'static str {
        match &self.own {
            Some(d) => d.name(),
            None => match self.targets.get(&SurfaceId::Window).map(|t| &t.kind) {
                Some(TargetKind::Surface(w)) => w.path(),
                _ => "wgpu surface",
            },
        }
    }

    /// Whether a frame could be rendered and held back now. Only buffers we present ourselves can
    /// be, and looks at whether one is free so that no work on a frame goes to waste.
    pub fn can_hold_back(&mut self, surface: SurfaceId, limit: usize) -> bool {
        let Some(d) = &self.own else {
            return false;
        };
        match self.targets.get(&surface).map(|t| &t.kind) {
            Some(TargetKind::Own(t)) => d.can_hold_back(t, limit),
            _ => false,
        }
    }

    /// Commits the oldest frame held back. Returns what the frame was traced as, and when.
    pub fn commit_held(&mut self, ctx: &mut impl Host, surface: SurfaceId) -> Option<(u64, i64)> {
        let d = self.own.as_mut()?;
        let TargetKind::Own(t) = &mut self.targets.get_mut(&surface)?.kind else {
            return None;
        };
        let token = d.commit_held(ctx, surface, t)?;
        Some((token?, clock::monotonic_ns()))
    }

    /// Forgets frames held back, which no longer fit the window.
    pub fn drop_held(&mut self, surface: SurfaceId) {
        if let Some(TargetKind::Own(t)) = self.targets.get_mut(&surface).map(|t| &mut t.kind) {
            t.drop_held();
        }
    }

    /// Records the next presentation to the window.
    pub fn trace_next(&mut self, trace: PresentTrace) {
        self.trace = Some(trace);
    }

    pub fn take_trace(&mut self) -> Option<PresentTrace> {
        self.trace.take()
    }

    /// Whether the pixels of pictures are needed after this renderer has handed them to the GPU.
    pub fn keep_picture_pixels(&mut self, keep: bool) {
        if let Some(d) = &mut self.own {
            d.keep_picture_pixels(keep);
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The GPU that draws, if what decodes video can hand frames over on it.
    pub fn video_render_node(&self) -> Option<std::path::PathBuf> {
        self.own.as_ref()?.video_render_node()
    }

    /// Something for another renderer to read frames of videos on the GPU with.
    #[cfg(target_os = "linux")]
    pub fn video_importer(&self) -> Option<anti_webui_wgpu::dmabuf::VideoImporter> {
        self.own.as_ref()?.video_importer()
    }

    /// A frame of a video couldn't be read on the GPU, since this was last asked.
    pub fn take_video_import_failed(&mut self) -> bool {
        self.own
            .as_mut()
            .is_some_and(|d| d.take_video_import_failed())
    }

    /// The scene of the last presented frame of a surface.
    pub fn last_scene(&self, surface: SurfaceId) -> Option<&Scene> {
        self.targets.get(&surface)?.prev.as_ref()
    }

    /// Frees what is kept for a surface. To call before the surface goes.
    pub fn forget(&mut self, ctx: &mut impl Host, surface: SurfaceId) {
        let Some(target) = self.targets.remove(&surface) else {
            return;
        };
        // The platform keeps the window for the next popup.
        #[cfg(windows)]
        let target = match target {
            Target {
                kind: TargetKind::Surface(w),
                window: Some(window),
                ..
            } if matches!(surface, SurfaceId::Popup(_)) => {
                self.spares.push((window, w));
                return;
            }
            t => t,
        };
        if let Some(d) = &mut self.own {
            let target = match target.kind {
                TargetKind::Own(t) => Some(t),
                TargetKind::Surface(_) => None,
            };
            d.forget(ctx, renderer_target(surface), target);
        }
    }

    /// The surface kept for this popup window, if there is one.
    fn take_spare(&mut self, window: RawWindowHandle) -> Option<WindowRenderer> {
        #[cfg(windows)]
        if let Some(i) = self.spares.iter().position(|(w, _)| *w == window) {
            return Some(self.spares.swap_remove(i).1);
        }
        let _ = window;
        None
    }

    /// Makes a surface ahead of time for a window the platform keeps for the next popup, so that
    /// the first popup doesn't wait for one either.
    #[cfg(windows)]
    fn prepare_spare(&mut self, ctx: &mut impl Host) {
        if std::mem::replace(&mut self.spare_prepared, true) {
            return;
        }
        let Some((display, window)) = ctx.spare_popup_window() else {
            return;
        };
        // Room for most menus without making the swapchain again.
        const SIDE: u32 = 512;
        // SAFETY: the platform keeps the window until the presenter is gone.
        match unsafe { self.gpu.add_window(display, window, SIDE, SIDE) } {
            Ok(w) => self.spares.push((window, w)),
            Err(e) => eprintln!("[anti-webui] no GPU surface for a spare popup: {e}"),
        }
    }

    /// Presents the scene if it differs from the last one on this surface. Returns whether a
    /// buffer was committed, and whether the frame must be retried.
    pub fn present(&mut self, ctx: &mut impl Host, surface: SurfaceId, scene: Scene) -> Presented {
        if let Err(e) = self.update_buffers(ctx) {
            return self.failed(ctx, e);
        }
        if !self.targets.contains_key(&surface) {
            let mut window = None;
            let kind = match self.own.as_ref().map(OwnPresenter::new_target) {
                Some(t) => TargetKind::Own(t),
                None => {
                    let size = (scene.width.max(1), scene.height.max(1));
                    let created = match ctx.raw_handles(surface) {
                        Some((display, w)) => {
                            window = Some(w);
                            match self.take_spare(w) {
                                Some(spare) => Ok(spare),
                                // SAFETY: the surface outlives the target: see `forget`.
                                None => unsafe { self.gpu.add_window(display, w, size.0, size.1) },
                            }
                        }
                        None => Err(GpuError::NoSurface),
                    };
                    match created {
                        Ok(w) => TargetKind::Surface(w),
                        Err(e) => {
                            eprintln!("[anti-webui] no GPU surface for a popup: {e}");
                            return Presented::Unchanged;
                        }
                    }
                }
            };
            self.targets.insert(
                surface,
                Target {
                    kind,
                    window,
                    prev: None,
                },
            );
        }
        let target = self.targets.get_mut(&surface).expect("just inserted");
        if ctx.take_exposed(surface) {
            target.prev = None;
        }
        let damage = match &target.prev {
            Some(prev) => damage::diff(prev, &scene),
            None => Damage::full(scene.width, scene.height),
        };
        // A dropped frame was what `prev` is, so the compositor still has to get its changes.
        let unsent = matches!(&target.kind, TargetKind::Own(t) if t.has_unsent());
        if damage.is_empty() && !unsent {
            if self.debug {
                eprintln!("[anti-webui] {surface:?}: frame unchanged");
            }
            #[cfg(windows)]
            if surface == SurfaceId::Window {
                self.prepare_spare(ctx);
            }
            return Presented::Unchanged;
        }
        let mut trace = self.trace.as_mut().filter(|_| surface == SurfaceId::Window);
        if let Some(t) = &mut trace {
            t.diff_done = clock::monotonic_ns();
        }
        let result = match &mut target.kind {
            TargetKind::Surface(window) => {
                ctx.set_opaque(surface, scene.clear.a >= 1.0);
                if let Some(t) = &trace {
                    ctx.request_presentation(surface, t.token);
                }
                ctx.prepare_commit(surface);
                match window.present_damaged(&self.gpu, &scene, &damage) {
                    (FrameResult::Presented, drawn) => {
                        ctx.committed(surface);
                        if let Some(t) = &mut trace {
                            t.buffer_ready = t.diff_done;
                            t.recorded = t.diff_done;
                            let now = clock::monotonic_ns();
                            t.submitted = now;
                            t.committed = now;
                        }
                        Ok(Some(Stats {
                            age: window.last_buffer_age(),
                            repaint: drawn,
                            ..full_stats(&scene)
                        }))
                    }
                    (FrameResult::Skipped, _) => Ok(None),
                }
            }
            TargetKind::Own(t) => self
                .own
                .as_mut()
                .expect("own targets come with it")
                .present(
                    &self.gpu,
                    ctx,
                    surface,
                    t,
                    &scene,
                    &damage,
                    trace.as_deref_mut(),
                ),
        };
        match result {
            Ok(Some(stats)) => {
                if self.debug {
                    stats.log(surface, &scene, &damage);
                }
                if let Some(t) = &mut trace {
                    t.damage = damage.area();
                    t.repaint = stats.repaint;
                    t.pixels = scene.width as i64 * scene.height as i64;
                    t.buffers = stats.buffers;
                }
                let again = target.prev.is_some();
                target.prev = Some(scene);
                // Once the window is on the screen, rather than holding up its first frame: with
                // its next frame, or a moment after it if nothing changes.
                #[cfg(windows)]
                if surface == SurfaceId::Window {
                    if again {
                        self.prepare_spare(ctx);
                    } else {
                        ctx.request_redraw_after(SPARE_AFTER);
                    }
                }
                #[cfg(not(windows))]
                let _ = again;
                Presented::Yes
            }
            Ok(None) => {
                if self.debug {
                    eprintln!("[anti-webui] {surface:?}: no free buffer; retrying");
                }
                Presented::Retry
            }
            Err(e) => self.failed(ctx, e),
        }
    }

    fn failed(&mut self, ctx: &mut impl Host, e: GpuError) -> Presented {
        eprintln!("[anti-webui] presenting our own buffers failed ({e}); using a wgpu surface");
        self.fall_back(ctx);
        Presented::Retry
    }

    /// Takes in what happened to the buffers, on whichever surface.
    fn update_buffers(&mut self, ctx: &mut impl Host) -> Result<(), GpuError> {
        let Some(d) = &mut self.own else {
            return Ok(());
        };
        let targets = self.targets.values_mut().filter_map(|t| match &mut t.kind {
            TargetKind::Own(t) => Some(t),
            TargetKind::Surface(_) => None,
        });
        d.update(ctx, targets.collect())
    }

    fn fall_back(&mut self, ctx: &mut impl Host) {
        let mut targets = std::mem::take(&mut self.targets);
        if let Some(d) = &mut self.own {
            let owned = targets.values_mut().filter_map(|t| match &mut t.kind {
                TargetKind::Own(t) => Some(t),
                TargetKind::Surface(_) => None,
            });
            d.release(ctx, owned.collect());
        }
        drop(targets);
        ctx.disable_explicit_sync();
        match surface_gpu(ctx) {
            Ok((gpu, window)) => {
                // The old presenter goes before the GPU it was created on.
                self.own = None;
                self.targets.insert(
                    SurfaceId::Window,
                    Target {
                        kind: TargetKind::Surface(window),
                        window: None,
                        prev: None,
                    },
                );
                self.gpu = Rc::new(gpu);
            }
            Err(e) => {
                eprintln!("[anti-webui] no GPU surface either: {e}");
                ctx.exit();
            }
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let targets = std::mem::take(&mut self.targets);
        if let Some(d) = &mut self.own {
            let owned = targets.into_values().filter_map(|t| match t.kind {
                TargetKind::Own(t) => Some(t),
                TargetKind::Surface(_) => None,
            });
            d.retire(owned);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presented {
    Yes,
    /// Nothing changed; nothing was committed.
    Unchanged,
    /// Nothing was committed; try again on the next frame.
    Retry,
}

fn surface_gpu(ctx: &mut impl Host) -> Result<(Gpu, WindowRenderer), GpuError> {
    let (display, window) = ctx
        .raw_handles(SurfaceId::Window)
        .ok_or(GpuError::NoSurface)?;
    let size = ctx.buffer_size();
    // SAFETY: the display connection and the surface outlive the presenter.
    unsafe { Gpu::with_window(display, window, size.width as u32, size.height as u32) }
}

struct Stats {
    age: Option<u64>,
    repaint: i64,
    buffers: usize,
}

fn full_stats(scene: &Scene) -> Stats {
    Stats {
        age: None,
        repaint: scene.width as i64 * scene.height as i64,
        buffers: 0,
    }
}

impl Stats {
    fn log(&self, surface: SurfaceId, scene: &Scene, damage: &Damage) {
        let total = (scene.width as i64 * scene.height as i64).max(1) as f64;
        let age = self.age.map_or("-".to_string(), |a| a.to_string());
        eprintln!(
            "[anti-webui] {surface:?}: damage {} rect(s) {:.1}% | buffer age {age} | repaint {:.1}% | buffers {}",
            damage.rects().len(),
            damage.area() as f64 * 100.0 / total,
            self.repaint as f64 * 100.0 / total,
            self.buffers,
        );
    }
}
