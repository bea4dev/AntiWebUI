//! The speed of a touchpad gesture and the inertia after it, for platforms that leave inertia to
//! the application (see `docs/design/08-scrolling-and-settings.md`).

use crate::settings::KineticSettings;
use std::time::Instant;

/// How many recent events the speed is computed from.
const SAMPLES: usize = 3;
/// Fingers that rested this long before leaving had stopped on purpose (milliseconds).
const LIFT_TIMEOUT: u32 = 200;
/// Logical pixels per second. Slower gestures have no inertia.
const MIN_SPEED: f32 = 30.0;
/// The time inertia takes is the speed over this, up to [`MAX_DURATION`]. Compared with the
/// deceleration in the settings, which gives the distance, as Chromium's constants are.
const DURATION_DECELERATION: f32 = 2300.0;
const DEFAULT_DECELERATION: f32 = 96.0 * 27.559;
const MAX_DURATION: f32 = 2.0;
/// A fling in progress is added to by the next one when the fingers come back this soon, moving
/// this fast (milliseconds and logical pixels per second).
const BOOST_TIMEOUT: u32 = 50;
const BOOST_MIN_MOTION: f32 = 150.0;
const BOOST_MIN_FLING: f32 = 350.0;
/// The speed inertia starts with, relative to the content's speed under the fingers. Chromium's
/// newer scaling moves content 2.5 times the axis speed and flings at the axis speed.
pub(crate) const FLING_SPEED_RATIO: f32 = 0.4;

#[derive(Clone, Copy, Debug)]
struct Sample {
    time: u32,
    dx: f32,
    dy: f32,
}

#[derive(Debug, Default)]
pub(crate) struct VelocityTracker {
    samples: Vec<Sample>,
}

impl VelocityTracker {
    pub fn record(&mut self, time: u32, dx: f32, dy: f32) {
        if self
            .samples
            .last()
            .is_some_and(|s| time.wrapping_sub(s.time) > LIFT_TIMEOUT)
        {
            self.samples.clear();
        }
        if self.samples.len() == SAMPLES {
            self.samples.remove(0);
        }
        self.samples.push(Sample { time, dx, dy });
    }

    /// Milliseconds from the last motion to `lift`.
    pub fn since_last(&self, lift: u32) -> u32 {
        self.samples.last().map_or(0, |s| lift.wrapping_sub(s.time))
    }

    /// Physical pixels per second when the fingers left at `lift`: the slope of the least-squares
    /// line through position over time. `None` if there is nothing to go by.
    pub fn velocity(&self, lift: u32) -> Option<(f32, f32)> {
        let first = self.samples.first()?;
        let last = self.samples.last()?;
        if lift.wrapping_sub(last.time) > LIFT_TIMEOUT {
            return None;
        }
        let n = self.samples.len() as f32;
        let (mut x, mut y) = (0.0, 0.0);
        let (mut st, mut stt, mut sx, mut sy, mut stx, mut sty) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for s in &self.samples {
            let t = s.time.wrapping_sub(first.time) as f32 / 1000.0;
            x += s.dx;
            y += s.dy;
            st += t;
            stt += t * t;
            sx += x;
            sy += y;
            stx += t * x;
            sty += t * y;
        }
        let det = n * stt - st * st;
        if det <= 0.0 {
            return None;
        }
        Some(((n * stx - st * sx) / det, (n * sty - st * sy) / det))
    }
}

/// Motion after the fingers leave: Chromium's `PhysicsBasedFlingCurve`. Position is a function of
/// time, so dropped frames do not change the path.
#[derive(Debug)]
pub(crate) struct Fling {
    start: Instant,
    /// Logical pixels per second, as it started.
    velocity: (f32, f32),
    /// Logical pixels per second at the last step.
    speed: f32,
    /// Where it ends, in physical pixels from the start.
    end: (f32, f32),
    /// Seconds.
    duration: f32,
    p1: (f32, f32),
    scale: f32,
    last_time: f32,
    /// Physical pixels moved so far.
    moved: (f32, f32),
}

