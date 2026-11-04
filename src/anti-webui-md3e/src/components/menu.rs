//! Menus, in the groups and with the shapes that change of Expressive. Sizes, colors and timing are
//! those of Jetpack Compose. See `docs/design/20-md3e-components.md`.

use super::icon::sized_icon;
use super::surface::{
    Corners, Ink, When, elevation_shadows, focus_ring, following, progress_of, round, when,
};
use crate::color::color_scheme;
use crate::icons::{self, Symbol};
use crate::motion::{fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// What a menu item says and has around that.
#[derive(Clone, Default)]
pub struct MenuItem {
    label: String,
    supporting: Option<String>,
    icon: Option<&'static Symbol>,
    trailing: Option<String>,
    trailing_icon: Option<&'static Symbol>,
    selected: Option<When>,
    disabled: Option<When>,
    on_click: Option<Rc<dyn Fn()>>,
}

/// An item of a menu.
pub fn menu_item(label: impl Into<String>) -> MenuItem {
    MenuItem {
        label: label.into(),
        ..MenuItem::default()
    }
}

impl MenuItem {
    /// A line below the label.
    pub fn supporting(mut self, text: impl Into<String>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    /// An icon before the label.
    pub fn icon(mut self, symbol: &'static Symbol) -> Self {
        self.icon = Some(symbol);
        self
    }

    /// A short text after the label, like a keyboard shortcut.
    pub fn trailing(mut self, text: impl Into<String>) -> Self {
        self.trailing = Some(text.into());
        self
    }

    /// An icon after the label.
    pub fn trailing_icon(mut self, symbol: &'static Symbol) -> Self {
        self.trailing_icon = Some(symbol);
        self
    }

    /// In the selected colors and shape while `f` returns true. Without an icon, a check grows in
    /// front of the label.
    pub fn selected_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.selected = Some(Rc::new(f));
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }

    /// What it does.
    pub fn on_click(mut self, f: impl Fn() + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// A menu that hangs from what it is a child of, or from `anchor` of [`MenuStyle`]. It closes when
/// `open` says so, which `on_dismiss` is the place to change: that runs for a press outside of it
/// and for Escape.
pub fn menu(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    groups: Vec<Vec<MenuItem>>,
) -> Show {
    styled_menu(open, on_dismiss, groups, MenuStyle::default())
}

/// How a menu looks and where it goes.
#[derive(Clone, Default)]
pub struct MenuStyle {
    vibrant: bool,
    placement: Placement,
    anchor: Option<NodeRef>,
    match_width: bool,
}

impl MenuStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// In a stronger color.
    pub fn vibrant(mut self) -> Self {
        self.vibrant = true;
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    pub fn anchor(mut self, anchor: NodeRef) -> Self {
        self.anchor = Some(anchor);
        self
    }

    /// At least as wide as what it hangs from.
    pub fn match_anchor_width(mut self) -> Self {
        self.match_width = true;
        self
    }
}

pub fn styled_menu(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    groups: Vec<Vec<MenuItem>>,
    style: MenuStyle,
) -> Show {
    crate::ensure_ready();
    let open: When = Rc::new(open);
    let on_dismiss: Rc<dyn Fn()> = Rc::new(on_dismiss);
    let fade_open = open.clone();
    let fade = following(
        move || if fade_open() { 1.0f32 } else { 0.0 },
        fast_effects(),
    );
    let size_open = open.clone();
    let size = following(
        move || if size_open() { 1.0f32 } else { 0.8 },
        fast_spatial(),
    );
    // What the platform closed can't come back, so it goes at once and gets built again.
    let gone = signal(false);
    let reset = open.clone();
    effect(move || {
        if reset() {
            gone.set(false);
        }
    });
    let wanted = open.clone();
    let origin = match style.placement {
        Placement::BelowStart | Placement::RightStart => (0.0, 0.0),
        Placement::BelowEnd | Placement::LeftStart => (1.0, 0.0),
        Placement::AboveStart => (0.0, 1.0),
        Placement::AboveEnd => (1.0, 1.0),
        Placement::BelowCenter => (0.5, 0.0),
        Placement::AboveCenter => (0.5, 1.0),
    };
    show(
        move || wanted() || (fade.get() > 0.0 && !gone.get()),
        move || {
            let dismiss = on_dismiss.clone();
            let groups = groups.clone();
            let mut surface = popup()
                .placement(style.placement)
                .gap(4)
                .on_dismiss(move || {
                    gone.set(true);
                    dismiss();
                });
            if let Some(anchor) = style.anchor {
                surface = surface.anchor(anchor);
            }
            if style.match_width {
                surface = surface.match_anchor_width();
            }
            let count = groups.len();
            let vibrant = style.vibrant;
            let refs: Rc<Vec<NodeRef>> =
                Rc::new(groups.iter().flatten().map(|_| node_ref()).collect());
            let next = Rc::new(Cell::new(0usize));
            surface.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(2.0)
                    .transform_origin(origin.0, origin.1)
                    .dyn_style(move |s| s.opacity(fade.get().clamp(0.0, 1.0)).scale(size.get()))
                    .children(
                        groups
                            .into_iter()
                            .enumerate()
                            .map(|(i, items)| {
                                group(i, count, items, vibrant, refs.clone(), next.clone())
                            })
                            .collect::<Vec<_>>(),
                    ),
            )
        },
    )
}

/// The corners of a group by its place among the groups, or of an item by its place in the group.
fn place_corners(i: usize, count: usize, outer: f32, inner: f32) -> Corners {
    if count <= 1 {
        round(outer)
    } else if i == 0 {
        [outer, outer, inner, inner]
    } else if i + 1 == count {
        [inner, inner, outer, outer]
    } else {
        round(inner)
    }
}

fn group(
    i: usize,
    count: usize,
    items: Vec<MenuItem>,
    vibrant: bool,
    refs: Rc<Vec<NodeRef>>,
    next: Rc<Cell<usize>>,
) -> Div {
    let [a, b, c, d] = if count <= 1 {
        round(16.0)
    } else {
        place_corners(i, count, 16.0, 8.0)
    };
    let total = items.len();
    let mut node = div()
        .flex()
        .flex_col()
        .gap(2.0)
        .py(2.0)
        .min_w(112.0)
        .shadows(elevation_shadows(2.0))
        .dyn_style(move |s| {
            let k = color_scheme();
            s.bg(if vibrant {
                k.tertiary_container
            } else {
                k.surface_container_low
            })
        })
        .children(
            items
                .into_iter()
                .enumerate()
                .map(|(j, item)| {
                    let at = next.get();
                    next.set(at + 1);
                    item_view(
                        item,
                        place_corners(j, total, 12.0, 4.0),
                        vibrant,
                        refs.clone(),
                        at,
                    )
                })
                .collect::<Vec<_>>(),
        );
    let style = node.style_mut();
    style.radius_top_left = Some(a);
    style.radius_top_right = Some(b);
    style.radius_bottom_right = Some(c);
    style.radius_bottom_left = Some(d);
    node
}

fn item_view(
    item: MenuItem,
    corners: Corners,
    vibrant: bool,
    refs: Rc<Vec<NodeRef>>,
    at: usize,
) -> Div {
    let MenuItem {
        label,
        supporting,
        icon,
        trailing,
        trailing_icon,
        selected,
        disabled,
        on_click,
    } = item;
    let selects = selected.is_some();
    let is_on = when(selected);
    let is_off = when(disabled);
    let (on, off) = (is_on.clone(), is_off.clone());
    let background = following(
        move || {
            let c = color_scheme();
            match (vibrant, on() && !off()) {
                (false, false) => c.surface_container_low,
                (false, true) => c.tertiary_container,
                (true, false) => c.tertiary_container,
                (true, true) => c.tertiary,
            }
        },
        fast_effects(),
    );
    let (on, off) = (is_on.clone(), is_off.clone());
    let content = move || {
        let c = color_scheme();
        if off() {
            let base = if vibrant {
                c.on_tertiary_container
            } else {
                c.on_surface
            };
            (base.with_alpha(0.38), base.with_alpha(0.38))
        } else {
            match (vibrant, on()) {
                (false, false) => (c.on_surface, c.on_surface_variant),
                (false, true) => (c.on_tertiary_container, c.on_tertiary_container),
                (true, false) => (c.on_tertiary_container, c.on_tertiary_container),
                (true, true) => (c.on_tertiary, c.on_tertiary),
            }
        }
    };
    let main = content.clone();
    let side = content.clone();
    let layer = content;
    let shape_on = is_on.clone();
    let wanted = move || if shape_on() { round(12.0) } else { corners };
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
    let ink = Ink::new(move |_| layer().0, is_off.clone());
    let focused = ink.focused;

    let lead = match icon {
        Some(symbol) => Some(
            sized_icon(symbol, 20.0)
                .mr(8.0)
                .dyn_style(move |s| s.text_color(side().1)),
        ),
        // The check grows from nothing.
        None if selects => {
            let t = progress_of(is_on.clone(), fast_spatial());
            let fade = progress_of(is_on.clone(), fast_effects());
            let color = side;
            Some(
                div()
                    .flex_none()
                    .h(20.0)
                    .overflow_hidden()
                    .dyn_style(move |s| {
                        let v = t.get().max(0.0);
                        s.w(20.0 * v)
                            .mr(8.0 * v)
                            .opacity(fade.get().clamp(0.0, 1.0))
                            .text_color(color().1)
                    })
                    .child(sized_icon(icons::CHECK, 20.0)),
            )
        }
        None => None,
    };
    let keys = refs.clone();
    let root_off = is_off.clone();
    let disabled_off = is_off;
    let run = on_click;
    let node = ink.paint(
        ink.listen(
            div()
                .flex()
                .flex_row()
                .items_center()
                .mx(4.0)
                .min_h(44.0)
                .max_w(280.0)
                .px(12.0)
                .py(if supporting.is_some() { 6.0 } else { 0.0 })
                .select_none()
                .focusable()
                .node_ref(refs[at])
                .dyn_style(move |s| {
                    let [a, b, c, d] = radii.map(|r| r.get());
                    let mut s = s
                        .bg(background.get())
                        .text_color(main().0)
                        .cursor(if root_off() {
                            CursorIcon::Default
                        } else {
                            CursorIcon::Pointer
                        });
                    s.radius_top_left = Some(a);
                    s.radius_top_right = Some(b);
                    s.radius_bottom_right = Some(c);
                    s.radius_bottom_left = Some(d);
                    focus_ring(s, focused.get(), &color_scheme())
                })
                .on_click(move |_| {
                    if let Some(run) = &run {
                        run();
                    }
                })
                .on_key_down(move |e| {
                    let to = match e.key {
                        Key::Named(NamedKey::ArrowDown) => at + 1,
                        Key::Named(NamedKey::ArrowUp) => at + keys.len() - 1,
                        _ => return,
                    };
                    e.prevent_default();
                    keys[to % keys.len()].focus();
                })
                .children(lead)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_grow(1.0)
                        .child(TextStyle::LABEL_LARGE.apply(div().child(text(label))))
                        .children(
                            supporting
                                .map(|line| TextStyle::BODY_MEDIUM.apply(div().child(text(line)))),
                        ),
                )
                .children(
                    trailing
                        .map(|line| TextStyle::LABEL_LARGE.apply(div().ml(12.0).child(text(line)))),
                )
                .children(trailing_icon.map(|symbol| sized_icon(symbol, 20.0).ml(8.0))),
        ),
    );
    node.disabled(move || disabled_off())
}
