//! Buttons, what is chosen and what is typed.

mod common;

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, NamedKey};
use anti_webui_core::prelude::*;
use anti_webui_widgets::*;
use common::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
    logo: false,
};
const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
    logo: false,
};

fn column_of(child: impl IntoView) -> Div {
    div().flex().flex_col().items_start().p(20.0).child(child)
}

/// The color of the box under a place.
fn color_under(ui: &mut Ui, x: f32, y: f32) -> Option<Color> {
    ui.update();
    let scene = run(ui, 300);
    boxes(&scene)
        .iter()
        .rev()
        .find(|q| {
            q.background.a > 0.0
                && q.rect
                    .contains(anti_webui_core::geometry::PointF::new(x, y))
        })
        .map(|q| q.background)
}

#[test]
fn a_theme_changes_what_is_shown_and_all_of_a_kind_share_a_sheet() {
    set_theme(Theme::light());
    let mut ui = start(|| page().child(column_of(button("One")).child(button("Two"))));
    let (x, y) = middle_of("One");
    assert_eq!(
        color_under(&mut ui, x, y),
        Some(Theme::light().surface_muted.snapped())
    );
    let shared = |text: &str| {
        let id = text_node(text).unwrap();
        anti_webui_core::tree::with_doc(|d| {
            let parent = d.get(id).unwrap().parent.unwrap();
            d.get(parent).unwrap().style.sheet.clone().unwrap()
        })
    };
    assert!(Rc::ptr_eq(&shared("One"), &shared("Two")));
    set_theme(Theme::dark());
    assert_eq!(
        color_under(&mut ui, x, y),
        Some(Theme::dark().surface_muted.snapped())
    );
    assert!(Rc::ptr_eq(&shared("One"), &shared("Two")));
    set_theme(Theme::light());
}

#[test]
fn a_theme_can_be_made() {
    let purple = rgb(0x8250df);
    set_theme(Theme {
        accent: purple,
        ..Theme::light()
    });
    let mut ui = start(|| column_of(primary_button("Go")));
    let (x, y) = middle_of("Go");
    assert_eq!(color_under(&mut ui, x, y), Some(purple.snapped()));
    set_theme(Theme::light());
}

#[test]
fn the_colors_of_a_button_can_be_set_directly_and_stay_in_every_state() {
    set_theme(Theme::light());
    let red = rgb(0xc0392b);
    let mut ui = start(move || column_of(button("Red").bg(red)));
    let (x, y) = middle_of("Red");
    assert_eq!(color_under(&mut ui, x, y), Some(red.snapped()));
    pointer_at(&mut ui, x, y);
    assert_eq!(color_under(&mut ui, x, y), Some(red.snapped()));
    pointer(&mut ui, x, y, true);
    assert_eq!(color_under(&mut ui, x, y), Some(red.snapped()));
}

#[test]
fn a_button_changes_with_the_pointer_and_not_when_disabled() {
    set_theme(Theme::light());
    let t = Theme::light();
    let off = signal(false);
    let clicks = Rc::new(Cell::new(0));
    let count = clicks.clone();
    let mut ui = start(move || {
        column_of(
            button("Press")
                .disabled(move || off.get())
                .on_click(move |_| count.set(count.get() + 1)),
        )
    });
    let (x, y) = middle_of("Press");
    pointer_at(&mut ui, x, y);
    assert_eq!(color_under(&mut ui, x, y), Some(t.hover.snapped()));
    pointer(&mut ui, x, y, true);
    assert_eq!(color_under(&mut ui, x, y), Some(t.pressed.snapped()));
    pointer(&mut ui, x, y, false);
    assert_eq!(clicks.get(), 1);
    off.set(true);
    assert_eq!(color_under(&mut ui, x, y), Some(t.surface_muted.snapped()));
    click(&mut ui, x, y);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn a_button_works_with_the_keyboard() {
    let clicks = Rc::new(Cell::new(0));
    let count = clicks.clone();
    let mut ui =
        start(move || column_of(button("Press").on_click(move |_| count.set(count.get() + 1))));
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::Enter);
    key(&mut ui, NamedKey::Space);
    assert_eq!(clicks.get(), 2);
}

