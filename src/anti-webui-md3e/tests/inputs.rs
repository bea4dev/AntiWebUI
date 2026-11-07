//! Text fields, search, menus, FAB menus, split buttons and pickers. See
//! `docs/design/20-md3e-components.md`.

mod common;

use anti_webui::core::event::{NamedKey, WindowEvent};
use anti_webui::core::geometry::PointI;
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonKind, Date, DatePickerStyle, SearchStyle, SplitButtonStyle, TextFieldStyle,
    TimePickerStyle, date_picker, dropdown_field, fab_menu, fab_menu_item, menu, menu_item,
    outlined_text_field, search_bar, split_button, text_field, time_picker,
};
use anti_webui_md3e::icons;
use common::*;

/// The tallest glyphs of a color are in the text that has it: where they stand and how big they are.
fn label_of(scene: &anti_webui::core::scene::Scene, color: Color) -> (f32, f32) {
    let run = glyphs(scene)
        .into_iter()
        .find(|g| g.color == color.snapped())
        .expect("the label");
    (run.glyphs[0].y, run.size)
}

#[test]
fn the_label_of_a_field_floats_when_it_gets_the_focus() {
    let value = signal(String::new());
    let mut ui =
        start_ui(move || div().child(text_field(value, TextFieldStyle::new().label("Name"))));
    let scene = settle(&mut ui);
    let (y_rest, size_rest) = label_of(&scene, C.on_surface_variant);
    assert_eq!(size_rest, 16.0);
    let line = colored(&scene, C.on_surface_variant)
        .pop()
        .expect("the line");
    assert_eq!(
        (line.rect.width, line.rect.height, line.rect.bottom()),
        (280, 1, 56)
    );
    assert_eq!(
        colored(&scene, C.surface_container_highest)
            .pop()
            .unwrap()
            .radii,
        [4.0, 4.0, 0.0, 0.0]
    );

    click(&mut ui, 100.0, 28.0);
    let scene = settle(&mut ui);
    let (y_up, size_up) = label_of(&scene, C.primary);
    assert_eq!(size_up, 12.0);
    assert!(y_up < y_rest, "{y_up} {y_rest}");
    let line = colored(&scene, C.primary).pop().expect("the line");
    assert_eq!(line.rect.height, 2, "thicker with the focus");
}

#[test]
fn a_field_with_text_keeps_its_label_up_without_the_focus() {
    let value = signal("Text".to_string());
    let mut ui =
        start_ui(move || div().child(text_field(value, TextFieldStyle::new().label("Name"))));
    let scene = settle(&mut ui);
    assert_eq!(label_of(&scene, C.on_surface_variant).1, 12.0);
    value.set(String::new());
    let scene = settle(&mut ui);
    assert_eq!(label_of(&scene, C.on_surface_variant).1, 16.0);
}

#[test]
fn a_field_in_error_says_so() {
    let value = signal(String::new());
    let mut ui = start_ui(move || {
        div().child(text_field(
            value,
            TextFieldStyle::new()
                .label("Name")
                .error_when(|| true)
                .supporting(|| "Wrong".to_string()),
        ))
    });
    let scene = settle(&mut ui);
    assert_eq!(
        colored(&scene, C.error)
            .pop()
            .expect("the line")
            .rect
            .height,
        1
    );
    let errors = glyphs(&scene)
        .into_iter()
        .filter(|g| g.color == C.error.snapped())
        .count();
    assert_eq!(errors, 2, "the label and the line below");
}

#[test]
fn typing_goes_to_the_value() {
    let value = signal(String::new());
    let mut ui = start_ui(move || {
        div().child(outlined_text_field(
            value,
            TextFieldStyle::new().label("Name"),
        ))
    });
    click(&mut ui, 100.0, 28.0);
    ui.handle_event(WindowEvent::Key(anti_webui::core::event::KeyEvent {
        key: Key::Character("a".to_string()),
        text: Some("a".to_string()),
        modifiers: Default::default(),
        pressed: true,
        repeat: false,
    }));
    ui.update();
    assert_eq!(value.get(), "a");
}

