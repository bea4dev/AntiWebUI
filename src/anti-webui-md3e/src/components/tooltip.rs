//! Tooltips. Sizes, colors and timing are those of Jetpack Compose, plus a short wait before they
//! show, which a pointer on a desktop needs. See `docs/design/20-md3e-components.md`.

use super::surface::elevation_shadows;
use crate::color::color_scheme;
use crate::motion::{fast_effects, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
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

/// When the tooltips show: a little sooner than the standard ones, and they wait a little before
/// they go, so that the pointer can reach a rich one.
fn options() -> TooltipOptions {
    TooltipOptions::new()
        .show_after(SHOW_AFTER)
        .hide_after(HIDE_AFTER)
        .gap(SPACE)
}

/// A tooltip on the standard behavior (`tooltip_with`), which grows and fades in and out.
fn hanging(
    target: Div,
    options: TooltipOptions,
    content: impl Fn(TooltipState) -> Div + 'static,
) -> Div {
    crate::ensure_ready();
    tooltip_with(target, options, move |tip| {
        let fade = follow(
            move || if tip.open() { 1.0f32 } else { 0.0 },
            fast_effects(),
        );
        let size = follow(
            move || if tip.open() { 1.0f32 } else { 0.8 },
            fast_spatial(),
        );
        tip.hold_while(move || fade.get() > 0.0);
        move || {
            content(tip).dyn_style(move |s| s.opacity(fade.get().clamp(0.0, 1.0)).scale(size.get()))
        }
    })
}

/// Shows a short text next to `target` while the pointer rests on it. `target` is what comes back,
/// and can be styled further.
pub fn tooltip(target: Div, label: impl Into<String>) -> Div {
    let label = label.into();
    hanging(target, options(), move |_| {
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
    })
}

/// Shows a title, a text and something to do next to `target` while the pointer rests on it or on
/// the tooltip.
pub fn rich_tooltip(target: Div, content: RichTooltip) -> Div {
    let options = options()
        .kind(PopupKind::Bubble)
        .placement(Placement::AboveStart);
    hanging(target, options, move |tip| {
        let RichTooltip {
            title,
            text: body,
            action,
        } = content.clone();
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
        tip.stay(div())
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
    })
}
