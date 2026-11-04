//! What shows above the rest: tooltips, toasts, dialogs and menus.

mod common;

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, NamedKey, PointerButton};
use anti_webui_core::prelude::*;
use anti_webui_widgets::*;
use common::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn column_of(child: impl IntoView) -> Div {
    div().flex().flex_col().items_start().p(20.0).child(child)
}

#[test]
fn a_tooltip_waits_and_goes_with_the_pointer() {
    let mut ui = start(|| column_of(tooltip(button("Target"), "Says this")));
    assert!(!has_text("Says this"));
    let (x, y) = middle_of("Target");
    pointer_at(&mut ui, x, y);
    assert!(!has_text("Says this"), "not at once");
    wait(&mut ui, 600);
    assert!(has_text("Says this"));
    pointer_at(&mut ui, 900.0, 700.0);
    assert!(!has_text("Says this"));
}

#[test]
fn a_tooltip_does_not_come_when_the_pointer_left_in_time() {
    let mut ui = start(|| column_of(tooltip(button("Target"), "Says this")));
    let (x, y) = middle_of("Target");
    pointer_at(&mut ui, x, y);
    pointer_at(&mut ui, 900.0, 700.0);
    wait(&mut ui, 600);
    assert!(!has_text("Says this"));
}

#[test]
fn a_tooltip_goes_when_its_target_is_pressed() {
    let mut ui = start(|| column_of(tooltip(button("Target"), "Says this")));
    let (x, y) = middle_of("Target");
    pointer_at(&mut ui, x, y);
    wait(&mut ui, 600);
    pointer(&mut ui, x, y, true);
    assert!(!has_text("Says this"));
}

#[test]
fn a_tooltip_comes_with_the_focus_from_the_keyboard() {
    let mut ui = start(|| column_of(tooltip(button("Target"), "Says this")).child(button("Next")));
    key(&mut ui, NamedKey::Tab);
    assert!(has_text("Says this"));
    key(&mut ui, NamedKey::Tab);
    assert!(!has_text("Says this"));
}

#[test]
fn a_toast_shows_and_goes_by_itself() {
    let mut ui = start(|| page().child(div()));
    toast("Saved");
    ui.update();
    assert!(has_text("Saved"));
    wait(&mut ui, 6000);
    run(&mut ui, 400);
    assert!(!has_text("Saved"));
}

#[test]
fn a_toast_can_be_seen_after_it_came_in() {
    let mut ui = start(|| page().child(div()));
    toast("Saved");
    ui.update();
    let scene = run(&mut ui, 300);
    let t = theme();
    let shown = boxes(&scene)
        .into_iter()
        .any(|q| q.background == t.overlay.snapped());
    assert!(shown, "no box of a toast");
    let (x, y) = middle_of("Saved");
    assert!(x > 500.0 && y > 600.0, "at the lower right, at {x},{y}");
}

#[test]
fn the_pointer_on_a_toast_makes_it_wait() {
    let mut ui = start(|| page().child(div()));
    toast("Saved");
    ui.update();
    run(&mut ui, 300);
    let (x, y) = middle_of("Saved");
    pointer_at(&mut ui, x, y);
    wait(&mut ui, 6000);
    run(&mut ui, 400);
    assert!(has_text("Saved"));
    pointer_at(&mut ui, 10.0, 10.0);
    wait(&mut ui, 6000);
    run(&mut ui, 400);
    assert!(!has_text("Saved"));
}

#[test]
fn a_toast_with_an_action_stays_until_it_is_used() {
    let done = Rc::new(Cell::new(false));
    let mut ui = start(|| page().child(div()));
    let flag = done.clone();
    toast_with(Toast::new("Moved").action("Undo", move || flag.set(true)));
    ui.update();
    run(&mut ui, 300);
    wait(&mut ui, 60_000);
    assert!(has_text("Moved"));
    click_text(&mut ui, "Undo");
    assert!(done.get());
    run(&mut ui, 400);
    assert!(!has_text("Moved"));
}

#[test]
fn a_toast_can_be_closed_and_lets_the_pointer_through_elsewhere() {
    let clicks = Rc::new(Cell::new(0));
    let count = clicks.clone();
    let mut ui = start(move || {
        page().child(column_of(
            button("Under").on_click(move |_| count.set(count.get() + 1)),
        ))
    });
    toast_with(Toast::new("Moved").lasting(Duration::from_secs(100)));
    ui.update();
    run(&mut ui, 300);
    click_text(&mut ui, "Under");
    assert_eq!(clicks.get(), 1);
    click_text(&mut ui, "×");
    run(&mut ui, 400);
    assert!(!has_text("Moved"));
    assert_at_rest(&mut ui);
}

