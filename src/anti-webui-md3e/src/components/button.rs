//! Buttons. Sizes, colors and timing are those of Jetpack Compose. See `docs/design/15-md3e.md`.

use super::surface::{Colors, Corners, Surface, filling_icon, round, surface};
use crate::color::ColorScheme;
use crate::icons::Symbol;
use crate::motion::{default_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Filled,
    Tonal,
    Elevated,
    Outlined,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    ExtraSmall,
    #[default]
    Small,
    Medium,
    Large,
    ExtraLarge,
}

struct Metrics {
    height: f32,
    padding: f32,
    square_radius: f32,
    pressed_radius: f32,
    outline: f32,
    text: TextStyle,
    icon: f32,
    icon_gap: f32,
}

impl ButtonSize {
    fn metrics(self) -> Metrics {
        let (height, padding, square_radius, pressed_radius, outline, text, icon, icon_gap) =
            match self {
                ButtonSize::ExtraSmall => (
                    32.0,
                    16.0,
                    12.0,
                    8.0,
                    1.0,
                    TextStyle::LABEL_LARGE,
                    20.0,
                    4.0,
                ),
                ButtonSize::Small => (
                    40.0,
                    16.0,
                    12.0,
                    8.0,
                    1.0,
                    TextStyle::LABEL_LARGE,
                    20.0,
                    8.0,
                ),
                ButtonSize::Medium => (
                    56.0,
                    24.0,
                    16.0,
                    12.0,
                    1.0,
                    TextStyle::TITLE_MEDIUM,
                    24.0,
                    8.0,
                ),
                ButtonSize::Large => (
                    96.0,
                    48.0,
                    28.0,
                    16.0,
                    2.0,
                    TextStyle::HEADLINE_SMALL,
                    32.0,
                    12.0,
                ),
                ButtonSize::ExtraLarge => (
                    136.0,
                    64.0,
                    28.0,
                    16.0,
                    3.0,
                    TextStyle::HEADLINE_LARGE,
                    40.0,
                    16.0,
                ),
            };
        Metrics {
            height,
            padding,
            square_radius,
            pressed_radius,
            outline,
            text,
            icon,
            icon_gap,
        }
    }
}

/// How a button looks.
#[derive(Clone)]
pub struct ButtonStyle {
    kind: ButtonKind,
    size: ButtonSize,
    square: bool,
    disabled: Option<Rc<dyn Fn() -> bool>>,
    icon: Option<&'static Symbol>,
    trailing_icon: Option<&'static Symbol>,
    joined: Option<Joined>,
}

/// Where a button is in a row of connected ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Joined {
    Alone,
    First,
    Middle,
    Last,
}

impl Joined {
    /// The shapes at rest, pressed and checked. The tokens are only there for the small size.
    fn shapes(self, full: f32) -> (Corners, Corners, Corners) {
        let sides = |inner: f32| match self {
            Joined::Alone => [full; 4],
            Joined::First => [full, inner, inner, full],
            Joined::Middle => [inner; 4],
            Joined::Last => [inner, full, full, inner],
        };
        (sides(8.0), sides(4.0), [full; 4])
    }
}

impl ButtonStyle {
    pub fn new(kind: ButtonKind) -> Self {
        Self {
            kind,
            size: ButtonSize::default(),
            square: false,
            disabled: None,
            icon: None,
            trailing_icon: None,
            joined: None,
        }
    }

    pub fn filled() -> Self {
        Self::new(ButtonKind::Filled)
    }

    pub fn tonal() -> Self {
        Self::new(ButtonKind::Tonal)
    }

    pub fn elevated() -> Self {
        Self::new(ButtonKind::Elevated)
    }

    pub fn outlined() -> Self {
        Self::new(ButtonKind::Outlined)
    }

