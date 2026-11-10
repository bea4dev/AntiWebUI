//! Batteries through UPower: the laptop's, as one for all its batteries, and those of mice,
//! keyboards and headsets.

use crate::dbus::{self, Props, get, put, string};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use futures_lite::StreamExt;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;
use zbus::zvariant::OwnedObjectPath;

const UPOWER: &str = "org.freedesktop.UPower";
const ROOT: &str = "/org/freedesktop/UPower";
const DISPLAY: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICE: &str = "org.freedesktop.UPower.Device";

/// Whether it is filling up or running down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BatteryState {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Empty,
    Full,
    /// Plugged in, but waiting to charge, like when a threshold says not yet.
    PendingCharge,
    PendingDischarge,
}

impl BatteryState {
    fn of(n: u32) -> Self {
        match n {
            1 => Self::Charging,
            2 => Self::Discharging,
            3 => Self::Empty,
            4 => Self::Full,
            5 => Self::PendingCharge,
            6 => Self::PendingDischarge,
            _ => Self::Unknown,
        }
    }
}

/// What a battery is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BatteryKind {
    /// The computer's own.
    Battery,
    Ups,
    Mouse,
    Keyboard,
    Phone,
    Tablet,
    GamingInput,
    Pen,
    Touchpad,
    Headset,
    Headphones,
    Speakers,
    /// What UPower calls by this number.
    Other(u32),
}

impl BatteryKind {
    fn of(n: u32) -> Self {
        match n {
            2 => Self::Battery,
            3 => Self::Ups,
            5 => Self::Mouse,
            6 => Self::Keyboard,
            8 => Self::Phone,
            10 => Self::Tablet,
            12 => Self::GamingInput,
            13 => Self::Pen,
            14 => Self::Touchpad,
            17 => Self::Headset,
            18 => Self::Speakers,
            19 => Self::Headphones,
            n => Self::Other(n),
        }
    }
}

/// A battery, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct BatteryDevice {
    pub kind: BatteryKind,
    pub model: String,
    pub vendor: String,
    /// From 0 to 100.
    pub percentage: f64,
    pub state: BatteryState,
    /// The name of an icon in the icon theme, like `battery-good-symbolic`.
    pub icon_name: String,
    /// What UPower calls it by, the same while it is there.
    pub path: String,
}

/// The batteries. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Battery(&'static Shared);

#[derive(Debug)]
struct Shared {
    present: SharedSignal<bool>,
    percentage: SharedSignal<f64>,
    state: SharedSignal<BatteryState>,
    on_battery: SharedSignal<bool>,
    time_to_empty: SharedSignal<Option<Duration>>,
    time_to_full: SharedSignal<Option<Duration>>,
    energy_rate: SharedSignal<f64>,
    energy: SharedSignal<f64>,
    energy_full: SharedSignal<f64>,
    health: SharedSignal<Option<f64>>,
    cycles: SharedSignal<Option<u32>>,
    icon_name: SharedSignal<String>,
    devices: SharedSignal<Vec<BatteryDevice>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The batteries. It starts following them the first time.
pub fn battery() -> Battery {
    Battery(dbus::init_then_follow(&SHARED, follow, || Shared {
        present: shared_signal(false),
        percentage: shared_signal(0.0),
        state: shared_signal(BatteryState::Unknown),
        on_battery: shared_signal(false),
        time_to_empty: shared_signal(None),
        time_to_full: shared_signal(None),
        energy_rate: shared_signal(0.0),
        energy: shared_signal(0.0),
        energy_full: shared_signal(0.0),
        health: shared_signal(None),
        cycles: shared_signal(None),
        icon_name: shared_signal(String::new()),
        devices: shared_signal(Vec::new()),
    }))
}

impl Battery {
    /// Whether there is a battery in the computer, which UPower tells of.
    pub fn available(&self) -> bool {
        self.0.present.get()
    }

    /// All of the computer's batteries as one, from 0 to 100.
    pub fn percentage(&self) -> f64 {
        self.0.percentage.get()
    }

    pub fn state(&self) -> BatteryState {
        self.0.state.get()
    }

    /// Plugged in or filling up.
    pub fn charging(&self) -> bool {
        matches!(
            self.state(),
            BatteryState::Charging | BatteryState::PendingCharge | BatteryState::Full
        )
    }

    /// Running on batteries rather than from the wall.
    pub fn on_battery(&self) -> bool {
        self.0.on_battery.get()
    }

    /// How long until empty, when that is known.
    pub fn time_to_empty(&self) -> Option<Duration> {
        self.0.time_to_empty.get()
    }

    /// How long until full, when that is known.
    pub fn time_to_full(&self) -> Option<Duration> {
        self.0.time_to_full.get()
    }

    /// How fast it fills up or runs down, in watts.
    pub fn energy_rate(&self) -> f64 {
        self.0.energy_rate.get()
    }

    /// How much energy is in it now, in watt-hours.
    pub fn energy(&self) -> f64 {
        self.0.energy.get()
    }

    /// How much energy it holds when full, in watt-hours.
    pub fn energy_full(&self) -> f64 {
        self.0.energy_full.get()
    }

    /// How much it holds when full against what it held when new, from 0 to 100, when the
    /// batteries say.
    pub fn health(&self) -> Option<f64> {
        self.0.health.get()
    }

    /// How many times it has been charged through, when the batteries say: the most of them.
    pub fn charge_cycles(&self) -> Option<u32> {
        self.0.cycles.get()
    }

