//! The axes of a variable font to look at: hover the lines, and watch the slow ones for steps (see
//! `docs/design/17-font-axes.md`).
//!
//! `cargo run --release -p anti-webui --example font_axes`

use anti_webui::prelude::*;

static FONT: &[u8] = include_bytes!("../../anti-webui-md3e/fonts/RobotoFlex.ttf");
const FAMILY: &str = "Roboto Flex";

fn line(label: &str, view: Div) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(16.0)
        .child(
            div()
                .w(220.0)
                .flex_none()
                .text_size(14.0)
                .text_color(rgb(0x707780))
                .child(label.to_string()),
        )
        .child(view)
}

fn hovered(label: &str, text: &str, state: impl Fn(Style) -> Style + 'static) -> Div {
    line(
        label,
        div()
            .text_size(28.0)
            .transition(Transition::ms(220).ease_out())
            .hover(state)
            .child(text.to_string()),
    )
}

/// Goes up and down an axis over and over.
fn sweeping(
    label: &str,
    seconds: f32,
    size: f32,
    f: impl Fn(Style, f32) -> Style + 'static,
) -> Div {
    let at = signal(0.0_f32);
    sweep(None, seconds, at);
    line(
        label,
        div()
            .text_size(size)
            .dyn_style(move |s| f(s, at.get()))
            .child("Handgloves"),
    )
}

fn sweep(started: Option<std::time::Instant>, seconds: f32, at: Signal<f32>) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        let t = now.saturating_duration_since(started).as_secs_f32() / seconds;
        at.set(1.0 - (t.fract() * 2.0 - 1.0).abs());
        sweep(Some(started), seconds, at);
    });
}

fn main() {
    App::new()
        .title("font axes")
        .size(1000, 880)
        .run(|| {
            register_font(FONT).unwrap();
            div()
                .size_full()
                .p(24.0)
                .flex()
                .flex_col()
                .gap(14.0)
                .bg(rgb(0xfafafa))
                .text_color(rgb(0x1a1c1e))
                .font_family(FAMILY)
                .child(hovered(
                    "Hover: GRAD, same width",
                    "Save the changes",
                    |s| s.font_axis(b"GRAD", 150.0),
                ))
                .child(hovered("Hover: font_weight", "Save the changes", |s| {
                    s.font_weight(FontWeight::BOLD)
                }))
                .child(hovered("Hover: wdth", "Save the changes", |s| {
                    s.font_axis(b"wdth", 151.0)
                }))
                .child(hovered("Hover: slnt and XTRA", "Save the changes", |s| {
                    s.font_axis(b"slnt", -10.0).font_axis(b"XTRA", 603.0)
                }))
                .child(line(
                    "opsz follows the size",
                    div().text_size(64.0).child("Hamburg"),
                ))
                .child(line(
                    "opsz set to 14",
                    div()
                        .text_size(64.0)
                        .font_axis(b"opsz", 14.0)
                        .child("Hamburg"),
                ))
                .child(sweeping("wght, 8 s: any steps?", 8.0, 28.0, |s, t| {
                    s.font_axis(b"wght", 300.0 + t * 500.0)
                }))
                .child(sweeping("wght, 8 s, big", 8.0, 90.0, |s, t| {
                    s.font_axis(b"wght", 300.0 + t * 500.0)
                }))
                .child(sweeping("GRAD, 1 s", 1.0, 28.0, |s, t| {
                    s.font_axis(b"GRAD", t * 150.0)
                }))
                .child(sweeping("GRAD, 8 s, big: any steps?", 8.0, 90.0, |s, t| {
                    s.font_axis(b"GRAD", t * 150.0)
                }))
        })
        .unwrap();
}
