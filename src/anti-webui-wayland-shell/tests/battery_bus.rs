//! Batteries and power profiles against a UPower and a power-profiles-daemon in their place, on a
//! bus of the test's own. Run with `--features battery,power-profiles`.
#![cfg(all(feature = "battery", feature = "power-profiles"))]

mod common;

use anti_webui_wayland_shell::{BatteryKind, BatteryState, battery, power_profiles};
use common::{connect, private_bus, until};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

struct UPower {
    on_battery: bool,
    devices: Vec<OwnedObjectPath>,
}

#[zbus::interface(name = "org.freedesktop.UPower")]
impl UPower {
    #[zbus(property)]
    fn on_battery(&self) -> bool {
        self.on_battery
    }

    fn enumerate_devices(&self) -> Vec<OwnedObjectPath> {
        self.devices.clone()
    }

    #[zbus(signal)]
    async fn device_added(emitter: &SignalEmitter<'_>, path: ObjectPath<'_>) -> zbus::Result<()>;
}

struct Device {
    kind: u32,
    state: u32,
    percentage: f64,
    model: String,
}

#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl Device {
    #[zbus(property, name = "Type")]
    fn kind(&self) -> u32 {
        self.kind
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state
    }

    #[zbus(property)]
    fn percentage(&self) -> f64 {
        self.percentage
    }

    #[zbus(property)]
    fn is_present(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn time_to_empty(&self) -> i64 {
        if self.state == 2 { 3600 } else { 0 }
    }

    #[zbus(property)]
    fn time_to_full(&self) -> i64 {
        0
    }

    #[zbus(property)]
    fn energy_rate(&self) -> f64 {
        12.5
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        "battery-good-symbolic".into()
    }

    #[zbus(property)]
    fn model(&self) -> String {
        self.model.clone()
    }

    #[zbus(property)]
    fn vendor(&self) -> String {
        String::new()
    }
}

struct Profiles {
    active: String,
}

#[zbus::interface(name = "org.freedesktop.UPower.PowerProfiles")]
impl Profiles {
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.active.clone()
    }

    #[zbus(property)]
    fn set_active_profile(&mut self, profile: String) {
        self.active = profile;
    }

    #[zbus(property)]
    fn profiles(&self) -> Vec<HashMap<String, OwnedValue>> {
        ["power-saver", "balanced", "performance"]
            .into_iter()
            .map(|p| HashMap::from([("Profile".to_owned(), Value::from(p).try_into().unwrap())]))
            .collect()
    }

    #[zbus(property)]
    fn performance_degraded(&self) -> String {
        String::new()
    }
}

const DISPLAY: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const BAT0: &str = "/org/freedesktop/UPower/devices/battery_BAT0";
const MOUSE: &str = "/org/freedesktop/UPower/devices/mouse_0";

fn battery_device(state: u32, percentage: f64) -> Device {
    Device {
        kind: 2,
        state,
        percentage,
        model: "Blade".into(),
    }
}

#[test]
fn batteries_and_profiles_are_read_followed_and_set() {
    let _bus = private_bus();
    let fake = connect(None);
    zbus::block_on(async {
        let s = fake.object_server();
        let devices = vec![OwnedObjectPath::try_from(BAT0).unwrap()];
        s.at(
            "/org/freedesktop/UPower",
            UPower {
                on_battery: true,
                devices,
            },
        )
        .await
        .unwrap();
        s.at(DISPLAY, battery_device(2, 64.0)).await.unwrap();
        s.at(BAT0, battery_device(2, 64.0)).await.unwrap();
        s.at(
            "/org/freedesktop/UPower/PowerProfiles",
            Profiles {
                active: "balanced".into(),
            },
        )
        .await
        .unwrap();
        fake.request_name("org.freedesktop.UPower").await.unwrap();
        fake.request_name("org.freedesktop.UPower.PowerProfiles")
            .await
            .unwrap();
    });

    // What is there at the start.
    let b = battery();
    until("the battery", || b.available() && b.devices().len() == 1);
    assert_eq!(b.percentage(), 64.0);
    assert_eq!(b.state(), BatteryState::Discharging);
    assert!(b.on_battery() && !b.charging());
    assert_eq!(b.time_to_empty(), Some(Duration::from_secs(3600)));
    assert_eq!(b.energy_rate(), 12.5);
    assert_eq!(b.devices()[0].kind, BatteryKind::Battery);

    // Plugging in changes the display device.
    zbus::block_on(async {
        let iface = fake
            .object_server()
            .interface::<_, Device>(DISPLAY)
            .await
            .unwrap();
        let mut d = iface.get_mut().await;
        d.state = 1;
        d.percentage = 65.0;
        d.state_changed(iface.signal_emitter()).await.unwrap();
        d.percentage_changed(iface.signal_emitter()).await.unwrap();
    });
    until("charging", || b.state() == BatteryState::Charging);
    assert_eq!(b.percentage(), 65.0);
    assert!(b.charging());

    // A mouse comes.
    zbus::block_on(async {
        let s = fake.object_server();
        s.at(
            MOUSE,
            Device {
                kind: 5,
                state: 2,
                percentage: 30.0,
                model: "Mouse".into(),
            },
        )
        .await
        .unwrap();
        let root = s
            .interface::<_, UPower>("/org/freedesktop/UPower")
            .await
            .unwrap();
        UPower::device_added(root.signal_emitter(), ObjectPath::try_from(MOUSE).unwrap())
            .await
            .unwrap();
    });
    until("the mouse", || b.devices().len() == 2);
    let mouse = b
        .devices()
        .into_iter()
        .find(|d| d.model == "Mouse")
        .unwrap();
    assert_eq!((mouse.kind, mouse.percentage), (BatteryKind::Mouse, 30.0));

    // Profiles.
    let p = power_profiles();
    until("the profiles", || p.available() && p.profiles().len() == 3);
    assert_eq!(p.active(), "balanced");
    let done = Rc::new(RefCell::new(None));
    let d = done.clone();
    p.set_active("performance")
        .then(move |r| *d.borrow_mut() = Some(r));
    until("the reply", || done.borrow().is_some());
    assert_eq!(*done.borrow(), Some(Ok(())));
    until("the new profile", || p.active() == "performance");

    // UPower going away empties it.
    drop(fake);
    until("nothing", || !b.available() && b.devices().is_empty());
    until("no profiles", || !p.available());
}