#[test]
fn an_outlined_field_draws_its_outline_with_a_shader_and_has_no_background() {
    let value = signal(String::new());
    let mut ui = start_ui(move || {
        div().child(outlined_text_field(
            value,
            TextFieldStyle::new().label("Name"),
        ))
    });
    let scene = settle(&mut ui);
    let outline = shaders(&scene).pop().expect("the outline");
    assert_eq!((outline.rect.width, outline.rect.height), (280, 56));
    assert!(colored(&scene, C.surface_container_highest).is_empty());
}

#[test]
fn a_dropdown_chooses_with_a_menu() {
    let value = signal("One".to_string());
    let mut ui = start_ui(move || {
        div().child(dropdown_field(
            value,
            vec!["One".to_string(), "Two".to_string()],
            TextFieldStyle::new().label("Number"),
            false,
        ))
    });
    settle(&mut ui);
    popups_shown(&mut ui);
    click(&mut ui, 100.0, 28.0);
    let shown = popups_shown(&mut ui);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].kind, PopupKind::Menu);
}

fn open_menu(ui: &mut anti_webui::core::Ui) -> anti_webui::core::platform::PopupParams {
    let shown = popups_shown(ui);
    let p = shown.first().cloned().expect("the menu");
    ui.handle_event(WindowEvent::PopupConfigured {
        id: p.id,
        origin: PointI::new(0, 100),
        size: p.wanted,
    });
    ui.update();
    p
}

#[test]
fn a_menu_opens_runs_what_is_chosen_and_closes() {
    let open = signal(false);
    let done = signal(0);
    let mut ui = start_ui(move || {
        div().w(100.0).h(40.0).child(menu(
            move || open.get(),
            move || open.set(false),
            vec![vec![
                menu_item("One").on_click(move || {
                    done.update(|d| *d += 1);
                    open.set(false);
                }),
                menu_item("Two"),
            ]],
        ))
    });
    settle(&mut ui);
    open.set(true);
    let params = open_menu(&mut ui);
    assert!(params.wanted.width >= 112);
    let frame = ui.paint_frame(std::time::Instant::now());
    assert_eq!(frame.popups.len(), 1);
    settle(&mut ui);
    // The first item is 2 from the top of the group, 44 high.
    let top = 100.0 + params.geometry.y as f32;
    click(&mut ui, params.geometry.x as f32 + 30.0, top + 24.0);
    assert_eq!(done.get(), 1);
    assert!(!open.get());
    let closing = |r: &anti_webui::core::platform::PlatformRequest| {
        matches!(
            r,
            anti_webui::core::platform::PlatformRequest::ClosePopup(_)
        )
    };
    assert!(
        !ui.take_requests().iter().any(closing),
        "not before it faded"
    );
    settle(&mut ui);
    let closes = ui.take_requests();
    assert!(
        closes.iter().any(|r| matches!(
            r,
            anti_webui::core::platform::PlatformRequest::ClosePopup(_)
        )),
        "it goes after fading"
    );
}

#[test]
fn a_menu_can_open_again_after_a_press_outside() {
    let open = signal(false);
    let mut ui = start_ui(move || {
        div().size_full().child(div().w(100.0).h(40.0).child(menu(
            move || open.get(),
            move || open.set(false),
            vec![vec![menu_item("One")]],
        )))
    });
    settle(&mut ui);
    open.set(true);
    open_menu(&mut ui);
    settle(&mut ui);
    click(&mut ui, 900.0, 700.0);
    assert!(!open.get());
    // Right away, while it would still be fading.
    open.set(true);
    assert_eq!(popups_shown(&mut ui).len(), 1, "built again");
}

#[test]
fn escape_closes_a_menu() {
    let open = signal(false);
    let mut ui = start_ui(move || {
        div().w(100.0).h(40.0).child(menu(
            move || open.get(),
            move || open.set(false),
            vec![vec![menu_item("One")]],
        ))
    });
    settle(&mut ui);
    open.set(true);
    open_menu(&mut ui);
    key(&mut ui, NamedKey::Escape);
    assert!(!open.get());
}

