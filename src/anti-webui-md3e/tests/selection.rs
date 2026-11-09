//! Switches, checkboxes, radio buttons, sliders and chips. See `docs/design/20-md3e-components.md`.

mod common;

use anti_webui::core::event::NamedKey;
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    CheckState, CheckboxStyle, ChipStyle, RadioStyle, SliderStyle, SwitchStyle, assist_chip,
    checkbox, filter_chip, input_chip, radio_button, range_slider, slider, styled_assist_chip,
    styled_checkbox, styled_radio_button, styled_slider, styled_switch, suggestion_chip, switch,
    tri_state_checkbox,
};
use anti_webui_md3e::icons;
use common::*;

fn handle(scene: &anti_webui::core::scene::Scene, color: Color) -> (i32, i32) {
    let q = colored(scene, color).pop().expect("the handle");
    assert_eq!(q.rect.width, q.rect.height);
    (q.rect.x, q.rect.width)
}

#[test]
fn a_switch_has_the_handle_of_its_state() {
    let on = signal(false);
    let mut ui = start_ui(move || div().child(switch(on)));
    let scene = settle(&mut ui);
    assert_eq!(handle(&scene, C.outline), (8, 16));
    // Pressed it is as big as it gets, wherever it is.
    pointer_at(&mut ui, 20.0, 24.0);
    pointer(&mut ui, 20.0, 24.0, true);
    let scene = settle(&mut ui);
    assert_eq!(handle(&scene, C.outline), (2, 28));
    pointer(&mut ui, 20.0, 24.0, false);
    assert!(on.get());
    let scene = settle(&mut ui);
    assert_eq!(handle(&scene, C.on_primary), (24, 24));
    assert_eq!(colored(&scene, C.primary).len(), 1);
    pointer(&mut ui, 20.0, 24.0, true);
    let scene = settle(&mut ui);
    assert_eq!(handle(&scene, C.on_primary), (22, 28));
}

#[test]
fn an_icon_makes_the_handle_of_a_switch_as_big_as_when_it_is_on() {
    let mut ui = start_ui(|| {
        div().child(styled_switch(
            signal(false),
            SwitchStyle::new().off_icon(icons::CLOSE),
        ))
    });
    let scene = settle(&mut ui);
    assert_eq!(handle(&scene, C.outline), (4, 24));
}

#[test]
fn the_handle_of_a_switch_bounces_on_its_way() {
    let on = signal(false);
    let mut ui = start_ui(move || div().child(switch(on)));
    settle(&mut ui);
    on.set(true);
    let mut furthest = 0;
    watch(&mut ui, 400, |scene| {
        furthest = furthest.max(colored(scene, C.on_primary).pop().map_or(0, |q| q.rect.x));
    });
    assert!(furthest > 24, "went as far as {furthest}");
}

#[test]
fn a_disabled_switch_does_not_react() {
    let clicks = signal(0);
    let on = signal(false);
    let mut ui = start_ui(move || {
        div().child(
            styled_switch(on, SwitchStyle::new().disabled_when(|| true))
                .on_click(move |_| clicks.update(|c| *c += 1)),
        )
    });
    click(&mut ui, 20.0, 24.0);
    assert_eq!(clicks.get(), 0);
    assert!(!on.get());
}

#[test]
fn a_checkbox_shows_a_mark_only_while_it_is_on() {
    let state = signal(CheckState::Off);
    let mut ui = start_ui(move || div().child(tri_state_checkbox(state)));
    let scene = settle(&mut ui);
    assert!(shaders(&scene).is_empty());
    let boxed = boxes(&scene).pop().expect("the box");
    assert_eq!((boxed.rect.width, boxed.rect.height), (20, 20));
    assert_eq!(boxed.border_color, C.on_surface_variant.snapped());
    state.set(CheckState::On);
    let scene = settle(&mut ui);
    assert_eq!(shaders(&scene).len(), 1);
    assert_eq!(colored(&scene, C.primary).len(), 1);
    state.set(CheckState::Mixed);
    let scene = settle(&mut ui);
    assert_eq!(shaders(&scene).len(), 1);
    state.set(CheckState::Off);
    let scene = settle(&mut ui);
    assert!(shaders(&scene).is_empty());
}

#[test]
fn a_checkbox_has_the_colors_of_its_state() {
    let mut ui = start_ui(|| {
        div()
            .child(styled_checkbox(
                signal(true),
                CheckboxStyle::new().error_when(|| true),
            ))
            .child(styled_checkbox(
                signal(false),
                CheckboxStyle::new().disabled_when(|| true),
            ))
    });
    let scene = settle(&mut ui);
    assert_eq!(colored(&scene, C.error).len(), 1);
    let faded = boxes(&scene).pop().expect("the disabled box");
    assert_eq!(faded.border_color, C.on_surface.with_alpha(0.38).snapped());
}

