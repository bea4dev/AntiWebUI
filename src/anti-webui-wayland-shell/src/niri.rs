//! niri's IPC: a line of JSON for each request and reply on `$NIRI_SOCKET`, and a stream of events
//! after `"EventStream"`.

use crate::ServiceError;
use crate::ipc::{Listeners, failed, flag, int, text};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::{Task, task};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::OnceLock;

fn socket() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("NIRI_SOCKET")?);
    path.exists().then_some(path)
}

/// Whether niri is running here.
pub fn available() -> bool {
    socket().is_some()
}

fn connect_and_ask(request: &Value) -> Result<(BufReader<UnixStream>, Value), ServiceError> {
    let path = socket().ok_or(ServiceError::Unavailable)?;
    let mut stream = UnixStream::connect(path).map_err(failed)?;
    let mut line = serde_json::to_string(request).map_err(failed)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(failed)?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).map_err(failed)?;
    let reply: Value = serde_json::from_str(&reply).map_err(failed)?;
    match (reply.get("Ok"), reply.get("Err")) {
        (Some(ok), _) => Ok((reader, ok.clone())),
        (_, Some(e)) => Err(ServiceError::Failed(
            e.as_str().map_or_else(|| e.to_string(), str::to_owned),
        )),
        _ => Err(ServiceError::Failed(reply.to_string())),
    }
}

/// Sends a request, like `json!("Outputs")`, and gives what is in the `Ok` of the reply.
pub fn request(request: Value) -> Task<Result<Value, ServiceError>> {
    task(move || connect_and_ask(&request).map(|(_, ok)| ok))
}

/// Does an action, like `json!({"FocusWorkspace": {"reference": {"Index": 2}}})`.
pub fn action(action: Value) -> Task<Result<(), ServiceError>> {
    task(move || connect_and_ask(&json!({ "Action": action })).map(|_| ()))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    pub id: u64,
    /// Its place on its output, from 1.
    pub idx: u32,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_active: bool,
    pub is_focused: bool,
    pub is_urgent: bool,
    pub active_window_id: Option<u64>,
}

impl Workspace {
    fn of(v: &Value) -> Self {
        Self {
            id: int(v, "id") as u64,
            idx: int(v, "idx") as u32,
            name: v.get("name").and_then(Value::as_str).map(str::to_owned),
            output: v.get("output").and_then(Value::as_str).map(str::to_owned),
            is_active: flag(v, "is_active"),
            is_focused: flag(v, "is_focused"),
            is_urgent: flag(v, "is_urgent"),
            active_window_id: v.get("active_window_id").and_then(Value::as_u64),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub pid: Option<i64>,
    pub workspace_id: Option<u64>,
    pub is_focused: bool,
    pub is_floating: bool,
    pub is_urgent: bool,
}

impl Window {
    fn of(v: &Value) -> Self {
        Self {
            id: int(v, "id") as u64,
            title: text(v, "title"),
            app_id: text(v, "app_id"),
            pid: v.get("pid").and_then(Value::as_i64),
            workspace_id: v.get("workspace_id").and_then(Value::as_u64),
            is_focused: flag(v, "is_focused"),
            is_floating: flag(v, "is_floating"),
            is_urgent: flag(v, "is_urgent"),
        }
    }
}

/// What the stream of events has said so far.
#[derive(Default, Clone, PartialEq, Debug)]
pub(crate) struct Model {
    pub workspaces: Vec<Workspace>,
    pub windows: Vec<Window>,
}

impl Model {
    /// Takes in one event.
    pub fn apply(&mut self, event: &Value) {
        let Some((name, e)) = event.as_object().and_then(|o| o.iter().next()) else {
            return;
        };
        let id = |key: &str| e.get(key).and_then(Value::as_u64);
        match name.as_str() {
            "WorkspacesChanged" => {
                self.workspaces = e
                    .get("workspaces")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().map(Workspace::of).collect())
                    .unwrap_or_default();
            }
            "WorkspaceActivated" => {
                let (Some(id), focused) = (id("id"), flag(e, "focused")) else {
                    return;
                };
                let output = self
                    .workspaces
                    .iter()
                    .find(|w| w.id == id)
                    .and_then(|w| w.output.clone());
                for w in &mut self.workspaces {
                    if w.output == output {
                        w.is_active = w.id == id;
                    }
                    if focused {
                        w.is_focused = w.id == id;
                    }
                }
            }
            "WorkspaceUrgencyChanged" => {
                if let Some(w) = self.workspaces.iter_mut().find(|w| Some(w.id) == id("id")) {
                    w.is_urgent = flag(e, "urgent");
                }
            }
            "WorkspaceActiveWindowChanged" => {
                if let Some(w) = self
                    .workspaces
                    .iter_mut()
                    .find(|w| Some(w.id) == id("workspace_id"))
                {
                    w.active_window_id = id("active_window_id");
                }
            }
            "WindowsChanged" => {
                self.windows = e
                    .get("windows")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().map(Window::of).collect())
                    .unwrap_or_default();
            }
            "WindowOpenedOrChanged" => {
                let Some(window) = e.get("window").map(Window::of) else {
                    return;
                };
                if window.is_focused {
                    for w in &mut self.windows {
                        w.is_focused = false;
                    }
                }
                match self.windows.iter_mut().find(|w| w.id == window.id) {
                    Some(w) => *w = window,
                    None => self.windows.push(window),
                }
            }
            "WindowClosed" => self.windows.retain(|w| Some(w.id) != id("id")),
            "WindowFocusChanged" => {
                let focused = id("id");
                for w in &mut self.windows {
                    w.is_focused = Some(w.id) == focused;
                }
            }
            "WindowUrgencyChanged" => {
                if let Some(w) = self.windows.iter_mut().find(|w| Some(w.id) == id("id")) {
                    w.is_urgent = flag(e, "urgent");
                }
            }
            _ => {}
        }
    }
}

