//! Cards, lists, dialogs, sheets and the carousel.

use super::{demo_is, row, section};
use anti_webui::prelude::*;
use anti_webui_md3e::color_scheme;
use anti_webui_md3e::components::{
    AlertDialog, ButtonStyle, CardKind, ListItemStyle, alert_dialog, bottom_sheet, button,
    carousel, clickable_card, dialog, divider, elevated_card, filled_card, icon, list_item,
    outlined_card, segmented_list, side_sheet, styled_button, styled_list_item, switch,
    text_button, tonal_button, vertical_divider,
};
use anti_webui_md3e::icons;
use anti_webui_md3e::typography::TextStyle;

pub fn page() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(24.0)
        .child(cards())
        .child(lists())
        .child(modals())
        .child(photos())
}

fn card_text(title: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(4.0)
        .w(168.0)
        .p(16.0)
        .child(TextStyle::TITLE_MEDIUM.apply(text(title.to_string())))
        .child(TextStyle::BODY_MEDIUM.apply(text("Supporting text that says a little more.")))
}

fn cards() -> Div {
    section("Card")
        .child(
            row()
                .items_start()
                .child(filled_card().child(card_text("Filled")))
                .child(elevated_card().child(card_text("Elevated")))
                .child(outlined_card().child(card_text("Outlined"))),
        )
        .child(
            row()
                .items_start()
                .child(clickable_card(CardKind::Filled).child(card_text("Click me")))
                .child(clickable_card(CardKind::Elevated).child(card_text("Click me")))
                .child(clickable_card(CardKind::Outlined).child(card_text("Click me"))),
        )
        .child(
            row()
                .h(40.0)
                .child(text("Left"))
                .child(vertical_divider())
                .child(text("Right")),
        )
        .child(divider().w(320.0))
}

fn lists() -> Div {
    let chosen = signal(1usize);
    let wifi = signal(true);
    section("List").child(
        row()
            .items_start()
            .gap(24.0)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(320.0)
                    .child(styled_list_item(
                        "One line",
                        ListItemStyle::new().leading(icon(icons::PERSON)),
                    ))
                    .child(styled_list_item(
                        "Two lines",
                        ListItemStyle::new()
                            .supporting("Something more about it")
                            .leading(icon(icons::MAIL))
                            .trailing(text("12:30")),
                    ))
                    .child(styled_list_item(
                        "Three lines",
                        ListItemStyle::new()
                            .overline("Overline")
                            .supporting("Something more about it")
                            .trailing(icon(icons::CHEVRON_RIGHT)),
                    ))
                    .child(
                        styled_list_item(
                            "Wi-Fi",
                            ListItemStyle::new()
                                .interactive()
                                .trailing(switch(move || wifi.get())),
                        )
                        .on_click(move |_| wifi.update(|v| *v = !*v)),
                    )
                    .child(list_item("Plain")),
            )
            .child(segmented_list(4, move |i, style: ListItemStyle| {
                styled_list_item(
                    ["Inbox", "Sent", "Drafts", "Trash"][i],
                    style
                        .leading(icon(
                            [icons::INBOX, icons::SEND, icons::DRAFTS, icons::DELETE][i],
                        ))
                        .selected_when(move || chosen.get() == i),
                )
                .w(280.0)
                .on_click(move |_| chosen.set(i))
            })),
    )
}

fn modals() -> Div {
    let alert = signal(demo_is("alert"));
    let custom = signal(demo_is("dialog"));
    let sheet = signal(demo_is("sheet"));
    let side = signal(demo_is("side"));
    section("Dialog and sheets")
        .child(
            row()
                .child(button("Alert dialog").on_click(move |_| alert.set(true)))
                .child(tonal_button("Dialog").on_click(move |_| custom.set(true)))
                .child(
                    styled_button("Bottom sheet", ButtonStyle::outlined())
                        .on_click(move |_| sheet.set(true)),
                )
                .child(
                    styled_button("Side sheet", ButtonStyle::outlined())
                        .on_click(move |_| side.set(true)),
                ),
        )
        .child(alert_dialog(
            move || alert.get(),
            move || alert.set(false),
            AlertDialog::new()
                .icon(icons::DELETE)
                .title("Delete this item?")
                .text("It can't be brought back. Everything that uses it will stop working.")
                .dismiss(move || text_button("Cancel").on_click(move |_| alert.set(false)))
                .confirm(move || text_button("Delete").on_click(move |_| alert.set(false))),
        ))
        .child(dialog(
            move || custom.get(),
            move || custom.set(false),
            move || {
                div()
                    .flex()
                    .flex_col()
                    .gap(16.0)
                    .p(24.0)
                    .w(320.0)
                    .child(TextStyle::HEADLINE_SMALL.apply(text("Anything goes")))
                    .child(styled_list_item(
                        "A row in a dialog",
                        ListItemStyle::new().leading(icon(icons::STAR)),
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(text_button("Close").on_click(move |_| custom.set(false))),
                    )
            },
        ))
        .child(bottom_sheet(
            move || sheet.get(),
            move || sheet.set(false),
            move || {
                div()
                    .flex()
                    .flex_col()
                    .pb(24.0)
                    .child(styled_list_item(
                        "Share",
                        ListItemStyle::new()
                            .interactive()
                            .leading(icon(icons::SHARE)),
                    ))
                    .child(styled_list_item(
                        "Get link",
                        ListItemStyle::new()
                            .interactive()
                            .leading(icon(icons::LINK)),
                    ))
                    .child(styled_list_item(
                        "Edit name",
                        ListItemStyle::new()
                            .interactive()
                            .leading(icon(icons::EDIT)),
                    ))
                    .child(styled_list_item(
                        "Delete",
                        ListItemStyle::new()
                            .interactive()
                            .leading(icon(icons::DELETE)),
                    ))
            },
        ))
        .child(side_sheet(
            move || side.get(),
            move || side.set(false),
            move || {
                div()
                    .flex()
                    .flex_col()
                    .gap(16.0)
                    .p(24.0)
                    .w(320.0)
                    .child(TextStyle::TITLE_LARGE.apply(text("Filters")))
                    .child(
                        TextStyle::BODY_MEDIUM
                            .apply(text("A side sheet holds what goes with the page."))
                            .dyn_style(|s| s.text_color(color_scheme().on_surface_variant)),
                    )
                    .child(text_button("Close").on_click(move |_| side.set(false)))
            },
        ))
}

fn photos() -> Div {
    let colors = [
        rgb(0x6750a4),
        rgb(0x7d5260),
        rgb(0x386a20),
        rgb(0x006a6a),
        rgb(0x8c4a00),
        rgb(0x3a5ba0),
        rgb(0x984061),
        rgb(0x4f5b62),
    ];
    section("Carousel").child(div().w(720.0).child(carousel(8, 240.0, move |i| {
        div()
            .size_full()
            .flex()
            .items_end()
            .p(16.0)
            .bg(colors[i])
            .text_color(Color::WHITE)
            .child(TextStyle::TITLE_MEDIUM.apply(text(format!("Photo {}", i + 1))))
    })))
}
