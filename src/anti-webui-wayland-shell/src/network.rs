//! Networks through NetworkManager: whether the internet can be reached, Wi-Fi and its access
//! points, and the wired connection.

use crate::ServiceError;
use crate::dbus::{self, Props, get, put, string};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use futures_lite::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const NM: &str = "org.freedesktop.NetworkManager";
const ROOT: &str = "/org/freedesktop/NetworkManager";
const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const WIRED: &str = "org.freedesktop.NetworkManager.Device.Wired";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";

/// `DeviceType` of an ethernet and a Wi-Fi device.
const ETHERNET: u32 = 1;
const WIFI: u32 = 2;
/// `State` of a device that is connected.
const ACTIVATED: u32 = 100;

/// How far out a connection reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Connectivity {
    #[default]
    Unknown,
    /// Not connected at all.
    None,
    /// Behind a login page, like a hotel's.
    Portal,
    /// Connected, but the internet can't be reached.
    Limited,
    Full,
}

impl Connectivity {
    fn of(n: u32) -> Self {
        match n {
            1 => Self::None,
            2 => Self::Portal,
            3 => Self::Limited,
            4 => Self::Full,
            _ => Self::Unknown,
        }
    }
}

/// What it takes to join a Wi-Fi network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Security {
    #[default]
    Open,
    Wep,
    /// WPA or WPA2 with a password.
    Wpa,
    /// WPA3 with a password (SAE).
    Wpa3,
    /// 802.1X, with an account rather than a password.
    Enterprise,
}

impl Security {
    fn of(p: &Props) -> Self {
        let flags: u32 = get(p, "Flags").unwrap_or(0);
        let keys = get::<u32>(p, "WpaFlags").unwrap_or(0) | get::<u32>(p, "RsnFlags").unwrap_or(0);
        if keys & 0x200 != 0 {
            Self::Enterprise
        } else if keys & 0x100 != 0 {
            Self::Wpa
        } else if keys & 0x400 != 0 {
            Self::Wpa3
        } else if keys & 0x800 != 0 || keys & 0x1000 != 0 {
            // Enhanced open: encrypted, but without a password.
            Self::Open
        } else if keys != 0 {
            Self::Wpa
        } else if flags & 1 != 0 {
            Self::Wep
        } else {
            Self::Open
        }
    }
}

/// A Wi-Fi network as an access point gives it, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessPoint {
    pub ssid: String,
    /// From 0 to 100.
    pub strength: u8,
    /// In MHz.
    pub frequency: u32,
    pub security: Security,
    /// A connection to it is saved, so it joins without asking.
    pub known: bool,
    /// It is the one connected to.
    pub active: bool,
    path: String,
}

/// The wired connection, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct Wired {
    /// Like `enp3s0`.
    pub interface: String,
    pub connected: bool,
    /// In Mb/s, 0 when not known.
    pub speed: u32,
}