#[test]
fn a_toggle_button_stays_pressed() {
    let on = signal(false);
    let mut ui = start(move || column_of(toggle_button(on, "Bold")));
    click_text(&mut ui, "Bold");
    assert!(on.get());
    click_text(&mut ui, "Bold");
    assert!(!on.get());
}

#[test]
fn a_checkbox_switches() {
    let on = signal(false);
    let mut ui = start(move || column_of(checkbox(on, "Agree")));
    click_text(&mut ui, "Agree");
    assert!(on.get());
    assert!(has_text("✓"));
    key(&mut ui, NamedKey::Space);
    assert!(!on.get());
    assert!(!has_text("✓"));
}

#[test]
fn a_disabled_checkbox_stays_as_it_is() {
    let on = signal(false);
    let mut ui = start(move || column_of(checkbox(on, "Agree").disabled(|| true)));
    click_text(&mut ui, "Agree");
    assert!(!on.get());
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::Space);
    assert!(!on.get());
}

#[test]
fn a_mix_turns_on_and_then_off() {
    let state = signal(CheckState::Mixed);
    let mut ui = start(move || column_of(tri_checkbox(state, "All")));
    click_text(&mut ui, "All");
    assert_eq!(state.get(), CheckState::On);
    click_text(&mut ui, "All");
    assert_eq!(state.get(), CheckState::Off);
    click_text(&mut ui, "All");
    assert_eq!(state.get(), CheckState::On);
}

#[test]
fn a_radio_button_sets_its_value() {
    let chosen = signal(0);
    let mut ui = start(move || column_of(radio(chosen, 1, "One")).child(radio(chosen, 2, "Two")));
    click_text(&mut ui, "Two");
    assert_eq!(chosen.get(), 2);
    click_text(&mut ui, "One");
    assert_eq!(chosen.get(), 1);
}

fn options() -> Vec<(u32, String)> {
    (0..3).map(|i| (i, format!("Option {i}"))).collect()
}

#[test]
fn a_radio_group_is_one_stop_and_goes_with_the_arrows() {
    let chosen = signal(0u32);
    let after = Rc::new(Cell::new(0));
    let count = after.clone();
    let mut ui = start(move || {
        column_of(radio_group(chosen, options()))
            .child(button("After").on_click(move |_| count.set(count.get() + 1)))
    });
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(chosen.get(), 1);
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(chosen.get(), 2);
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(chosen.get(), 0, "it goes around");
    key(&mut ui, NamedKey::ArrowUp);
    assert_eq!(chosen.get(), 2);
    click_text(&mut ui, "Option 1");
    assert_eq!(chosen.get(), 1);
    // One more Tab is what comes after it.
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::Enter);
    assert_eq!(after.get(), 1);
}

#[test]
fn a_switch_moves_its_thumb_over_time() {
    set_theme(Theme::light());
    let on = signal(false);
    let mut ui = start(move || column_of(switch(on)));
    let thumb = |ui: &mut Ui, ms: u64| {
        let scene = run(ui, ms);
        colored(&scene, Color::WHITE)[0].rect.x
    };
    let from = thumb(&mut ui, 8);
    click(&mut ui, 30.0, 30.0);
    assert!(on.get());
    let soon = thumb(&mut ui, 24);
    assert!(soon > from && soon < from + 16, "from {from} to {soon}");
    let then = thumb(&mut ui, 400);
    assert_eq!(then, from + 16);
    assert_at_rest(&mut ui);
}

#[test]
fn a_slider_goes_with_the_pointer_also_outside_of_it() {
    let value = signal(0.0f32);
    let mut ui = start(move || column_of(slider(value, 0.0, 100.0).w(200.0)));
    pointer_at(&mut ui, 120.0, 30.0);
    pointer(&mut ui, 120.0, 30.0, true);
    assert_eq!(value.get(), 50.0);
    pointer_at(&mut ui, 170.0, 30.0);
    assert_eq!(value.get(), 75.0);
    pointer_at(&mut ui, 170.0, 500.0);
    assert_eq!(value.get(), 75.0, "only sideways counts");
    pointer_at(&mut ui, 900.0, 500.0);
    assert_eq!(value.get(), 100.0);
    pointer(&mut ui, 900.0, 500.0, false);
    pointer_at(&mut ui, 20.0, 30.0);
    assert_eq!(
        value.get(),
        100.0,
        "released outside of it, it stops following"
    );
}

