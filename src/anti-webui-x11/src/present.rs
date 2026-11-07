//! Presenting our own buffers through DRI3 and Present: pixmaps from dmabufs, update regions,
//! when frames reached the screen, and explicit sync (see "提示" in
//! `docs/design/35-platform-x11.md`).

use anti_webui_core::geometry::RectI;
use anti_webui_core::platform::{
    BufferEvent, BufferId, DmabufFeedback, DmabufPlaneDesc, DmabufTranche, Presentation, Shown,
    SyncPoints, TimelineId,
};
use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::time::{Duration, Instant};
use x11rb::NONE;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::present::{self, CompleteKind, CompleteMode};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, Pixmap, Rectangle, Window};
use x11rb::protocol::{dri3, dri3::ConnectionExt as _};
use x11rb::xcb_ffi::XCBConnection;

/// `DRM_FORMAT_ARGB8888`, which buffers are made in whatever the depth.
const ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// `wp_presentation_feedback.kind`, which `Shown::flags` follows.
const KIND_VSYNC: u32 = 1;
const KIND_HW_CLOCK: u32 = 2;
const KIND_HW_COMPLETION: u32 = 4;
const KIND_ZERO_COPY: u32 = 8;
/// A presentation the server never answers doesn't hold up frames for longer than this.
const GIVE_UP: Duration = Duration::from_millis(250);
/// `CLOCK_MONOTONIC`, which Present's `ust` is in.
const CLOCK_MONOTONIC: u32 = 1;

/// What the server can do.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Extensions {
    pub present: Option<(u32, u32)>,
    pub dri3: Option<(u32, u32)>,
    pub xfixes: bool,
}

impl Extensions {
    pub fn query(conn: &XCBConnection) -> Self {
        let version =
            |name: &'static str| conn.extension_information(name).ok().flatten().is_some();
        let present = version(present::X11_EXTENSION_NAME)
            .then(|| present::query_version(conn, 1, 4).ok()?.reply().ok())
            .flatten()
            .map(|r| (r.major_version, r.minor_version));
        let dri3 = version(dri3::X11_EXTENSION_NAME)
            .then(|| conn.dri3_query_version(1, 4).ok()?.reply().ok())
            .flatten()
            .map(|r| (r.major_version, r.minor_version));
        let xfixes = version(xfixes::X11_EXTENSION_NAME)
            && conn
                .xfixes_query_version(5, 0)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some();
        Self {
            present,
            dri3,
            xfixes,
        }
    }

    /// Whether pixmaps can be made from dmabufs with modifiers and presented.
    pub fn dmabuf(&self) -> bool {
        self.present.is_some() && self.dri3.is_some_and(|v| v >= (1, 2)) && self.xfixes
    }
}

/// The GPU the server draws with, asked on a short connection of its own before anything else
/// starts (see "読み込む Vulkan ドライバ").
pub fn server_main_device() -> Option<u64> {
    let (conn, screen) = XCBConnection::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    main_device(&conn, root)
}

pub(crate) fn main_device(conn: &XCBConnection, root: Window) -> Option<u64> {
    conn.extension_information(dri3::X11_EXTENSION_NAME)
        .ok()
        .flatten()?;
    let reply = conn.dri3_open(root, NONE).ok()?.reply().ok()?;
    let stat = rustix::fs::fstat(&reply.device_fd).ok()?;
    Some(stat.st_rdev)
}

/// What is kept per window presented to.
#[derive(Default)]
pub(crate) struct Frames {
    /// Presentations the server hasn't said are done, oldest first.
    pending: VecDeque<InFlight>,
    pub last_complete: Option<Instant>,
    /// What the next presentation is traced as.
    next_token: Option<u64>,
    /// The server has said a presentation is done: what presents through a GPU surface goes
    /// through Present too, and can be waited for.
    seen_complete: bool,
    /// For the update region of presentations.
    region: Option<xfixes::Region>,
}

struct InFlight {
    /// Our own presentations have serials; those of a GPU surface come in order.
    serial: Option<u32>,
    token: Option<u64>,
    at: Instant,
}

impl Frames {
    /// Whether a presentation is still waiting for the server, forgetting those it won't answer.
    pub fn pending(&mut self, now: Instant) -> bool {
        let seen = self.seen_complete;
        self.pending
            .retain(|f| now - f.at < GIVE_UP && (seen || f.serial.is_some()));
        !self.pending.is_empty()
    }

    /// When the oldest presentation still waiting was made.
    pub fn oldest(&self) -> Option<Instant> {
        self.pending.front().map(|f| f.at)
    }

