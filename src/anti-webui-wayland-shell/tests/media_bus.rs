//! MPRIS players, against fakes on a bus of the test's own. Run with `--features media`.
#![cfg(feature = "media")]

mod common;

use anti_webui_wayland_shell::{PlaybackStatus, media};
use common::until;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";

struct Root(&'static str);

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(property)]
    fn identity(&self) -> String {
        self.0.into()
    }
}

struct Fake {
    status: String,
    title: String,
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Fake {
    async fn play_pause(&mut self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.calls.lock().unwrap().push("PlayPause".into());
        self.status = if self.status == "Playing" {
            "Paused".into()
        } else {
            "Playing".into()
        };
        let _ = self.playback_status_changed(&emitter).await;
    }

    fn seek(&self, offset: i64) {
        self.calls.lock().unwrap().push(format!("Seek {offset}"));
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.status.clone()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let track = ObjectPath::try_from("/track/1").unwrap();
        m.insert(
            "mpris:trackid".into(),
            Value::from(track).try_into().unwrap(),
        );
        m.insert(
            "xesam:title".into(),
            Value::from(self.title.clone()).try_into().unwrap(),
        );
        m.insert(
            "xesam:artist".into(),
            Value::from(vec!["A", "B"]).try_into().unwrap(),
        );
        m.insert(
            "xesam:album".into(),
            Value::from("Album").try_into().unwrap(),
        );
        m.insert(
            "mpris:length".into(),
            Value::from(60_000_000i64).try_into().unwrap(),
        );
        m
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        3_000_000
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
}

fn player(
    name: &str,
    identity: &'static str,
    status: &str,
    calls: Arc<Mutex<Vec<String>>>,
) -> zbus::Connection {
    zbus::block_on(async {
        zbus::connection::Builder::session()
            .unwrap()
            .name(name.to_owned())
            .unwrap()
            .serve_at(PATH, Root(identity))
            .unwrap()
            .serve_at(
                PATH,
                Fake {
                    status: status.into(),
                    title: "First".into(),
                    calls,
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap()
    })
}

#[test]
fn players_are_followed_and_told() {
    let _bus = common::private_bus();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fake = player(
        "org.mpris.MediaPlayer2.fake",
        "Fake",
        "Paused",
        calls.clone(),
    );
    let m = media();
    until("the player", || m.players().len() == 1);
    let p = m.current().unwrap();
    assert_eq!(p.name, "org.mpris.MediaPlayer2.fake");
    assert_eq!(p.identity, "Fake");
    assert_eq!(p.status, PlaybackStatus::Paused);
    assert_eq!((p.title.as_str(), p.album.as_str()), ("First", "Album"));
    assert_eq!(p.artists, ["A", "B"]);
    assert_eq!(p.length, Some(Duration::from_secs(60)));
    assert!(p.can_go_next && p.can_pause && !p.can_go_previous);
    assert_eq!(p.position(), Duration::from_secs(3));

    // Another one that comes later and doesn't play.
    let other = player(
        "org.mpris.MediaPlayer2.other",
        "Other",
        "Paused",
        calls.clone(),
    );
    until("the other player", || m.players().len() == 2);

    // Play-pause reaches the player, and what it says comes back.
    let done = Rc::new(RefCell::new(None));
    let d = done.clone();
    p.play_pause().then(move |r| *d.borrow_mut() = Some(r));
    until("the reply", || done.borrow().is_some());
    assert_eq!(*done.borrow(), Some(Ok(())));
    assert_eq!(calls.lock().unwrap()[0], "PlayPause");
    until("it to play", || {
        m.current()
            .is_some_and(|p| p.status == PlaybackStatus::Playing && p.identity == "Fake")
    });
    std::thread::sleep(Duration::from_millis(300));
    let at = m.current().unwrap().position();
    assert!(
        at >= Duration::from_millis(3250) && at < Duration::from_secs(5),
        "{at:?}"
    );

    // A new title.
    zbus::block_on(async {
        let iface = fake
            .object_server()
            .interface::<_, Fake>(PATH)
            .await
            .unwrap();
        iface.get_mut().await.title = "Second".into();
        iface
            .get()
            .await
            .metadata_changed(iface.signal_emitter())
            .await
            .unwrap();
    });
    until("the new title", || {
        m.players().iter().any(|p| p.title == "Second")
    });

    let done = Rc::new(RefCell::new(None));
    let d = done.clone();
    m.current()
        .unwrap()
        .seek(-1.5)
        .then(move |r| *d.borrow_mut() = Some(r));
    until("the seek", || done.borrow().is_some());
    assert!(calls.lock().unwrap().contains(&"Seek -1500000".to_owned()));

    // Players go with their connections.
    drop(fake);
    until("the player to go", || m.players().len() == 1);
    assert_eq!(m.current().unwrap().identity, "Other");
    drop(other);
    until("the other to go", || m.players().is_empty());
    assert_eq!(m.current(), None);
}
