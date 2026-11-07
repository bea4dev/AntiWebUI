//! The clipboard of the whole desktop, whoever owns it, and what was on it before
//! (`wlr_data_control`).

use crate::conn::{State as Conn, link};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex, OnceLock};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as device, zwlr_data_control_manager_v1 as manager,
    zwlr_data_control_offer_v1 as offer, zwlr_data_control_source_v1 as source,
};

/// Text, in the order of preference.
const TEXT: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];
/// What password managers put next to what shouldn't be kept.
const SECRET: &str = "x-kde-passwordManagerHint";

/// The clipboard. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Clipboard(Option<&'static Link>);

#[derive(Debug)]
struct Link {
    manager: manager::ZwlrDataControlManagerV1,
    device: device::ZwlrDataControlDeviceV1,
    shared: Arc<Shared>,
}

#[derive(Debug)]
pub(crate) struct Shared {
    text: SharedSignal<Option<String>>,
    primary: SharedSignal<Option<String>>,
    history: SharedSignal<Vec<String>>,
    limit: Mutex<usize>,
}

impl Shared {
    fn took(&self, primary: bool, text: String, keep: bool) {
        if primary {
            self.primary.set(Some(text));
            return;
        }
        if keep && !text.trim().is_empty() {
            let limit = self.limit.lock().map(|l| *l).unwrap_or(50);
            self.history.update(|h| {
                h.retain(|t| *t != text);
                h.insert(0, text.clone());
                h.truncate(limit);
            });
        }
        self.text.set(Some(text));
    }
}

static LINK: OnceLock<Option<Link>> = OnceLock::new();

/// The clipboard. It starts watching the first time.
pub fn clipboard() -> Clipboard {
    Clipboard(
        LINK.get_or_init(|| {
            let link = link()?;
            let seat = link.seat.as_ref()?;
            let manager: manager::ZwlrDataControlManagerV1 = link.bind(2, ())?;
            let shared = Arc::new(Shared {
                text: shared_signal(None),
                primary: shared_signal(None),
                history: shared_signal(Vec::new()),
                limit: Mutex::new(50),
            });
            let device = manager.get_data_device(seat, &link.qh, shared.clone());
            link.sync();
            Some(Link {
                manager,
                device,
                shared,
            })
        })
        .as_ref(),
    )
}

impl Clipboard {
    pub fn available(&self) -> bool {
        self.0.is_some()
    }

    /// The text on the clipboard, if it has some.
    pub fn text(&self) -> Option<String> {
        self.0.and_then(|l| l.shared.text.get())
    }

    /// The text selected last, which a middle click pastes.
    pub fn primary(&self) -> Option<String> {
        self.0.and_then(|l| l.shared.primary.get())
    }

    /// What was copied since this started watching, the latest first, each once. What a password
    /// manager marks as secret is left out.
    pub fn history(&self) -> Vec<String> {
        self.0.map(|l| l.shared.history.get()).unwrap_or_default()
    }

    pub fn clear_history(&self) {
        if let Some(l) = self.0 {
            l.shared.history.set(Vec::new());
        }
    }

    /// How much of it is kept; 50 at first.
    pub fn set_history_limit(&self, limit: usize) {
        if let Some(l) = self.0 {
            if let Ok(mut n) = l.shared.limit.lock() {
                *n = limit;
            }
            l.shared.history.update(|h| h.truncate(limit));
        }
    }

    /// Puts text on the clipboard, for every program.
    pub fn set_text(&self, text: impl Into<String>) {
        let (Some(l), Some(conn)) = (self.0, link()) else {
            return;
        };
        let s = l
            .manager
            .create_data_source(&conn.qh, Arc::new(text.into()));
        for mime in TEXT {
            s.offer(mime.to_owned());
        }
        l.device.set_selection(Some(&s));
        conn.flush();
    }
}

/// The offers going on, which go when another takes their place.
#[derive(Default)]
pub(crate) struct State {
    selection: Option<offer::ZwlrDataControlOfferV1>,
    primary: Option<offer::ZwlrDataControlOfferV1>,
}

type Mimes = Arc<Mutex<Vec<String>>>;

impl Dispatch<device::ZwlrDataControlDeviceV1, Arc<Shared>> for Conn {
    fn event(
        conn: &mut Self,
        _: &device::ZwlrDataControlDeviceV1,
        event: device::Event,
        shared: &Arc<Shared>,
        wl: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let (new, primary) = match event {
            device::Event::Selection { id } => (id, false),
            device::Event::PrimarySelection { id } => (id, true),
            _ => return,
        };
        let slot = if primary {
            &mut conn.clipboard.primary
        } else {
            &mut conn.clipboard.selection
        };
        if let Some(old) = std::mem::replace(slot, new.clone()) {
            old.destroy();
        }
        let Some(new) = new else {
            return;
        };
        let mimes = new
            .data::<Mimes>()
            .and_then(|m| m.lock().ok().map(|m| m.clone()))
            .unwrap_or_default();
        let Some(mime) = TEXT.iter().find(|t| mimes.iter().any(|m| m == *t)) else {
            return;
        };
        let keep = !mimes.iter().any(|m| m == SECRET);
        let Ok((read, write)) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC) else {
            return;
        };
        new.receive(mime.to_string(), write.as_fd());
        let _ = wl.flush();
        drop(write);
        let shared = shared.clone();
        // Whoever owns it may take its time writing; this thread doesn't wait.
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if std::fs::File::from(read).read_to_end(&mut buf).is_ok() {
                let text = String::from_utf8_lossy(&buf).replace("\r\n", "\n");
                shared.took(primary, text, keep);
            }
        });
    }

    event_created_child!(Conn, device::ZwlrDataControlDeviceV1, [
        device::EVT_DATA_OFFER_OPCODE => (offer::ZwlrDataControlOfferV1, Mimes::default()),
    ]);
}

impl Dispatch<offer::ZwlrDataControlOfferV1, Mimes> for Conn {
    fn event(
        _: &mut Self,
        _: &offer::ZwlrDataControlOfferV1,
        event: offer::Event,
        mimes: &Mimes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer::Event::Offer { mime_type } = event
            && let Ok(mut m) = mimes.lock()
        {
            m.push(mime_type);
        }
    }
}

impl Dispatch<source::ZwlrDataControlSourceV1, Arc<String>> for Conn {
    fn event(
        _: &mut Self,
        source: &source::ZwlrDataControlSourceV1,
        event: source::Event,
        text: &Arc<String>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            source::Event::Send { fd, .. } => {
                let text = text.clone();
                std::thread::spawn(move || {
                    let _ = std::fs::File::from(fd).write_all(text.as_bytes());
                });
            }
            source::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Conn: ignore manager::ZwlrDataControlManagerV1);