    pub fn request_presentation(&mut self, token: u64) {
        self.next_token = Some(token);
    }

    /// Something else presented to the window.
    pub fn committed(&mut self) {
        self.pending.push_back(InFlight {
            serial: None,
            token: self.next_token.take(),
            at: Instant::now(),
        });
    }
}

/// A dmabuf, with the pixmaps made of it for the depths it is shown in.
struct Buffer {
    fd: OwnedFd,
    width: u16,
    height: u16,
    modifier: u64,
    planes: Vec<(u32, u32)>,
    pixmaps: Vec<(u8, Pixmap)>,
}

#[derive(Default)]
pub(crate) struct PresentState {
    pub ext: Extensions,
    pub feedback: Option<DmabufFeedback>,
    pub explicit_available: bool,
    explicit_disabled: bool,
    next_id: u64,
    serial: u32,
    buffers: HashMap<BufferId, Buffer>,
    by_pixmap: HashMap<Pixmap, BufferId>,
    pub buffer_events: Vec<BufferEvent>,
    pub presentations: Vec<Presentation>,
    /// The last vblank seen, for the refresh rate.
    last_msc: Option<(u64, u64)>,
    refresh_ns: u32,
}

impl PresentState {
    /// Sets up Present for `window`, with what DRI3 can make, for the main window.
    pub fn new(conn: &XCBConnection, root: Window, window: Window, depth: u8) -> Self {
        let ext = Extensions::query(conn);
        let mut state = Self {
            ext,
            ..Default::default()
        };
        if ext.present.is_none() {
            return state;
        }
        state.select(conn, window);
        if !ext.dmabuf() {
            return state;
        }
        let device = main_device(conn, root);
        let modifiers = conn
            .dri3_get_supported_modifiers(window, depth, 32)
            .ok()
            .and_then(|c| c.reply().ok());
        if let (Some(device), Some(m)) = (device, modifiers) {
            let tranche = |mods: &[u64], scanout| DmabufTranche {
                device,
                formats: mods.iter().map(|&m| (ARGB8888, m)).collect(),
                scanout,
            };
            let mut tranches = Vec::new();
            if !m.window_modifiers.is_empty() {
                tranches.push(tranche(&m.window_modifiers, true));
            }
            if !m.screen_modifiers.is_empty() {
                tranches.push(tranche(&m.screen_modifiers, false));
            }
            if !tranches.is_empty() {
                state.feedback = Some(DmabufFeedback {
                    main_device: device,
                    tranches,
                    generation: 1,
                });
            }
        }
        let syncobj = ext.present.is_some_and(|v| v >= (1, 4))
            && ext.dri3.is_some_and(|v| v >= (1, 4))
            && present::query_capabilities(conn, window)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| r.capabilities & u32::from(present::Capability::SYNCOBJ) != 0);
        state.explicit_available = syncobj;
        state
    }

    /// Asks for what becomes of presentations to `window`.
    pub fn select(&mut self, conn: &XCBConnection, window: Window) {
        let Ok(eid) = conn.generate_id() else { return };
        let mask = present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY;
        let _ = present::select_input(conn, eid, window, mask);
    }

    pub fn clock(&self) -> Option<u32> {
        self.ext.present.map(|_| CLOCK_MONOTONIC)
    }

    pub fn explicit_sync(&self) -> bool {
        self.explicit_available && !self.explicit_disabled
    }

    pub fn disable_explicit_sync(&mut self) {
        self.explicit_disabled = true;
    }

    /// Keeps the dmabuf; pixmaps are made of it when it is first presented, in the depth of the
    /// window it goes to.
    pub fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        modifier: u64,
        planes: &[DmabufPlaneDesc<'_>],
    ) -> Option<BufferId> {
        if !self.ext.dmabuf() || planes.is_empty() || planes.len() > 4 {
            return None;
        }
        let fd = planes[0].fd.try_clone_to_owned().ok()?;
        // Every plane of our buffers is in the one dmabuf.
        if planes
            .iter()
            .any(|p| p.fd.as_raw_fd() != planes[0].fd.as_raw_fd())
        {
            return None;
        }
        self.next_id += 1;
        let id = BufferId(self.next_id);
        self.buffers.insert(
            id,
            Buffer {
                fd,
                width: width.min(u16::MAX as u32) as u16,
                height: height.min(u16::MAX as u32) as u16,
                modifier,
                planes: planes.iter().map(|p| (p.stride, p.offset)).collect(),
                pixmaps: Vec::new(),
            },
        );
        Some(id)
    }

    pub fn destroy_buffer(&mut self, conn: &XCBConnection, id: BufferId) {
        if let Some(b) = self.buffers.remove(&id) {
            for (_, pixmap) in b.pixmaps {
                self.by_pixmap.remove(&pixmap);
                let _ = conn.free_pixmap(pixmap);
            }
        }
    }

    /// The pixmap of a buffer in `depth`, made now if there is none.
    fn pixmap(
        &mut self,
        conn: &XCBConnection,
        id: BufferId,
        window: Window,
        depth: u8,
    ) -> Option<Pixmap> {
        let b = self.buffers.get_mut(&id)?;
        if let Some(&(_, p)) = b.pixmaps.iter().find(|(d, _)| *d == depth) {
            return Some(p);
        }
        let pixmap = conn.generate_id().ok()?;
        let plane = |i: usize| b.planes.get(i).copied().unwrap_or((0, 0));
        let (p0, p1, p2, p3) = (plane(0), plane(1), plane(2), plane(3));
        let fds = (0..b.planes.len())
            .map(|_| b.fd.try_clone())
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let made = conn
            .dri3_pixmap_from_buffers(
                pixmap, window, b.width, b.height, p0.0, p0.1, p1.0, p1.1, p2.0, p2.1, p3.0, p3.1,
                depth, 32, b.modifier, fds,
            )
            .ok()?
            .check();
        if made.is_err() {
            return None;
        }
        b.pixmaps.push((depth, pixmap));
        self.by_pixmap.insert(pixmap, id);
        Some(pixmap)
    }

    /// Presents a buffer to `window` with `damage` as what changed.
    #[allow(clippy::too_many_arguments)]
    pub fn present(
        &mut self,
        conn: &XCBConnection,
        frames: &mut Frames,
        window: Window,
        depth: u8,
        id: BufferId,
        damage: &[RectI],
        sync: Option<SyncPoints>,
    ) {
        let Some(pixmap) = self.pixmap(conn, id, window, depth) else {
            self.buffer_events.push(BufferEvent::Failed(id));
            return;
        };
        let region = match frames.region {
            Some(r) => r,
            None => {
                let Ok(r) = conn.generate_id() else { return };
                if conn.xfixes_create_region(r, &[]).is_err() {
                    return;
                }
                frames.region = Some(r);
                r
            }
        };
        let rects: Vec<Rectangle> = damage.iter().map(|r| rectangle(*r)).collect();
        let update = if rects.is_empty() {
            NONE
        } else {
            let _ = conn.xfixes_set_region(region, &rects);
            region
        };
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        let options = u32::from(present::Option::NONE);
        let sent = match sync {
            Some(s) => present::pixmap_synced(
                conn,
                window,
                pixmap,
                serial,
                NONE,
                update,
                0,
                0,
                NONE,
                s.acquire.0.0 as u32,
                s.release.0.0 as u32,
                s.acquire.1,
                s.release.1,
                options,
                0,
                0,
                0,
                &[],
            )
            .is_ok(),
            None => present::pixmap(
                conn,
                window,
                pixmap,
                serial,
                NONE,
                update,
                0,
                0,
                NONE,
                NONE,
                NONE,
                options,
                0,
                0,
                0,
                &[],
            )
            .is_ok(),
        };
        if sent {
            frames.pending.push_back(InFlight {
                serial: Some(serial),
                token: frames.next_token.take(),
                at: Instant::now(),
            });
        }
    }

    /// Shares a DRM syncobj timeline with the server.
    pub fn import_timeline(
        &mut self,
        conn: &XCBConnection,
        window: Window,
        fd: BorrowedFd<'_>,
    ) -> Option<TimelineId> {
        if !self.explicit_sync() {
            return None;
        }
        let id = conn.generate_id().ok()?;
        let fd = fd.try_clone_to_owned().ok()?;
        conn.dri3_import_syncobj(id, window, fd)
            .ok()?
            .check()
            .ok()?;
        Some(TimelineId(id as u64))
    }

    pub fn destroy_timeline(&mut self, conn: &XCBConnection, id: TimelineId) {
        let _ = conn.dri3_free_syncobj(id.0 as u32);
    }

    /// A pixmap is free again (implicit sync).
    pub fn idle(&mut self, pixmap: Pixmap) {
        if let Some(&id) = self.by_pixmap.get(&pixmap) {
            self.buffer_events.push(BufferEvent::Released(id));
        }
    }

    /// A presentation to the window is done.
    pub fn complete(&mut self, frames: &mut Frames, e: &present::CompleteNotifyEvent) {
        if e.kind != CompleteKind::PIXMAP {
            return;
        }
        frames.seen_complete = true;
        frames.last_complete = Some(Instant::now());
        let done = match frames
            .pending
            .iter()
            .position(|f| f.serial == Some(e.serial))
        {
            Some(i) => {
                let older = frames.pending.drain(..i).filter_map(|f| f.token);
                let older: Vec<u64> = older.collect();
                for token in older {
                    self.presentations.push(Presentation { token, shown: None });
                }
                frames.pending.pop_front()
            }
            None => frames.pending.pop_front(),
        };
        if e.mode != CompleteMode::SKIP {
            self.learn_refresh(e.msc, e.ust);
        }
        let Some(token) = done.and_then(|f| f.token) else {
            return;
        };
        let shown = (e.mode != CompleteMode::SKIP).then(|| Shown {
            time_ns: (e.ust * 1000) as i64,
            refresh_ns: self.refresh_ns,
            flags: if e.mode == CompleteMode::FLIP {
                KIND_VSYNC | KIND_HW_CLOCK | KIND_HW_COMPLETION | KIND_ZERO_COPY
            } else {
                KIND_VSYNC | KIND_HW_CLOCK
            },
        });
        self.presentations.push(Presentation { token, shown });
    }

    /// The time between vblanks, from two frames shown a few vblanks apart.
    fn learn_refresh(&mut self, msc: u64, ust: u64) {
        if let Some((last_msc, last_ust)) = self.last_msc.replace((msc, ust))
            && msc > last_msc
            && msc - last_msc <= 8
            && ust > last_ust
        {
            let ns = (ust - last_ust) * 1000 / (msc - last_msc);
            if (2_000_000..100_000_000).contains(&ns) {
                self.refresh_ns = ns as u32;
            }
        }
    }

    pub fn release(&mut self, conn: &XCBConnection) {
        let ids: Vec<BufferId> = self.buffers.keys().copied().collect();
        for id in ids {
            self.destroy_buffer(conn, id);
        }
    }
}

