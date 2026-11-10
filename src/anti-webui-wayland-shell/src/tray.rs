//! The system tray: StatusNotifierItem, with the menus of `com.canonical.dbusmenu`. This is a host
//! of the watcher there is, or the watcher itself when there is none.

use crate::ServiceError;
use crate::dbus::{self, Props, get, put, string};
use anti_webui_core::images::Bitmap;
use anti_webui_core::reactive::Signal;
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use anti_webui_core::view::{View, dyn_view};
use anti_webui_widgets::{MenuEntry, menu, menu_item, menu_separator, submenu};
use futures_lite::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use zbus::fdo::RequestNameFlags;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, Message, MessageStream};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// What items are, the first more often than the other.
const ITEMS: [&str; 2] = [
    "org.kde.StatusNotifierItem",
    "org.freedesktop.StatusNotifierItem",
];
const MENU: &str = "com.canonical.dbusmenu";

/// Whether an item wants to be looked at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrayStatus {
    /// Nothing much to tell: a tray may hide it.
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

/// An icon: a name in the icon theme, pictures, or both.
#[derive(Clone, Debug, Default)]
pub struct TrayIcon {
    pub name: Option<String>,
    /// Where else to look for `name`, for programs that bring their own icons.
    pub theme_path: Option<String>,
    /// The same picture at each size there is, smallest first.
    pub pixmaps: Vec<Arc<Bitmap>>,
}

impl PartialEq for TrayIcon {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.theme_path == other.theme_path
            && same_bitmaps(&self.pixmaps, &other.pixmaps)
    }
}

fn same_bitmaps(a: &[Arc<Bitmap>], b: &[Arc<Bitmap>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| Arc::ptr_eq(a, b))
}

impl TrayIcon {
    /// The picture for a box `size` pixels across: the smallest that is as big, or else the
    /// largest.
    pub fn best(&self, size: u32) -> Option<Arc<Bitmap>> {
        self.pixmaps
            .iter()
            .find(|b| b.width().max(b.height()) >= size)
            .or(self.pixmaps.last())
            .cloned()
    }

    fn is_empty(&self) -> bool {
        self.name.is_none() && self.pixmaps.is_empty()
    }
}

/// What shows when the pointer rests on an item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayTooltip {
    pub title: String,
    /// May have some of the markup of notifications in it.
    pub body: String,
    pub icon: Option<TrayIcon>,
}

/// An item of the tray, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayItem {
    pub id: String,
    pub title: String,
    pub status: TrayStatus,
    pub icon: TrayIcon,
    /// What to show instead of `icon` while it needs attention.
    pub attention_icon: Option<TrayIcon>,
    /// What to draw on top of `icon`.
    pub overlay_icon: Option<TrayIcon>,
    pub tooltip: Option<TrayTooltip>,
    /// It only has a menu: a click should open that rather than activate it.
    pub item_is_menu: bool,
    pub has_menu: bool,
    /// The unique name of its program on the bus.
    service: String,
    path: String,
    interface: &'static str,
    menu_path: Option<String>,
}

/// Checked or chosen, for an entry that can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuToggle {
    Check(bool),
    Radio(bool),
}

/// An entry of an item's menu.
#[derive(Clone, Debug, Default)]
pub struct TrayMenuItem {
    /// What [`TrayItem::menu_clicked`] takes.
    pub id: i32,
    /// Without the underscores that mark the access key.
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub separator: bool,
    pub toggle: Option<MenuToggle>,
    pub icon_name: Option<String>,
    /// Not read yet: the menu gives it as PNG.
    pub icon: Option<Arc<Bitmap>>,
    /// Like `Control+Q`.
    pub shortcut: Option<String>,
    /// A submenu, if it has entries.
    pub children: Vec<TrayMenuItem>,
}

impl PartialEq for TrayMenuItem {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.enabled == other.enabled
            && self.visible == other.visible
            && self.separator == other.separator
            && self.toggle == other.toggle
            && self.icon_name == other.icon_name
            && self.icon.as_ref().map(Arc::as_ptr) == other.icon.as_ref().map(Arc::as_ptr)
            && self.shortcut == other.shortcut
            && self.children == other.children
    }
}

/// The tray. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Tray(&'static Shared);

#[derive(Debug)]
struct Shared {
    available: SharedSignal<bool>,
    items: SharedSignal<Vec<TrayItem>>,
    /// The menus that have been read, by item, kept up to date from then on.
    menus: Mutex<HashMap<String, Menu>>,
}

