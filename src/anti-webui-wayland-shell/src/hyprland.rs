//! Hyprland's IPC: commands on `.socket.sock`, events on `.socket2.sock`, in the directory of the
//! running instance.

use crate::ServiceError;
use crate::ipc::{Listeners, failed, flag, int, text};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::{Task, task};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The directory of the running instance's sockets.
fn socket_dir() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let mut dirs = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        dirs.push(PathBuf::from(runtime).join("hypr").join(&signature));
    }
    dirs.push(PathBuf::from("/tmp/hypr").join(&signature));
    dirs.into_iter().find(|d| d.join(".socket.sock").exists())
}

/// Whether Hyprland is running here.
pub fn available() -> bool {
    socket_dir().is_some()
}

fn request_now(command: &str) -> Result<String, ServiceError> {
    let dir = socket_dir().ok_or(ServiceError::Unavailable)?;
    let mut stream = UnixStream::connect(dir.join(".socket.sock")).map_err(failed)?;
    stream.write_all(command.as_bytes()).map_err(failed)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).map_err(failed)?;
    Ok(reply)
}

fn json_now(command: &str) -> Result<Value, ServiceError> {
    serde_json::from_str(&request_now(&format!("j/{command}"))?).map_err(failed)
}

/// Sends anything `hyprctl` would, like `dispatch workspace 3`, and gives the reply.
pub fn request(command: &str) -> Task<Result<String, ServiceError>> {
    let command = command.to_owned();
    task(move || request_now(&command))
}

/// Asks for JSON, like `devices` for what `hyprctl -j devices` gives.
pub fn request_json(command: &str) -> Task<Result<Value, ServiceError>> {
    let command = command.to_owned();
    task(move || json_now(&command))
}

fn expect_ok(command: String) -> Task<Result<(), ServiceError>> {
    task(move || {
        let reply = request_now(&command)?;
        if reply.trim() == "ok" {
            Ok(())
        } else {
            Err(ServiceError::Failed(reply.trim().to_owned()))
        }
    })
}

/// Runs a dispatcher, like `workspace 3` or `exec kitty`.
pub fn dispatch(args: &str) -> Task<Result<(), ServiceError>> {
    expect_ok(format!("dispatch {args}"))
}

/// Changes an option for now, like `general:gaps_out` to `8`.
pub fn keyword(key: &str, value: &str) -> Task<Result<(), ServiceError>> {
    expect_ok(format!("keyword {key} {value}"))
}

/// A workspace as monitors and windows refer to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceRef {
    pub id: i64,
    pub name: String,
}

