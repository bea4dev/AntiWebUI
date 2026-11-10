//! What the D-Bus services share (see "共通の作り" in `docs/design/37-wayland-shell.md`): the two
//! buses, one thread nobody sees that runs all that is asked of them, and reading properties.

#![allow(dead_code, reason = "each service uses some of this")]

use crate::ServiceError;
use anti_webui_core::task::Task;
use futures_lite::StreamExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use zbus::fdo::DBusProxy;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, Message, MessageStream};

/// Properties of an object by name, as `GetAll` and `PropertiesChanged` give them.
pub(crate) type Props = HashMap<String, OwnedValue>;

static EXECUTOR: OnceLock<Arc<async_executor::Executor<'static>>> = OnceLock::new();

fn executor() -> &'static async_executor::Executor<'static> {
    EXECUTOR.get_or_init(|| {
        let executor = Arc::new(async_executor::Executor::new());
        let run = executor.clone();
        std::thread::Builder::new()
            .name("anti-webui dbus".into())
            .spawn(move || zbus::block_on(run.run(futures_lite::future::pending::<()>())))
            .expect("a thread for D-Bus");
        executor
    })
}

/// Runs `f` on the D-Bus thread.
pub(crate) fn spawn(f: impl Future<Output = ()> + Send + 'static) {
    executor().spawn(f).detach();
}

/// A service's shared state, made the first time it is asked for, and `follow` started on the
/// D-Bus thread once the state is in `cell` — not before, or it could find `cell` empty.
pub(crate) fn init_then_follow<T: Send + Sync, F: Future<Output = ()> + Send + 'static>(
    cell: &'static std::sync::OnceLock<T>,
    follow: impl FnOnce() -> F,
    make: impl FnOnce() -> T,
) -> &'static T {
    let mut made = false;
    let shared = cell.get_or_init(|| {
        made = true;
        make()
    });
    if made {
        spawn(follow());
    }
    shared
}

/// Runs `f` on the D-Bus thread, its result coming back to this one.
pub(crate) fn run<T: Send + 'static>(
    f: impl Future<Output = Result<T, ServiceError>> + Send + 'static,
) -> Task<Result<T, ServiceError>> {
    let (task, done) = Task::pending();
    spawn(async move { done.complete(f.await) });
    task
}

static SYSTEM: async_lock::OnceCell<Option<Connection>> = async_lock::OnceCell::new();
static SESSION: async_lock::OnceCell<Option<Connection>> = async_lock::OnceCell::new();

/// The system bus, where NetworkManager, UPower, BlueZ and logind are. `DBUS_SYSTEM_BUS_ADDRESS`
/// puts it elsewhere, as tests do.
pub(crate) async fn system() -> Result<Connection, ServiceError> {
    SYSTEM
        .get_or_init(|| connect(zbus::connection::Builder::system(), "system"))
        .await
        .clone()
        .ok_or(ServiceError::Unavailable)
}

/// The session bus, where players, notifications and the tray are.
pub(crate) async fn session() -> Result<Connection, ServiceError> {
    SESSION
        .get_or_init(|| connect(zbus::connection::Builder::session(), "session"))
        .await
        .clone()
        .ok_or(ServiceError::Unavailable)
}

/// Connects, with what reads from the bus running on the D-Bus thread rather than a thread of
/// zbus's own.
async fn connect(
    builder: zbus::Result<zbus::connection::Builder<'static>>,
    which: &str,
) -> Option<Connection> {
    let built = match builder {
        Ok(b) => b.internal_executor(false).build().await,
        Err(e) => Err(e),
    };
    let conn = built
        .map_err(|e| eprintln!("[anti-webui] no {which} bus: {e}"))
        .ok()?;
    let ticking = conn.clone();
    spawn(async move {
        loop {
            ticking.executor().tick().await;
        }
    });
    Some(conn)
}

/// What went wrong, as a service tells it.
pub(crate) fn failed(e: impl Into<zbus::Error>) -> ServiceError {
    match e.into() {
        zbus::Error::MethodError(name, message, _) => match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => ServiceError::Unavailable,
            "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.NotSupported" => ServiceError::Unsupported,
            _ => ServiceError::Failed(message.unwrap_or_else(|| name.to_string())),
        },
        zbus::Error::NameTaken => ServiceError::NameTaken,
        e => ServiceError::Failed(e.to_string()),
    }
}

