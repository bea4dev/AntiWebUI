//! Settings for the whole framework (see `docs/design/08-scrolling-and-settings.md`).

use crate::channel::UiSender;
use crate::reactive::{Signal, create_root, signal};
use std::cell::RefCell;
use std::sync::Mutex;
use std::thread::ThreadId;

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Settings {
    pub scroll: ScrollSettings,
    pub rendering: RenderingSettings,
    pub images: ImageSettings,
    pub video: crate::video::VideoSettings,
    pub debug: DebugSettings,
}

/// What helps while making an app.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct DebugSettings {
    /// Shows what frames take and how they are paced at the top right of the window
    /// (`ANTIWEBUI_PERF_OVERLAY=1` does too). See `docs/design/30-performance-overlay.md`.
    pub performance_overlay: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImageSettings {
    /// How much of what pictures take to keep when nothing on the screen uses it, in bytes. What
    /// is on the screen stays whatever this says.
    pub cache_bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RenderingSettings {
    /// Whether frames are produced ahead of what the compositor has taken.
    pub frame_advance: FrameAdvance,
}

/// Whether the work on a frame may start before the compositor has taken the last one, so that
/// the CPU works on one frame while the GPU works on the one before (triple buffering). Frames
/// that don't fit into a refresh then can still come at every refresh, at the cost of a refresh
/// of latency for each frame ahead. A frame after a pause is never ahead of anything, so this
/// doesn't affect typing or clicks. See `docs/design/10-frame-pacing.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameAdvance {
    /// Renders one frame ahead only while frames keep being too late without it.
    #[default]
    Auto,
    /// Always renders one frame ahead while frames are continuous.
    Enable,
    /// Never renders ahead: the lowest latency, and refreshes skipped when frames are late.
    Disable,
    /// Renders up to this many frames ahead while frames are continuous. 0 is the same as
    /// [`FrameAdvance::Disable`]. More than 4 counts as 4.
    Custom { max_advance: usize },
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScrollSettings {
    /// Multiplies the motion of fingers on a touchpad, and inertia that follows from it.
    pub touchpad_factor: f32,
    /// Logical pixels per wheel notch.
    pub wheel_step: f32,
    pub kinetic: KineticSettings,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct KineticSettings {
    /// Whether scrolling goes on after the fingers leave a touchpad.
    pub enabled: bool,
    /// Multiplies the speed inertia starts with. Only where the framework computes inertia, which
    /// macOS and Windows do themselves. So do the settings below.
    pub velocity_factor: f32,
    /// In logical pixels per second squared; larger stops sooner. The distance is the speed
    /// squared over twice this.
    pub deceleration: f32,
    /// The furthest one fling goes, in window sizes.
    pub max_screens: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scroll: ScrollSettings {
                touchpad_factor: 1.0,
                wheel_step: 48.0,
                kinetic: KineticSettings {
                    enabled: true,
                    velocity_factor: 1.0,
                    // 96 pixels per inch at Chromium's 2.7559e-5 inches per millisecond squared.
                    deceleration: 2645.7,
                    max_screens: 3.0,
                },
            },
            rendering: RenderingSettings {
                frame_advance: FrameAdvance::Auto,
            },
            images: ImageSettings {
                cache_bytes: 256 << 20,
            },
            video: Default::default(),
            debug: DebugSettings::default(),
        }
    }
}

thread_local! {
    // In its own root: it outlives whichever owner happens to read it first.
    static SETTINGS: Signal<Settings> = create_root(|| signal(Settings::default())).0;
}

/// The current settings. Reading them in an effect or a view subscribes to changes.
pub fn settings() -> Settings {
    SETTINGS.with(|s| s.get())
}

/// Changes the settings. They apply from the next event on, in the windows of every thread.
pub fn update_settings(f: impl FnOnce(&mut Settings)) {
    SETTINGS.with(|s| s.update(f));
    broadcast();
}

pub(crate) fn settings_untracked() -> Settings {
    SETTINGS.with(|s| s.get_untracked())
}

/// Takes the settings of another thread, without telling the others again.
pub(crate) fn adopt(settings: Settings) {
    SETTINGS.with(|s| s.set_if_changed(settings));
}

/// The UI threads, which hear of settings changed on any of them (see
/// `docs/design/36-multi-window.md`).
static UI_THREADS: Mutex<Vec<(ThreadId, UiSender<Settings>)>> = Mutex::new(Vec::new());

/// Leaves [`UI_THREADS`] with the thread.
struct Member;

impl Drop for Member {
    fn drop(&mut self) {
        let here = std::thread::current().id();
        if let Ok(mut threads) = UI_THREADS.lock() {
            threads.retain(|(t, _)| *t != here);
        }
    }
}

thread_local! {
    static MEMBER: RefCell<Option<Member>> = const { RefCell::new(None) };
}

/// This thread runs windows: it hears of settings changed on the others.
pub(crate) fn join_broadcast() {
    if MEMBER.with(|m| m.borrow().is_some()) {
        return;
    }
    let sender = crate::channel::ui_channel(adopt);
    UI_THREADS
        .lock()
        .expect("no panics in here")
        .push((std::thread::current().id(), sender));
    MEMBER.with(|m| *m.borrow_mut() = Some(Member));
}

fn broadcast() {
    let here = std::thread::current().id();
    let threads = UI_THREADS.lock().expect("no panics in here");
    if threads.iter().all(|(t, _)| *t == here) {
        return;
    }
    let settings = settings_untracked();
    for (thread, sender) in threads.iter() {
        if *thread != here {
            sender.send(settings.clone());
        }
    }
}

pub(crate) fn with_settings<R>(f: impl FnOnce(&Settings) -> R) -> R {
    SETTINGS.with(|s| s.with_untracked(f))
}
