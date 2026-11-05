//! What `transition` needs to know about styles: how to time a change, and which properties it can
//! go through (see `docs/design/13-animation.md`).

use crate::*;
use std::time::Duration;

/// How the progress through a transition maps to the progress of the value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Easing {
    Linear,
    /// Like CSS `cubic-bezier(x1, y1, x2, y2)`.
    CubicBezier(f32, f32, f32, f32),
    /// Like CSS `ease`.
    #[default]
    Ease,
}

impl Easing {
    pub const EASE_IN: Easing = Easing::CubicBezier(0.42, 0.0, 1.0, 1.0);
    pub const EASE_OUT: Easing = Easing::CubicBezier(0.0, 0.0, 0.58, 1.0);
    pub const EASE_IN_OUT: Easing = Easing::CubicBezier(0.42, 0.0, 0.58, 1.0);

    /// The progress of the value at `t` in `0.0..=1.0`.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::Ease => cubic_bezier(0.25, 0.1, 0.25, 1.0, t),
            Easing::CubicBezier(x1, y1, x2, y2) => cubic_bezier(x1, y1, x2, y2, t),
        }
    }
}

fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let curve = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    if x <= 0.0 || x >= 1.0 {
        return x;
    }
    // x grows with t, so narrowing it down always finds it.
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    let mut t = x;
    for _ in 0..24 {
        let at = curve(x1, x2, t);
        if (at - x).abs() < 1e-5 {
            break;
        }
        if at < x {
            low = t;
        } else {
            high = t;
        }
        t = (low + high) * 0.5;
    }
    curve(y1, y2, t)
}

/// How long a change takes and how it goes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transition {
    pub duration: Duration,
    pub easing: Easing,
}

impl Transition {
    pub const fn new(duration: Duration) -> Self {
        Self {
            duration,
            easing: Easing::Ease,
        }
    }

    pub const fn ms(milliseconds: u64) -> Self {
        Self::new(Duration::from_millis(milliseconds))
    }

    pub const fn easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    pub const fn linear(self) -> Self {
        self.easing(Easing::Linear)
    }

    pub const fn ease_in(self) -> Self {
        self.easing(Easing::EASE_IN)
    }

    pub const fn ease_out(self) -> Self {
        self.easing(Easing::EASE_OUT)
    }

    pub const fn ease_in_out(self) -> Self {
        self.easing(Easing::EASE_IN_OUT)
    }

    pub const fn cubic_bezier(self, x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        self.easing(Easing::CubicBezier(x1, y1, x2, y2))
    }
}

/// What can be named for `transition_of`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prop {
    Translate,
    Scale,
    Rotate,
    Opacity,
    ImageOpacity,
    Blur,
    BackdropBlur,
    Background,
    BorderColor,
    OutlineColor,
    TextColor,
    FontWeight,
    FontAxes,
    Radius,
    Shadow,
    Width,
    Height,
    Margin,
    Padding,
    Inset,
    Gap,
    BorderWidth,
    SvgFill,
    SvgStroke,
    StrokeWidth,
    FillOpacity,
    StrokeOpacity,
}

/// The value of an animatable property.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimatedValue {
    Number(f32),
    Color(Color),
    Length(Length),
    Shadows(Vec<BoxShadow>),
    Weight(FontWeight),
    Axes(FontAxes),
    Paint(SvgPaint),
}

