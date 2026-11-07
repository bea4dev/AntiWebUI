//! Sliders.
//!
//! What a slider does (the pointer, the keys, the focus) and what it looks like are apart:
//! `slider_with` gives the look a [`SliderState`] and adds the doing to the box that comes back.
//! `styled_slider` keeps the standard look and lets each part of it be changed. See
//! `docs/design/33-customization.md`.

use crate::theme::{Theme, focus_ring, themed};
use anti_webui_core::prelude::*;
use std::rc::Rc;

fn slider_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).rounded(10.0))
        .focus_visible(|s| focus_ring(s, t))
        .disabled(|s| s.cursor(CursorIcon::Default))
}

fn track_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.border))
}

fn fill_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.accent))
        .disabled(|s| s.bg(t.text_disabled))
        .accent_background()
}

fn thumb_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(Color::WHITE).border_color(t.border_strong))
        .hover(|s| s.border_color(t.accent))
        .active(|s| s.border_color(t.accent))
        .focus_visible(|s| focus_ring(s, t))
        .disabled(|s| s.bg(t.surface_muted).border_color(t.border))
        .accent_border()
}

/// Which values a slider takes and how it is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SliderOptions {
    step: Option<f32>,
    vertical: bool,
    inset: f32,
}

impl SliderOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Only stops at the low end, the low end plus `step`, and so on. The keys go by `step` too.
    pub fn step(mut self, step: f32) -> Self {
        self.step = (step > 0.0).then_some(step);
        self
    }

    /// Goes up rather than right: the low end is at the bottom.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// How far in from the ends of the box the ends of the track are, in logical pixels: where the
    /// pointer gives the low and the high end.
    pub fn inset(mut self, inset: f32) -> Self {
        self.inset = inset.max(0.0);
        self
    }
}

/// What a slider's look gets to see. Reading it tracks it.
///
/// A slider has one thumb, or two for a range: the first is the low end of the range.
#[derive(Clone, Copy)]
pub struct SliderState {
    values: [Option<Binding<f32>>; 2],
    min: f32,
    max: f32,
    options: SliderOptions,
    held: Signal<Option<usize>>,
    hovered: Signal<bool>,
    keyed: [Signal<bool>; 2],
    thumbs: [NodeRef; 2],
    has_thumbs: Signal<bool>,
}

impl SliderState {
    fn new(values: [Option<Binding<f32>>; 2], min: f32, max: f32, options: SliderOptions) -> Self {
        Self {
            values,
            min,
            max: if max > min { max } else { min + f32::EPSILON },
            options,
            held: signal(None),
            hovered: signal(false),
            keyed: [signal(false), signal(false)],
            thumbs: [node_ref(), node_ref()],
            has_thumbs: signal(false),
        }
    }

    /// How many thumbs: 1, or 2 for a range.
    pub fn count(&self) -> usize {
        if self.values[0].is_some() { 2 } else { 1 }
    }

    fn binding(&self, i: usize) -> Binding<f32> {
        match (i, self.values[0]) {
            (0, Some(start)) => start,
            _ => self.values[1].expect("a slider always has its last value"),
        }
    }

    /// The value of the last thumb: the only one, or the high end of a range.
    pub fn value(&self) -> f32 {
        self.value_of(self.count() - 1)
    }

    pub fn value_of(&self, i: usize) -> f32 {
        self.binding(i).get()
    }