#[test]
fn a_slider_goes_with_the_keys() {
    let value = signal(50.0f32);
    let mut ui = start(move || column_of(slider(value, 0.0, 100.0).w(200.0)));
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(value.get(), 55.0);
    key(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(value.get(), 50.0);
    key(&mut ui, NamedKey::PageUp);
    assert!(value.get() > 55.0);
    key(&mut ui, NamedKey::Home);
    assert_eq!(value.get(), 0.0);
    key(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(value.get(), 0.0);
    key(&mut ui, NamedKey::End);
    assert_eq!(value.get(), 100.0);
}

#[test]
fn a_slider_with_steps_stops_on_them() {
    let value = signal(50.0f32);
    let mut ui = start(move || column_of(stepped_slider(value, 0.0, 100.0, 25.0).w(200.0)));
    pointer_at(&mut ui, 90.0, 30.0);
    pointer(&mut ui, 90.0, 30.0, true);
    pointer(&mut ui, 90.0, 30.0, false);
    assert_eq!(value.get(), 25.0);
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(value.get(), 50.0);
}

#[test]
fn a_text_field_takes_what_is_typed() {
    let value = signal(String::new());
    let mut ui = start(move || column_of(text_field(value).w(200.0)));
    key(&mut ui, NamedKey::Tab);
    typed(&mut ui, "hello");
    assert_eq!(value.get(), "hello");
}

#[test]
fn a_disabled_text_field_takes_nothing() {
    let value = signal(String::new());
    let mut ui = start(move || column_of(text_field(value).w(200.0).disabled(|| true)));
    key(&mut ui, NamedKey::Tab);
    click(&mut ui, 60.0, 30.0);
    typed(&mut ui, "hello");
    assert_eq!(value.get(), "");
}

fn start_number(value: Signal<f64>, step: f64) -> Ui {
    start(move || column_of(number_field(value, 0.0, 10.0, step).w(120.0)))
}

/// The text in the field.
fn field_text() -> String {
    anti_webui_core::tree::with_doc(|d| {
        d.nodes
            .iter()
            .find_map(|(_, n)| n.edit().map(|e| e.text.clone()))
            .unwrap_or_default()
    })
}

#[test]
fn a_number_counts_when_it_is_confirmed() {
    let value = signal(1.0f64);
    let mut ui = start_number(value, 1.0);
    assert_eq!(field_text(), "1");
    key(&mut ui, NamedKey::Tab);
    typed(&mut ui, "7");
    assert_eq!(value.get(), 1.0, "not yet");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(value.get(), 7.0);
}

#[test]
fn a_number_is_kept_in_its_range() {
    let value = signal(1.0f64);
    let mut ui = start_number(value, 1.0);
    key(&mut ui, NamedKey::Tab);
    typed(&mut ui, "99");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(value.get(), 10.0);
    assert_eq!(field_text(), "10");
}

#[test]
fn what_is_not_a_number_goes_back() {
    let value = signal(3.0f64);
    let mut ui = start_number(value, 1.0);
    key(&mut ui, NamedKey::Tab);
    typed(&mut ui, "abc");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(value.get(), 3.0);
    assert_eq!(field_text(), "3");
}

#[test]
fn a_number_is_confirmed_when_the_focus_goes() {
    let value = signal(1.0f64);
    let mut ui = start_number(value, 1.0);
    key(&mut ui, NamedKey::Tab);
    typed(&mut ui, "4");
    click(&mut ui, 900.0, 700.0);
    assert_eq!(value.get(), 4.0);
}

#[test]
fn a_number_goes_by_steps_without_collecting_errors() {
    let value = signal(0.0f64);
    let mut ui = start_number(value, 0.1);
    assert_eq!(field_text(), "0.0");
    key(&mut ui, NamedKey::Tab);
    for _ in 0..3 {
        key(&mut ui, NamedKey::ArrowUp);
    }
    assert_eq!(field_text(), "0.3");
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(field_text(), "0.2");
    for _ in 0..48 {
        key(&mut ui, NamedKey::ArrowUp);
    }
    assert_eq!(value.get(), 50.0 * 0.1, "as many steps, and not their sum");
}

#[test]
fn from_between_steps_a_number_goes_to_the_next_one() {
    let value = signal(1.5f64);
    let mut ui = start_number(value, 1.0);
    assert_eq!(field_text(), "1.5", "it shows that it is between them");
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowUp);
    assert_eq!(value.get(), 2.0);
    click(&mut ui, 900.0, 700.0);
    value.set(1.5);
    ui.update();
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(value.get(), 1.0);
}

#[test]
fn the_buttons_of_a_number_go_by_steps() {
    let value = signal(5.0f64);
    let mut ui = start_number(value, 1.0);
    click_text(&mut ui, "▲");
    assert_eq!(value.get(), 6.0);
    assert_eq!(field_text(), "6");
    click_text(&mut ui, "▼");
    click_text(&mut ui, "▼");
    assert_eq!(value.get(), 4.0);
}

#[test]
fn a_number_shows_what_changes_it_from_outside() {
    let value = signal(5.0f64);
    let mut ui = start_number(value, 1.0);
    value.set(8.0);
    ui.update();
    assert_eq!(field_text(), "8");
}

#[test]
fn what_the_buttons_of_a_number_show_stays_inside_the_corners() {
    set_theme(Theme::light());
    let hover = Theme::light().hover.snapped();
    let mut ui = start_number(signal(5.0), 1.0);
    for arrow in ["▲", "▼"] {
        let (x, y) = middle_of(arrow);
        pointer_at(&mut ui, x, y);
        let scene = run(&mut ui, 300);
        let shown = colored(&scene, Theme::light().hover);
        let lit = shown.iter().find(|q| q.background == hover).expect("hover");
        // 6 pixels of the box, less the 1 of its border.
        assert_eq!(lit.clip.radii, [5.0; 4], "{arrow}");
    }
}

#[test]
fn a_disabled_number_stays_as_it_is() {
    let value = signal(5.0f64);
    let mut ui = start(move || {
        column_of(
            number_field(value, 0.0, 10.0, 1.0)
                .w(120.0)
                .disabled(|| true),
        )
    });
    click_text(&mut ui, "▲");
    assert_eq!(value.get(), 5.0);
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowUp);
    assert_eq!(value.get(), 5.0);
}

