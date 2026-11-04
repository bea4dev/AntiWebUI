//! Tooltips. Sizes, colors and timing are those of Jetpack Compose, plus a short wait before they
//! show, which a pointer on a desktop needs. See `docs/design/20-md3e-components.md`.

use super::surface::{elevation_shadows, following};
use crate::color::color_scheme;
use crate::motion::{fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const SHOW_AFTER: Duration = Duration::from_millis(400);
const HIDE_AFTER: Duration = Duration::from_millis(200);
const SPACE: i32 = 4;

/// What a rich tooltip says.
#[derive(Clone, Default)]
pub struct RichTooltip {
    title: Option<String>,
    text: String,
    action: Option<(String, Rc<dyn Fn()>)>,
}

impl RichTooltip {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Something to do from the tooltip.
    pub fn action(mut self, label: impl Into<String>, f: impl Fn() + 'static) -> Self {
        self.action = Some((label.into(), Rc::new(f)));
        self
    }
}

/// Whether a tooltip should be up, from the hovering of its target and of itself.
#[derive(Clone, Copy)]
struct Hover {
    wanted: Signal<bool>,
    inside: Signal<u8>,
}

impl Hover {
    fn new() -> Self {
        Self {
            wanted: signal(false),
            inside: signal(0),
        }
    }

    /// Follows the pointer going in and out of something, one of two.
    fn track(self, bit: u8, on: bool, timer: &Rc<Cell<Option<Timeout>>>) {
        self.inside.update(|v| {
            if on {
                *v |= bit;
            } else {
                *v &= !bit;
            }
        });
        if let Some(running) = timer.take() {
            running.cancel();
        }
        let wait = if self.inside.get_untracked() != 0 {
            if self.wanted.get_untracked() {
                return;
            }
            SHOW_AFTER
        } else {
            if !self.wanted.get_untracked() {
                return;
            }
            HIDE_AFTER
        };
        let to = self.inside.get_untracked() != 0;
        let wanted = self.wanted;
        timer.set(Some(set_timeout(wait, move || wanted.set(to))));
    }
}

fn hanging(
    target: Div,
    kind: PopupKind,
    placement: Placement,
    hover: Hover,
    timer: Rc<Cell<Option<Timeout>>>,
    content: impl Fn() -> Div + 'static,
) -> Div {
    crate::ensure_ready();
    let wanted = hover.wanted;
    let on = move || if wanted.get() { 1.0f32 } else { 0.0 };
    let fade = following(on, fast_effects());
    let size = following(
        move || if wanted.get() { 1.0f32 } else { 0.8 },
        fast_spatial(),
    );
    let target_timer = timer;
    target
        .on_hover(move |on| hover.track(1, on, &target_timer))
        .on_pointer_down(move |_| {
            wanted.set(false);
        })
        .child(show(
            move || wanted.get() || fade.get() > 0.0,
            move || {
                popup().kind(kind).placement(placement).gap(SPACE).child(
                    content().dyn_style(move |s| {
                        s.opacity(fade.get().clamp(0.0, 1.0)).scale(size.get())
                    }),
                )
            },
        ))
}

/// Shows a short text next to `target` while the pointer rests on it. `target` is what comes back,
/// and can be styled further.
pub fn tooltip(target: Div, label: impl Into<String>) -> Div {
    let label = label.into();
    hanging(
        target,
        PopupKind::Tooltip,
        Placement::AboveCenter,
        Hover::new(),
        Rc::default(),
        move || {
            TextStyle::BODY_SMALL.apply(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w(40.0)
                    .min_h(24.0)
                    .max_w(200.0)
                    .px(8.0)
                    .py(4.0)
                    .rounded(4.0)
                    .dyn_style(|s| {
                        let c = color_scheme();
                        s.bg(c.inverse_surface).text_color(c.inverse_on_surface)
                    })
                    .child(text(label.clone())),
            )
        },
    )
}

/// Shows a title, a text and something to do next to `target` while the pointer rests on it or on
/// the tooltip.
pub fn rich_tooltip(target: Div, content: RichTooltip) -> Div {
    let hover = Hover::new();
    let timer: Rc<Cell<Option<Timeout>>> = Rc::default();
    let inner_timer = timer.clone();
    hanging(
        target,
        PopupKind::Bubble,
        Placement::AboveStart,
        hover,
        timer,
        move || {
            let RichTooltip {
                title,
                text: body,
                action,
            } = content.clone();
            let inner_timer = inner_timer.clone();
            let title = title.map(|title| {
                TextStyle::TITLE_SMALL
                    .apply(div().pt(12.0).child(text(title)))
                    .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
            });
            let action = action.map(|(label, f)| {
                TextStyle::LABEL_LARGE.apply(
                    div()
                        .flex()
                        .items_center()
                        .self_start()
                        .min_h(36.0)
                        .cursor(CursorIcon::Pointer)
                        .dyn_style(|s| s.text_color(color_scheme().primary))
                        .on_click(move |_| f())
                        .child(text(label)),
                )
            });
            div()
                .flex()
                .flex_col()
                .min_w(40.0)
                .min_h(24.0)
                .max_w(320.0)
                .px(16.0)
                .pb(8.0)
                .rounded(12.0)
                .shadows(elevation_shadows(2.0))
                .dyn_style(|s| s.bg(color_scheme().surface_container))
                .on_hover(move |on| hover.track(2, on, &inner_timer))
                .children(title)
                .child(
                    TextStyle::BODY_MEDIUM.apply(
                        div()
                            .pt(8.0)
                            .pb(8.0)
                            .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
                            .child(text(body)),
                    ),
                )
                .children(action)
        },
    )
}
