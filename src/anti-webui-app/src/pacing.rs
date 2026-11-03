//! When frames are produced: what became of the last ones, and whether to render ahead of the
//! compositor (see `docs/design/10-frame-pacing.md`).

use anti_webui_core::settings::{FrameAdvance, settings};
use anti_webui_wayland::{Ctx, Presentation, SurfaceId};
use anti_webui_wgpu::linux_sync;
use std::collections::VecDeque;
use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

/// More than this has no use: each frame ahead is one more refresh of latency.
const MAX_ADVANCE: usize = 4;
/// A frame that starts this soon after it became possible had been waiting for that moment.
const CONTINUOUS_NS: i64 = 2_000_000;
/// What the compositor hasn't said by then, it won't say.
const GIVE_UP_NS: i64 = 1_000_000_000;

/// How many of the frames looked at are skipped before rendering ahead is tried out...
const LOOKED_AT: usize = 32;
const SKIPS_TO_TRY: usize = 4;
/// A gap on the screen counts for at most this many refreshes when comparing rates: what is
/// compared is how frames come, not the odd stall.
const LONGEST_GAP: i64 = 4;
/// ...how much faster than before the frames must then come for it to stay...
const WORTH_IT: f64 = 1.1;
/// ...how long it isn't tried again if not, doubling every time...
const FIRST_RETRY: Duration = Duration::from_secs(8);
const LONGEST_RETRY: Duration = Duration::from_secs(64);
/// ...and which part of a refresh a frame has to fit in for it to end.
const CALM_FRACTION: f64 = 0.7;
/// How long frames have to fit in that before stopping. Doubles if it was too soon.
const FIRST_HOLD: Duration = Duration::from_secs(2);
const LONGEST_HOLD: Duration = Duration::from_secs(16);
const TOO_SOON: Duration = Duration::from_secs(5);
/// After this long without frames, what was learned about the load no longer holds.
const FORGET_AFTER: Duration = Duration::from_secs(5);

/// What the presenter records about one presentation. Times are `CLOCK_MONOTONIC` nanoseconds.
#[derive(Debug, Default)]
pub(crate) struct PresentTrace {
    pub token: u64,
    pub diff_done: i64,
    pub buffer_ready: i64,
    pub recorded: i64,
    pub submitted: i64,
    /// 0 while the frame is held back.
    pub committed: i64,
    pub damage: i64,
    pub repaint: i64,
    pub pixels: i64,
    pub buffers: usize,
    /// Signals when the GPU is done with the frame. `None` if it already was, or isn't known.
    pub fence: Option<OwnedFd>,
}

/// Everything known about a frame. Times are `CLOCK_MONOTONIC` nanoseconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameRecord {
    pub token: u64,
    pub callback: Option<i64>,
    pub start: i64,
    pub ui_done: i64,
    pub diff_done: i64,
    pub buffer_ready: i64,
    pub recorded: i64,
    pub submitted: i64,
    pub committed: i64,
    pub end: i64,
    pub gpu_done: Option<i64>,
    pub presented: Option<i64>,
    pub refresh: u32,
    pub flags: u32,
    pub discarded: bool,
    pub damage: i64,
    pub repaint: i64,
    pub pixels: i64,
    pub buffers: usize,
}

impl FrameRecord {
    /// How many refreshes went by without anything new before this frame. `None` if the frame
    /// didn't start as soon as it could, so that the gap says nothing about how fast frames are.
    pub fn skipped_after(&self, prev: &FrameRecord) -> Option<usize> {
        let (before, shown, callback) = (prev.presented?, self.presented?, self.callback?);
        if self.refresh == 0 || self.start - callback.max(prev.end) > CONTINUOUS_NS {
            return None;
        }
        let refresh = self.refresh as i64;
        Some(((shown - before + refresh / 2) / refresh - 1).max(0) as usize)
    }

    /// How long the frame took from its start until the GPU was done.
    fn elapsed(&self) -> i64 {
        self.gpu_done.unwrap_or(0).max(self.end) - self.start
    }
}

