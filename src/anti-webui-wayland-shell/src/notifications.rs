//! Being the notification daemon: owning `org.freedesktop.Notifications` on the session bus, keeping
//! what programs send, and telling them when one is clicked or closed.

use crate::ServiceError;
use crate::dbus::{self, Props, get, string};
use anti_webui_core::images::Bitmap;
use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// How long one shows when the program leaves it to the server.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How much a notification matters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    /// Shows even when not to be disturbed, and until dismissed.
    Critical,
}

/// Why one was closed, as the specification numbers them.
#[derive(Clone, Copy)]
enum Closed {
    Expired = 1,
    Dismissed = 2,
    ByProgram = 3,
}

/// A notification, as it was when read.
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// An icon name or a `file://` path, as the program gave it.
    pub app_icon: String,
    pub summary: String,
    /// With markup, of which only `b`, `i`, `u`, `a` and `img` are left; entities stay as they
    /// are, ready for `rich_text`; [`Notification::body_text`] has it as plain text.
    pub body: String,
    /// Each action's key and label. `default` is what clicking the notification does.
    pub actions: Vec<(String, String)>,
    pub urgency: Urgency,
    /// A picture sent with it.
    pub image: Option<Arc<Bitmap>>,
    /// A picture to show by name or path, when it came as one.
    pub image_path: Option<String>,
    /// How long it shows; `None` until dismissed.
    pub expires: Option<Duration>,
    /// Stays after an action is invoked.
    pub resident: bool,
    /// Not to be kept once it has shown.
    pub transient: bool,
    /// The program's `.desktop` file, without `.desktop`.
    pub desktop_entry: Option<String>,
    received: Instant,
}

impl PartialEq for Notification {
    fn eq(&self, other: &Self) -> bool {
        // The same picture is the same bitmap.
        let image = |n: &Self| n.image.as_ref().map(|b| b.id());
        self.id == other.id
            && self.received == other.received
            && self.app_name == other.app_name
            && self.app_icon == other.app_icon
            && self.summary == other.summary
            && self.body == other.body
            && self.actions == other.actions
            && self.urgency == other.urgency
            && image(self) == image(other)
            && self.image_path == other.image_path
            && self.expires == other.expires
            && self.resident == other.resident
            && self.transient == other.transient
            && self.desktop_entry == other.desktop_entry
    }
}

impl Notification {
    /// When it came.
    pub fn received(&self) -> Instant {
        self.received
    }

    /// The body as plain text: no tags, entities decoded, `<br>` as new lines.
    pub fn body_text(&self) -> String {
        decode_entities(&strip_tags(&self.body))
    }

    /// Does an action, like clicking a button of it, and closes it unless it is resident.
    pub fn invoke(&self, key: &str) {
        if let Some(server) = server() {
            server.invoke(self.id, key, self.resident);
        }
    }

    /// Closes it, as its close button would.
    pub fn dismiss(&self) {
        if let Some(server) = server() {
            server.close(self.id, Closed::Dismissed);
        }
    }
}

/// The notification daemon. Cheap to clone; reading tracks, on any thread.
#[derive(Clone, Debug)]
pub struct NotificationServer(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    conn: Connection,
    list: SharedSignal<Vec<Notification>>,
    /// The ones still to be shown, by id.
    popups: SharedSignal<Vec<u32>>,
    do_not_disturb: SharedSignal<bool>,
    next: AtomicU32,
    timers: Mutex<Sender<(Instant, u32, Instant)>>,
}

static SERVER: Mutex<Option<NotificationServer>> = Mutex::new(None);

fn server() -> Option<Arc<Inner>> {
    SERVER.lock().ok()?.as_ref().map(|s| s.0.clone())
}

/// Becomes the notification daemon, or is it already. When another program is,
/// [`ServiceError::NameTaken`]; asking again later tries again.
pub fn notification_server() -> Result<NotificationServer, ServiceError> {
    let mut slot = SERVER.lock().map_err(|_| ServiceError::Unavailable)?;
    if let Some(s) = slot.as_ref() {
        return Ok(s.clone());
    }
    let (tx, rx) = channel();
    dbus::spawn(async move {
        let _ = tx.send(start().await);
    });
    let server = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| ServiceError::Failed("the bus didn't answer".into()))??;
    *slot = Some(server.clone());
    Ok(server)
}