/// Calls a method and reads what comes back.
pub(crate) async fn call<B, R>(
    conn: &Connection,
    dest: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Result<R, ServiceError>
where
    B: zbus::export::serde::ser::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let reply = conn
        .call_method(Some(dest), path, Some(interface), method, body)
        .await
        .map_err(failed)?;
    reply.body().deserialize().map_err(failed)
}

/// All the properties of an interface of an object.
pub(crate) async fn get_all(
    conn: &Connection,
    dest: &str,
    path: &str,
    interface: &str,
) -> Option<Props> {
    call(
        conn,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &(interface,),
    )
    .await
    .ok()
}

/// Sets a property.
pub(crate) async fn set_property(
    conn: &Connection,
    dest: &str,
    path: &str,
    interface: &str,
    name: &str,
    value: zbus::zvariant::Value<'_>,
) -> Result<(), ServiceError> {
    call(
        conn,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "Set",
        &(interface, name, value),
    )
    .await
}

/// A property as a type, if it is there and of that type.
pub(crate) fn get<T: TryFrom<OwnedValue>>(props: &Props, name: &str) -> Option<T> {
    T::try_from(props.get(name)?.clone()).ok()
}

pub(crate) fn string(props: &Props, name: &str) -> String {
    get(props, name).unwrap_or_default()
}

/// Signals sent from objects under `path`.
pub(crate) async fn signals(
    conn: &Connection,
    path: &'static str,
    interface: Option<&'static str>,
) -> Option<MessageStream> {
    let mut rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path_namespace(path)
        .ok()?;
    if let Some(interface) = interface {
        rule = rule.interface(interface).ok()?;
    }
    MessageStream::for_match_rule(rule.build(), conn, Some(1024))
        .await
        .ok()
}

/// `PropertiesChanged`: the interface, what changed and what is to be asked for again.
pub(crate) fn properties_changed(msg: &Message) -> Option<(String, Props, Vec<String>)> {
    let header = msg.header();
    if header.interface()?.as_str() != "org.freedesktop.DBus.Properties"
        || header.member()?.as_str() != "PropertiesChanged"
    {
        return None;
    }
    msg.body().deserialize().ok()
}

/// The path and member of a signal.
pub(crate) fn signal_of(msg: &Message) -> Option<(String, String, String)> {
    let header = msg.header();
    Some((
        header.path()?.to_string(),
        header.interface()?.to_string(),
        header.member()?.to_string(),
    ))
}

/// Runs `serve` whenever `name` has an owner, stopping it when the name loses its owner and
/// calling `gone`. A service that starts later is served from then on.
pub(crate) async fn while_owned<F, Fut>(
    conn: &Connection,
    name: &'static str,
    mut serve: F,
    gone: impl Fn(),
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let Ok(dbus) = DBusProxy::new(conn).await else {
        return;
    };
    let Ok(mut changes) = dbus
        .receive_name_owner_changed_with_args(&[(0, name)])
        .await
    else {
        return;
    };
    let owned = |c: &zbus::fdo::NameOwnerChanged| {
        c.args().map(|a| a.new_owner().is_some()).unwrap_or(false)
    };
    let mut has_owner = match name.try_into() {
        Ok(n) => dbus.name_has_owner(n).await.unwrap_or(false),
        Err(_) => return,
    };
    loop {
        if has_owner {
            let changes = &mut changes;
            let ended_by_owner = futures_lite::future::or(
                async {
                    serve().await;
                    false
                },
                async {
                    while let Some(c) = changes.next().await {
                        if !owned(&c) {
                            return true;
                        }
                    }
                    true
                },
            )
            .await;
            gone();
            if ended_by_owner {
                has_owner = false;
                continue;
            }
        }
        // Until it has an owner again.
        loop {
            match changes.next().await {
                Some(c) if owned(&c) => break,
                Some(_) => {}
                None => return,
            }
        }
        has_owner = true;
    }
}

/// Sets a signal only when the value is another, so that what reads it runs only then.
pub(crate) fn put<T: PartialEq + Send + 'static>(
    signal: &anti_webui_core::shared::SharedSignal<T>,
    value: T,
) {
    if signal.with_untracked(|v| *v != value) {
        signal.set(value);
    }
}