/// Decides whether to render ahead when asked to do so automatically: only while it's keeping
/// frames from being skipped, because each frame ahead costs a refresh of latency. Whether it does
/// isn't predicted but tried out, because that depends on how the CPU, the GPU and the compositor
/// happen to fit together.
#[derive(Debug)]
pub(crate) struct AutoAdvance {
    state: AutoState,
    /// The last frames looked at: whether a refresh was skipped before each, and the time since
    /// the frame before on the screen.
    looked_at: VecDeque<(bool, i64)>,
    /// How many frames in a row were continuous. The first [`LOOKED_AT`] of them say little: frames
    /// come slowly for a moment after a pause, whether or not they are ahead.
    burst: usize,
    /// Since when frames have been fitting well.
    calm_since: Option<i64>,
    hold: Duration,
    retry: Duration,
    /// When rendering ahead last ended.
    ended: Option<i64>,
    last_frame: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum AutoState {
    Off,
    /// Not trying again until then.
    Unhelpful {
        until: i64,
    },
    /// Rendering ahead, to see if frames come faster than the time between them before.
    Trying {
        before: f64,
    },
    On,
}

impl Default for AutoAdvance {
    fn default() -> Self {
        Self {
            state: AutoState::Off,
            looked_at: VecDeque::with_capacity(LOOKED_AT),
            burst: 0,
            calm_since: None,
            hold: FIRST_HOLD,
            retry: FIRST_RETRY,
            ended: None,
            last_frame: None,
        }
    }
}

impl AutoAdvance {
    pub fn engaged(&self) -> bool {
        matches!(self.state, AutoState::Trying { .. } | AutoState::On)
    }

    fn mean_interval(&self) -> f64 {
        let sum: i64 = self.looked_at.iter().map(|&(_, interval)| interval).sum();
        sum as f64 / self.looked_at.len().max(1) as f64
    }

    pub fn observe(&mut self, prev: &FrameRecord, this: &FrameRecord) {
        let forgotten = self
            .last_frame
            .is_some_and(|t| this.start - t > FORGET_AFTER.as_nanos() as i64);
        self.last_frame = Some(this.start);
        if forgotten {
            *self = Self {
                last_frame: self.last_frame,
                ..Self::default()
            };
        }
        let (Some(skipped), Some(shown), Some(before)) =
            (this.skipped_after(prev), this.presented, prev.presented)
        else {
            self.burst = 0;
            return;
        };
        self.burst += 1;
        if self.burst <= LOOKED_AT && !self.engaged() {
            self.looked_at.clear();
            return;
        }
        if self.looked_at.len() == LOOKED_AT {
            self.looked_at.pop_front();
        }
        let gap = shown - before;
        let gap = gap.min(this.refresh as i64 * LONGEST_GAP);
        self.looked_at.push_back((skipped > 0, gap));
        match self.state {
            AutoState::Unhelpful { until } if this.start < until => {}
            AutoState::Off | AutoState::Unhelpful { .. } => self.consider_trying(this),
            AutoState::Trying { before } => self.judge_trial(before, this),
            AutoState::On => self.consider_ending(this),
        }
    }

    fn consider_trying(&mut self, this: &FrameRecord) {
        let skips = self.looked_at.iter().filter(|&&(s, _)| s).count();
        if skips < SKIPS_TO_TRY || self.looked_at.len() < LOOKED_AT {
            return;
        }
        self.state = AutoState::Trying {
            before: self.mean_interval(),
        };
        self.looked_at.clear();
        self.calm_since = None;
        if self
            .ended
            .is_some_and(|t| this.start - t < TOO_SOON.as_nanos() as i64)
        {
            self.hold = (self.hold * 2).min(LONGEST_HOLD);
        }
    }

    fn judge_trial(&mut self, before: f64, this: &FrameRecord) {
        if self.looked_at.len() < LOOKED_AT {
            return;
        }
        if self.mean_interval() * WORTH_IT <= before {
            self.state = AutoState::On;
            self.retry = FIRST_RETRY;
        } else {
            self.state = AutoState::Unhelpful {
                until: this.start + self.retry.as_nanos() as i64,
            };
            self.retry = (self.retry * 2).min(LONGEST_RETRY);
            self.looked_at.clear();
        }
    }