async fn start() -> Result<NotificationServer, ServiceError> {
    let conn = dbus::session().await?;
    let (timers, due) = channel();
    let inner = Arc::new(Inner {
        conn: conn.clone(),
        list: shared_signal(Vec::new()),
        popups: shared_signal(Vec::new()),
        do_not_disturb: shared_signal(false),
        next: AtomicU32::new(1),
        timers: Mutex::new(timers),
    });
    let objects = conn.object_server();
    objects
        .at(PATH, Daemon(inner.clone()))
        .await
        .map_err(dbus::failed)?;
    let reply = conn
        .request_name_with_flags(NAME, RequestNameFlags::DoNotQueue.into())
        .await
        .map_err(dbus::failed);
    match reply {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        other => {
            let _ = objects.remove::<Daemon, _>(PATH).await;
            return Err(match other {
                Err(e) => e,
                Ok(_) => ServiceError::NameTaken,
            });
        }
    }
    let weak = Arc::downgrade(&inner);
    std::thread::Builder::new()
        .name("anti-webui notifications".into())
        .spawn(move || expire(due, weak))
        .map_err(|e| ServiceError::Failed(e.to_string()))?;
    Ok(NotificationServer(inner))
}

/// Takes notifications off the screen when their time is up.
fn expire(due: std::sync::mpsc::Receiver<(Instant, u32, Instant)>, server: std::sync::Weak<Inner>) {
    // Soonest first: when, which, and when the one it was for came.
    let mut heap: BinaryHeap<std::cmp::Reverse<(Instant, u32, Instant)>> = BinaryHeap::new();
    loop {
        let next = heap.peek().map(|t| t.0.0);
        let got = match next {
            Some(at) => due.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => due
                .recv()
                .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(t) => heap.push(std::cmp::Reverse(t)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        while let Some(std::cmp::Reverse((at, id, received))) = heap.peek().copied() {
            if at > now {
                break;
            }
            heap.pop();
            let Some(server) = server.upgrade() else {
                return;
            };
            // Unless it was replaced since.
            let current = server
                .list
                .with_untracked(|l| l.iter().any(|n| n.id == id && n.received == received));
            if current {
                server.timed_out(id);
            }
        }
    }
}

impl NotificationServer {
    /// All that are kept, the newest first.
    pub fn notifications(&self) -> Vec<Notification> {
        self.0.list.get()
    }

    /// Those to be on the screen now: not expired, not dismissed, and not come while not to be
    /// disturbed unless critical. The newest first.
    pub fn popups(&self) -> Vec<Notification> {
        let shown = self.0.popups.get();
        self.0.list.with(|l| {
            l.iter()
                .filter(|n| shown.contains(&n.id))
                .cloned()
                .collect()
        })
    }

    pub fn do_not_disturb(&self) -> bool {
        self.0.do_not_disturb.get()
    }

    /// While on, what comes is kept but doesn't pop up, unless it is critical.
    pub fn set_do_not_disturb(&self, on: bool) {
        self.0.do_not_disturb.set(on);
    }

    /// Dismisses one.
    pub fn remove(&self, id: u32) {
        self.0.close(id, Closed::Dismissed);
    }

    /// Dismisses them all.
    pub fn clear(&self) {
        let ids: Vec<u32> = self
            .0
            .list
            .with_untracked(|l| l.iter().map(|n| n.id).collect());
        for id in ids {
            self.0.close(id, Closed::Dismissed);
        }
    }
}

impl Inner {
    fn emit_closed(&self, id: u32, why: Closed) {
        let conn = self.conn.clone();
        dbus::spawn(async move {
            if let Ok(emitter) = SignalEmitter::new(&conn, PATH) {
                let _ = Daemon::notification_closed(&emitter, id, why as u32).await;
            }
        });
    }

    /// Takes one away, telling the program why.
    fn close(&self, id: u32, why: Closed) {
        let had = self.list.with_untracked(|l| l.iter().any(|n| n.id == id));
        if !had {
            return;
        }
        self.list.update(|l| l.retain(|n| n.id != id));
        self.popups.update(|p| p.retain(|n| *n != id));
        self.emit_closed(id, why);
    }

    fn timed_out(&self, id: u32) {
        self.popups.update(|p| p.retain(|n| *n != id));
        let transient = self
            .list
            .with_untracked(|l| l.iter().any(|n| n.id == id && n.transient));
        if transient {
            self.list.update(|l| l.retain(|n| n.id != id));
        }
        self.emit_closed(id, Closed::Expired);
    }

    fn invoke(&self, id: u32, key: &str, resident: bool) {
        let conn = self.conn.clone();
        let key = key.to_owned();
        dbus::spawn(async move {
            if let Ok(emitter) = SignalEmitter::new(&conn, PATH) {
                let _ = Daemon::action_invoked(&emitter, id, &key).await;
            }
        });
        if !resident {
            self.close(id, Closed::Dismissed);
        }
    }

    /// Keeps a notification a program sent, in place of the one it replaces.
    fn add(&self, n: Notification) {
        let id = n.id;
        let popup = n.urgency == Urgency::Critical || !self.do_not_disturb.get_untracked();
        if let (Some(after), Ok(timers)) = (n.expires, self.timers.lock()) {
            let _ = timers.send((n.received + after, id, n.received));
        }
        self.list.update(|l| {
            l.retain(|old| old.id != id);
            l.insert(0, n);
        });
        self.popups.update(|p| {
            p.retain(|old| *old != id);
            if popup {
                p.insert(0, id);
            }
        });
    }
}

/// What is served on the bus.
struct Daemon(Arc<Inner>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Daemon {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: Props,
        expire_timeout: i32,
    ) -> u32 {
        let replaces = replaces_id != 0
            && self
                .0
                .list
                .with_untracked(|l| l.iter().any(|n| n.id == replaces_id));
        let id = if replaces {
            replaces_id
        } else {
            self.0.next.fetch_add(1, Ordering::Relaxed)
        };
        let urgency = match get::<u8>(&hints, "urgency") {
            Some(0) => Urgency::Low,
            Some(2) => Urgency::Critical,
            _ => Urgency::Normal,
        };
        let expires = match expire_timeout {
            0 => None,
            t if t > 0 => Some(Duration::from_millis(t as u64)),
            _ if urgency == Urgency::Critical => None,
            _ => Some(DEFAULT_TIMEOUT),
        };
        let image = ["image-data", "image_data", "icon_data"]
            .iter()
            .find_map(|k| hints.get(*k).and_then(image_of));
        let image_path = ["image-path", "image_path"]
            .iter()
            .map(|k| string(&hints, k))
            .find(|p| !p.is_empty());
        self.0.add(Notification {
            id,
            app_name,
            app_icon,
            summary,
            body: sanitize(&body),
            actions: actions
                .chunks_exact(2)
                .map(|a| (a[0].clone(), a[1].clone()))
                .collect(),
            urgency,
            image,
            image_path,
            expires,
            resident: get(&hints, "resident").unwrap_or(false),
            transient: get(&hints, "transient").unwrap_or(false),
            desktop_entry: get::<String>(&hints, "desktop-entry").filter(|d| !d.is_empty()),
            received: Instant::now(),
        });
        id
    }

    fn close_notification(&self, id: u32) {
        self.0.close(id, Closed::ByProgram);
    }

    fn get_capabilities(&self) -> Vec<&'static str> {
        vec![
            "actions",
            "body",
            "body-markup",
            "body-hyperlinks",
            "body-images",
            "icon-static",
            "persistence",
        ]
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("anti-webui", "anti-webui", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

/// A picture in a hint: width, height, row stride, alpha, bits per sample, channels and bytes.
fn image_of(v: &OwnedValue) -> Option<Arc<Bitmap>> {
    let (w, h, stride, alpha, bits, channels, data) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(v.clone()).ok()?;
    image_from(w, h, stride, alpha, bits, channels, &data)
}

fn image_from(
    w: i32,
    h: i32,
    stride: i32,
    alpha: bool,
    bits: i32,
    channels: i32,
    data: &[u8],
) -> Option<Arc<Bitmap>> {
    let channels = channels as usize;
    if w <= 0 || h <= 0 || bits != 8 || !(3..=4).contains(&channels) || alpha != (channels == 4) {
        return None;
    }
    let (w, h, stride) = (w as usize, h as usize, stride as usize);
    if stride < w * channels || data.len() < stride * (h - 1) + w * channels {
        return None;
    }
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for px in data[y * stride..y * stride + w * channels].chunks_exact(channels) {
            let a = if alpha { px[3] as u32 } else { 255 };
            let pre = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            pixels.extend_from_slice(&[pre(px[0]), pre(px[1]), pre(px[2]), a as u8]);
        }
    }
    Some(Arc::new(Bitmap::from_premultiplied(
        w as u32, h as u32, pixels, !alpha,
    )))
}

/// The tags a body may keep.
const KEPT: [&str; 5] = ["b", "i", "u", "a", "img"];

/// The name of a tag, `<…>` without the brackets, and whether it closes.
fn tag_name(tag: &str) -> (String, bool) {
    let tag = tag.trim();
    let (closing, rest) = match tag.strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, tag),
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (name.to_ascii_lowercase(), closing)
}

/// Goes through `body`, calling `tag` for each tag and keeping what it gives in its place; a `<`
/// that starts no tag becomes `&lt;`.
fn retag(body: &str, mut tag: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        match after.find('>') {
            Some(end) => {
                if let Some(kept) = tag(&after[..end]) {
                    out.push_str(&kept);
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("&lt;");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Leaves only the tags in [`KEPT`], and `<br>` as a new line.
fn sanitize(body: &str) -> String {
    retag(body, |tag| {
        let (name, _) = tag_name(tag);
        if name == "br" {
            Some("\n".into())
        } else if KEPT.contains(&name.as_str()) {
            Some(format!("<{tag}>"))
        } else {
            None
        }
    })
}

fn strip_tags(body: &str) -> String {
    retag(body, |tag| (tag_name(tag).0 == "br").then(|| "\n".into()))
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let decoded = after.find(';').filter(|e| *e <= 10).and_then(|end| {
            let name = &after[..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                n => n
                    .strip_prefix("#x")
                    .or_else(|| n.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| n.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }?;
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_keep_only_simple_markup() {
        let body =
            "<b>Hi</b> <span color='red'>there</span><br/>a <a href=\"x\">link</a> &amp; 1 < 2";
        assert_eq!(
            sanitize(body),
            "<b>Hi</b> there\na <a href=\"x\">link</a> &amp; 1 &lt; 2"
        );
        let n = sanitize(body);
        assert_eq!(decode_entities(&strip_tags(&n)), "Hi there\na link & 1 < 2");
        assert_eq!(decode_entities("&#65;&#x42;&bogus; &"), "AB&bogus; &");
    }

    #[test]
    fn pictures_in_hints_become_premultiplied() {
        // 1x1 RGBA, half transparent red; 2x1 RGB with a padded stride.
        let b = image_from(1, 1, 4, true, 8, 4, &[255, 0, 0, 128]).unwrap();
        assert!(!b.is_opaque());
        let mut px = Vec::new();
        b.with_levels(|_, _, l| px = l.to_vec());
        assert_eq!(px, [128, 0, 0, 128]);
        let b = image_from(2, 1, 8, false, 8, 3, &[1, 2, 3, 4, 5, 6, 0, 0]).unwrap();
        b.with_levels(|_, _, l| px = l.to_vec());
        assert_eq!(px, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert!(image_from(1, 1, 4, true, 16, 4, &[0; 8]).is_none());
    }
}