fn rectangle(r: RectI) -> Rectangle {
    Rectangle {
        x: r.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        y: r.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        width: r.width.clamp(0, u16::MAX as i32) as u16,
        height: r.height.clamp(0, u16::MAX as i32) as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(
        serial: u32,
        mode: CompleteMode,
        msc: u64,
        ust: u64,
    ) -> present::CompleteNotifyEvent {
        present::CompleteNotifyEvent {
            response_type: 0,
            extension: 0,
            sequence: 0,
            length: 0,
            event_type: 0,
            kind: CompleteKind::PIXMAP,
            mode,
            event: 0,
            window: 0,
            serial,
            ust,
            msc,
        }
    }

    #[test]
    fn what_was_skipped_over_and_shown_is_told_with_the_refresh() {
        let mut state = PresentState::default();
        let mut frames = Frames::default();
        for (serial, token) in [(1, 10), (2, 11), (3, 12)] {
            frames.next_token = Some(token);
            frames.pending.push_back(InFlight {
                serial: Some(serial),
                token: frames.next_token.take(),
                at: Instant::now(),
            });
        }
        state.complete(
            &mut frames,
            &complete(1, CompleteMode::FLIP, 100, 1_000_000),
        );
        // The second never came: the third went on top of it.
        state.complete(
            &mut frames,
            &complete(3, CompleteMode::COPY, 102, 1_016_666),
        );
        assert!(!frames.pending(Instant::now()));
        let p = &state.presentations;
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].token, 10);
        assert_eq!(p[0].shown.unwrap().flags, 15);
        assert_eq!((p[1].token, p[1].shown), (11, None));
        let third = p[2].shown.unwrap();
        assert_eq!(third.time_ns, 1_016_666_000);
        assert_eq!(third.refresh_ns, 8_333_000);
        assert_eq!(third.flags, KIND_VSYNC | KIND_HW_CLOCK);
    }

    #[test]
    fn a_gpu_surface_is_waited_for_only_once_present_answers_for_it() {
        let mut frames = Frames::default();
        frames.committed();
        assert!(!frames.pending(Instant::now()), "may never be answered");
        frames.seen_complete = true;
        frames.committed();
        assert!(frames.pending(Instant::now()));
        assert!(!frames.pending(Instant::now() + GIVE_UP), "not for ever");
    }
}
