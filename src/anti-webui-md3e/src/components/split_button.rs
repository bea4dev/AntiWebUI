//! Split buttons: a button and a smaller one next to it for more. Sizes, colors and timing are those
//! of Jetpack Compose; the arrow turns like in its sample. See `docs/design/20-md3e-components.md`.

use super::button::{ButtonKind, ButtonSize, colors as kind_colors, elevations};
use super::icon::sized_icon;
use super::surface::{Surface, When, following, surface};
use crate::color::ColorScheme;
use crate::icons;
use crate::icons::Symbol;
use crate::motion::default_effects;
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;

const GAP: f32 = 2.0;

struct Metrics {
    height: f32,
    inner: f32,
    pressed: f32,
    leading: (f32, f32),
    trailing: f32,
    icon: f32,
    text: TextStyle,
}

fn metrics(size: ButtonSize) -> Metrics {
    let (height, inner, pressed, leading, trailing, icon, text) = match size {
        ButtonSize::ExtraSmall => (
            32.0,
            4.0,
            8.0,
            (12.0, 10.0),
            13.0,
            22.0,
            TextStyle::LABEL_LARGE,
        ),
        ButtonSize::Small => (
            40.0,
            4.0,
            12.0,
            (16.0, 12.0),
            13.0,
            22.0,
            TextStyle::LABEL_LARGE,
        ),
        ButtonSize::Medium => (
            56.0,
            4.0,
            12.0,
            (24.0, 24.0),
            15.0,
            26.0,
            TextStyle::TITLE_MEDIUM,
        ),
        ButtonSize::Large => (
            96.0,
            8.0,
            20.0,
            (48.0, 48.0),
            29.0,
            38.0,
            TextStyle::HEADLINE_SMALL,
        ),
        ButtonSize::ExtraLarge => (
            136.0,
            12.0,
            20.0,
            (64.0, 64.0),
            43.0,
            50.0,
            TextStyle::HEADLINE_LARGE,
        ),
    };
    Metrics {
        height,
        inner,
        pressed,
        leading,
        trailing,
        icon,
        text,
    }
}

/// How a split button looks.
#[derive(Clone)]
pub struct SplitButtonStyle {
    kind: ButtonKind,
    size: ButtonSize,
    icon: Option<&'static Symbol>,
    disabled: Option<When>,
}

impl SplitButtonStyle {
    pub fn new(kind: ButtonKind) -> Self {
        Self {
            kind,
            size: ButtonSize::default(),
            icon: None,
            disabled: None,
        }
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// An icon before the label.
    pub fn icon(mut self, symbol: &'static Symbol) -> Self {
        self.icon = Some(symbol);
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// A split button: `on_click` for the main part, `on_more` for the part with the arrow, which turns
/// while `open` returns true and is usually what opens a [`menu`](super::menu).
pub fn split_button(
    label: impl IntoTextSource,
    open: impl Fn() -> bool + 'static,
    on_click: impl Fn() + 'static,
    on_more: impl Fn() + 'static,
    style: SplitButtonStyle,
) -> Div {
    let SplitButtonStyle {
        kind,
        size,
        icon,
        disabled,
    } = style;
    let m = metrics(size);
    let full = m.height / 2.0;
    let open: When = Rc::new(open);
    let turn_open = open.clone();
    let turn = following(
        move || if turn_open() { 180.0f32 } else { 0.0 },
        Spring::from_ratio(1500.0, 1.0),
    );
    let colors = move |c: &ColorScheme, _: bool| kind_colors(kind, c);
    let levels = elevations(kind);
    let outline = if kind == ButtonKind::Outlined {
        1.0
    } else {
        0.0
    };
    let leading = surface(Surface {
        colors: Rc::new(colors),
        disabled_content: |c| c.on_surface.with_alpha(0.38),
        rest: [full, m.inner, m.inner, full],
        pressed: [full, m.pressed, m.pressed, full],
        checked_shape: [full, m.inner, m.inner, full],
        spring: default_effects(),
        levels,
        outline,
        disabled: disabled.clone(),
        checked: None,
    })
    .h(m.height)
    .pl(m.leading.0)
    .pr(m.leading.1)
    .gap(8.0)
    .on_click(move |_| on_click())
    .children(icon.map(|symbol| sized_icon(symbol, m.icon * 0.9)))
    .child(m.text.apply(text(label)));
    let trailing = surface(Surface {
        colors: Rc::new(colors),
        disabled_content: |c| c.on_surface.with_alpha(0.38),
        rest: [m.inner, full, full, m.inner],
        pressed: [m.pressed, full, full, m.pressed],
        checked_shape: [full; 4],
        spring: default_effects(),
        levels,
        outline,
        disabled,
        checked: Some(open),
    })
    .h(m.height)
    .px(m.trailing)
    .on_click(move |_| on_more())
    .child(sized_icon(icons::KEYBOARD_ARROW_DOWN, m.icon).dyn_style(move |s| s.rotate(turn.get())));
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(GAP)
        .child(leading)
        .child(trailing)
}
