//! Presentation: our own dmabuf buffers with damage tracking, or a wgpu surface as the fallback
//! (see `docs/design/05-rendering.md`, `06-platform-wayland.md` and `35-platform-x11.md`).

use crate::host::Host;
use crate::pacing::PresentTrace;
use anti_webui_core::damage::{self, Damage, DamageHistory};
use anti_webui_core::platform::{
    BufferEvent, BufferId, DmabufFeedback, DmabufPlaneDesc, SurfaceId, SyncPoints, TimelineId,
};
use anti_webui_core::scene::Scene;
use anti_webui_wgpu::dmabuf::{DRM_FORMAT_ARGB8888, DmabufContext, DmabufImage};
use anti_webui_wgpu::linux_sync::{self, DrmDevice};
use anti_webui_wgpu::{FrameResult, Gpu, GpuError, Renderer, WindowRenderer, wgpu};
use std::collections::{HashMap, VecDeque};
use std::os::fd::AsFd;

/// Buffers beyond this make the compositor wait for us instead. Frames held back use one more each.
const MAX_BUFFERS: usize = 4;

/// Presents to the window and the popups. They share the GPU, and all but the fallback also the
/// renderer with its glyph atlas.
pub(crate) struct Presenter {
    // Dropped before `gpu`.
    targets: HashMap<SurfaceId, Target>,
    dmabuf: Option<DmabufPresenter>,
    gpu: Gpu,
    debug: bool,
    /// Filled in while presenting to the window, if asked for.
    trace: Option<PresentTrace>,
}

/// What is kept per surface.
struct Target {
    kind: TargetKind,
    /// The last presented scene, to diff the next one against.
    prev: Option<Scene>,
}

