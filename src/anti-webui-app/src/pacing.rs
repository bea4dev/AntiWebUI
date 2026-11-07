//! When frames are produced: what became of the last ones, and whether to render ahead of the
//! compositor (see `docs/design/10-frame-pacing.md`).

use crate::clock;
use crate::host::Host;
use crate::vsync::FrameClock;
use anti_webui_core::platform::{Presentation, SurfaceId};
use anti_webui_core::settings::{FrameAdvance, settings};
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

/// More than this has no use: each frame ahead is one more refresh of latency.
const MAX_ADVANCE: usize = 4;
/// A frame that starts this soon after it became possible had been waiting for that moment.
const CONTINUOUS_NS: i64 = 2_000_000;
/// What the compositor hasn't said by then, it won't say.
const GIVE_UP_NS: i64 = 1_000_000_000;

/// The first frames of a burst are slow whatever the load, so they say nothing.
const WARM_UP: usize = 4;
/// How many of the frames looked at are skipped before rendering ahead is tried out...
const LOOKED_AT: usize = 32;
const SKIPS_TO_TRY: usize = 4;
/// A longer gap between frames is a pause, not a frame that started a little late.
const PAUSE_NS: i64 = 100_000_000;
/// Frames come slowly for a while after a pause, whether or not they are ahead. Until this many,
/// rendering ahead looks better than it is...
const SETTLED: usize = 64;
/// ...so a trial that early is shorter, and stays only if the gain is more than that can explain.
/// A smaller one is tried again once settled.
const EARLY_TRIAL: usize = 16;
const CLEAR_GAIN: f64 = 1.5;
/// A gap on the screen counts for at most this many refreshes when comparing rates: what is
/// compared is how frames come, not the odd stall.
const LONGEST_GAP: i64 = 4;
/// ...how much faster than before the frames must then come for it to stay...
const WORTH_IT: f64 = 1.1;
/// ...how long it isn't tried again if not, doubling every time...
const FIRST_RETRY: Duration = Duration::from_secs(8);
const LONGEST_RETRY: Duration = Duration::from_secs(64);
/// ...and which part of a refresh a frame has to fit in for it to end. Well below what frames get
/// skipped at, so that it doesn't keep starting and ending.
const CALM_FRACTION: f64 = 0.5;
/// How long frames have to fit in that before stopping, not counting pauses. Doubles if it was too
/// soon.
const FIRST_HOLD: Duration = Duration::from_secs(2);
const LONGEST_HOLD: Duration = Duration::from_secs(16);
const TOO_SOON: Duration = Duration::from_secs(5);
/// After this long without frames, the load may have changed, so a trial is worth another try.
/// Rendering ahead isn't given up for that: it costs little if the load has become lighter, and it
/// ends by itself.
const FORGET_AFTER: Duration = Duration::from_secs(5);

/// What the presenter records about one presentation. Times are nanoseconds of [`clock`].
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

/// Everything known about a frame. Times are nanoseconds of [`clock`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameRecord {
    pub token: u64,
    pub callback: Option<i64>,
    pub start: i64,
    /// What animations were given as the time.
    pub frame_time: i64,
    /// The frame's place on the refresh grid, if known.
    pub slot: Option<i64>,
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
    /// How many frames in a row were continuous.
    burst: usize,
    /// There was a trial before [`SETTLED`] that said nothing clear.
    tried_early: bool,
    /// For how long frames have been fitting well.
    calm: i64,
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
        early: bool,
    },
    On,
}

