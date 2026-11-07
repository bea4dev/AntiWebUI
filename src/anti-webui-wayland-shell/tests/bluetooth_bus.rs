//! Bluetooth against a small BlueZ on a bus of the test's own. Run with `--features bluetooth`.
#![cfg(feature = "bluetooth")]

mod common;

use anti_webui_wayland_shell::bluetooth;
use common::{private_bus, until};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;

const HCI0: &str = "/org/bluez/hci0";
const PHONES: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";

type Log = Arc<Mutex<Vec<String>>>;

struct Adapter {
    powered: bool,
    discovering: bool,
}

#[zbus::interface(name = "org.bluez.Adapter1")]
impl Adapter {
    #[zbus(property)]
    fn address(&self) -> String {
        "00:11:22:33:44:55".into()
    }

    #[zbus(property)]
    fn alias(&self) -> String {
        "laptop".into()
    }

    #[zbus(property)]
    fn powered(&self) -> bool {
        self.powered
    }

    #[zbus(property)]
    fn set_powered(&mut self, on: bool) {
        self.powered = on;
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        self.discovering
    }

    async fn start_discovery(&mut self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
        self.discovering = true;
        let _ = self.discovering_changed(&e).await;
    }

    async fn stop_discovery(&mut self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
        self.discovering = false;
        let _ = self.discovering_changed(&e).await;
    }

    async fn remove_device(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        device: OwnedObjectPath,
    ) {
        let _ = server.remove::<Device, _>(&device).await;
    }
}

struct Device {
    connected: bool,
    trusted: bool,
    log: Log,
}

#[zbus::interface(name = "org.bluez.Device1")]
impl Device {
    #[zbus(property)]
    fn address(&self) -> String {
        "AA:BB:CC:DD:EE:FF".into()
    }

    #[zbus(property)]
    fn name(&self) -> String {
        "Phone".into()
    }

    #[zbus(property)]
    fn icon(&self) -> String {
        "phone".into()
    }

    #[zbus(property)]
    fn paired(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn connected(&self) -> bool {
        self.connected
    }

    #[zbus(property)]
    fn trusted(&self) -> bool {
        self.trusted
    }

    #[zbus(property)]
    fn set_trusted(&mut self, on: bool) {
        self.trusted = on;
    }

    #[zbus(property)]
    fn adapter(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(HCI0).unwrap()
    }

    async fn connect(&mut self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
        self.log.lock().unwrap().push("connect".into());
        self.connected = true;
        let _ = self.connected_changed(&e).await;
    }

    async fn disconnect(&mut self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) {
        self.connected = false;
        let _ = self.connected_changed(&e).await;
    }
}

struct Battery;

#[zbus::interface(name = "org.bluez.Battery1")]
impl Battery {
    #[zbus(property)]
    fn percentage(&self) -> u8 {
        80
    }
}

fn bluez(log: &Log) -> zbus::Connection {
    let log = log.clone();
    zbus::block_on(async move {
        zbus::connection::Builder::session()
            .unwrap()
            .serve_at("/", zbus::fdo::ObjectManager)
            .unwrap()
            .serve_at(
                HCI0,
                Adapter {
                    powered: true,
                    discovering: false,
                },
            )
            .unwrap()
            .serve_at(
                PHONES,
                Device {
                    connected: false,
                    trusted: false,
                    log,
                },
            )
            .unwrap()
            .name("org.bluez")
            .unwrap()
            .build()
            .await
            .unwrap()
    })
}

fn wait<T: Clone + 'static>(task: anti_webui_core::task::Task<T>) -> T {
    let got = Rc::new(RefCell::new(None));
    let g = got.clone();
    task.then(move |r| *g.borrow_mut() = Some(r));
    until("the reply", || got.borrow().is_some());
    got.borrow().clone().unwrap()
}

#[test]
fn bluez_is_read_followed_and_told() {
    let _bus = private_bus();
    let log = Log::default();
    let fake = bluez(&log);
    let bt = bluetooth();
    until("the adapter", || bt.available() && !bt.devices().is_empty());
    let adapter = bt.adapter().unwrap();
    assert_eq!(
        (
            adapter.name.as_str(),
            adapter.address.as_str(),
            adapter.powered
        ),
        ("laptop", "00:11:22:33:44:55", true)
    );
    let phone = bt.devices()[0].clone();
    assert_eq!(
        (
            phone.name.as_str(),
            phone.icon.as_str(),
            phone.paired,
            phone.connected
        ),
        ("Phone", "phone", true, false)
    );
    assert_eq!(phone.battery, None);

    // What is asked reaches BlueZ, and what changes comes back.
    assert_eq!(wait(bt.set_powered(false)), Ok(()));
    until("powered off", || !bt.adapter().unwrap().powered);
    assert_eq!(wait(bt.set_discovering(true)), Ok(()));
    until("discovering", || bt.adapter().unwrap().discovering);
    assert_eq!(wait(phone.connect()), Ok(()));
    until("connected", || bt.devices()[0].connected);
    assert_eq!(log.lock().unwrap().as_slice(), ["connect"]);
    assert_eq!(wait(phone.set_trusted(true)), Ok(()));
    until("trusted", || bt.devices()[0].trusted);

    // An interface that comes later is taken in.
    zbus::block_on(fake.object_server().at(PHONES, Battery)).unwrap();
    until("the battery", || bt.devices()[0].battery == Some(80));

    // Forgetting it removes it.
    assert_eq!(wait(phone.forget()), Ok(()));
    until("the device to go", || bt.devices().is_empty());

    // BlueZ going and coming back is followed.
    drop(fake);
    until("BlueZ to go", || !bt.available());
    let _again = bluez(&log);
    until("BlueZ to come back", || {
        bt.available() && bt.devices().len() == 1
    });
}

/// Reads this machine's BlueZ, changing nothing:
/// `cargo test -p anti-webui-wayland-shell --features bluetooth --test bluetooth_bus -- --ignored --nocapture`.
#[test]
#[ignore = "reads this machine's BlueZ"]
fn this_machine() {
    let bt = bluetooth();
    std::thread::sleep(std::time::Duration::from_millis(800));
    eprintln!(
        "{} {:#?}\n{:#?}",
        bt.available(),
        bt.adapters(),
        bt.devices()
    );
}
