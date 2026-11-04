//! Switches, checkboxes, radio buttons, sliders and chips.

use super::{row, section};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    CheckState, CheckboxStyle, ChipStyle, RadioStyle, SliderStyle, SwitchStyle, assist_chip,
    checkbox, filter_chip, input_chip, radio_button, range_slider, slider, styled_assist_chip,
    styled_checkbox, styled_filter_chip, styled_radio_button, styled_slider, styled_switch,
    suggestion_chip, switch, tri_state_checkbox,
};
use anti_webui_md3e::icons;

pub fn page() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(24.0)
        .child(switches())
        .child(checkboxes())
        .child(radios())
        .child(sliders())
        .child(chips())
}

fn switches() -> Div {
    let flags = [signal(false), signal(true), signal(false), signal(true)];
    let toggle = |on: Signal<bool>| move |_: &PointerEvent| on.update(|v| *v = !*v);
    section("Switch").child(
        row()
            .child(switch(move || flags[0].get()).on_click(toggle(flags[0])))
            .child(switch(move || flags[1].get()).on_click(toggle(flags[1])))
            .child(
                styled_switch(
                    move || flags[2].get(),
                    SwitchStyle::new().on_icon(icons::CHECK),
                )
                .on_click(toggle(flags[2])),
            )
            .child(
                styled_switch(
                    move || flags[3].get(),
                    SwitchStyle::new()
                        .on_icon(icons::CHECK)
                        .off_icon(icons::CLOSE),
                )
                .on_click(toggle(flags[3])),
            )
            .child(styled_switch(
                || false,
                SwitchStyle::new().disabled_when(|| true),
            ))
            .child(styled_switch(
                || true,
                SwitchStyle::new().disabled_when(|| true),
            )),
    )
}

fn checkboxes() -> Div {
    let items = [signal(true), signal(false), signal(true)];
    let all = move || {
        let on = items.iter().filter(|i| i.get()).count();
        match on {
            0 => CheckState::Off,
            3 => CheckState::On,
            _ => CheckState::Indeterminate,
        }
    };
    let error = signal(false);
    section("Checkbox").child(
        row()
            .child(tri_state_checkbox(all).on_click(move |_| {
                let to = untrack(all) != CheckState::On;
                for item in items {
                    item.set(to);
                }
            }))
            .children(items.map(|item| {
                checkbox(move || item.get()).on_click(move |_| item.update(|v| *v = !*v))
            }))
            .child(
                styled_checkbox(
                    move || {
                        if error.get() {
                            CheckState::On
                        } else {
                            CheckState::Off
                        }
                    },
                    CheckboxStyle::new().error_when(|| true),
                )
                .on_click(move |_| error.update(|v| *v = !*v)),
            )
            .child(styled_checkbox(
                || CheckState::On,
                CheckboxStyle::new().disabled_when(|| true),
            ))
            .child(styled_checkbox(
                || CheckState::Off,
                CheckboxStyle::new().disabled_when(|| true),
            )),
    )
}

fn radios() -> Div {
    let chosen = signal(1);
    section("Radio button").child(
        row()
            .children(
                [0, 1, 2].map(|i| {
                    radio_button(move || chosen.get() == i).on_click(move |_| chosen.set(i))
                }),
            )
            .child(styled_radio_button(
                || true,
                RadioStyle::new().disabled_when(|| true),
            ))
            .child(styled_radio_button(
                || false,
                RadioStyle::new().disabled_when(|| true),
            )),
    )
}

fn sliders() -> Div {
    let plain = signal(0.4f32);
    let labelled = signal(30.0f32);
    let stepped = signal(4.0f32);
    let centered = signal(0.2f32);
    let range = signal((20.0f32, 70.0f32));
    let thick = signal(0.6f32);
    let upright = signal(0.5f32);
    section("Slider").child(
        div()
            .flex()
            .flex_row()
            .gap(32.0)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(8.0)
                    .child(slider(move || plain.get(), move |v| plain.set(v)).w(320.0))
                    .child(
                        styled_slider(
                            move || labelled.get(),
                            move |v| labelled.set(v),
                            SliderStyle::new()
                                .range(0.0..=100.0)
                                .label(|v| format!("{v:.0}")),
                        )
                        .w(320.0),
                    )
                    .child(
                        styled_slider(
                            move || stepped.get(),
                            move |v| stepped.set(v),
                            SliderStyle::new().range(0.0..=10.0).steps(9),
                        )
                        .w(320.0),
                    )
                    .child(
                        styled_slider(
                            move || centered.get(),
                            move |v| centered.set(v),
                            SliderStyle::new().range(-1.0..=1.0).centered(),
                        )
                        .w(320.0),
                    )
                    .child(
                        range_slider(
                            move || range.get(),
                            move |v| range.set(v),
                            SliderStyle::new().range(0.0..=100.0),
                        )
                        .w(320.0),
                    )
                    .child(
                        styled_slider(
                            move || thick.get(),
                            move |v| thick.set(v),
                            SliderStyle::new()
                                .track(36.0)
                                .corner(12.0)
                                .handle_height(52.0),
                        )
                        .w(320.0)
                        .h(56.0),
                    )
                    .child(
                        styled_slider(|| 0.3, |_| {}, SliderStyle::new().disabled_when(|| true))
                            .w(320.0),
                    ),
            )
            .child(styled_slider(
                move || upright.get(),
                move |v| upright.set(v),
                SliderStyle::new().vertical(),
            )),
    )
}

fn chips() -> Div {
    let filters = [signal(true), signal(false), signal(false)];
    let people = signal(vec!["Alex", "Sam", "Kim"]);
    section("Chips")
        .child(
            row()
                .child(assist_chip("Assist"))
                .child(styled_assist_chip(
                    "With icon",
                    ChipStyle::new().icon(icons::EVENT),
                ))
                .child(styled_assist_chip(
                    "Elevated",
                    ChipStyle::new().icon(icons::SHARE).elevated(),
                ))
                .child(styled_assist_chip(
                    "Disabled",
                    ChipStyle::new().disabled_when(|| true),
                ))
                .child(suggestion_chip("Suggestion")),
        )
        .child(
            row()
                .children(["Mine", "Shared", "Recent"].into_iter().zip(filters).map(
                    |(label, on)| {
                        filter_chip(label, move || on.get())
                            .on_click(move |_| on.update(|v| *v = !*v))
                    },
                ))
                .child({
                    let on = signal(false);
                    styled_filter_chip(
                        "Favorite",
                        move || on.get(),
                        ChipStyle::new().icon(icons::FAVORITE),
                    )
                    .on_click(move |_| on.update(|v| *v = !*v))
                }),
        )
        .child(
            row().child(
                for_each(
                    move || people.get(),
                    |name| *name,
                    move |name| {
                        input_chip(
                            name,
                            ChipStyle::new()
                                .icon(icons::PERSON)
                                .trailing(icons::CLOSE, move || {
                                    people.update(|p| p.retain(|n| *n != name))
                                }),
                        )
                    },
                )
                .enter(|s| s.opacity(0.0).scale(0.8))
                .exit(|s| s.opacity(0.0).scale(0.8))
                .moves(Transition::ms(200).ease_out()),
            ),
        )
}
