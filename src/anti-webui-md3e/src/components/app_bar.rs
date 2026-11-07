//! App bars, with what they do while the content scrolls. Sizes, colors and timing are those of
//! Jetpack Compose. See `docs/design/20-md3e-components.md`.

use crate::color::color_scheme;
use crate::motion::{default_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const COLLAPSED: f32 = 64.0;

/// The kinds of top app bars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppBarKind {
    #[default]
    Small,
    Center,
    Medium,
    Large,
    MediumFlexible,
    LargeFlexible,
}

/// What a top app bar does while what is below it scrolls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppBarScroll {
    /// It stays, and only its color changes.
    #[default]
    Pinned,
    /// It goes up with the content and comes back as soon as that goes down.
    EnterAlways,
    /// It collapses as far as it goes, and comes back when the content is at its top.
    ExitUntilCollapsed,
}

/// What a top app bar has.
#[derive(Default)]
pub struct TopAppBar {
    kind: AppBarKind,
    title: String,
    subtitle: Option<String>,
    navigation: Option<View>,
    actions: Vec<View>,
    centered: bool,
}

impl TopAppBar {
    pub fn new(kind: AppBarKind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            centered: kind == AppBarKind::Center,
            ..Self::default()
        }
    }

    /// A smaller line below the title. Only the flexible kinds and the small one have this.
    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    /// Usually an icon button to go back or to open a drawer.
    pub fn navigation(mut self, view: impl IntoView) -> Self {
        self.navigation = Some(view.into_view());
        self
    }

    /// Usually an icon button.
    pub fn action(mut self, view: impl IntoView) -> Self {
        self.actions.push(view.into_view());
        self
    }

    /// With the large title in the middle.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }
}

/// The height when open, and the styles of the large title and its subtitle.
fn layout(kind: AppBarKind, has_subtitle: bool) -> (f32, TextStyle, TextStyle, f32) {
    match kind {
        AppBarKind::Small | AppBarKind::Center => (
            COLLAPSED,
            TextStyle::TITLE_LARGE,
            TextStyle::LABEL_MEDIUM,
            0.0,
        ),
        AppBarKind::Medium => (
            112.0,
            TextStyle::HEADLINE_SMALL,
            TextStyle::LABEL_LARGE,
            24.0,
        ),
        AppBarKind::Large => (
            152.0,
            TextStyle::HEADLINE_MEDIUM,
            TextStyle::LABEL_LARGE,
            28.0,
        ),
        AppBarKind::MediumFlexible => (
            if has_subtitle { 136.0 } else { 112.0 },
            TextStyle::HEADLINE_MEDIUM,
            TextStyle::LABEL_LARGE,
            24.0,
        ),
        AppBarKind::LargeFlexible => (
            if has_subtitle { 152.0 } else { 120.0 },
            TextStyle::DISPLAY_SMALL,
            TextStyle::TITLE_MEDIUM,
            28.0,
        ),
    }
}

/// A top app bar that doesn't react to anything. To have it react to scrolling, use
/// [`top_app_bar_over`].
pub fn top_app_bar(bar: TopAppBar) -> Div {
    bar_view(bar, signal(0.0), signal(false))
}

/// Puts a top app bar over a column of content that scrolls below it. It is what comes back, and
/// should get its height from what it is in.
pub fn top_app_bar_over(bar: TopAppBar, scroll: AppBarScroll, content: Div) -> Div {
    crate::ensure_ready();
    let (open, _, _, _) = layout(bar.kind, bar.subtitle.is_some());
    let limit = open - COLLAPSED;
    // How far it went up: that of the whole bar for `EnterAlways`, otherwise of its lower row.
    let offset = signal(0.0f32);
    let overlapped = signal(false);
    let last = Cell::new(0.0f32);
    let waiting: Rc<Cell<Option<Timeout>>> = Rc::default();
    let settling = signal(false);
    let settled_to = animated(0.0f32);
    effect(move || {
        let v = settled_to.get();
        if settling.get_untracked() {
            offset.set(v);
        }
    });
    let reach = if scroll == AppBarScroll::EnterAlways {
        open
    } else {
        limit
    };
    let scrolled = move |p: PointF| {
        let delta = p.y - last.replace(p.y);
        overlapped.set(p.y > 0.01);
        match scroll {
            AppBarScroll::Pinned => {}
            AppBarScroll::ExitUntilCollapsed => offset.set(p.y.clamp(0.0, reach)),
            AppBarScroll::EnterAlways => {
                settling.set(false);
                offset.update(|o| *o = (*o + delta).clamp(0.0, reach));
                if let Some(running) = waiting.take() {
                    running.cancel();
                }
                // Not left halfway once things have stopped moving.
                waiting.set(Some(set_timeout(Duration::from_millis(150), move || {
                    let now = offset.get_untracked();
                    if now > 0.0 && now < reach {
                        settled_to.set(now);
                        settling.set(true);
                        settled_to.spring_to(
                            if now < reach / 2.0 { 0.0 } else { reach },
                            default_effects(),
                        );
                    }
                })));
            }
        }
    };
    let lifts = scroll == AppBarScroll::EnterAlways;
    let shown = bar_view(bar, offset, overlapped);
    let shown = if lifts {
        // The lower row of a bar goes first.
        shown.dyn_style(move |s| s.translate_y(-(offset.get() - limit).max(0.0)))
    } else {
        shown
    };
    div()
        .relative()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            content
                .flex_grow(1.0)
                .overflow_y_scroll()
                .pt(open)
                .on_scrolled(scrolled),
        )
        .child(shown.absolute().left(0.0).right(0.0).top(0.0).z_index(1))
}