    pub fn text() -> Self {
        Self::new(ButtonKind::Text)
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Corners that are only rounded, rather than a pill.
    pub fn square(mut self) -> Self {
        self.square = true;
        self
    }

    /// An icon in front of the label.
    pub fn icon(mut self, symbol: &'static Symbol) -> Self {
        self.icon = Some(symbol);
        self
    }

    /// An icon behind the label.
    pub fn trailing_icon(mut self, symbol: &'static Symbol) -> Self {
        self.trailing_icon = Some(symbol);
        self
    }

    pub(super) fn joined(mut self, joined: Joined) -> Self {
        self.joined = Some(joined);
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// The colors of a kind of button: background, label and border.
pub(super) fn colors(kind: ButtonKind, c: &ColorScheme) -> Colors {
    let none = Color::TRANSPARENT;
    match kind {
        ButtonKind::Filled => (c.primary, c.on_primary, none),
        ButtonKind::Tonal => (c.secondary_container, c.on_secondary_container, none),
        ButtonKind::Elevated => (c.surface_container_low, c.primary, none),
        ButtonKind::Outlined => (none, c.on_surface_variant, c.outline_variant),
        ButtonKind::Text => (none, c.primary, none),
    }
}

/// The same for a toggle button.
fn toggle_colors(kind: ButtonKind, c: &ColorScheme, checked: bool) -> Colors {
    let none = Color::TRANSPARENT;
    match (kind, checked) {
        (ButtonKind::Filled, false) => (c.surface_container, c.on_surface_variant, none),
        (ButtonKind::Filled | ButtonKind::Elevated, true) => (c.primary, c.on_primary, none),
        (ButtonKind::Tonal, true) => (c.secondary, c.on_secondary, none),
        (ButtonKind::Outlined, true) => (c.inverse_surface, c.inverse_on_surface, none),
        (ButtonKind::Text, true) => (c.secondary_container, c.on_secondary_container, none),
        (kind, false) => colors(kind, c),
    }
}

/// How high a kind of button is at rest and under the pointer.
pub(super) fn elevations(kind: ButtonKind) -> (f32, f32) {
    match kind {
        ButtonKind::Filled | ButtonKind::Tonal => (0.0, 1.0),
        ButtonKind::Elevated => (1.0, 2.0),
        ButtonKind::Outlined | ButtonKind::Text => (0.0, 0.0),
    }
}

/// A filled button in the default size: the one for the main action.
pub fn button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::filled())
}

pub fn tonal_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::tonal())
}

pub fn elevated_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::elevated())
}

pub fn outlined_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::outlined())
}

pub fn text_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::text())
}

/// A button of any kind and size.
pub fn styled_button(label: impl IntoTextSource, style: ButtonStyle) -> Div {
    build(label, style, None)
}

/// A filled button that stays on or off: what it shows follows `checked`, which the program changes
/// in `on_click`.
pub fn toggle_button(label: impl IntoTextSource, checked: impl Fn() -> bool + 'static) -> Div {
    styled_toggle_button(label, checked, ButtonStyle::filled())
}

/// A toggle button of any kind and size.
pub fn styled_toggle_button(
    label: impl IntoTextSource,
    checked: impl Fn() -> bool + 'static,
    style: ButtonStyle,
) -> Div {
    build(label, style, Some(Rc::new(checked)))
}

fn build(
    label: impl IntoTextSource,
    style: ButtonStyle,
    checked: Option<Rc<dyn Fn() -> bool>>,
) -> Div {
    let ButtonStyle {
        kind,
        size,
        square,
        disabled,
        icon,
        trailing_icon,
        joined,
    } = style;
    let metrics = size.metrics();
    let full = metrics.height / 2.0;
    let toggles = checked.is_some();
    // Round first and rounded when checked, or the other way around.
    let (usual, on_check) = if square {
        (metrics.square_radius, full)
    } else {
        (full, metrics.square_radius)
    };
    let (rest, pressed, checked_shape) = match joined {
        Some(joined) => joined.shapes(full),
        None => (round(usual), round(metrics.pressed_radius), round(on_check)),
    };
    let root = surface(Surface {
        colors: Rc::new(move |c, on| {
            if toggles {
                toggle_colors(kind, c, on)
            } else {
                colors(kind, c)
            }
        }),
        disabled_content: |c| c.on_surface_variant.with_alpha(0.38),
        rest,
        pressed,
        checked_shape,
        // Plain buttons don't bounce, and toggle buttons do: see `Button.kt` and `ToggleButton.kt`.
        spring: if toggles {
            fast_spatial()
        } else {
            default_effects()
        },
        levels: elevations(kind),
        outline: if kind == ButtonKind::Outlined {
            metrics.outline
        } else {
            0.0
        },
        disabled,
        checked: checked.clone(),
    })
    .gap(metrics.icon_gap)
    .h(metrics.height)
    .px(metrics.padding);
    let icon_of = |symbol| filling_icon(symbol, metrics.icon, checked.clone());
    let root = match icon {
        Some(symbol) => root.child(icon_of(symbol)),
        None => root,
    };
    let root = root.child(metrics.text.apply(text(label)));
    match trailing_icon {
        Some(symbol) => root.child(icon_of(symbol)),
        None => root,
    }
}