    /// Where [`value`](Self::value) is between the ends, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        self.fraction_of(self.count() - 1)
    }

    pub fn fraction_of(&self, i: usize) -> f32 {
        self.fraction_at(self.value_of(i))
    }

    fn fraction_at(&self, v: f32) -> f32 {
        ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    pub fn min(&self) -> f32 {
        self.min
    }

    pub fn max(&self) -> f32 {
        self.max
    }

    pub fn options(&self) -> SliderOptions {
        self.options
    }

    /// The size of a step, if it only stops at steps.
    pub fn step(&self) -> Option<f32> {
        self.options.step
    }

    pub fn vertical(&self) -> bool {
        self.options.vertical
    }

    /// The thumb that the pointer holds.
    pub fn held(&self) -> Option<usize> {
        self.held.get()
    }

    /// The pointer holds a thumb.
    pub fn dragging(&self) -> bool {
        self.held.get().is_some()
    }

    /// The pointer is over the slider.
    pub fn hovered(&self) -> bool {
        self.hovered.get()
    }

    /// A thumb, or the whole slider when the look has no thumbs, has the focus from the keyboard.
    pub fn focus_visible(&self) -> bool {
        self.keyed[0].get() || self.keyed[1].get()
    }

    /// Thumb `i` has the focus from the keyboard.
    pub fn focus_visible_of(&self, i: usize) -> bool {
        let i = if self.count() == 1 { 1 } else { i.min(1) };
        self.keyed[i].get()
    }

    /// Makes `el` thumb `i`: the focus stops on it and the keys move it. Without thumbs, the whole
    /// slider is one stop and the keys move the last value.
    pub fn thumb(&self, i: usize, el: Div) -> Div {
        let state = *self;
        let i = i.min(self.count() - 1);
        self.has_thumbs.set(true);
        let slot = if self.count() == 1 { 1 } else { i };
        el.focusable()
            .access(move |a| state.told(a, i))
            .node_ref(self.thumbs[slot])
            .on_focus_visible(move |on| state.keyed[slot].set(on))
            .on_key_down(move |e| state.key(i, e))
    }

    /// What thumb `i` says to assistive technology.
    fn told(&self, a: Access, i: usize) -> Access {
        let a =
            a.role(Role::Slider)
                .value(self.value_of(i) as f64, self.min as f64, self.max as f64);
        match self.options.step {
            Some(step) => a.step(step as f64),
            None => a,
        }
    }

    fn snap(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        match self.options.step {
            Some(s) => (self.min + ((v - self.min) / s).round() * s).clamp(self.min, self.max),
            None => v,
        }
    }

    /// Sets thumb `i` to `v`, keeping the two of a range in order.
    fn change(&self, i: usize, v: f32) {
        let v = self.snap(v);
        let v = match (self.count(), i) {
            (2, 0) => v.min(self.binding(1).get_untracked()),
            (2, _) => v.max(self.binding(0).get_untracked()),
            _ => v,
        };
        let binding = self.binding(i);
        if binding.get_untracked() != v {
            binding.set(v);
        }
    }

    fn key(&self, i: usize, e: &KeyboardEvent) {
        let span = self.max - self.min;
        let small = self.options.step.unwrap_or(span / 20.0);
        let big = (small * 5.0).max(span / 10.0);
        let now = self.binding(i).get_untracked();
        let next = match e.key {
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => now - small,
            Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => now + small,
            Key::Named(NamedKey::PageDown) => now - big,
            Key::Named(NamedKey::PageUp) => now + big,
            Key::Named(NamedKey::Home) => self.min,
            Key::Named(NamedKey::End) => self.max,
            _ => return,
        };
        e.prevent_default();
        self.change(i, next);
    }

    /// Where the pointer is along the track, from 0 to 1.
    fn along(&self, g: &GrabEvent) -> f32 {
        let inset = self.options.inset * untrack(window_scale).as_f64() as f32;
        let t = if self.options.vertical {
            let h = g.rect.height as f32;
            (h - g.local.y - inset) / (h - inset * 2.0).max(1.0)
        } else {
            let w = g.rect.width as f32;
            (g.local.x - inset) / (w - inset * 2.0).max(1.0)
        };
        t.clamp(0.0, 1.0)
    }

    /// The thumb that a press at `t` moves: the nearest one.
    fn nearest(&self, t: f32) -> usize {
        if self.count() == 1 {
            return 0;
        }
        let (a, b) = untrack(|| (self.fraction_of(0), self.fraction_of(1)));
        if (t - a).abs() < (t - b).abs() || (t < a && a == b) {
            0
        } else {
            1
        }
    }
}

type Part = Rc<dyn Fn(Div, SliderState) -> Div>;

/// Changes to the parts of the standard look. Each gets the part with its standard styles on and
/// returns it; what it says goes above the theme in every state.
#[derive(Clone, Default)]
pub struct SliderParts {
    track: Option<Part>,
    fill: Option<Part>,
    thumb: Option<Part>,
}

impl SliderParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bar along the whole width.
    pub fn track(mut self, f: impl Fn(Div, SliderState) -> Div + 'static) -> Self {
        self.track = Some(Rc::new(f));
        self
    }

    /// The part of the track that is on, inside the track.
    pub fn fill(mut self, f: impl Fn(Div, SliderState) -> Div + 'static) -> Self {
        self.fill = Some(Rc::new(f));
        self
    }

    /// What is held, each of them for a range. It stays inside the track whatever its size.
    pub fn thumb(mut self, f: impl Fn(Div, SliderState) -> Div + 'static) -> Self {
        self.thumb = Some(Rc::new(f));
        self
    }
}

fn apply(part: &Option<Part>, el: Div, s: SliderState) -> Div {
    match part {
        Some(f) => f(el, s),
        None => el,
    }
}

/// A horizontal slider bound to `value` in `min..=max`, which goes in twentieths with the keys.
pub fn slider(value: impl Into<Binding<f32>>, min: f32, max: f32) -> Div {
    styled_slider(value, min, max, SliderParts::new())
}

/// A slider that only stops at `min`, `min + step`, and so on.
pub fn stepped_slider(value: impl Into<Binding<f32>>, min: f32, max: f32, step: f32) -> Div {
    slider_with(value, min, max, SliderOptions::new().step(step), |s| {
        standard(s, SliderParts::new())
    })
}

