//! Our own dmabuf buffers, presented with damage tracking (see `docs/design/06-platform-wayland.md`
//! and `35-platform-x11.md`).

use super::{Stats, renderer_target};
use crate::clock;
use crate::host::Host;
use crate::pacing::PresentTrace;
use anti_webui_core::damage::{Damage, DamageHistory};
use anti_webui_core::platform::{
    BufferEvent, BufferId, DmabufFeedback, DmabufPlaneDesc, SurfaceId, SyncPoints, TimelineId,
};
use anti_webui_core::scene::Scene;
use anti_webui_wgpu::dmabuf::{DRM_FORMAT_ARGB8888, DmabufContext, DmabufImage};
use anti_webui_wgpu::linux_sync::{self, DrmDevice};
use anti_webui_wgpu::{Gpu, GpuError, Renderer, wgpu};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::fd::AsFd;
use std::rc::{Rc, Weak};

/// Buffers beyond this make the compositor wait for us instead. Frames held back use one more each.
const MAX_BUFFERS: usize = 4;

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
pub(crate) struct DmabufPresenter {
    /// Shared with the other windows of the thread on the same GPU.
    ctx: Rc<RefCell<DmabufContext>>,
    renderer: Renderer,
    sync: Sync,
    generation: u64,
    modifiers: Vec<u64>,
}

/// The buffers of one surface.
#[derive(Default)]
pub(crate) struct DmabufTarget {
    slots: Vec<Slot>,
    /// Replaced buffers the compositor hasn't released: it may still be showing one, even
    /// putting it on the screen as it is, so neither it nor its memory may go yet.
    retiring: Vec<Slot>,
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
    pub fn drop_held(&mut self) {
        for frame in self.held.drain(..) {
            self.slots[frame.slot].held = false;
            match &mut self.unsent {
                Some(d) => d.extend(&frame.damage),
                None => self.unsent = Some(frame.damage),
            }
        }
    }

    /// Whether frames were dropped whose changes the compositor hasn't had.
    pub fn has_unsent(&self) -> bool {
        self.unsent.is_some()
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

type SharedGpu = (Rc<Gpu>, Rc<RefCell<DmabufContext>>);
/// A device the display server uses, and the GPU on it while some window has it.
type KnownGpu = (u64, Weak<Gpu>, Weak<RefCell<DmabufContext>>);

thread_local! {
    /// The GPUs the windows of this thread draw with.
    static SHARED: RefCell<Vec<KnownGpu>> = const { RefCell::new(Vec::new()) };
}

/// The GPU of another window of this thread on the same device, or a new one: making a device
/// takes tens of milliseconds and a good deal of memory.
fn shared_gpu(main_device: u64) -> Result<SharedGpu, GpuError> {
    let found = SHARED.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|(_, gpu, _)| gpu.strong_count() > 0);
        s.iter()
            .filter(|(device, _, _)| *device == main_device)
            .find_map(|(_, gpu, ctx)| Some((gpu.upgrade()?, ctx.upgrade()?)))
    });
    if let Some(found) = found {
        return Ok(found);
    }
    let (gpu, ctx) = Gpu::for_dmabuf(main_device)?;
    let (gpu, ctx) = (Rc::new(gpu), Rc::new(RefCell::new(ctx)));
    SHARED.with(|s| {
        s.borrow_mut()
            .push((main_device, Rc::downgrade(&gpu), Rc::downgrade(&ctx)))
    });
    Ok((gpu, ctx))
}

impl DmabufPresenter {
    pub fn new(ctx: &mut impl Host) -> Result<(Rc<Gpu>, Self), GpuError> {
        let feedback = ctx
            .dmabuf_feedback()
            .ok_or_else(|| GpuError::Dmabuf("the compositor sent no dmabuf feedback".into()))?;
        let generation = feedback.generation;
        let (gpu, shared) = shared_gpu(feedback.main_device)?;
        let dctx = shared.borrow();
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
        drop(dctx);
        Ok((
            gpu,
            Self {
                ctx: shared,
                renderer,
                sync,
                generation,
                modifiers,
            },
        ))
    }

