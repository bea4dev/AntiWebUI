//! Pictures. See `docs/design/22-images.md`.

use crate::theme::{Theme, themed};
use anti_webui_core::prelude::*;
use std::time::Duration;

/// How long a picture takes to come up once it is there.
const FADE: Transition = Transition::new(Duration::from_millis(180)).ease_out();

fn image_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface_muted).text_color(t.text_muted))
}

/// A picture that fills its box, cut as needed. Until it is there the box has the color of the
/// theme, then it fades in. If it can't be shown, `alt` says what it would have been. Say how big
/// the box is, or it will have the size of the picture as soon as that is known:
/// `image_view("a.jpg", "A cat").w(240.0).h(160.0).rounded(8.0)`.
pub fn image_view(source: impl IntoImageSource, alt: impl Into<String>) -> Img {
    let shown = signal(false);
    let failed = signal(false);
    let alt = alt.into();
    img(source)
        .object_fit(ObjectFit::Cover)
        .select_none()
        .sheet(|| themed(image_sheet))
        .transition_of([Prop::ImageOpacity], FADE)
        .dyn_style(move |s| s.image_opacity(if shown.get() { 1.0 } else { 0.0 }))
        .on_decoded(move || {
            shown.set(true);
            failed.set(false);
        })
        .on_error(move |_| failed.set(true))
        .child(show(
            move || failed.get(),
            move || {
                div()
                    .absolute()
                    .inset(0.0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(8.0)
                    .text_size(12.0)
                    .text_align(TextAlign::Center)
                    .child(text(alt.clone()))
            },
        ))
}

fn avatar_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.accent_muted)
            .text_color(t.accent_text)
            .font_weight(FontWeight::SEMIBOLD)
    })
}

/// What stands for someone without a picture: the first letters of the first two words.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

const AVATAR: f32 = 36.0;

fn avatar_initials(name: &str) -> Div {
    div()
        .absolute()
        .inset(0.0)
        .flex()
        .items_center()
        .justify_center()
        .text_size(14.0)
        .select_none()
        .child(text(initials(name)))
}

/// A round box with the first letters of a name. Give it another `.size()` for another size.
pub fn avatar(name: impl Into<String>) -> Div {
    div()
        .relative()
        .flex_none()
        .size(AVATAR)
        .rounded(AVATAR)
        .sheet(|| themed(avatar_sheet))
        .child(avatar_initials(&name.into()))
}

/// A round picture of someone, with the first letters of their name until it is there, and if it
/// can't be shown.
pub fn image_avatar(source: impl IntoImageSource, name: impl Into<String>) -> Img {
    let shown = signal(false);
    let name = name.into();
    img(source)
        .flex_none()
        .size(AVATAR)
        .rounded(AVATAR)
        .object_fit(ObjectFit::Cover)
        .sheet(|| themed(avatar_sheet))
        .on_decoded(move || shown.set(true))
        .on_error(move |_| shown.set(false))
        .child(show(move || !shown.get(), move || avatar_initials(&name)))
}

fn media_card_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface).border_color(t.border))
}

/// A card with a picture across the top. What goes below goes in `card_body`.
pub fn media_card(source: impl IntoImageSource, alt: impl Into<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(8.0)
        .border(1.0)
        .overflow_hidden()
        .sheet(|| themed(media_card_sheet))
        .child(image_view(source, alt).w_full().h(160.0).flex_none())
}

/// What goes below the picture of a `media_card`.
pub fn card_body() -> Div {
    div().flex().flex_col().gap(8.0).p(16.0)
}
