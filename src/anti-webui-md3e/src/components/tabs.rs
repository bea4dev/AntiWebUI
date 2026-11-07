//! Tabs. Sizes, colors and timing are those of Jetpack Compose, and selected icons fill in on top
//! of that. See `docs/design/20-md3e-components.md`.

use super::surface::{Ink, When, filling_icon, focus_ring};
use crate::color::color_scheme;
use crate::icons::Symbol;
use crate::motion::{default_effects, default_spatial, fast_effects};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const SCROLLABLE_MIN: f32 = 90.0;
const SCROLLABLE_EDGE: f32 = 52.0;

/// One of the tabs.
#[derive(Clone)]
pub struct Tab {
    label: Option<String>,
    icon: Option<&'static Symbol>,
}

pub fn tab(label: impl Into<String>) -> Tab {
    Tab {
        label: Some(label.into()),
        icon: None,
    }
}

pub fn icon_tab(icon: &'static Symbol, label: impl Into<String>) -> Tab {
    Tab {
        label: Some(label.into()),
        icon: Some(icon),
    }
}

/// How a row of tabs looks and behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabsStyle {
    secondary: bool,
    scrollable: bool,
}

impl TabsStyle {
    pub fn primary() -> Self {
        Self::default()
    }

    /// For what is below the main tabs: the indicator is as wide as the tab.
    pub fn secondary() -> Self {
        Self {
            secondary: true,
            ..Self::default()
        }
    }

    /// As wide as they need to be, and the row scrolls to the selected one.
    pub fn scrollable(mut self) -> Self {
        self.scrollable = true;
        self
    }
}

/// A row of tabs that all get the same width, bound to the index of the chosen one. The behavior
/// is the standard tabs' (`tabs_with`): the row is one stop for Tab and the arrow keys go through
/// the tabs.
pub fn tabs(labels: Vec<Tab>, selected: impl Into<Binding<usize>>) -> Div {
    styled_tabs(labels, selected, TabsStyle::primary())
}

pub fn styled_tabs(labels: Vec<Tab>, selected: impl Into<Binding<usize>>, style: TabsStyle) -> Div {
    let count = labels.len().max(1);
    tabs_with(selected, count, move |state| look(labels, state, style))
}