#[test]
fn a_fab_menu_opens_from_the_item_closest_to_it() {
    let open = signal(false);
    let chosen = signal(0);
    let mut ui = start_ui(move || {
        div().p(300.0).child(fab_menu(
            icons::ADD,
            move || open.get(),
            move || open.update(|o| *o = !*o),
            vec![
                fab_menu_item(icons::IMAGE, "Far", || {}),
                fab_menu_item(icons::VIDEOCAM, "Near", move || chosen.set(1)),
            ],
        ))
    });
    let toggle = |scene: &anti_webui::core::scene::Scene| {
        boxes(scene)
            .into_iter()
            .find(|q| q.rect.height == 56 && q.rect.width == 56)
    };
    let scene = settle(&mut ui);
    let button = toggle(&scene).expect("the button");
    assert_eq!(button.radii, [16.0; 4]);
    assert_eq!(button.background, C.primary_container.snapped());
    click(&mut ui, 328.0, 328.0);
    assert!(open.get());
    let mut order = Vec::new();
    watch(&mut ui, 500, |scene| {
        let wide: Vec<_> = all_boxes(scene)
            .into_iter()
            .filter(|q| q.rect.y < 300 && q.rect.width > 0 && q.rect.height == 56)
            .map(|q| q.rect.y)
            .collect();
        if order.last() != Some(&wide) {
            order.push(wide);
        }
    });
    let first_shown = order.iter().find(|w| !w.is_empty()).expect("items");
    assert_eq!(first_shown.len(), 1, "one at a time: {order:?}");
    assert!(first_shown[0] > 200, "the closest first: {order:?}");
    let scene = settle(&mut ui);
    let button = toggle(&scene).expect("the button");
    assert_eq!(button.radii, [28.0; 4]);
    assert_eq!(button.background, C.primary.snapped());
    assert_at_rest(&mut ui);
}

#[test]
fn clicking_an_item_of_a_fab_menu_does_not_toggle_it() {
    let open = signal(true);
    let chosen = signal(0);
    let mut ui = start_ui(move || {
        div().p(300.0).child(fab_menu(
            icons::ADD,
            move || open.get(),
            move || open.update(|o| *o = !*o),
            vec![fab_menu_item(icons::IMAGE, "Item", move || chosen.set(1))],
        ))
    });
    let scene = settle(&mut ui);
    let item = colored(&scene, C.primary_container)
        .pop()
        .expect("the item");
    click(
        &mut ui,
        (item.rect.right() - 20) as f32,
        (item.rect.y + 28) as f32,
    );
    assert_eq!(chosen.get(), 1);
    assert!(open.get());
}

#[test]
fn the_parts_of_a_split_button_do_different_things() {
    let main = signal(0);
    let more = signal(0);
    let open = signal(false);
    let mut ui = start_ui(move || {
        div().flex().child(split_button(
            "Send",
            move || open.get(),
            move || main.update(|m| *m += 1),
            move || more.update(|m| *m += 1),
            SplitButtonStyle::new(ButtonKind::Filled),
        ))
    });
    let scene = settle(&mut ui);
    let parts = colored(&scene, C.primary);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[1].rect.x - parts[0].rect.right(), 2);
    assert_eq!(parts[0].radii, [20.0, 4.0, 4.0, 20.0]);
    assert_eq!(parts[1].radii, [4.0, 20.0, 20.0, 4.0]);
    click(&mut ui, (parts[0].rect.x + 10) as f32, 20.0);
    assert_eq!((main.get(), more.get()), (1, 0));
    click(&mut ui, (parts[1].rect.x + 10) as f32, 20.0);
    assert_eq!((main.get(), more.get()), (1, 1));
    open.set(true);
    let scene = settle(&mut ui);
    let trailing = colored(&scene, C.primary).pop().unwrap();
    let full = trailing.rect.width.min(trailing.rect.height) as f32 / 2.0;
    assert!(
        trailing.radii[0] >= full,
        "round while open: {:?}",
        trailing.radii
    );
}

