//! Sway's IPC, which is i3's: `i3-ipc`, a length and a type, then JSON, on `$SWAYSOCK`.

use crate::ServiceError;
use crate::ipc::{Listeners, failed, flag, int, text};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::{Task, task};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::OnceLock;

const MAGIC: &[u8; 6] = b"i3-ipc";
pub const RUN_COMMAND: u32 = 0;
pub const GET_WORKSPACES: u32 = 1;
pub const SUBSCRIBE: u32 = 2;
pub const GET_OUTPUTS: u32 = 3;
pub const GET_TREE: u32 = 4;
/// Events have this bit set in their type.
const EVENT: u32 = 0x8000_0000;

fn socket() -> Option<PathBuf> {
    let path = std::env::var_os("SWAYSOCK").or_else(|| std::env::var_os("I3SOCK"))?;
    let path = PathBuf::from(path);
    path.exists().then_some(path)
}

/// Whether sway (or i3) is running here.
pub fn available() -> bool {
    socket().is_some()
}

pub(crate) fn encode(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(14 + payload.len());
    m.extend_from_slice(MAGIC);
    m.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    m.extend_from_slice(&kind.to_ne_bytes());
    m.extend_from_slice(payload);
    m
}

pub(crate) fn read_message(r: &mut impl Read) -> std::io::Result<(u32, Vec<u8>)> {
    let mut head = [0u8; 14];
    r.read_exact(&mut head)?;
    if &head[..6] != MAGIC {
        return Err(std::io::Error::other("not i3-ipc"));
    }
    let len = u32::from_ne_bytes(head[6..10].try_into().expect("four bytes")) as usize;
    let kind = u32::from_ne_bytes(head[10..14].try_into().expect("four bytes"));
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((kind, payload))
}

fn ask(stream: &mut UnixStream, kind: u32, payload: &str) -> Result<Value, ServiceError> {
    stream
        .write_all(&encode(kind, payload.as_bytes()))
        .map_err(failed)?;
    let (_, reply) = read_message(stream).map_err(failed)?;
    serde_json::from_slice(&reply).map_err(failed)
}

fn request_now(kind: u32, payload: &str) -> Result<Value, ServiceError> {
    let mut stream =
        UnixStream::connect(socket().ok_or(ServiceError::Unavailable)?).map_err(failed)?;
    ask(&mut stream, kind, payload)
}

/// Sends a message of a type, like [`GET_OUTPUTS`], and gives the reply.
pub fn request(kind: u32, payload: &str) -> Task<Result<Value, ServiceError>> {
    let payload = payload.to_owned();
    task(move || request_now(kind, &payload))
}

/// Runs commands, like `workspace 3`.
pub fn command(command: &str) -> Task<Result<(), ServiceError>> {
    let command = command.to_owned();
    task(move || {
        let reply = request_now(RUN_COMMAND, &command)?;
        let failure = reply
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| !flag(r, "success"));
        match failure {
            Some(r) => Err(ServiceError::Failed(text(r, "error"))),
            None => Ok(()),
        }
    })
}

/// The tree of outputs, workspaces and windows.
pub fn tree() -> Task<Result<Value, ServiceError>> {
    request(GET_TREE, "")
}

/// Puts something in quotes for a command.
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    pub id: i64,
    /// The number at the start of its name, or -1.
    pub num: i64,
    pub name: String,
    pub output: String,
    /// Shown on its output.
    pub visible: bool,
    pub focused: bool,
    pub urgent: bool,
}

impl Workspace {
    fn of(v: &Value) -> Self {
        Self {
            id: int(v, "id"),
            num: v.get("num").and_then(Value::as_i64).unwrap_or(-1),
            name: text(v, "name"),
            output: text(v, "output"),
            visible: flag(v, "visible"),
            focused: flag(v, "focused"),
            urgent: flag(v, "urgent"),
        }
    }
}

/// What kind of event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Workspace,
    Output,
    Mode,
    Window,
    BarConfigUpdate,
    Binding,
    Shutdown,
    Tick,
    BarStateUpdate,
    Input,
    Other(u32),
}

impl EventKind {
    fn of(kind: u32) -> Self {
        match kind & !EVENT {
            0 => Self::Workspace,
            1 => Self::Output,
            2 => Self::Mode,
            3 => Self::Window,
            4 => Self::BarConfigUpdate,
            5 => Self::Binding,
            6 => Self::Shutdown,
            7 => Self::Tick,
            0x14 => Self::BarStateUpdate,
            0x15 => Self::Input,
            other => Self::Other(other),
        }
    }
}

/// An event, with what sway says of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: EventKind,
    pub payload: Value,
}

static LISTENERS: Listeners<Event> = Listeners::new();
static WORKSPACES: OnceLock<SharedSignal<Vec<Workspace>>> = OnceLock::new();

fn fetch_workspaces() -> Option<Vec<Workspace>> {
    let reply = request_now(GET_WORKSPACES, "").ok()?;
    Some(reply.as_array()?.iter().map(Workspace::of).collect())
}

fn tracked() -> &'static SharedSignal<Vec<Workspace>> {
    WORKSPACES.get_or_init(|| {
        let list = shared_signal(fetch_workspaces().unwrap_or_default());
        let follow_list = list.clone();
        if let Some(path) = socket()
            && let Ok(mut stream) = UnixStream::connect(path)
            && ask(
                &mut stream,
                SUBSCRIBE,
                r#"["workspace","output","window","mode"]"#,
            )
            .is_ok()
        {
            let _ = std::thread::Builder::new()
                .name("anti-webui sway".into())
                .spawn(move || {
                    while let Ok((kind, payload)) = read_message(&mut stream) {
                        let kind = EventKind::of(kind);
                        if matches!(kind, EventKind::Workspace | EventKind::Output)
                            && let Some(w) = fetch_workspaces()
                            && follow_list.with_untracked(|l| *l != w)
                        {
                            follow_list.set(w);
                        }
                        let payload = serde_json::from_slice(&payload).unwrap_or(Value::Null);
                        LISTENERS.send(&Event { kind, payload });
                    }
                });
        }
        list
    })
}

/// The workspaces. Reading tracks, on any thread.
pub fn workspaces() -> Vec<Workspace> {
    tracked().get()
}

/// Calls `f` on this thread with every workspace, output, window and mode event, until the owner
/// this was called under goes.
pub fn on_event(f: impl Fn(&Event) + 'static) {
    tracked();
    LISTENERS.listen(f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_go_both_ways() {
        let m = encode(RUN_COMMAND, b"workspace 3");
        assert_eq!(&m[..6], MAGIC);
        let (kind, payload) = read_message(&mut m.as_slice()).unwrap();
        assert_eq!(
            (kind, payload.as_slice()),
            (RUN_COMMAND, b"workspace 3".as_slice())
        );
        assert_eq!(EventKind::of(0x8000_0003), EventKind::Window);
        assert_eq!(quote(r#"a "b"\"#), r#""a \"b\"\\""#);
    }
}
