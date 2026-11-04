//! Badges. Sizes and colors are those of Jetpack Compose, which doesn't animate them: here they
//! pop in and out with a spring. See `docs/design/20-md3e-components.md`.

use super::surface::following;
use crate::color::color_scheme;
use crate::motion::fast_spatial;
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;

const SMALL: f32 = 6.0;
const LARGE: f32 = 16.0;

/// A dot that says that something is new.
pub fn badge() -> Div {
    div()
        .w(SMALL)
        .h(SMALL)
        .flex_none()
        .rounded(SMALL / 2.0)
        .dyn_style(|s| s.bg(color_scheme().error))
}

/// A badge with a short text that may change, that is only there while the text isn't empty.
pub fn text_badge(label: impl Fn() -> String + 'static) -> Div {
    let label = Rc::new(label);
    let empty = label.clone();
    let shown = following(
        move || if empty().is_empty() { 0.0f32 } else { 1.0 },
        fast_spatial(),
    );
    let content = label.clone();
    TextStyle::LABEL_SMALL.apply(
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .min_w(LARGE)
            .h(LARGE)
            .px(4.0)
            .rounded(LARGE / 2.0)
            .dyn_style(move |s| {
                let c = color_scheme();
                let v = shown.get();
                let s = s.bg(c.error).text_color(c.on_error);
                if v <= 0.01 {
                    s.opacity(0.0)
                } else {
                    s.scale(v)
                }
            })
            .child(text(move || content())),
    )
}

/// Puts a badge on the top right corner of something, usually an icon.
pub fn badged(anchor: Div, badge: Div, large: bool) -> Div {
    let (over, top) = if large { (12.0, -2.0) } else { (SMALL, 0.0) };
    div().relative().flex_none().child(anchor).child(
        badge
            .absolute()
            .left(pct(100.0))
            .ml(-over)
            .top(top)
            .pointer_events_none(),
    )
}
