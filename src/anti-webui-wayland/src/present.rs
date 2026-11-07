//! Presenting our own buffers: linux-dmabuf feedback and buffers, explicit sync timelines, and
//! damage (see `docs/design/06-platform-wayland.md`).

use crate::state::State;
use anti_webui_core::platform::{
    BufferEvent, BufferId, DmabufFeedback, DmabufPlaneDesc, DmabufTranche, Presentation, Shown,
    SurfaceId, SyncPoints, TimelineId,
};
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
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};

#[derive(Default)]
pub(crate) struct PendingFeedback {
    table: Vec<(u32, u64)>,
    main_device: u64,
    tranches: Vec<DmabufTranche>,
    tranche: DmabufTranche,
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
    pub timelines:
        HashMap<TimelineId, wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1>,
    pub presentation: Option<wp_presentation::WpPresentation>,
    pub presentation_clock: Option<u32>,
    pub presentations: Vec<Presentation>,
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

    /// Stops explicit sync on the surfaces (needed before presenting through WSI).
    pub(crate) fn disable_explicit_sync(&mut self) {
        let popups = self.popups.values_mut().map(|p| &mut p.frame);
        for f in std::iter::once(&mut self.window_frame).chain(popups) {
            if let Some(s) = f.syncobj.take() {
                s.destroy();
            }
        }
    }

    pub(crate) fn present_buffer(
        &mut self,
        surface: SurfaceId,
        id: BufferId,
        damage: &[anti_webui_core::geometry::RectI],
        sync: Option<SyncPoints>,
    ) {
        self.prepare_commit(surface);
        let Some(wl) = self.wl_surface(surface).cloned() else {
            return;
        };
        let Some(buffer) = self.present.buffers.get(&id) else {
            return;
        };
        wl.attach(Some(buffer), 0, 0);
        for r in damage {
            wl.damage_buffer(r.x, r.y, r.width, r.height);
        }
        if let (Some(sync), Some(manager)) = (sync, self.present.syncobj_manager.clone()) {
            let qh = self.qh.clone();
            let frame = self.frame_state(surface).expect("the surface exists");
            let object = frame
                .syncobj
                .get_or_insert_with(|| manager.get_surface(&wl, &qh, ()))
                .clone();
            let point = |(timeline, value): (TimelineId, u64)| {
                self.present
                    .timelines
                    .get(&timeline)
                    .map(|t| (t, (value >> 32) as u32, value as u32))
            };
            if let (Some(a), Some(r)) = (point(sync.acquire), point(sync.release)) {
                object.set_acquire_point(a.0, a.1, a.2);
                object.set_release_point(r.0, r.1, r.2);
            }
        }
        wl.commit();
        self.committed(surface);
    }

    /// Asks to be told what becomes of the next commit of the surface.
    pub(crate) fn request_presentation(&mut self, surface: SurfaceId, token: u64) -> bool {
        let (Some(presentation), Some(wl)) =
            (self.present.presentation.clone(), self.wl_surface(surface))
        else {
            return false;
        };
        presentation.feedback(wl, &self.qh, token);
        true
    }

    pub(crate) fn apply_opaque_region(&mut self, surface: SurfaceId) {
        let size = self.logical_size;
        let qh = self.qh.clone();
        let compositor = self.compositor.clone();
        let Some(wl) = self.wl_surface(surface).cloned() else {
            return;
        };
        let Some(f) = self.frame_state(surface) else {
            return;
        };
        let wanted = f.opaque.then_some(size);
        if f.opaque_applied == wanted {
            return;
        }
        f.opaque_applied = wanted;
        match wanted {
            Some((w, h)) => {
                let region = compositor.create_region(&qh, ());
                region.add(0, 0, w, h);
                wl.set_opaque_region(Some(&region));
                region.destroy();
            }
            None => wl.set_opaque_region(None),
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
impl Dispatch<wp_presentation::WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.present.presentation_clock = Some(clk_id);
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, u64> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        token: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let shown = match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                flags,
                ..
            } => {
                let secs = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                Some(Shown {
                    time_ns: secs as i64 * 1_000_000_000 + tv_nsec as i64,
                    refresh_ns: refresh,
                    flags: match flags {
                        WEnum::Value(f) => f.bits(),
                        WEnum::Unknown(f) => f,
                    },
                })
            }
            wp_presentation_feedback::Event::Discarded => None,
            _ => return,
        };
        state.present.presentations.push(Presentation {
            token: *token,
            shown,
        });
    }
}

delegate_noop!(State: ignore wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1);
delegate_noop!(State: ignore wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1);
delegate_noop!(State: ignore wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1);
delegate_noop!(State: wayland_client::protocol::wl_region::WlRegion);