impl AnimatedValue {
    /// How far apart the values are, for telling how far along something is: only the fraction
    /// between two distances means anything.
    pub fn distance(&self, other: &AnimatedValue) -> f32 {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => (a - b).abs(),
            (Self::Paint(SvgPaint::Color(a)), Self::Paint(SvgPaint::Color(b)))
            | (Self::Color(a), Self::Color(b)) => {
                let (a, b) = (a.premultiplied(), b.premultiplied());
                a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum()
            }
            (Self::Length(Length::Px(a)), Self::Length(Length::Px(b)))
            | (Self::Length(Length::Percent(a)), Self::Length(Length::Percent(b))) => (a - b).abs(),
            (Self::Length(Length::Device(a)), Self::Length(Length::Device(b))) => {
                (a - b).abs() as f32
            }
            (Self::Shadows(a), Self::Shadows(b)) => a
                .iter()
                .zip(b)
                .map(|(a, b)| (a.offset_x - b.offset_x).abs() + (a.offset_y - b.offset_y).abs())
                .sum(),
            (Self::Weight(a), Self::Weight(b)) => (a.0 as f32 - b.0 as f32).abs(),
            (Self::Axes(a), Self::Axes(b)) => a.distance(b),
            _ => 0.0,
        }
    }

    /// The value at `t` of the way from `self` to `to`, or `None` if nothing is in between.
    pub fn interpolate(&self, to: &AnimatedValue, t: f32) -> Option<AnimatedValue> {
        let number = |a: f32, b: f32| a + (b - a) * t;
        Some(match (self, to) {
            (Self::Number(a), Self::Number(b)) => Self::Number(number(*a, *b)),
            (Self::Color(a), Self::Color(b)) => Self::Color(mix_premultiplied(*a, *b, t)),
            // Only colors have anything in between.
            (Self::Paint(SvgPaint::Color(a)), Self::Paint(SvgPaint::Color(b))) => {
                Self::Paint(SvgPaint::Color(mix_premultiplied(*a, *b, t)))
            }
            (Self::Length(Length::Px(a)), Self::Length(Length::Px(b))) => {
                Self::Length(Length::Px(number(*a, *b)))
            }
            (Self::Length(Length::Percent(a)), Self::Length(Length::Percent(b))) => {
                Self::Length(Length::Percent(number(*a, *b)))
            }
            (Self::Length(Length::Device(a)), Self::Length(Length::Device(b))) => {
                Self::Length(Length::Device(number(*a as f32, *b as f32).round() as i32))
            }
            (Self::Shadows(a), Self::Shadows(b)) => {
                let longest = a.len().max(b.len());
                let mut out = Vec::with_capacity(longest);
                for i in 0..longest {
                    // One that isn't there is the other one, invisible.
                    let invisible = |s: &BoxShadow| BoxShadow {
                        color: s.color.with_alpha(0.0),
                        ..*s
                    };
                    let (x, y) = match (a.get(i), b.get(i)) {
                        (Some(x), Some(y)) => (*x, *y),
                        (Some(x), None) => (*x, invisible(x)),
                        (None, Some(y)) => (invisible(y), *y),
                        (None, None) => unreachable!(),
                    };
                    if x.inset != y.inset {
                        return None;
                    }
                    out.push(BoxShadow {
                        offset_x: number(x.offset_x, y.offset_x),
                        offset_y: number(x.offset_y, y.offset_y),
                        blur: number(x.blur, y.blur),
                        spread: number(x.spread, y.spread),
                        color: mix_premultiplied(x.color, y.color, t),
                        inset: x.inset,
                    });
                }
                Self::Shadows(out)
            }
            (Self::Weight(a), Self::Weight(b)) => {
                Self::Weight(FontWeight(number(a.0 as f32, b.0 as f32).round() as u16))
            }
            (Self::Axes(a), Self::Axes(b)) => Self::Axes(a.interpolate(b, t)),
            _ => return None,
        })
    }
}

/// Going to or from a transparent color doesn't darken on the way, unlike [`Color::mix`].
pub fn mix_premultiplied(a: Color, b: Color, t: f32) -> Color {
    let (pa, pb) = (a.premultiplied(), b.premultiplied());
    let l = |i: usize| pa[i] + (pb[i] - pa[i]) * t;
    let alpha = l(3);
    if alpha <= 0.0 {
        return Color::TRANSPARENT;
    }
    Color::new(l(0) / alpha, l(1) / alpha, l(2) / alpha, alpha)
}

/// One animatable property of [`ComputedStyle`].
pub struct AnimatedField {
    pub prop: Prop,
    pub get: fn(&ComputedStyle) -> AnimatedValue,
    pub set: fn(&mut ComputedStyle, AnimatedValue),
    pub same: fn(&ComputedStyle, &ComputedStyle) -> bool,
}

macro_rules! animated_fields {
    ($( $prop:ident : $($field:ident as $kind:ident),+ ;)*) => {
        /// Every animatable property.
        pub const ANIMATED_FIELDS: &[AnimatedField] = &[
            $($(AnimatedField {
                prop: Prop::$prop,
                get: |c| AnimatedValue::$kind(c.$field.clone()),
                set: |c, v| {
                    if let AnimatedValue::$kind(v) = v {
                        c.$field = v;
                    }
                },
                same: |a, b| a.$field == b.$field,
            },)+)*
        ];
    };
}