    /// The name of an icon in the icon theme for how it is, like `battery-good-charging-symbolic`.
    pub fn icon_name(&self) -> String {
        self.0.icon_name.get()
    }

    /// Each battery, the computer's and those of what is connected to it, like a mouse.
    pub fn devices(&self) -> Vec<BatteryDevice> {
        self.0.devices.get()
    }
}

/// The devices UPower tells of, by path.
type Devices = BTreeMap<String, Props>;

fn device(path: &str, p: &Props) -> Option<BatteryDevice> {
    let kind = get::<u32>(p, "Type")?;
    // Mains power and the display device aren't batteries.
    if kind == 1 || path == DISPLAY || !get::<bool>(p, "IsPresent").unwrap_or(true) {
        return None;
    }
    Some(BatteryDevice {
        kind: BatteryKind::of(kind),
        model: string(p, "Model"),
        vendor: string(p, "Vendor"),
        percentage: get(p, "Percentage").unwrap_or(0.0),
        state: BatteryState::of(get(p, "State").unwrap_or(0)),
        icon_name: string(p, "IconName"),
        path: path.to_owned(),
    })
}

fn seconds(p: &Props, name: &str) -> Option<Duration> {
    get::<i64>(p, name)
        .filter(|s| *s > 0)
        .map(|s| Duration::from_secs(s as u64))
}

fn publish(root: &Props, display: &Props, devices: &Devices) {
    let Some(s) = SHARED.get() else {
        return;
    };
    put(&s.on_battery, get(root, "OnBattery").unwrap_or(false));
    put(
        &s.present,
        get(display, "IsPresent").unwrap_or(false) && get::<u32>(display, "Type") == Some(2),
    );
    put(&s.percentage, get(display, "Percentage").unwrap_or(0.0));
    put(
        &s.state,
        BatteryState::of(get(display, "State").unwrap_or(0)),
    );
    put(&s.time_to_empty, seconds(display, "TimeToEmpty"));
    put(&s.time_to_full, seconds(display, "TimeToFull"));
    put(&s.energy_rate, get(display, "EnergyRate").unwrap_or(0.0));
    put(&s.energy, get(display, "Energy").unwrap_or(0.0));
    put(&s.energy_full, get(display, "EnergyFull").unwrap_or(0.0));
    // The computer's own batteries (not a mouse's), which the display device leaves these out of.
    let own: Vec<&Props> = devices
        .values()
        .filter(|p| {
            get::<u32>(p, "Type") == Some(2)
                && get::<bool>(p, "PowerSupply").unwrap_or(false)
                && get::<bool>(p, "IsPresent").unwrap_or(true)
        })
        .collect();
    let healths: Vec<f64> = own
        .iter()
        .filter_map(|p| get::<f64>(p, "Capacity"))
        .filter(|c| *c > 0.0)
        .collect();
    put(
        &s.health,
        (!healths.is_empty()).then(|| healths.iter().sum::<f64>() / healths.len() as f64),
    );
    put(
        &s.cycles,
        own.iter()
            .filter_map(|p| get::<i32>(p, "ChargeCycles"))
            .filter(|c| *c >= 0)
            .max()
            .map(|c| c as u32),
    );
    put(&s.icon_name, string(display, "IconName"));
    put(
        &s.devices,
        devices
            .iter()
            .filter_map(|(path, p)| device(path, p))
            .collect(),
    );
}

async fn follow() {
    let Ok(conn) = dbus::system().await else {
        return;
    };
    dbus::while_owned(
        &conn,
        UPOWER,
        || serve(&conn),
        || publish(&Props::new(), &Props::new(), &Devices::new()),
    )
    .await;
}

async fn serve(conn: &zbus::Connection) {
    // Signals first, so that nothing is missed between reading and following.
    let Some(mut signals) = dbus::signals(conn, ROOT, None).await else {
        return;
    };
    let mut root = dbus::get_all(conn, UPOWER, ROOT, UPOWER)
        .await
        .unwrap_or_default();
    let mut display = dbus::get_all(conn, UPOWER, DISPLAY, DEVICE)
        .await
        .unwrap_or_default();
    let mut devices = Devices::new();
    let paths: Vec<OwnedObjectPath> =
        dbus::call(conn, UPOWER, ROOT, UPOWER, "EnumerateDevices", &())
            .await
            .unwrap_or_default();
    for path in paths {
        if let Some(p) = dbus::get_all(conn, UPOWER, &path, DEVICE).await {
            devices.insert(path.to_string(), p);
        }
    }
    publish(&root, &display, &devices);
    while let Some(Ok(msg)) = signals.next().await {
        let Some((path, interface, member)) = dbus::signal_of(&msg) else {
            continue;
        };
        if let Some((_, changed, _)) = dbus::properties_changed(&msg) {
            let props = if path == ROOT {
                &mut root
            } else if path == DISPLAY {
                &mut display
            } else {
                match devices.get_mut(&path) {
                    Some(props) => props,
                    None => continue,
                }
            };
            props.extend(changed);
        } else if interface == UPOWER && (member == "DeviceAdded" || member == "DeviceRemoved") {
            let Ok(device) = msg.body().deserialize::<OwnedObjectPath>() else {
                continue;
            };
            if member == "DeviceRemoved" {
                devices.remove(device.as_str());
            } else if let Some(p) = dbus::get_all(conn, UPOWER, &device, DEVICE).await {
                devices.insert(device.to_string(), p);
            }
        } else {
            continue;
        }
        publish(&root, &display, &devices);
    }
}
