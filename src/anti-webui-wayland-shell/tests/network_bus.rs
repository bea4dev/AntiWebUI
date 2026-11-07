//! The network service against a small NetworkManager on a bus of the test's own. Run with
//! `--features network`.
#![cfg(feature = "network")]

mod common;

use anti_webui_wayland_shell::{Connectivity, NetError, Security, network};
use common::until;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const NM: &str = "/org/freedesktop/NetworkManager";

fn path(s: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(s.to_owned()).unwrap()
}

/// What the client asked for.
type Asked = Arc<Mutex<Vec<String>>>;

struct Root {
    connectivity: u32,
    wireless_enabled: bool,
    asked: Asked,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Root {
    #[zbus(property)]
    fn devices(&self) -> Vec<OwnedObjectPath> {
        vec![
            path(&format!("{NM}/Devices/1")),
            path(&format!("{NM}/Devices/2")),
        ]
    }

    #[zbus(property)]
    fn connectivity(&self) -> u32 {
        self.connectivity
    }

    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.wireless_enabled
    }

    #[zbus(property)]
    fn set_wireless_enabled(&mut self, on: bool) {
        self.wireless_enabled = on;
    }

    fn activate_connection(
        &self,
        connection: OwnedObjectPath,
        _device: OwnedObjectPath,
        ap: OwnedObjectPath,
    ) -> OwnedObjectPath {
        self.asked.lock().unwrap().push(format!(
            "activate {} {}",
            connection.as_str(),
            ap.as_str()
        ));
        path(&format!("{NM}/ActiveConnection/1"))
    }

    fn add_and_activate_connection(
        &self,
        settings: HashMap<String, HashMap<String, OwnedValue>>,
        _device: OwnedObjectPath,
        ap: OwnedObjectPath,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        let psk = settings
            .get("802-11-wireless-security")
            .and_then(|s| s.get("psk"))
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default();
        self.asked
            .lock()
            .unwrap()
            .push(format!("add {} {psk}", ap.as_str()));
        (
            path(&format!("{NM}/Settings/2")),
            path(&format!("{NM}/ActiveConnection/1")),
        )
    }
}

struct Device {
    kind: u32,
    interface: &'static str,
    state: u32,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl Device {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.kind
    }

    #[zbus(property)]
    fn interface(&self) -> String {
        self.interface.into()
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state
    }
}

struct Wireless;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl Wireless {
    #[zbus(property)]
    fn access_points(&self) -> Vec<OwnedObjectPath> {
        (1..=4)
            .map(|n| path(&format!("{NM}/AccessPoint/{n}")))
            .collect()
    }

    #[zbus(property)]
    fn active_access_point(&self) -> OwnedObjectPath {
        path(&format!("{NM}/AccessPoint/1"))
    }
}

struct Wired;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wired")]
impl Wired {
    #[zbus(property)]
    fn speed(&self) -> u32 {
        1000
    }
}

struct Ap {
    ssid: &'static str,
    strength: u8,
    rsn: u32,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl Ap {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        self.ssid.as_bytes().to_vec()
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.strength
    }

    #[zbus(property)]
    fn frequency(&self) -> u32 {
        5180
    }

    #[zbus(property)]
    fn flags(&self) -> u32 {
        u32::from(self.rsn != 0)
    }

    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        0
    }

    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        self.rsn
    }
}

struct Settings;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
impl Settings {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        vec![path(&format!("{NM}/Settings/1"))]
    }
}

struct Saved;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Saved {
    fn get_settings(&self) -> HashMap<String, HashMap<String, OwnedValue>> {
        let ssid = OwnedValue::try_from(zbus::zvariant::Value::from(b"home".to_vec())).unwrap();
        HashMap::from([(
            "802-11-wireless".to_owned(),
            HashMap::from([("ssid".to_owned(), ssid)]),
        )])
    }
}

