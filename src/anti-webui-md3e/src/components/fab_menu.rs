//! The FAB that opens into a menu. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::icon::sized_icon;
use super::surface::{Ink, When, elevation_shadows, focus_ring, following, progress_of, when};
use crate::color::color_scheme;
use crate::icons::{self, Symbol};
use crate::motion::{fast_effects, fast_spatial, slow_effects};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;

const ITEM_HEIGHT: f32 = 56.0;
const BUTTON: f32 = 56.0;

/// An item of a FAB menu.
#[derive(Clone)]
pub struct FabMenuItem {
    icon: &'static Symbol,
    label: String,
    on_click: Rc<dyn Fn()>,
}

pub fn fab_menu_item(
    icon: &'static Symbol,
    label: impl Into<String>,
    on_click: impl Fn() + 'static,
) -> FabMenuItem {
    FabMenuItem {
        icon,
        label: label.into(),
        on_click: Rc::new(on_click),
    }
}

/// A FAB that stays where it is and opens upward into items, the closest one first. It turns into a
/// smaller circle that shows a cross while `open` returns true, which the program changes in
/// `on_toggle`. The items don't take room: they go over what is above it.
pub fn fab_menu(
    symbol: &'static Symbol,
    open: impl Fn() -> bool + 'static,
    on_toggle: impl Fn() + 'static,
    items: Vec<FabMenuItem>,
) -> Div {
    crate::ensure_ready();
    let open: When = Rc::new(open);
    let count = items.len();
    // Steps up to the number of items and back: the items follow one by one.
    let stagger_open = open.clone();
    let stagger = following(
        move || if stagger_open() { count as f32 } else { 0.0 },
        slow_effects(),
    );
    let rows: Vec<Div> = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let shown: When = Rc::new(move || (i as f32) >= count as f32 - stagger.get().round());
            menu_row(item, shown)
        })
        .collect();
    div()
        .relative()
        .flex_none()
        .w(BUTTON)
        .h(BUTTON)
        .child(
            div()
                .absolute()
                .right(0.0)
                .bottom(BUTTON + 8.0)
                .z_index(1)
                .flex()
                .flex_col()
                .items_end()
                .gap(4.0)
                .pointer_events_none()
                .children(rows),
        )
        .child(toggle(symbol, open).on_click(move |_| on_toggle()))
}

fn menu_row(item: FabMenuItem, shown: When) -> Div {
    let FabMenuItem {
        icon,
        label,
        on_click,
    } = item;
    let width = progress_of(shown.clone(), fast_spatial());
    let fade = progress_of(shown.clone(), fast_effects());
    let natural = signal(0.0f32);
    let ink = Ink::new(|c| c.on_primary_container, when(None));
    let focused = ink.focused;
    let gone = shown.clone();
    ink.paint(
        ink.listen(
            div()
                .flex()
                .flex_none()
                .justify_center()
                .h(ITEM_HEIGHT)
                .rounded(ITEM_HEIGHT / 2.0)
                .overflow_hidden()
                .select_none()
                .focusable()
                .cursor(CursorIcon::Pointer)
                .dyn_style(move |s| {
                    let c = color_scheme();
                    let s = s
                        .bg(c.primary_container)
                        .text_color(c.on_primary_container)
                        .w(natural.get() * width.get().max(0.0))
                        .opacity(fade.get().clamp(0.0, 1.0));
                    let s = if gone() { s.pointer_events_auto() } else { s };
                    focus_ring(s, focused.get(), &c)
                })
                .on_click(move |_| on_click())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_none()
                        .items_center()
                        .gap(8.0)
                        .px(24.0)
                        .on_resized(move |w, _| natural.set(w))
                        .child(sized_icon(icon, 24.0))
                        .child(
                            TextStyle::TITLE_MEDIUM
                                .apply(text(label))
                                .whitespace_nowrap(),
                        ),
                ),
        ),
    )
}

/// The button of the menu, which turns into a smaller circle of another color while checked.
fn toggle(symbol: &'static Symbol, checked: When) -> Div {
    let t = progress_of(checked, fast_spatial());
    let lerp = move |a: f32, b: f32| a + (b - a) * t.get();
    let ink = Ink::new(
        move |c| {
            if t.get() > 0.5 {
                c.on_primary
            } else {
                c.on_primary_container
            }
        },
        when(None),
    );
    let focused = ink.focused;
    let plain = sized_icon(symbol, 24.0).absolute();
    let close = sized_icon(icons::CLOSE, 24.0).absolute();
    ink.paint(
        ink.listen(
            div()
                .relative()
                .flex_none()
                .select_none()
                .focusable()
                .cursor(CursorIcon::Pointer)
                .shadows(elevation_shadows(3.0))
                .w(BUTTON)
                .h(BUTTON)
                .dyn_style(move |s| {
                    let c = color_scheme();
                    let v = t.get().clamp(0.0, 1.0);
                    let s = s
                        .rounded(lerp(16.0, 28.0))
                        .bg(mix_premultiplied(c.primary_container, c.primary, v))
                        .text_color(mix_premultiplied(c.on_primary_container, c.on_primary, v));
                    focus_ring(s, focused.get(), &c)
                })
                .child(plain.left(16.0).top(16.0).dyn_style(move |s| {
                    let v = t.get();
                    s.rotate(90.0 * v).opacity(if v < 0.5 { 1.0 } else { 0.0 })
                }))
                .child(close.left(16.0).top(16.0).dyn_style(move |s| {
                    let v = t.get();
                    s.rotate(90.0 * v - 90.0)
                        .opacity(if v < 0.5 { 0.0 } else { 1.0 })
                })),
        ),
    )
}