/// A slider with the standard look, with its parts changed by `parts`.
pub fn styled_slider(
    value: impl Into<Binding<f32>>,
    min: f32,
    max: f32,
    parts: SliderParts,
) -> Div {
    slider_with(value, min, max, SliderOptions::new(), move |s| {
        standard(s, parts)
    })
}

/// A slider with two thumbs for a range from `start` to `end`, which don't pass each other. Each
/// thumb is a stop for Tab.
pub fn range_slider(
    start: impl Into<Binding<f32>>,
    end: impl Into<Binding<f32>>,
    min: f32,
    max: f32,
) -> Div {
    range_slider_with(start, end, min, max, SliderOptions::new(), |s| {
        standard(s, SliderParts::new())
    })
}

/// A slider that looks like whatever `look` builds. The slider adds the pointer, the keys and the
/// focus to the box that comes back; where the pointer is across that box is where the value goes.
pub fn slider_with(
    value: impl Into<Binding<f32>>,
    min: f32,
    max: f32,
    options: SliderOptions,
    look: impl FnOnce(SliderState) -> Div,
) -> Div {
    behave(
        SliderState::new([None, Some(value.into())], min, max, options),
        look,
    )
}

/// [`range_slider`] with a look of its own. The look should make each thumb with
/// [`SliderState::thumb`], so that each is a stop for Tab.
pub fn range_slider_with(
    start: impl Into<Binding<f32>>,
    end: impl Into<Binding<f32>>,
    min: f32,
    max: f32,
    options: SliderOptions,
    look: impl FnOnce(SliderState) -> Div,
) -> Div {
    behave(
        SliderState::new([Some(start.into()), Some(end.into())], min, max, options),
        look,
    )
}

fn behave(state: SliderState, look: impl FnOnce(SliderState) -> Div) -> Div {
    let root = look(state)
        .select_none()
        .on_hover(move |on| state.hovered.set(on))
        .grab_threshold(0.0)
        .on_grab(move |g| match g.phase {
            GrabPhase::Start => {
                let t = state.along(g);
                let i = state.nearest(t);
                state.held.set(Some(i));
                if untrack(|| state.has_thumbs.get()) {
                    state.thumbs[if state.count() == 1 { 1 } else { i }].focus();
                }
                state.change(i, state.min + (state.max - state.min) * t);
            }
            GrabPhase::Move => {
                if let Some(i) = state.held.get_untracked() {
                    let t = state.along(g);
                    state.change(i, state.min + (state.max - state.min) * t);
                }
            }
            GrabPhase::End | GrabPhase::Cancel => state.held.set(None),
        });
    if untrack(|| state.has_thumbs.get()) {
        root
    } else {
        let last = state.count() - 1;
        root.focusable()
            .access(move |a| state.told(a, last))
            .on_focus_visible(move |on| state.keyed[1].set(on))
            .on_key_down(move |e| state.key(last, e))
    }
}

fn standard(s: SliderState, parts: SliderParts) -> Div {
    let two = s.count() == 2;
    let fill = div()
        .h_full()
        .rounded(2.0)
        .sheet(|| themed(fill_sheet))
        .dyn_style(move |st| {
            let (from, to) = if two {
                (s.fraction_of(0), s.fraction_of(1))
            } else {
                (0.0, s.fraction())
            };
            st.ml(pct(from * 100.0)).w(pct((to - from) * 100.0))
        });
    let track = div()
        .w_full()
        .h(4.0)
        .rounded(2.0)
        .sheet(|| themed(track_sheet))
        .child(apply(&parts.fill, fill, s));
    let thumb = |i: usize| {
        let el = div()
            .flex_none()
            .w(16.0)
            .h(16.0)
            .rounded(8.0)
            .border(1.0)
            .shadow_sm()
            .sheet(|| themed(thumb_sheet));
        // A range has a stop for each thumb; one thumb leaves the stop to the whole slider.
        let el = if two { s.thumb(i, el) } else { el };
        apply(&parts.thumb, el, s)
    };
    // The thumbs sit between spacers that share the room left by them, so they stay inside the
    // track whatever their size: left = fraction × (width − thumbs).
    let spacer =
        move |grow: fn(SliderState) -> f32| div().dyn_style(move |st| st.flex_grow(grow(s)));
    let rail = div().absolute().inset(0.0).flex().flex_row().items_center();
    let rail = if two {
        rail.child(spacer(|s| s.fraction_of(0)))
            .child(thumb(0))
            .child(spacer(|s| s.fraction_of(1) - s.fraction_of(0)))
            .child(thumb(1))
            .child(spacer(|s| 1.0 - s.fraction_of(1)))
    } else {
        rail.child(spacer(|s| s.fraction()))
            .child(thumb(0))
            .child(spacer(|s| 1.0 - s.fraction()))
    };
    div()
        .relative()
        .h(20.0)
        .min_w(80.0)
        .flex()
        .items_center()
        .sheet(|| themed(slider_sheet))
        .child(apply(&parts.track, track, s))
        .child(rail)
}