/// Why joining a network didn't happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    /// It needs a password, which wasn't given or isn't saved.
    NeedsSecret,
    Service(ServiceError),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::NeedsSecret => f.write_str("a password is needed"),
            NetError::Service(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for NetError {}

impl From<ServiceError> for NetError {
    fn from(e: ServiceError) -> Self {
        NetError::Service(e)
    }
}

/// The networks. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Network(&'static Shared);

#[derive(Debug)]
struct Shared {
    available: SharedSignal<bool>,
    connectivity: SharedSignal<Connectivity>,
    wifi_enabled: SharedSignal<bool>,
    wifi: SharedSignal<Option<AccessPoint>>,
    access_points: SharedSignal<Vec<AccessPoint>>,
    wired: SharedSignal<Option<Wired>>,
    /// The Wi-Fi device, and the saved connection of each SSID, for joining.
    wifi_device: Mutex<Option<String>>,
    saved: Mutex<HashMap<String, String>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The networks. It starts following them the first time.
pub fn network() -> Network {
    Network(SHARED.get_or_init(|| {
        dbus::spawn(follow());
        Shared {
            available: shared_signal(false),
            connectivity: shared_signal(Connectivity::Unknown),
            wifi_enabled: shared_signal(false),
            wifi: shared_signal(None),
            access_points: shared_signal(Vec::new()),
            wired: shared_signal(None),
            wifi_device: Mutex::new(None),
            saved: Mutex::new(HashMap::new()),
        }
    }))
}

impl Network {
    /// Whether NetworkManager is there.
    pub fn available(&self) -> bool {
        self.0.available.get()
    }

    pub fn connectivity(&self) -> Connectivity {
        self.0.connectivity.get()
    }

    /// Whether Wi-Fi is on, which turning airplane mode on turns off.
    pub fn wifi_enabled(&self) -> bool {
        self.0.wifi_enabled.get()
    }

    pub fn set_wifi_enabled(&self, on: bool) -> Task<Result<(), ServiceError>> {
        dbus::run(async move {
            let conn = dbus::system().await?;
            dbus::set_property(&conn, NM, ROOT, NM, "WirelessEnabled", Value::from(on)).await
        })
    }

    /// The access point Wi-Fi is connected to.
    pub fn wifi(&self) -> Option<AccessPoint> {
        self.0.wifi.get()
    }

    /// The Wi-Fi networks around, each once, the strongest first.
    pub fn access_points(&self) -> Vec<AccessPoint> {
        self.0.access_points.get()
    }

    /// Looks for networks again; what is found comes in [`Network::access_points`].
    pub fn scan(&self) -> Task<Result<(), ServiceError>> {
        let device = self.wifi_device();
        dbus::run(async move {
            let device = device.ok_or(ServiceError::Unavailable)?;
            let conn = dbus::system().await?;
            let options: HashMap<&str, Value> = HashMap::new();
            dbus::call(&conn, NM, &device, WIRELESS, "RequestScan", &(options,)).await
        })
    }

    /// Joins a network: with its saved connection if there is one, or else with `password`,
    /// saving it.
    pub fn connect(
        &self,
        ap: &AccessPoint,
        password: Option<String>,
    ) -> Task<Result<(), NetError>> {
        let device = self.wifi_device();
        let saved = self
            .0
            .saved
            .lock()
            .ok()
            .and_then(|s| s.get(&ap.ssid).cloned());
        let ap = ap.clone();
        let (task, done) = Task::pending();
        dbus::spawn(async move { done.complete(join(device, saved, ap, password).await) });
        task
    }

    /// Leaves the Wi-Fi network, staying off it until asked to join one.
    pub fn disconnect(&self) -> Task<Result<(), ServiceError>> {
        let device = self.wifi_device();
        dbus::run(async move {
            let device = device.ok_or(ServiceError::Unavailable)?;
            let conn = dbus::system().await?;
            dbus::call(&conn, NM, &device, DEVICE, "Disconnect", &()).await
        })
    }

    /// The first wired device.
    pub fn wired(&self) -> Option<Wired> {
        self.0.wired.get()
    }

    fn wifi_device(&self) -> Option<String> {
        self.0.wifi_device.lock().ok().and_then(|d| d.clone())
    }
}

/// Joins `ap` on `device`, with a saved connection or the password.
async fn join(
    device: Option<String>,
    saved: Option<String>,
    ap: AccessPoint,
    password: Option<String>,
) -> Result<(), NetError> {
    let device = device.ok_or(ServiceError::Unavailable)?;
    let conn = dbus::system().await?;
    if let Some(saved) = saved {
        let body = (path(&saved)?, path(&device)?, path(&ap.path)?);
        return call_joining::<_, OwnedObjectPath>(&conn, "ActivateConnection", &body)
            .await
            .map(drop);
    }
    let security = match (ap.security, password) {
        (Security::Open, _) => HashMap::new(),
        (Security::Enterprise, _) => {
            return Err(NetError::Service(ServiceError::Unsupported));
        }
        (_, None) => return Err(NetError::NeedsSecret),
        (Security::Wep, Some(key)) => HashMap::from([
            ("key-mgmt", Value::from("none")),
            ("wep-key0", Value::from(key)),
            ("wep-key-type", Value::from(1u32)),
        ]),
        (Security::Wpa3, Some(psk)) => {
            HashMap::from([("key-mgmt", Value::from("sae")), ("psk", Value::from(psk))])
        }
        (Security::Wpa, Some(psk)) => HashMap::from([
            ("key-mgmt", Value::from("wpa-psk")),
            ("psk", Value::from(psk)),
        ]),
    };
    let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    if !security.is_empty() {
        settings.insert("802-11-wireless-security", security);
    }
    let body = (settings, path(&device)?, path(&ap.path)?);
    call_joining::<_, (OwnedObjectPath, OwnedObjectPath)>(&conn, "AddAndActivateConnection", &body)
        .await
        .map(drop)
}

fn path(s: &str) -> Result<ObjectPath<'_>, ServiceError> {
    ObjectPath::try_from(s).map_err(|e| ServiceError::Failed(e.to_string()))
}

/// Calls a method of NetworkManager that joins a network, telling a missing password from the
/// rest of what can go wrong.
async fn call_joining<B, R>(conn: &zbus::Connection, method: &str, body: &B) -> Result<R, NetError>
where
    B: zbus::export::serde::ser::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let reply = match conn
        .call_method(Some(NM), ROOT, Some(NM), method, body)
        .await
    {
        Ok(reply) => reply,
        Err(zbus::Error::MethodError(name, ..)) if name.as_str().contains("NoSecrets") => {
            return Err(NetError::NeedsSecret);
        }
        Err(e) => return Err(dbus::failed(e).into()),
    };
    reply
        .body()
        .deserialize()
        .map_err(|e| NetError::Service(dbus::failed(e)))
}

