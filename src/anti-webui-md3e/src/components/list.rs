//! List items, with the shapes that change of Expressive. Sizes, colors and timing are those of
//! Jetpack Compose. See `docs/design/20-md3e-components.md`.

use super::surface::{Corners, Ink, When, focus_ring, following, round, when};
use crate::color::{ColorScheme, color_scheme};
use crate::motion::{default_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const BASE: f32 = 4.0;
const HOVERED: f32 = 12.0;
const ACTIVE: f32 = 16.0;

/// What a list item says and has around that.
#[derive(Default)]
pub struct ListItemStyle {
    overline: Option<String>,
    supporting: Option<String>,
    leading: Option<View>,
    trailing: Option<View>,
    selected: Option<When>,
    disabled: Option<When>,
    interactive: bool,
    /// Corners while it does nothing, when it is part of a group.
    corners: Option<Corners>,
}

impl ListItemStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// A small line above the headline.
    pub fn overline(mut self, text: impl Into<String>) -> Self {
        self.overline = Some(text.into());
        self
    }

    /// A line or more below the headline.
    pub fn supporting(mut self, text: impl Into<String>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    /// Something before the text, usually an icon or an avatar.
    pub fn leading(mut self, view: impl IntoView) -> Self {
        self.leading = Some(view.into_view());
        self
    }

    /// Something after the text, like a switch or a short text.
    pub fn trailing(mut self, view: impl IntoView) -> Self {
        self.trailing = Some(view.into_view());
        self
    }

    /// Reacts to the pointer and the keyboard like a button.
    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// In the selected colors while `f` returns true. It also makes the item interactive.
    pub fn selected_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.selected = Some(Rc::new(f));
        self.interactive = true;
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// A row of a list.
pub fn list_item(headline: impl IntoTextSource) -> Div {
    styled_list_item(headline, ListItemStyle::new())
}

/// The colors of the container, the headline and the rest.
fn colors(c: &ColorScheme, selected: bool, off: bool) -> (Color, Color, Color) {
    if off {
        let faded = c.on_surface.with_alpha(0.38);
        (Color::TRANSPARENT, faded, faded)
    } else if selected {
        (
            c.secondary_container,
            c.on_secondary_container,
            c.on_secondary_container,
        )
    } else {
        (Color::TRANSPARENT, c.on_surface, c.on_surface_variant)
    }
}

pub fn styled_list_item(headline: impl IntoTextSource, style: ListItemStyle) -> Div {
    crate::ensure_ready();
    let ListItemStyle {
        overline,
        supporting,
        leading,
        trailing,
        selected,
        disabled,
        interactive,
        corners,
    } = style;
    let is_selected = when(selected);
    let is_off = when(disabled);
    let lines = 1 + overline.is_some() as u32 + supporting.is_some() as u32;
    let min_height = match lines {
        1 => 56.0,
        2 => 72.0,
        _ => 88.0,
    };
    let (selected_now, off_now) = (is_selected.clone(), is_off.clone());
    let palette = move || colors(&color_scheme(), selected_now(), off_now());
    let background = following(
        {
            let palette = palette.clone();
            move || palette().0
        },
        default_effects(),
    );
    let main = following(
        {
            let palette = palette.clone();
            move || palette().1
        },
        default_effects(),
    );
    let rest = following(move || palette().2, default_effects());

    let words = div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .flex_shrink(1.0)
        .min_w(0.0)
        .justify_center()
        .children(overline.map(|line| {
            TextStyle::LABEL_SMALL
                .apply(div().child(text(line)))
                .dyn_style(move |s| s.text_color(rest.get()))
        }))
        .child(
            TextStyle::BODY_LARGE
                .apply(div().child(text(headline)))
                .dyn_style(move |s| s.text_color(main.get())),
        )
        .children(supporting.map(|line| {
            TextStyle::BODY_MEDIUM
                .apply(div().child(text(line)))
                .dyn_style(move |s| s.text_color(rest.get()))
        }));
    let side = |view: View| {
        div()
            .flex()
            .flex_none()
            .items_center()
            .dyn_style(move |s| s.text_color(rest.get()))
            .child(view)
    };
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(12.0)
        .min_h(min_height)
        .px(16.0)
        .py(10.0)
        .select_none()
        .children(leading.map(side))
        .child(words)
        .children(trailing.map(side));
    if !interactive {
        return row.dyn_style(move |s| s.bg(background.get()));
    }

    let ink_selected = is_selected.clone();
    let ink = Ink::new(
        move |c| {
            if ink_selected() {
                c.on_secondary_container
            } else {
                c.on_surface
            }
        },
        is_off.clone(),
    );
    let Ink {
        hovered,
        focused,
        pressed,
        ..
    } = ink;
    let base = corners.unwrap_or(round(BASE));
    let wanted = {
        let is_selected = is_selected.clone();
        move || {
            if pressed.get() || focused.get() || is_selected() {
                round(ACTIVE)
            } else if hovered.get() {
                round(HOVERED)
            } else {
                base
            }
        }
    };
    let first = untrack(&wanted);
    let radii = first.map(animated);
    let last = Cell::new(first);
    effect(move || {
        let now = wanted();
        if last.replace(now) != now {
            untrack(|| {
                for (corner, to) in radii.iter().zip(now) {
                    corner.spring_to(to, fast_spatial());
                }
            });
        }
    });
    let looks_off = is_off.clone();
    let disabled_off = is_off;
    ink.paint(ink.listen(row.focusable().dyn_style(move |s| {
        let [a, b, c, d] = radii.map(|r| r.get());
        let mut s = s.bg(background.get());
        s.radius_top_left = Some(a);
        s.radius_top_right = Some(b);
        s.radius_bottom_right = Some(c);
        s.radius_bottom_left = Some(d);
        let s = s.cursor(if looks_off() {
            CursorIcon::Default
        } else {
            CursorIcon::Pointer
        });
        focus_ring(s, focused.get(), &color_scheme())
    })))
    .disabled(move || disabled_off())
}

/// List items with little space between them, whose group is rounded on the outside. Each item is
/// made by `item` from a style that has to be used for it.
pub fn segmented_list<I: IntoView>(count: usize, item: impl Fn(usize, ListItemStyle) -> I) -> Div {
    let last = count.saturating_sub(1);
    div().flex().flex_col().gap(2.0).children(
        (0..count)
            .map(|i| {
                let top = if i == 0 { ACTIVE } else { BASE };
                let bottom = if i == last { ACTIVE } else { BASE };
                let style = ListItemStyle {
                    corners: Some([top, top, bottom, bottom]),
                    interactive: true,
                    ..ListItemStyle::default()
                };
                item(i, style)
            })
            .collect::<Vec<_>>(),
    )
}
