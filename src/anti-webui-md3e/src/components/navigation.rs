//! Navigation bars, rails and drawer items. Sizes, colors and timing are those of Jetpack Compose,
//! and selected icons fill in on top of that. See `docs/design/20-md3e-components.md`.

use super::badge::{badged, text_badge};
use super::surface::{Ink, When, filling_icon, focus_ring, progress_of};
use crate::color::{ColorScheme, color_scheme};
use crate::icons::Symbol;
use crate::motion::{default_effects, default_spatial, fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;

/// A destination of a bar or a rail.
#[derive(Clone)]
pub struct NavItem {
    icon: &'static Symbol,
    label: Option<String>,
    badge: Option<Rc<dyn Fn() -> String>>,
    disabled: Option<When>,
}

/// A destination with an icon and a label.
pub fn nav_item(icon: &'static Symbol, label: impl Into<String>) -> NavItem {
    NavItem {
        icon,
        label: Some(label.into()),
        badge: None,
        disabled: None,
    }
}

/// A destination with only an icon.
pub fn nav_icon(icon: &'static Symbol) -> NavItem {
    NavItem {
        icon,
        label: None,
        badge: None,
        disabled: None,
    }
}

impl NavItem {
    /// A badge on its icon, there while the text isn't empty.
    pub fn badge(mut self, f: impl Fn() -> String + 'static) -> Self {
        self.badge = Some(Rc::new(f));
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// What selecting moves: the icon and label colors follow, and the indicator grows.
struct ItemState {
    selected: When,
    disabled: When,
    ink: Ink,
    icon_color: Animated<Color>,
    label_color: Animated<Color>,
    grown: Animated<f32>,
    fade: Animated<f32>,
}

fn item_state(
    selected: When,
    disabled: Option<When>,
    label_selected: fn(&ColorScheme) -> Color,
    grow: Spring,
) -> ItemState {
    let disabled = super::surface::when(disabled);
    let (on, off) = (selected.clone(), disabled.clone());
    let icon_color = follow(
        move || {
            let c = color_scheme();
            if off() {
                c.on_surface_variant.with_alpha(0.38)
            } else if on() {
                c.on_secondary_container
            } else {
                c.on_surface_variant
            }
        },
        default_effects(),
    );
    let (on, off) = (selected.clone(), disabled.clone());
    let label_color = follow(
        move || {
            let c = color_scheme();
            if off() {
                c.on_surface_variant.with_alpha(0.38)
            } else if on() {
                label_selected(&c)
            } else {
                c.on_surface_variant
            }
        },
        default_effects(),
    );
    ItemState {
        grown: progress_of(selected.clone(), grow),
        fade: progress_of(selected.clone(), default_effects()),
        ink: Ink::new_when(|c| c.on_surface, disabled.clone()),
        selected,
        disabled,
        icon_color,
        label_color,
    }
}

/// The icon of an item, with what makes it what it is.
fn item_icon(item: &NavItem, size: f32, selected: When, color: Animated<Color>) -> Div {
    let icon =
        filling_icon(item.icon, size, Some(selected)).dyn_style(move |s| s.text_color(color.get()));
    match item.badge.clone() {
        Some(badge) => badged(icon, text_badge(move || badge()), true),
        None => icon,
    }
}

/// The pill behind an icon: a state layer as big as it gets, and an indicator growing from the
/// middle or from the left.
fn indicator(state: &ItemState, w: f32, h: f32, from_middle: bool) -> Div {
    let grown = state.grown;
    let fade = state.fade;
    state
        .ink
        .paint(
            div()
                .absolute()
                .w(w)
                .h(h)
                .rounded(h / 2.0)
                .pointer_events_none(),
        )
        .child(
            div()
                .absolute()
                .top(0.0)
                .h(h)
                .rounded(h / 2.0)
                .dyn_style(move |s| {
                    let width = w * grown.get().max(0.0);
                    let left = if from_middle { (w - width) / 2.0 } else { 0.0 };
                    s.w(width)
                        .left(left)
                        .bg(color_scheme().secondary_container)
                        .opacity(fade.get().clamp(0.0, 1.0))
                }),
        )
}

fn listening(state: &ItemState, node: Div) -> Div {
    let on = state.selected.clone();
    let node = node.access(move |a| a.role(Role::Button).selected(on()));
    let off = state.disabled.clone();
    let disabled = state.disabled.clone();
    let focused = state.ink.focused;
    state
        .ink
        .listen_centered(node.select_none().focusable().dyn_style(move |s| {
            let s = s.cursor(if off() {
                CursorIcon::Default
            } else {
                CursorIcon::Pointer
            });
            focus_ring(s, focused.get(), &color_scheme())
        }))
        .disabled(move || disabled())
}

/// Calls `f` for each item with whether it is the selected one and its index.
fn items<V: IntoView>(
    items: Vec<NavItem>,
    selected: Rc<dyn Fn() -> usize>,
    on_select: Rc<dyn Fn(usize)>,
    f: impl Fn(usize, NavItem, When) -> V,
    wrap: impl Fn(V, Rc<dyn Fn(&PointerEvent)>) -> Div,
) -> Vec<Div> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let selected = selected.clone();
            let on_select = on_select.clone();
            let view = f(i, item, Rc::new(move || selected() == i));
            wrap(view, Rc::new(move |_: &PointerEvent| on_select(i)))
        })
        .collect()
}

