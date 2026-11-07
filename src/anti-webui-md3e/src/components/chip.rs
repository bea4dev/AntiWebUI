//! Chips, with the shapes that change of the Expressive overloads. Sizes, colors and timing are those
//! of Jetpack Compose. See `docs/design/20-md3e-components.md`.

use super::icon::sized_icon;
use super::surface::{Surface, When, filling_icon, progress_of, round, surface, when};
use crate::color::{ColorScheme, color_scheme};
use crate::icons::{self, Symbol};
use crate::motion::{default_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;

const HEIGHT: f32 = 32.0;
const ICON: f32 = 18.0;
const CORNER: f32 = 8.0;
const CORNER_REST: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Assist,
    Suggestion,
    Filter,
    Input,
}

/// What else a chip has and how it looks.
#[derive(Clone, Default)]
pub struct ChipStyle {
    icon: Option<&'static Symbol>,
    trailing: Option<&'static Symbol>,
    on_trailing: Option<Rc<dyn Fn()>>,
    elevated: bool,
    disabled: Option<When>,
}

impl ChipStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// An icon before the label.
    pub fn icon(mut self, symbol: &'static Symbol) -> Self {
        self.icon = Some(symbol);
        self
    }

    /// An icon after the label, for an input chip usually what removes it. `f` runs when that is
    /// clicked, instead of the click of the chip.
    pub fn trailing(mut self, symbol: &'static Symbol, f: impl Fn() + 'static) -> Self {
        self.trailing = Some(symbol);
        self.on_trailing = Some(Rc::new(f));
        self
    }

    /// With a shadow and a background rather than an outline.
    pub fn elevated(mut self) -> Self {
        self.elevated = true;
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// A chip that does something related to what is around it.
pub fn assist_chip(label: impl IntoTextSource) -> Div {
    styled_assist_chip(label, ChipStyle::new())
}

pub fn styled_assist_chip(label: impl IntoTextSource, style: ChipStyle) -> Div {
    build(Kind::Assist, label, style, None)
}

/// A chip that proposes something.
pub fn suggestion_chip(label: impl IntoTextSource) -> Div {
    styled_suggestion_chip(label, ChipStyle::new())
}

pub fn styled_suggestion_chip(label: impl IntoTextSource, style: ChipStyle) -> Div {
    build(Kind::Suggestion, label, style, None)
}

/// A chip that is on or off, bound to `selected`, which it flips when it is clicked. Without an icon
/// of its own, a check grows in front of the label while it is on. The behavior is the standard
/// one's (`toggle_with`).
pub fn filter_chip(label: impl IntoTextSource, selected: impl Into<Binding<bool>>) -> Div {
    styled_filter_chip(label, selected, ChipStyle::new())
}

pub fn styled_filter_chip(
    label: impl IntoTextSource,
    selected: impl Into<Binding<bool>>,
    style: ChipStyle,
) -> Div {
    toggle_with(selected, move |t| {
        build(Kind::Filter, label, style, Some(Rc::new(move || t.on())))
    })
}

/// A chip for something that was put in, like a person to send something to.
pub fn input_chip(label: impl IntoTextSource, style: ChipStyle) -> Div {
    build(Kind::Input, label, style, None)
}

/// An input chip that is on or off, bound to `selected`, which it flips when it is clicked.
pub fn selectable_input_chip(
    label: impl IntoTextSource,
    selected: impl Into<Binding<bool>>,
    style: ChipStyle,
) -> Div {
    toggle_with(selected, move |t| {
        build(Kind::Input, label, style, Some(Rc::new(move || t.on())))
    })
}

/// Background, label, leading icon, trailing icon and outline.
fn colors(kind: Kind, elevated: bool, on: bool, off: bool, c: &ColorScheme) -> [Color; 5] {
    let none = Color::TRANSPARENT;
    let selects = matches!(kind, Kind::Filter | Kind::Input);
    if off {
        let faded = c.on_surface.with_alpha(0.38);
        let weak = c.on_surface.with_alpha(0.12);
        let background = if elevated || (selects && on) {
            weak
        } else {
            none
        };
        let outline = if elevated || (selects && on) {
            none
        } else {
            weak
        };
        return [background, faded, faded, faded, outline];
    }
    let background = match (elevated, selects && on) {
        (_, true) => c.secondary_container,
        (true, false) => c.surface_container_low,
        (false, false) => none,
    };
    let outline = if elevated || (selects && on) {
        none
    } else {
        c.outline_variant
    };
    let (label, leading, trailing) = match (kind, on) {
        (Kind::Assist, _) => (c.on_surface, c.primary, c.on_surface_variant),
        (Kind::Suggestion, _) => (c.on_surface_variant, c.primary, c.on_surface_variant),
        (Kind::Filter, false) => (
            c.on_surface_variant,
            c.on_surface_variant,
            c.on_surface_variant,
        ),
        (Kind::Filter, true) => (
            c.on_secondary_container,
            c.on_secondary_container,
            c.on_secondary_container,
        ),
        (Kind::Input, false) => (
            c.on_surface_variant,
            c.on_surface_variant,
            c.on_surface_variant,
        ),
        (Kind::Input, true) => (
            c.on_secondary_container,
            c.primary,
            c.on_secondary_container,
        ),
    };
    [background, label, leading, trailing, outline]
}

fn build(kind: Kind, label: impl IntoTextSource, style: ChipStyle, selected: Option<When>) -> Div {
    let ChipStyle {
        icon,
        trailing,
        on_trailing,
        elevated,
        disabled,
    } = style;
    let selects = selected.is_some();
    let morphs = matches!(kind, Kind::Filter | Kind::Input);
    let is_on = when(selected.clone());
    let is_off = when(disabled.clone());
    let levels = match (elevated, kind) {
        (true, _) => (1.0, 2.0),
        (false, Kind::Filter) => (0.0, 1.0),
        (false, _) => (0.0, 0.0),
    };
    let (rest, checked_shape) = if morphs {
        (CORNER_REST, HEIGHT / 2.0)
    } else {
        (CORNER, CORNER)
    };
    let looks_off = is_off.clone();
    let chip = surface(Surface {
        colors: Rc::new(move |c, on| {
            let [background, label, _, _, outline] = colors(kind, elevated, on, looks_off(), c);
            (background, label, outline)
        }),
        disabled_content: |c| c.on_surface.with_alpha(0.38),
        rest: round(rest),
        pressed: round(CORNER),
        checked_shape: round(checked_shape),
        spring: fast_spatial(),
        levels,
        outline: if elevated { 0.0 } else { 1.0 },
        disabled,
        checked: selected.clone(),
    })
    .h(HEIGHT);

    let tint = |at: usize, on: When, off: When| {
        move |s: Style| {
            let c = color_scheme();
            s.text_color(colors(kind, elevated, on(), off(), &c)[at])
        }
    };
    let trail_pad = if trailing.is_some() { 8.0 } else { 16.0 };
    let leading_pad = if kind == Kind::Input && icon.is_none() {
        12.0
    } else {
        16.0
    };
    let trailing_end = if trailing.is_some() {
        8.0
    } else if kind == Kind::Input {
        12.0
    } else {
        16.0
    };
    let gap = if morphs { 4.0 } else { 8.0 };

    let chip = match icon {
        Some(symbol) => chip.pl(8.0).child(
            filling_icon(symbol, ICON, selected.clone().filter(|_| morphs))
                .mr(gap)
                .dyn_style(tint(2, is_on.clone(), is_off.clone())),
        ),
        // The check grows from nothing, and what is next to it makes room.
        None if kind == Kind::Filter && selects => {
            let t = progress_of(is_on.clone(), fast_spatial());
            let fade = progress_of(is_on.clone(), default_effects());
            chip.dyn_style(move |s| s.pl(leading_pad - 8.0 * t.get().clamp(0.0, 1.2)))
                .child(
                    div()
                        .flex_none()
                        .h(ICON)
                        .overflow_hidden()
                        .dyn_style(move |s| {
                            let v = t.get().max(0.0);
                            s.w(ICON * v)
                                .mr(gap * v)
                                .opacity(fade.get().clamp(0.0, 1.0))
                        })
                        .child(sized_icon(icons::CHECK, ICON).dyn_style(tint(
                            2,
                            is_on.clone(),
                            is_off.clone(),
                        ))),
                )
        }
        None => chip.pl(leading_pad),
    };
    let chip = chip.child(TextStyle::LABEL_LARGE.apply(text(label)));
    match trailing {
        Some(symbol) => {
            let run = on_trailing;
            chip.pr(trailing_end).child(
                sized_icon(symbol, ICON)
                    .ml(if morphs { gap } else { trail_pad })
                    .cursor(CursorIcon::Pointer)
                    .dyn_style(tint(3, is_on, is_off))
                    .on_click(move |e| {
                        e.stop_propagation();
                        if let Some(run) = &run {
                            run();
                        }
                    }),
            )
        }
        None => chip.pr(trailing_end),
    }
}