#[test]
fn a_search_bar_opens_over_the_page_in_place_and_goes_back() {
    let value = signal(String::new());
    let open = signal(false);
    let mut ui = start_ui(move || {
        div().p(50.0).child(search_bar(
            value,
            move || open.get(),
            move |v| open.set(v),
            || div().h(300.0),
            SearchStyle::new().placeholder("Search").width(360.0),
        ))
    });
    let scene = settle(&mut ui);
    let pill = colored(&scene, C.surface_container_high)
        .pop()
        .expect("the bar");
    assert_eq!((pill.rect.x, pill.rect.y, pill.rect.height), (50, 50, 56));
    click(&mut ui, 200.0, 78.0);
    assert!(open.get());
    let scene = settle(&mut ui);
    let panel = colored(&scene, C.surface_container_high)
        .pop()
        .expect("the panel");
    assert_eq!(
        (panel.rect.x, panel.rect.y, panel.rect.width),
        (50, 50, 360)
    );
    assert_eq!(panel.rect.height, 56 + 1 + 300);
    // The focus is in its field.
    ui.handle_event(WindowEvent::Key(anti_webui::core::event::KeyEvent {
        key: Key::Character("x".to_string()),
        text: Some("x".to_string()),
        modifiers: Default::default(),
        pressed: true,
        repeat: false,
    }));
    ui.update();
    assert_eq!(value.get(), "x");
    key(&mut ui, NamedKey::Escape);
    assert!(!open.get());
    let scene = settle(&mut ui);
    let pill = colored(&scene, C.surface_container_high);
    assert_eq!(pill.len(), 1, "only the bar is left");
    assert_at_rest(&mut ui);
}

#[test]
fn a_full_screen_search_covers_the_window() {
    let value = signal(String::new());
    let open = signal(true);
    let mut ui = start_ui(move || {
        div().p(50.0).child(search_bar(
            value,
            move || open.get(),
            move |v| open.set(v),
            || div().h(300.0),
            SearchStyle::new().width(360.0).full_screen(),
        ))
    });
    let scene = settle(&mut ui);
    let panel = colored(&scene, C.surface_container_low)
        .pop()
        .expect("the panel");
    assert_eq!(
        (
            panel.rect.x,
            panel.rect.y,
            panel.rect.width,
            panel.rect.height
        ),
        (0, 0, 1000, 800)
    );
    assert_eq!(panel.radii, [0.0; 4]);
}

#[test]
fn dates_are_counted_right() {
    assert_eq!(Date::new(2026, 11, 4).weekday(), 3);
    assert_eq!(Date::new(2000, 1, 1).weekday(), 6);
    assert_eq!(Date::new(1970, 1, 1).weekday(), 4);
    assert_eq!(Date::new(2024, 2, 29).weekday(), 4);
}

#[test]
fn a_date_picker_chooses_a_day_and_moves_between_months() {
    let date = signal(Some(Date::new(2024, 2, 10)));
    let mut ui = start_ui(move || {
        div().child(date_picker(
            move || date.get(),
            move |d| date.set(Some(d)),
            DatePickerStyle::new(),
        ))
    });
    let scene = settle(&mut ui);
    let chosen = colored(&scene, C.primary).pop().expect("the selected day");
    assert_eq!((chosen.rect.width, chosen.rect.height), (40, 40));
    // February 2024 starts on a Thursday, and the 10th is a Saturday in the second row.
    let (x, y) = (chosen.rect.x + 20, chosen.rect.y + 20);
    assert_eq!(x, 12 + 6 * 48 + 24);
    click(&mut ui, (x - 48) as f32, y as f32);
    assert_eq!(date.get(), Some(Date::new(2024, 2, 9)));
    let scene = settle(&mut ui);
    let chosen = colored(&scene, C.primary).pop().expect("the selected day");
    assert_eq!(chosen.rect.x + 20, x - 48);
    // The arrow to the next month is on the right of its row.
    let row_y = 120 + 1 + 28;
    click(&mut ui, 360.0 - 12.0 - 20.0, row_y as f32);
    let mut moving = 0;
    watch(&mut ui, 300, |scene| {
        moving = moving.max(glyphs(scene).len());
    });
    assert_at_rest(&mut ui);
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.primary).is_empty(), "another month");
    assert!(moving > glyphs(&scene).len(), "both months on the way");
}