#[derive(Clone, Debug)]
struct Menu {
    service: String,
    path: String,
    entries: SharedSignal<Vec<TrayMenuItem>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The tray. It starts following it the first time.
pub fn tray() -> Tray {
    Tray(dbus::init_then_follow(&SHARED, follow, || {
        Shared {
            available: shared_signal(false),
            items: shared_signal(Vec::new()),
            menus: Mutex::new(HashMap::new()),
        }
    }))
}

impl Tray {
    /// Whether there is a watcher, someone else's or this one.
    pub fn available(&self) -> bool {
        self.0.available.get()
    }

    /// The items, in the order they came.
    pub fn items(&self) -> Vec<TrayItem> {
        self.0.items.get()
    }
}

impl TrayItem {
    /// The same while the item is there.
    pub fn key(&self) -> String {
        format!("{}{}", self.service, self.path)
    }

    fn ask(&self, method: &'static str, args: (i32, i32)) -> Task<Result<(), ServiceError>> {
        let (service, path, interface) = (self.service.clone(), self.path.clone(), self.interface);
        dbus::run(async move {
            let conn = dbus::session().await?;
            dbus::call(&conn, &service, &path, interface, method, &args).await
        })
    }

    /// What a left click does; `x` and `y` are where on the screen, which few use.
    pub fn activate(&self, x: i32, y: i32) -> Task<Result<(), ServiceError>> {
        self.ask("Activate", (x, y))
    }

    /// What a middle click does.
    pub fn secondary_activate(&self, x: i32, y: i32) -> Task<Result<(), ServiceError>> {
        self.ask("SecondaryActivate", (x, y))
    }

    /// Asks the program to show its own menu, for items without a dbusmenu.
    pub fn context_menu(&self, x: i32, y: i32) -> Task<Result<(), ServiceError>> {
        self.ask("ContextMenu", (x, y))
    }

    /// The wheel turned over it.
    pub fn scroll(&self, delta: i32, vertical: bool) -> Task<Result<(), ServiceError>> {
        let (service, path, interface) = (self.service.clone(), self.path.clone(), self.interface);
        dbus::run(async move {
            let conn = dbus::session().await?;
            let orientation = if vertical { "vertical" } else { "horizontal" };
            dbus::call(
                &conn,
                &service,
                &path,
                interface,
                "Scroll",
                &(delta, orientation),
            )
            .await
        })
    }

    /// Its menu, read the first time this is called and followed from then on. Empty until it
    /// has been read, and for items without one.
    pub fn menu(&self) -> Vec<TrayMenuItem> {
        let (Some(menu_path), Some(shared)) = (&self.menu_path, SHARED.get()) else {
            return Vec::new();
        };
        let entries = {
            let Ok(mut menus) = shared.menus.lock() else {
                return Vec::new();
            };
            let menu = menus.entry(self.key()).or_insert_with(|| {
                let menu = Menu {
                    service: self.service.clone(),
                    path: menu_path.clone(),
                    entries: shared_signal(Vec::new()),
                };
                dbus::spawn(fetch_menu(menu.clone()));
                menu
            });
            menu.entries.clone()
        };
        entries.get()
    }

    /// An entry of its menu was chosen.
    pub fn menu_clicked(&self, id: i32) -> Task<Result<(), ServiceError>> {
        let (service, path) = (self.service.clone(), self.menu_path.clone());
        dbus::run(async move {
            let path = path.ok_or(ServiceError::Unsupported)?;
            let conn = dbus::session().await?;
            let event = (id, "clicked", Value::from(0i32), 0u32);
            dbus::call(&conn, &service, &path, MENU, "Event", &event).await
        })
    }

    /// A submenu is about to open: some programs fill it only then.
    pub fn menu_about_to_show(&self, id: i32) -> Task<Result<(), ServiceError>> {
        let menu = SHARED
            .get()
            .and_then(|s| s.menus.lock().ok()?.get(&self.key()).cloned());
        let (service, path) = (self.service.clone(), self.menu_path.clone());
        dbus::run(async move {
            let path = path.ok_or(ServiceError::Unsupported)?;
            let conn = dbus::session().await?;
            let changed: bool = dbus::call(&conn, &service, &path, MENU, "AboutToShow", &(id,))
                .await
                .unwrap_or(false);
            if changed && let Some(menu) = menu {
                fetch_menu(menu).await;
            }
            Ok(())
        })
    }
}

/// Reads a menu as it is now.
async fn fetch_menu(menu: Menu) {
    let Ok(conn) = dbus::session().await else {
        return;
    };
    let _: Result<bool, _> = dbus::call(
        &conn,
        &menu.service,
        &menu.path,
        MENU,
        "AboutToShow",
        &(0i32,),
    )
    .await;
    let layout: Result<(u32, Layout), _> = dbus::call(
        &conn,
        &menu.service,
        &menu.path,
        MENU,
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()),
    )
    .await;
    if let Ok((_, (_, _, children))) = layout {
        put(&menu.entries, entries(children));
    }
}

