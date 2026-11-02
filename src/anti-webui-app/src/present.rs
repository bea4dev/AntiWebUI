//! Presentation on Wayland: our own dmabuf buffers with damage tracking, or a wgpu surface as
//! the fallback (see `docs/design/05-rendering.md` and `06-platform-wayland.md`).

use anti_webui_core::damage::{self, Damage, DamageHistory};
use anti_webui_core::scene::Scene;
use anti_webui_wayland::{BufferEvent, BufferId, Ctx, DmabufPlaneDesc, SyncPoints, TimelineId};
use anti_webui_wgpu::dmabuf::{DRM_FORMAT_ARGB8888, DmabufContext, DmabufImage};
use anti_webui_wgpu::linux_sync::{self, DrmDevice};
use anti_webui_wgpu::{FrameResult, Gpu, GpuError, Renderer, WindowRenderer, wgpu};
use std::os::fd::AsFd;

/// Buffers beyond this make the compositor wait for us instead.
const MAX_BUFFERS: usize = 4;

pub(crate) struct Presenter {
    // Dropped before `gpu`.
    backend: Backend,
    gpu: Gpu,
    /// The last presented scene, to diff the next one against.
    prev: Option<Scene>,
    debug: bool,
}

// There is one per window.
#[allow(clippy::large_enum_variant)]
enum Backend {
    Surface(WindowRenderer),
    Dmabuf(DmabufPresenter),
}

fn debug_enabled() -> bool {
    std::env::var_os("ANTIWEBUI_DEBUG_DAMAGE").is_some_and(|v| v != "0")
}

