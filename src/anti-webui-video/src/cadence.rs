//! Which frame goes with which refresh of the screen. When frames take a whole number of refreshes,
//! the time alone would show them unevenly: with a frame ending right at a refresh, the slightest
//! wobble of the clock shows it once, then three times. So then the count of refreshes decides, for
//! as long as that stays near the time. This is `VideoCadenceEstimator` of Chromium for N = 1. See
//! `docs/design/24-video.md`.

/// A cadence has to last this long before the frames have drifted too far from the time to go on.
/// As in Chromium.
const MINIMUM_RUN: f64 = 8.0;

/// How many refreshes each frame gets, if it is the same for all, and it stays near the time.
pub fn refreshes_per_frame(frame: f64, refresh: f64) -> Option<u32> {
    if frame <= 0.0 || refresh <= 0.0 {
        return None;
    }
    let n = (frame / refresh).round();
    if n < 1.0 {
        return None;
    }
    let drift_per_frame = (frame - n * refresh).abs();
    let drift_of_run = drift_per_frame * (MINIMUM_RUN / frame);
    // More than a refresh away from where it should be is what can be seen.
    (drift_of_run < refresh).then_some(n as u32)
}

/// Decides for one stream.
#[derive(Clone, Debug, Default)]
pub struct Cadence {
    /// The refresh the current frame went up at, while counting.
    shown_at: Option<i64>,
    /// Refreshes per frame, while counting.
    counting: Option<u32>,
}

impl Cadence {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// How many frames to go on by. `tick` is which refresh the frame being made will show at, and
    /// `media` where the clock will be then. `current` is the time of the frame showing, `next` the
    /// times of the frames after that, and `frame` how long they take, all in seconds.
    pub fn advance(
        &mut self,
        tick: Option<i64>,
        media: f64,
        refresh: Option<f64>,
        frame: f64,
        current: Option<f64>,
        next: &[f64],
    ) -> usize {
        let by_time = next.iter().take_while(|&&pts| pts <= media).count();
        let per_frame = refresh.and_then(|r| refreshes_per_frame(frame, r));
        let (Some(tick), Some(n), Some(current), Some(shown_at)) =
            (tick, per_frame, current, self.shown_at)
        else {
            self.counting = per_frame;
            if by_time > 0 || self.shown_at.is_none() {
                self.shown_at = tick;
            }
            return by_time;
        };
        self.counting = Some(n);
        let by_count = ((tick - shown_at).max(0) / n as i64) as usize;
        let by_count = by_count.min(next.len());
        // Where that leaves it, against the time.
        let would_show = if by_count == 0 {
            current
        } else {
            next[by_count - 1]
        };
        if (media - would_show) >= frame * 1.5 || (would_show - media) > frame * 0.5 {
            // Too far from the clock: start counting over from there.
            if by_time > 0 {
                self.shown_at = Some(tick);
            }
            return by_time;
        }
        if by_count > 0 {
            self.shown_at = Some(shown_at + (by_count as u32 * n) as i64);
        }
        by_count
    }

