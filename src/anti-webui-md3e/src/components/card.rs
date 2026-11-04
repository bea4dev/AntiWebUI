//! Cards and dividers. Sizes and colors are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::surface::{Surface, When, elevation_shadows, round, surface};
use crate::color::color_scheme;
use crate::motion::fast_spatial;
use anti_webui::prelude::*;
use std::rc::Rc;

const CORNER: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Filled,
    Elevated,
    Outlined,
}

fn plain(kind: CardKind) -> Div {
    crate::ensure_ready();
    let shadows = if kind == CardKind::Elevated {
        elevation_shadows(1.0)
    } else {
        Vec::new()
    };
    div()
        .flex()
        .flex_col()
        .rounded(CORNER)
        .overflow_hidden()
        .shadows(shadows)
        .dyn_style(move |s| {
            let c = color_scheme();
            let s = s.text_color(c.on_surface);
            match kind {
                CardKind::Filled => s.bg(c.surface_container_highest),
                CardKind::Elevated => s.bg(c.surface_container_low),
                CardKind::Outlined => s.bg(c.surface).border(1.0).border_color(c.outline_variant),
            }
        })
}

/// A card on a slightly different background.
pub fn filled_card() -> Div {
    plain(CardKind::Filled)
}

/// A card with a shadow.
pub fn elevated_card() -> Div {
    plain(CardKind::Elevated)
}

/// A card with an outline.
pub fn outlined_card() -> Div {
    plain(CardKind::Outlined)
}

/// A card that can be clicked, which reacts like a button: with a state layer, ripples and a shadow
/// that rises under the pointer.
pub fn clickable_card(kind: CardKind) -> Div {
    styled_clickable_card(kind, None)
}

/// A card that can be clicked, unless `disabled` says otherwise.
pub fn styled_clickable_card(kind: CardKind, disabled: Option<When>) -> Div {
    surface(Surface {
        colors: Rc::new(move |c, _| {
            let none = Color::TRANSPARENT;
            match kind {
                CardKind::Filled => (c.surface_container_highest, c.on_surface, none),
                CardKind::Elevated => (c.surface_container_low, c.on_surface, none),
                CardKind::Outlined => (c.surface, c.on_surface, c.outline_variant),
            }
        }),
        disabled_content: |c| c.on_surface.with_alpha(0.38),
        rest: round(CORNER),
        pressed: round(CORNER),
        checked_shape: round(CORNER),
        spring: fast_spatial(),
        levels: match kind {
            CardKind::Filled => (0.0, 1.0),
            CardKind::Elevated => (1.0, 2.0),
            CardKind::Outlined => (0.0, 0.0),
        },
        outline: if kind == CardKind::Outlined { 1.0 } else { 0.0 },
        disabled,
        checked: None,
    })
    .flex_col()
    .items_stretch()
    .justify_start()
    .overflow_hidden()
}

/// A thin line across what it is in.
pub fn divider() -> Div {
    div()
        .h(1.0)
        .flex_none()
        .w_full()
        .dyn_style(|s| s.bg(color_scheme().outline_variant))
}

/// A thin line along what it is in.
pub fn vertical_divider() -> Div {
    div()
        .w(1.0)
        .flex_none()
        .self_stretch()
        .dyn_style(|s| s.bg(color_scheme().outline_variant))
}