fn look(labels: Vec<Tab>, state: TabsState, style: TabsStyle) -> Div {
    crate::ensure_ready();
    let TabsStyle {
        secondary,
        scrollable,
    } = style;
    let count = labels.len().max(1);
    let tall = labels.iter().any(|t| t.icon.is_some() && t.label.is_some());
    let height = if tall { 72.0 } else { 48.0 };
    let selected: Rc<dyn Fn() -> usize> = Rc::new(move || state.selected());
    let row_width = signal(0.0f32);
    let tab_widths: Vec<Signal<f32>> = (0..count).map(|_| signal(0.0)).collect();
    let content_widths: Vec<Signal<f32>> = (0..count).map(|_| signal(0.0)).collect();
    let scroller = node_ref();

    let (widths, contents) = (tab_widths.clone(), content_widths.clone());
    let chosen = selected.clone();
    // Where the indicator should be, from what was measured.
    let target = move || {
        let i = chosen().min(count - 1);
        let left: f32 = if scrollable {
            SCROLLABLE_EDGE + widths[..i].iter().map(|w| w.get()).sum::<f32>()
        } else {
            row_width.get() / count as f32 * i as f32
        };
        let tab_w = if scrollable {
            widths[i].get()
        } else {
            row_width.get() / count as f32
        };
        let w = if secondary {
            tab_w
        } else {
            contents[i].get().min(tab_w - 32.0).max(24.0)
        };
        (left + (tab_w - w) / 2.0, w, left, tab_w)
    };
    let place = target.clone();
    let mover = target.clone();
    // The first measure has nothing to come from.
    let measured = signal(false);
    let indicator_x = animated(0.0f32);
    let indicator_w = animated(0.0f32);
    effect(move || {
        let (x, w, _, tab_w) = place();
        if tab_w <= 0.0 {
            return;
        }
        untrack(|| {
            if measured.get() {
                indicator_x.spring_to(x, default_spatial());
                indicator_w.spring_to(w, default_spatial());
            } else {
                indicator_x.set(x);
                indicator_w.set(w);
                measured.set(true);
            }
        });
    });
    if scrollable {
        let scroll_x = animated(0.0f32);
        effect(move || {
            let (_, _, left, tab_w) = mover();
            let wanted = left - (row_width.get() - tab_w) / 2.0;
            untrack(|| {
                scroll_x.spring_to(wanted.max(0.0), default_spatial());
            });
        });
        effect(move || {
            let x = scroll_x.get();
            scroller.scroll_to(x, 0.0);
        });
    }

    let children: Vec<Div> = labels
        .into_iter()
        .enumerate()
        .map(|(i, tab)| {
            let on: When = Rc::new(move || state.is_selected(i));
            let keyed = move || state.is_selected(i) && state.focus_visible();
            let (tab_width, content_width) = (tab_widths[i], content_widths[i]);
            let node = state
                .tab(i, one_tab(tab, on, keyed, secondary, height, content_width))
                .on_resized(move |w, _| tab_width.set(w));
            if scrollable {
                node.flex_none().min_w(SCROLLABLE_MIN)
            } else {
                node.flex_grow(1.0).flex_basis(0.0)
            }
        })
        .collect();

    let line = div()
        .absolute()
        .left(0.0)
        .right(0.0)
        .bottom(0.0)
        .h(1.0)
        .dyn_style(|s| s.bg(color_scheme().outline_variant));
    let mark = div().absolute().bottom(0.0).h(3.0).dyn_style(move |s| {
        let s = s
            .left(indicator_x.get())
            .w(indicator_w.get().max(0.0))
            .bg(color_scheme().primary);
        if secondary { s } else { s.rounded_t(3.0) }
    });
    let row = div()
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_stretch()
        .h(height)
        .select_none()
        .dyn_style(|s| s.bg(color_scheme().surface));
    if scrollable {
        div()
            .relative()
            .flex_none()
            .h(height)
            .flex()
            .flex_row()
            .overflow_scroll()
            .node_ref(scroller)
            .on_resized(move |w, _| row_width.set(w))
            .child(
                row.px(SCROLLABLE_EDGE)
                    .self_start()
                    .children(children)
                    .child(line)
                    .child(mark),
            )
    } else {
        row.on_resized(move |w, _| row_width.set(w))
            .children(children)
            .child(line)
            .child(mark)
    }
}

fn one_tab(
    tab: Tab,
    on: When,
    keyed: impl Fn() -> bool + 'static,
    secondary: bool,
    height: f32,
    content_width: Signal<f32>,
) -> Div {
    let colored = on.clone();
    let wanted = move || {
        let c = color_scheme();
        match (colored(), secondary) {
            (true, false) => c.primary,
            (true, true) => c.on_surface,
            (false, _) => c.on_surface_variant,
        }
    };
    let in_color = animated(untrack(&wanted));
    let last = Cell::new(untrack(&wanted));
    let spring_on = on.clone();
    effect(move || {
        let now = wanted();
        if last.replace(now) != now {
            // Faster on the way out, as in `Tab.kt`.
            let spring = if untrack(|| spring_on()) {
                default_effects()
            } else {
                fast_effects()
            };
            untrack(|| in_color.spring_to(now, spring));
        }
    });
    let ink_on = on.clone();
    let ink = Ink::new(
        move |c| {
            if ink_on() {
                if secondary { c.on_surface } else { c.primary }
            } else {
                c.on_surface
            }
        },
        Rc::new(|| false),
    );
    // The row has the focus; the chosen tab shows it.
    let focused = ink.focused;
    effect(move || focused.set(keyed()));
    let inner = div()
        .flex()
        .flex_col()
        .flex_none()
        .items_center()
        .gap(if tab.icon.is_some() { 2.0 } else { 0.0 })
        .on_resized(move |w, _| content_width.set(w))
        .children(tab.icon.map(|symbol| filling_icon(symbol, 24.0, Some(on))))
        .children(
            tab.label
                .map(|label| TextStyle::TITLE_SMALL.apply(text(label))),
        );
    ink.paint(
        ink.listen(
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(height)
                .px(16.0)
                .cursor(CursorIcon::Pointer)
                .dyn_style(move |s| {
                    focus_ring(s.text_color(in_color.get()), focused.get(), &color_scheme())
                })
                .child(inner),
        ),
    )
}
