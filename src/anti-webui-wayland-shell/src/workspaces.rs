//! Workspaces, whichever compositor tells of them: its own IPC where there is one, which tells more,
//! and `ext_workspace_v1` otherwise (see "汎用ワークスペース" in
//! `docs/design/37-wayland-shell.md`).

use crate::ServiceError;
use crate::conn::{State, link};
use crate::output::Output;
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use std::sync::{Arc, Mutex, OnceLock};
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, event_created_child};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1 as group_v1, ext_workspace_handle_v1 as handle_v1,
    ext_workspace_manager_v1 as manager_v1,
};

/// Where workspaces are read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceSource {
    Hyprland,
    Niri,
    Sway,
    /// `ext_workspace_v1`, which any compositor may have.
    Ext,
}

/// What can be done to a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceAction {
    Activate,
    Remove,
    /// Moving it to another output.
    Assign,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Ext(handle_v1::ExtWorkspaceHandleV1),
    #[cfg(feature = "hyprland")]
    Hyprland {
        id: i64,
        name: String,
    },
    #[cfg(feature = "niri")]
    Niri(u64),
    #[cfg(feature = "sway")]
    Sway(String),
}

/// A workspace, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    id: String,
    name: String,
    output: Option<String>,
    active: bool,
    urgent: bool,
    hidden: bool,
    coordinates: Vec<u32>,
    can: [bool; 3],
    target: Target,
}

impl Workspace {
    /// What the source calls it by, which stays the same while it is there.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The connector name of the output it is on, like [`Output::name`].
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Shown on its output.
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_urgent(&self) -> bool {
        self.urgent
    }

    /// Not meant to be shown, like Hyprland's special workspaces.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Where it is in the grid of workspaces, if the source says.
    pub fn coordinates(&self) -> &[u32] {
        &self.coordinates
    }

    pub fn can(&self, action: WorkspaceAction) -> bool {
        self.can[action as usize]
    }

    /// Shows it on its output.
    pub fn activate(&self) -> Task<Result<(), ServiceError>> {
        if !self.can(WorkspaceAction::Activate) {
            return Task::ready(Err(ServiceError::Unsupported));
        }
        match &self.target {
            Target::Ext(h) => ext_commit(|| h.activate()),
            #[cfg(feature = "hyprland")]
            Target::Hyprland { id, name } => match name.strip_prefix("special:") {
                Some(special) => {
                    crate::hyprland::dispatch(&format!("togglespecialworkspace {special}"))
                }
                None => crate::hyprland::dispatch(&format!("workspace {id}")),
            },
            #[cfg(feature = "niri")]
            Target::Niri(id) => crate::niri::action(serde_json::json!({
                "FocusWorkspace": { "reference": { "Id": id } }
            })),
            #[cfg(feature = "sway")]
            Target::Sway(name) => {
                crate::sway::command(&format!("workspace {}", crate::sway::quote(name)))
            }
        }
    }

    pub fn remove(&self) -> Task<Result<(), ServiceError>> {
        if !self.can(WorkspaceAction::Remove) {
            return Task::ready(Err(ServiceError::Unsupported));
        }
        match &self.target {
            Target::Ext(h) => ext_commit(|| h.remove()),
            #[allow(unreachable_patterns)]
            _ => Task::ready(Err(ServiceError::Unsupported)),
        }
    }

    /// Moves it to another output.
    pub fn assign(&self, output: &Output) -> Task<Result<(), ServiceError>> {
        if !self.can(WorkspaceAction::Assign) {
            return Task::ready(Err(ServiceError::Unsupported));
        }
        match &self.target {
            Target::Ext(h) => {
                let Some(group) = ext().and_then(|e| e.group_of(&output.name)) else {
                    return Task::ready(Err(ServiceError::Unsupported));
                };
                ext_commit(|| h.assign(&group.0))
            }
            #[cfg(feature = "hyprland")]
            Target::Hyprland { id, .. } => {
                crate::hyprland::dispatch(&format!("moveworkspacetomonitor {id} {}", output.name))
            }
            #[cfg(feature = "niri")]
            Target::Niri(id) => crate::niri::action(serde_json::json!({
                "MoveWorkspaceToMonitor": { "output": output.name, "reference": { "Id": id } }
            })),
            #[cfg(feature = "sway")]
            Target::Sway(name) => crate::sway::command(&format!(
                "workspace {}; move workspace to output {}",
                crate::sway::quote(name),
                crate::sway::quote(&output.name)
            )),
        }
    }
}

/// The workspaces of the desktop. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workspaces {
    source: Option<WorkspaceSource>,
}