/// An entry of a dbusmenu layout: its id, properties and children.
type Layout = (i32, Props, Vec<OwnedValue>);

fn entries(children: Vec<OwnedValue>) -> Vec<TrayMenuItem> {
    children
        .into_iter()
        .filter_map(|v| Layout::try_from(v).ok())
        .map(|(id, p, children)| {
            let on = get::<i32>(&p, "toggle-state") == Some(1);
            let toggle = match string(&p, "toggle-type").as_str() {
                "checkmark" => Some(MenuToggle::Check(on)),
                "radio" => Some(MenuToggle::Radio(on)),
                _ => None,
            };
            let shortcut: Vec<Vec<String>> = get(&p, "shortcut").unwrap_or_default();
            TrayMenuItem {
                id,
                label: without_mnemonic(&string(&p, "label")),
                enabled: get(&p, "enabled").unwrap_or(true),
                visible: get(&p, "visible").unwrap_or(true),
                separator: string(&p, "type") == "separator",
                toggle,
                icon_name: Some(string(&p, "icon-name")).filter(|n| !n.is_empty()),
                icon: None,
                shortcut: Some(
                    shortcut
                        .iter()
                        .map(|keys| keys.join("+"))
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .filter(|s| !s.is_empty()),
                children: entries(children),
            }
        })
        .collect()
}

/// `_File` is `File`, and `__` is `_`.
fn without_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c != '_' {
            out.push(c);
        } else if let Some(next) = chars.next() {
            out.push(next);
        }
    }
    out
}

/// Pictures as items give them: width, height and ARGB in network order.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

fn bitmaps(pixmaps: Pixmaps) -> Vec<Arc<Bitmap>> {
    let mut out: Vec<Arc<Bitmap>> = pixmaps
        .into_iter()
        .filter_map(|(w, h, argb)| {
            let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
            let n = w as usize * h as usize;
            if n == 0 || argb.len() < n * 4 {
                return None;
            }
            let mut rgba = Vec::with_capacity(n * 4);
            let mut opaque = true;
            for px in argb[..n * 4].chunks_exact(4) {
                let a = px[0] as u32;
                opaque &= a == 255;
                let pre = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                rgba.extend_from_slice(&[pre(px[1]), pre(px[2]), pre(px[3]), a as u8]);
            }
            Some(Arc::new(Bitmap::from_premultiplied(w, h, rgba, opaque)))
        })
        .collect();
    out.sort_by_key(|b| b.width() * b.height());
    out
}

/// An item as this keeps it: what it was registered as, where it is and what it has said.
struct Entry {
    registered: String,
    item: TrayItem,
    props: Props,
}

/// Where an item is, from what it was registered as: a bus name, perhaps followed by a path.
fn place(registered: &str) -> (String, String) {
    match registered.find('/') {
        Some(at) => (registered[..at].to_owned(), registered[at..].to_owned()),
        None => (registered.to_owned(), "/StatusNotifierItem".to_owned()),
    }
}

/// Makes an icon of three properties, keeping the pictures it had if they are the same.
fn icon(p: &Props, old: &Props, old_icon: Option<&TrayIcon>, name: &str, pixmap: &str) -> TrayIcon {
    let pixmaps = match (old_icon, p.get(pixmap)) {
        (Some(i), Some(v)) if old.get(pixmap) == Some(v) => i.pixmaps.clone(),
        _ => bitmaps(get(p, pixmap).unwrap_or_default()),
    };
    TrayIcon {
        name: Some(string(p, name)).filter(|n| !n.is_empty()),
        theme_path: Some(string(p, "IconThemePath")).filter(|n| !n.is_empty()),
        pixmaps,
    }
}

