//! Floating action buttons. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/19-buttons.md`.

use super::surface::{Colors, Surface, round, surface};
use crate::color::ColorScheme;
use crate::icons::Symbol;
use crate::motion::{default_effects, fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FabSize {
    #[default]
    Baseline,
    Medium,
    Large,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FabColor {
    #[default]
    PrimaryContainer,
    SecondaryContainer,
    TertiaryContainer,
    Primary,
    Secondary,
    Tertiary,
}

fn colors(color: FabColor, c: &ColorScheme) -> Colors {
    let none = Color::TRANSPARENT;
    match color {
        FabColor::PrimaryContainer => (c.primary_container, c.on_primary_container, none),
        FabColor::SecondaryContainer => (c.secondary_container, c.on_secondary_container, none),
        FabColor::TertiaryContainer => (c.tertiary_container, c.on_tertiary_container, none),
        FabColor::Primary => (c.primary, c.on_primary, none),
        FabColor::Secondary => (c.secondary, c.on_secondary, none),
        FabColor::Tertiary => (c.tertiary, c.on_tertiary, none),
    }
}

struct Metrics {
    size: f32,
    icon: f32,
    radius: f32,
    extended_icon: f32,
    extended_padding: f32,
    extended_gap: f32,
    text: TextStyle,
}

fn metrics(size: FabSize) -> Metrics {
    let (side, icon, radius, extended_icon, extended_padding, extended_gap, text) = match size {
        FabSize::Baseline => (56.0, 24.0, 16.0, 24.0, 16.0, 8.0, TextStyle::TITLE_MEDIUM),
        FabSize::Medium => (80.0, 28.0, 20.0, 28.0, 26.0, 16.0, TextStyle::TITLE_LARGE),
        FabSize::Large => (
            96.0,
            36.0,
            28.0,
            32.0,
            28.0,
            20.0,
            TextStyle::HEADLINE_SMALL,
        ),
    };
    Metrics {
        size: side,
        icon,
        radius,
        extended_icon,
        extended_padding,
        extended_gap,
        text,
    }
}

/// How a floating action button looks.
#[derive(Clone, Default)]
pub struct FabStyle {
    size: FabSize,
    color: FabColor,
    expanded: Option<Rc<dyn Fn() -> bool>>,
}

impl FabStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(mut self, size: FabSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: FabColor) -> Self {
        self.color = color;
        self
    }

    /// For an extended button: shows its label only while `f` returns true.
    pub fn expanded_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.expanded = Some(Rc::new(f));
        self
    }
}

fn fab_surface(style: &FabStyle, radius: f32) -> Div {
    let color = style.color;
    surface(Surface {
        colors: Rc::new(move |c, _| colors(color, c)),
        disabled_content: |c| c.on_surface_variant.with_alpha(0.38),
        rest: round(radius),
        pressed: round(radius),
        checked_shape: round(radius),
        spring: default_effects(),
        levels: (3.0, 4.0),
        outline: 0.0,
        disabled: None,
        checked: None,
    })
}

/// The button for the main thing to do on a screen.
pub fn fab(symbol: &'static Symbol) -> Div {
    styled_fab(symbol, FabStyle::new())
}

/// A floating action button of any size and color.
pub fn styled_fab(symbol: &'static Symbol, style: FabStyle) -> Div {
    let metrics = metrics(style.size);
    fab_surface(&style, metrics.radius)
        .size(metrics.size)
        .child(super::sized_icon(symbol, metrics.icon))
}

/// A floating action button with a label.
pub fn extended_fab(symbol: &'static Symbol, label: impl IntoTextSource) -> Div {
    styled_extended_fab(symbol, label, FabStyle::new())
}

/// An extended floating action button of any size and color, which can close over its label.
pub fn styled_extended_fab(
    symbol: &'static Symbol,
    label: impl IntoTextSource,
    style: FabStyle,
) -> Div {
    let metrics = metrics(style.size);
    let expanded = style.expanded.clone();
    let is_expanded = move || expanded.as_ref().is_none_or(|f| f());
    let at_start = if untrack(&is_expanded) { 1.0f32 } else { 0.0 };
    let open = animated(at_start);
    let shown = animated(at_start);
    let wanted = Cell::new(at_start);
    effect(move || {
        let target = if is_expanded() { 1.0 } else { 0.0 };
        if wanted.replace(target) != target {
            untrack(|| {
                open.spring_to(target, fast_spatial());
                shown.spring_to(target, fast_effects());
            });
        }
    });
    let inner = node_ref();
    // Once it is all the way open it takes the room it needs, in case the label changes.
    let room = div()
        .flex()
        .flex_row()
        .flex_none()
        .overflow_hidden()
        .dyn_style(move |s| {
            let s = s.opacity(shown.get().clamp(0.0, 1.0));
            let open = open.get();
            if open == 1.0 {
                return s;
            }
            let width = inner
                .rect()
                .map_or(0.0, |r| with_doc(|d| d.scale.to_logical(r.width as f32)));
            s.w((width * open).max(0.0))
        })
        .child(
            div()
                .flex()
                .flex_none()
                .pl(metrics.extended_gap)
                .node_ref(inner)
                .child(metrics.text.apply(text(label)).whitespace_nowrap()),
        );
    fab_surface(&style, metrics.radius)
        .h(metrics.size)
        .min_w(metrics.size)
        .px(metrics.extended_padding)
        .child(super::sized_icon(symbol, metrics.extended_icon))
        .child(room)
}
