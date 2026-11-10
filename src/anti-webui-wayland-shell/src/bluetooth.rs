//! Bluetooth through BlueZ: the adapters, the devices they know of, and connecting to them.

use crate::ServiceError;
use crate::dbus::{self, Props, get, put, string};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use futures_lite::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use zbus::zvariant::{OwnedObjectPath, Value};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";

/// A Bluetooth adapter, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct BluetoothAdapter {
    /// What it shows itself as to others.
    pub name: String,
    pub address: String,
    pub powered: bool,
    /// Looking for devices.
    pub discovering: bool,
    /// Others can find it.
    pub discoverable: bool,
    path: String,
}

/// A device an adapter knows of, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct BluetoothDevice {
    /// What it was named here, or else what it calls itself.
    pub name: String,
    pub address: String,
    /// The name of an icon in the icon theme for what it is, like `audio-headset`.
    pub icon: String,
    pub paired: bool,
    pub trusted: bool,
    pub connected: bool,
    /// From 0 to 100, for devices that tell.
    pub battery: Option<u8>,
    path: String,
    adapter: String,
}

/// Bluetooth. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Bluetooth(&'static Shared);

#[derive(Debug)]
struct Shared {
    adapters: SharedSignal<Vec<BluetoothAdapter>>,
    devices: SharedSignal<Vec<BluetoothDevice>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// Bluetooth. It starts following it the first time.
pub fn bluetooth() -> Bluetooth {
    Bluetooth(dbus::init_then_follow(&SHARED, follow, || {
        Shared {
            adapters: shared_signal(Vec::new()),
            devices: shared_signal(Vec::new()),
        }
    }))
}

impl Bluetooth {
    /// Whether BlueZ is there with an adapter.
    pub fn available(&self) -> bool {
        self.0.adapters.with(|a| !a.is_empty())
    }

    /// The adapter in use: `hci0` if there is one, or else the first.
    pub fn adapter(&self) -> Option<BluetoothAdapter> {
        self.0.adapters.with(|a| a.first().cloned())
    }

    pub fn adapters(&self) -> Vec<BluetoothAdapter> {
        self.0.adapters.get()
    }

    /// Every device the adapters know of, connected ones first, then paired ones.
    pub fn devices(&self) -> Vec<BluetoothDevice> {
        self.0.devices.get()
    }

    /// Turns the adapter on or off.
    pub fn set_powered(&self, on: bool) -> Task<Result<(), ServiceError>> {
        let path = self
            .0
            .adapters
            .with_untracked(|a| a.first().map(|a| a.path.clone()));
        dbus::run(async move {
            let path = path.ok_or(ServiceError::Unavailable)?;
            let conn = dbus::system().await?;
            dbus::set_property(&conn, BLUEZ, &path, ADAPTER, "Powered", Value::from(on)).await
        })
    }

    /// Starts or stops looking for devices; what is found comes in [`Bluetooth::devices`].
    pub fn set_discovering(&self, on: bool) -> Task<Result<(), ServiceError>> {
        let path = self
            .0
            .adapters
            .with_untracked(|a| a.first().map(|a| a.path.clone()));
        let method = if on {
            "StartDiscovery"
        } else {
            "StopDiscovery"
        };
        dbus::run(async move {
            let path = path.ok_or(ServiceError::Unavailable)?;
            let conn = dbus::system().await?;
            dbus::call(&conn, BLUEZ, &path, ADAPTER, method, &()).await
        })
    }
}

impl BluetoothDevice {
    fn call(&self, method: &'static str) -> Task<Result<(), ServiceError>> {
        let path = self.path.clone();
        dbus::run(async move {
            let conn = dbus::system().await?;
            dbus::call(&conn, BLUEZ, &path, DEVICE, method, &()).await
        })
    }

    /// Connects to it, with each profile it has that is meant to connect by itself.
    pub fn connect(&self) -> Task<Result<(), ServiceError>> {
        self.call("Connect")
    }

    pub fn disconnect(&self) -> Task<Result<(), ServiceError>> {
        self.call("Disconnect")
    }

    /// Pairs with it. Devices that ask for a code can't be paired with this yet.
    pub fn pair(&self) -> Task<Result<(), ServiceError>> {
        self.call("Pair")
    }

    /// Whether it may connect without asking.
    pub fn set_trusted(&self, on: bool) -> Task<Result<(), ServiceError>> {
        let path = self.path.clone();
        dbus::run(async move {
            let conn = dbus::system().await?;
            dbus::set_property(&conn, BLUEZ, &path, DEVICE, "Trusted", Value::from(on)).await
        })
    }

