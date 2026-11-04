//! The pages of the showcase.

pub mod actions;
pub mod communication;
pub mod containment;
pub mod inputs;
pub mod navigation;
pub mod selection;

use anti_webui::prelude::*;
use anti_webui_md3e::color_scheme;
use anti_webui_md3e::typography::TextStyle;

/// Whether a shot asks for this to be open from the start. See `main`.
pub fn demo_is(name: &str) -> bool {
    std::env::var("SHOWCASE_OPEN").is_ok_and(|v| v == name)
}

/// A row of things to try.
pub fn row() -> Div {
    div().flex().flex_row().items_center().gap(12.0)
}

/// A title with what it is about below it.
pub fn section(title: &str) -> Div {
    div().flex().flex_col().gap(12.0).child(
        TextStyle::TITLE_MEDIUM
            .apply(text(title.to_string()))
            .dyn_style(|s| s.text_color(color_scheme().primary)),
    )
}
