//! The tray against a fake item on a bus of its own, with no watcher there but this one. Run with
//! `--features tray`. `what_the_real_tray_has` reads the session bus of the machine instead:
//! `--features tray --test tray_bus -- --ignored what_the_real_tray_has --nocapture`, alone.

#![cfg(target_os = "linux")]
#![cfg(feature = "tray")]

mod common;

use anti_webui_wayland_shell::{MenuToggle, TrayStatus, tray};
use common::{connect, private_bus, until};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

/// Width, height and ARGB, as items give pictures.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

#[derive(Default)]
struct Seen {
    activated: Vec<(i32, i32)>,
    events: Vec<(i32, String)>,
}

struct Item {
    title: Arc<Mutex<String>>,
    seen: Arc<Mutex<Seen>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, x: i32, y: i32) {
        self.seen.lock().unwrap().activated.push((x, y));
    }

    #[zbus(property)]
    fn id(&self) -> String {
        "fake".into()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        self.title.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "NeedsAttention".into()
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        "fake-icon".into()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        // Half-transparent red: A, R, G, B.
        vec![
            (2, 2, [255, 0, 0, 255].repeat(4)),
            (1, 1, vec![128, 255, 0, 0]),
        ]
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (
            String::new(),
            Vec::new(),
            "Fake".into(),
            "a <b>tip</b>".into(),
        )
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'static> {
        ObjectPath::try_from("/MenuBar").unwrap()
    }

    #[zbus(signal)]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn entry(id: i32, props: &[(&str, Value<'static>)], children: Vec<OwnedValue>) -> Layout {
    let props = props
        .iter()
        .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.clone()).unwrap()))
        .collect();
    (id, props, children)
}

fn child(layout: Layout) -> OwnedValue {
    OwnedValue::try_from(Value::from(layout)).unwrap()
}

struct Menu {
    seen: Arc<Mutex<Seen>>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, _parent: i32, _depth: i32, _props: Vec<String>) -> (u32, Layout) {
        let open = entry(1, &[("label", "_Open".into())], Vec::new());
        let line = entry(2, &[("type", "separator".into())], Vec::new());
        let check = entry(
            3,
            &[
                ("label", "Mute".into()),
                ("toggle-type", "checkmark".into()),
                ("toggle-state", 1i32.into()),
            ],
            Vec::new(),
        );
        let deep = entry(
            5,
            &[("label", "Quit".into()), ("enabled", false.into())],
            Vec::new(),
        );
        let sub = entry(
            4,
            &[
                ("label", "More".into()),
                ("children-display", "submenu".into()),
            ],
            vec![child(deep)],
        );
        let root = entry(
            0,
            &[],
            vec![child(open), child(line), child(check), child(sub)],
        );
        (1, root)
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn event(&self, id: i32, event: String, _data: OwnedValue, _timestamp: u32) {
        self.seen.lock().unwrap().events.push((id, event));
    }
}

#[test]
fn items_come_change_answer_and_go() {
    let _bus = private_bus();
    let tray = tray();
    until("the tray to be the watcher", || tray.available());

    let title = Arc::new(Mutex::new("Fake item".to_owned()));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let conn = connect(None);
    zbus::block_on(async {
        let server = conn.object_server();
        let item = Item {
            title: title.clone(),
            seen: seen.clone(),
        };
        server.at("/StatusNotifierItem", item).await.unwrap();
        let menu = Menu { seen: seen.clone() };
        server.at("/MenuBar", menu).await.unwrap();
        conn.call_method(
            Some("org.kde.StatusNotifierWatcher"),
            "/StatusNotifierWatcher",
            Some("org.kde.StatusNotifierWatcher"),
            "RegisterStatusNotifierItem",
            &("/StatusNotifierItem",),
        )
        .await
        .unwrap();
    });

    until("the item", || tray.items().len() == 1);
    let item = tray.items().remove(0);
    assert_eq!(item.id, "fake");
    assert_eq!(item.title, "Fake item");
    assert_eq!(item.status, TrayStatus::NeedsAttention);
    assert_eq!(item.icon.name.as_deref(), Some("fake-icon"));
    assert!(item.has_menu && !item.item_is_menu);
    let tooltip = item.tooltip.clone().unwrap();
    assert_eq!(
        (tooltip.title.as_str(), tooltip.body.as_str()),
        ("Fake", "a <b>tip</b>")
    );
    let sizes: Vec<u32> = item.icon.pixmaps.iter().map(|b| b.width()).collect();
    assert_eq!(sizes, [1, 2]);
    let mut px = Vec::new();
    item.icon.pixmaps[0].with_levels(|_, _, p| px = p.to_vec());
    assert_eq!(px, [128, 0, 0, 128]);

    // A new title, told without its value.
    *title.lock().unwrap() = "Renamed".into();
    zbus::block_on(async {
        let emitter = SignalEmitter::new(&conn, "/StatusNotifierItem").unwrap();
        Item::new_title(&emitter).await.unwrap();
    });
    until("the new title", || tray.items()[0].title == "Renamed");
    // The pictures didn't change, so they are the ones there were.
    let renamed = tray.items().remove(0);
    assert!(Arc::ptr_eq(&renamed.icon.pixmaps[0], &item.icon.pixmaps[0]));

    let ((), _owner) = anti_webui_core::reactive::create_root(|| {
        let activated = item.activate(3, 4);
        until("activating", || activated.is_done());
        assert_eq!(activated.get(), Some(Ok(())));
        assert_eq!(seen.lock().unwrap().activated, [(3, 4)]);

        assert!(
            item.menu().is_empty(),
            "the menu is read after it is asked for"
        );
        until("the menu", || !item.menu().is_empty());
        let menu = item.menu();
        assert_eq!(menu.len(), 4);
        assert_eq!(menu[0].label, "Open");
        assert!(menu[1].separator);
        assert_eq!(menu[2].toggle, Some(MenuToggle::Check(true)));
        assert_eq!(menu[3].children.len(), 1);
        assert_eq!(menu[3].children[0].label, "Quit");
        assert!(!menu[3].children[0].enabled && menu[0].enabled);

        let clicked = item.menu_clicked(1);
        until("the click", || clicked.is_done());
        assert_eq!(seen.lock().unwrap().events, [(1, "clicked".to_owned())]);
    });

    drop(conn);
    until("the item to go", || tray.items().is_empty());
    assert!(tray.available());
}

#[test]
#[ignore = "reads this machine's tray"]
fn what_the_real_tray_has() {
    let tray = tray();
    until("a watcher", || tray.available());
    std::thread::sleep(std::time::Duration::from_millis(500));
    let ((), _owner) = anti_webui_core::reactive::create_root(|| {
        for item in tray.items() {
            let sizes: Vec<_> = item
                .icon
                .pixmaps
                .iter()
                .map(|b| (b.width(), b.height()))
                .collect();
            eprintln!(
                "{:?} {:?} {:?} icon {:?} {sizes:?} tooltip {:?} menu {}",
                item.id, item.title, item.status, item.icon.name, item.tooltip, item.has_menu
            );
            if item.has_menu {
                let _ = item.menu();
                let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while item.menu().is_empty() && std::time::Instant::now() < end {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                for e in item.menu() {
                    eprintln!(
                        "  {} {:?} sep {} {:?} children {}",
                        e.id,
                        e.label,
                        e.separator,
                        e.toggle,
                        e.children.len()
                    );
                }
            }
        }
    });
}
