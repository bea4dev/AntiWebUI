//! Media players through MPRIS: every `org.mpris.MediaPlayer2.*` on the session bus, what each is
//! playing, and playing, pausing and skipping.

use crate::ServiceError;
use crate::dbus::{self, Props, get, put, string};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use anti_webui_core::task::Task;
use futures_lite::StreamExt;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use zbus::Connection;
use zbus::fdo::DBusProxy;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
/// playerctld's stand-in for whichever player it thinks is current, which would show twice.
const PLAYERCTLD: &str = "org.mpris.MediaPlayer2.playerctld";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

/// Whether a player is playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl PlaybackStatus {
    fn of(s: &str) -> Self {
        match s {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// A player, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    /// Its name on the bus, like `org.mpris.MediaPlayer2.spotify`.
    pub name: String,
    /// What it calls itself, like `Spotify`.
    pub identity: String,
    pub status: PlaybackStatus,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    /// Where the cover is, usually a `file://` or `https://` URL.
    pub art_url: Option<String>,
    pub length: Option<Duration>,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
    /// From 0 to 1, if it has a volume of its own.
    pub volume: Option<f64>,
    track: Option<String>,
    /// Where it was, and when that was.
    position: (Duration, Instant),
    rate: f64,
    /// When it last started playing, counted; 0 if it never has since it was seen.
    played: u64,
}

impl Player {
    /// Where it is in what it plays: what it said last, and the time since then while it plays.
    /// Players don't tell as it goes, so this isn't tracked; read it under a timer, like one from
    /// `local_time(Every::Second)`.
    pub fn position(&self) -> Duration {
        let (at, when) = self.position;
        let now = if self.status == PlaybackStatus::Playing {
            at + when.elapsed().mul_f64(self.rate.max(0.0))
        } else {
            at
        };
        match self.length {
            Some(length) => now.min(length),
            None => now,
        }
    }

    fn call(&self, method: &'static str) -> Task<Result<(), ServiceError>> {
        let name = self.name.clone();
        dbus::run(async move {
            let conn = dbus::session().await?;
            dbus::call(&conn, &name, PATH, PLAYER, method, &()).await
        })
    }

    pub fn play_pause(&self) -> Task<Result<(), ServiceError>> {
        self.call("PlayPause")
    }

    pub fn play(&self) -> Task<Result<(), ServiceError>> {
        self.call("Play")
    }

    pub fn pause(&self) -> Task<Result<(), ServiceError>> {
        self.call("Pause")
    }

    pub fn next(&self) -> Task<Result<(), ServiceError>> {
        self.call("Next")
    }

    pub fn previous(&self) -> Task<Result<(), ServiceError>> {
        self.call("Previous")
    }

    /// Goes forward by `seconds`, or back if it is negative.
    pub fn seek(&self, seconds: f64) -> Task<Result<(), ServiceError>> {
        let name = self.name.clone();
        let micros = (seconds * 1e6) as i64;
        dbus::run(async move {
            let conn = dbus::session().await?;
            dbus::call(&conn, &name, PATH, PLAYER, "Seek", &(micros,)).await
        })
    }

    /// Goes to a place in what it plays.
    pub fn set_position(&self, position: Duration) -> Task<Result<(), ServiceError>> {
        let (name, track) = (self.name.clone(), self.track.clone());
        let micros = position.as_micros() as i64;
        dbus::run(async move {
            let track = track.ok_or(ServiceError::Unsupported)?;
            let track = OwnedObjectPath::try_from(track).map_err(dbus::failed)?;
            let conn = dbus::session().await?;
            dbus::call(&conn, &name, PATH, PLAYER, "SetPosition", &(track, micros)).await
        })
    }

    /// From 0 to 1.
    pub fn set_volume(&self, volume: f64) -> Task<Result<(), ServiceError>> {
        let name = self.name.clone();
        let volume = volume.clamp(0.0, 1.0);
        dbus::run(async move {
            let conn = dbus::session().await?;
            dbus::set_property(&conn, &name, PATH, PLAYER, "Volume", Value::from(volume)).await
        })
    }
}

/// The players. Cheap to copy; reading tracks, on any thread.
#[derive(Clone, Copy, Debug)]
pub struct Media(&'static Shared);

#[derive(Debug)]
struct Shared {
    players: SharedSignal<Vec<Player>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// The players. It starts following them the first time.
pub fn media() -> Media {
    Media(dbus::init_then_follow(&SHARED, follow, || {
        Shared {
            players: shared_signal(Vec::new()),
        }
    }))
}

impl Media {
    /// Every player, in the order they came.
    pub fn players(&self) -> Vec<Player> {
        self.0.players.get()
    }

    /// The one to show: the one that started playing last; or else the one that played last;
    /// or else the first.
    pub fn current(&self) -> Option<Player> {
        current(&self.players())
    }
}

fn current(players: &[Player]) -> Option<Player> {
    let latest = |playing: bool| {
        players
            .iter()
            .filter(|p| p.played > 0 && (!playing || p.status == PlaybackStatus::Playing))
            .max_by_key(|p| p.played)
    };
    latest(true)
        .or_else(|| latest(false))
        .or(players.first())
        .cloned()
}

/// A player as the stream of signals has told of it.
struct Entry {
    name: String,
    /// Its unique name, which its signals come from.
    owner: String,
    root: Props,
    player: Props,
    position: (Duration, Instant),
    played: u64,
}

impl Entry {
    fn status(&self) -> PlaybackStatus {
        PlaybackStatus::of(&string(&self.player, "PlaybackStatus"))
    }

    fn rate(&self) -> f64 {
        get(&self.player, "Rate").unwrap_or(1.0)
    }

    fn view(&self) -> Player {
        let meta: HashMap<String, OwnedValue> = get(&self.player, "Metadata").unwrap_or_default();
        let length = get::<i64>(&meta, "mpris:length")
            .or_else(|| get::<u64>(&meta, "mpris:length").map(|l| l as i64))
            .filter(|l| *l > 0)
            .map(|l| Duration::from_micros(l as u64));
        let track = get::<OwnedObjectPath>(&meta, "mpris:trackid")
            .map(|p| p.to_string())
            .or_else(|| get::<String>(&meta, "mpris:trackid"));
        Player {
            name: self.name.clone(),
            identity: string(&self.root, "Identity"),
            status: self.status(),
            title: string(&meta, "xesam:title"),
            artists: get::<Vec<String>>(&meta, "xesam:artist")
                .or_else(|| get::<String>(&meta, "xesam:artist").map(|a| vec![a]))
                .unwrap_or_default(),
            album: string(&meta, "xesam:album"),
            art_url: get::<String>(&meta, "mpris:artUrl").filter(|u| !u.is_empty()),
            length,
            can_go_next: get(&self.player, "CanGoNext").unwrap_or(false),
            can_go_previous: get(&self.player, "CanGoPrevious").unwrap_or(false),
            can_play: get(&self.player, "CanPlay").unwrap_or(false),
            can_pause: get(&self.player, "CanPause").unwrap_or(false),
            can_seek: get(&self.player, "CanSeek").unwrap_or(false),
            volume: get(&self.player, "Volume"),
            track,
            position: self.position,
            rate: self.rate(),
            played: self.played,
        }
    }

    /// Where it is now, by what it said last.
    fn now(&self) -> Duration {
        let (at, when) = self.position;
        if self.status() == PlaybackStatus::Playing {
            at + when.elapsed().mul_f64(self.rate().max(0.0))
        } else {
            at
        }
    }
}

/// Asks a player where it is.
async fn position(conn: &Connection, owner: &str) -> (Duration, Instant) {
    let micros: Option<OwnedValue> = dbus::call(
        conn,
        owner,
        PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(PLAYER, "Position"),
    )
    .await
    .ok();
    let micros = micros.and_then(|v| i64::try_from(v).ok()).unwrap_or(0);
    (Duration::from_micros(micros.max(0) as u64), Instant::now())
}

/// The state of all players, as the D-Bus thread keeps it.
#[derive(Default)]
struct Players {
    list: Vec<Entry>,
    /// How many times any player has started playing.
    plays: u64,
}

impl Players {
    async fn add(
        &mut self,
        conn: &Connection,
        bus: &DBusProxy<'_>,
        name: &str,
        owner: Option<String>,
    ) {
        if !name.starts_with(PREFIX) || name == PLAYERCTLD {
            return;
        }
        let owner = match owner {
            Some(o) => o,
            None => match name.try_into() {
                Ok(n) => match bus.get_name_owner(n).await {
                    Ok(o) => o.to_string(),
                    Err(_) => return,
                },
                Err(_) => return,
            },
        };
        let root = dbus::get_all(conn, &owner, PATH, ROOT)
            .await
            .unwrap_or_default();
        let player = dbus::get_all(conn, &owner, PATH, PLAYER)
            .await
            .unwrap_or_default();
        let position = position(conn, &owner).await;
        self.list.retain(|e| e.name != name);
        let mut entry = Entry {
            name: name.to_owned(),
            owner,
            root,
            player,
            position,
            played: 0,
        };
        if entry.status() == PlaybackStatus::Playing {
            self.plays += 1;
            entry.played = self.plays;
        }
        self.list.push(entry);
    }

    async fn changed(&mut self, conn: &Connection, sender: &str, interface: &str, changed: Props) {
        let Some(e) = self.list.iter_mut().find(|e| e.owner == sender) else {
            return;
        };
        if interface == ROOT {
            e.root.extend(changed);
            return;
        }
        let was = e.status();
        let rebase = changed.contains_key("PlaybackStatus") || changed.contains_key("Rate");
        let new_track = changed.contains_key("Metadata");
        if rebase {
            // Where it got to at the old rate, before the new one counts.
            e.position = (e.now(), Instant::now());
        }
        e.player.extend(changed);
        if was != PlaybackStatus::Playing && e.status() == PlaybackStatus::Playing {
            self.plays += 1;
            e.played = self.plays;
        }
        if new_track || (rebase && was != e.status()) {
            let owner = e.owner.clone();
            let at = position(conn, &owner).await;
            if let Some(e) = self.list.iter_mut().find(|e| e.owner == owner) {
                e.position = at;
            }
        }
    }

    fn publish(&self) {
        if let Some(s) = SHARED.get() {
            put(&s.players, self.list.iter().map(Entry::view).collect());
        }
    }
}

enum Heard {
    Signal(zbus::Message),
    Owner(zbus::fdo::NameOwnerChanged),
}

async fn follow() {
    let Ok(conn) = dbus::session().await else {
        return;
    };
    let Ok(bus) = DBusProxy::new(&conn).await else {
        return;
    };
    // Signals first, so that nothing is missed between reading and following.
    let Some(signals) = dbus::signals(&conn, PATH, None).await else {
        return;
    };
    let Ok(owners) = bus.receive_name_owner_changed().await else {
        return;
    };
    let mut heard = signals
        .filter_map(|m| m.ok().map(Heard::Signal))
        .or(owners.map(Heard::Owner));
    let mut players = Players::default();
    for name in bus.list_names().await.unwrap_or_default() {
        players.add(&conn, &bus, name.as_str(), None).await;
    }
    players.publish();
    while let Some(h) = heard.next().await {
        match h {
            Heard::Owner(c) => {
                let Ok(args) = c.args() else {
                    continue;
                };
                let name = args.name().to_string();
                if !name.starts_with(PREFIX) {
                    continue;
                }
                match args.new_owner().as_ref() {
                    Some(owner) => {
                        players
                            .add(&conn, &bus, &name, Some(owner.to_string()))
                            .await
                    }
                    None => players.list.retain(|e| e.name != name),
                }
            }
            Heard::Signal(msg) => {
                let Some(sender) = msg.header().sender().map(|s| s.to_string()) else {
                    continue;
                };
                if let Some((interface, changed, _)) = dbus::properties_changed(&msg) {
                    players.changed(&conn, &sender, &interface, changed).await;
                } else if let Some((_, interface, member)) = dbus::signal_of(&msg)
                    && interface == PLAYER
                    && member == "Seeked"
                {
                    let Ok(micros) = msg.body().deserialize::<i64>() else {
                        continue;
                    };
                    if let Some(e) = players.list.iter_mut().find(|e| e.owner == sender) {
                        e.position = (Duration::from_micros(micros.max(0) as u64), Instant::now());
                    }
                } else {
                    continue;
                }
            }
        }
        players.publish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(name: &str, status: PlaybackStatus, played: u64) -> Player {
        Player {
            name: name.into(),
            identity: String::new(),
            status,
            title: String::new(),
            artists: Vec::new(),
            album: String::new(),
            art_url: None,
            length: Some(Duration::from_secs(10)),
            can_go_next: false,
            can_go_previous: false,
            can_play: false,
            can_pause: false,
            can_seek: false,
            volume: None,
            track: None,
            position: (Duration::from_secs(3), Instant::now()),
            rate: 1.0,
            played,
        }
    }

    #[test]
    fn the_current_player_is_the_one_that_started_playing_last() {
        use PlaybackStatus::*;
        let list = [
            player("a", Paused, 0),
            player("b", Paused, 2),
            player("c", Playing, 1),
        ];
        assert_eq!(current(&list).unwrap().name, "c");
        let list = [
            player("a", Stopped, 0),
            player("b", Paused, 2),
            player("c", Paused, 1),
        ];
        assert_eq!(current(&list).unwrap().name, "b");
        let list = [player("a", Stopped, 0), player("b", Stopped, 0)];
        assert_eq!(current(&list).unwrap().name, "a");
        assert_eq!(current(&[]), None);
    }

    #[test]
    fn position_runs_while_playing_and_stops_at_the_end() {
        let mut p = player("a", PlaybackStatus::Paused, 0);
        assert_eq!(p.position(), Duration::from_secs(3));
        p.status = PlaybackStatus::Playing;
        p.position.1 = Instant::now() - Duration::from_secs(2);
        let at = p.position();
        assert!(
            at >= Duration::from_secs(5) && at < Duration::from_millis(5500),
            "{at:?}"
        );
        p.position.1 = Instant::now() - Duration::from_secs(60);
        assert_eq!(p.position(), Duration::from_secs(10));
    }
}
