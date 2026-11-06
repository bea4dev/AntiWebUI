//! The time animations are given: on the screen's refresh grid when frames land on it steadily, so
//! that a frame callback that comes late doesn't move them by how late it was (see
//! `docs/design/10-frame-pacing.md`).

use std::collections::VecDeque;

/// How many times on the screen are kept: about a second at 120 Hz.
const SHOWN: usize = 128;
/// Times on the screen can only be late, never early, so the grid goes through the earliest of the
/// last ones...
const RECENT: usize = 32;
/// ...and its period comes from the earliest of the first and the last of those kept, if they are
/// at least this many refreshes apart. What the compositor says can be rounded (8.333 ms for a
/// 119.88 Hz screen, 9 µs a refresh, 1 ms a second).
const PERIOD_SPAN: f64 = 48.0;
/// Where frames start in a refresh, in this many parts...
const PHASES: usize = 32;
/// ...counting the last frames most: a frame weighs half as much after about 35 more.
const PHASE_DECAY: f32 = 0.98;
/// Each part counts with this many on either side, so that the frames go to the refresh they
/// start in when the widest gap between them is the boundary.
const PHASE_SPREAD: usize = 2;
/// The boundary moves only to somewhere this much emptier, so that it doesn't wander.
const MOVE_BOUNDARY: f32 = 2.0;
/// How far frames start after the refresh they go to, on average: by every frame at first...
const SETTLING: u32 = 64;
/// ...then only a little, so that steps between frames stay whole refreshes.
const OFFSET_RATE: f64 = 1.0 / 256.0;
/// How many frames in a row have to reach the screen the same number of refreshes after their
/// place on the grid. Otherwise where a frame goes isn't known at its start, and the start is the
/// better guess.
const STEADY: usize = 8;

/// The screen's refresh grid and where frames go on it. Times are `CLOCK_MONOTONIC` nanoseconds.
#[derive(Debug, Default)]
pub(crate) struct FrameClock {
    shown: VecDeque<i64>,
    /// What the compositor says.
    refresh: Option<i64>,
    phases: [f32; PHASES],
    boundary: Option<usize>,
    /// How far frames start after their place on the grid.
    offset: f64,
    settled: u32,
    /// Refreshes from a frame's place on the grid to the screen, for the last frames shown.
    latencies: VecDeque<i64>,
    last_start: Option<i64>,
    last_time: Option<i64>,
}

/// What [`FrameClock::frame_time`] says about a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameTime {
    /// For animations.
    pub time: i64,
    /// The frame's place on the grid, to compare with when it reaches the screen.
    pub slot: Option<i64>,
}

/// Where `seg`'s earliest time puts the refresh of its last one.
fn earliest(seg: impl Iterator<Item = i64> + Clone, period: f64) -> Option<f64> {
    let last = seg.clone().last()? as f64;
    seg.map(|t| {
        let t = t as f64;
        t + ((last - t) / period).round() * period
    })
    .min_by(f64::total_cmp)
}

impl FrameClock {
    /// A frame reached the screen at `time`, `refresh` apart from the next (0 if not constant).
    pub fn shown(&mut self, time: i64, refresh: u32, slot: Option<i64>) {
        if refresh > 0 {
            let refresh = refresh as i64;
            // Another mode: what came before is on another grid.
            if self.refresh.is_some_and(|r| (r - refresh).abs() * 100 > r) {
                *self = Self::default();
            }
            self.refresh = Some(refresh);
        }
        if self.shown.len() == SHOWN {
            self.shown.pop_front();
        }
        self.shown.push_back(time);
        if let (Some(slot), Some(period)) = (slot, self.period()) {
            if self.latencies.len() == STEADY {
                self.latencies.pop_front();
            }
            self.latencies
                .push_back(((time - slot) as f64 / period).round() as i64);
        }
    }

    fn period(&self) -> Option<f64> {
        let said = self.refresh? as f64;
        if self.shown.len() < SHOWN {
            return Some(said);
        }
        // Projecting with a period that is off favours the oldest times, so once more with the
        // better one.
        let mut period = said;
        for _ in 0..2 {
            let first = earliest(self.shown.iter().copied().take(RECENT), period)?;
            let last = earliest(self.shown.iter().copied().skip(SHOWN - RECENT), period)?;
            let refreshes = ((last - first) / period).round();
            if refreshes < PERIOD_SPAN {
                return Some(said);
            }
            period = (last - first) / refreshes;
        }
        Some(period)
    }

    /// A refresh: the last one the screen is known to have had.
    pub fn refresh_at(&self) -> Option<i64> {
        let period = self.period()?;
        let recent = self.shown.iter().copied().skip(self.shown.len().saturating_sub(RECENT));
        earliest(recent, period).map(|t| t as i64)
    }

    /// The time for animations in a frame that starts at `start`.
    pub fn frame_time(&mut self, start: i64, pause: i64) -> FrameTime {
        if self.last_start.is_some_and(|t| start - t > pause) {
            // Frames that come after a pause don't land like those before it.
            self.latencies.clear();
        }
        self.last_start = Some(start);
        let slot = self.place(start);
        let steady = self.latencies.len() == STEADY
            && self.latencies.iter().all(|&l| l == self.latencies[0]);
        let time = match slot {
            Some(slot) if steady => slot + self.offset.round() as i64,
            _ => start,
        };
        let time = self.last_time.map_or(time, |last| time.max(last));
        self.last_time = Some(time);
        FrameTime { time, slot }
    }