impl Fling {
    /// `velocity` is in logical pixels per second, `boost` is how much it was added to by earlier
    /// flings, and `viewport` is the window in logical pixels. `None` if it is too slow to go on.
    pub fn new(
        velocity: (f32, f32),
        boost: f32,
        settings: &KineticSettings,
        viewport: (f32, f32),
        scale: f32,
        start: Instant,
    ) -> Option<Self> {
        let velocity = (
            velocity.0 * settings.velocity_factor,
            velocity.1 * settings.velocity_factor,
        );
        let top = velocity.0.abs().max(velocity.1.abs());
        if !top.is_finite() || top < MIN_SPEED {
            return None;
        }
        let deceleration = settings.deceleration.max(1.0);
        let bound = settings.max_screens.max(0.0) * boost;
        let distance = |v: f32, size: f32| (v * v / (2.0 * deceleration)).min(size * bound);
        let (dx, dy) = (
            distance(velocity.0, viewport.0).copysign(velocity.0),
            distance(velocity.1, viewport.1).copysign(velocity.1),
        );
        let duration =
            (top / (deceleration * DURATION_DECELERATION / DEFAULT_DECELERATION)).min(MAX_DURATION);
        // The curve starts at the speed of the fingers: raise the first control point until it
        // reaches the top, then move it to the left.
        let along = if velocity.1.abs() == top { dy } else { dx };
        let slope = top * duration / along.abs().max(f32::MIN_POSITIVE);
        let p1 = if slope * 0.2 < 1.0 {
            (0.2, 0.2 * slope)
        } else {
            (1.0 / slope, 1.0)
        };
        Some(Self {
            start,
            velocity,
            speed: (velocity.0).hypot(velocity.1),
            end: (dx * scale, dy * scale),
            duration,
            p1,
            scale,
            last_time: 0.0,
            moved: (0.0, 0.0),
        })
    }

    /// Physical pixels to move since the last step, and whether this is the last one.
    pub fn step(&mut self, now: Instant) -> (f32, f32, bool) {
        let t = now.saturating_duration_since(self.start).as_secs_f32();
        let done = t >= self.duration;
        let progress = if done {
            1.0
        } else {
            ease(self.p1, t / self.duration)
        };
        let at = (self.end.0 * progress, self.end.1 * progress);
        let delta = (at.0 - self.moved.0, at.1 - self.moved.1);
        if t > self.last_time {
            self.speed = delta.0.hypot(delta.1) / self.scale / (t - self.last_time);
            self.last_time = t;
        }
        self.moved = at;
        (delta.0, delta.1, done)
    }
}

/// The cubic bezier from (0, 0) to (1, 1) through `p1` and (0.55, 1): the `y` for an `x` in 0..1.
fn ease(p1: (f32, f32), x: f32) -> f32 {
    const P2: (f32, f32) = (0.55, 1.0);
    let curve = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    // `x` grows with `t`, so bisection finds it whatever the slope.
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..24 {
        let t = (low + high) / 2.0;
        if curve(p1.0, P2.0, t) < x {
            low = t;
        } else {
            high = t;
        }
    }
    curve(p1.1, P2.1, (low + high) / 2.0)
}

/// What is left of a fling that fingers interrupted: if they flick the same way right away, the
/// new fling is added to it (Chromium's `FlingBooster`).
#[derive(Debug)]
pub(crate) struct Booster {
    velocity: (f32, f32),
    speed: f32,
    cutoff: u32,
    last_motion: Option<u32>,
}

impl Booster {
    pub fn new(fling: &Fling, time: u32) -> Self {
        Self {
            velocity: fling.velocity,
            speed: fling.speed,
            cutoff: time.wrapping_add(BOOST_TIMEOUT),
            last_motion: None,
        }
    }

    fn timed_out(&self, time: u32) -> bool {
        (time.wrapping_sub(self.cutoff) as i32) > 0
    }

    fn same_way(&self, x: f32, y: f32) -> bool {
        self.velocity.0 * x + self.velocity.1 * y > 0.0
    }