fn bar_view(bar: TopAppBar, offset: Signal<f32>, overlapped: Signal<bool>) -> Div {
    crate::ensure_ready();
    let TopAppBar {
        kind,
        title,
        subtitle,
        navigation,
        actions,
        centered,
    } = bar;
    let (open, large_style, sub_style, bottom_pad) = layout(kind, subtitle.is_some());
    let two_rows = open > COLLAPSED;
    let has_navigation = navigation.is_some();
    let limit = (open - COLLAPSED).max(1.0);
    // 0 open, 1 collapsed.
    let f = move || (offset.get() / limit).clamp(0.0, 1.0);
    let color_now = follow(
        move || {
            let c = color_scheme();
            if overlapped.get() {
                c.surface_container
            } else {
                c.surface
            }
        },
        default_effects(),
    );
    let background = move || {
        if two_rows {
            let c = color_scheme();
            let t = Easing::CubicBezier(0.4, 0.0, 1.0, 1.0).apply(f());
            mix_premultiplied(c.surface, c.surface_container, t)
        } else {
            color_now.get()
        }
    };
    let small_title = {
        let title = title.clone();
        let subtitle = if two_rows { None } else { subtitle.clone() };
        div()
            .absolute()
            .left(0.0)
            .right(0.0)
            .top(0.0)
            .h(COLLAPSED)
            .flex()
            .flex_col()
            .justify_center()
            .pointer_events_none()
            .dyn_style(move |s| {
                let s = if two_rows {
                    s.opacity(Easing::CubicBezier(0.8, 0.0, 0.8, 0.15).apply(f()))
                } else {
                    s
                };
                if centered {
                    s.items_center()
                } else {
                    s.items_start().pl(if has_navigation { 60.0 } else { 16.0 })
                }
            })
            .child(
                TextStyle::TITLE_LARGE
                    .apply(text(title))
                    .dyn_style(|s| s.text_color(color_scheme().on_surface)),
            )
            .children(subtitle.map(|line| {
                TextStyle::LABEL_MEDIUM
                    .apply(text(line))
                    .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
            }))
    };
    let top_row = div()
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .h(COLLAPSED)
        .px(4.0)
        .child(small_title)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .children(navigation)
                .z_index(1),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .children(actions)
                .z_index(1),
        );
    let lower = two_rows.then(|| {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .justify_end()
            .overflow_hidden()
            .px(16.0)
            .dyn_style(move |s| {
                let s = s.h((limit - offset.get()).max(0.0)).opacity(1.0 - f());
                if centered {
                    s.items_center()
                } else {
                    s.items_start()
                }
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .pb(bottom_pad - 8.0)
                    .child(
                        large_style
                            .apply(text(title))
                            .dyn_style(|s| s.text_color(color_scheme().on_surface)),
                    )
                    .children(subtitle.map(|line| {
                        sub_style
                            .apply(text(line))
                            .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
                    })),
            )
    });
    div()
        .flex()
        .flex_col()
        .flex_none()
        .select_none()
        .dyn_style(move |s| s.bg(background()))
        .child(top_row)
        .children(lower)
}

/// A bar at the bottom of a window with actions and maybe a FAB.
pub fn bottom_app_bar(actions: Vec<Div>, fab: Option<Div>) -> Div {
    crate::ensure_ready();
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .h(80.0)
        .pl(4.0)
        .pr(12.0)
        .dyn_style(|s| {
            let c = color_scheme();
            s.bg(c.surface_container).text_color(c.on_surface)
        })
        .child(div().flex().flex_row().items_center().children(actions))
        .children(fab)
}

/// A bar at the bottom of a window with evenly spread actions, which goes away while `hidden`
/// returns true.
pub fn flexible_bottom_app_bar(items: Vec<Div>, hidden: impl Fn() -> bool + 'static) -> Div {
    crate::ensure_ready();
    let height = follow(
        move || if hidden() { 0.0f32 } else { COLLAPSED },
        fast_spatial(),
    );
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .overflow_hidden()
        .px(16.0)
        .dyn_style(move |s| {
            let c = color_scheme();
            s.h(height.get().max(0.0))
                .bg(c.surface_container)
                .text_color(c.on_surface)
        })
        .children(items)
}