impl WorkspaceRef {
    fn of(v: Option<&Value>) -> Self {
        v.map(|v| Self {
            id: int(v, "id"),
            name: text(v, "name"),
        })
        .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub width: i64,
    pub height: i64,
    pub x: i64,
    pub y: i64,
    pub scale: f64,
    pub focused: bool,
    pub active_workspace: WorkspaceRef,
    /// Id 0 when none is shown.
    pub special_workspace: WorkspaceRef,
}

impl Monitor {
    fn of(v: &Value) -> Self {
        Self {
            id: int(v, "id"),
            name: text(v, "name"),
            description: text(v, "description"),
            width: int(v, "width"),
            height: int(v, "height"),
            x: int(v, "x"),
            y: int(v, "y"),
            scale: v.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
            focused: flag(v, "focused"),
            active_workspace: WorkspaceRef::of(v.get("activeWorkspace")),
            special_workspace: WorkspaceRef::of(v.get("specialWorkspace")),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    /// Below 0 for special workspaces.
    pub id: i64,
    pub name: String,
    pub monitor: String,
    pub windows: i64,
    pub has_fullscreen: bool,
    pub last_window_title: String,
}

impl Workspace {
    fn of(v: &Value) -> Self {
        Self {
            id: int(v, "id"),
            name: text(v, "name"),
            monitor: text(v, "monitor"),
            windows: int(v, "windows"),
            has_fullscreen: flag(v, "hasfullscreen"),
            last_window_title: text(v, "lastwindowtitle"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Client {
    pub address: String,
    pub class: String,
    pub title: String,
    pub workspace: WorkspaceRef,
    pub monitor: i64,
    pub floating: bool,
    pub fullscreen: bool,
    pub pid: i64,
    pub at: (i64, i64),
    pub size: (i64, i64),
    /// 0 for the window with the keyboard, then counting back.
    pub focus_history_id: i64,
}

impl Client {
    fn of(v: &Value) -> Self {
        let pair = |key: &str| {
            let a = v.get(key).and_then(Value::as_array);
            let at = |i: usize| {
                a.and_then(|a| a.get(i))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            };
            (at(0), at(1))
        };
        Self {
            address: text(v, "address"),
            class: text(v, "class"),
            title: text(v, "title"),
            workspace: WorkspaceRef::of(v.get("workspace")),
            monitor: int(v, "monitor"),
            floating: flag(v, "floating"),
            fullscreen: flag(v, "fullscreen"),
            pid: int(v, "pid"),
            at: pair("at"),
            size: pair("size"),
            focus_history_id: int(v, "focusHistoryID"),
        }
    }
}

/// What `.socket2.sock` tells. Where Hyprland sends an event twice, as `workspace` and
/// `workspacev2`, the one with more in it is given its own variant and the other is
/// [`Event::Other`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `workspacev2`
    Workspace {
        id: i64,
        name: String,
    },
    /// `focusedmon`
    FocusedMonitor {
        monitor: String,
        workspace: String,
    },
    /// `activewindow`
    ActiveWindow {
        class: String,
        title: String,
    },
    /// `activewindowv2`
    ActiveWindowAddress(String),
    /// `openwindow`
    OpenWindow {
        address: String,
        workspace: String,
        class: String,
        title: String,
    },
    /// `closewindow`
    CloseWindow {
        address: String,
    },
    /// `createworkspacev2`
    CreateWorkspace {
        id: i64,
        name: String,
    },
    /// `destroyworkspacev2`
    DestroyWorkspace {
        id: i64,
        name: String,
    },
    /// `moveworkspacev2`
    MoveWorkspace {
        id: i64,
        name: String,
        monitor: String,
    },
    /// `renameworkspace`
    RenameWorkspace {
        id: i64,
        name: String,
    },
    /// `activespecial`
    ActiveSpecial {
        name: String,
        monitor: String,
    },
    /// `urgent`
    Urgent {
        address: String,
    },
    /// `fullscreen`
    Fullscreen(bool),
    /// `monitoradded`
    MonitorAdded {
        name: String,
    },
    /// `monitorremoved`
    MonitorRemoved {
        name: String,
    },
    /// `submap`
    Submap(String),
    Other {
        name: String,
        data: String,
    },
}

impl Event {
    /// The name Hyprland gives it.
    pub fn name(&self) -> &str {
        match self {
            Event::Workspace { .. } => "workspacev2",
            Event::FocusedMonitor { .. } => "focusedmon",
            Event::ActiveWindow { .. } => "activewindow",
            Event::ActiveWindowAddress(_) => "activewindowv2",
            Event::OpenWindow { .. } => "openwindow",
            Event::CloseWindow { .. } => "closewindow",
            Event::CreateWorkspace { .. } => "createworkspacev2",
            Event::DestroyWorkspace { .. } => "destroyworkspacev2",
            Event::MoveWorkspace { .. } => "moveworkspacev2",
            Event::RenameWorkspace { .. } => "renameworkspace",
            Event::ActiveSpecial { .. } => "activespecial",
            Event::Urgent { .. } => "urgent",
            Event::Fullscreen(_) => "fullscreen",
            Event::MonitorAdded { .. } => "monitoradded",
            Event::MonitorRemoved { .. } => "monitorremoved",
            Event::Submap(_) => "submap",
            Event::Other { name, .. } => name,
        }
    }
}

/// One line of `.socket2.sock`, `NAME>>DATA`.
pub(crate) fn parse_event(line: &str) -> Option<Event> {
    let (name, data) = line.split_once(">>")?;
    let fields = |n: usize| -> Vec<String> { data.splitn(n, ',').map(str::to_owned).collect() };
    let id = |s: &str| s.parse::<i64>().unwrap_or(0);
    let e = match name {
        "workspacev2" | "createworkspacev2" | "destroyworkspacev2" => {
            let f = fields(2);
            let (i, n) = (id(&f[0]), f.get(1).cloned().unwrap_or_default());
            match name {
                "workspacev2" => Event::Workspace { id: i, name: n },
                "createworkspacev2" => Event::CreateWorkspace { id: i, name: n },
                _ => Event::DestroyWorkspace { id: i, name: n },
            }
        }
        "moveworkspacev2" => {
            let f = fields(3);
            Event::MoveWorkspace {
                id: id(&f[0]),
                name: f.get(1).cloned().unwrap_or_default(),
                monitor: f.get(2).cloned().unwrap_or_default(),
            }
        }
        "renameworkspace" => {
            let f = fields(2);
            Event::RenameWorkspace {
                id: id(&f[0]),
                name: f.get(1).cloned().unwrap_or_default(),
            }
        }
        "focusedmon" => {
            let f = fields(2);
            Event::FocusedMonitor {
                monitor: f[0].clone(),
                workspace: f.get(1).cloned().unwrap_or_default(),
            }
        }
        "activewindow" => {
            let f = fields(2);
            Event::ActiveWindow {
                class: f[0].clone(),
                title: f.get(1).cloned().unwrap_or_default(),
            }
        }
        "activewindowv2" => Event::ActiveWindowAddress(data.to_owned()),
        "openwindow" => {
            let f = fields(4);
            Event::OpenWindow {
                address: f[0].clone(),
                workspace: f.get(1).cloned().unwrap_or_default(),
                class: f.get(2).cloned().unwrap_or_default(),
                title: f.get(3).cloned().unwrap_or_default(),
            }
        }
        "closewindow" => Event::CloseWindow {
            address: data.to_owned(),
        },
        "activespecial" => {
            let f = fields(2);
            Event::ActiveSpecial {
                name: f[0].clone(),
                monitor: f.get(1).cloned().unwrap_or_default(),
            }
        }
        "urgent" => Event::Urgent {
            address: data.to_owned(),
        },
        "fullscreen" => Event::Fullscreen(data == "1"),
        "monitoradded" => Event::MonitorAdded {
            name: data.to_owned(),
        },
        "monitorremoved" => Event::MonitorRemoved {
            name: data.to_owned(),
        },
        "submap" => Event::Submap(data.to_owned()),
        _ => Event::Other {
            name: name.to_owned(),
            data: data.to_owned(),
        },
    };
    Some(e)
}

/// What an event makes worth asking again.
#[derive(Default, Clone, Copy)]
struct Stale {
    monitors: bool,
    workspaces: bool,
    clients: bool,
}

fn staled_by(name: &str) -> Stale {
    let workspaces = [
        "workspace",
        "workspacev2",
        "focusedmon",
        "focusedmonv2",
        "createworkspace",
        "createworkspacev2",
        "destroyworkspace",
        "destroyworkspacev2",
        "moveworkspace",
        "moveworkspacev2",
        "renameworkspace",
        "activespecial",
        "activespecialv2",
    ];
    let windows = [
        "openwindow",
        "closewindow",
        "movewindow",
        "movewindowv2",
        "activewindow",
        "activewindowv2",
        "windowtitle",
        "windowtitlev2",
        "changefloatingmode",
        "fullscreen",
        "urgent",
        "minimized",
        "pin",
    ];
    let monitors = [
        "monitoradded",
        "monitoraddedv2",
        "monitorremoved",
        "monitorremovedv2",
    ];
    let w = workspaces.contains(&name);
    let c = windows.contains(&name);
    let m = monitors.contains(&name);
    Stale {
        monitors: w || m,
        workspaces: w || c || m,
        clients: c,
    }
}

struct Tracker {
    monitors: SharedSignal<Vec<Monitor>>,
    workspaces: SharedSignal<Vec<Workspace>>,
    clients: SharedSignal<Vec<Client>>,
    active: SharedSignal<Option<Client>>,
}

static LISTENERS: Listeners<Event> = Listeners::new();
static TRACKER: OnceLock<Tracker> = OnceLock::new();

fn set_if_changed<T: PartialEq + Send + 'static>(s: &SharedSignal<T>, value: T) {
    if s.with_untracked(|v| *v != value) {
        s.set(value);
    }
}

impl Tracker {
    fn refresh(&self, stale: Stale) {
        let list =
            |command: &str| -> Option<Vec<Value>> { json_now(command).ok()?.as_array().cloned() };
        if stale.monitors
            && let Some(l) = list("monitors")
        {
            set_if_changed(&self.monitors, l.iter().map(Monitor::of).collect());
        }
        if stale.workspaces
            && let Some(l) = list("workspaces")
        {
            let mut w: Vec<Workspace> = l.iter().map(Workspace::of).collect();
            w.sort_by_key(|w| w.id);
            set_if_changed(&self.workspaces, w);
        }
        if stale.clients {
            if let Some(l) = list("clients") {
                set_if_changed(&self.clients, l.iter().map(Client::of).collect());
            }
            if let Ok(v) = json_now("activewindow") {
                let active = v.get("address").is_some().then(|| Client::of(&v));
                set_if_changed(&self.active, active);
            }
        }
    }
}

/// Reads everything once and then follows the events, the first time.
fn tracker() -> &'static Tracker {
    TRACKER.get_or_init(|| {
        let t = Tracker {
            monitors: shared_signal(Vec::new()),
            workspaces: shared_signal(Vec::new()),
            clients: shared_signal(Vec::new()),
            active: shared_signal(None),
        };
        t.refresh(Stale {
            monitors: true,
            workspaces: true,
            clients: true,
        });
        if let Some(dir) = socket_dir()
            && let Ok(stream) = UnixStream::connect(dir.join(".socket2.sock"))
        {
            let _ = std::thread::Builder::new()
                .name("anti-webui hyprland".into())
                .spawn(move || follow(stream));
        }
        t
    })
}

fn follow(stream: UnixStream) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut stale = Stale::default();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if let Some(e) = parse_event(line.trim_end_matches('\n')) {
            let s = staled_by(e.name());
            stale.monitors |= s.monitors;
            stale.workspaces |= s.workspaces;
            stale.clients |= s.clients;
            LISTENERS.send(&e);
        }
        // A burst of events is asked about once.
        if reader.buffer().is_empty()
            && let Some(t) = TRACKER.get()
        {
            t.refresh(std::mem::take(&mut stale));
        }
    }
}

/// The monitors. Reading tracks, on any thread.
pub fn monitors() -> Vec<Monitor> {
    tracker().monitors.get()
}

/// The workspaces, by id. Reading tracks, on any thread.
pub fn workspaces() -> Vec<Workspace> {
    tracker().workspaces.get()
}

/// The windows. Reading tracks, on any thread.
pub fn clients() -> Vec<Client> {
    tracker().clients.get()
}

/// The window with the keyboard. Reading tracks, on any thread.
pub fn active_window() -> Option<Client> {
    tracker().active.get()
}

/// Calls `f` on this thread with every event, until the owner this was called under goes.
pub fn on_event(f: impl Fn(&Event) + 'static) {
    tracker();
    LISTENERS.listen(f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_read_with_their_fields() {
        assert_eq!(
            parse_event("workspacev2>>3,code"),
            Some(Event::Workspace {
                id: 3,
                name: "code".into()
            })
        );
        assert_eq!(
            parse_event("openwindow>>5a1b,2,kitty,a, title, with commas"),
            Some(Event::OpenWindow {
                address: "5a1b".into(),
                workspace: "2".into(),
                class: "kitty".into(),
                title: "a, title, with commas".into()
            })
        );
        assert_eq!(parse_event("fullscreen>>1"), Some(Event::Fullscreen(true)));
        assert_eq!(
            parse_event("workspace>>3"),
            Some(Event::Other {
                name: "workspace".into(),
                data: "3".into()
            })
        );
        assert_eq!(parse_event("garbage"), None);
    }

    #[test]
    fn replies_of_any_version_are_read() {
        let v: Value = serde_json::from_str(
            r#"{"address":"0x1","class":"kitty","title":"t","workspace":{"id":2,"name":"2"},
                "fullscreen":2,"floating":false,"at":[10,20],"size":[300,200],"pid":7}"#,
        )
        .unwrap();
        let c = Client::of(&v);
        assert!(c.fullscreen);
        assert_eq!((c.at, c.size, c.workspace.id), ((10, 20), (300, 200), 2));
        let m = Monitor::of(
            &serde_json::from_str(
                r#"{"id":0,"name":"DP-1","scale":1.5,"focused":true,
                    "activeWorkspace":{"id":1,"name":"1"},"specialWorkspace":{"id":0,"name":""}}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            (m.name.as_str(), m.scale, m.active_workspace.id),
            ("DP-1", 1.5, 1)
        );
    }

    #[test]
    fn events_say_what_to_ask_again() {
        let s = staled_by("workspacev2");
        assert!(s.monitors && s.workspaces && !s.clients);
        let s = staled_by("openwindow");
        assert!(!s.monitors && s.workspaces && s.clients);
        let s = staled_by("submap");
        assert!(!s.monitors && !s.workspaces && !s.clients);
    }
}