    /// Takes motion of the new gesture in logical pixels. Returns whether it can still boost.
    pub fn motion(&mut self, time: u32, dx: f32, dy: f32) -> bool {
        if self.timed_out(time) || !self.same_way(dx, dy) {
            return false;
        }
        if let Some(last) = self.last_motion {
            let dt = time.wrapping_sub(last) as f32 / 1000.0;
            if dt >= 0.001 && dx.hypot(dy) / dt < BOOST_MIN_MOTION {
                return false;
            }
        }
        self.last_motion = Some(time);
        self.cutoff = time.wrapping_add(BOOST_TIMEOUT);
        true
    }

    /// The new fling's velocity and how much was added to it, when the fingers left at `time`.
    pub fn boost(&self, time: u32, velocity: (f32, f32)) -> Option<((f32, f32), f32)> {
        if self.timed_out(time)
            || !self.same_way(velocity.0, velocity.1)
            || self.speed < BOOST_MIN_FLING
            || velocity.0.hypot(velocity.1) < BOOST_MIN_FLING
        {
            return None;
        }
        let boosted = (velocity.0 + self.velocity.0, velocity.1 + self.velocity.1);
        let top = |v: (f32, f32)| v.0.abs().max(v.1.abs());
        Some((boosted, top(boosted) / top(velocity)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use std::time::Duration;

    fn kinetic() -> KineticSettings {
        Settings::default().scroll.kinetic
    }

    fn tracked(samples: &[(u32, f32)]) -> VelocityTracker {
        let mut v = VelocityTracker::default();
        for &(t, d) in samples {
            v.record(t, 0.0, d);
        }
        v
    }

    #[test]
    fn velocity_of_even_motion() {
        // 10 pixels every 10 milliseconds.
        let v = tracked(&[(100, 10.0), (110, 10.0), (120, 10.0)]);
        let (x, y) = v.velocity(125).unwrap();
        assert_eq!(x, 0.0);
        assert!((y - 1000.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn velocity_follows_the_latest_events() {
        let v = tracked(&[
            (100, 90.0),
            (110, 90.0),
            (120, 10.0),
            (130, 10.0),
            (140, 10.0),
        ]);
        let (_, y) = v.velocity(140).unwrap();
        assert!((y - 1000.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn velocity_needs_recent_motion_over_time() {
        let v = tracked(&[(100, 10.0), (110, 10.0)]);
        assert!(v.velocity(310).is_some());
        assert!(v.velocity(311).is_none());
        assert!(tracked(&[(100, 10.0)]).velocity(100).is_none());
        assert!(tracked(&[(100, 10.0), (100, 10.0)]).velocity(100).is_none());
        assert!(VelocityTracker::default().velocity(100).is_none());
        // A pause splits the motion.
        let v = tracked(&[(100, 10.0), (110, 10.0), (500, 10.0)]);
        assert!(v.velocity(500).is_none());
    }

    #[test]
    fn velocity_across_the_clock_wrapping() {
        let v = tracked(&[
            (u32::MAX - 9, 10.0),
            (u32::MAX.wrapping_add(1), 10.0),
            (10, 10.0),
        ]);
        let (_, y) = v.velocity(10).unwrap();
        assert!((y - 1000.0).abs() < 1.0, "{y}");
    }

    const WINDOW: (f32, f32) = (800.0, 467.0);

    fn fling(velocity: (f32, f32), k: &KineticSettings, start: Instant) -> Option<Fling> {
        Fling::new(velocity, 1.0, k, WINDOW, 1.0, start)
    }

    #[test]
    fn slow_gestures_have_no_inertia() {
        let now = Instant::now();
        assert!(fling((0.0, 29.0), &kinetic(), now).is_none());
        assert!(fling((0.0, 0.0), &kinetic(), now).is_none());
        assert!(fling((0.0, 31.0), &kinetic(), now).is_some());
    }

    /// Runs a fling to its end and returns where it got and how long it took.
    fn run(
        velocity: (f32, f32),
        k: &KineticSettings,
        scale: f32,
        frame: Duration,
    ) -> (f32, f32, Duration) {
        let start = Instant::now();
        let mut f = Fling::new(velocity, 1.0, k, WINDOW, scale, start).unwrap();
        let (mut x, mut y, mut t) = (0.0, 0.0, Duration::ZERO);
        loop {
            t += frame;
            let (dx, dy, done) = f.step(start + t);
            x += dx;
            y += dy;
            if done {
                return (x, y, t);
            }
        }
    }

    #[test]
    fn matches_chromium() {
        // Measured on Chrome 155: speed, distance, and the end of the curve from its source.
        for (v, d) in [
            (200.0, 7.6),
            (500.0, 47.0),
            (1000.0, 189.0),
            (2000.0, 756.0),
            (3000.0, 1401.0),
            (12000.0, 1401.0),
        ] {
            let (_, y, t) = run((0.0, v), &kinetic(), 1.0, Duration::from_millis(1));
            assert!((y - d).abs() < 2.0, "{v}: {y} vs {d}");
            let duration = (v / 2300.0).min(2.0);
            assert!((t.as_secs_f32() - duration).abs() < 0.01, "{v}: {t:?}");
        }
    }

    #[test]
    fn starts_at_the_speed_of_the_fingers() {
        let start = Instant::now();
        for v in [500.0, 2000.0] {
            let mut f = fling((0.0, v), &kinetic(), start).unwrap();
            let (_, dy, _) = f.step(start + Duration::from_millis(2));
            assert!((dy / 0.002 - v).abs() < v * 0.05, "{v}: {dy}");
            // Along either axis.
            let mut f = fling((v, 10.0), &kinetic(), start).unwrap();
            let (dx, _, _) = f.step(start + Duration::from_millis(2));
            assert!((dx / 0.002 - v).abs() < v * 0.05, "{v}: {dx}");
        }
    }

    #[test]
    fn the_path_does_not_depend_on_the_frame_rate() {
        let v = (900.0, 3000.0);
        let a = run(v, &kinetic(), 1.8, Duration::from_micros(4167));
        let b = run(v, &kinetic(), 1.8, Duration::from_millis(33));
        assert!((a.0 - b.0).abs() < 0.01, "{a:?} vs {b:?}");
        assert!((a.1 - b.1).abs() < 0.01, "{a:?} vs {b:?}");
    }

    #[test]
    fn each_axis_has_its_own_distance_and_bound() {
        // Distance goes with the square of the speed along each axis.
        let (x, y, _) = run((-1000.0, 2000.0), &kinetic(), 1.0, Duration::from_millis(1));
        assert!((x + 189.0).abs() < 1.0, "{x}");
        assert!((y - 756.0).abs() < 1.0, "{y}");
        let (x, y, _) = run((5000.0, 5000.0), &kinetic(), 1.0, Duration::from_millis(1));
        assert!((x - 2400.0).abs() < 1.0, "{x}");
        assert!((y - 1401.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn it_slows_down() {
        let start = Instant::now();
        for v in [800.0, 2000.0, 5000.0] {
            let mut f = fling((0.0, v), &kinetic(), start).unwrap();
            let mut last = f32::MAX;
            let mut i = 0;
            loop {
                i += 1;
                let (_, dy, done) = f.step(start + Duration::from_millis(i * 16));
                if done {
                    break;
                }
                assert!(dy >= 0.0, "{v}: moved backwards at step {i}");
                assert!(dy <= last + 0.01, "{v}: sped up at step {i}");
                last = dy;
            }
        }
    }

    #[test]
    fn looks_the_same_at_any_scale() {
        let a = run((0.0, 2000.0), &kinetic(), 1.0, Duration::from_millis(8));
        let b = run((0.0, 2000.0), &kinetic(), 1.8, Duration::from_millis(8));
        assert!((a.1 * 1.8 - b.1).abs() < 0.5, "{a:?} vs {b:?}");
    }

    #[test]
    fn settings_shape_the_curve() {
        let frame = Duration::from_millis(8);
        let base = run((0.0, 1000.0), &kinetic(), 1.0, frame);
        let mut k = kinetic();
        k.velocity_factor = 2.0;
        let fast = run((1000.0, 1000.0), &k, 1.0, frame);
        assert!((fast.1 - base.1 * 4.0).abs() < 2.0, "{fast:?} vs {base:?}");
        assert!((fast.0 - base.1 * 4.0).abs() < 2.0, "{fast:?} vs {base:?}");
        let mut k = kinetic();
        k.deceleration /= 2.0;
        let slippery = run((0.0, 1000.0), &k, 1.0, frame);
        assert!((slippery.1 - base.1 * 2.0).abs() < 2.0, "{slippery:?}");
        assert!(slippery.2 > base.2);
        let mut k = kinetic();
        k.max_screens = 1.0;
        let short = run((0.0, 5000.0), &k, 1.0, frame);
        assert!((short.1 - 467.0).abs() < 1.0, "{short:?}");
    }

    #[test]
    fn a_boost_raises_the_bound() {
        let start = Instant::now();
        let mut f = Fling::new((0.0, 6000.0), 2.0, &kinetic(), WINDOW, 1.0, start).unwrap();
        let (_, dy, _) = f.step(start + Duration::from_secs(3));
        assert!((dy - 2802.0).abs() < 1.0, "{dy}");
    }

    #[test]
    fn it_knows_how_fast_it_is_going_in_logical_pixels() {
        let start = Instant::now();
        let speed_at = |scale: f32, ms: u64| {
            let mut f = Fling::new((0.0, 1000.0), 1.0, &kinetic(), WINDOW, scale, start).unwrap();
            f.step(start + Duration::from_millis(ms - 8));
            f.step(start + Duration::from_millis(ms));
            f.speed
        };
        assert!(
            (speed_at(1.0, 16) - 960.0).abs() < 60.0,
            "{}",
            speed_at(1.0, 16)
        );
        assert!(
            (speed_at(1.0, 300) - 270.0).abs() < 60.0,
            "{}",
            speed_at(1.0, 300)
        );
        assert!((speed_at(1.8, 300) - speed_at(1.0, 300)).abs() < 1.0);
    }

    fn booster(velocity: f32, speed: f32) -> Booster {
        let start = Instant::now();
        let mut f = fling((0.0, velocity), &kinetic(), start).unwrap();
        f.speed = speed;
        Booster::new(&f, 1000)
    }

    #[test]
    fn flicks_in_a_row_add_up() {
        let mut b = booster(1000.0, 800.0);
        assert!(b.motion(1020, 0.0, 10.0));
        assert!(b.motion(1030, 0.0, 10.0));
        let (v, boost) = b.boost(1040, (0.0, 2000.0)).unwrap();
        assert_eq!(v, (0.0, 3000.0));
        assert_eq!(boost, 1.5);
    }

    #[test]
    fn what_keeps_flicks_from_adding_up() {
        // Too long after the fling stopped, or after the last motion.
        assert!(!booster(1000.0, 800.0).motion(1051, 0.0, 10.0));
        let mut b = booster(1000.0, 800.0);
        assert!(b.motion(1040, 0.0, 10.0));
        assert!(b.boost(1091, (0.0, 2000.0)).is_none());
        // The other way.
        assert!(!booster(1000.0, 800.0).motion(1010, 0.0, -10.0));
        assert!(booster(1000.0, 800.0).boost(1010, (0.0, -2000.0)).is_none());
        // Fingers moving too slowly to keep it.
        let mut b = booster(1000.0, 800.0);
        assert!(b.motion(1010, 0.0, 10.0));
        assert!(!b.motion(1020, 0.0, 1.0));
        // A fling that had nearly stopped, or a slow new one.
        assert!(booster(1000.0, 340.0).boost(1010, (0.0, 2000.0)).is_none());
        assert!(booster(1000.0, 800.0).boost(1010, (0.0, 340.0)).is_none());
    }
}
