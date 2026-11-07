//! Icon buttons. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/19-buttons.md`.

use super::button::ButtonSize;
use super::surface::{Colors, Surface, filling_icon, round, surface};
use crate::color::ColorScheme;
use crate::icons::Symbol;
use crate::motion::default_effects;
use anti_webui::prelude::*;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconButtonKind {
    Standard,
    Filled,
    Tonal,
    Outlined,
}

/// How much room there is next to the icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconButtonWidth {
    Narrow,
    #[default]
    Default,
    Wide,
}

struct Metrics {
    height: f32,
    icon: f32,
    /// Next to the icon, for each width.
    padding: [f32; 3],
    square_radius: f32,
    pressed_radius: f32,
    outline: f32,
}

fn metrics(size: ButtonSize) -> Metrics {
    let (height, icon, padding, square_radius, pressed_radius, outline) = match size {
        ButtonSize::ExtraSmall => (32.0, 20.0, [4.0, 6.0, 10.0], 12.0, 8.0, 1.0),
        ButtonSize::Small => (40.0, 24.0, [4.0, 8.0, 14.0], 12.0, 8.0, 1.0),
        ButtonSize::Medium => (56.0, 24.0, [12.0, 16.0, 24.0], 16.0, 12.0, 1.0),
        ButtonSize::Large => (96.0, 32.0, [16.0, 32.0, 48.0], 28.0, 16.0, 2.0),
        ButtonSize::ExtraLarge => (136.0, 40.0, [32.0, 48.0, 72.0], 28.0, 16.0, 3.0),
    };
    Metrics {
        height,
        icon,
        padding,
        square_radius,
        pressed_radius,
        outline,
    }
}

/// How an icon button looks.
#[derive(Clone)]
pub struct IconButtonStyle {
    kind: IconButtonKind,
    size: ButtonSize,
    width: IconButtonWidth,
    square: bool,
    disabled: Option<Rc<dyn Fn() -> bool>>,
}

impl IconButtonStyle {
    pub fn new(kind: IconButtonKind) -> Self {
        Self {
            kind,
            size: ButtonSize::default(),
            width: IconButtonWidth::default(),
            square: false,
            disabled: None,
        }
    }

    pub fn standard() -> Self {
        Self::new(IconButtonKind::Standard)
    }

    pub fn filled() -> Self {
        Self::new(IconButtonKind::Filled)
    }

    pub fn tonal() -> Self {
        Self::new(IconButtonKind::Tonal)
    }

    pub fn outlined() -> Self {
        Self::new(IconButtonKind::Outlined)
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn width(mut self, width: IconButtonWidth) -> Self {
        self.width = width;
        self
    }

    /// Corners that are only rounded, rather than a pill.
    pub fn square(mut self) -> Self {
        self.square = true;
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

fn colors(kind: IconButtonKind, c: &ColorScheme) -> Colors {
    let none = Color::TRANSPARENT;
    match kind {
        IconButtonKind::Standard => (none, c.on_surface_variant, none),
        IconButtonKind::Filled => (c.primary, c.on_primary, none),
        IconButtonKind::Tonal => (c.secondary_container, c.on_secondary_container, none),
        IconButtonKind::Outlined => (none, c.on_surface_variant, c.outline_variant),
    }
}

fn toggle_colors(kind: IconButtonKind, c: &ColorScheme, checked: bool) -> Colors {
    let none = Color::TRANSPARENT;
    match (kind, checked) {
        (IconButtonKind::Standard, true) => (none, c.primary, none),
        (IconButtonKind::Filled, false) => (c.surface_container, c.on_surface_variant, none),
        (IconButtonKind::Tonal, true) => (c.secondary, c.on_secondary, none),
        (IconButtonKind::Outlined, true) => (c.inverse_surface, c.inverse_on_surface, none),
        (kind, _) => colors(kind, c),
    }
}

/// An icon button without a background, for what is less important.
pub fn icon_button(symbol: &'static Symbol) -> Div {
    styled_icon_button(symbol, IconButtonStyle::standard())
}

pub fn filled_icon_button(symbol: &'static Symbol) -> Div {
    styled_icon_button(symbol, IconButtonStyle::filled())
}

pub fn tonal_icon_button(symbol: &'static Symbol) -> Div {
    styled_icon_button(symbol, IconButtonStyle::tonal())
}

pub fn outlined_icon_button(symbol: &'static Symbol) -> Div {
    styled_icon_button(symbol, IconButtonStyle::outlined())
}

/// An icon button of any kind and size.
pub fn styled_icon_button(symbol: &'static Symbol, style: IconButtonStyle) -> Div {
    build(symbol, style, None)
}

/// An icon button that stays on or off, bound to `on`, which it flips when it is clicked. Its icon
/// fills in while it is on. The behavior is the standard one's (`toggle_with`).
pub fn icon_toggle_button(
    symbol: &'static Symbol,
    on: impl Into<Binding<bool>>,
    style: IconButtonStyle,
) -> Div {
    toggle_with(on, move |t| {
        build(symbol, style, Some(Rc::new(move || t.on())))
    })
}

fn build(
    symbol: &'static Symbol,
    style: IconButtonStyle,
    checked: Option<Rc<dyn Fn() -> bool>>,
) -> Div {
    let IconButtonStyle {
        kind,
        size,
        width,
        square,
        disabled,
    } = style;
    let metrics = metrics(size);
    let padding = metrics.padding[width as usize];
    let full = metrics.height.min(metrics.icon + padding * 2.0) / 2.0;
    let toggles = checked.is_some();
    let (rest, on_check) = if square {
        (metrics.square_radius, full)
    } else {
        (full, metrics.square_radius)
    };
    surface(Surface {
        colors: Rc::new(move |c, on| {
            if toggles {
                toggle_colors(kind, c, on)
            } else {
                colors(kind, c)
            }
        }),
        disabled_content: |c| c.on_surface.with_alpha(0.38),
        rest: round(rest),
        pressed: round(metrics.pressed_radius),
        checked_shape: round(on_check),
        // On purpose, says `IconButton.kt`: what bounces here would look wrong this small.
        spring: default_effects(),
        levels: (0.0, 0.0),
        outline: if kind == IconButtonKind::Outlined {
            metrics.outline
        } else {
            0.0
        },
        disabled,
        checked: checked.clone(),
    })
    .h(metrics.height)
    .px(padding)
    .child(filling_icon(symbol, metrics.icon, checked))
}
