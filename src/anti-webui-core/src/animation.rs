//! Things that change over time: code that runs once per frame like `requestAnimationFrame`,
//! values that move by themselves, and the transitions of styles (see
//! `docs/design/13-animation.md`).

use crate::reactive::{Signal, batch, signal};
use crate::tree::Document;
use anti_webui_style::{AnimatedValue, Color, Style, Transition};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

type FrameCallback = Box<dyn FnOnce(Instant)>;

thread_local! {
    static CALLBACKS: RefCell<Vec<FrameCallback>> = const { RefCell::new(Vec::new()) };
    static WANTED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` before the next frame is produced, on the UI thread, with the time of that frame.
/// Asking again from `f` animates: a frame comes as often as the compositor takes them.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use anti_webui_core::animation::request_frame;
///
/// fn spin(angle: Signal<f32>) {
///     request_frame(move |_| {
///         angle.update(|a| *a += 1.0);
///         spin(angle);
///     });
/// }
/// ```
pub fn request_frame(f: impl FnOnce(Instant) + 'static) {
    CALLBACKS.with(|c| c.borrow_mut().push(Box::new(f)));
    WANTED.with(|w| w.set(true));
}

pub(crate) fn frame_requested() -> bool {
    WANTED.with(|w| w.get())
}

/// What was requested from here on waits for the next frame. Reactions come once, after all of
/// them: something reading two moving values shouldn't run twice per frame.
pub(crate) fn run_frame_callbacks(now: Instant) {
    WANTED.with(|w| w.set(false));
    let callbacks = CALLBACKS.with(|c| std::mem::take(&mut *c.borrow_mut()));
    batch(|| {
        for f in callbacks {
            f(now);
        }
    });
}

/// How a node comes and goes as part of a `show` or a `for_each`. Only there while it does.
pub(crate) struct Presence {
    /// What the first resolution of its style goes from.
    pub starting: Option<Style>,
    /// What it goes to before it is removed.
    pub exit: Style,
    pub exiting: bool,
    /// `exiting` just ended: what it does now still counts as part of the exit.
    pub returning: bool,
    pub timing: Transition,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            starting: None,
            exit: Style::default(),
            exiting: false,
            returning: false,
            timing: Transition::ms(0),
        }
    }
}

/// A transition going on in a node.
pub(crate) struct RunningTransition {
    /// Index into [`anti_webui_style::ANIMATED_FIELDS`].
    pub field: usize,
    from: AnimatedValue,
    pub to: AnimatedValue,
    pub current: AnimatedValue,
    start: Instant,
    timing: Transition,
    /// How far along the value is, for shortening the way back.
    progress: f32,
    /// `current` is `to`: style resolution removes it, showing the value it reached.
    pub finished: bool,
}

impl RunningTransition {
    pub fn new(
        field: usize,
        from: AnimatedValue,
        to: AnimatedValue,
        timing: Transition,
        start: Instant,
    ) -> Self {
        Self {
            field,
            current: from.clone(),
            from,
            to,
            start,
            timing,
            progress: 0.0,
            finished: false,
        }
    }

    /// Goes on from where it is towards `to` instead.
    pub fn redirect(&mut self, to: AnimatedValue, mut timing: Transition, now: Instant) {
        // Going back the way it came takes the time that it took to come, like in CSS.
        if to == self.from {
            timing.duration = timing.duration.mul_f32(self.progress.clamp(0.0, 1.0));
        }
        self.from = self.current.clone();
        self.to = to;
        self.timing = timing;
        self.start = now;
        self.progress = 0.0;
        self.finished = false;
    }

    fn advance(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.start);
        let duration = self.timing.duration;
        if elapsed >= duration {
            self.current = self.to.clone();
            self.progress = 1.0;
            self.finished = true;
            return;
        }
        let t = elapsed.as_secs_f32() / duration.as_secs_f32();
        self.progress = self.timing.easing.apply(t);
        if let Some(v) = self.from.interpolate(&self.to, self.progress) {
            self.current = v;
        }
    }
}

/// Moves every transition on to `now`. The rest is what any change of style goes through.
pub(crate) fn advance_transitions(doc: &mut Document, now: Instant) {
    if doc.transitioning.is_empty() {
        return;
    }
    let nodes: Vec<_> = doc.transitioning.iter().copied().collect();
    for id in nodes {
        let Some(node) = doc.nodes.get_mut(id) else {
            doc.transitioning.remove(&id);
            continue;
        };
        for r in &mut node.transitions {
            r.advance(now);
        }
        doc.mark_style_of(id);
    }
}

