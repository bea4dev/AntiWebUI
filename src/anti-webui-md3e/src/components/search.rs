//! Search bars: a pill that opens into a place for results, in place or over the whole window. Sizes,
//! colors and timing are those of Jetpack Compose. See `docs/design/20-md3e-components.md`.

use super::card::divider;
use super::icon::sized_icon;
use super::surface::When;
use crate::color::color_scheme;
use crate::icons::{self, Symbol};
use crate::motion::{default_spatial, fast_effects, slow_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const HEIGHT: f32 = 56.0;
const MIN_WIDTH: f32 = 360.0;
const DOCKED_MIN: f32 = 240.0;

/// What a search bar has and how it opens.
#[derive(Default)]
pub struct SearchStyle {
    placeholder: String,
    icon: Option<&'static Symbol>,
    full_screen: bool,
    width: Option<f32>,
}

impl SearchStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Instead of the magnifying glass.
    pub fn icon(mut self, symbol: &'static Symbol) -> Self {
        self.icon = Some(symbol);
        self
    }

    /// Opens over the whole window rather than below where it is.
    pub fn full_screen(mut self) -> Self {
        self.full_screen = true;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
}

fn pill_row(
    value: Signal<String>,
    icon: &'static Symbol,
    placeholder: String,
    input_ref: Option<NodeRef>,
    leading_click: Option<Rc<dyn Fn()>>,
    back: Option<Animated<f32>>,
) -> Div {
    let hint = move || value.with(|v| v.is_empty());
    let mut input = TextStyle::BODY_LARGE
        .apply(text_input())
        .value(value)
        .flex_grow(1.0)
        .flex_basis(0.0)
        .min_w(0.0)
        .bg(Color::TRANSPARENT)
        .dyn_style(|s| {
            let c = color_scheme();
            s.text_color(c.on_surface)
                .caret_color(c.primary)
                .selection_color(c.primary.with_alpha(0.4))
        });
    if let Some(input_ref) = input_ref {
        input = input.node_ref(input_ref);
    }
    let glass = sized_icon(icon, 24.0).absolute().dyn_style(move |s| {
        let v = back.map_or(0.0, |b| b.get()).clamp(0.0, 1.0);
        s.opacity(1.0 - v)
    });
    let arrow = back.map(|t| {
        sized_icon(icons::ARROW_BACK, 24.0)
            .absolute()
            .dyn_style(move |s| s.opacity(t.get().clamp(0.0, 1.0)))
    });
    let leading = div()
        .relative()
        .flex_none()
        .w(24.0)
        .h(24.0)
        .mx(16.0)
        .cursor(CursorIcon::Pointer)
        .dyn_style(|s| s.text_color(color_scheme().on_surface))
        .child(glass)
        .children(arrow);
    let leading = match leading_click {
        Some(f) => leading.on_click(move |_| f()),
        None => leading,
    };
    let clear = move |s: Style| {
        let s = s.text_color(color_scheme().on_surface_variant);
        if hint() {
            s.opacity(0.0).pointer_events_none()
        } else {
            s.cursor(CursorIcon::Pointer)
        }
    };
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .h(HEIGHT)
        .child(leading)
        .child(
            div()
                .relative()
                .flex()
                .flex_grow(1.0)
                .flex_basis(0.0)
                .min_w(0.0)
                .items_center()
                .child(input)
                .child(
                    TextStyle::BODY_LARGE
                        .apply(text(placeholder))
                        .absolute()
                        .left(0.0)
                        .pointer_events_none()
                        .dyn_style(move |s| {
                            let s = s.text_color(color_scheme().on_surface_variant);
                            if hint() { s } else { s.opacity(0.0) }
                        }),
                ),
        )
        .child(
            sized_icon(icons::CLOSE, 24.0)
                .mx(16.0)
                .dyn_style(clear)
                .on_click(move |_| value.set(String::new())),
        )
}

/// A search bar bound to `value`. It opens while `open` returns true and shows `results` then;
/// `on_open` gets what `open` should become, when it is clicked or left.
pub fn search_bar(
    value: Signal<String>,
    open: impl Fn() -> bool + 'static,
    on_open: impl Fn(bool) + 'static,
    results: impl Fn() -> Div + 'static,
    style: SearchStyle,
) -> Div {
    crate::ensure_ready();
    let SearchStyle {
        placeholder,
        icon,
        full_screen,
        width,
    } = style;
    let icon = icon.unwrap_or(icons::SEARCH);
    let open: When = Rc::new(open);
    let on_open: Rc<dyn Fn(bool)> = Rc::new(on_open);
    let t_open = open.clone();
    let t = animated(0.0f32);
    let was = Cell::new(false);
    effect(move || {
        let now = t_open();
        if was.replace(now) != now {
            untrack(|| {
                if now {
                    t.spring_to(1.0, slow_spatial());
                } else {
                    t.spring_to(0.0, default_spatial());
                }
            });
        }
    });
    let content_open = open.clone();
    let content_fade = follow(
        move || if content_open() { 1.0f32 } else { 0.0 },
        fast_effects(),
    );
    let bar = node_ref();
    let window = signal((0.0f32, 0.0f32));
    let content_height = signal(0.0f32);
    let wanted = open.clone();
    let click_open = on_open.clone();
    let key_close = on_open.clone();
    let scrim_close = on_open.clone();
    let arrow_close = on_open;
    let closed_row = pill_row(value, icon, placeholder.clone(), None, None, None);
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(width.unwrap_or(MIN_WIDTH))
        .h(HEIGHT)
        .rounded(HEIGHT / 2.0)
        .node_ref(bar)
        .select_none()
        .cursor(CursorIcon::Text)
        .dyn_style(move |s| {
            let c = color_scheme();
            s.bg(c.surface_container_high).text_color(c.on_surface)
        })
        .on_pointer_down(move |_| click_open(true))
        .child(closed_row)
        .child(show(
            move || wanted() || t.get() > 0.0,
            move || {
                let input = node_ref();
                let key_close = key_close.clone();
                let scrim_close = scrim_close.clone();
                let arrow_close = arrow_close.clone();
                let leading: Rc<dyn Fn()> = Rc::new(move || arrow_close(false));
                let row = pill_row(
                    value,
                    icon,
                    placeholder.clone(),
                    Some(input),
                    Some(leading),
                    Some(t),
                );
                let content = results();
                let view = portal()
                    .modal()
                    .child(
                        div()
                            .absolute()
                            .left(0.0)
                            .top(0.0)
                            .right(0.0)
                            .bottom(0.0)
                            .on_resized(move |w, h| window.set((w, h)))
                            .on_click(move |_| scrim_close(false)),
                    )
                    .child(
                        div()
                            .absolute()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .on_key_down(move |e| {
                                if matches!(e.key, Key::Named(NamedKey::Escape)) {
                                    e.prevent_default();
                                    key_close(false);
                                }
                            })
                            .dyn_style(move |s| {
                                let c = color_scheme();
                                let v = t.get().max(0.0);
                                let progress = v.min(1.0);
                                let (window_w, window_h) = window.get();
                                // Where it is as of the last layout, also if that was the first.
                                let rect = bar
                                    .logical_rect()
                                    .map_or((0.0, 0.0, MIN_WIDTH), |r| (r.x, r.y, r.width));
                                let (left, top, w, h, corner) = if full_screen {
                                    (
                                        rect.0 * (1.0 - progress),
                                        rect.1 * (1.0 - progress),
                                        rect.2 + (window_w - rect.2) * progress,
                                        HEIGHT + (window_h - HEIGHT) * v,
                                        HEIGHT / 2.0 * (1.0 - progress),
                                    )
                                } else {
                                    let open_h = (content_height.get() + HEIGHT + 1.0)
                                        .clamp(DOCKED_MIN, (window_h * 2.0 / 3.0).max(DOCKED_MIN));
                                    (rect.0, rect.1, rect.2, HEIGHT + (open_h - HEIGHT) * v, 28.0)
                                };
                                s.left(left)
                                    .top(top)
                                    .w(w)
                                    .h(h.max(HEIGHT))
                                    .rounded(corner)
                                    .bg(if full_screen {
                                        c.surface_container_low
                                    } else {
                                        c.surface_container_high
                                    })
                                    .text_color(c.on_surface)
                            })
                            .child(row)
                            .child(
                                div()
                                    .dyn_style(move |s| {
                                        s.opacity(content_fade.get().clamp(0.0, 1.0))
                                    })
                                    .child(divider()),
                            )
                            .child(
                                div()
                                    .flex_grow(1.0)
                                    .flex_basis(0.0)
                                    .min_h(0.0)
                                    .overflow_y_scroll()
                                    .dyn_style(move |s| {
                                        s.opacity(content_fade.get().clamp(0.0, 1.0))
                                    })
                                    .child(content.on_resized(move |_, h| content_height.set(h))),
                            ),
                    )
                    .into_view();
                input.focus();
                view
            },
        ))
}
