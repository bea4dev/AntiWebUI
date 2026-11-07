//! Our own `IOSurface` buffers on macOS, drawn with damage tracking and shown as the contents of a
//! layer (see "提示" in `docs/design/37-platform-macos.md`). Core Animation takes no damage, so
//! what this saves is the GPU's work.

use super::{Stats, renderer_target};
use crate::clock;
use crate::host::Host;
use crate::pacing::PresentTrace;
use anti_webui_core::damage::{Damage, DamageHistory};
use anti_webui_core::platform::{BufferId, SurfaceId};
use anti_webui_core::scene::Scene;
use anti_webui_wgpu::iosurface::{FORMAT, IoSurfaceImage};
use anti_webui_wgpu::{Gpu, GpuError, Renderer};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

/// Buffers beyond this make us wait for the window server instead: one shown, one it may still
/// read, and one to draw.
const MAX_BUFFERS: usize = 3;

struct Slot {
    image: IoSurfaceImage,
    buffer: BufferId,
    /// The frame last rendered into it; 0 if none.
    frame: u64,
}

/// What surfaces presenting `IOSurface`s share.
pub(crate) struct IoSurfacePresenter {
    gpu: Rc<Gpu>,
    renderer: Renderer,
}

/// The buffers of one surface.
#[derive(Default)]
pub(crate) struct IoSurfaceTarget {
    slots: Vec<Slot>,
    history: DamageHistory,
    /// Frames rendered so far.
    frame: u64,
    size: (u32, u32),
    /// The slot the layer shows.
    front: Option<usize>,
}

impl IoSurfaceTarget {
    /// Nothing is held back here.
    pub fn drop_held(&mut self) {}

    pub fn has_unsent(&self) -> bool {
        false
    }
}

thread_local! {
    /// The GPU the windows of this thread draw with: making a device takes tens of milliseconds
    /// and a good deal of memory.
    static SHARED: RefCell<Weak<Gpu>> = const { RefCell::new(Weak::new()) };
}

fn shared_gpu() -> Result<Rc<Gpu>, GpuError> {
    if let Some(gpu) = SHARED.with(|s| s.borrow().upgrade()) {
        return Ok(gpu);
    }
    let gpu = Rc::new(Gpu::for_iosurface()?);
    SHARED.with(|s| *s.borrow_mut() = Rc::downgrade(&gpu));
    Ok(gpu)
}

impl IoSurfacePresenter {
    pub fn new(_ctx: &mut impl Host) -> Result<(Rc<Gpu>, Self), GpuError> {
        let gpu = shared_gpu()?;
        let renderer = Renderer::new(&gpu.device, FORMAT);
        Ok((gpu.clone(), Self { gpu, renderer }))
    }

    pub fn name(&self) -> &'static str {
        "IOSurface"
    }

    pub fn new_target(&self) -> IoSurfaceTarget {
        IoSurfaceTarget::default()
    }

    /// Frames go to the layer as soon as they are drawn.
    pub fn can_hold_back(&self, _t: &IoSurfaceTarget, _limit: usize) -> bool {
        false
    }

    pub fn commit_held(
        &mut self,
        _ctx: &mut impl Host,
        _surface: SurfaceId,
        _t: &mut IoSurfaceTarget,
    ) -> Option<Option<u64>> {
        None
    }

    pub fn keep_picture_pixels(&mut self, keep: bool) {
        self.renderer.set_release_picture_pixels(!keep);
    }

    pub fn video_render_node(&self) -> Option<std::path::PathBuf> {
        None
    }

    pub fn take_video_import_failed(&mut self) -> bool {
        self.renderer.take_video_import_failed()
    }

    pub fn forget(&mut self, ctx: &mut impl Host, target: u64, buffers: Option<IoSurfaceTarget>) {
        self.renderer.forget_target(target);
        if let Some(mut t) = buffers {
            drop_slots(&mut t, ctx);
        }
    }

    /// Nothing comes back from the window server about buffers.
    pub fn update(
        &mut self,
        _ctx: &mut impl Host,
        _targets: Vec<&mut IoSurfaceTarget>,
    ) -> Result<(), GpuError> {
        Ok(())
    }

    pub fn release(&mut self, ctx: &mut impl Host, targets: Vec<&mut IoSurfaceTarget>) {
        for t in targets {
            drop_slots(t, ctx);
        }
    }

    /// The `IOSurface`s go with the layers that show them, when those let go of them.
    pub fn retire(&mut self, _targets: impl Iterator<Item = IoSurfaceTarget>) {}

    /// Renders and shows a frame. `Ok(None)` means no buffer was free.
    #[allow(clippy::too_many_arguments)]
    pub fn present(
        &mut self,
        gpu: &Gpu,
        ctx: &mut impl Host,
        surface: SurfaceId,
        target: &mut IoSurfaceTarget,
        scene: &Scene,
        damage: &Damage,
        mut trace: Option<&mut PresentTrace>,
    ) -> Result<Option<Stats>, GpuError> {
        let size = (scene.width.max(1), scene.height.max(1));
        if size != target.size {
            target.size = size;
            drop_slots(target, ctx);
            target.history.clear();
        }
        let Some(index) = self.free_slot(ctx, target)? else {
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
        gpu.wait_until_scheduled();
        if let Some(t) = &mut trace {
            t.submitted = clock::monotonic_ns();
        }
        slot.frame = frame;
        target.frame = frame;
        target.history.push(damage.clone());
        target.front = Some(index);
        ctx.set_opaque(surface, scene.clear.a >= 1.0);
        if let Some(t) = &trace {
            ctx.request_presentation(surface, t.token);
        }
        ctx.present_buffer(surface, slot.buffer, damage.rects(), None);
        if let Some(t) = &mut trace {
            t.committed = clock::monotonic_ns();
        }
        Ok(Some(Stats {
            age,
            repaint: repaint.area(),
            buffers: target.slots.len(),
        }))
    }

    /// The buffer holding the most recent frame among those nobody reads, making one if none is
    /// free. The one shown may be read by the window server at any time; the ones before it are
    /// until it has composited the next.
    fn free_slot(
        &mut self,
        ctx: &mut impl Host,
        target: &mut IoSurfaceTarget,
    ) -> Result<Option<usize>, GpuError> {
        let best = (0..target.slots.len())
            .filter(|&i| Some(i) != target.front && !target.slots[i].image.surface.is_in_use())
            .max_by_key(|&i| target.slots[i].frame);
        if best.is_some() {
            return Ok(best);
        }
        if target.slots.len() >= MAX_BUFFERS {
            return Ok(None);
        }
        let (w, h) = target.size;
        let image = self.gpu.allocate_iosurface(w, h)?;
        let buffer = ctx
            .create_iosurface_buffer(&image.surface)
            .ok_or_else(|| GpuError::IoSurface("the window is gone".into()))?;
        target.slots.push(Slot {
            image,
            buffer,
            frame: 0,
        });
        Ok(Some(target.slots.len() - 1))
    }
}

fn drop_slots(target: &mut IoSurfaceTarget, ctx: &mut impl Host) {
    target.front = None;
    for slot in target.slots.drain(..) {
        ctx.destroy_buffer(slot.buffer);
    }
}