    fn consider_ending(&mut self, this: &FrameRecord) {
        if this.elapsed() as f64 > this.refresh as f64 * CALM_FRACTION {
            self.calm_since = None;
            return;
        }
        let since = *self.calm_since.get_or_insert(this.start);
        if this.start - since >= self.hold.as_nanos() as i64 {
            self.state = AutoState::Off;
            self.ended = Some(this.start);
            self.looked_at.clear();
        }
    }
}

struct InFlight {
    record: FrameRecord,
    fence: Option<OwnedFd>,
    /// The compositor will say what became of it.
    feedback: bool,
}

/// What became of the frames, and what follows from that.
pub(crate) struct Pacing {
    flying: VecDeque<InFlight>,
    last: Option<FrameRecord>,
    next_token: u64,
    /// `Instant` and `CLOCK_MONOTONIC` at the same moment.
    base: (Instant, i64),
    /// Add to presentation times to get `CLOCK_MONOTONIC`.
    presentation_offset: Option<i64>,
    auto: AutoAdvance,
    /// `ANTIWEBUI_FRAME_ADVANCE`, for trying out without changing the program.
    forced: Option<FrameAdvance>,
    started: i64,
    callback: Option<i64>,
    ui_done: i64,
}

/// Reads `auto`, `on`, `off` or a number.
fn parse_advance(value: &str) -> Option<FrameAdvance> {
    match value {
        "auto" => Some(FrameAdvance::Auto),
        "on" => Some(FrameAdvance::Enable),
        "off" => Some(FrameAdvance::Disable),
        n => n
            .parse()
            .ok()
            .map(|max_advance| FrameAdvance::Custom { max_advance }),
    }
}

impl Pacing {
    pub fn new() -> Self {
        Self {
            flying: VecDeque::new(),
            last: None,
            next_token: 0,
            base: (Instant::now(), linux_sync::monotonic_ns()),
            presentation_offset: None,
            auto: AutoAdvance::default(),
            forced: std::env::var("ANTIWEBUI_FRAME_ADVANCE")
                .ok()
                .and_then(|v| parse_advance(&v)),
            started: 0,
            callback: None,
            ui_done: 0,
        }
    }

    /// How many finished frames may wait for the compositor to take them.
    pub fn max_advance(&self) -> usize {
        let advance = self
            .forced
            .unwrap_or_else(|| settings().rendering.frame_advance);
        match advance {
            FrameAdvance::Auto => usize::from(self.auto.engaged()),
            FrameAdvance::Enable => 1,
            FrameAdvance::Disable => 0,
            FrameAdvance::Custom { max_advance } => max_advance.min(MAX_ADVANCE),
        }
    }

    /// To call first thing in a frame. Passes to `finished` the frames nothing more will be
    /// learned about.
    pub fn begin_frame(&mut self, ctx: &mut Ctx<'_>, mut finished: impl FnMut(&FrameRecord)) {
        self.collect(ctx, &mut finished);
        self.started = linux_sync::monotonic_ns();
        let (instant, ns) = self.base;
        self.callback = ctx
            .last_frame_callback(SurfaceId::Window)
            .map(|t| ns + t.saturating_duration_since(instant).as_nanos() as i64);
    }

    /// To call when the UI's part of the frame is done.
    pub fn trace(&mut self) -> PresentTrace {
        self.ui_done = linux_sync::monotonic_ns();
        self.next_token += 1;
        PresentTrace {
            token: self.next_token,
            ..Default::default()
        }
    }

    pub fn end_frame(&mut self, trace: PresentTrace, presented: bool, feedback: bool) {
        if !presented {
            return;
        }
        self.flying.push_back(InFlight {
            record: FrameRecord {
                token: trace.token,
                callback: self.callback,
                start: self.started,
                ui_done: self.ui_done,
                diff_done: trace.diff_done,
                buffer_ready: trace.buffer_ready,
                recorded: trace.recorded,
                submitted: trace.submitted,
                committed: trace.committed,
                end: linux_sync::monotonic_ns(),
                damage: trace.damage,
                repaint: trace.repaint,
                pixels: trace.pixels,
                buffers: trace.buffers,
                ..Default::default()
            },
            fence: trace.fence,
            feedback,
        });
    }

    /// A frame held back went to the compositor.
    pub fn committed(&mut self, token: u64, time: i64) {
        if let Some(f) = self.flying.iter_mut().find(|f| f.record.token == token) {
            f.record.committed = time;
        }
    }