static PREFER: Mutex<Option<WorkspaceSource>> = Mutex::new(None);
static CHOSEN: OnceLock<Option<WorkspaceSource>> = OnceLock::new();

/// The workspaces of the desktop, from the compositor's own IPC if it has one and it is built in,
/// otherwise from `ext_workspace_v1`.
pub fn workspaces() -> Workspaces {
    Workspaces {
        source: *CHOSEN.get_or_init(choose),
    }
}

fn choose() -> Option<WorkspaceSource> {
    let preferred = PREFER.lock().ok().and_then(|p| *p);
    let mut order = vec![
        WorkspaceSource::Hyprland,
        WorkspaceSource::Niri,
        WorkspaceSource::Sway,
        WorkspaceSource::Ext,
    ];
    if let Some(p) = preferred {
        order.retain(|s| *s != p);
        order.insert(0, p);
    }
    order.into_iter().find(|s| source_available(*s))
}

fn source_available(source: WorkspaceSource) -> bool {
    match source {
        #[cfg(feature = "hyprland")]
        WorkspaceSource::Hyprland => crate::hyprland::available(),
        #[cfg(feature = "niri")]
        WorkspaceSource::Niri => crate::niri::available(),
        #[cfg(feature = "sway")]
        WorkspaceSource::Sway => crate::sway::available(),
        WorkspaceSource::Ext => ext().is_some(),
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

impl Workspaces {
    /// Reads from `source` first where it is there. Call before anything reads workspaces.
    pub fn prefer(source: WorkspaceSource) {
        if let Ok(mut p) = PREFER.lock() {
            *p = Some(source);
        }
    }

    pub fn source(&self) -> Option<WorkspaceSource> {
        self.source
    }

    pub fn available(&self) -> bool {
        self.source.is_some()
    }

    /// All of them, in the order the source gives, output by output.
    pub fn all(&self) -> Vec<Workspace> {
        match self.source {
            Some(WorkspaceSource::Ext) => ext().map(|e| e.list.get()).unwrap_or_default(),
            #[cfg(feature = "hyprland")]
            Some(WorkspaceSource::Hyprland) => from_hyprland(),
            #[cfg(feature = "niri")]
            Some(WorkspaceSource::Niri) => from_niri(),
            #[cfg(feature = "sway")]
            Some(WorkspaceSource::Sway) => from_sway(),
            _ => Vec::new(),
        }
    }

    /// Those on an output.
    pub fn on(&self, output: &Output) -> Vec<Workspace> {
        let mut all = self.all();
        all.retain(|w| w.output() == Some(output.name.as_str()));
        all
    }

    /// The one shown on an output.
    pub fn active(&self, output: &Output) -> Option<Workspace> {
        self.on(output).into_iter().find(|w| w.active && !w.hidden)
    }

    /// Whether [`Workspaces::create`] can make one on the output.
    pub fn can_create(&self, output: &Output) -> bool {
        match self.source {
            Some(WorkspaceSource::Ext) => ext()
                .and_then(|e| e.group_of(&output.name))
                .is_some_and(|g| g.1),
            #[cfg(feature = "sway")]
            Some(WorkspaceSource::Sway) => true,
            _ => false,
        }
    }

    /// Makes a workspace on an output. Sway also goes there.
    pub fn create(&self, name: &str, output: &Output) -> Task<Result<(), ServiceError>> {
        if !self.can_create(output) {
            return Task::ready(Err(ServiceError::Unsupported));
        }
        match self.source {
            Some(WorkspaceSource::Ext) => {
                let group = ext()
                    .and_then(|e| e.group_of(&output.name))
                    .expect("checked");
                let name = name.to_owned();
                ext_commit(move || group.0.create_workspace(name))
            }
            #[cfg(feature = "sway")]
            Some(WorkspaceSource::Sway) => crate::sway::command(&format!(
                "focus output {}; workspace {}",
                crate::sway::quote(&output.name),
                crate::sway::quote(name)
            )),
            _ => Task::ready(Err(ServiceError::Unsupported)),
        }
    }
}

// --- ext_workspace_v1 -------------------------------------------------------------------------

/// What other threads read of `ext_workspace_v1`; the user data of the manager.
pub(crate) struct ExtShared {
    manager: OnceLock<manager_v1::ExtWorkspaceManagerV1>,
    list: SharedSignal<Vec<Workspace>>,
    /// Each group: its handle, whether it can make workspaces, and its outputs.
    groups: Mutex<Vec<(group_v1::ExtWorkspaceGroupHandleV1, bool, Vec<String>)>>,
}

impl ExtShared {
    fn group_of(&self, output: &str) -> Option<(group_v1::ExtWorkspaceGroupHandleV1, bool)> {
        let groups = self.groups.lock().ok()?;
        groups
            .iter()
            .find(|(_, _, outputs)| outputs.iter().any(|o| o == output))
            .map(|(g, create, _)| (g.clone(), *create))
    }
}

static EXT: OnceLock<Option<Arc<ExtShared>>> = OnceLock::new();

/// Binds `ext_workspace_v1` the first time, and waits for what it tells.
fn ext() -> Option<&'static ExtShared> {
    EXT.get_or_init(|| {
        let link = link()?;
        let shared = Arc::new(ExtShared {
            manager: OnceLock::new(),
            list: shared_signal(Vec::new()),
            groups: Mutex::new(Vec::new()),
        });
        let manager: manager_v1::ExtWorkspaceManagerV1 = link.bind(1, shared.clone())?;
        let _ = shared.manager.set(manager);
        link.sync();
        Some(shared)
    })
    .as_deref()
}

/// Asks for something and commits it.
fn ext_commit(ask: impl FnOnce()) -> Task<Result<(), ServiceError>> {
    let (Some(e), Some(link)) = (ext(), link()) else {
        return Task::ready(Err(ServiceError::Unavailable));
    };
    ask();
    if let Some(m) = e.manager.get() {
        m.commit();
    }
    link.flush();
    Task::ready(Ok(()))
}

#[derive(Default)]
pub(crate) struct ExtState {
    groups: Vec<Group>,
    workspaces: Vec<Entry>,
}

struct Group {
    handle: group_v1::ExtWorkspaceGroupHandleV1,
    caps: u32,
    outputs: Vec<wl_output::WlOutput>,
    workspaces: Vec<handle_v1::ExtWorkspaceHandleV1>,
}

struct Entry {
    handle: handle_v1::ExtWorkspaceHandleV1,
    id: String,
    name: String,
    coordinates: Vec<u32>,
    state: u32,
    caps: u32,
}

impl State {
    fn publish_workspaces(&self, shared: &ExtShared) {
        let ws = &self.workspaces;
        let output_of = |h: &handle_v1::ExtWorkspaceHandleV1| {
            let group = ws.groups.iter().find(|g| g.workspaces.contains(h))?;
            group.outputs.iter().find_map(|o| self.output_name(o))
        };
        let mut list: Vec<Workspace> = ws
            .workspaces
            .iter()
            .map(|e| Workspace {
                id: if e.id.is_empty() {
                    format!("{}", e.handle.id().protocol_id())
                } else {
                    e.id.clone()
                },
                name: e.name.clone(),
                output: output_of(&e.handle),
                active: e.state & 1 != 0,
                urgent: e.state & 2 != 0,
                hidden: e.state & 4 != 0,
                coordinates: e.coordinates.clone(),
                can: [e.caps & 1 != 0, e.caps & 4 != 0, e.caps & 8 != 0],
                target: Target::Ext(e.handle.clone()),
            })
            .collect();
        if list.iter().all(|w| !w.coordinates.is_empty()) {
            list.sort_by(|a, b| (&a.output, &a.coordinates).cmp(&(&b.output, &b.coordinates)));
        }
        if shared.list.with_untracked(|l| *l != list) {
            shared.list.set(list);
        }
        let groups = ws
            .groups
            .iter()
            .map(|g| {
                let outputs = g
                    .outputs
                    .iter()
                    .filter_map(|o| self.output_name(o))
                    .collect();
                (g.handle.clone(), g.caps & 1 != 0, outputs)
            })
            .collect();
        if let Ok(mut g) = shared.groups.lock() {
            *g = groups;
        }
    }
}

use wayland_client::Proxy;

fn bits<T>(w: WEnum<T>, value: impl Fn(T) -> u32) -> u32 {
    match w {
        WEnum::Value(v) => value(v),
        WEnum::Unknown(u) => u,
    }
}

impl Dispatch<manager_v1::ExtWorkspaceManagerV1, Arc<ExtShared>> for State {
    fn event(
        state: &mut Self,
        _: &manager_v1::ExtWorkspaceManagerV1,
        event: manager_v1::Event,
        shared: &Arc<ExtShared>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            manager_v1::Event::WorkspaceGroup { workspace_group } => {
                state.workspaces.groups.push(Group {
                    handle: workspace_group,
                    caps: 0,
                    outputs: Vec::new(),
                    workspaces: Vec::new(),
                })
            }
            manager_v1::Event::Workspace { workspace } => state.workspaces.workspaces.push(Entry {
                handle: workspace,
                id: String::new(),
                name: String::new(),
                coordinates: Vec::new(),
                state: 0,
                caps: 0,
            }),
            manager_v1::Event::Done => state.publish_workspaces(shared),
            _ => {}
        }
    }

    event_created_child!(State, manager_v1::ExtWorkspaceManagerV1, [
        manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (group_v1::ExtWorkspaceGroupHandleV1, ()),
        manager_v1::EVT_WORKSPACE_OPCODE => (handle_v1::ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<group_v1::ExtWorkspaceGroupHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &group_v1::ExtWorkspaceGroupHandleV1,
        event: group_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let groups = &mut state.workspaces.groups;
        let Some(at) = groups.iter().position(|g| g.handle == *handle) else {
            return;
        };
        let g = &mut groups[at];
        match event {
            group_v1::Event::Capabilities { capabilities } => {
                g.caps = bits(capabilities, |c| c.bits());
            }
            group_v1::Event::OutputEnter { output } => g.outputs.push(output),
            group_v1::Event::OutputLeave { output } => g.outputs.retain(|o| *o != output),
            group_v1::Event::WorkspaceEnter { workspace } => g.workspaces.push(workspace),
            group_v1::Event::WorkspaceLeave { workspace } => {
                g.workspaces.retain(|w| *w != workspace)
            }
            group_v1::Event::Removed => {
                groups.remove(at).handle.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<handle_v1::ExtWorkspaceHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &handle_v1::ExtWorkspaceHandleV1,
        event: handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let list = &mut state.workspaces.workspaces;
        let Some(at) = list.iter().position(|e| e.handle == *handle) else {
            return;
        };
        let e = &mut list[at];
        match event {
            handle_v1::Event::Id { id } => e.id = id,
            handle_v1::Event::Name { name } => e.name = name,
            handle_v1::Event::Coordinates { coordinates } => {
                e.coordinates = coordinates
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes(c.try_into().expect("four bytes")))
                    .collect();
            }
            handle_v1::Event::State { state } => e.state = bits(state, |s| s.bits()),
            handle_v1::Event::Capabilities { capabilities } => {
                e.caps = bits(capabilities, |c| c.bits())
            }
            handle_v1::Event::Removed => {
                let gone = list.remove(at).handle;
                for g in &mut state.workspaces.groups {
                    g.workspaces.retain(|w| *w != gone);
                }
                gone.destroy();
            }
            _ => {}
        }
    }
}

// --- From the compositors' own IPC ------------------------------------------------------------

#[cfg(feature = "hyprland")]
fn from_hyprland() -> Vec<Workspace> {
    let monitors = crate::hyprland::monitors();
    let mut list: Vec<Workspace> = crate::hyprland::workspaces()
        .into_iter()
        .map(|w| {
            let special = w.id < 0;
            Workspace {
                id: w.id.to_string(),
                active: monitors
                    .iter()
                    .any(|m| m.active_workspace.id == w.id || m.special_workspace.id == w.id),
                urgent: false,
                hidden: special,
                coordinates: if special {
                    Vec::new()
                } else {
                    vec![w.id as u32]
                },
                can: [true, false, true],
                target: Target::Hyprland {
                    id: w.id,
                    name: w.name.clone(),
                },
                output: Some(w.monitor),
                name: w.name,
            }
        })
        .collect();
    list.sort_by_key(|w| (w.hidden, w.coordinates.clone()));
    list
}

#[cfg(feature = "niri")]
fn from_niri() -> Vec<Workspace> {
    let mut list: Vec<Workspace> = crate::niri::workspaces()
        .into_iter()
        .map(|w| Workspace {
            id: w.id.to_string(),
            name: w.name.clone().unwrap_or_else(|| w.idx.to_string()),
            output: w.output,
            active: w.is_active,
            urgent: w.is_urgent,
            hidden: false,
            coordinates: vec![w.idx],
            can: [true, false, true],
            target: Target::Niri(w.id),
        })
        .collect();
    list.sort_by(|a, b| (&a.output, &a.coordinates).cmp(&(&b.output, &b.coordinates)));
    list
}

#[cfg(feature = "sway")]
fn from_sway() -> Vec<Workspace> {
    crate::sway::workspaces()
        .into_iter()
        .map(|w| Workspace {
            id: w.id.to_string(),
            output: Some(w.output),
            active: w.visible,
            urgent: w.urgent,
            hidden: false,
            coordinates: if w.num >= 0 {
                vec![w.num as u32]
            } else {
                Vec::new()
            },
            can: [true, false, true],
            target: Target::Sway(w.name.clone()),
            name: w.name,
        })
        .collect()
}