#[test]
fn networks_are_read_followed_and_joined() {
    let _bus = common::private_bus();
    let asked = Asked::default();
    let fake = common::connect(None);
    zbus::block_on(async {
        let s = fake.object_server();
        s.at(
            NM,
            Root {
                connectivity: 4,
                wireless_enabled: true,
                asked: asked.clone(),
            },
        )
        .await
        .unwrap();
        let wifi = format!("{NM}/Devices/1");
        s.at(
            wifi.as_str(),
            Device {
                kind: 2,
                interface: "wlan0",
                state: 100,
            },
        )
        .await
        .unwrap();
        s.at(wifi.as_str(), Wireless).await.unwrap();
        let wired = format!("{NM}/Devices/2");
        s.at(
            wired.as_str(),
            Device {
                kind: 1,
                interface: "enp3s0",
                state: 30,
            },
        )
        .await
        .unwrap();
        s.at(wired.as_str(), Wired).await.unwrap();
        // Two of the same network, an open one and one with a password nobody saved.
        for (n, ssid, strength, rsn) in [
            (1, "home", 70, 0x188),
            (2, "home", 40, 0x188),
            (3, "cafe", 90, 0),
            (4, "office", 50, 0x188),
        ] {
            s.at(
                format!("{NM}/AccessPoint/{n}").as_str(),
                Ap {
                    ssid,
                    strength,
                    rsn,
                },
            )
            .await
            .unwrap();
        }
        s.at(format!("{NM}/Settings").as_str(), Settings)
            .await
            .unwrap();
        s.at(format!("{NM}/Settings/1").as_str(), Saved)
            .await
            .unwrap();
        fake.request_name("org.freedesktop.NetworkManager")
            .await
            .unwrap();
    });

    let net = network();
    until("NetworkManager", || {
        net.available() && !net.access_points().is_empty()
    });
    assert_eq!(net.connectivity(), Connectivity::Full);
    assert!(net.wifi_enabled());
    let shown: Vec<_> = net
        .access_points()
        .iter()
        .map(|a| (a.ssid.clone(), a.strength, a.security, a.known, a.active))
        .collect();
    assert_eq!(
        shown,
        [
            ("cafe".to_owned(), 90, Security::Open, false, false),
            ("home".to_owned(), 70, Security::Wpa, true, true),
            ("office".to_owned(), 50, Security::Wpa, false, false),
        ]
    );
    assert_eq!(net.wifi().map(|a| a.ssid), Some("home".to_owned()));
    let wired = net.wired().unwrap();
    assert_eq!(
        (wired.interface.as_str(), wired.connected, wired.speed),
        ("enp3s0", false, 1000)
    );

    // What changes is followed.
    zbus::block_on(async {
        let s = fake.object_server();
        let root = s.interface::<_, Root>(NM).await.unwrap();
        root.get_mut().await.connectivity = 2;
        root.get()
            .await
            .connectivity_changed(root.signal_emitter())
            .await
            .unwrap();
        let ap = s
            .interface::<_, Ap>(format!("{NM}/AccessPoint/1").as_str())
            .await
            .unwrap();
        ap.get_mut().await.strength = 30;
        let emitter: &SignalEmitter = ap.signal_emitter();
        ap.get().await.strength_changed(emitter).await.unwrap();
    });
    until("portal", || net.connectivity() == Connectivity::Portal);
    until("weaker", || net.wifi().is_some_and(|a| a.strength == 30));
    // Still once, and now the other access point of it is the stronger.
    let home = net
        .access_points()
        .into_iter()
        .find(|a| a.ssid == "home")
        .unwrap();
    assert_eq!((home.strength, home.active), (40, true));

    // Turning Wi-Fi off sets the property.
    let done = Rc::new(RefCell::new(None));
    let d = done.clone();
    net.set_wifi_enabled(false)
        .then(move |r| *d.borrow_mut() = Some(r));
    until("the reply", || done.borrow().is_some());
    assert_eq!(*done.borrow(), Some(Ok(())));
    until("Wi-Fi off", || !net.wifi_enabled());

    // A network with a password nobody saved needs one.
    let office = net
        .access_points()
        .into_iter()
        .find(|a| a.ssid == "office")
        .unwrap();
    let joined = Rc::new(RefCell::new(None));
    let j = joined.clone();
    net.connect(&office, None)
        .then(move |r| *j.borrow_mut() = Some(r));
    until("the answer", || joined.borrow().is_some());
    assert_eq!(*joined.borrow(), Some(Err(NetError::NeedsSecret)));
    *joined.borrow_mut() = None;
    let j = joined.clone();
    net.connect(&office, Some("hunter2".into()))
        .then(move |r| *j.borrow_mut() = Some(r));
    until("the answer", || joined.borrow().is_some());
    assert_eq!(*joined.borrow(), Some(Ok(())));
    assert!(
        asked
            .lock()
            .unwrap()
            .contains(&format!("add {NM}/AccessPoint/4 hunter2"))
    );

    // A saved one joins with what is saved.
    *joined.borrow_mut() = None;
    let j = joined.clone();
    net.connect(&home, None)
        .then(move |r| *j.borrow_mut() = Some(r));
    until("the answer", || joined.borrow().is_some());
    assert_eq!(*joined.borrow(), Some(Ok(())));
    assert!(
        asked
            .lock()
            .unwrap()
            .contains(&format!("activate {NM}/Settings/1 {NM}/AccessPoint/2"))
    );
}

/// Reads this machine's NetworkManager, changing nothing: `-- --ignored this_machine`.
#[test]
#[ignore = "reads this machine's NetworkManager"]
fn this_machine() {
    let net = network();
    until("NetworkManager", || net.available());
    std::thread::sleep(std::time::Duration::from_millis(500));
    eprintln!(
        "{:?} wifi {} {:?}\n{:#?}\nwired {:?}",
        net.connectivity(),
        net.wifi_enabled(),
        net.wifi().map(|a| (a.ssid, a.strength, a.security)),
        net.access_points()
            .iter()
            .map(|a| (a.ssid.as_str(), a.strength, a.security, a.known, a.active))
            .collect::<Vec<_>>(),
        net.wired()
    );
}