/// What NetworkManager has told so far.
#[derive(Default)]
struct Model {
    root: Props,
    /// Devices by path, with the properties of each of their interfaces.
    devices: BTreeMap<String, Props>,
    access_points: BTreeMap<String, Props>,
    /// The SSID of each saved connection, by its path.
    saved: BTreeMap<String, String>,
}

fn paths(p: &Props, name: &str) -> Vec<String> {
    get::<Vec<OwnedObjectPath>>(p, name)
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string())
        .collect()
}

fn device_type(p: &Props) -> u32 {
    get(p, "DeviceType").unwrap_or(0)
}

impl Model {
    fn wifi_devices(&self) -> impl Iterator<Item = (&String, &Props)> {
        self.devices.iter().filter(|(_, p)| device_type(p) == WIFI)
    }

    fn access_point(&self, path: &str, active: &[String]) -> Option<AccessPoint> {
        let p = self.access_points.get(path)?;
        let ssid =
            String::from_utf8_lossy(&get::<Vec<u8>>(p, "Ssid").unwrap_or_default()).into_owned();
        if ssid.is_empty() {
            return None;
        }
        Some(AccessPoint {
            known: self.saved.values().any(|s| *s == ssid),
            active: active.iter().any(|a| a == path),
            ssid,
            strength: get(p, "Strength").unwrap_or(0),
            frequency: get(p, "Frequency").unwrap_or(0),
            security: Security::of(p),
            path: path.to_owned(),
        })
    }

    fn publish(&self) {
        let Some(s) = SHARED.get() else {
            return;
        };
        put(&s.available, !self.root.is_empty());
        put(
            &s.connectivity,
            Connectivity::of(get(&self.root, "Connectivity").unwrap_or(0)),
        );
        put(
            &s.wifi_enabled,
            get(&self.root, "WirelessEnabled").unwrap_or(false),
        );
        let active: Vec<String> = self
            .wifi_devices()
            .filter_map(|(_, p)| get::<OwnedObjectPath>(p, "ActiveAccessPoint"))
            .map(|p| p.to_string())
            .filter(|p| p != "/")
            .collect();
        put(
            &s.wifi,
            active.first().and_then(|a| self.access_point(a, &active)),
        );
        // Each SSID once: the strongest of its access points, active if any of them is.
        let mut by_ssid: BTreeMap<String, AccessPoint> = BTreeMap::new();
        for (_, p) in self.wifi_devices() {
            for path in paths(p, "AccessPoints") {
                let Some(ap) = self.access_point(&path, &active) else {
                    continue;
                };
                match by_ssid.get_mut(&ap.ssid) {
                    Some(kept) => {
                        let active = kept.active || ap.active;
                        if ap.strength > kept.strength {
                            *kept = ap;
                        }
                        kept.active = active;
                    }
                    None => {
                        by_ssid.insert(ap.ssid.clone(), ap);
                    }
                }
            }
        }
        let mut list: Vec<AccessPoint> = by_ssid.into_values().collect();
        list.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.ssid.cmp(&b.ssid)));
        put(&s.access_points, list);
        put(
            &s.wired,
            self.devices
                .values()
                .find(|p| device_type(p) == ETHERNET)
                .map(|p| Wired {
                    interface: string(p, "Interface"),
                    connected: get::<u32>(p, "State") == Some(ACTIVATED),
                    speed: get(p, "Speed").unwrap_or(0),
                }),
        );
        if let Ok(mut d) = s.wifi_device.lock() {
            *d = self.wifi_devices().next().map(|(path, _)| path.clone());
        }
        if let Ok(mut saved) = s.saved.lock() {
            *saved = self
                .saved
                .iter()
                .map(|(path, ssid)| (ssid.clone(), path.clone()))
                .collect();
        }
    }

    /// Reads the devices and access points it doesn't have yet, and forgets those that went.
    async fn sync(&mut self, conn: &zbus::Connection) {
        let devices = paths(&self.root, "Devices");
        self.devices.retain(|path, _| devices.contains(path));
        for path in devices {
            if self.devices.contains_key(&path) {
                continue;
            }
            let Some(mut p) = dbus::get_all(conn, NM, &path, DEVICE).await else {
                continue;
            };
            let more = match device_type(&p) {
                WIFI => Some(WIRELESS),
                ETHERNET => Some(WIRED),
                _ => None,
            };
            if let Some(interface) = more
                && let Some(more) = dbus::get_all(conn, NM, &path, interface).await
            {
                p.extend(more);
            }
            self.devices.insert(path, p);
        }
        let access_points: Vec<String> = self
            .wifi_devices()
            .flat_map(|(_, p)| paths(p, "AccessPoints"))
            .collect();
        self.access_points
            .retain(|path, _| access_points.contains(path));
        for path in access_points {
            if self.access_points.contains_key(&path) {
                continue;
            }
            if let Some(p) = dbus::get_all(conn, NM, &path, ACCESS_POINT).await {
                self.access_points.insert(path, p);
            }
        }
    }

    async fn read_saved(&mut self, conn: &zbus::Connection, path: &str) {
        let settings: Result<HashMap<String, HashMap<String, OwnedValue>>, _> =
            dbus::call(conn, NM, path, CONNECTION, "GetSettings", &()).await;
        let ssid = settings
            .ok()
            .and_then(|s| get::<Vec<u8>>(s.get("802-11-wireless")?, "ssid"))
            .map(|s| String::from_utf8_lossy(&s).into_owned());
        match ssid {
            Some(ssid) => {
                self.saved.insert(path.to_owned(), ssid);
            }
            None => {
                self.saved.remove(path);
            }
        }
    }
}