// There are few of them.
#[allow(clippy::large_enum_variant)]
enum TargetKind {
    Surface(WindowRenderer),
    Dmabuf(DmabufTarget),
}

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
        let force_surface = std::env::var("ANTIWEBUI_PRESENT").is_ok_and(|v| v == "surface");
        if !force_surface {
            match DmabufPresenter::new(ctx) {
                Ok((gpu, d)) => {
                    if debug {
                        eprintln!("[anti-webui] presenting dmabufs ({} sync)", d.sync.name());
                    }
                    return Ok(Self {
                        targets: HashMap::new(),
                        dmabuf: Some(d),
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
        Ok(Self {
            targets: HashMap::from([(
                SurfaceId::Window,
                Target {
                    kind: TargetKind::Surface(window),
                    prev: None,
                },
            )]),
            dmabuf: None,
            gpu,
            debug,
            trace: None,
        })
    }

    /// How buffers get to the compositor, for reports.
    pub fn path(&self) -> &'static str {
        match &self.dmabuf {
            Some(d) => d.sync.name(),
            None => "wgpu surface",
        }
    }

    /// Whether a frame could be rendered and held back now. Only buffers we present ourselves can
    /// be, and looks at whether one is free so that no work on a frame goes to waste.
    pub fn can_hold_back(&mut self, surface: SurfaceId, limit: usize) -> bool {
        let Some(d) = &self.dmabuf else {
            return false;
        };
        match self.targets.get(&surface).map(|t| &t.kind) {
            Some(TargetKind::Dmabuf(t)) => t.held.len() < limit && d.has_free_slot(t),
            _ => false,
        }
    }

    /// Commits the oldest frame held back. Returns what the frame was traced as, and when.
    pub fn commit_held(&mut self, ctx: &mut impl Host, surface: SurfaceId) -> Option<(u64, i64)> {
        let d = self.dmabuf.as_mut()?;
        let TargetKind::Dmabuf(t) = &mut self.targets.get_mut(&surface)?.kind else {
            return None;
        };
        let frame = t.held.pop_front()?;
        let token = frame.token;
        d.commit(ctx, surface, t, frame);
        Some((token?, linux_sync::monotonic_ns()))
    }

    /// Forgets frames held back, which no longer fit the window.
    pub fn drop_held(&mut self, surface: SurfaceId) {
        if let Some(TargetKind::Dmabuf(t)) = self.targets.get_mut(&surface).map(|t| &mut t.kind) {
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
        if let Some(d) = &mut self.dmabuf {
            d.renderer.set_release_picture_pixels(!keep);
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The GPU that draws, if what decodes video can hand frames over on it.
    pub fn video_render_node(&self) -> Option<std::path::PathBuf> {
        Some(self.dmabuf.as_ref()?.ctx.render_node()?.to_owned())
    }

    /// Something for another renderer to read frames of videos on the GPU with.
    pub fn video_importer(&self) -> Option<anti_webui_wgpu::dmabuf::VideoImporter> {
        self.dmabuf.as_ref()?.ctx.video_importer().ok()
    }

    /// A frame of a video couldn't be read on the GPU, since this was last asked.
    pub fn take_video_import_failed(&mut self) -> bool {
        self.dmabuf
            .as_mut()
            .is_some_and(|d| d.renderer.take_video_import_failed())
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
        if let Some(d) = &mut self.dmabuf {
            d.renderer.forget_target(renderer_target(surface));
            if let TargetKind::Dmabuf(mut t) = target.kind {
                d.drop_slots(&mut t, ctx);
            }
        }
    }

    /// Presents the scene if it differs from the last one on this surface. Returns whether a
    /// buffer was committed, and whether the frame must be retried.
    pub fn present(&mut self, ctx: &mut impl Host, surface: SurfaceId, scene: Scene) -> Presented {
        if let Err(e) = self.update_buffers(ctx) {
            return self.failed(ctx, e);
        }
        if !self.targets.contains_key(&surface) {
            let kind = match &self.dmabuf {
                Some(_) => TargetKind::Dmabuf(DmabufTarget::default()),
                None => {
                    let (display, window) = ctx.raw_handles(surface);
                    let size = (scene.width.max(1), scene.height.max(1));
                    // SAFETY: the surface outlives the target: see `forget`.
                    let created = unsafe { self.gpu.add_window(display, window, size.0, size.1) };
                    match created {
                        Ok(w) => TargetKind::Surface(w),
                        Err(e) => {
                            eprintln!("[anti-webui] no GPU surface for a popup: {e}");
                            return Presented::Unchanged;
                        }
                    }
                }
            };
            self.targets.insert(surface, Target { kind, prev: None });
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
        let unsent = matches!(&target.kind, TargetKind::Dmabuf(t) if t.unsent.is_some());
        if damage.is_empty() && !unsent {
            if self.debug {
                eprintln!("[anti-webui] {surface:?}: frame unchanged");
            }
            return Presented::Unchanged;
        }
        let mut trace = self.trace.as_mut().filter(|_| surface == SurfaceId::Window);
        if let Some(t) = &mut trace {
            t.diff_done = linux_sync::monotonic_ns();
        }
        let result = match &mut target.kind {
            TargetKind::Surface(window) => {
                ctx.set_opaque(surface, scene.clear.a >= 1.0);
                if let Some(t) = &trace {
                    ctx.request_presentation(surface, t.token);
                }
                ctx.prepare_commit(surface);
                match window.present(&self.gpu, &scene) {
                    FrameResult::Presented => {
                        ctx.committed(surface);
                        if let Some(t) = &mut trace {
                            t.buffer_ready = t.diff_done;
                            t.recorded = t.diff_done;
                            let now = linux_sync::monotonic_ns();
                            t.submitted = now;
                            t.committed = now;
                        }
                        Ok(Some(full_stats(&scene)))
                    }
                    FrameResult::Skipped => Ok(None),
                }
            }
            TargetKind::Dmabuf(t) => self
                .dmabuf
                .as_mut()
                .expect("dmabuf targets come with it")
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
                target.prev = Some(scene);
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
        eprintln!("[anti-webui] dmabuf presentation failed ({e}); using a wgpu surface");
        self.fall_back(ctx);
        Presented::Retry
    }

    /// Takes in what happened to the buffers, on whichever surface.
    fn update_buffers(&mut self, ctx: &mut impl Host) -> Result<(), GpuError> {
        let Some(d) = &mut self.dmabuf else {
            return Ok(());
        };
        let targets = self.targets.values_mut().filter_map(|t| match &mut t.kind {
            TargetKind::Dmabuf(t) => Some(t),
            TargetKind::Surface(_) => None,
        });
        let mut targets: Vec<&mut DmabufTarget> = targets.collect();
        for event in ctx.take_buffer_events() {
            match event {
                BufferEvent::Failed(_) => {
                    return Err(GpuError::Dmabuf("the compositor rejected a buffer".into()));
                }
                BufferEvent::Released(id) => {
                    let slots = targets.iter_mut().flat_map(|t| t.slots.iter_mut());
                    if let Some(s) = slots.into_iter().find(|s| s.buffer == id) {
                        s.busy = false;
                    }
                }
            }
        }
        if let Some(f) = ctx.dmabuf_feedback()
            && f.generation != d.generation
        {
            d.generation = f.generation;
            let modifiers = wanted_modifiers(f, &d.ctx);
            if debug_enabled() {
                let scanout = f.tranches.iter().map(|t| t.scanout).collect::<Vec<_>>();
                eprintln!(
                    "[anti-webui] dmabuf feedback: tranches that can scan out {scanout:?}, using {} modifiers",
                    modifiers.len()
                );
            }
            if modifiers != d.modifiers {
                d.modifiers = modifiers;
                for t in &mut targets {
                    d.drop_slots(t, ctx);
                }
            }
        }
        Ok(())
    }

    fn fall_back(&mut self, ctx: &mut impl Host) {
        let mut targets = std::mem::take(&mut self.targets);
        if let Some(d) = &mut self.dmabuf {
            for target in targets.values_mut() {
                if let TargetKind::Dmabuf(t) = &mut target.kind {
                    d.drop_slots(t, ctx);
                }
            }
            d.release_protocol_objects(ctx);
        }
        drop(targets);
        ctx.disable_explicit_sync();
        match surface_gpu(ctx) {
            Ok((gpu, window)) => {
                // The old presenter goes before the GPU it was created on.
                self.dmabuf = None;
                self.targets.insert(
                    SurfaceId::Window,
                    Target {
                        kind: TargetKind::Surface(window),
                        prev: None,
                    },
                );
                self.gpu = gpu;
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

fn surface_gpu(ctx: &mut impl Host) -> Result<(Gpu, WindowRenderer), GpuError> {
    let (display, window) = ctx.raw_handles(SurfaceId::Window);
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

// ---------------------------------------------------------------------------------------------

struct Slot {
    image: DmabufImage,
    buffer: BufferId,
    /// The frame last rendered into it; 0 if none.
    frame: u64,
    /// Committed and not yet released by the compositor (implicit sync).
    busy: bool,
    /// Holds a frame that hasn't been committed.
    held: bool,
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

/// What surfaces presenting our own buffers share.
struct DmabufPresenter {
    ctx: DmabufContext,
    renderer: Renderer,
    sync: Sync,
    generation: u64,
    modifiers: Vec<u64>,
}

/// The buffers of one surface.
#[derive(Default)]
struct DmabufTarget {
    slots: Vec<Slot>,
    history: DamageHistory,
    /// Frames rendered so far.
    frame: u64,
    size: (u32, u32),
    /// Finished frames waiting for the compositor to take the one before, oldest first.
    held: VecDeque<HeldFrame>,
    /// What changed in frames that were dropped instead of committed.
    unsent: Option<Damage>,
}

/// A finished frame that hasn't been committed.
struct HeldFrame {
    slot: usize,
    damage: Damage,
    acquire: Option<(TimelineId, u64)>,
    opaque: bool,
    token: Option<u64>,
}

impl DmabufTarget {
    fn drop_held(&mut self) {
        for frame in self.held.drain(..) {
            self.slots[frame.slot].held = false;
            match &mut self.unsent {
                Some(d) => d.extend(&frame.damage),
                None => self.unsent = Some(frame.damage),
            }
        }
    }
}

/// What buffers may be made with. What the compositor can put on the screen as it is comes first if
/// it can be drawn to, as the driver would otherwise pick from all of them.
fn wanted_modifiers(feedback: &DmabufFeedback, ctx: &DmabufContext) -> Vec<u64> {
    let scanout = feedback.scanout_modifiers(DRM_FORMAT_ARGB8888);
    if ctx.supports_any(&scanout) {
        scanout
    } else {
        feedback.modifiers(DRM_FORMAT_ARGB8888)
    }
}

impl DmabufPresenter {
    fn new(ctx: &mut impl Host) -> Result<(Gpu, Self), GpuError> {
        let feedback = ctx
            .dmabuf_feedback()
            .ok_or_else(|| GpuError::Dmabuf("the compositor sent no dmabuf feedback".into()))?;
        let generation = feedback.generation;
        let (gpu, dctx) = Gpu::for_dmabuf(feedback.main_device)?;
        let modifiers = wanted_modifiers(feedback, &dctx);
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
        let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
        if let Ok(importer) = dctx.video_importer() {
            renderer.set_video_importer(importer);
        }
        Ok((
            gpu,
            Self {
                ctx: dctx,
                renderer,
                sync,
                generation,
                modifiers,
            },
        ))
    }

    /// Renders and commits a frame. `Ok(None)` means no buffer was free.
    #[allow(clippy::too_many_arguments)]
    fn present(
        &mut self,
        gpu: &Gpu,
        ctx: &mut impl Host,
        surface: SurfaceId,
        target: &mut DmabufTarget,
        scene: &Scene,
        damage: &Damage,
        mut trace: Option<&mut PresentTrace>,
    ) -> Result<Option<Stats>, GpuError> {
        let size = (scene.width.max(1), scene.height.max(1));
        if size != target.size {
            target.size = size;
            self.drop_slots(target, ctx);
            target.history.clear();
        }

        // A frame waiting for the compositor is ahead of it already, and shouldn't hold up the
        // UI to get further ahead.
        let hold_back = surface == SurfaceId::Window && ctx.frame_pending(surface);
        let Some(index) = self.free_slot(gpu, ctx, target, !hold_back)? else {
            return Ok(None);
        };
        if let Some(t) = &mut trace {
            t.buffer_ready = linux_sync::monotonic_ns();
        }
        let frame = target.frame + 1;
        let slot = &mut target.slots[index];
        let age = (slot.frame > 0).then(|| frame - slot.frame);
        let repaint = match age {
            Some(1) => Some(damage.clone()),
            Some(age) => target.history.last(age as usize - 1).map(|mut d| {
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
        self.renderer.set_target(renderer_target(surface));
        self.renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &slot.image.view,
            scene,
            &repaint,
        );
        if let Some(t) = &mut trace {
            t.recorded = linux_sync::monotonic_ns();
        }
        gpu.queue.submit([encoder.finish()]);
        let done = self.ctx.end_access(&mut slot.image)?;
        if let Some(t) = &mut trace {
            t.submitted = linux_sync::monotonic_ns();
        }

        let acquire = match &mut self.sync {
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
                Some((*acquire_id, *point))
            }
            Sync::Implicit => {
                if let Some(f) = &done {
                    linux_sync::dmabuf_import_write_fence(slot.image.fd(), f.as_fd())
                        .map_err(|e| GpuError::Dmabuf(format!("attaching a fence: {e}")))?;
                }
                None
            }
        };
        slot.frame = frame;
        target.frame = frame;
        target.history.push(damage.clone());
        let held = HeldFrame {
            slot: index,
            damage: damage.clone(),
            acquire,
            opaque: scene.clear.a >= 1.0,
            token: trace.as_ref().map(|t| t.token),
        };
        if hold_back {
            target.slots[index].held = true;
            target.held.push_back(held);
        } else {
            self.commit(ctx, surface, target, held);
            if let Some(t) = &mut trace {
                t.committed = linux_sync::monotonic_ns();
            }
        }
        if let Some(t) = &mut trace {
            t.fence = done;
        }
        Ok(Some(Stats {
            age,
            repaint: repaint.area(),
            buffers: target.slots.len(),
        }))
    }

    /// Hands a finished frame to the compositor.
    fn commit(
        &mut self,
        ctx: &mut impl Host,
        surface: SurfaceId,
        target: &mut DmabufTarget,
        frame: HeldFrame,
    ) {
        let slot = &mut target.slots[frame.slot];
        let sync = frame.acquire.map(|acquire| {
            let release = slot.release.as_mut().expect("explicit sync slot");
            release.point += 1;
            SyncPoints {
                acquire,
                release: (release.id, release.point),
            }
        });
        ctx.set_opaque(surface, frame.opaque);
        if let Some(token) = frame.token {
            ctx.request_presentation(surface, token);
        }
        let mut damage = frame.damage;
        if let Some(unsent) = target.unsent.take() {
            damage.extend(&unsent);
        }
        ctx.present_buffer(surface, slot.buffer, damage.rects(), sync);
        slot.busy = true;
        slot.held = false;
    }

    /// Whether the buffer can be rendered to, waiting until `deadline` for that.
    fn slot_free(&self, s: &Slot, deadline: i64) -> bool {
        if s.held {
            return false;
        }
        match (&self.sync, &s.release) {
            (Sync::Explicit { drm, .. }, Some(r)) => {
                r.point == 0 || drm.wait_point(r.handle, r.point, deadline).unwrap_or(false)
            }
            _ => !s.busy,
        }
    }

    fn has_free_slot(&self, target: &DmabufTarget) -> bool {
        target.slots.len() < MAX_BUFFERS + target.held.len()
            || target.slots.iter().any(|s| self.slot_free(s, 0))
    }

    /// The free buffer holding the most recent frame, allocating one if none is free.
    fn free_slot(
        &mut self,
        gpu: &Gpu,
        ctx: &mut impl Host,
        target: &mut DmabufTarget,
        may_wait: bool,
    ) -> Result<Option<usize>, GpuError> {
        let best = (0..target.slots.len())
            .filter(|&i| self.slot_free(&target.slots[i], 0))
            .max_by_key(|&i| target.slots[i].frame);
        if best.is_some() {
            return Ok(best);
        }
        if target.slots.len() < MAX_BUFFERS + target.held.len() {
            return self.allocate(gpu, ctx, target).map(Some);
        }
        // All buffers are with the compositor: briefly wait for the oldest (explicit sync).
        let oldest = (0..target.slots.len())
            .filter(|&i| !target.slots[i].held)
            .min_by_key(|&i| target.slots[i].frame);
        if let (true, Some(i), Sync::Explicit { .. }) = (may_wait, oldest, &self.sync) {
            let deadline = linux_sync::monotonic_ns() + 50_000_000;
            if self.slot_free(&target.slots[i], deadline) {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    fn allocate(
        &mut self,
        gpu: &Gpu,
        ctx: &mut impl Host,
        target: &mut DmabufTarget,
    ) -> Result<usize, GpuError> {
        let (w, h) = target.size;
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
        target.slots.push(Slot {
            image,
            buffer,
            frame: 0,
            busy: false,
            held: false,
            release,
        });
        Ok(target.slots.len() - 1)
    }

    fn drop_slots(&mut self, target: &mut DmabufTarget, ctx: &mut impl Host) {
        target.held.clear();
        target.unsent = None;
        for slot in target.slots.drain(..) {
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

    fn release_protocol_objects(&mut self, ctx: &mut impl Host) {
        if let Sync::Explicit { acquire_id, .. } = &self.sync {
            ctx.destroy_timeline(*acquire_id);
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let targets = std::mem::take(&mut self.targets);
        if let Some(d) = &mut self.dmabuf {
            for target in targets.into_values() {
                if let TargetKind::Dmabuf(mut t) = target.kind {
                    for slot in t.slots.drain(..) {
                        d.ctx.retire(slot.image);
                    }
                }
            }
        }
    }
}

fn setup_explicit_sync(ctx: &mut impl Host, dctx: &DmabufContext) -> Option<Sync> {
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