#[test]
fn a_search_field_empties_with_escape_and_its_button() {
    let value = signal(String::new());
    let mut ui = start(move || column_of(search_field(value, "Search").w(200.0)));
    assert!(!has_text("×"));
    key(&mut ui, NamedKey::Tab);
    typed(&mut ui, "abc");
    assert_eq!(value.get(), "abc");
    assert!(has_text("×"));
    key(&mut ui, NamedKey::Escape);
    assert_eq!(value.get(), "");
    typed(&mut ui, "x");
    click_text(&mut ui, "×");
    assert_eq!(value.get(), "");
    typed(&mut ui, "y");
    assert_eq!(value.get(), "y", "the focus stays in the field");
}

#[test]
fn the_box_of_a_field_is_what_is_styled() {
    set_theme(Theme::light());
    let red = rgb(0xc0392b);
    let mut ui = start(move || {
        column_of(number_field(signal(1.0), 0.0, 10.0, 1.0).w(120.0).bg(red))
            .child(select(signal(0u32), options()).bg(red))
    });
    let (_, y) = middle_of("▲");
    assert_eq!(color_under(&mut ui, 40.0, y), Some(red.snapped()));
    let (x, y) = middle_of("Option 0");
    assert_eq!(color_under(&mut ui, x, y), Some(red.snapped()));
}

#[test]
fn a_list_box_chooses_with_the_pointer_and_the_keys() {
    let value = signal(0u32);
    let mut ui = start(move || column_of(list_box(value, options()).w(200.0)));
    click_text(&mut ui, "Option 2");
    assert_eq!(value.get(), 2);
    key(&mut ui, NamedKey::ArrowUp);
    assert_eq!(value.get(), 1);
    key(&mut ui, NamedKey::Home);
    assert_eq!(value.get(), 0);
    key(&mut ui, NamedKey::End);
    assert_eq!(value.get(), 2);
}