/// What [`Animated`] can move: up to four numbers that make sense to interpolate.
pub trait Animatable: Copy + PartialEq + 'static {
    fn to_components(self) -> [f32; 4];
    fn from_components(components: [f32; 4]) -> Self;
}

impl Animatable for f32 {
    fn to_components(self) -> [f32; 4] {
        [self, 0.0, 0.0, 0.0]
    }

    fn from_components(c: [f32; 4]) -> Self {
        c[0]
    }
}

impl Animatable for (f32, f32) {
    fn to_components(self) -> [f32; 4] {
        [self.0, self.1, 0.0, 0.0]
    }

    fn from_components(c: [f32; 4]) -> Self {
        (c[0], c[1])
    }
}

/// Premultiplied, so that going to or from a transparent color doesn't darken on the way.
impl Animatable for Color {
    fn to_components(self) -> [f32; 4] {
        self.premultiplied()
    }

    fn from_components(c: [f32; 4]) -> Self {
        if c[3] <= 0.0 {
            Color::TRANSPARENT
        } else {
            let unmultiply = |v: f32| (v / c[3]).clamp(0.0, 1.0);
            Color::new(
                unmultiply(c[0]),
                unmultiply(c[1]),
                unmultiply(c[2]),
                c[3].min(1.0),
            )
        }
    }
}

/// A motion defined by physics instead of a duration. It keeps its speed when the target changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

impl Default for Spring {
    fn default() -> Self {
        Self {
            stiffness: 170.0,
            damping: 26.0,
            mass: 1.0,
        }
    }
}

/// The time step of a spring, so that it moves the same at any frame rate.
const SPRING_STEP: f32 = 0.001;
/// A spring is done this close to the target, as a fraction of the way it had to go.
const SPRING_REST: f32 = 0.0005;
/// ...or this close, for when it had almost no way to go.
const SPRING_MIN_REST: f32 = 0.0001;

enum Motion {
    None,
    Tween {
        from: [f32; 4],
        to: [f32; 4],
        start: Instant,
        timing: Transition,
    },
    Spring {
        to: [f32; 4],
        spring: Spring,
        /// How close counts as there.
        rest: f32,
        /// What is left of the last frame's time, less than a step.
        left: f32,
        last: Instant,
    },
}

struct Run {
    motion: Motion,
    /// Identifies this motion: frames that were asked for an earlier one do nothing.
    generation: u32,
    /// Per second.
    velocity: [f32; 4],
    /// When `velocity` was last worked out.
    velocity_at: Instant,
}

/// A value that moves by itself. Read it like a signal: what reads it runs again on every frame
/// while it moves, and not at all otherwise.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use std::time::Duration;
///
/// let x = animated(0.0_f32);
/// let view = div()
///     .dyn_style(move |s| s.translate_x(x.get()))
///     .on_click(move |_| x.to(300.0, Transition::ms(300).ease_out()));
/// ```
///
/// When its owner is disposed it stops by itself.
pub struct Animated<T: 'static> {
    value: Signal<T>,
    run: Signal<Run>,
}

impl<T> Clone for Animated<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Animated<T> {}

/// Creates a value owned by the current owner.
pub fn animated<T: Animatable>(value: T) -> Animated<T> {
    Animated {
        value: signal(value),
        run: signal(Run {
            motion: Motion::None,
            generation: 0,
            velocity: [0.0; 4],
            velocity_at: Instant::now(),
        }),
    }
}

impl<T: Animatable> Animated<T> {
    /// Reads the value, tracking the read.
    pub fn get(&self) -> T {
        self.value.get()
    }

    pub fn get_untracked(&self) -> T {
        self.value.get_untracked()
    }

    /// Goes from where it is to `target`.
    pub fn to(&self, target: T, timing: Transition) {
        let from = self.get_untracked().to_components();
        let to = target.to_components();
        if timing.duration.is_zero() {
            self.set(target);
            return;
        }
        self.begin(Motion::Tween {
            from,
            to,
            start: Instant::now(),
            timing,
        });
    }