    /// The refresh at which [`Cadence::advance`] goes on, while counting.
    pub fn next_tick(&self) -> Option<i64> {
        Some(self.shown_at? + self.counting? as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small and repeatable wobble in seconds, within `size`.
    fn wobble(i: u64, size: f64) -> f64 {
        let x = i.wrapping_mul(2654435761).wrapping_mul(i ^ 0x5bd1e995) >> 40;
        ((x % 2001) as f64 / 1000.0 - 1.0) * size
    }

    /// How many refreshes each of the frames shows for, on a screen with a little wobble in when
    /// frames start and a little in the clock.
    fn play(fps: f64, hz: f64, seconds: f64, known_refresh: bool, jitter: f64) -> Vec<u32> {
        let (frame, refresh) = (1.0 / fps, 1.0 / hz);
        let total = (seconds * fps) as usize;
        let times: Vec<f64> = (0..total).map(|i| i as f64 * frame).collect();
        let mut cadence = Cadence::default();
        let mut index = 0;
        let mut counts = vec![0u32; total];
        counts[0] = 1;
        let refreshes = (seconds * hz) as u64;
        for r in 1..refreshes {
            let now = r as f64 * refresh + wobble(r, jitter);
            let media = r as f64 * refresh + wobble(r + 99, jitter);
            let next = &times[(index + 1)..(index + 6).min(total)];
            if next.is_empty() {
                break;
            }
            let by = cadence.advance(
                Some((now / refresh).round() as i64),
                media,
                known_refresh.then_some(refresh),
                frame,
                Some(times[index]),
                next,
            );
            index += by;
            counts[index] += 1;
        }
        // The ends aren't what this is about.
        counts[2..index.saturating_sub(2)].to_vec()
    }

    #[test]
    fn cadences() {
        assert_eq!(refreshes_per_frame(1.0 / 30.0, 1.0 / 60.0), Some(2));
        assert_eq!(refreshes_per_frame(1.0 / 24.0, 1.0 / 120.0), Some(5));
        assert_eq!(refreshes_per_frame(1.0 / 60.0, 1.0 / 60.0), Some(1));
        assert_eq!(refreshes_per_frame(1001.0 / 30000.0, 1.0 / 60.0), Some(2));
        assert_eq!(refreshes_per_frame(1001.0 / 24000.0, 1.0 / 120.0), Some(5));
        // These are 3:2, and less than one for each.
        assert_eq!(refreshes_per_frame(1.0 / 24.0, 1.0 / 60.0), None);
        assert_eq!(refreshes_per_frame(1.0 / 120.0, 1.0 / 60.0), None);
        assert_eq!(refreshes_per_frame(1.0 / 30.0, 0.0), None);
    }

    #[test]
    fn whole_numbers_of_refreshes_are_even_with_a_wobbly_clock() {
        for (fps, hz, n) in [
            (30.0, 60.0, 2),
            (60.0, 60.0, 1),
            (24.0, 120.0, 5),
            (60.0, 120.0, 2),
            (30.0, 120.0, 4),
        ] {
            let counts = play(fps, hz, 10.0, true, 0.002);
            assert!(
                counts.iter().all(|&c| c == n),
                "{fps} fps at {hz} Hz: {counts:?}"
            );
        }
    }

    /// What this is here for: without knowing the refresh, the same wobble shows them unevenly.
    #[test]
    fn the_time_alone_is_uneven_with_a_wobbly_clock() {
        let counts = play(30.0, 60.0, 10.0, false, 0.002);
        assert!(counts.iter().any(|&c| c != 2), "{counts:?}");
    }

    #[test]
    fn three_two_comes_from_the_time() {
        let counts = play(24.0, 60.0, 10.0, true, 0.001);
        let twos = counts.iter().filter(|&&c| c == 2).count();
        let threes = counts.iter().filter(|&&c| c == 3).count();
        assert_eq!(twos + threes, counts.len(), "{counts:?}");
        assert!(twos.abs_diff(threes) <= 2, "{twos} and {threes}");
    }

    #[test]
    fn frames_faster_than_the_screen_skip() {
        // Each refresh still shows one that is due, without falling behind.
        let frame = 1.0 / 120.0;
        let refresh = 1.0 / 60.0;
        let times: Vec<f64> = (0..240).map(|i| i as f64 * frame).collect();
        let mut cadence = Cadence::default();
        let mut index = 0;
        for r in 1..100 {
            let t = r as f64 * refresh;
            index += cadence.advance(
                Some(r),
                t,
                Some(refresh),
                frame,
                Some(times[index]),
                &times[index + 1..(index + 10).min(240)],
            );
            assert!((times[index] - t).abs() < frame * 2.0, "{r}: {index}");
        }
    }

    #[test]
    fn a_stall_catches_up_with_the_time() {
        let (frame, refresh) = (1.0 / 30.0, 1.0 / 60.0);
        let times: Vec<f64> = (0..600).map(|i| i as f64 * frame).collect();
        let mut cadence = Cadence::default();
        let mut index = 0;
        let mut step = |t: f64, index: &mut usize| {
            *index += cadence.advance(
                Some((t / refresh).round() as i64),
                t,
                Some(refresh),
                frame,
                Some(times[*index]),
                &times[*index + 1..(*index + 8).min(600)],
            );
        };
        for r in 1..60 {
            step(r as f64 * refresh, &mut index);
        }
        // The UI thread was busy for half a second. Only so much is ever ready to go on to.
        step(1.5, &mut index);
        assert!(times[index] > 1.1, "{}", times[index]);
        for r in 1..30 {
            step(1.5 + r as f64 * refresh, &mut index);
        }
        let t = 1.5 + 29.0 * refresh;
        assert!((times[index] - t).abs() <= frame * 2.0, "{}", times[index]);
    }

    #[test]
    fn it_says_when_it_goes_on_while_counting() {
        let (frame, refresh) = (1.0 / 30.0, 1.0 / 60.0);
        let mut cadence = Cadence::default();
        let times = [0.0, frame, frame * 2.0];
        assert_eq!(cadence.next_tick(), None);
        cadence.advance(
            Some(7),
            0.0,
            Some(refresh),
            frame,
            Some(times[0]),
            &times[1..],
        );
        assert_eq!(cadence.next_tick(), Some(9));
        // Without knowing about refreshes there is nothing to count.
        let mut cadence = Cadence::default();
        cadence.advance(None, 0.0, None, frame, Some(times[0]), &times[1..]);
        assert_eq!(cadence.next_tick(), None);
    }
}