#[test]
fn a_time_picker_turns_its_hand_and_goes_on_to_the_minutes() {
    let time = signal((9u32, 41u32));
    let mut ui = start_ui(move || {
        div().flex().child(time_picker(
            move || time.get(),
            move |t| time.set(t),
            TimePickerStyle::new().twenty_four_hours(),
        ))
    });
    settle(&mut ui);
    // The dial is 256, 80 + 36 from the top, in the middle of the picker.
    let dial = (112.0 + 12.0, 116.0 + 128.0);
    let center_x = 216.0 / 2.0;
    let _ = dial;
    let (cx, cy) = (center_x, 116.0 + 128.0);
    // 3 o'clock on the outside.
    click(&mut ui, cx + 101.0, cy);
    assert_eq!(time.get().0, 3);
    // Now it goes on to the minutes, after a moment.
    wait(&mut ui, 500);
    settle(&mut ui);
    click(&mut ui, cx, cy + 101.0);
    assert_eq!(time.get(), (3, 30));
    assert_at_rest(&mut ui);
}

#[test]
fn a_time_picker_has_am_and_pm_unless_it_counts_to_24() {
    let time = signal((9u32, 0u32));
    let mut ui = start_ui(move || {
        div().flex().child(time_picker(
            move || time.get(),
            move |t| time.set(t),
            TimePickerStyle::new(),
        ))
    });
    let scene = settle(&mut ui);
    // AM is selected: 96 + 24 + 96 + 12 from the left.
    let selected = colored(&scene, C.primary_container);
    assert_eq!(selected.len(), 2, "the hour and AM");
    click(&mut ui, 96.0 + 24.0 + 96.0 + 12.0 + 26.0, 60.0);
    assert_eq!(time.get(), (21, 0));
}

#[test]
fn the_items_of_a_fab_menu_open_to_the_width_of_their_labels() {
    let open = signal(false);
    let closing = signal(0);
    let mut ui = start_ui(move || {
        div().p(300.0).child(fab_menu(
            icons::ADD,
            move || open.get(),
            || {},
            vec![fab_menu_item(
                icons::IMAGE,
                "A label with several words",
                move || closing.set(1),
            )],
        ))
    });
    settle(&mut ui);
    open.set(true);
    let mut widths = Vec::new();
    watch(&mut ui, 800, |scene| {
        widths.extend(
            all_boxes(scene)
                .iter()
                .filter(|q| q.rect.y < 300 && q.rect.height == 56)
                .map(|q| q.rect.width),
        );
    });
    let full = *widths.last().expect("the item");
    assert!(full > 150, "all of the label on one line: {full}");
    let step = widths
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .max()
        .unwrap();
    assert!(step < full / 3, "in one movement: {widths:?}");
}

#[test]
fn the_keys_go_through_a_menu_past_what_is_disabled_and_choosing_closes_it() {
    let open = signal(false);
    let chosen = signal("");
    let mut ui = start_ui(move || {
        div().w(100.0).h(40.0).child(menu(
            move || open.get(),
            move || open.set(false),
            vec![vec![
                menu_item("One").on_click(move || chosen.set("one")),
                menu_item("Off")
                    .disabled_when(|| true)
                    .on_click(move || chosen.set("off")),
                menu_item("Three").on_click(move || chosen.set("three")),
            ]],
        ))
    });
    settle(&mut ui);
    open.set(true);
    open_menu(&mut ui);
    settle(&mut ui);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::Enter);
    assert_eq!(chosen.get(), "three", "past what is disabled");
    assert!(!open.get(), "choosing closes it");
}