    fn collect(&mut self, ctx: &mut Ctx<'_>, finished: &mut impl FnMut(&FrameRecord)) {
        if self.presentation_offset.is_none()
            && let Some(clock) = ctx.presentation_clock()
            && let Ok(theirs) = linux_sync::clock_ns(clock)
        {
            self.presentation_offset = Some(linux_sync::monotonic_ns() - theirs);
        }
        for p in ctx.take_presentations() {
            self.apply_presentation(&p);
        }
        for f in &mut self.flying {
            if let Some(fence) = &f.fence {
                match linux_sync::sync_file_signal(fence.as_fd()) {
                    // Drivers don't have to say when.
                    Ok(Some(time)) => {
                        f.record.gpu_done = (time > 0).then_some(time);
                        f.fence = None;
                    }
                    Ok(None) => {}
                    Err(_) => f.fence = None,
                }
            }
        }
        let now = linux_sync::monotonic_ns();
        while let Some(front) = self.flying.front() {
            let known = front.fence.is_none() && !front.feedback && front.record.committed != 0;
            if !known && now - front.record.start < GIVE_UP_NS {
                break;
            }
            let frame = self.flying.pop_front().expect("just looked at it").record;
            if let Some(prev) = &self.last {
                self.auto.observe(prev, &frame);
            }
            finished(&frame);
            self.last = Some(frame);
        }
    }

    fn apply_presentation(&mut self, p: &Presentation) {
        let offset = self.presentation_offset.unwrap_or(0);
        let Some(f) = self.flying.iter_mut().find(|f| f.record.token == p.token) else {
            return;
        };
        f.feedback = false;
        match p.shown {
            Some(s) => {
                f.record.presented = Some(s.time_ns + offset);
                f.record.refresh = s.refresh_ns;
                f.record.flags = s.flags;
            }
            None => f.record.discarded = true,
        }
    }

