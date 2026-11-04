//! The components side by side. The first button switches to the dark scheme and back.
//!
//! `cargo run --release -p anti-webui-md3e --example gallery`

use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonSize, ButtonStyle, IconButtonStyle, button, button_group, connected_button_group,
    elevated_button, extended_fab, fab, filled_icon_button, icon, icon_button, icon_toggle_button,
    loading_indicator, outlined_button, outlined_icon_button, styled_button, text_button,
    toggle_button, tonal_button, tonal_icon_button, wavy_progress,
};
use anti_webui_md3e::{ColorScheme, color_scheme, icons, set_color_scheme};

fn row() -> Div {
    div().flex().flex_row().items_center().gap(12.0)
}

fn main() {
    App::new()
        .title("Material 3 Expressive")
        .size(900, 900)
        .run(|| {
            let progress = signal(0.4f32);
            let dark = signal(false);
            let off = signal(false);
            let bold = signal(false);
            let liked = signal(true);
            let tab = signal(0usize);
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
                .child(
                    row()
                        .child(styled_button(
                            "Save",
                            ButtonStyle::filled().icon(icons::SAVE),
                        ))
                        .child(
                            toggle_button("Bold", move || bold.get())
                                .on_click(move |_| bold.update(|b| *b = !*b)),
                        )
                        .child(icon_button(icons::SEARCH))
                        .child(filled_icon_button(icons::ADD))
                        .child(tonal_icon_button(icons::EDIT))
                        .child(outlined_icon_button(icons::DELETE))
                        .child(
                            icon_toggle_button(
                                icons::FAVORITE,
                                move || liked.get(),
                                IconButtonStyle::standard(),
                            )
                            .on_click(move |_| liked.update(|l| *l = !*l)),
                        ),
                )
                .child(
                    row()
                        .child(fab(icons::ADD))
                        .child(extended_fab(icons::EDIT, "Compose"))
                        .child(button_group([
                            button("Back"),
                            tonal_button("Play"),
                            button("Next"),
                        ]))
                        .child(connected_button_group(
                            ["Day", "Week", "Month"],
                            ButtonStyle::filled(),
                            move |i| tab.get() == i,
                            move |i| tab.set(i),
                        )),
                )
        })
        .unwrap();
}