#[test]
fn clicking_a_checkbox_or_a_radio_button_changes_what_it_is_bound_to() {
    let checked = signal(false);
    let chosen = signal(0);
    let mut ui = start_ui(move || {
        div()
            .flex()
            .flex_row()
            .child(checkbox(checked))
            .child(radio_button(chosen, 1))
    });
    click(&mut ui, 24.0, 24.0);
    assert!(checked.get());
    click(&mut ui, 72.0, 24.0);
    assert_eq!(chosen.get(), 1);
    click(&mut ui, 24.0, 24.0);
    assert!(!checked.get(), "and back");
}

#[test]
fn a_radio_button_has_a_dot_while_selected() {
    let on = signal(false);
    let mut ui = start_ui(move || div().child(radio_button(on, true)));
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.primary).is_empty());
    on.set(true);
    let scene = settle(&mut ui);
    let dot = colored(&scene, C.primary).pop().expect("the dot");
    assert_eq!((dot.rect.width, dot.rect.height), (10, 10));
    // In the middle of the 48 box.
    assert_eq!((dot.rect.x, dot.rect.y), (19, 19));
}

#[test]
fn a_disabled_radio_button_is_faded() {
    let mut ui = start_ui(|| {
        div().child(styled_radio_button(
            signal(1),
            1,
            RadioStyle::new().disabled_when(|| true),
        ))
    });
    let scene = settle(&mut ui);
    let faded = C.on_surface.with_alpha(0.38);
    assert_eq!(colored(&scene, faded).len(), 1);
}

#[test]
fn a_slider_follows_the_pointer_and_the_keys() {
    let value = signal(0.5f32);
    let mut ui = start_ui(move || div().child(slider(value)));
    // The track is 236 from 2 to 238.
    pointer_at(&mut ui, 61.0, 24.0);
    pointer(&mut ui, 61.0, 24.0, true);
    assert!((value.get() - 0.25).abs() < 0.001, "{}", value.get());
    pointer_at(&mut ui, 179.0, 24.0);
    assert!((value.get() - 0.75).abs() < 0.001, "{}", value.get());
    // Beyond the ends it stays at the ends.
    pointer_at(&mut ui, 900.0, 24.0);
    assert_eq!(value.get(), 1.0);
    pointer(&mut ui, 900.0, 24.0, false);
    pointer_at(&mut ui, 61.0, 24.0);
    assert_eq!(value.get(), 1.0, "it only follows while it is held");
    key(&mut ui, NamedKey::ArrowLeft);
    assert!((value.get() - 0.95).abs() < 0.001);
    key(&mut ui, NamedKey::Home);
    assert_eq!(value.get(), 0.0);
    key(&mut ui, NamedKey::End);
    assert_eq!(value.get(), 1.0);
}

#[test]
fn a_slider_snaps_to_its_steps_and_stays_in_its_range() {
    let value = signal(0.0f32);
    let mut ui = start_ui(move || {
        div().child(styled_slider(
            value,
            SliderStyle::new().range(10.0..=20.0).steps(4),
        ))
    });
    pointer_at(&mut ui, 110.0, 24.0);
    pointer(&mut ui, 110.0, 24.0, true);
    pointer(&mut ui, 110.0, 24.0, false);
    assert_eq!(value.get(), 14.0);
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(value.get(), 16.0);
    key(&mut ui, NamedKey::End);
    assert_eq!(value.get(), 20.0);
}

#[test]
fn a_range_slider_moves_the_closest_handle_and_never_past_the_other() {
    let (start, end) = (signal(20.0f32), signal(80.0f32));
    let values = move || (start.get(), end.get());
    let mut ui = start_ui(move || {
        div().child(range_slider(
            start,
            end,
            SliderStyle::new().range(0.0..=100.0),
        ))
    });
    pointer_at(&mut ui, 61.0, 24.0);
    pointer(&mut ui, 61.0, 24.0, true);
    assert_eq!(values().1, 80.0);
    assert!((values().0 - 25.0).abs() < 0.01);
    pointer_at(&mut ui, 220.0, 24.0);
    assert_eq!(values().0, 80.0, "up to the other and no further");
    pointer(&mut ui, 220.0, 24.0, false);
    pointer_at(&mut ui, 200.0, 24.0);
    pointer(&mut ui, 200.0, 24.0, true);
    assert!(values().1 > 80.0);
    assert_eq!(values().0, 80.0);
}

#[test]
fn a_vertical_slider_goes_up() {
    let value = signal(0.5f32);
    let mut ui = start_ui(move || div().child(styled_slider(value, SliderStyle::new().vertical())));
    pointer_at(&mut ui, 24.0, 61.0);
    pointer(&mut ui, 24.0, 61.0, true);
    assert!((value.get() - 0.75).abs() < 0.001, "{}", value.get());
}

#[test]
fn a_disabled_slider_does_not_change() {
    let value = signal(0.5f32);
    let mut ui = start_ui(move || {
        div().child(styled_slider(
            value,
            SliderStyle::new().disabled_when(|| true),
        ))
    });
    pointer_at(&mut ui, 61.0, 24.0);
    pointer(&mut ui, 61.0, 24.0, true);
    assert_eq!(value.get(), 0.5);
}

