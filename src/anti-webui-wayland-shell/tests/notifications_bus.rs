//! Being the notification daemon, on a bus of the test's own with a program sending notifications.
//! Run with `--features notifications`.

#![cfg(target_os = "linux")]
#![cfg(feature = "notifications")]

mod common;

use anti_webui_wayland_shell::{Urgency, notification_server};
use common::until;
use futures_lite::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::zvariant::Value;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// Sends a notification as a program would, and gives its id.
fn notify(
    client: &zbus::Connection,
    replaces: u32,
    summary: &str,
    body: &str,
    actions: &[&str],
    hints: HashMap<&str, Value<'_>>,
    timeout: i32,
) -> u32 {
    zbus::block_on(async {
        client
            .call_method(
                Some(NAME),
                PATH,
                Some(NAME),
                "Notify",
                &(
                    "app", replaces, "icon", summary, body, actions, hints, timeout,
                ),
            )
            .await
            .unwrap()
            .body()
            .deserialize()
            .unwrap()
    })
}

#[test]
fn notifications_are_kept_shown_and_answered() {
    let _bus = common::private_bus();
    let server = notification_server().expect("the name on a bus of our own");
    assert!(notification_server().is_ok(), "the same server again");
    let client = common::connect(None);

    // What the program hears back.
    let heard: Arc<Mutex<Vec<(String, u32, String)>>> = Arc::default();
    let h = heard.clone();
    let listen = client.clone();
    let (ready_tx, ready) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        zbus::block_on(async move {
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(NAME)
                .unwrap()
                .build();
            let mut stream = zbus::MessageStream::for_match_rule(rule, &listen, None)
                .await
                .unwrap();
            ready_tx.send(()).unwrap();
            while let Some(Ok(msg)) = stream.next().await {
                let member = msg.header().member().unwrap().to_string();
                let entry = if member == "ActionInvoked" {
                    let (id, key): (u32, String) = msg.body().deserialize().unwrap();
                    (member, id, key)
                } else {
                    let (id, reason): (u32, u32) = msg.body().deserialize().unwrap();
                    (member, id, reason.to_string())
                };
                h.lock().unwrap().push(entry);
            }
        })
    });
    ready.recv().unwrap();
    let heard_has = |e: (&str, u32, &str)| {
        heard
            .lock()
            .unwrap()
            .iter()
            .any(|(m, i, a)| (m.as_str(), *i, a.as_str()) == e)
    };

    let info: (String, String, String, String) = zbus::block_on(async {
        client
            .call_method(Some(NAME), PATH, Some(NAME), "GetServerInformation", &())
            .await
            .unwrap()
            .body()
            .deserialize()
            .unwrap()
    });
    assert_eq!((info.0.as_str(), info.3.as_str()), ("anti-webui", "1.2"));

    // One with actions and a picture, never expiring.
    let mut hints = HashMap::new();
    hints.insert("urgency", Value::from(2u8));
    hints.insert(
        "image-data",
        Value::from((
            2i32,
            1i32,
            6i32,
            false,
            8i32,
            3i32,
            vec![1u8, 2, 3, 4, 5, 6],
        )),
    );
    let first = notify(
        &client,
        0,
        "Hello",
        "<b>bold</b> <font>plain</font> &amp; more",
        &["default", "Open", "later", "Later"],
        hints,
        0,
    );
    until("the notification", || server.notifications().len() == 1);
    let n = &server.notifications()[0];
    assert_eq!((n.id, n.summary.as_str()), (first, "Hello"));
    assert_eq!(n.body, "<b>bold</b> plain &amp; more");
    assert_eq!(n.body_text(), "bold plain & more");
    assert_eq!(n.urgency, Urgency::Critical);
    assert_eq!(n.expires, None);
    assert_eq!(
        n.actions,
        [
            ("default".to_owned(), "Open".to_owned()),
            ("later".to_owned(), "Later".to_owned())
        ]
    );
    let image = n.image.as_ref().expect("the picture");
    assert_eq!((image.width(), image.height()), (2, 1));
    assert_eq!(server.popups().len(), 1);

    // Clicking it tells the program, and closes it.
    n.invoke("default");
    until("the action", || {
        heard_has(("ActionInvoked", first, "default"))
    });
    until("the close", || {
        heard_has(("NotificationClosed", first, "2"))
    });
    assert!(server.notifications().is_empty() && server.popups().is_empty());

    // One that expires leaves the screen but stays kept.
    let second = notify(&client, 0, "Soon gone", "", &[], HashMap::new(), 150);
    until("it to show", || server.popups().len() == 1);
    until("it to expire", || server.popups().is_empty());
    until("the expiry", || {
        heard_has(("NotificationClosed", second, "1"))
    });
    assert_eq!(server.notifications()[0].id, second);

    // Replacing one keeps its id, and the old one's time doesn't count any more.
    let third = notify(&client, 0, "Short", "", &[], HashMap::new(), 200);
    assert_eq!(
        notify(&client, third, "Long", "", &[], HashMap::new(), 0),
        third
    );
    until("the replacement", || {
        server.notifications()[0].summary == "Long"
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(server.popups().iter().any(|n| n.id == third));
    assert_eq!(server.notifications().len(), 2);

    // The program closes it itself.
    zbus::block_on(async {
        client
            .call_method(Some(NAME), PATH, Some(NAME), "CloseNotification", &(third,))
            .await
            .unwrap();
    });
    until("the program's close", || {
        heard_has(("NotificationClosed", third, "3"))
    });
    assert!(server.notifications().iter().all(|n| n.id != third));

    // Not to be disturbed: kept, not shown, unless critical.
    server.set_do_not_disturb(true);
    notify(&client, 0, "Quiet", "", &[], HashMap::new(), 0);
    let mut critical = HashMap::new();
    critical.insert("urgency", Value::from(2u8));
    let loud = notify(&client, 0, "Loud", "", &[], critical, -1);
    until("both", || server.notifications().len() == 3);
    let popups: Vec<u32> = server.popups().iter().map(|n| n.id).collect();
    assert_eq!(popups, [loud]);
    assert_eq!(server.notifications()[0].expires, None, "critical stays");

    server.clear();
    assert!(server.notifications().is_empty() && server.popups().is_empty());
}