    /// The refresh a frame starting at `start` goes to.
    fn place(&mut self, start: i64) -> Option<i64> {
        if self.shown.len() < 4 {
            return None;
        }
        let period = self.period()?;
        let grid = self.refresh_at()? as f64;
        let start = start as f64;
        let part = |t: f64| ((t - grid).rem_euclid(period) / period * PHASES as f64) as usize;
        for p in &mut self.phases {
            *p *= PHASE_DECAY;
        }
        self.phases[part(start).min(PHASES - 1)] += 1.0;
        let around = |i: usize| -> f32 {
            (0..=2 * PHASE_SPREAD)
                .map(|j| self.phases[(i + PHASES + j - PHASE_SPREAD) % PHASES])
                .sum()
        };
        let emptiest = (0..PHASES)
            .min_by(|&a, &b| around(a).total_cmp(&around(b)))
            .expect("there are parts");
        match self.boundary {
            Some(b) if around(b) <= MOVE_BOUNDARY * around(emptiest) + 0.5 => {}
            _ => {
                self.boundary = Some(emptiest);
                self.settled = 0;
            }
        }
        let boundary = (self.boundary? as f64 + 0.5) / PHASES as f64 * period;
        let edge = grid + boundary;
        let slot = edge + ((start - edge) / period).floor() * period;
        let after = start - slot;
        self.settled = (self.settled + 1).min(SETTLING + 1);
        if self.settled == 1 {
            self.offset = after;
        } else {
            let rate = if self.settled <= SETTLING {
                1.0 / self.settled as f64
            } else {
                OFFSET_RATE
            };
            self.offset += rate * (after - self.offset);
        }
        Some(slot as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;
    /// A 119.88 Hz screen that says it is 120 Hz.
    const PERIOD: i64 = 8_341_675;
    const SAID: u32 = 8_333_333;

    /// Some lateness that looks random but isn't.
    fn late(i: i64, most: i64) -> i64 {
        (i * 7919 % 101) * most / 100
    }

    /// Frames starting `wait` after each refresh give or take `jitter`, on the screen 3 refreshes
    /// later give or take `shown_jitter`.
    fn run(
        clock: &mut FrameClock,
        frames: std::ops::Range<i64>,
        wait: i64,
        jitter: i64,
        shown_jitter: i64,
    ) -> Vec<(i64, FrameTime)> {
        let mut out = Vec::new();
        let mut pending = VecDeque::new();
        for i in frames {
            let vblank = i * PERIOD;
            while let Some(&(at, slot)) = pending.front() {
                if at > vblank + wait {
                    break;
                }
                pending.pop_front();
                clock.shown(at, SAID, slot);
            }
            let start = vblank + wait + late(i, jitter);
            let t = clock.frame_time(start, 100 * MS);
            out.push((i, t));
            pending.push_back((vblank + 3 * PERIOD + late(i + 13, shown_jitter), t.slot));
        }
        out
    }

    #[test]
    fn steps_are_whole_refreshes_however_late_frames_start() {
        let mut clock = FrameClock::default();
        let frames = run(&mut clock, 0..400, MS, 6 * MS, MS);
        for pair in frames[200..].windows(2) {
            let step = pair[1].1.time - pair[0].1.time;
            // The earliest of the last times on the screen changes now and then.
            assert!((step - PERIOD).abs() < 100_000, "step {step}");
        }
    }

    #[test]
    fn frame_times_are_starts_on_average() {
        let mut clock = FrameClock::default();
        let frames = run(&mut clock, 0..400, 2 * MS, 4 * MS, 0);
        let mean: i64 = frames[200..]
            .iter()
            .map(|(i, t)| t.time - (i * PERIOD + 2 * MS + late(*i, 4 * MS)))
            .sum::<i64>()
            / 200;
        assert!(mean.abs() < 300_000, "off by {mean} on average");
    }

    #[test]
    fn the_period_comes_from_the_screen() {
        let mut clock = FrameClock::default();
        run(&mut clock, 0..400, MS, MS, 2 * MS);
        let period = clock.period().unwrap();
        assert!((period - PERIOD as f64).abs() < 300.0, "{period}");
        let refresh = clock.refresh_at().unwrap();
        assert!(refresh.rem_euclid(PERIOD) < 50_000);
    }

    #[test]
    fn starts_are_used_until_frames_land_steadily() {
        let mut clock = FrameClock::default();
        let frames = run(&mut clock, 0..5, MS, 0, 0);
        assert!(frames.iter().all(|(i, t)| t.time == i * PERIOD + MS));
    }

    #[test]
    fn starts_are_used_after_a_pause() {
        let mut clock = FrameClock::default();
        run(&mut clock, 0..300, MS, 3 * MS, 0);
        let frames = run(&mut clock, 400..403, MS, 3 * MS, 0);
        for (i, t) in frames {
            assert_eq!(t.time, i * PERIOD + MS + late(i, 3 * MS));
        }
    }

    #[test]
    fn starts_are_used_when_frames_land_unevenly() {
        let mut clock = FrameClock::default();
        run(&mut clock, 0..300, MS, MS, 0);
        // The compositor takes some frames a refresh later than others.
        for (n, slot) in [0, 1, 0, 1, 1, 0, 0, 1].into_iter().enumerate() {
            clock.shown(n as i64 * PERIOD, SAID, Some((n as i64 - 3 - slot) * PERIOD));
        }
        let start = 301 * PERIOD + MS + 123;
        assert_eq!(clock.frame_time(start, 100 * MS).time, start);
    }

    #[test]
    fn time_never_goes_back() {
        let mut clock = FrameClock::default();
        let frames = run(&mut clock, 0..400, MS, 6 * MS, MS);
        assert!(frames.windows(2).all(|p| p[1].1.time >= p[0].1.time));
    }
}