    /// Goes from where it is to `target` like a spring, keeping the speed it has now.
    pub fn spring_to(&self, target: T, spring: Spring) {
        let from = self.get_untracked().to_components();
        let to = target.to_components();
        let way = from
            .iter()
            .zip(to)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        self.begin(Motion::Spring {
            to,
            spring,
            rest: (way * SPRING_REST).max(SPRING_MIN_REST),
            left: 0.0,
            last: Instant::now(),
        });
    }

    /// Stops moving and takes this value right away.
    pub fn set(&self, value: T) {
        self.stop();
        self.value.set(value);
    }

    /// Stops where it is.
    pub fn stop(&self) {
        self.run.update_untracked(|r| {
            r.motion = Motion::None;
            r.generation = r.generation.wrapping_add(1);
            r.velocity = [0.0; 4];
        });
    }

    fn begin(&self, motion: Motion) {
        let generation = self.run.update_untracked(|r| {
            r.motion = motion;
            r.generation = r.generation.wrapping_add(1);
            r.generation
        });
        self.next_frame(generation);
    }

    fn next_frame(self, generation: u32) {
        request_frame(move |now| {
            // A signal of it is gone when its owner is.
            if !self.value.is_alive() || !self.run.is_alive() {
                return;
            }
            let current = self.value.get_untracked().to_components();
            let step = self
                .run
                .update_untracked(|r| (r.generation == generation).then(|| r.step(current, now)));
            let Some((value, moving)) = step else {
                return;
            };
            self.value.set_if_changed(T::from_components(value));
            if moving {
                self.next_frame(generation);
            }
        });
    }
}

impl Run {
    /// Where it is at `now`, and whether it is still on its way.
    fn step(&mut self, current: [f32; 4], now: Instant) -> ([f32; 4], bool) {
        let (value, moving) = match &mut self.motion {
            Motion::None => (current, false),
            Motion::Tween {
                from,
                to,
                start,
                timing,
            } => {
                let elapsed = now.saturating_duration_since(*start);
                if elapsed >= timing.duration {
                    (*to, false)
                } else {
                    let t = elapsed.as_secs_f32() / timing.duration.as_secs_f32();
                    let p = timing.easing.apply(t);
                    let at = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * p);
                    (at, true)
                }
            }
            Motion::Spring {
                to,
                spring,
                rest,
                left,
                last,
            } => {
                // Long pauses aren't worth simulating step by step: it would be there anyway.
                let dt = now
                    .saturating_duration_since(*last)
                    .min(Duration::from_secs(4));
                *last = now;
                *left += dt.as_secs_f32();
                let mut at = current;
                let mut velocity = self.velocity;
                while *left >= SPRING_STEP {
                    *left -= SPRING_STEP;
                    for i in 0..4 {
                        let force =
                            spring.stiffness * (to[i] - at[i]) - spring.damping * velocity[i];
                        velocity[i] += force / spring.mass * SPRING_STEP;
                        at[i] += velocity[i] * SPRING_STEP;
                    }
                }
                self.velocity = velocity;
                self.velocity_at = now;
                let settled = (0..4)
                    .all(|i| (to[i] - at[i]).abs() <= *rest && velocity[i].abs() <= *rest * 10.0);
                if settled { (*to, false) } else { (at, true) }
            }
        };
        if !matches!(self.motion, Motion::Spring { .. }) {
            // A spring that takes over goes on at the same speed.
            let dt = now
                .saturating_duration_since(self.velocity_at)
                .as_secs_f32();
            if dt > 0.0 {
                self.velocity = std::array::from_fn(|i| (value[i] - current[i]) / dt);
                self.velocity_at = now;
            }
        }
        if !moving {
            self.motion = Motion::None;
            self.velocity = [0.0; 4];
        }
        (value, moving)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::create_root;

    #[test]
    fn going_elsewhere_again_and_again_leaves_one_motion_going() {
        let (x, owner) = create_root(|| animated(0.0_f32));
        let now = Instant::now();
        for i in 0..5 {
            x.to(i as f32, Transition::ms(1000));
        }
        x.spring_to(9.0, Spring::default());
        let waiting = |n: usize| CALLBACKS.with(|c| assert_eq!(c.borrow().len(), n));
        waiting(6);
        run_frame_callbacks(now + Duration::from_millis(1));
        waiting(1);
        run_frame_callbacks(now + Duration::from_millis(2));
        waiting(1);
        x.stop();
        run_frame_callbacks(now + Duration::from_millis(3));
        waiting(0);
        owner.dispose();
    }
}
