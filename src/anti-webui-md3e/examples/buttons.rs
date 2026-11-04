//! The button family, for trying out.
//!
//! `cargo run --release -p anti-webui-md3e --example buttons`

use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonSize, ButtonStyle, FabColor, FabSize, FabStyle, IconButtonStyle, IconButtonWidth, button,
    button_group, connected_button_group, elevated_button, extended_fab, fab, filled_icon_button,
    icon_button, icon_toggle_button, outlined_button, outlined_icon_button, styled_button,
    styled_extended_fab, styled_fab, styled_icon_button, styled_toggle_button, text_button,
    tonal_button, tonal_icon_button,
};
use anti_webui_md3e::{ColorScheme, color_scheme, icons, set_color_scheme};

fn row() -> Div {
    div().flex().flex_row().items_center().gap(12.0)
}

fn heading(s: &str) -> Div {
    div().text_size(14.0).pt(8.0).child(s.to_string())
}

fn main() {
    App::new()
        .title("Buttons")
        .size(1000, 960)
        .run(|| {
            let dark = signal(false);
            effect(move || {
                set_color_scheme(if dark.get() {
                    ColorScheme::dark()
                } else {
                    ColorScheme::light()
                });
            });
            let toggles = [signal(false), signal(true), signal(false), signal(true)];
            let icon_toggles = [signal(false), signal(true), signal(false), signal(true)];
            let tab = signal(0usize);
            let open = signal(true);
            let sized = |size| {
                styled_button(
                    "Icon",
                    ButtonStyle::filled().size(size).icon(icons::FAVORITE),
                )
            };
            let toggle = |i: usize, label: &'static str, style: ButtonStyle| {
                let on = toggles[i];
                styled_toggle_button(label, move || on.get(), style.icon(icons::FAVORITE))
                    .on_click(move |_| on.update(|v| *v = !*v))
            };
            let icon_toggle = |i: usize, style: IconButtonStyle| {
                let on = icon_toggles[i];
                icon_toggle_button(icons::FAVORITE, move || on.get(), style)
                    .on_click(move |_| on.update(|v| *v = !*v))
            };
            div()
                .flex()
                .flex_col()
                .gap(10.0)
                .p(20.0)
                .size_full()
                .dyn_style(|s| {
                    let c = color_scheme();
                    s.bg(c.surface).text_color(c.on_surface)
                })
                .child(
                    row()
                        .child(heading(
                            "With an icon. The button at the end switches to dark.",
                        ))
                        .child(icon_button(icons::DARK_MODE).on_click(move |_| {
                            dark.update(|d| *d = !*d);
                        })),
                )
                .child(
                    row()
                        .child(sized(ButtonSize::ExtraSmall))
                        .child(sized(ButtonSize::Small))
                        .child(sized(ButtonSize::Medium))
                        .child(sized(ButtonSize::Large))
                        .child(styled_button(
                            "Next",
                            ButtonStyle::outlined().trailing_icon(icons::ARROW_FORWARD),
                        )),
                )
                .child(heading(
                    "Toggle: click them. The shape bounces and the icon fills in.",
                ))
                .child(
                    row()
                        .child(toggle(0, "Filled", ButtonStyle::filled()))
                        .child(toggle(1, "Tonal", ButtonStyle::tonal()))
                        .child(toggle(2, "Elevated", ButtonStyle::elevated()))
                        .child(toggle(3, "Outlined", ButtonStyle::outlined())),
                )
                .child(heading("Icon buttons, and icon toggles"))
                .child(
                    row()
                        .child(icon_button(icons::SEARCH))
                        .child(filled_icon_button(icons::ADD))
                        .child(tonal_icon_button(icons::EDIT))
                        .child(outlined_icon_button(icons::DELETE))
                        .child(styled_icon_button(
                            icons::MENU,
                            IconButtonStyle::filled().width(IconButtonWidth::Narrow),
                        ))
                        .child(styled_icon_button(
                            icons::MENU,
                            IconButtonStyle::filled().width(IconButtonWidth::Wide),
                        ))
                        .child(styled_icon_button(
                            icons::MENU,
                            IconButtonStyle::tonal().size(ButtonSize::Medium).square(),
                        ))
                        .child(icon_toggle(0, IconButtonStyle::standard()))
                        .child(icon_toggle(1, IconButtonStyle::filled()))
                        .child(icon_toggle(2, IconButtonStyle::tonal()))
                        .child(icon_toggle(3, IconButtonStyle::outlined())),
                )
                .child(heading(
                    "Floating action buttons. The first extended one opens and closes.",
                ))
                .child(
                    row()
                        .child(fab(icons::ADD))
                        .child(styled_fab(
                            icons::ADD,
                            FabStyle::new()
                                .size(FabSize::Medium)
                                .color(FabColor::SecondaryContainer),
                        ))
                        .child(styled_fab(
                            icons::ADD,
                            FabStyle::new()
                                .size(FabSize::Large)
                                .color(FabColor::Tertiary),
                        ))
                        .child(
                            styled_extended_fab(
                                icons::EDIT,
                                "Compose",
                                FabStyle::new().expanded_when(move || open.get()),
                            )
                            .on_click(move |_| open.update(|o| *o = !*o)),
                        )
                        .child(extended_fab(icons::EDIT, "Compose")),
                )
                .child(heading(
                    "A group: hold a button down, and it takes from its neighbors.",
                ))
                .child(
                    row()
                        .child(button_group([
                            button("First"),
                            tonal_button("Second"),
                            elevated_button("Third"),
                            outlined_button("Fourth"),
                            text_button("Fifth"),
                        ]))
                        .child(button_group([
                            filled_icon_button(icons::SKIP_PREVIOUS),
                            tonal_icon_button(icons::PLAY_ARROW),
                            filled_icon_button(icons::SKIP_NEXT),
                        ])),
                )
                .child(heading("A connected group: choose one."))
                .child(connected_button_group(
                    ["Day", "Week", "Month", "Year"],
                    ButtonStyle::filled(),
                    move |i| tab.get() == i,
                    move |i| tab.set(i),
                ))
        })
        .unwrap();
}