fn start_dialog(open: Signal<bool>, closed: Rc<Cell<u32>>) -> Ui {
    start(move || {
        column_of(button("Open").on_click(move |_| open.set(true))).child(alert_dialog(
            move || open.get(),
            move || {
                closed.set(closed.get() + 1);
                open.set(false);
            },
            "Title",
            "Body",
            move || vec![button("Cancel"), primary_button("Yes")],
        ))
    })
}

#[test]
fn a_dialog_comes_goes_and_stops_asking_for_frames() {
    let open = signal(false);
    let mut ui = start_dialog(open, Rc::default());
    assert!(!has_text("Title"));
    click_text(&mut ui, "Open");
    assert!(has_text("Title"));
    run(&mut ui, 300);
    open.set(false);
    ui.update();
    assert!(has_text("Title"), "still fading");
    run(&mut ui, 400);
    assert!(!has_text("Title"));
    assert_at_rest(&mut ui);
}

#[test]
fn escape_and_the_scrim_dismiss_a_dialog_and_its_inside_does_not() {
    let open = signal(true);
    let closed: Rc<Cell<u32>> = Rc::default();
    let mut ui = start_dialog(open, closed.clone());
    run(&mut ui, 300);
    click_text(&mut ui, "Body");
    assert_eq!(closed.get(), 0, "its inside");
    key(&mut ui, NamedKey::Escape);
    assert_eq!(closed.get(), 1);
    assert!(!open.get());
    run(&mut ui, 400);
    open.set(true);
    ui.update();
    run(&mut ui, 300);
    click(&mut ui, 5.0, 795.0);
    assert_eq!(closed.get(), 2);
}

#[test]
fn nothing_under_a_dialog_can_be_pressed() {
    let open = signal(true);
    let clicks = Rc::new(Cell::new(0));
    let count = clicks.clone();
    let mut ui = start(move || {
        column_of(button("Under").on_click(move |_| count.set(count.get() + 1))).child(dialog(
            move || open.get(),
            || {},
            || text("Above"),
        ))
    });
    run(&mut ui, 300);
    click_text(&mut ui, "Under");
    assert_eq!(clicks.get(), 0);
}

#[test]
fn tab_stays_in_a_dialog_and_the_focus_goes_back() {
    let open = signal(false);
    let under = Rc::new(Cell::new(0));
    let cancel = Rc::new(Cell::new(0));
    let (u, c) = (under.clone(), cancel.clone());
    let mut ui = start(move || {
        let (u, c) = (u.clone(), c.clone());
        column_of(
            button("Under")
                .on_click(move |_| u.set(u.get() + 1))
                .on_key_down(move |e| {
                    if e.key == Key::Character("o".to_owned()) {
                        open.set(true);
                    }
                }),
        )
        .child(alert_dialog(
            move || open.get(),
            move || open.set(false),
            "Title",
            "Body",
            move || {
                let c = c.clone();
                vec![button("Cancel").on_click(move |_| c.set(c.get() + 1))]
            },
        ))
    });
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::Enter);
    assert_eq!(under.get(), 1, "the focus is on it");
    typed(&mut ui, "o");
    run(&mut ui, 300);
    // Around and around: the panel itself and the button.
    for _ in 0..4 {
        key(&mut ui, NamedKey::Tab);
        key(&mut ui, NamedKey::Enter);
    }
    assert!(cancel.get() >= 1);
    assert_eq!(under.get(), 1, "it never got out");
    key(&mut ui, NamedKey::Escape);
    run(&mut ui, 400);
    key(&mut ui, NamedKey::Enter);
    assert_eq!(under.get(), 2, "the focus came back");
}

fn entries(log: Rc<Cell<&'static str>>) -> Vec<MenuEntry> {
    let say = |what: &'static str, log: &Rc<Cell<&'static str>>| {
        let log = log.clone();
        move || log.set(what)
    };
    vec![
        menu_item("First")
            .shortcut("Ctrl+F")
            .on_click(say("first", &log))
            .into(),
        menu_item("Off")
            .disabled_when(|| true)
            .on_click(say("off", &log))
            .into(),
        menu_separator(),
        menu_item("Third").on_click(say("third", &log)).into(),
        submenu(
            "More",
            vec![
                menu_item("Inner one")
                    .on_click(say("inner one", &log))
                    .into(),
                menu_item("Inner two")
                    .on_click(say("inner two", &log))
                    .into(),
            ],
        ),
    ]
}