async fn follow() {
    let Ok(conn) = dbus::system().await else {
        return;
    };
    dbus::while_owned(&conn, NM, || serve(&conn), || Model::default().publish()).await;
}

async fn serve(conn: &zbus::Connection) {
    // Signals first, so that nothing is missed between reading and following.
    let Some(mut signals) = dbus::signals(conn, ROOT, None).await else {
        return;
    };
    let mut model = Model {
        root: dbus::get_all(conn, NM, ROOT, NM).await.unwrap_or_default(),
        ..Default::default()
    };
    let saved: Vec<OwnedObjectPath> =
        dbus::call(conn, NM, SETTINGS, SETTINGS_IFACE, "ListConnections", &())
            .await
            .unwrap_or_default();
    for path in saved {
        model.read_saved(conn, &path).await;
    }
    model.sync(conn).await;
    model.publish();
    while let Some(Ok(msg)) = signals.next().await {
        let Some((path, interface, member)) = dbus::signal_of(&msg) else {
            continue;
        };
        if let Some((_, changed, _)) = dbus::properties_changed(&msg) {
            let props = if path == ROOT {
                &mut model.root
            } else if let Some(p) = model.devices.get_mut(&path) {
                p
            } else if let Some(p) = model.access_points.get_mut(&path) {
                p
            } else {
                continue;
            };
            props.extend(changed);
        } else if interface == SETTINGS_IFACE
            && (member == "NewConnection" || member == "ConnectionRemoved")
        {
            let Ok(saved) = msg.body().deserialize::<OwnedObjectPath>() else {
                continue;
            };
            if member == "ConnectionRemoved" {
                model.saved.remove(saved.as_str());
            } else {
                model.read_saved(conn, &saved).await;
            }
        } else if interface == CONNECTION && member == "Updated" {
            model.read_saved(conn, &path).await;
        } else if !(interface == NM && (member == "DeviceAdded" || member == "DeviceRemoved")
            || interface == WIRELESS
                && (member == "AccessPointAdded" || member == "AccessPointRemoved"))
        {
            continue;
        }
        // The lists in the properties say what is there; the signals only say when to look.
        if let Ok(added) = msg.body().deserialize::<OwnedObjectPath>() {
            if member == "AccessPointAdded"
                && let Some(p) = model.devices.get_mut(&path)
            {
                let mut list: Vec<OwnedObjectPath> = get(p, "AccessPoints").unwrap_or_default();
                if !list.contains(&added) {
                    list.push(added);
                    if let Ok(v) = OwnedValue::try_from(Value::from(list)) {
                        p.insert("AccessPoints".into(), v);
                    }
                }
            } else if member == "DeviceAdded" {
                let mut list: Vec<OwnedObjectPath> =
                    get(&model.root, "Devices").unwrap_or_default();
                if !list.contains(&added) {
                    list.push(added);
                    if let Ok(v) = OwnedValue::try_from(Value::from(list)) {
                        model.root.insert("Devices".into(), v);
                    }
                }
            }
        }
        model.sync(conn).await;
        model.publish();
    }
}
