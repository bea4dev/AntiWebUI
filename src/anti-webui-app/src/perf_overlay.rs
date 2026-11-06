//! Feeds the performance overlay with what became of the frames, and says when it needs a frame of
//! its own. See `docs/design/30-performance-overlay.md`.

use crate::pacing::FrameRecord;
use anti_webui_core::overlay::{FrameSample, PerformanceOverlay};
use anti_webui_core::settings::settings;

/// After the app stops drawing, how long until the overlay shows the frames that were still on
/// their way.
pub(crate) const LAST_WORD: std::time::Duration = std::time::Duration::from_millis(200);

pub(crate) struct OverlayFeed {
    /// `ANTIWEBUI_PERF_OVERLAY`, which goes before the settings.
    forced: Option<bool>,
    overlay: Option<PerformanceOverlay>,
    last_presented: Option<i64>,
    last_gpu_done: Option<i64>,
    /// The last frame waited for the compositor before it was committed: it was rendered ahead.
    ahead: bool,
    /// The last frame that had nothing new of the app's, which isn't one of its frames to count.
    own: Option<u64>,
}

impl OverlayFeed {
    pub fn new() -> Self {
        Self {
            forced: std::env::var("ANTIWEBUI_PERF_OVERLAY")
                .ok()
                .map(|v| !matches!(v.as_str(), "" | "0" | "off" | "false")),
            overlay: None,
            last_presented: None,
            last_gpu_done: None,
            ahead: false,
            own: None,
        }
    }

    /// Whether the overlay is to show, as of now.
    pub fn update(&mut self) -> bool {
        let shown = self
            .forced
            .unwrap_or_else(|| settings().debug.performance_overlay);
        if shown != self.overlay.is_some() {
            self.overlay = shown.then(PerformanceOverlay::new);
            self.last_presented = None;
            self.last_gpu_done = None;
        }
        shown
    }

    /// What became of a frame.
    pub fn add(&mut self, r: &FrameRecord) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if r.discarded {
            return;
        }
        let ms = |ns: i64| ns as f32 / 1e6;
        let interval = r.presented.zip(self.last_presented).map(|(a, b)| ms(a - b));
        if r.presented.is_some() {
            self.last_presented = r.presented;
        }
        // Not the time spent waiting for the frame before it to be drawn.
        let gpu = r.gpu_done.map(|done| {
            let began = r.submitted.max(self.last_gpu_done.unwrap_or(i64::MIN));
            ms(done - began).max(0.0)
        });
        if r.gpu_done.is_some() {
            self.last_gpu_done = r.gpu_done;
        }
        if self.own == Some(r.token) {
            return;
        }
        // Held back, it went to the compositor at a later frame callback.
        self.ahead = r.committed > r.end;
        overlay.push(FrameSample {
            interval,
            cpu: ms(r.end - r.start),
            ui: ms(r.ui_done - r.start),
            gpu,
            latency: r.presented.map(|p| ms(p - r.start)),
            refresh: (r.refresh > 0).then(|| ms(r.refresh as i64)),
        });
    }

    /// Puts the overlay on the window's scene. `token` is that of the frame, and `own` says it
    /// is the overlay's own frame.
    pub fn paint(
        &mut self,
        scene: &mut anti_webui_core::scene::Scene,
        scale: anti_webui_core::geometry::Scale,
        advance: String,
        token: u64,
        own: bool,
    ) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if own {
            self.own = Some(token);
        }
        overlay.set_advance(if self.ahead {
            format!("{advance}, ahead")
        } else {
            advance
        });
        overlay.paint(scene, scale);
    }

    /// The app has stopped drawing for now. Says whether to draw once more for the overlay, to
    /// show the frames that were still on their way: not after a frame that was already that.
    pub fn wants_last_word(&self, own: bool) -> bool {
        self.overlay.is_some() && !own
    }

    /// Whether the frame starting now has nothing new of the app's in it, like the one the overlay
    /// asks for, or one asked for again because there was no buffer for it.
    pub fn is_own_frame(&self, app_wants_one: bool) -> bool {
        self.overlay.is_some() && !app_wants_one
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    /// A frame that starts at `start` ms and is shown 16 ms later.
    fn record(token: u64, start: i64) -> FrameRecord {
        let start = start * MS;
        FrameRecord {
            token,
            start,
            ui_done: start + MS,
            submitted: start + 2 * MS,
            end: start + 3 * MS,
            gpu_done: Some(start + 5 * MS),
            presented: Some(start + 16 * MS),
            refresh: 16 * MS as u32,
            ..Default::default()
        }
    }

    fn feed() -> OverlayFeed {
        let mut f = OverlayFeed::new();
        f.forced = Some(true);
        assert!(f.update());
        f
    }

    fn samples(f: &OverlayFeed) -> Vec<FrameSample> {
        f.overlay.as_ref().unwrap().frames().copied().collect()
    }

    #[test]
    fn frames_become_samples() {
        let mut f = feed();
        f.add(&record(1, 0));
        f.add(&record(2, 16));
        let s = samples(&f);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].interval, None, "nothing was shown before it");
        assert_eq!(s[1].interval, Some(16.0));
        assert_eq!((s[1].cpu, s[1].ui), (3.0, 1.0));
        assert_eq!(s[1].gpu, Some(3.0));
        assert_eq!(s[1].latency, Some(16.0));
        assert_eq!(s[1].refresh, Some(16.0));
    }

    #[test]
    fn the_gpu_time_leaves_out_waiting_for_the_frame_before() {
        let mut f = feed();
        let mut a = record(1, 0);
        a.gpu_done = Some(10 * MS);
        // Submitted at 6 ms while the GPU was busy until 10 ms, and done at 12 ms.
        let mut b = record(2, 4);
        b.gpu_done = Some(12 * MS);
        f.add(&a);
        f.add(&b);
        assert_eq!(samples(&f)[1].gpu, Some(2.0));
    }

    #[test]
    fn a_frame_rendered_ahead_says_so() {
        let mut f = feed();
        let mut r = record(1, 0);
        // Committed as soon as it was made.
        r.committed = 2 * MS;
        f.add(&r);
        assert!(!f.ahead);
        // Held back until the next frame callback.
        r.token = 2;
        r.committed = 10 * MS;
        f.add(&r);
        assert!(f.ahead);
    }

    #[test]
    fn the_overlays_own_frame_isnt_counted_and_asks_for_no_other() {
        let mut f = feed();
        f.add(&record(1, 0));
        assert!(f.wants_last_word(false));
        assert!(f.is_own_frame(false));
        let mut scene = anti_webui_core::scene::Scene::default();
        f.paint(&mut scene, Scale(120), "Disable".into(), 2, true);
        assert!(!f.wants_last_word(true), "no frame after its own");
        f.add(&record(2, 300));
        assert_eq!(samples(&f).len(), 1);
        // The app's next frame comes after a pause, which the numbers leave out.
        f.add(&record(3, 600));
        assert_eq!(samples(&f)[1].interval, Some(300.0));
    }

    #[test]
    fn a_frame_the_app_wants_is_the_apps() {
        let f = feed();
        assert!(f.wants_last_word(false));
        assert!(!f.is_own_frame(true));
    }

    #[test]
    fn nothing_is_its_own_while_it_is_hidden() {
        let mut f = OverlayFeed::new();
        f.forced = Some(false);
        assert!(!f.update());
        assert!(!f.is_own_frame(false));
        assert!(!f.wants_last_word(false));
    }

    use anti_webui_core::geometry::Scale;
}