    /// How frames are synchronized with the compositor, for reports.
    pub fn name(&self) -> &'static str {
        self.sync.name()
    }

    pub fn new_target(&self) -> DmabufTarget {
        DmabufTarget::default()
    }

    pub fn can_hold_back(&self, t: &DmabufTarget, limit: usize) -> bool {
        t.held.len() < limit && self.has_free_slot(t)
    }

    /// Commits the oldest frame held back, if there is one; what it was traced as.
    pub fn commit_held(
        &mut self,
        ctx: &mut impl Host,
        surface: SurfaceId,
        t: &mut DmabufTarget,
    ) -> Option<Option<u64>> {
        let frame = t.held.pop_front()?;
        let token = frame.token;
        self.commit(ctx, surface, t, frame);
        Some(token)
    }

    pub fn keep_picture_pixels(&mut self, keep: bool) {
        self.renderer.set_release_picture_pixels(!keep);
    }

    pub fn video_render_node(&self) -> Option<std::path::PathBuf> {
        Some(self.ctx.borrow().render_node()?.to_owned())
    }

    pub fn video_importer(&self) -> Option<anti_webui_wgpu::dmabuf::VideoImporter> {
        self.ctx.borrow().video_importer().ok()
    }

    pub fn take_video_import_failed(&mut self) -> bool {
        self.renderer.take_video_import_failed()
    }

    /// Frees what is kept for a surface that goes.
    pub fn forget(&mut self, ctx: &mut impl Host, target: u64, buffers: Option<DmabufTarget>) {
        self.renderer.forget_target(target);
        if let Some(mut t) = buffers {
            self.destroy_slots(&mut t, ctx);
        }
    }

    /// Takes in what happened to the buffers of every surface.
    pub fn update(
        &mut self,
        ctx: &mut impl Host,
        mut targets: Vec<&mut DmabufTarget>,
    ) -> Result<(), GpuError> {
        for event in ctx.take_buffer_events() {
            match event {
                BufferEvent::Failed(_) => {
                    return Err(GpuError::Dmabuf("the compositor rejected a buffer".into()));
                }
                BufferEvent::Released(id) => {
                    let slots = targets
                        .iter_mut()
                        .flat_map(|t| t.slots.iter_mut().chain(t.retiring.iter_mut()));
                    if let Some(s) = slots.into_iter().find(|s| s.buffer == id) {
                        s.busy = false;
                    }
                }
            }
        }
        for t in &mut targets {
            self.reap_retiring(t, ctx);
        }
        if let Some(f) = ctx.dmabuf_feedback()
            && f.generation != self.generation
        {
            self.generation = f.generation;
            let modifiers = wanted_modifiers(f, &self.ctx.borrow());
            if super::debug_enabled() {
                let scanout = f.tranches.iter().map(|t| t.scanout).collect::<Vec<_>>();
                eprintln!(
                    "[anti-webui] dmabuf feedback: tranches that can scan out {scanout:?}, using {} modifiers",
                    modifiers.len()
                );
            }
            // Buffers made with a modifier still on the list stay: the compositor sends new
            // feedback whenever it starts or stops putting the surface on a plane, and making
            // them again each time would replace the buffer on the screen every frame.
            if modifiers != self.modifiers {
                for t in &mut targets {
                    if t.slots
                        .iter()
                        .any(|s| !modifiers.contains(&s.image.modifier))
                    {
                        self.replace_slots(t, ctx);
                    }
                }
                self.modifiers = modifiers;
            }
        }
        Ok(())
    }

    /// Lets go of every buffer and what the compositor has of ours, before falling back.
    pub fn release(&mut self, ctx: &mut impl Host, targets: Vec<&mut DmabufTarget>) {
        for t in targets {
            self.destroy_slots(t, ctx);
        }
        self.release_protocol_objects(ctx);
    }

    /// Lets go of the buffers when the presenter goes.
    pub fn retire(&mut self, targets: impl Iterator<Item = DmabufTarget>) {
        for mut t in targets {
            for slot in t.slots.drain(..).chain(t.retiring.drain(..)) {
                self.ctx.borrow_mut().retire(slot.image);
            }
        }
    }

    /// Renders and commits a frame. `Ok(None)` means no buffer was free.
    #[allow(clippy::too_many_arguments)]
    pub fn present(
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
            self.replace_slots(target, ctx);
            target.history.clear();
        }

        // A frame waiting for the compositor is ahead of it already, and shouldn't hold up the
        // UI to get further ahead.
        let hold_back = surface == SurfaceId::Window && ctx.frame_pending(surface);
        let Some(index) = self.free_slot(gpu, ctx, target, !hold_back)? else {
            return Ok(None);
        };
        if let Some(t) = &mut trace {
            t.buffer_ready = clock::monotonic_ns();
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
        self.ctx.borrow_mut().begin_access(&mut slot.image, wait)?;
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
            t.recorded = clock::monotonic_ns();
        }
        gpu.queue.submit([encoder.finish()]);
        let done = self.ctx.borrow_mut().end_access(&mut slot.image)?;
        if let Some(t) = &mut trace {
            t.submitted = clock::monotonic_ns();
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
                t.committed = clock::monotonic_ns();
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
            let deadline = clock::monotonic_ns() + 50_000_000;
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
        let image = self.ctx.borrow_mut().allocate(gpu, w, h, &self.modifiers)?;
        if super::debug_enabled() {
            eprintln!(
                "[anti-webui] new buffer {w}x{h}, modifier {:#x}, from {}",
                image.modifier,
                if image.from_gbm() { "GBM" } else { "Vulkan" }
            );
        }
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

    /// Stops using the buffers, for new ones of another size or modifier. Those the compositor
    /// still has go once it releases them.
    fn replace_slots(&mut self, target: &mut DmabufTarget, ctx: &mut impl Host) {
        target.drop_held();
        target.unsent = None;
        target.retiring.append(&mut target.slots);
        self.reap_retiring(target, ctx);
    }

    /// Frees the replaced buffers the compositor has released.
    fn reap_retiring(&mut self, target: &mut DmabufTarget, ctx: &mut impl Host) {
        let mut i = 0;
        while i < target.retiring.len() {
            if self.slot_free(&target.retiring[i], 0) {
                let slot = target.retiring.swap_remove(i);
                self.destroy_slot(slot, ctx);
            } else {
                i += 1;
            }
        }
    }

    /// Frees every buffer at once, as when the surface goes.
    fn destroy_slots(&mut self, target: &mut DmabufTarget, ctx: &mut impl Host) {
        target.held.clear();
        target.unsent = None;
        for slot in target.slots.drain(..).chain(target.retiring.drain(..)) {
            self.destroy_slot(slot, ctx);
        }
    }

    fn destroy_slot(&mut self, slot: Slot, ctx: &mut impl Host) {
        ctx.destroy_buffer(slot.buffer);
        if let Some(r) = slot.release {
            ctx.destroy_timeline(r.id);
            if let Sync::Explicit { drm, .. } = &self.sync {
                drm.destroy_syncobj(r.handle);
            }
        }
        self.ctx.borrow_mut().retire(slot.image);
    }

    fn release_protocol_objects(&mut self, ctx: &mut impl Host) {
        if let Sync::Explicit { acquire_id, .. } = &self.sync {
            ctx.destroy_timeline(*acquire_id);
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