/// Opens a dropdown and plays the platform for it.
fn open_menu(log: Rc<Cell<&'static str>>) -> Ui {
    let mut ui = start(move || column_of(dropdown_button("File", entries(log))));
    click_text(&mut ui, "File");
    let shown = popups_shown(&mut ui);
    assert_eq!(shown.len(), 1);
    place(&mut ui, &shown[0]);
    let _ = ui.paint_frame(Instant::now());
    ui
}

#[test]
fn a_menu_opens_and_a_row_does_what_it_says_and_closes_it() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log.clone());
    assert!(has_text("First"));
    assert!(has_text("Ctrl+F"));
    click_text(&mut ui, "Third");
    assert_eq!(log.get(), "third");
    assert!(!has_text("First"));
}

#[test]
fn a_row_that_is_disabled_does_nothing() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log.clone());
    click_text(&mut ui, "Off");
    assert_eq!(log.get(), "");
    assert!(has_text("First"), "it stays open");
}

#[test]
fn the_keys_go_through_a_menu_past_what_can_not_be_chosen() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log.clone());
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::Enter);
    assert_eq!(log.get(), "third", "over what is off and the line");
    assert!(!has_text("First"));
}

#[test]
fn the_keys_go_around_a_menu_and_to_its_ends() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log.clone());
    key(&mut ui, NamedKey::End);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::Space);
    assert_eq!(log.get(), "first");
}

#[test]
fn escape_closes_a_menu_and_gives_the_focus_back() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log);
    key(&mut ui, NamedKey::Escape);
    assert!(!has_text("First"));
    run(&mut ui, 300);
    // The button has it again: Enter opens the menu again.
    key(&mut ui, NamedKey::Enter);
    assert!(has_text("First"));
}

#[test]
fn tab_closes_a_menu() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log);
    key(&mut ui, NamedKey::Tab);
    assert!(!has_text("First"));
}

#[test]
fn a_submenu_opens_with_the_keys_and_comes_back() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log.clone());
    key(&mut ui, NamedKey::End);
    assert!(!has_text("Inner one"));
    key(&mut ui, NamedKey::ArrowRight);
    assert!(has_text("Inner one"));
    let shown = popups_shown(&mut ui);
    assert_eq!(shown.len(), 1, "a popup of its own");
    place(&mut ui, &shown[0]);
    let _ = ui.paint_frame(Instant::now());
    key(&mut ui, NamedKey::ArrowLeft);
    assert!(!has_text("Inner one"), "left closes it");
    assert!(has_text("First"), "and only it");
    run(&mut ui, 300);
    key(&mut ui, NamedKey::ArrowRight);
    let shown = popups_shown(&mut ui);
    place(&mut ui, &shown[0]);
    let _ = ui.paint_frame(Instant::now());
    run(&mut ui, 300);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::Enter);
    assert_eq!(log.get(), "inner two");
    assert!(!has_text("First"), "everything closes");
}

#[test]
fn a_submenu_opens_when_the_pointer_rests_on_its_row() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = open_menu(log);
    let (x, y) = middle_of("More");
    pointer_at(&mut ui, x, y);
    assert!(!has_text("Inner one"));
    wait(&mut ui, 300);
    assert!(has_text("Inner one"));
    let (x, y) = middle_of("First");
    pointer_at(&mut ui, x, y);
    wait(&mut ui, 300);
    assert!(!has_text("Inner one"), "another row closes it");
}

#[test]
fn a_context_menu_opens_where_it_is_pressed() {
    let log: Rc<Cell<&'static str>> = Rc::default();
    let mut ui = start(move || {
        column_of(context_menu(
            div().w(400.0).h(200.0).child(text("Area")),
            entries(log),
        ))
    });
    press_with(
        &mut ui,
        150.0,
        90.0,
        true,
        PointerButton::Primary,
        Modifiers::default(),
    );
    press_with(
        &mut ui,
        150.0,
        90.0,
        false,
        PointerButton::Primary,
        Modifiers::default(),
    );
    assert!(!has_text("First"), "the primary button does nothing");
    press_with(
        &mut ui,
        150.0,
        90.0,
        true,
        PointerButton::Secondary,
        Modifiers::default(),
    );
    press_with(
        &mut ui,
        150.0,
        90.0,
        false,
        PointerButton::Secondary,
        Modifiers::default(),
    );
    assert!(has_text("First"));
    let shown = popups_shown(&mut ui);
    assert_eq!(shown.len(), 1);
    assert_eq!((shown[0].anchor.x, shown[0].anchor.y), (150, 90));
    place(&mut ui, &shown[0]);
    let _ = ui.paint_frame(Instant::now());
    key(&mut ui, NamedKey::Escape);
    assert!(!has_text("First"));
}
