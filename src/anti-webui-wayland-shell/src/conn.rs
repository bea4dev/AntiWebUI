//! The shell's own Wayland connection, for what the compositor tells of the desktop, read on a
//! thread nobody sees (see "Wayland の機能と接続" in `docs/design/37-wayland-shell.md`). It is
//! another client than the UI's, so what it hears never waits for a frame.

use crate::output::{Output, OutputTransform};
use anti_webui_core::geometry::{PointI, SizeI};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_callback, wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

/// What the connection tells, for any thread to read.
pub(crate) struct Shared {
    pub outputs: SharedSignal<Vec<Output>>,
    /// How the services reach the compositor; `None` without one.
    pub link: Option<Link>,
}

/// The connection, for asking the compositor things from any thread. What comes back goes to the
/// connection's thread, through the user data of what was asked.
pub(crate) struct Link {
    pub conn: Connection,
    pub qh: QueueHandle<State>,
    registry: wl_registry::WlRegistry,
    /// The globals there were at the start: name, interface and version.
    globals: Vec<(u32, String, u32)>,
    pub seat: Option<wl_seat::WlSeat>,
}

impl Link {
    /// Binds a global of the compositor's, if it has one.
    pub fn bind<I, U>(&self, max: u32, data: U) -> Option<I>
    where
        I: Proxy + 'static,
        U: Send + Sync + 'static,
        State: Dispatch<I, U>,
    {
        let (name, _, version) = self
            .globals
            .iter()
            .find(|(_, interface, _)| interface == I::interface().name)?;
        Some(
            self.registry
                .bind(*name, (*version).min(max), &self.qh, data),
        )
    }

    pub fn flush(&self) {
        let _ = self.conn.flush();
    }

    /// Waits until the compositor has answered everything asked so far, a short while at most:
    /// what a global tells when bound is there to read after this.
    pub fn sync(&self) {
        let done: Synced = Arc::default();
        self.conn.display().sync(&self.qh, done.clone());
        self.flush();
        let (lock, cvar) = &*done;
        let guard = lock.lock().expect("no panics in here");
        let _ = cvar.wait_timeout_while(guard, Duration::from_millis(500), |done| !*done);
    }
}

type Synced = Arc<(Mutex<bool>, Condvar)>;

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The `wl_output`s of this connection by connector name, for asking about one from any thread.
static OUTPUT_OBJECTS: Mutex<Vec<(String, wl_output::WlOutput)>> = Mutex::new(Vec::new());

/// This connection's `wl_output` for an output.
pub(crate) fn output_object(name: &str) -> Option<wl_output::WlOutput> {
    let list = OUTPUT_OBJECTS.lock().ok()?;
    list.iter().find(|(n, _)| n == name).map(|(_, o)| o.clone())
}

/// Connects the first time; with nothing to tell if there is no compositor.
pub(crate) fn shared() -> &'static Shared {
    SHARED.get_or_init(|| {
        let outputs = shared_signal(Vec::new());
        let link = match start(outputs.clone()) {
            Ok(link) => Some(link),
            Err(e) => {
                eprintln!("[anti-webui] the shell can't reach the compositor: {e}");
                None
            }
        };
        Shared { outputs, link }
    })
}

/// The link, if there is a compositor.
pub(crate) fn link() -> Option<&'static Link> {
    shared().link.as_ref()
}

struct Entry {
    global: u32,
    object: wl_output::WlOutput,
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
    info: Output,
    /// `wl_output.scale`, for when there is no `xdg_output` to work out a fractional one.
    int_scale: i32,
    done: bool,
}

pub(crate) struct State {
    xdg_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: Vec<Entry>,
    published: SharedSignal<Vec<Output>>,
    pub(crate) workspaces: crate::workspaces::ExtState,
    pub(crate) toplevels: crate::toplevels::State,
    pub(crate) clipboard: crate::clipboard::State,
}

// What capture objects tell goes to their jobs, through their user data.
delegate_noop!(State: ignore wayland_client::protocol::wl_shm::WlShm);
delegate_noop!(State: wayland_client::protocol::wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wayland_client::protocol::wl_buffer::WlBuffer);

impl State {
    /// The connector name of an output of this connection.
    pub(crate) fn output_name(&self, output: &wl_output::WlOutput) -> Option<String> {
        let e = self.outputs.iter().find(|e| e.object == *output)?;
        Some(if e.info.name.is_empty() {
            format!("output-{}", e.global)
        } else {
            e.info.name.clone()
        })
    }

    fn add_output(
        &mut self,
        registry: &wl_registry::WlRegistry,
        qh: &QueueHandle<Self>,
        global: u32,
        version: u32,
    ) {
        let object: wl_output::WlOutput = registry.bind(global, version.min(4), qh, global);
        let xdg = self
            .xdg_manager
            .as_ref()
            .map(|m| m.get_xdg_output(&object, qh, global));
        self.outputs.push(Entry {
            global,
            object,
            xdg,
            info: Output::default(),
            int_scale: 1,
            done: false,
        });
    }