fn item(
    service: &str,
    path: &str,
    interface: &'static str,
    p: &Props,
    old: Option<&Entry>,
) -> TrayItem {
    let empty = Props::new();
    let old_props = old.map_or(&empty, |e| &e.props);
    let old_item = old.map(|e| &e.item);
    let some = |i: TrayIcon| Some(i).filter(|i| !i.is_empty());
    let tooltip = get::<(String, Pixmaps, String, String)>(p, "ToolTip").and_then(
        |(name, pixmaps, title, body)| {
            let same = old_props.get("ToolTip") == p.get("ToolTip");
            let icon = match old_item.and_then(|i| i.tooltip.as_ref()) {
                Some(t) if same => t.icon.clone(),
                _ => some(TrayIcon {
                    name: Some(name).filter(|n| !n.is_empty()),
                    theme_path: None,
                    pixmaps: bitmaps(pixmaps),
                }),
            };
            (!title.is_empty() || !body.is_empty()).then_some(TrayTooltip { title, body, icon })
        },
    );
    let menu_path = get::<OwnedObjectPath>(p, "Menu")
        .map(|m| m.to_string())
        .filter(|m| !m.is_empty() && m != "/");
    TrayItem {
        id: string(p, "Id"),
        title: string(p, "Title"),
        status: match string(p, "Status").as_str() {
            "Passive" => TrayStatus::Passive,
            "NeedsAttention" => TrayStatus::NeedsAttention,
            _ => TrayStatus::Active,
        },
        icon: icon(
            p,
            old_props,
            old_item.map(|i| &i.icon),
            "IconName",
            "IconPixmap",
        ),
        attention_icon: some(icon(
            p,
            old_props,
            old_item.and_then(|i| i.attention_icon.as_ref()),
            "AttentionIconName",
            "AttentionIconPixmap",
        )),
        overlay_icon: some(icon(
            p,
            old_props,
            old_item.and_then(|i| i.overlay_icon.as_ref()),
            "OverlayIconName",
            "OverlayIconPixmap",
        )),
        tooltip,
        item_is_menu: get(p, "ItemIsMenu").unwrap_or(false),
        has_menu: menu_path.is_some(),
        service: service.to_owned(),
        path: path.to_owned(),
        interface,
        menu_path,
    }
}

/// The watcher this is when there is no other.
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        // Programs that give only a path are where the call came from.
        let registered = if service.starts_with('/') {
            let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
            format!("{sender}{service}")
        } else {
            service.to_owned()
        };
        let new = match self.items.lock() {
            Ok(mut items) if !items.contains(&registered) => {
                items.push(registered.clone());
                true
            }
            _ => false,
        };
        if new {
            let _ = Self::status_notifier_item_registered(&emitter, &registered).await;
        }
    }

    async fn register_status_notifier_host(
        &self,
        _service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let _ = Self::status_notifier_host_registered(&emitter).await;
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().map(|i| i.clone()).unwrap_or_default()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

async fn stream(
    conn: &Connection,
    rule: zbus::Result<MatchRule<'static>>,
) -> Option<MessageStream> {
    MessageStream::for_match_rule(rule.ok()?, conn, Some(1024))
        .await
        .ok()
}

fn signals_of(interface: &'static str) -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(interface)?
        .build())
}

fn owner_changes() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .build())
}

async fn owner(conn: &Connection, name: &str) -> Option<String> {
    if name.starts_with(':') {
        return Some(name.to_owned());
    }
    dbus::call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetNameOwner",
        &(name,),
    )
    .await
    .ok()
}

/// The items there are and what to tell.
struct Items {
    conn: Connection,
    list: Vec<Entry>,
}

impl Items {
    fn publish(&self) {
        let Some(s) = SHARED.get() else {
            return;
        };
        put(&s.items, self.list.iter().map(|e| e.item.clone()).collect());
        if let Ok(mut menus) = s.menus.lock() {
            menus.retain(|key, _| self.list.iter().any(|e| e.item.key() == *key));
        }
    }

    async fn read(&self, service: &str, path: &str) -> Option<(&'static str, Props)> {
        for interface in ITEMS {
            if let Some(p) = dbus::get_all(&self.conn, service, path, interface).await {
                return Some((interface, p));
            }
        }
        None
    }

    async fn add(&mut self, registered: &str) {
        if self.list.iter().any(|e| e.registered == registered) {
            return;
        }
        let (name, path) = place(registered);
        let Some(service) = owner(&self.conn, &name).await else {
            return;
        };
        let Some((interface, props)) = self.read(&service, &path).await else {
            return;
        };
        let item = item(&service, &path, interface, &props, None);
        self.list.push(Entry {
            registered: registered.to_owned(),
            item,
            props,
        });
        self.publish();
    }