    /// What is still under way when the window closes. Times may be missing.
    pub fn drain(&mut self, finished: &mut impl FnMut(&FrameRecord)) {
        for f in self.flying.drain(..) {
            finished(&f.record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;
    const REFRESH: i64 = 8 * MS;

    /// A frame starting right at its callback, `refreshes` after the frame before on the screen.
    fn frame(start: i64, elapsed: i64, shown: i64) -> FrameRecord {
        FrameRecord {
            callback: Some(start - 100_000),
            start,
            committed: start + MS,
            end: start + MS,
            gpu_done: Some(start + elapsed),
            presented: Some(shown),
            refresh: REFRESH as u32,
            ..Default::default()
        }
    }

    /// Feeds frames that take `elapsed`, on the screen every `step`, starting at `from`.
    fn feed(auto: &mut AutoAdvance, from: i64, count: i64, elapsed: i64, step: i64) -> i64 {
        let mut prev = frame(from - step, elapsed, from);
        for i in 0..count {
            let start = from + i * step;
            let this = frame(start, elapsed, start + step);
            auto.observe(&prev, &this);
            prev = this;
        }
        from + count * step
    }

    #[test]
    fn a_steady_rate_skips_nothing() {
        let prev = frame(0, 2 * MS, REFRESH);
        let this = frame(REFRESH, 2 * MS, 2 * REFRESH);
        assert_eq!(this.skipped_after(&prev), Some(0));
    }

    #[test]
    fn a_gap_on_the_screen_counts_in_refreshes() {
        let prev = frame(0, 2 * MS, REFRESH);
        let this = frame(REFRESH, 2 * MS, 4 * REFRESH);
        assert_eq!(this.skipped_after(&prev), Some(2));
    }

    #[test]
    fn a_gap_is_rounded_to_refreshes() {
        // Times on the screen aren't exactly a whole number of refreshes apart.
        let prev = frame(0, 2 * MS, REFRESH);
        let this = frame(REFRESH, 2 * MS, REFRESH + 2 * REFRESH - 100_000);
        assert_eq!(this.skipped_after(&prev), Some(1));
        let this = frame(REFRESH, 2 * MS, REFRESH + REFRESH - 100_000);
        assert_eq!(this.skipped_after(&prev), Some(0));
    }

    #[test]
    fn a_frame_after_a_pause_says_nothing() {
        let prev = frame(0, 2 * MS, REFRESH);
        let mut this = frame(100 * MS, 2 * MS, 100 * MS + REFRESH);
        this.callback = Some(REFRESH);
        assert_eq!(this.skipped_after(&prev), None);
    }

    #[test]
    fn a_frame_that_had_to_wait_for_the_cpu_counts() {
        // The callback came while the frame before was still being worked on.
        let mut prev = frame(0, 2 * MS, REFRESH);
        prev.end = 12 * MS;
        let mut this = frame(12 * MS, 2 * MS, 3 * REFRESH);
        this.callback = Some(REFRESH);
        assert_eq!(this.skipped_after(&prev), Some(1));
    }

    #[test]
    fn frames_not_shown_say_nothing() {
        let prev = frame(0, 2 * MS, REFRESH);
        let mut this = frame(REFRESH, 2 * MS, 0);
        this.presented = None;
        assert_eq!(this.skipped_after(&prev), None);
        assert_eq!(prev.skipped_after(&this), None);
    }

    /// Frames on the screen every other refresh, until rendering ahead is being tried.
    fn start_trial(auto: &mut AutoAdvance, from: i64) -> i64 {
        feed(auto, from, 2 * LOOKED_AT as i64, 10 * MS, 2 * REFRESH)
    }

    /// Frames on the screen every other refresh, for `wait`.
    fn keep_skipping(auto: &mut AutoAdvance, from: i64, wait: Duration) -> i64 {
        let count = wait.as_nanos() as i64 / (2 * REFRESH);
        feed(auto, from, count, 10 * MS, 2 * REFRESH)
    }

    /// Enough frames to judge the trial by, on the screen every `step`.
    fn finish_trial(auto: &mut AutoAdvance, from: i64, step: i64) -> i64 {
        feed(auto, from, LOOKED_AT as i64 + 1, 10 * MS, step)
    }

    fn turned_on(auto: &mut AutoAdvance) -> i64 {
        let t = start_trial(auto, 0);
        finish_trial(auto, t, REFRESH)
    }

    #[test]
    fn frames_with_no_gpu_time_count_by_the_cpu() {
        let mut this = frame(0, 5 * MS, REFRESH);
        this.gpu_done = None;
        assert_eq!(this.elapsed(), MS);
        // Finishing before the CPU did can't be right.
        this.gpu_done = Some(1);
        assert_eq!(this.elapsed(), MS);
    }

    #[test]
    fn stays_off_while_frames_keep_up() {
        let mut auto = AutoAdvance::default();
        feed(&mut auto, 0, 1000, 2 * MS, REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_few_skips_are_not_enough() {
        let mut auto = AutoAdvance::default();
        let t = feed(&mut auto, 0, 100, 2 * MS, REFRESH);
        let t = feed(&mut auto, t, SKIPS_TO_TRY as i64 - 2, 2 * MS, 2 * REFRESH);
        feed(&mut auto, t, 100, 2 * MS, REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn is_tried_when_frames_keep_being_skipped() {
        let mut auto = AutoAdvance::default();
        start_trial(&mut auto, 0);
        assert!(auto.engaged());
    }

    #[test]
    fn the_first_frames_of_a_burst_do_not_start_a_trial() {
        let mut auto = AutoAdvance::default();
        feed(&mut auto, 0, 2 * LOOKED_AT as i64 - 2, 10 * MS, 2 * REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_pause_starts_a_burst_over() {
        let mut auto = AutoAdvance::default();
        let t = feed(&mut auto, 0, 2 * LOOKED_AT as i64 - 2, 10 * MS, 2 * REFRESH);
        let mut after_pause = frame(t + 100 * MS, 10 * MS, t + 120 * MS);
        after_pause.callback = Some(t);
        auto.observe(&frame(t - 2 * REFRESH, 10 * MS, t), &after_pause);
        feed(
            &mut auto,
            t + 200 * MS,
            LOOKED_AT as i64,
            10 * MS,
            2 * REFRESH,
        );
        assert!(!auto.engaged());
    }

    #[test]
    fn an_odd_stall_does_not_make_a_trial_look_good() {
        let mut auto = AutoAdvance::default();
        // Frames on the screen every other refresh, also during the trial, apart from a long
        // stall before it. Without a limit, that would make the rate look better afterwards.
        let t = feed(&mut auto, 0, LOOKED_AT as i64 + 8, 10 * MS, 2 * REFRESH);
        let t = feed(&mut auto, t, 1, 10 * MS, 500 * MS);
        let t = feed(&mut auto, t, LOOKED_AT as i64 - 9, 10 * MS, 2 * REFRESH);
        assert!(auto.engaged());
        finish_trial(&mut auto, t, 2 * REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn stays_on_if_frames_come_faster() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        // Nothing is skipped any more because of rendering ahead, for a long time.
        feed(&mut auto, t, 10_000, 10 * MS, REFRESH);
        assert!(auto.engaged());
        assert_eq!(auto.state, AutoState::On);
    }

    #[test]
    fn ends_if_frames_come_no_faster() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        let t = finish_trial(&mut auto, t, 2 * REFRESH);
        assert!(!auto.engaged(), "it only cost latency");
        // Not tried again right away, however many are skipped.
        feed(&mut auto, t, 100, 10 * MS, 2 * REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_small_gain_is_not_worth_it() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        // 5% faster: not worth a refresh of latency.
        finish_trial(&mut auto, t, 2 * REFRESH * 100 / 105);
        assert!(!auto.engaged());
    }

    #[test]
    fn an_unhelpful_trial_is_repeated_later_and_less_often() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        let t = finish_trial(&mut auto, t, 2 * REFRESH);
        assert!(!auto.engaged());
        let t = keep_skipping(&mut auto, t, FIRST_RETRY - Duration::from_millis(100));
        assert!(!auto.engaged(), "too soon");
        let t = keep_skipping(&mut auto, t, Duration::from_millis(200));
        assert!(auto.engaged(), "the load may have changed");
        let t = finish_trial(&mut auto, t, 2 * REFRESH);
        assert!(!auto.engaged());
        let t = keep_skipping(&mut auto, t, FIRST_RETRY + Duration::from_millis(100));
        assert!(!auto.engaged(), "twice as long now");
        keep_skipping(&mut auto, t, FIRST_RETRY);
        assert!(auto.engaged());
    }

    #[test]
    fn a_trial_that_helped_resets_how_long_to_wait() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        let t = finish_trial(&mut auto, t, 2 * REFRESH);
        let t = keep_skipping(&mut auto, t, FIRST_RETRY + Duration::from_millis(100));
        assert!(auto.engaged());
        finish_trial(&mut auto, t, REFRESH);
        assert_eq!(auto.state, AutoState::On);
        assert_eq!(auto.retry, FIRST_RETRY);
    }

    #[test]
    fn ends_after_frames_have_been_fitting_for_a_while() {
        let mut auto = AutoAdvance::default();
        let t = turned_on(&mut auto);
        let per_second = 1000 * MS / REFRESH;
        let t = feed(&mut auto, t, per_second, 2 * MS, REFRESH);
        assert!(auto.engaged(), "too soon");
        feed(&mut auto, t, per_second + 2, 2 * MS, REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_slow_frame_starts_the_wait_over() {
        let mut auto = AutoAdvance::default();
        let mut t = turned_on(&mut auto);
        let per_second = 1000 * MS / REFRESH;
        for _ in 0..4 {
            t = feed(&mut auto, t, per_second, 2 * MS, REFRESH);
            t = feed(&mut auto, t, 1, 10 * MS, REFRESH);
        }
        assert!(auto.engaged());
    }

    #[test]
    fn coming_back_too_soon_doubles_the_wait() {
        let mut auto = AutoAdvance::default();
        let per_second = 1000 * MS / REFRESH;
        let t = turned_on(&mut auto);
        let t = feed(&mut auto, t, 2 * per_second + 2, 2 * MS, REFRESH);
        assert!(!auto.engaged());
        // Frames have been continuous, so it takes less than after a pause.
        let t = feed(&mut auto, t, LOOKED_AT as i64, 10 * MS, 2 * REFRESH);
        assert!(auto.engaged());
        assert_eq!(auto.hold, FIRST_HOLD * 2);
        let t = finish_trial(&mut auto, t, REFRESH);
        let t = feed(&mut auto, t, 3 * per_second, 2 * MS, REFRESH);
        assert!(auto.engaged(), "2 s are no longer enough");
        feed(&mut auto, t, per_second + 2, 2 * MS, REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_long_pause_forgets_the_load() {
        let mut auto = AutoAdvance::default();
        let t = turned_on(&mut auto);
        assert!(auto.engaged());
        feed(&mut auto, t + 6000 * MS, 2, 2 * MS, REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn advance_settings_can_be_given_in_the_environment() {
        assert_eq!(parse_advance("auto"), Some(FrameAdvance::Auto));
        assert_eq!(parse_advance("on"), Some(FrameAdvance::Enable));
        assert_eq!(parse_advance("off"), Some(FrameAdvance::Disable));
        assert_eq!(
            parse_advance("2"),
            Some(FrameAdvance::Custom { max_advance: 2 })
        );
        assert_eq!(parse_advance("what"), None);
    }
}