impl Presenter {
    pub fn new(ctx: &mut Ctx<'_>) -> Result<Self, GpuError> {
        let debug = debug_enabled();
        let force_surface = std::env::var("ANTIWEBUI_PRESENT").is_ok_and(|v| v == "surface");
        if !force_surface {
            match DmabufPresenter::new(ctx) {
                Ok((gpu, d)) => {
                    if debug {
                        eprintln!("[anti-webui] presenting dmabufs ({} sync)", d.sync.name());
                    }
                    return Ok(Self {
                        backend: Backend::Dmabuf(d),
                        gpu,
                        prev: None,
                        debug,
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
        Ok(Self {
            backend: Backend::Surface(window),
            gpu,
            prev: None,
            debug,
        })
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The scene of the last presented frame.
    pub fn last_scene(&self) -> Option<&Scene> {
        self.prev.as_ref()
    }

    /// Presents the scene if it differs from the last one. Returns whether a buffer was
    /// committed, and whether the frame must be retried.
    pub fn present(&mut self, ctx: &mut Ctx<'_>, scene: Scene) -> Presented {
        let damage = match &self.prev {
            Some(prev) => damage::diff(prev, &scene),
            None => Damage::full(scene.width, scene.height),
        };
        if damage.is_empty() {
            if self.debug {
                eprintln!("[anti-webui] frame unchanged");
            }
            return Presented::Unchanged;
        }
        ctx.set_opaque(scene.clear.a >= 1.0);
        let result = match &mut self.backend {
            Backend::Surface(window) => match window.present(&self.gpu, &scene) {
                FrameResult::Presented => Ok(Some(full_stats(&scene))),
                FrameResult::Skipped => Ok(None),
            },
            Backend::Dmabuf(d) => d.present(&self.gpu, ctx, &scene, &damage),
        };
        match result {
            Ok(Some(stats)) => {
                if self.debug {
                    stats.log(&scene, &damage);
                }
                self.prev = Some(scene);
                Presented::Yes
            }
            Ok(None) => {
                if self.debug {
                    eprintln!("[anti-webui] no free buffer; retrying");
                }
                Presented::Retry
            }
            Err(e) => {
                eprintln!("[anti-webui] dmabuf presentation failed ({e}); using a wgpu surface");
                self.fall_back(ctx);
                Presented::Retry
            }
        }
    }

    fn fall_back(&mut self, ctx: &mut Ctx<'_>) {
        if let Backend::Dmabuf(d) = &mut self.backend {
            d.release_protocol_objects(ctx);
        }
        ctx.disable_explicit_sync();
        match surface_gpu(ctx) {
            Ok((gpu, window)) => {
                // The old backend goes before the GPU it was created on.
                self.backend = Backend::Surface(window);
                self.gpu = gpu;
                self.prev = None;
            }
            Err(e) => {
                eprintln!("[anti-webui] no GPU surface either: {e}");
                ctx.exit();
            }
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

fn surface_gpu(ctx: &mut Ctx<'_>) -> Result<(Gpu, WindowRenderer), GpuError> {
    let (display, window) = ctx.raw_handles();
    let size = ctx.buffer_size();
    // SAFETY: the Wayland connection and surface outlive the presenter.
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
    fn log(&self, scene: &Scene, damage: &Damage) {
        let total = (scene.width as i64 * scene.height as i64).max(1) as f64;
        let age = self.age.map_or("-".to_string(), |a| a.to_string());
        eprintln!(
            "[anti-webui] damage {} rect(s) {:.1}% | buffer age {age} | repaint {:.1}% | buffers {}",
            damage.rects().len(),
            damage.area() as f64 * 100.0 / total,
            self.repaint as f64 * 100.0 / total,
            self.buffers,
        );
    }
}

// ---------------------------------------------------------------------------------------------

struct Slot {
    image: DmabufImage,
    buffer: BufferId,
    /// The frame last rendered into it; 0 if none.
    frame: u64,
    /// Committed and not yet released by the compositor (implicit sync).
    busy: bool,
    /// Explicit sync: the timeline the compositor signals when done with this buffer.
    release: Option<ReleaseTimeline>,
}

struct ReleaseTimeline {
    handle: u32,
    id: TimelineId,
    point: u64,
}

enum Sync {
    Explicit {
        drm: DrmDevice,
        acquire: u32,
        acquire_id: TimelineId,
        point: u64,
        /// A binary syncobj used to move sync files onto timeline points.
        scratch: u32,
    },
    Implicit,
}

impl Sync {
    fn name(&self) -> &'static str {
        match self {
            Sync::Explicit { .. } => "explicit",
            Sync::Implicit => "implicit",
        }
    }
}

struct DmabufPresenter {
    slots: Vec<Slot>,
    ctx: DmabufContext,
    renderer: Renderer,
    sync: Sync,
    history: DamageHistory,
    /// Frames presented so far.
    frame: u64,
    size: (u32, u32),
    generation: u64,
    modifiers: Vec<u64>,
}

impl DmabufPresenter {
    fn new(ctx: &mut Ctx<'_>) -> Result<(Gpu, Self), GpuError> {
        let feedback = ctx
            .dmabuf_feedback()
            .ok_or_else(|| GpuError::Dmabuf("the compositor sent no dmabuf feedback".into()))?;
        let modifiers = feedback.modifiers(DRM_FORMAT_ARGB8888);
        let generation = feedback.generation;
        let (gpu, dctx) = Gpu::for_dmabuf(feedback.main_device)?;
        if !dctx.supports_any(&modifiers) {
            return Err(GpuError::Dmabuf(
                "no ARGB8888 modifier shared with the compositor".into(),
            ));
        }
        let sync = match setup_explicit_sync(ctx, &dctx) {
            Some(s) => s,
            None => {
                ctx.disable_explicit_sync();
                Sync::Implicit
            }
        };
        let renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
        Ok((
            gpu,
            Self {
                slots: Vec::new(),
                ctx: dctx,
                renderer,
                sync,
                history: DamageHistory::default(),
                frame: 0,
                size: (0, 0),
                generation,
                modifiers,
            },
        ))
    }

    /// Renders and commits a frame. `Ok(None)` means no buffer was free.
    fn present(
        &mut self,
        gpu: &Gpu,
        ctx: &mut Ctx<'_>,
        scene: &Scene,
        damage: &Damage,
    ) -> Result<Option<Stats>, GpuError> {
        for event in ctx.take_buffer_events() {
            match event {
                BufferEvent::Failed(_) => {
                    return Err(GpuError::Dmabuf("the compositor rejected a buffer".into()));
                }
                BufferEvent::Released(id) => {
                    if let Some(s) = self.slots.iter_mut().find(|s| s.buffer == id) {
                        s.busy = false;
                    }
                }
            }
        }
        if let Some(f) = ctx.dmabuf_feedback()
            && f.generation != self.generation
        {
            self.generation = f.generation;
            let modifiers = f.modifiers(DRM_FORMAT_ARGB8888);
            if modifiers != self.modifiers {
                self.modifiers = modifiers;
                self.drop_slots(ctx);
            }
        }
        let size = (scene.width.max(1), scene.height.max(1));
        if size != self.size {
            self.size = size;
            self.drop_slots(ctx);
            self.history.clear();
        }

        let Some(index) = self.free_slot(gpu, ctx)? else {
            return Ok(None);
        };
        let frame = self.frame + 1;
        let slot = &mut self.slots[index];
        let age = (slot.frame > 0).then(|| frame - slot.frame);
        let repaint = match age {
            Some(1) => Some(damage.clone()),
            Some(age) => self.history.last(age as usize - 1).map(|mut d| {
                d.extend(damage);
                d
            }),
            None => None,
        }
        .unwrap_or_else(|| Damage::full(size.0, size.1));

        let wait = match self.sync {
            // Readers the compositor queued on the dmabuf before releasing it.
            Sync::Implicit if slot.frame > 0 => Some(
                linux_sync::dmabuf_export_write_fence(slot.image.fd())
                    .map_err(|e| GpuError::Dmabuf(format!("exporting a fence: {e}")))?,
            ),
            _ => None,
        };
        self.ctx.begin_access(&mut slot.image, wait)?;
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &slot.image.view,
            scene,
            &repaint,
        );
        gpu.queue.submit([encoder.finish()]);
        let done = self.ctx.end_access(&mut slot.image)?;

        let sync = match &mut self.sync {
            Sync::Explicit {
                drm,
                acquire,
                acquire_id,
                point,
                scratch,
            } => {
                *point += 1;
                drm.set_point(*acquire, *point, *scratch, done.as_ref().map(|f| f.as_fd()))
                    .map_err(|e| GpuError::Dmabuf(format!("setting the acquire point: {e}")))?;
                let release = slot.release.as_mut().expect("explicit sync slot");
                release.point += 1;
                Some(SyncPoints {
                    acquire: (*acquire_id, *point),
                    release: (release.id, release.point),
                })
            }
            Sync::Implicit => {
                if let Some(f) = &done {
                    linux_sync::dmabuf_import_write_fence(slot.image.fd(), f.as_fd())
                        .map_err(|e| GpuError::Dmabuf(format!("attaching a fence: {e}")))?;
                }
                None
            }
        };
        ctx.present_buffer(slot.buffer, damage.rects(), sync);
        slot.busy = true;
        slot.frame = frame;
        self.frame = frame;
        self.history.push(damage.clone());
        Ok(Some(Stats {
            age,
            repaint: repaint.area(),
            buffers: self.slots.len(),
        }))
    }

    /// The free buffer holding the most recent frame, allocating one if none is free.
    fn free_slot(&mut self, gpu: &Gpu, ctx: &mut Ctx<'_>) -> Result<Option<usize>, GpuError> {
        let free = |s: &Slot, sync: &Sync, deadline: i64| -> bool {
            match (sync, &s.release) {
                (Sync::Explicit { drm, .. }, Some(r)) => {
                    s.frame == 0 || drm.wait_point(r.handle, r.point, deadline).unwrap_or(false)
                }
                _ => !s.busy,
            }
        };
        let best = (0..self.slots.len())
            .filter(|&i| free(&self.slots[i], &self.sync, 0))
            .max_by_key(|&i| self.slots[i].frame);
        if best.is_some() {
            return Ok(best);
        }
        if self.slots.len() < MAX_BUFFERS {
            return self.allocate(gpu, ctx).map(Some);
        }
        // All buffers are with the compositor: briefly wait for the oldest (explicit sync).
        let oldest = (0..self.slots.len()).min_by_key(|&i| self.slots[i].frame);
        if let (Some(i), Sync::Explicit { .. }) = (oldest, &self.sync) {
            let deadline = linux_sync::monotonic_ns() + 50_000_000;
            if free(&self.slots[i], &self.sync, deadline) {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    fn allocate(&mut self, gpu: &Gpu, ctx: &mut Ctx<'_>) -> Result<usize, GpuError> {
        let (w, h) = self.size;
        let image = self.ctx.allocate(gpu, w, h, &self.modifiers)?;
        let planes: Vec<DmabufPlaneDesc<'_>> = image
            .planes
            .iter()
            .map(|p| DmabufPlaneDesc {
                fd: image.fd(),
                offset: p.offset,
                stride: p.stride,
            })
            .collect();
        let buffer = ctx
            .create_dmabuf_buffer(w, h, image.format, image.modifier, &planes)
            .ok_or_else(|| GpuError::Dmabuf("linux-dmabuf is unavailable".into()))?;
        drop(planes);
        let release = match &self.sync {
            Sync::Explicit { drm, .. } => {
                let timeline = (|| {
                    let handle = drm.create_syncobj().ok()?;
                    let fd = drm.export_syncobj(handle).ok()?;
                    let id = ctx.import_timeline(fd.as_fd())?;
                    Some(ReleaseTimeline {
                        handle,
                        id,
                        point: 0,
                    })
                })();
                Some(timeline.ok_or_else(|| {
                    GpuError::Dmabuf("could not create a release timeline".into())
                })?)
            }
            Sync::Implicit => None,
        };
        self.slots.push(Slot {
            image,
            buffer,
            frame: 0,
            busy: false,
            release,
        });
        Ok(self.slots.len() - 1)
    }

    fn drop_slots(&mut self, ctx: &mut Ctx<'_>) {
        for slot in self.slots.drain(..) {
            ctx.destroy_buffer(slot.buffer);
            if let Some(r) = slot.release {
                ctx.destroy_timeline(r.id);
                if let Sync::Explicit { drm, .. } = &self.sync {
                    drm.destroy_syncobj(r.handle);
                }
            }
            self.ctx.retire(slot.image);
        }
    }

    fn release_protocol_objects(&mut self, ctx: &mut Ctx<'_>) {
        self.drop_slots(ctx);
        if let Sync::Explicit { acquire_id, .. } = &self.sync {
            ctx.destroy_timeline(*acquire_id);
        }
    }
}

impl Drop for DmabufPresenter {
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            self.ctx.retire(slot.image);
        }
    }
}

fn setup_explicit_sync(ctx: &mut Ctx<'_>, dctx: &DmabufContext) -> Option<Sync> {
    if std::env::var("ANTIWEBUI_SYNC").is_ok_and(|v| v == "implicit")
        || !ctx.explicit_sync_available()
    {
        return None;
    }
    let drm = DrmDevice::open(dctx.render_node()?).ok()?;
    let acquire = drm.create_syncobj().ok()?;
    let scratch = drm.create_syncobj().ok()?;
    let fd = drm.export_syncobj(acquire).ok()?;
    let acquire_id = ctx.import_timeline(fd.as_fd())?;
    Some(Sync::Explicit {
        drm,
        acquire,
        acquire_id,
        point: 0,
        scratch,
    })
}