/// A bar of 3 to 5 destinations for the bottom of a window, with labels below icons.
pub fn navigation_bar(
    destinations: Vec<NavItem>,
    selected: impl Fn() -> usize + 'static,
    on_select: impl Fn(usize) + 'static,
) -> Div {
    crate::ensure_ready();
    let bar = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_stretch()
        .gap(8.0)
        .min_h(80.0)
        .dyn_style(|s| s.bg(color_scheme().surface_container));
    bar.children(items(
        destinations,
        Rc::new(selected),
        Rc::new(on_select),
        |_, item, on| {
            let state = item_state(on, item.disabled.clone(), |c| c.secondary, fast_spatial());
            top_item(&state, &item, 12.0)
        },
        |view, click| {
            view.flex_grow(1.0)
                .flex_basis(0.0)
                .on_click(move |e| click(e))
        },
    ))
}

/// An item with a pill and the label below, centered in what it gets.
fn top_item(state: &ItemState, item: &NavItem, pad: f32) -> Div {
    let label_color = state.label_color;
    let holder = div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(56.0)
        .h(32.0)
        .child(indicator(state, 56.0, 32.0, true))
        // Above the indicator, which isn't in the flow.
        .child(
            item_icon(item, 24.0, state.selected.clone(), state.icon_color)
                .relative()
                .z_index(1),
        );
    listening(
        state,
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(4.0)
            .py(pad)
            .child(holder)
            .children(item.label.clone().map(|label| {
                TextStyle::LABEL_MEDIUM
                    .apply(text(label))
                    .dyn_style(move |s| s.text_color(label_color.get()))
            })),
    )
}

/// The shorter bar of Expressive. With `start_icons` the labels are next to the icons, in the pill.
pub fn short_navigation_bar(
    destinations: Vec<NavItem>,
    selected: impl Fn() -> usize + 'static,
    on_select: impl Fn(usize) + 'static,
    start_icons: bool,
) -> Div {
    crate::ensure_ready();
    let bar = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_stretch()
        .min_h(64.0)
        .dyn_style(|s| s.bg(color_scheme().surface_container));
    bar.children(items(
        destinations,
        Rc::new(selected),
        Rc::new(on_select),
        move |_, item, on| {
            if start_icons {
                let state = item_state(
                    on,
                    item.disabled.clone(),
                    |c| c.on_secondary_container,
                    default_spatial(),
                );
                start_item(&state, &item)
            } else {
                let state = item_state(
                    on,
                    item.disabled.clone(),
                    |c| c.secondary,
                    default_spatial(),
                );
                top_item(&state, &item, 6.0)
            }
        },
        |view, click| {
            view.flex_grow(1.0)
                .flex_basis(0.0)
                .on_click(move |e| click(e))
        },
    ))
}