    fn entry(&mut self, global: u32) -> Option<&mut Entry> {
        self.outputs.iter_mut().find(|e| e.global == global)
    }

    fn publish(&self) {
        let list: Vec<Output> = self
            .outputs
            .iter()
            .filter(|e| e.done)
            .map(|e| {
                let mut o = e.info.clone();
                if o.name.is_empty() {
                    o.name = format!("output-{}", e.global);
                }
                o.scale = scale_of(&o, e.int_scale);
                if o.logical_size == SizeI::default() {
                    let s = e.int_scale.max(1);
                    o.logical_size = SizeI::new(o.mode.width / s, o.mode.height / s);
                }
                o
            })
            .collect();
        if let Ok(mut objects) = OUTPUT_OBJECTS.lock() {
            *objects = self
                .outputs
                .iter()
                .filter(|e| e.done)
                .zip(&list)
                .map(|(e, o)| (o.name.clone(), e.object.clone()))
                .collect();
        }
        if self.published.with_untracked(|p| *p != list) {
            self.published.set(list);
        }
    }
}

/// How many pixels of the mode there are to a logical pixel.
fn scale_of(o: &Output, int_scale: i32) -> f32 {
    let mode = if o.transform.is_sideways() {
        o.mode.height
    } else {
        o.mode.width
    };
    if o.logical_size.width > 0 && mode > 0 {
        mode as f32 / o.logical_size.width as f32
    } else {
        int_scale.max(1) as f32
    }
}

fn start(published: SharedSignal<Vec<Output>>) -> Result<Link, Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let mut state = State {
        xdg_manager: globals.bind(&qh, 1..=3, ()).ok(),
        outputs: Vec::new(),
        published,
        workspaces: Default::default(),
        toplevels: Default::default(),
        clipboard: Default::default(),
    };
    let seat = globals.bind(&qh, 1..=7, ()).ok();
    let found: Vec<(u32, u32)> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == wl_output::WlOutput::interface().name)
            .map(|g| (g.name, g.version))
            .collect()
    });
    for (global, version) in found {
        state.add_output(globals.registry(), &qh, global, version);
    }
    // What the outputs are, before anyone reads them.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    state.publish();
    let link = Link {
        conn: conn.clone(),
        qh: qh.clone(),
        registry: globals.registry().clone(),
        globals: globals.contents().with_list(|list| {
            list.iter()
                .map(|g| (g.name, g.interface.clone(), g.version))
                .collect()
        }),
        seat,
    };
    std::thread::Builder::new()
        .name("anti-webui shell".into())
        .spawn(move || while queue.blocking_dispatch(&mut state).is_ok() {})?;
    Ok(link)
}

impl Dispatch<wl_callback::WlCallback, Synced> for State {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        done: &Synced,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            let (lock, cvar) = &**done;
            *lock.lock().expect("no panics in here") = true;
            cvar.notify_all();
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == wl_output::WlOutput::interface().name => {
                state.add_output(registry, qh, name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(at) = state.outputs.iter().position(|e| e.global == name) {
                    let e = state.outputs.remove(at);
                    if let Some(x) = e.xdg {
                        x.destroy();
                    }
                    if e.object.version() >= 3 {
                        e.object.release();
                    }
                    state.publish();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(e) = state.entry(*global) else {
            return;
        };
        match event {
            wl_output::Event::Geometry {
                make,
                model,
                transform,
                ..
            } => {
                e.info.make = make;
                e.info.model = model;
                if let WEnum::Value(t) = transform {
                    e.info.transform = OutputTransform::from_wayland(t);
                }
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                refresh,
            } => {
                let current = match flags {
                    WEnum::Value(f) => f.contains(wl_output::Mode::Current),
                    WEnum::Unknown(bits) => bits & 1 != 0,
                };
                if current {
                    e.info.mode = SizeI::new(width, height);
                    e.info.refresh_mhz = refresh;
                }
            }
            wl_output::Event::Scale { factor } => e.int_scale = factor,
            wl_output::Event::Name { name } => e.info.name = name,
            wl_output::Event::Description { description } => e.info.description = description,
            wl_output::Event::Done => {
                e.done = true;
                state.publish();
            }
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        xdg: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(e) = state.entry(*global) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => e.info.position = PointI::new(x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                e.info.logical_size = SizeI::new(width, height)
            }
            zxdg_output_v1::Event::Name { name } if e.info.name.is_empty() => e.info.name = name,
            zxdg_output_v1::Event::Description { description } if e.info.description.is_empty() => {
                e.info.description = description
            }
            // From version 3 on, `wl_output.done` says it instead.
            zxdg_output_v1::Event::Done if xdg.version() < 3 => {
                e.done = true;
                state.publish();
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);