#[test]
fn the_label_of_a_slider_shows_while_it_is_held() {
    let mut ui = start_ui(|| {
        div().pt(100.0).child(styled_slider(
            signal(50.0),
            SliderStyle::new()
                .range(0.0..=100.0)
                .label(|v| format!("{v:.0}")),
        ))
    });
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.inverse_surface).is_empty());
    pointer_at(&mut ui, 120.0, 124.0);
    pointer(&mut ui, 120.0, 124.0, true);
    let scene = settle(&mut ui);
    let bubble = colored(&scene, C.inverse_surface).pop().expect("the label");
    assert_eq!(bubble.rect.height, 24);
    // Centered above the handle, 4 away from it.
    assert_eq!(bubble.rect.x + bubble.rect.width / 2, 120);
    assert_eq!(bubble.rect.bottom(), 100 + 2 - 4);
    pointer(&mut ui, 120.0, 124.0, false);
    pointer_at(&mut ui, 900.0, 700.0);
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.inverse_surface).is_empty());
}

fn chip_rect(view: impl FnOnce() -> Div + 'static) -> anti_webui::core::geometry::RectI {
    let place = node_ref();
    let mut ui = start_ui(move || div().flex().child(view().node_ref(place)));
    settle(&mut ui);
    place.rect().expect("laid out")
}

#[test]
fn chips_have_the_room_of_compose() {
    let plain = chip_rect(|| assist_chip("Word"));
    assert_eq!(plain.height, 32);
    // 8, 18, 8 before the label and 16 after it, against 16 on both sides.
    let with_icon = chip_rect(|| styled_assist_chip("Word", ChipStyle::new().icon(icons::STAR)));
    assert_eq!(with_icon.width - plain.width, 18);
    assert_eq!(chip_rect(|| suggestion_chip("Word")).width, plain.width);
}

#[test]
fn a_filter_chip_grows_a_check_while_selected() {
    let on = signal(false);
    let place = node_ref();
    let mut ui = start_ui(move || div().flex().child(filter_chip("Word", on).node_ref(place)));
    settle(&mut ui);
    let before = place.rect().unwrap().width;
    on.set(true);
    let scene = settle(&mut ui);
    assert_eq!(place.rect().unwrap().width - before, 14);
    let chip = colored(&scene, C.secondary_container)
        .pop()
        .expect("selected");
    assert_eq!(chip.radii, [16.0; 4], "selected it is a pill");
    on.set(false);
    let scene = settle(&mut ui);
    assert_eq!(place.rect().unwrap().width, before);
    let outlined = boxes(&scene).pop().expect("the chip");
    assert_eq!(outlined.radii, [12.0; 4]);
    assert_eq!(outlined.border_color, C.outline_variant.snapped());
}

#[test]
fn the_trailing_icon_of_an_input_chip_does_its_own_thing() {
    let removed = signal(0);
    let clicked = signal(0);
    let place = node_ref();
    let mut ui = start_ui(move || {
        div().flex().child(
            input_chip(
                "Word",
                ChipStyle::new().trailing(icons::CLOSE, move || removed.update(|c| *c += 1)),
            )
            .node_ref(place)
            .on_click(move |_| clicked.update(|c| *c += 1)),
        )
    });
    settle(&mut ui);
    let r = place.rect().unwrap();
    click(&mut ui, (r.right() - 17) as f32, 16.0);
    assert_eq!((removed.get(), clicked.get()), (1, 0));
    click(&mut ui, 20.0, 16.0);
    assert_eq!((removed.get(), clicked.get()), (1, 1));
}

#[test]
fn nothing_here_asks_for_frames_at_rest() {
    let mut ui = start_ui(|| {
        div()
            .child(switch(signal(true)))
            .child(checkbox(signal(true)))
            .child(radio_button(signal(true), true))
            .child(slider(signal(0.5)))
            .child(filter_chip("Word", signal(true)))
    });
    assert_at_rest(&mut ui);
}

#[test]
fn a_slider_goes_by_pages_too() {
    let value = signal(0.5f32);
    let mut ui = start_ui(move || div().child(slider(value)));
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::PageUp);
    assert!((value.get() - 0.75).abs() < 0.001, "{}", value.get());
    key(&mut ui, NamedKey::PageDown);
    assert!((value.get() - 0.5).abs() < 0.001, "{}", value.get());
}

#[test]
fn a_wavy_slider_flows_only_while_the_pointer_is_on_it() {
    let mut ui = start_ui(|| div().child(styled_slider(signal(0.6), SliderStyle::new().wavy())));
    assert_at_rest(&mut ui);
    pointer_at(&mut ui, 100.0, 24.0);
    settle(&mut ui);
    assert!(ui.animating(), "the wave goes on while the pointer is on it");
    pointer_at(&mut ui, 600.0, 400.0);
    assert_at_rest(&mut ui);
}