    /// Unpairs it and has the adapter forget it.
    pub fn forget(&self) -> Task<Result<(), ServiceError>> {
        let (path, adapter) = (self.path.clone(), self.adapter.clone());
        dbus::run(async move {
            let conn = dbus::system().await?;
            let device = OwnedObjectPath::try_from(path).map_err(dbus::failed)?;
            dbus::call(&conn, BLUEZ, &adapter, ADAPTER, "RemoveDevice", &(device,)).await
        })
    }
}

/// The objects BlueZ tells of: path, then interface, then properties.
type Objects = BTreeMap<String, HashMap<String, Props>>;

fn adapter(path: &str, p: &Props) -> BluetoothAdapter {
    BluetoothAdapter {
        name: Some(string(p, "Alias"))
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| string(p, "Name")),
        address: string(p, "Address"),
        powered: get(p, "Powered").unwrap_or(false),
        discovering: get(p, "Discovering").unwrap_or(false),
        discoverable: get(p, "Discoverable").unwrap_or(false),
        path: path.to_owned(),
    }
}

fn device(path: &str, interfaces: &HashMap<String, Props>) -> Option<BluetoothDevice> {
    let p = interfaces.get(DEVICE)?;
    let address = string(p, "Address");
    Some(BluetoothDevice {
        name: Some(string(p, "Alias"))
            .filter(|a| !a.is_empty())
            .or_else(|| Some(string(p, "Name")).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| address.clone()),
        address,
        icon: string(p, "Icon"),
        paired: get(p, "Paired").unwrap_or(false),
        trusted: get(p, "Trusted").unwrap_or(false),
        connected: get(p, "Connected").unwrap_or(false),
        battery: interfaces.get(BATTERY).and_then(|b| get(b, "Percentage")),
        path: path.to_owned(),
        adapter: get::<OwnedObjectPath>(p, "Adapter")
            .map(|a| a.to_string())
            .unwrap_or_default(),
    })
}

fn publish(objects: &Objects) {
    let Some(s) = SHARED.get() else {
        return;
    };
    let mut adapters: Vec<BluetoothAdapter> = objects
        .iter()
        .filter_map(|(path, i)| Some(adapter(path, i.get(ADAPTER)?)))
        .collect();
    // `hci0` first; the map keeps the rest in order of path.
    adapters.sort_by_key(|a| !a.path.ends_with("/hci0"));
    let mut devices: Vec<BluetoothDevice> = objects
        .iter()
        .filter_map(|(path, i)| device(path, i))
        .collect();
    devices.sort_by(|a, b| {
        (!a.connected, !a.paired, a.name.to_lowercase()).cmp(&(
            !b.connected,
            !b.paired,
            b.name.to_lowercase(),
        ))
    });
    put(&s.adapters, adapters);
    put(&s.devices, devices);
}

async fn follow() {
    let Ok(conn) = dbus::system().await else {
        return;
    };
    dbus::while_owned(&conn, BLUEZ, || serve(&conn), || publish(&Objects::new())).await;
}

async fn serve(conn: &zbus::Connection) {
    // Properties change on the objects; objects come and go through the manager on "/".
    let Some(properties) = dbus::signals(conn, "/org/bluez", None).await else {
        return;
    };
    let Some(managed) = dbus::signals(conn, "/", Some(OBJECT_MANAGER)).await else {
        return;
    };
    let mut signals = properties.or(managed);
    let all: HashMap<OwnedObjectPath, HashMap<String, Props>> =
        dbus::call(conn, BLUEZ, "/", OBJECT_MANAGER, "GetManagedObjects", &())
            .await
            .unwrap_or_default();
    let mut objects: Objects = all.into_iter().map(|(p, i)| (p.to_string(), i)).collect();
    publish(&objects);
    while let Some(Ok(msg)) = signals.next().await {
        let Some((path, interface, member)) = dbus::signal_of(&msg) else {
            continue;
        };
        if let Some((interface, changed, invalidated)) = dbus::properties_changed(&msg) {
            let Some(props) = objects.get_mut(&path).and_then(|i| i.get_mut(&interface)) else {
                continue;
            };
            props.extend(changed);
            for name in invalidated {
                props.remove(&name);
            }
        } else if interface == OBJECT_MANAGER && member == "InterfacesAdded" {
            let Ok((object, added)) = msg
                .body()
                .deserialize::<(OwnedObjectPath, HashMap<String, Props>)>()
            else {
                continue;
            };
            // Other services may have a manager on "/" too.
            if !object.as_str().starts_with("/org/bluez") {
                continue;
            }
            objects.entry(object.to_string()).or_default().extend(added);
        } else if interface == OBJECT_MANAGER && member == "InterfacesRemoved" {
            let Ok((object, removed)) = msg.body().deserialize::<(OwnedObjectPath, Vec<String>)>()
            else {
                continue;
            };
            let Some(interfaces) = objects.get_mut(object.as_str()) else {
                continue;
            };
            for name in removed {
                interfaces.remove(&name);
            }
            if interfaces.is_empty() {
                objects.remove(object.as_str());
            }
        } else {
            continue;
        }
        publish(&objects);
    }
}