impl Default for AutoAdvance {
    fn default() -> Self {
        Self {
            state: AutoState::Off,
            looked_at: VecDeque::with_capacity(LOOKED_AT),
            burst: 0,
            tried_early: false,
            calm: 0,
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
        if forgotten && self.state != AutoState::On {
            *self = Self {
                last_frame: self.last_frame,
                ..Self::default()
            };
        }
        let (Some(skipped), Some(shown), Some(before)) =
            (this.skipped_after(prev), this.presented, prev.presented)
        else {
            if this.start - prev.end > PAUSE_NS {
                self.burst = 0;
            }
            return;
        };
        self.burst += 1;
        if self.burst <= WARM_UP && !matches!(self.state, AutoState::Trying { .. }) {
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
            AutoState::Trying { before, early } => self.judge_trial(before, early, this),
            AutoState::On => self.consider_ending(prev, this),
        }
    }

    fn consider_trying(&mut self, this: &FrameRecord) {
        let skips = self.looked_at.iter().filter(|&&(s, _)| s).count();
        let early = self.burst < SETTLED;
        if skips < SKIPS_TO_TRY || (early && self.tried_early) {
            return;
        }
        self.state = AutoState::Trying {
            before: self.mean_interval(),
            early,
        };
        self.looked_at.clear();
        self.calm = 0;
        if self
            .ended
            .is_some_and(|t| this.start - t < TOO_SOON.as_nanos() as i64)
        {
            self.hold = (self.hold * 2).min(LONGEST_HOLD);
        }
    }

    fn judge_trial(&mut self, before: f64, early: bool, this: &FrameRecord) {
        let frames = if early { EARLY_TRIAL } else { LOOKED_AT };
        if self.looked_at.len() < frames {
            return;
        }
        let needed = if early { CLEAR_GAIN } else { WORTH_IT };
        if self.mean_interval() * needed <= before {
            self.state = AutoState::On;
            self.retry = FIRST_RETRY;
        } else if early && self.mean_interval() * WORTH_IT <= before {
            self.state = AutoState::Off;
            self.tried_early = true;
            self.looked_at.clear();
        } else {
            self.state = AutoState::Unhelpful {
                until: this.start + self.retry.as_nanos() as i64,
            };
            self.retry = (self.retry * 2).min(LONGEST_RETRY);
            self.looked_at.clear();
        }
    }

    fn consider_ending(&mut self, prev: &FrameRecord, this: &FrameRecord) {
        if this.elapsed() as f64 > this.refresh as f64 * CALM_FRACTION {
            self.calm = 0;
            return;
        }
        self.calm += this.start - prev.start;
        if self.calm >= self.hold.as_nanos() as i64 {
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
    /// `Instant` and [`clock`] at the same moment.
    base: (Instant, i64),
    /// Add to presentation times to get [`clock`]'s.
    presentation_offset: Option<i64>,
    auto: AutoAdvance,
    /// `ANTIWEBUI_FRAME_ADVANCE`, for trying out without changing the program.
    forced: Option<FrameAdvance>,
    started: i64,
    callback: Option<i64>,
    ui_done: i64,
    /// Going by the last frame shown.
    refresh: Option<Duration>,
    clock: FrameClock,
    /// `ANTIWEBUI_FRAME_TIME=start` gives animations when frames start, as before there was
    /// [`FrameClock`].
    start_time_only: bool,
    frame: crate::vsync::FrameTime,
}

/// The first refresh after `now`, of those that `last` was the start of one of.
fn next_refresh(last: Instant, refresh: Duration, now: Instant) -> Instant {
    let since = now.saturating_duration_since(last).as_nanos();
    let refreshes = since / refresh.as_nanos().max(1) + 1;
    last + Duration::from_nanos((refreshes * refresh.as_nanos()) as u64)
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
            base: (Instant::now(), clock::monotonic_ns()),
            presentation_offset: None,
            auto: AutoAdvance::default(),
            forced: std::env::var("ANTIWEBUI_FRAME_ADVANCE")
                .ok()
                .and_then(|v| parse_advance(&v)),
            started: 0,
            callback: None,
            ui_done: 0,
            refresh: None,
            clock: FrameClock::default(),
            start_time_only: std::env::var("ANTIWEBUI_FRAME_TIME").is_ok_and(|v| v == "start"),
            frame: crate::vsync::FrameTime {
                time: 0,
                slot: None,
            },
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

    /// How rendering ahead is set and, for `Auto`, what it is doing, as the performance overlay
    /// says it.
    pub fn advance_words(&self) -> String {
        let words = match self
            .forced
            .unwrap_or_else(|| settings().rendering.frame_advance)
        {
            FrameAdvance::Auto => format!(
                "Auto: {}",
                match self.auto.state {
                    AutoState::Off => "off",
                    AutoState::Unhelpful { .. } => "off (no gain)",
                    AutoState::Trying { .. } => "trying",
                    AutoState::On => "on",
                }
            ),
            FrameAdvance::Enable => "Enable".to_owned(),
            FrameAdvance::Disable => "Disable".to_owned(),
            FrameAdvance::Custom { max_advance } => format!("Custom {max_advance}"),
        };
        if self.forced.is_some() {
            format!("{words} (env)")
        } else {
            words
        }
    }

    /// To call first thing in a frame. Passes to `finished` the frames nothing more will be
    /// learned about.
    pub fn begin_frame(&mut self, ctx: &mut impl Host, mut finished: impl FnMut(&FrameRecord)) {
        self.collect(ctx, &mut finished);
        self.started = clock::monotonic_ns();
        let (instant, ns) = self.base;
        self.callback = ctx
            .last_frame_callback(SurfaceId::Window)
            .map(|t| ns + t.saturating_duration_since(instant).as_nanos() as i64);
        self.frame = self.clock.frame_time(self.started, PAUSE_NS);
        if self.start_time_only {
            self.frame.time = self.started;
        }
    }

    /// The time animations go by in this frame: on the refresh grid when frames land on it
    /// steadily, and when the frame started otherwise.
    pub fn frame_time(&self) -> Instant {
        let (instant, ns) = self.base;
        let since = Duration::from_nanos((self.frame.time - ns).max(0) as u64);
        instant + since
    }

    /// To call when the UI's part of the frame is done.
    pub fn trace(&mut self) -> PresentTrace {
        self.ui_done = clock::monotonic_ns();
        self.next_token += 1;
        PresentTrace {
            token: self.next_token,
            ..Default::default()
        }
    }

    /// How often the screen refreshes, going by the last frame shown.
    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// When a refresh was, going by the frames shown. This has none of the wobble of when a frame
    /// callback gets here, nor of times on the screen that come late.
    pub fn last_refresh(&self) -> Option<Instant> {
        let ago = clock::monotonic_ns() - self.clock.refresh_at()?;
        let ago = Duration::from_nanos(ago.max(0) as u64);
        Instant::now().checked_sub(ago)
    }

    /// How long until the frame callback that would come if something had been committed, going
    /// by the last one. `None` before there has been one, or if the refresh isn't known.
    pub fn until_next_refresh(&self, ctx: &mut impl Host, now: Instant) -> Option<Duration> {
        let last = ctx.last_frame_callback(SurfaceId::Window)?;
        let refresh = self.refresh?;
        Some(next_refresh(last, refresh, now).saturating_duration_since(now))
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
                frame_time: self.frame.time,
                slot: self.frame.slot,
                ui_done: self.ui_done,
                diff_done: trace.diff_done,
                buffer_ready: trace.buffer_ready,
                recorded: trace.recorded,
                submitted: trace.submitted,
                committed: trace.committed,
                end: clock::monotonic_ns(),
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

    fn collect(&mut self, ctx: &mut impl Host, finished: &mut impl FnMut(&FrameRecord)) {
        if self.presentation_offset.is_none()
            && let Some(id) = ctx.presentation_clock()
            && let Ok(theirs) = clock::clock_ns(id)
        {
            self.presentation_offset = Some(clock::monotonic_ns() - theirs);
        }
        for p in ctx.take_presentations() {
            self.apply_presentation(&p);
        }
        #[cfg(target_os = "linux")]
        for f in &mut self.flying {
            use std::os::fd::AsFd;
            if let Some(fence) = &f.fence {
                match anti_webui_wgpu::linux_sync::sync_file_signal(fence.as_fd()) {
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
        let now = clock::monotonic_ns();
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
        // Zero means that the output has no constant rate.
        if let Some(s) = p.shown.filter(|s| s.refresh_ns > 0) {
            self.refresh = Some(Duration::from_nanos(s.refresh_ns as u64));
        }
        let offset = self.presentation_offset.unwrap_or(0);
        let f = self.flying.iter_mut().find(|f| f.record.token == p.token);
        if let Some(s) = p.shown {
            let slot = f.as_ref().and_then(|f| f.record.slot);
            self.clock.shown(s.time_ns + offset, s.refresh_ns, slot);
        }
        let Some(f) = f else {
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

    #[test]
    fn the_next_refresh_keeps_in_step_with_the_last_callback() {
        let last = Instant::now();
        let refresh = Duration::from_micros(8333);
        let at = |us| last + Duration::from_micros(us);
        assert_eq!(next_refresh(last, refresh, last), at(8333));
        assert_eq!(next_refresh(last, refresh, at(300)), at(8333));
        assert_eq!(next_refresh(last, refresh, at(8332)), at(8333));
        assert_eq!(next_refresh(last, refresh, at(8333)), at(16666));
        assert_eq!(next_refresh(last, refresh, at(80_000)), at(83330));
        // A clock that says it is earlier than the callback can't happen by much.
        assert_eq!(next_refresh(at(100), refresh, last), at(8433));
    }

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
        let mut t = from;
        for _ in 0..=SETTLED {
            t = feed(auto, t, 1, 10 * MS, 2 * REFRESH);
            if auto.engaged() {
                return t;
            }
        }
        panic!("no trial");
    }

    /// Frames on the screen every other refresh, for `wait`.
    fn keep_skipping(auto: &mut AutoAdvance, from: i64, wait: Duration) -> i64 {
        let count = wait.as_nanos() as i64 / (2 * REFRESH);
        feed(auto, from, count, 10 * MS, 2 * REFRESH)
    }

    /// Frames on the screen every `step`, until the trial has been judged.
    fn finish_trial(auto: &mut AutoAdvance, from: i64, step: i64) -> i64 {
        let mut t = from;
        for _ in 0..=LOOKED_AT {
            t = feed(auto, t, 1, 10 * MS, step);
            if !matches!(auto.state, AutoState::Trying { .. }) {
                return t;
            }
        }
        panic!("no verdict");
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
        feed(&mut auto, 0, WARM_UP as i64, 10 * MS, 2 * REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_trial_starts_soon_after_the_first_frames() {
        let mut auto = AutoAdvance::default();
        feed(
            &mut auto,
            0,
            (WARM_UP + SKIPS_TO_TRY + 2) as i64,
            10 * MS,
            2 * REFRESH,
        );
        assert!(auto.engaged());
    }

    #[test]
    fn a_pause_starts_a_burst_over() {
        let mut auto = AutoAdvance::default();
        let t = feed(&mut auto, 0, WARM_UP as i64, 10 * MS, 2 * REFRESH);
        let mut after_pause = frame(t + 200 * MS, 10 * MS, t + 220 * MS);
        after_pause.callback = Some(t);
        auto.observe(&frame(t - 2 * REFRESH, 10 * MS, t), &after_pause);
        feed(
            &mut auto,
            t + 300 * MS,
            WARM_UP as i64,
            10 * MS,
            2 * REFRESH,
        );
        assert!(!auto.engaged());
    }

    #[test]
    fn a_frame_starting_a_little_late_is_not_a_pause() {
        let mut auto = AutoAdvance::default();
        let t = feed(&mut auto, 0, 10, 2 * MS, REFRESH);
        let mut late = frame(t + 5 * MS, 2 * MS, t + 2 * REFRESH);
        late.callback = Some(t);
        auto.observe(&frame(t - REFRESH, 2 * MS, t), &late);
        assert_eq!(auto.burst, 10);
    }

    #[test]
    fn an_odd_stall_does_not_make_a_trial_look_good() {
        let mut auto = AutoAdvance::default();
        // Frames on the screen every other refresh, also during the trial, apart from a long
        // stall before it. Without a limit, that would make the rate look better afterwards.
        let t = feed(&mut auto, 0, WARM_UP as i64 + 3, 10 * MS, 2 * REFRESH);
        let t = feed(&mut auto, t, 1, 10 * MS, 500 * MS);
        let t = start_trial(&mut auto, t);
        finish_trial(&mut auto, t, 2 * REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_clear_gain_stays_right_away() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        feed(&mut auto, t, EARLY_TRIAL as i64 + 1, 10 * MS, REFRESH);
        assert_eq!(auto.state, AutoState::On);
    }

    #[test]
    fn a_small_early_gain_is_tried_again_once_settled() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        // 1.3 times as fast: frames after a pause could look like that without any real gain.
        let step = 2 * REFRESH * 10 / 13;
        let t = feed(&mut auto, t, EARLY_TRIAL as i64 + 1, 10 * MS, step);
        assert!(!auto.engaged());
        let frames = SETTLED - auto.burst - 1;
        let t = feed(&mut auto, t, frames as i64, 10 * MS, 2 * REFRESH);
        assert!(!auto.engaged(), "not settled yet");
        let t = feed(&mut auto, t, 3, 10 * MS, 2 * REFRESH);
        assert!(auto.engaged());
        // Now the same gain is enough, once it has been seen for longer.
        let t = feed(&mut auto, t, EARLY_TRIAL as i64 + 1, 10 * MS, step);
        assert!(matches!(auto.state, AutoState::Trying { .. }));
        finish_trial(&mut auto, t, step);
        assert_eq!(auto.state, AutoState::On);
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
        let t = start_trial(&mut auto, t);
        assert_eq!(auto.hold, FIRST_HOLD * 2);
        let t = finish_trial(&mut auto, t, REFRESH);
        let t = feed(&mut auto, t, 3 * per_second, 2 * MS, REFRESH);
        assert!(auto.engaged(), "2 s are no longer enough");
        feed(&mut auto, t, per_second + 2, 2 * MS, REFRESH);
        assert!(!auto.engaged());
    }

    #[test]
    fn a_long_pause_keeps_rendering_ahead() {
        let mut auto = AutoAdvance::default();
        let t = turned_on(&mut auto);
        feed(&mut auto, t + 100_000 * MS, 2, 10 * MS, REFRESH);
        assert_eq!(auto.state, AutoState::On);
    }

    #[test]
    fn a_long_pause_forgets_that_it_did_not_help() {
        let mut auto = AutoAdvance::default();
        let t = start_trial(&mut auto, 0);
        let t = finish_trial(&mut auto, t, 2 * REFRESH);
        assert!(matches!(auto.state, AutoState::Unhelpful { .. }));
        let t = t + 6000 * MS;
        feed(&mut auto, t, 2, 10 * MS, 2 * REFRESH);
        assert_eq!(auto.state, AutoState::Off);
        assert_eq!(auto.retry, FIRST_RETRY);
    }

    /// A pause, and then frames that fit in a refresh for `wait`.
    fn light_burst(auto: &mut AutoAdvance, from: i64, wait: Duration) -> i64 {
        let t = from + 1000 * MS;
        let mut after_pause = frame(t, 2 * MS, t + REFRESH);
        after_pause.callback = Some(from);
        auto.observe(&frame(from - REFRESH, 2 * MS, from), &after_pause);
        // The first frames of a burst aren't looked at.
        let warm_up = (WARM_UP + 1) as i64;
        let count = warm_up + wait.as_nanos() as i64 / REFRESH;
        feed(auto, t + REFRESH, count, 2 * MS, REFRESH)
    }

    #[test]
    fn pauses_do_not_count_as_frames_fitting() {
        let mut auto = AutoAdvance::default();
        let t = turned_on(&mut auto);
        let t = light_burst(&mut auto, t, Duration::from_millis(500));
        // The pause before this one was longer than the wait.
        light_burst(&mut auto, t + 10_000 * MS, Duration::from_millis(500));
        assert_eq!(auto.state, AutoState::On);
    }

    #[test]
    fn short_bursts_of_light_frames_add_up() {
        let mut auto = AutoAdvance::default();
        let mut t = turned_on(&mut auto);
        for _ in 0..3 {
            t = light_burst(&mut auto, t, Duration::from_millis(500));
            assert_eq!(auto.state, AutoState::On);
        }
        light_burst(&mut auto, t, Duration::from_millis(600));
        assert_eq!(auto.state, AutoState::Off);
    }

    #[test]
    fn the_first_frames_after_a_pause_do_not_start_the_wait_over() {
        let mut auto = AutoAdvance::default();
        let t = turned_on(&mut auto);
        let t = light_burst(&mut auto, t, Duration::from_millis(1500));
        let calm = auto.calm;
        // Slow, as frames are right after a pause.
        let at = t + 1000 * MS;
        let mut after_pause = frame(at, 10 * MS, at + REFRESH);
        after_pause.callback = Some(t);
        auto.observe(&frame(t - REFRESH, 2 * MS, t), &after_pause);
        feed(&mut auto, at + REFRESH, WARM_UP as i64, 7 * MS, REFRESH);
        assert_eq!(auto.calm, calm);
    }

    #[test]
    fn a_frame_a_little_over_half_a_refresh_is_not_clearly_fitting() {
        let mut auto = AutoAdvance::default();
        let t = turned_on(&mut auto);
        let elapsed = REFRESH * 6 / 10;
        feed(&mut auto, t, 10_000, elapsed, REFRESH);
        assert_eq!(auto.state, AutoState::On);
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