#[test]
fn a_list_of_many_chooses_with_ctrl_and_shift() {
    let values = signal(Vec::<u32>::new());
    let five = || {
        (0..5)
            .map(|i| (i, format!("Option {i}")))
            .collect::<Vec<_>>()
    };
    let mut ui = start(move || column_of(multi_list_box(values, five()).w(200.0)));
    let sorted = || {
        let mut v = values.get();
        v.sort();
        v
    };
    click_text(&mut ui, "Option 1");
    assert_eq!(sorted(), [1]);
    let (x, y) = middle_of("Option 3");
    click_with(&mut ui, x, y, CTRL);
    assert_eq!(sorted(), [1, 3], "ctrl adds");
    click_with(&mut ui, x, y, CTRL);
    assert_eq!(sorted(), [1], "and takes away");
    click_text(&mut ui, "Option 1");
    let (x, y) = middle_of("Option 4");
    click_with(&mut ui, x, y, SHIFT);
    assert_eq!(sorted(), [1, 2, 3, 4], "shift goes from the last");
    click_text(&mut ui, "Option 2");
    assert_eq!(sorted(), [2], "without them it is only that");
}

#[test]
fn a_list_of_many_goes_with_the_keys() {
    let values = signal(Vec::<u32>::new());
    let five = || {
        (0..5)
            .map(|i| (i, format!("Option {i}")))
            .collect::<Vec<_>>()
    };
    let mut ui = start(move || column_of(multi_list_box(values, five()).w(200.0)));
    let sorted = || {
        let mut v = values.get();
        v.sort();
        v
    };
    click_text(&mut ui, "Option 1");
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(sorted(), [2]);
    key_with(&mut ui, NamedKey::ArrowDown, SHIFT);
    assert_eq!(sorted(), [2, 3]);
    key_with(&mut ui, NamedKey::ArrowDown, CTRL);
    assert_eq!(sorted(), [2, 3], "ctrl only moves the cursor");
    key(&mut ui, NamedKey::Space);
    assert_eq!(sorted(), [2, 3, 4], "space switches what the cursor is on");
    key(&mut ui, NamedKey::Space);
    assert_eq!(sorted(), [2, 3]);
}

#[test]
fn pressing_in_the_list_of_a_select_keeps_what_is_lit() {
    set_theme(Theme::light());
    let accent = Theme::light().accent.snapped();
    let value = signal(0u32);
    let mut ui = start(move || column_of(select(value, options()).w(200.0)));
    click_text(&mut ui, "Option 0");
    let shown = popups_shown(&mut ui);
    place(&mut ui, &shown[0]);
    let _ = ui.paint_frame(Instant::now());
    let (x, y) = middle_of("Option 2");
    let lit_at = |ui: &mut Ui| {
        ui.update();
        let frame = ui.paint_frame(Instant::now());
        boxes(&frame.popups[0].1)
            .iter()
            .find(|q| q.background == accent)
            .expect("a row that is lit")
            .rect
            .y
    };
    pointer_at(&mut ui, x, y);
    let hovered = lit_at(&mut ui);
    pointer(&mut ui, x, y, true);
    assert_eq!(lit_at(&mut ui), hovered);
}

#[test]
fn the_cursor_of_a_list_of_many_shows_from_the_keyboard() {
    set_theme(Theme::light());
    let accent = Theme::light().accent.snapped();
    let values = signal(Vec::<u32>::new());
    let mut ui = start(move || column_of(multi_list_box(values, options()).w(200.0)));
    let outlined = |ui: &mut Ui| {
        boxes(&run(ui, 300))
            .iter()
            .filter(|q| q.border_color == accent)
            .count()
    };
    assert_eq!(outlined(&mut ui), 0);
    key(&mut ui, NamedKey::Tab);
    assert_eq!(outlined(&mut ui), 1);
}

#[test]
fn pressing_in_the_list_of_a_select_does_not_open_it_again() {
    let value = signal(0u32);
    let mut ui = start(move || column_of(select(value, options()).w(200.0)));
    click_text(&mut ui, "Option 0");
    let shown = popups_shown(&mut ui);
    assert_eq!(shown.len(), 1);
    place(&mut ui, &shown[0]);
    let _ = ui.paint_frame(Instant::now());
    ui.take_requests();
    // The list repeats the name, and the one in the list comes last.
    let (x, y) = middle_of("Option 2");
    click(&mut ui, x, y);
    assert_eq!(value.get(), 2);
    assert_eq!(popups_closed(&mut ui), 1);
    assert!(popups_shown(&mut ui).is_empty());
}
