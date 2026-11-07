//! Floating toolbars. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::fab::{FabColor, FabSize, FabStyle, styled_fab};
use super::surface::{When, elevation_shadows, progress_of};
use crate::color::color_scheme;
use crate::icons::Symbol;
use crate::motion::fast_spatial;
use anti_webui::prelude::*;
use std::rc::Rc;

const THICK: f32 = 64.0;
const PADDING: f32 = 8.0;
const FAB_GAP: f32 = 8.0;

/// How a floating toolbar looks.
#[derive(Clone, Default)]
pub struct ToolbarStyle {
    vibrant: bool,
    vertical: bool,
    expanded: Option<When>,
}

impl ToolbarStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// In a stronger color.
    pub fn vibrant(mut self) -> Self {
        self.vibrant = true;
        self
    }

    /// Goes down rather than right.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// Opens and closes with this. Open otherwise.
    pub fn expanded_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.expanded = Some(Rc::new(f));
        self
    }
}

fn colors(vibrant: bool) -> (Color, Color) {
    let c = color_scheme();
    if vibrant {
        (c.primary_container, c.on_primary_container)
    } else {
        (c.surface_container, c.on_surface)
    }
}

/// A pill with what it holds, whose length follows `e` from 0 to all of it.
fn pill(items: Vec<Div>, vibrant: bool, vertical: bool, e: Animated<f32>, level: f32) -> Div {
    let natural = signal(0.0f32);
    let inner = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(4.0)
        .p(PADDING)
        .on_resized(move |w, h| natural.set(if vertical { h } else { w }))
        .children(items);
    let inner = if vertical {
        inner.flex_col()
    } else {
        inner.flex_row()
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .overflow_hidden()
        .rounded(THICK / 2.0)
        .dyn_style(move |s| {
            let (background, content) = colors(vibrant);
            let length = natural.get() * e.get().max(0.0);
            let s = s
                .bg(background)
                .text_color(content)
                .shadows(elevation_shadows(level * e.get().clamp(0.0, 1.0)));
            let s = if vertical {
                s.flex_col().w(THICK).h(length)
            } else {
                s.flex_row().h(THICK).w(length)
            };
            // Until it knows how long it is, it takes what it needs.
            if natural.get() <= 0.0 {
                s.opacity(0.0)
            } else {
                s
            }
        })
        .child(inner)
}

/// A pill of actions that floats over the content, and can close to nothing.
pub fn floating_toolbar(items: Vec<Div>, style: ToolbarStyle) -> Div {
    crate::ensure_ready();
    let ToolbarStyle {
        vibrant,
        vertical,
        expanded,
    } = style;
    let open: When = match expanded {
        Some(f) => f,
        None => Rc::new(|| true),
    };
    let e = progress_of(open, fast_spatial());
    pill(items, vibrant, vertical, e, 0.0)
}

/// A floating toolbar next to a FAB, which grows as the toolbar closes into it.
pub fn floating_toolbar_with_fab(
    items: Vec<Div>,
    fab: &'static Symbol,
    on_fab: impl Fn() + 'static,
    style: ToolbarStyle,
) -> Div {
    crate::ensure_ready();
    let ToolbarStyle {
        vibrant,
        vertical,
        expanded,
    } = style;
    let open: When = match expanded {
        Some(f) => f,
        None => Rc::new(|| true),
    };
    let e = follow(
        {
            let open = open.clone();
            move || if open() { 1.0f32 } else { 0.0 }
        },
        fast_spatial(),
    );
    let color = if vibrant {
        FabColor::TertiaryContainer
    } else {
        FabColor::PrimaryContainer
    };
    let button = styled_fab(fab, FabStyle::new().size(FabSize::Baseline).color(color))
        .on_click(move |_| on_fab())
        .dyn_style(move |s| {
            let size = 56.0 + 24.0 * (1.0 - e.get()).max(-0.1);
            s.w(size).h(size)
        });
    let holder = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(80.0)
        .h(80.0)
        .child(button);
    let root = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(FAB_GAP - 12.0)
        .child(pill(items, vibrant, vertical, e, 1.0))
        .child(holder);
    if vertical {
        root.flex_col()
    } else {
        root.flex_row()
    }
}