struct Tracker {
    workspaces: SharedSignal<Vec<Workspace>>,
    windows: SharedSignal<Vec<Window>>,
}

static LISTENERS: Listeners<Value> = Listeners::new();
static TRACKER: OnceLock<Tracker> = OnceLock::new();

fn tracker() -> &'static Tracker {
    TRACKER.get_or_init(|| {
        let t = Tracker {
            workspaces: shared_signal(Vec::new()),
            windows: shared_signal(Vec::new()),
        };
        if let Ok((reader, _)) = connect_and_ask(&json!("EventStream")) {
            let (workspaces, windows) = (t.workspaces.clone(), t.windows.clone());
            // The stream starts with all there is; it is taken in before anyone reads.
            let (ready_tx, ready) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new()
                .name("anti-webui niri".into())
                .spawn(move || follow(reader, workspaces, windows, ready_tx));
            let _ = ready.recv_timeout(std::time::Duration::from_millis(500));
        }
        t
    })
}

fn follow(
    mut reader: BufReader<UnixStream>,
    workspaces: SharedSignal<Vec<Workspace>>,
    windows: SharedSignal<Vec<Window>>,
    ready: std::sync::mpsc::Sender<()>,
) {
    let mut model = Model::default();
    let mut seen = (false, false);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        model.apply(&event);
        seen.0 |= event.get("WorkspacesChanged").is_some();
        seen.1 |= event.get("WindowsChanged").is_some();
        if workspaces.with_untracked(|w| *w != model.workspaces) {
            workspaces.set(model.workspaces.clone());
        }
        if windows.with_untracked(|w| *w != model.windows) {
            windows.set(model.windows.clone());
        }
        if seen == (true, true) {
            let _ = ready.send(());
        }
        LISTENERS.send(&event);
    }
}

/// The workspaces. Reading tracks, on any thread.
pub fn workspaces() -> Vec<Workspace> {
    tracker().workspaces.get()
}

/// The windows. Reading tracks, on any thread.
pub fn windows() -> Vec<Window> {
    tracker().windows.get()
}

/// The window with the keyboard. Reading tracks, on any thread.
pub fn focused_window() -> Option<Window> {
    windows().into_iter().find(|w| w.is_focused)
}

/// Calls `f` on this thread with every event, as niri sends it, until the owner this was called
/// under goes.
pub fn on_event(f: impl Fn(&Value) + 'static) {
    tracker();
    LISTENERS.listen(f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_of_events_keeps_the_model() {
        let mut m = Model::default();
        m.apply(&json!({"WorkspacesChanged": {"workspaces": [
            {"id": 1, "idx": 1, "name": null, "output": "DP-1", "is_active": true, "is_focused": true},
            {"id": 2, "idx": 2, "name": "web", "output": "DP-1", "is_active": false},
            {"id": 3, "idx": 1, "name": null, "output": "DP-2", "is_active": true}
        ]}}));
        m.apply(&json!({"WorkspaceActivated": {"id": 2, "focused": true}}));
        let active: Vec<_> = m
            .workspaces
            .iter()
            .map(|w| (w.id, w.is_active, w.is_focused))
            .collect();
        assert_eq!(
            active,
            [(1, false, false), (2, true, true), (3, true, false)]
        );

        m.apply(&json!({"WindowOpenedOrChanged": {"window": {"id": 9, "title": "a", "is_focused": true}}}));
        m.apply(&json!({"WindowOpenedOrChanged": {"window": {"id": 10, "title": "b", "is_focused": true}}}));
        assert_eq!(
            m.windows
                .iter()
                .map(|w| (w.id, w.is_focused))
                .collect::<Vec<_>>(),
            [(9, false), (10, true)]
        );
        m.apply(&json!({"WindowClosed": {"id": 10}}));
        m.apply(&json!({"WindowFocusChanged": {"id": 9}}));
        assert_eq!(m.windows.len(), 1);
        assert!(m.windows[0].is_focused);
    }
}
