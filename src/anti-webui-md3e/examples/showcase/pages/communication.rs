//! Progress, badges, tooltips and snackbars.

use super::{demo_is, row, section};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonStyle, RichTooltip, Snackbar, badge, badged, button, circular_progress,
    circular_wavy_progress, filled_icon_button, icon, icon_button, indeterminate_circular_progress,
    indeterminate_circular_wavy_progress, indeterminate_linear_progress,
    indeterminate_wavy_progress, linear_progress, loading_indicator, outlined_button, rich_tooltip,
    slider, snackbars, styled_button, text_badge, tonal_button, tooltip, wavy_progress,
};
use anti_webui_md3e::icons;

pub fn page() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(24.0)
        .child(progress())
        .child(badges())
        .child(tooltips())
        .child(snacks())
}

fn progress() -> Div {
    let value = signal(0.6f32);
    section("Progress")
        .child(slider(value).w(320.0))
        .child(
            row()
                .gap(24.0)
                .child(circular_progress(move || value.get()))
                .child(circular_wavy_progress(move || value.get()))
                .child(indeterminate_circular_progress())
                .child(indeterminate_circular_wavy_progress())
                .child(loading_indicator()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(16.0)
                .child(linear_progress(move || value.get()).w(320.0))
                .child(wavy_progress(move || value.get()).w(320.0))
                .child(indeterminate_linear_progress().w(320.0))
                .child(indeterminate_wavy_progress().w(320.0)),
        )
}

fn badges() -> Div {
    let count = signal(3);
    section("Badge").child(
        row()
            .gap(24.0)
            .child(badged(icon(icons::NOTIFICATIONS), badge(), false))
            .child(badged(
                icon(icons::MAIL),
                text_badge(move || match count.get() {
                    0 => String::new(),
                    n => n.to_string(),
                }),
                true,
            ))
            .child(tonal_button("Add").on_click(move |_| count.update(|c| *c += 1)))
            .child(outlined_button("Clear").on_click(move |_| count.set(0))),
    )
}

fn tooltips() -> Div {
    section("Tooltip").child(
        row()
            .child(tooltip(icon_button(icons::SEARCH), "Search"))
            .child(tooltip(filled_icon_button(icons::ADD), "Add a new item"))
            .child(rich_tooltip(
                button("Rich tooltip"),
                RichTooltip::new("Rich tooltips say more, and can have something to do in them.")
                    .title("Title")
                    .action("Learn more", || {}),
            )),
    )
}

fn snacks() -> Div {
    let bars = snackbars();
    if demo_is("snackbar") {
        bars.show(
            Snackbar::new("Photo saved")
                .action("Undo", || {})
                .closable(),
        );
    }
    section("Snackbar")
        .child(
            row()
                .child(button("Short").on_click(move |_| bars.show(Snackbar::new("Photo saved"))))
                .child(
                    styled_button("With an action", ButtonStyle::tonal()).on_click(move |_| {
                        bars.show(Snackbar::new("Message archived").action("Undo", || {}));
                    }),
                )
                .child(
                    styled_button("Closable", ButtonStyle::outlined()).on_click(move |_| {
                        bars.show(
                            Snackbar::new("Couldn't reach the server")
                                .action("Retry", || {})
                                .closable(),
                        );
                    }),
                ),
        )
        .child(bars.view())
}
