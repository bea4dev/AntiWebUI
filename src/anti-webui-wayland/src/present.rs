//! Presenting our own buffers: linux-dmabuf feedback and buffers, explicit sync timelines, and
//! damage (see `docs/design/06-platform-wayland.md`).

use crate::state::State;
use std::collections::HashMap;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::fs::FileExt;
use wayland_client::protocol::wl_buffer;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1, wp_linux_drm_syncobj_surface_v1,
    wp_linux_drm_syncobj_timeline_v1,
};

/// What the compositor can use, from `zwp_linux_dmabuf_feedback_v1`.
#[derive(Clone, Debug, Default)]
pub struct DmabufFeedback {
    /// `dev_t` of the device the compositor composites with.
    pub main_device: u64,
    /// In order of preference.
    pub tranches: Vec<DmabufTranche>,
    /// Increases whenever the compositor sends new feedback.
    pub generation: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DmabufTranche {
    pub device: u64,
    /// `(DRM fourcc, modifier)` pairs.
    pub formats: Vec<(u32, u64)>,
    /// Buffers in this tranche can be scanned out directly.
    pub scanout: bool,
}

impl DmabufFeedback {
    /// Modifiers for `format` on the main device, most preferred tranche first.
    pub fn modifiers(&self, format: u32) -> Vec<u64> {
        let mut out = Vec::new();
        for t in self
            .tranches
            .iter()
            .filter(|t| t.device == self.main_device)
        {
            for &(f, m) in &t.formats {
                if f == format && !out.contains(&m) {
                    out.push(m);
                }
            }
        }
        out
    }
}

#[derive(Default)]
pub(crate) struct PendingFeedback {
    table: Vec<(u32, u64)>,
    main_device: u64,
    tranches: Vec<DmabufTranche>,
    tranche: DmabufTranche,
}

/// One plane of a dmabuf to wrap in a `wl_buffer`.
#[derive(Clone, Copy, Debug)]
pub struct DmabufPlaneDesc<'a> {
    pub fd: BorrowedFd<'a>,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferEvent {
    /// The compositor no longer reads the buffer (implicit sync only).
    Released(BufferId),
    /// The compositor rejected the buffer.
    Failed(BufferId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimelineId(u64);

/// Explicit synchronization points for one commit.
#[derive(Clone, Copy, Debug)]
pub struct SyncPoints {
    pub acquire: (TimelineId, u64),
    pub release: (TimelineId, u64),
}

#[derive(Default)]
pub(crate) struct PresentState {
    pub dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    pub feedback_object: Option<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>,
    pub feedback: Option<DmabufFeedback>,
    pub pending_feedback: PendingFeedback,
    pub buffers: HashMap<BufferId, wl_buffer::WlBuffer>,
    pub buffer_events: Vec<BufferEvent>,
    pub next_id: u64,
    pub syncobj_manager: Option<wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1>,
    pub syncobj_surface: Option<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1>,
    pub timelines:
        HashMap<TimelineId, wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1>,
    /// Whether the surface is opaque, and the logical size the opaque region was set for.
    pub opaque: bool,
    pub opaque_applied: Option<(i32, i32)>,
}

impl State {
    pub(crate) fn create_dmabuf_buffer(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        planes: &[DmabufPlaneDesc<'_>],
    ) -> Option<BufferId> {
        let dmabuf = self.present.dmabuf.as_ref()?;
        self.present.next_id += 1;
        let id = BufferId(self.present.next_id);
        let params = dmabuf.create_params(&self.qh, id);
        for (i, p) in planes.iter().enumerate() {
            params.add(
                p.fd,
                i as u32,
                p.offset,
                p.stride,
                (modifier >> 32) as u32,
                modifier as u32,
            );
        }
        let buffer = params.create_immed(
            width as i32,
            height as i32,
            format,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &self.qh,
            id,
        );
        params.destroy();
        self.present.buffers.insert(id, buffer);
        Some(id)
    }

    pub(crate) fn destroy_buffer(&mut self, id: BufferId) {
        if let Some(b) = self.present.buffers.remove(&id) {
            b.destroy();
        }
    }

    pub(crate) fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Option<TimelineId> {
        let manager = self.present.syncobj_manager.as_ref()?;
        if self.present.syncobj_surface.is_none() {
            self.present.syncobj_surface = Some(manager.get_surface(&self.surface, &self.qh, ()));
        }
        self.present.next_id += 1;
        let id = TimelineId(self.present.next_id);
        let timeline = manager.import_timeline(fd, &self.qh, ());
        self.present.timelines.insert(id, timeline);
        Some(id)
    }

    pub(crate) fn destroy_timeline(&mut self, id: TimelineId) {
        if let Some(t) = self.present.timelines.remove(&id) {
            t.destroy();
        }
    }

    /// Stops explicit sync on the surface (needed before presenting through WSI).
    pub(crate) fn disable_explicit_sync(&mut self) {
        if let Some(s) = self.present.syncobj_surface.take() {
            s.destroy();
        }
    }

    pub(crate) fn present_buffer(
        &mut self,
        id: BufferId,
        damage: &[anti_webui_core::geometry::RectI],
        sync: Option<SyncPoints>,
    ) {
        let Some(buffer) = self.present.buffers.get(&id) else {
            return;
        };
        self.surface.attach(Some(buffer), 0, 0);
        for r in damage {
            self.surface.damage_buffer(r.x, r.y, r.width, r.height);
        }
        if let (Some(sync), Some(surface)) = (sync, &self.present.syncobj_surface) {
            let point = |(timeline, value): (TimelineId, u64)| {
                self.present
                    .timelines
                    .get(&timeline)
                    .map(|t| (t, (value >> 32) as u32, value as u32))
            };
            if let (Some(a), Some(r)) = (point(sync.acquire), point(sync.release)) {
                surface.set_acquire_point(a.0, a.1, a.2);
                surface.set_release_point(r.0, r.1, r.2);
            }
        }
        self.apply_opaque_region();
        self.surface.commit();
    }

    pub(crate) fn apply_opaque_region(&mut self) {
        let size = self.logical_size;
        let wanted = self.present.opaque.then_some(size);
        if self.present.opaque_applied == wanted {
            return;
        }
        self.present.opaque_applied = wanted;
        match wanted {
            Some((w, h)) => {
                let region = self.compositor.create_region(&self.qh, ());
                region.add(0, 0, w, h);
                self.surface.set_opaque_region(Some(&region));
                region.destroy();
            }
            None => self.surface.set_opaque_region(None),
        }
    }
}

fn read_format_table(fd: OwnedFd, size: u32) -> Vec<(u32, u64)> {
    let file = std::fs::File::from(fd);
    let mut bytes = vec![0u8; size as usize];
    if file.read_exact_at(&mut bytes, 0).is_err() {
        return Vec::new();
    }
    bytes
        .chunks_exact(16)
        .map(|e| {
            let format = u32::from_ne_bytes(e[0..4].try_into().unwrap());
            let modifier = u64::from_ne_bytes(e[8..16].try_into().unwrap());
            (format, modifier)
        })
        .collect()
}

fn dev_t(bytes: &[u8]) -> u64 {
    bytes
        .get(..8)
        .map_or(0, |b| u64::from_ne_bytes(b.try_into().unwrap()))
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::{Event, TrancheFlags};
        let p = &mut state.present.pending_feedback;
        match event {
            Event::FormatTable { fd, size } => p.table = read_format_table(fd, size),
            Event::MainDevice { device } => p.main_device = dev_t(&device),
            Event::TrancheTargetDevice { device } => p.tranche.device = dev_t(&device),
            Event::TrancheFormats { indices } => {
                for i in indices.chunks_exact(2) {
                    let i = u16::from_ne_bytes([i[0], i[1]]) as usize;
                    if let Some(&f) = p.table.get(i) {
                        p.tranche.formats.push(f);
                    }
                }
            }
            Event::TrancheFlags { flags } => {
                p.tranche.scanout =
                    matches!(flags, WEnum::Value(f) if f.contains(TrancheFlags::Scanout));
            }
            Event::TrancheDone => {
                let t = std::mem::take(&mut p.tranche);
                p.tranches.push(t);
            }
            Event::Done => {
                let generation = state
                    .present
                    .feedback
                    .as_ref()
                    .map_or(1, |f| f.generation + 1);
                state.present.feedback = Some(DmabufFeedback {
                    main_device: p.main_device,
                    tranches: std::mem::take(&mut p.tranches),
                    generation,
                });
                state.redraw_requested = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, BufferId> for State {
    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        id: &BufferId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            state.present.buffer_events.push(BufferEvent::Failed(*id));
            state.redraw_requested = true;
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferId> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        id: &BufferId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.present.buffer_events.push(BufferEvent::Released(*id));
        }
    }
}

delegate_noop!(State: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(State: ignore wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1);
delegate_noop!(State: ignore wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1);
delegate_noop!(State: ignore wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1);
delegate_noop!(State: wayland_client::protocol::wl_region::WlRegion);