/// An item with the label next to the icon, both in a pill that grows from the middle.
fn start_item(state: &ItemState, item: &NavItem) -> Div {
    let label_color = state.label_color;
    let width = signal(0.0f32);
    let grown = state.grown;
    let fade = state.fade;
    listening(
        state,
        div().flex().items_center().justify_center().child(
            state.ink.paint(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(4.0)
                    .h(40.0)
                    .px(16.0)
                    .rounded(20.0)
                    .on_resized(move |w, _| width.set(w))
                    .child(
                        div()
                            .absolute()
                            .top(0.0)
                            .h(40.0)
                            .rounded(20.0)
                            .dyn_style(move |s| {
                                let full = width.get();
                                let w = full * grown.get().max(0.0);
                                s.w(w)
                                    .left((full - w) / 2.0)
                                    .bg(color_scheme().secondary_container)
                                    .opacity(fade.get().clamp(0.0, 1.0))
                            }),
                    )
                    .child(
                        item_icon(item, 24.0, state.selected.clone(), state.icon_color)
                            .relative()
                            .z_index(1),
                    )
                    .children(item.label.clone().map(|label| {
                        TextStyle::LABEL_MEDIUM
                            .apply(div().relative().z_index(1).child(text(label)))
                            .dyn_style(move |s| s.text_color(label_color.get()))
                    })),
            ),
        ),
    )
}

/// A rail of destinations for the side of a window, 80 wide.
pub fn navigation_rail(
    destinations: Vec<NavItem>,
    selected: impl Fn() -> usize + 'static,
    on_select: impl Fn(usize) + 'static,
) -> Div {
    crate::ensure_ready();
    let rail = div()
        .flex()
        .flex_col()
        .flex_none()
        .items_stretch()
        .gap(4.0)
        .py(4.0)
        .w(80.0)
        .dyn_style(|s| s.bg(color_scheme().surface));
    rail.children(items(
        destinations,
        Rc::new(selected),
        Rc::new(on_select),
        |_, item, on| {
            let state = item_state(on, item.disabled.clone(), |c| c.secondary, fast_spatial());
            top_item(&state, &item, 4.0).min_h(56.0)
        },
        |view, click| view.on_click(move |e| click(e)),
    ))
}

const WIDE_COLLAPSED: f32 = 96.0;
const WIDE_EXPANDED: f32 = 220.0;
const WIDE_PAD: f32 = 20.0;

