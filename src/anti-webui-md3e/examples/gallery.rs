//! The components side by side. The first button switches to the dark scheme and back.
//!
//! `cargo run --release -p anti-webui-md3e --example gallery`

use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonSize, ButtonStyle, button, elevated_button, icon, loading_indicator, outlined_button,
    styled_button, text_button, tonal_button, wavy_progress,
};
use anti_webui_md3e::{ColorScheme, color_scheme, icons, set_color_scheme};

fn row() -> Div {
    div().flex().flex_row().items_center().gap(12.0)
}

fn main() {
    App::new()
        .title("Material 3 Expressive")
        .size(900, 720)
        .run(|| {
            let progress = signal(0.4f32);
            let dark = signal(false);
            let off = signal(false);
            effect(move || {
                set_color_scheme(if dark.get() {
                    ColorScheme::dark()
                } else {
                    ColorScheme::light()
                });
            });
            div()
                .flex()
                .flex_col()
                .gap(24.0)
                .p(24.0)
                .h(pct(100.0))
                .dyn_style(|s| {
                    let c = color_scheme();
                    s.bg(c.surface).text_color(c.on_surface)
                })
                .child(
                    row()
                        .child(button("Filled").on_click(move |_| dark.update(|d| *d = !*d)))
                        .child(tonal_button("Tonal"))
                        .child(elevated_button("Elevated"))
                        .child(outlined_button("Outlined"))
                        .child(text_button("Text")),
                )
                .child(
                    row()
                        .child(styled_button(
                            "XS",
                            ButtonStyle::filled().size(ButtonSize::ExtraSmall),
                        ))
                        .child(styled_button("Small", ButtonStyle::tonal().square()))
                        .child(styled_button(
                            "Medium",
                            ButtonStyle::filled().size(ButtonSize::Medium).square(),
                        ))
                        .child(styled_button(
                            "Large",
                            ButtonStyle::outlined().size(ButtonSize::Large),
                        )),
                )
                .child(
                    row()
                        .child(styled_button(
                            "Disabled",
                            ButtonStyle::filled().disabled_when(move || off.get()),
                        ))
                        .child(text_button("Toggle").on_click(move |_| off.update(|v| *v = !*v))),
                )
                .child(
                    row()
                        .child(wavy_progress(move || progress.get()))
                        .child(text_button("-10%").on_click(move |_| {
                            progress.update(|p| *p = (*p - 0.1).max(0.0));
                        }))
                        .child(text_button("+10%").on_click(move |_| {
                            progress.update(|p| *p = (*p + 0.1).min(1.0));
                        })),
                )
                .child(row().child(loading_indicator()))
                .child(
                    row()
                        .child(icon(icons::HOME))
                        .child(icon(icons::SEARCH))
                        .child(icon(icons::SETTINGS))
                        .child(icon(icons::FAVORITE).font_axis(b"FILL", 1.0)),
                )
        })
        .unwrap();
}