    fn remove(&mut self, registered: &str) {
        let before = self.list.len();
        self.list.retain(|e| e.registered != registered);
        if self.list.len() != before {
            self.publish();
        }
    }

    /// An item said something of itself changed.
    async fn changed(&mut self, sender: &str, path: &str, member: &str, msg: &Message) {
        let Some(at) = self
            .list
            .iter()
            .position(|e| e.item.service == sender && e.item.path == path)
        else {
            return;
        };
        let props = if member == "NewStatus" {
            let Ok(status) = msg.body().deserialize::<String>() else {
                return;
            };
            let mut props = self.list[at].props.clone();
            props.insert(
                "Status".into(),
                Value::from(status).try_into().expect("a string"),
            );
            props
        } else {
            match self.read(sender, path).await {
                Some((_, p)) => p,
                None => return,
            }
        };
        let e = &self.list[at];
        let item = item(sender, path, e.item.interface, &props, Some(e));
        let e = &mut self.list[at];
        e.item = item;
        e.props = props;
        self.publish();
    }
}

async fn follow() {
    let Ok(conn) = dbus::session().await else {
        return;
    };
    let host = format!("org.kde.StatusNotifierHost-{}-1", std::process::id());
    let _ = conn.request_name(host.as_str()).await;
    let ours = Arc::new(Mutex::new(Vec::new()));
    let _ = conn
        .object_server()
        .at(
            WATCHER_PATH,
            Watcher {
                items: ours.clone(),
            },
        )
        .await;
    // Signals first, so that nothing is missed between reading and following.
    let (Some(watcher), Some(kde), Some(freedesktop), Some(menus), Some(owners)) = (
        stream(&conn, signals_of(WATCHER)).await,
        stream(&conn, signals_of(ITEMS[0])).await,
        stream(&conn, signals_of(ITEMS[1])).await,
        stream(&conn, signals_of(MENU)).await,
        stream(&conn, owner_changes()).await,
    ) else {
        return;
    };
    let mut signals = owners.or(watcher).or(kde).or(freedesktop).or(menus);
    loop {
        let is_ours = matches!(
            conn.request_name_with_flags(WATCHER, RequestNameFlags::DoNotQueue.into())
                .await,
            Ok(zbus::fdo::RequestNameReply::PrimaryOwner
                | zbus::fdo::RequestNameReply::AlreadyOwner)
        );
        let Some(watcher_owner) = owner(&conn, WATCHER).await else {
            continue;
        };
        let mut items = Items {
            conn: conn.clone(),
            list: Vec::new(),
        };
        if !is_ours {
            let _: Result<(), _> = dbus::call(
                &conn,
                WATCHER,
                WATCHER_PATH,
                WATCHER,
                "RegisterStatusNotifierHost",
                &(host.as_str(),),
            )
            .await;
            let props = dbus::get_all(&conn, WATCHER, WATCHER_PATH, WATCHER)
                .await
                .unwrap_or_default();
            let registered: Vec<String> =
                get(&props, "RegisteredStatusNotifierItems").unwrap_or_default();
            for r in registered {
                items.add(&r).await;
            }
        }
        if let Some(s) = SHARED.get() {
            put(&s.available, true);
        }
        items.publish();
        while let Some(Ok(msg)) = signals.next().await {
            let header = msg.header();
            let (Some(sender), Some(path), Some(interface), Some(member)) = (
                header.sender().map(|s| s.to_string()),
                header.path().map(|p| p.to_string()),
                header.interface().map(|i| i.to_string()),
                header.member().map(|m| m.to_string()),
            ) else {
                continue;
            };
            match interface.as_str() {
                "org.freedesktop.DBus" => {
                    let Ok((name, _, new)) = msg.body().deserialize::<(String, String, String)>()
                    else {
                        continue;
                    };
                    if !new.is_empty() {
                        continue;
                    }
                    if name == WATCHER && !is_ours {
                        break;
                    }
                    let gone: Vec<String> = items
                        .list
                        .iter()
                        .filter(|e| e.item.service == name)
                        .map(|e| e.registered.clone())
                        .collect();
                    for registered in gone {
                        items.remove(&registered);
                        if is_ours {
                            if let Ok(mut o) = ours.lock() {
                                o.retain(|r| *r != registered);
                            }
                            if let Ok(emitter) = SignalEmitter::new(&conn, WATCHER_PATH) {
                                let _ = Watcher::status_notifier_item_unregistered(
                                    &emitter,
                                    &registered,
                                )
                                .await;
                            }
                        }
                    }
                }
                WATCHER if sender == watcher_owner => {
                    let Ok(registered) = msg.body().deserialize::<String>() else {
                        continue;
                    };
                    match member.as_str() {
                        "StatusNotifierItemRegistered" => items.add(&registered).await,
                        "StatusNotifierItemUnregistered" => items.remove(&registered),
                        _ => {}
                    }
                }
                MENU if member == "LayoutUpdated" || member == "ItemsPropertiesUpdated" => {
                    let stale: Vec<Menu> = SHARED
                        .get()
                        .and_then(|s| s.menus.lock().ok())
                        .map(|m| {
                            m.values()
                                .filter(|m| m.service == sender && m.path == path)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    for menu in stale {
                        dbus::spawn(fetch_menu(menu));
                    }
                }
                i if ITEMS.contains(&i) => items.changed(&sender, &path, &member, &msg).await,
                _ => {}
            }
        }
        if let Some(s) = SHARED.get() {
            put(&s.available, false);
            put(&s.items, Vec::new());
        }
        if is_ours {
            return;
        }
    }
}

/// The item's menu as the standard menu of `anti-webui-widgets`, open while `open` is true and
/// closing it when something is chosen or the pointer goes elsewhere. It follows the item's menu
/// as that changes.
pub fn tray_menu(item: &TrayItem, open: Signal<bool>) -> View {
    let item = item.clone();
    dyn_view(move || {
        if open.get() {
            // Programs fill some menus only when told they are about to be shown.
            item.menu_about_to_show(0);
        }
        let entries = menu_entries(&item, &item.menu());
        menu(move || open.get(), move || open.set(false), entries)
    })
}

fn menu_entries(item: &TrayItem, list: &[TrayMenuItem]) -> Vec<MenuEntry> {
    list.iter()
        .filter(|m| m.visible)
        .map(|m| {
            if m.separator {
                return menu_separator();
            }
            if !m.children.is_empty() {
                return submenu(m.label.clone(), menu_entries(item, &m.children));
            }
            let mut row = menu_item(m.label.clone());
            if let Some(MenuToggle::Check(on) | MenuToggle::Radio(on)) = m.toggle {
                row = row.checked_when(move || on);
            }
            if !m.enabled {
                row = row.disabled_when(|| true);
            }
            if let Some(keys) = &m.shortcut {
                row = row.shortcut(keys.clone());
            }
            let (item, id) = (item.clone(), m.id);
            row.on_click(move || {
                item.menu_clicked(id);
            })
            .into()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_lose_their_access_keys() {
        assert_eq!(without_mnemonic("_File"), "File");
        assert_eq!(without_mnemonic("Save __as"), "Save _as");
        assert_eq!(without_mnemonic("Quit_"), "Quit");
    }

    #[test]
    fn items_are_found_by_what_they_registered_as() {
        assert_eq!(
            place(":1.196/org/ayatana/NotificationItem/discord"),
            (
                ":1.196".into(),
                "/org/ayatana/NotificationItem/discord".into()
            )
        );
        assert_eq!(
            place("org.kde.StatusNotifierItem-170870-1"),
            (
                "org.kde.StatusNotifierItem-170870-1".into(),
                "/StatusNotifierItem".into()
            )
        );
    }

    #[test]
    fn pixmaps_become_premultiplied_rgba_smallest_first() {
        let big = (2, 2, [255, 1, 2, 3].repeat(4));
        let small = (1, 1, vec![128, 255, 0, 0]);
        let b = bitmaps(vec![big, small]);
        assert_eq!((b[0].width(), b[1].width()), (1, 2));
        assert!(!b[0].is_opaque() && b[1].is_opaque());
        let mut px = Vec::new();
        b[0].with_levels(|_, _, p| px = p.to_vec());
        assert_eq!(px, [128, 0, 0, 128]);
        let icon = TrayIcon {
            pixmaps: b,
            ..Default::default()
        };
        assert_eq!(icon.best(2).unwrap().width(), 2);
        assert_eq!(icon.best(64).unwrap().width(), 2);
        assert_eq!(icon.best(1).unwrap().width(), 1);
    }
}