/// A rail that can open: its width and every item change between the two layouts, on one spring.
/// `expanded` follows the program, as does what `header` shows.
pub fn wide_navigation_rail(
    destinations: Vec<NavItem>,
    selected: impl Fn() -> usize + 'static,
    on_select: impl Fn(usize) + 'static,
    expanded: impl Fn() -> bool + 'static,
    header: Option<Div>,
) -> Div {
    crate::ensure_ready();
    let expanded: When = Rc::new(expanded);
    let q = progress_of(expanded, default_spatial());
    let selected: Rc<dyn Fn() -> usize> = Rc::new(selected);
    let on_select: Rc<dyn Fn(usize)> = Rc::new(on_select);
    let rail = div()
        .flex()
        .flex_col()
        .flex_none()
        .items_stretch()
        .pt(44.0)
        .overflow_hidden()
        .dyn_style(move |s| {
            let v = q.get().max(0.0);
            s.w(WIDE_COLLAPSED + (WIDE_EXPANDED - WIDE_COLLAPSED) * v)
                .gap(4.0 * (1.0 - v.min(1.0)))
                .bg(color_scheme().surface)
        })
        .children(header.map(|h| h.mb(40.0)));
    rail.children(
        destinations
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let selected = selected.clone();
                let on_select = on_select.clone();
                wide_item(item, Rc::new(move || selected() == i), q).on_click(move |_| on_select(i))
            })
            .collect::<Vec<_>>(),
    )
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn wide_item(item: NavItem, selected: When, q: Animated<f32>) -> Div {
    let label_on = selected.clone();
    let label_color = follow(
        move || {
            let c = color_scheme();
            match (label_on(), q.get() < 0.5) {
                (false, _) => c.on_surface_variant,
                (true, true) => c.secondary,
                (true, false) => c.on_secondary_container,
            }
        },
        default_effects(),
    );
    let state = item_state(
        selected,
        item.disabled.clone(),
        |c| c.secondary,
        default_spatial(),
    );
    let grown = state.grown;
    let label_width = signal(0.0f32);
    let label_height = signal(0.0f32);
    const ICON: f32 = 24.0;
    let pill_w = move |t: f32| lerp(ICON, ICON + label_width.get() + 8.0, t) + 32.0;
    let pill_h = move |t: f32| ICON + 2.0 * lerp(4.0, 16.0, t);
    let height = move |t: f32| lerp(pill_h(t) + 4.0 + label_height.get(), pill_h(t), t);
    let layer = state.ink.paint(
        div()
            .absolute()
            .left(WIDE_PAD)
            .top(0.0)
            .pointer_events_none()
            .dyn_style(move |s| {
                let t = q.get().max(0.0);
                let h = pill_h(t);
                s.w(pill_w(t)).h(h).rounded(h / 2.0)
            }),
    );
    let pill = div()
        .absolute()
        .left(WIDE_PAD)
        .top(0.0)
        .dyn_style(move |s| {
            let t = q.get().max(0.0);
            let h = pill_h(t);
            s.w(pill_w(t) * grown.get().max(0.0))
                .h(h)
                .rounded(h / 2.0)
                .bg(color_scheme().secondary_container)
                .opacity(grown.get().clamp(0.0, 1.0))
        });
    let icon = item_icon(&item, ICON, state.selected.clone(), state.icon_color)
        .absolute()
        .left(WIDE_PAD + 16.0)
        .dyn_style(move |s| {
            let t = q.get().max(0.0);
            s.top(lerp(4.0, (height(t) - ICON) / 2.0, t.min(1.0)))
        });
    let label = item.label.clone().map(|label| {
        let held = move |s: Style| {
            let t = q.get().max(0.0);
            // It fades out on the way, changes style and place, and fades back in.
            let at_top = t < 0.5;
            let (style, x, y) = if at_top {
                (
                    TextStyle::LABEL_MEDIUM,
                    (WIDE_COLLAPSED - label_width.get()) / 2.0,
                    pill_h(t) + 4.0,
                )
            } else {
                (
                    TextStyle::LABEL_LARGE,
                    WIDE_PAD + 16.0 + ICON + 8.0,
                    (height(t) - label_height.get()) / 2.0,
                )
            };
            style
                .apply(s)
                .left(x)
                .top(y)
                .text_color(label_color.get())
                .opacity((4.0 * (t - 0.5) * (t - 0.5)).clamp(0.0, 1.0))
        };
        div()
            .absolute()
            .whitespace_nowrap()
            .on_resized(move |w, h| {
                label_width.set(w);
                label_height.set(h);
            })
            .dyn_style(held)
            .child(text(label))
    });
    listening(
        &state,
        div()
            .relative()
            .flex_none()
            .dyn_style(move |s| s.h(height(q.get().max(0.0))))
            .child(layer)
            .child(pill)
            .child(icon)
            .children(label),
    )
}

/// A destination in a navigation drawer. Keep some room around it, 12 on the sides.
pub fn drawer_item(
    icon: &'static Symbol,
    label: impl IntoTextSource,
    selected: impl Fn() -> bool + 'static,
) -> Div {
    let selected: When = Rc::new(selected);
    let on = selected.clone();
    let background = follow(
        move || {
            if on() {
                color_scheme().secondary_container
            } else {
                Color::TRANSPARENT
            }
        },
        fast_effects(),
    );
    let on = selected.clone();
    let content = follow(
        move || {
            let c = color_scheme();
            if on() {
                c.on_secondary_container
            } else {
                c.on_surface_variant
            }
        },
        fast_effects(),
    );
    let layer_on = selected.clone();
    let ink = Ink::new(
        move |c| {
            if layer_on() {
                c.on_secondary_container
            } else {
                c.on_surface
            }
        },
        || false,
    );
    let focused = ink.focused;
    ink.paint(
        ink.listen(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(12.0)
                .min_h(56.0)
                .pl(16.0)
                .pr(24.0)
                .rounded(28.0)
                .select_none()
                .focusable()
                .cursor(CursorIcon::Pointer)
                .dyn_style(move |s| {
                    focus_ring(
                        s.bg(background.get()).text_color(content.get()),
                        focused.get(),
                        &color_scheme(),
                    )
                })
                .child(filling_icon(icon, 24.0, Some(selected)))
                .child(TextStyle::LABEL_LARGE.apply(text(label))),
        ),
    )
}
