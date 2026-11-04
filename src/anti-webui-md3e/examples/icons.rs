//! Icons, for looking at. Click the heart.
//!
//! `cargo run --release -p anti-webui-md3e --example icons`

use anti_webui::prelude::*;
use anti_webui_md3e::components::{icon, sized_icon};
use anti_webui_md3e::icons;
use anti_webui_md3e::{ColorScheme, color_scheme, set_color_scheme};

fn row() -> Div {
    div().flex().flex_row().items_center().gap(12.0)
}

fn label(s: &str) -> Div {
    div().w(150.0).text_size(14.0).child(s.to_string())
}

fn main() {
    App::new()
        .title("Icons")
        .size(760, 560)
        .run(|| {
            let liked = signal(false);
            let dark = signal(false);
            effect(move || {
                set_color_scheme(if dark.get() {
                    ColorScheme::dark()
                } else {
                    ColorScheme::light()
                });
            });
            let some = [
                icons::HOME,
                icons::SEARCH,
                icons::SETTINGS,
                icons::FAVORITE,
                icons::DELETE,
                icons::ARROW_BACK,
                icons::CHECK,
                icons::_10K,
            ];
            let all = move |f: fn(Div) -> Div| {
                row().children(some.iter().map(move |s| f(icon(s))).collect::<Vec<_>>())
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap(16.0)
                .p(24.0)
                .dyn_style(move |s| {
                    let c = color_scheme();
                    s.bg(c.surface).text_color(c.on_surface)
                })
                .child(row().child(label("Usual")).child(all(|i| i)))
                .child(
                    row()
                        .child(label("Filled"))
                        .child(all(|i| i.font_axis(b"FILL", 1.0))),
                )
                .child(
                    row()
                        .child(label("Weight 200 / 700"))
                        .child(all(|i| i.font_weight(FontWeight(200))))
                        .child(icon(icons::HOME).font_weight(FontWeight::BOLD)),
                )
                .child(
                    row()
                        .child(label("Grade -50 / 200"))
                        .child(icon(icons::SEARCH).font_axis(b"GRAD", -50.0))
                        .child(icon(icons::SEARCH))
                        .child(icon(icons::SEARCH).font_axis(b"GRAD", 200.0)),
                )
                .child(row().child(label("20, 24, 32, 40, 48")).children(
                    [20.0, 24.0, 32.0, 40.0, 48.0].map(|size| sized_icon(icons::SETTINGS, size)),
                ))
                .child(
                    row().child(label("Next to text")).child(
                        row()
                            .gap(4.0)
                            .text_size(16.0)
                            .child(icon(icons::DOWNLOAD))
                            .child("Download")
                            .child(sized_icon(icons::ARROW_FORWARD, 16.0)),
                    ),
                )
                .child(
                    row().child(label("Click: FILL moves")).child(
                        sized_icon(icons::FAVORITE, 48.0)
                            .transition(Transition::ms(200).ease_out())
                            .dyn_style(move |s| {
                                s.font_axis(b"FILL", if liked.get() { 1.0 } else { 0.0 })
                            })
                            .on_click(move |_| liked.update(|l| *l = !*l)),
                    ),
                )
                .child(
                    row()
                        .child(label("Click: dark"))
                        .child(icon(icons::DARK_MODE).on_click(move |_| dark.update(|d| *d = !*d))),
                )
        })
        .unwrap();
}
