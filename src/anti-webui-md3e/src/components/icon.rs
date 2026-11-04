//! Icons. See `docs/design/18-icons.md`.

use crate::icons::Symbol;
use anti_webui::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    static REGISTERED: RefCell<HashSet<*const u8>> = RefCell::new(HashSet::new());
}

/// Fonts belong to the thread that uses them.
fn register(symbol: &'static Symbol) {
    if REGISTERED.with(|r| r.borrow_mut().insert(symbol.font.as_ptr())) {
        register_font_as(symbol.font, symbol.family).expect("the font of an icon");
    }
}

/// An icon of the usual size, in the color of the text around it. Axes such as `FILL` can be set
/// and animated like those of any text:
///
/// ```no_run
/// use anti_webui::prelude::*;
/// use anti_webui_md3e::components::icon;
/// use anti_webui_md3e::icons;
///
/// let liked = signal(false);
/// icon(icons::FAVORITE)
///     .transition(Transition::ms(200).ease_out())
///     .dyn_style(move |s| s.font_axis(b"FILL", if liked.get() { 1.0 } else { 0.0 }));
/// ```
pub fn icon(symbol: &'static Symbol) -> Div {
    sized_icon(symbol, 24.0)
}

/// An icon of a size in logical pixels. Its box is square, whatever the scale makes of the size.
pub fn sized_icon(symbol: &'static Symbol, size: f32) -> Div {
    register(symbol);
    let mut icon = div()
        .size(size)
        .flex_none()
        .font_family(symbol.family)
        .text_size(size)
        .line_height(LineHeight::Relative(1.0))
        .font_weight(FontWeight::NORMAL)
        .letter_spacing(0.0)
        .text_align(TextAlign::Center)
        .select_none();
    // What the text around it has doesn't mean anything for a picture.
    let style = icon.style_mut();
    style.font_style = Some(FontStyle::Normal);
    style.underline = Some(false);
    style.strikethrough = Some(false);
    icon.child(symbol.ch.to_string())
}
