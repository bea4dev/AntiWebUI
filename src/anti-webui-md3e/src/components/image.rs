//! Pictures: what shows while they load, and where they go in other components. See
//! `docs/design/22-images.md`.

use super::icon::sized_icon;
use crate::color::color_scheme;
use crate::icons;
use anti_webui::prelude::*;
use std::time::Duration;

/// As long as the fade of an image that Material describes for loading.
const FADE: Transition = Transition::new(Duration::from_millis(300)).ease_out();
const AVATAR: f32 = 40.0;

/// A picture that fills its box, cut as needed. Until it is there the box is a surface, then it
/// fades in. If it can't be shown there is an icon instead.
pub fn image(source: impl IntoImageSource) -> Img {
    crate::ensure_ready();
    let shown = signal(false);
    let failed = signal(false);
    img(source)
        .object_fit(ObjectFit::Cover)
        .select_none()
        .transition_of([Prop::ImageOpacity], FADE)
        .dyn_style(move |s| {
            let c = color_scheme();
            s.bg(c.surface_container_highest)
                .text_color(c.on_surface_variant)
                .image_opacity(if shown.get() { 1.0 } else { 0.0 })
        })
        .on_decoded(move || {
            shown.set(true);
            failed.set(false);
        })
        .on_error(move |_| failed.set(true))
        .child(show(
            move || failed.get(),
            || {
                div()
                    .absolute()
                    .inset(0.0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(sized_icon(icons::BROKEN_IMAGE, 24.0))
            },
        ))
}

/// What stands for someone without a picture: the first letters of the first two words.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

fn initials_in_box(name: &str) -> Div {
    crate::typography::TextStyle::TITLE_MEDIUM.apply(
        div()
            .absolute()
            .inset(0.0)
            .flex()
            .items_center()
            .justify_center()
            .select_none()
            .child(text(initials(name))),
    )
}

/// A round box with the first letters of a name, of the size for the start of a list item.
pub fn initials_avatar(name: impl Into<String>) -> Div {
    crate::ensure_ready();
    div()
        .relative()
        .flex_none()
        .size(AVATAR)
        .rounded(AVATAR)
        .dyn_style(|s| {
            let c = color_scheme();
            s.bg(c.primary_container).text_color(c.on_primary_container)
        })
        .child(initials_in_box(&name.into()))
}

/// A round picture of someone, of the size for the start of a list item, with the first letters of
/// their name until it is there, and if it can't be shown.
pub fn avatar(source: impl IntoImageSource, name: impl Into<String>) -> Img {
    crate::ensure_ready();
    let shown = signal(false);
    let name = name.into();
    img(source)
        .flex_none()
        .size(AVATAR)
        .rounded(AVATAR)
        .object_fit(ObjectFit::Cover)
        .dyn_style(|s| {
            let c = color_scheme();
            s.bg(c.primary_container).text_color(c.on_primary_container)
        })
        .on_decoded(move || shown.set(true))
        .on_error(move |_| shown.set(false))
        .child(show(move || !shown.get(), move || initials_in_box(&name)))
}

/// A picture across the top of a card.
pub fn card_media(source: impl IntoImageSource) -> Img {
    image(source).w_full().h(168.0).flex_none()
}

/// A picture for an item of a carousel, filling the item.
pub fn carousel_image(source: impl IntoImageSource) -> Img {
    image(source).w_full().h_full()
}