animated_fields! {
    Translate: translate_x as Number, translate_y as Number;
    Scale: scale_x as Number, scale_y as Number;
    Rotate: rotate as Number;
    Opacity: opacity as Number;
    ImageOpacity: image_opacity as Number;
    Blur: blur as Number;
    BackdropBlur: backdrop_blur as Number;
    Background: background as Color;
    BorderColor: border_color as Color;
    OutlineColor: outline_color as Color;
    TextColor: color as Color;
    FontWeight: font_weight as Weight;
    FontAxes: font_axes as Axes;
    Radius: radius_top_left as Number, radius_top_right as Number,
        radius_bottom_right as Number, radius_bottom_left as Number;
    Shadow: shadows as Shadows;
    Width: width as Length;
    Height: height as Length;
    Margin: margin_top as Length, margin_right as Length,
        margin_bottom as Length, margin_left as Length;
    Padding: padding_top as Number, padding_right as Number,
        padding_bottom as Number, padding_left as Number;
    Inset: inset_top as Length, inset_right as Length,
        inset_bottom as Length, inset_left as Length;
    Gap: row_gap as Number, column_gap as Number;
    SvgFill: fill_paint as Paint;
    SvgStroke: stroke_paint as Paint;
    StrokeWidth: stroke_width as Number;
    FillOpacity: fill_opacity as Number;
    StrokeOpacity: stroke_opacity as Number;
    BorderWidth: border_top_width as Number, border_right_width as Number,
        border_bottom_width as Number, border_left_width as Number;
}

impl ComputedStyle {
    /// How a change of `prop` should go. The last thing said about it counts.
    pub fn transition_for(&self, prop: Prop) -> Option<Transition> {
        self.transitions
            .iter()
            .rev()
            .find(|(p, _)| p.is_none_or(|p| p == prop))
            .map(|(_, t)| *t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weight_is_a_whole_number_on_the_way() {
        let (a, b) = (
            AnimatedValue::Weight(FontWeight(400)),
            AnimatedValue::Weight(FontWeight(700)),
        );
        assert_eq!(
            a.interpolate(&b, 0.509),
            Some(AnimatedValue::Weight(FontWeight(553)))
        );
        assert_eq!(a.distance(&b), 300.0);
    }

    #[test]
    fn easings_start_and_end_where_they_should() {
        for e in [
            Easing::Linear,
            Easing::Ease,
            Easing::EASE_IN,
            Easing::EASE_OUT,
            Easing::EASE_IN_OUT,
        ] {
            assert_eq!(e.apply(0.0), 0.0);
            assert_eq!(e.apply(1.0), 1.0);
            let mut last = 0.0;
            for i in 1..100 {
                let v = e.apply(i as f32 / 100.0);
                assert!(v >= last, "{e:?} goes back at {i}");
                last = v;
            }
        }
        // The curves solved by bisection to high precision.
        assert!((Easing::Ease.apply(0.5) - 0.8024).abs() < 0.001);
        assert!((Easing::EASE_OUT.apply(0.5) - 0.68464).abs() < 0.001);
        assert!((Easing::EASE_IN.apply(0.5) - 0.31536).abs() < 0.001);
        assert!((Easing::EASE_IN_OUT.apply(0.5) - 0.5).abs() < 0.001);
    }

    #[test]
    fn a_color_from_transparent_stays_that_color() {
        let blue = Color::new(0.0, 0.0, 1.0, 1.0);
        let c = mix_premultiplied(Color::TRANSPARENT, blue, 0.25);
        assert!((c.b - 1.0).abs() < 1e-6, "{c:?}");
        assert!((c.a - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_shadow_that_comes_fades_in() {
        let s = BoxShadow {
            blur: 10.0,
            color: Color::BLACK,
            ..Default::default()
        };
        let AnimatedValue::Shadows(v) = AnimatedValue::Shadows(vec![])
            .interpolate(&AnimatedValue::Shadows(vec![s]), 0.5)
            .unwrap()
        else {
            panic!();
        };
        assert_eq!(v[0].blur, 10.0);
        assert!((v[0].color.a - 0.5).abs() < 1e-6);
    }

    #[test]
    fn what_has_nothing_in_between_is_said_so() {
        let auto = AnimatedValue::Length(Length::Auto);
        assert!(
            auto.interpolate(&AnimatedValue::Length(Length::Px(1.0)), 0.5)
                .is_none()
        );
    }

    #[test]
    fn the_last_transition_said_counts() {
        let fast = Transition::ms(1);
        let slow = Transition::ms(2);
        let c = ComputedStyle {
            transitions: vec![(None, slow), (Some(Prop::Scale), fast)],
            ..Default::default()
        };
        assert_eq!(c.transition_for(Prop::Scale), Some(fast));
        assert_eq!(c.transition_for(Prop::Opacity), Some(slow));
        assert_eq!(ComputedStyle::default().transition_for(Prop::Scale), None);
    }
}
