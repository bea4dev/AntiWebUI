//! What organizes the rest, and what only shows something.

mod common;

use anti_webui_core::Ui;
use anti_webui_core::event::NamedKey;
use anti_webui_core::prelude::*;
use anti_webui_core::scene::Primitive;
use anti_webui_core::tree::with_doc;
use anti_webui_widgets::*;
use common::*;
use std::rc::Rc;
use std::time::Instant;

fn column_of(child: impl IntoView) -> Div {
    div().flex().flex_col().p(20.0).child(child)
}

fn start_tabs(current: Signal<usize>) -> Ui {
    start(move || {
        column_of(tabs(
            current,
            vec![
                tab("One", || text("Shows one")),
                tab("Two", || text("Shows two")),
                tab("Three", || text("Shows three")),
            ],
        ))
    })
}

#[test]
fn tabs_show_what_is_chosen() {
    let current = signal(0usize);
    let mut ui = start_tabs(current);
    assert!(has_text("Shows one"));
    assert!(!has_text("Shows two"));
    click_text(&mut ui, "Two");
    assert_eq!(current.get(), 1);
    assert!(has_text("Shows two"));
    assert!(!has_text("Shows one"));
}

#[test]
fn tabs_go_with_the_arrows_and_are_one_stop() {
    let current = signal(0usize);
    let mut ui = start_tabs(current);
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(current.get(), 1);
    key(&mut ui, NamedKey::ArrowLeft);
    key(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(current.get(), 2, "they go around");
    key(&mut ui, NamedKey::Home);
    assert_eq!(current.get(), 0);
    key(&mut ui, NamedKey::End);
    assert_eq!(current.get(), 2);
}

/// How high the box around what a disclosure opens is.
fn opened_height(content: &str) -> i32 {
    let id = text_node(content).expect("the content");
    with_doc(|d| {
        // The text, what holds it, and what clips that.
        let holder = d.get(id).unwrap().parent.unwrap();
        let clip = d.get(holder).unwrap().parent.unwrap();
        d.window_rect(clip).unwrap().height
    })
}

#[test]
fn a_disclosure_opens_smoothly_and_lets_go_of_what_is_in_it() {
    let open = signal(false);
    let mut ui = start(move || column_of(disclosure(open, "Title", || text("Inside"))));
    assert!(!has_text("Inside"), "it doesn't exist while closed");
    click_text(&mut ui, "Title");
    assert!(open.get());
    assert!(has_text("Inside"));
    run(&mut ui, 40);
    let early = opened_height("Inside");
    run(&mut ui, 400);
    let late = opened_height("Inside");
    assert!(late > 0);
    assert!(early < late, "{early} then {late}");
    assert_at_rest(&mut ui);
    click_text(&mut ui, "Title");
    assert!(has_text("Inside"), "it stays while it closes");
    run(&mut ui, 400);
    wait(&mut ui, 400);
    assert!(!has_text("Inside"));
    assert_at_rest(&mut ui);
}

#[test]
fn a_disclosure_that_is_open_from_the_start_does_not_animate() {
    let open = signal(true);
    let mut ui = start(move || column_of(disclosure(open, "Title", || text("Inside"))));
    let _ = ui.paint(Instant::now());
    ui.update();
    let first = opened_height("Inside");
    assert!(first > 0);
    run(&mut ui, 400);
    assert_eq!(opened_height("Inside"), first);
}

#[test]
fn reopening_a_disclosure_in_time_keeps_what_is_in_it() {
    let open = signal(true);
    let mut ui = start(move || column_of(disclosure(open, "Title", || text("Inside"))));
    let _ = ui.paint(Instant::now());
    ui.update();
    open.set(false);
    ui.update();
    run(&mut ui, 40);
    open.set(true);
    ui.update();
    wait(&mut ui, 400);
    assert!(has_text("Inside"));
}

#[test]
fn an_accordion_has_one_open_at_a_time() {
    let mut ui = start(|| {
        column_of(accordion(vec![
            (
                "A".to_owned(),
                Rc::new(|| text("Inside A").into_view()) as Rc<dyn Fn() -> View>,
            ),
            ("B".to_owned(), Rc::new(|| text("Inside B").into_view())),
        ]))
    });
    click_text(&mut ui, "A");
    assert!(has_text("Inside A"));
    click_text(&mut ui, "B");
    run(&mut ui, 300);
    wait(&mut ui, 400);
    assert!(has_text("Inside B"));
    assert!(!has_text("Inside A"));
    click_text(&mut ui, "B");
    run(&mut ui, 300);
    wait(&mut ui, 400);
    assert!(!has_text("Inside B"));
}

#[test]
fn a_progress_bar_fills_as_far_as_it_says() {
    set_theme(Theme::light());
    let value = signal(0.25f32);
    let mut ui = start(move || column_of(progress_bar(move || value.get()).w(400.0)));
    let accent = Theme::light().accent;
    let width = |ui: &mut Ui| colored(&run(ui, 400), accent)[0].rect.width;
    assert_eq!(width(&mut ui), 100);
    value.set(0.5);
    ui.update();
    assert_eq!(width(&mut ui), 200);
    value.set(2.0);
    ui.update();
    assert_eq!(width(&mut ui), 400, "not beyond the track");
    assert_at_rest(&mut ui);
}

#[test]
fn an_indeterminate_bar_moves_and_stops_with_its_owner() {
    set_theme(Theme::light());
    let shown = signal(true);
    let mut ui = start(move || {
        column_of(show(
            move || shown.get(),
            || indeterminate_progress_bar().w(400.0),
        ))
    });
    let accent = Theme::light().accent;
    let a = colored(&run(&mut ui, 300), accent)[0].rect.x;
    let b = colored(&run(&mut ui, 500), accent)[0].rect.x;
    assert_ne!(a, b);
    assert!(ui.animating());
    shown.set(false);
    ui.update();
    assert_at_rest(&mut ui);
}

#[test]
fn a_spinner_turns_without_being_laid_out_and_stops_with_its_owner() {
    let shown = signal(true);
    let mut ui = start(move || column_of(show(move || shown.get(), spinner)));
    let scene = run(&mut ui, 100);
    let turning = scene
        .primitives
        .iter()
        .any(|p| matches!(p, Primitive::Layer(_)));
    assert!(turning, "a layer turns");
    shown.set(false);
    ui.update();
    assert_at_rest(&mut ui);
}

#[test]
fn a_split_goes_with_the_pointer_and_the_keys() {
    let ratio = signal(0.5f32);
    let mut ui = start(move || {
        div().size_full().flex().child(
            split_row(ratio, text("Left"), text("Right"))
                .flex_1()
                .h(400.0),
        )
    });
    let _ = ui.paint(Instant::now());
    let middle = 500.0;
    pointer_at(&mut ui, middle, 100.0);
    pointer(&mut ui, middle, 100.0, true);
    pointer_at(&mut ui, 250.0, 100.0);
    assert!((ratio.get() - 0.25).abs() < 0.01, "{}", ratio.get());
    pointer(&mut ui, 250.0, 100.0, false);
    pointer_at(&mut ui, 700.0, 100.0);
    assert!((ratio.get() - 0.25).abs() < 0.01, "released");
    let _ = ui.paint(Instant::now());
    let (x, _) = middle_of("Right");
    assert_eq!(x, 625.5, "the second pane is what is right of the line");
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowRight);
    assert!((ratio.get() - 0.27).abs() < 0.01, "{}", ratio.get());
    pointer_at(&mut ui, 300.0, 100.0);
}

#[test]
fn a_split_keeps_some_room_for_both() {
    let ratio = signal(0.5f32);
    let mut ui = start(move || {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(split_col(ratio, text("Top"), text("Bottom")).flex_1())
    });
    let _ = ui.paint(Instant::now());
    pointer_at(&mut ui, 100.0, 400.0);
    pointer(&mut ui, 100.0, 400.0, true);
    pointer_at(&mut ui, 100.0, 0.0);
    assert!(ratio.get() >= 0.05);
    pointer_at(&mut ui, 100.0, 800.0);
    assert!(ratio.get() <= 0.95);
}

#[test]
fn a_breadcrumb_goes_where_it_says_and_not_from_where_one_is() {
    let went = signal(0);
    let mut ui = start(move || {
        column_of(breadcrumb(vec![
            crumb("Home").on_click(move || went.set(1)),
            crumb("Here"),
        ]))
    });
    click_text(&mut ui, "Here");
    assert_eq!(went.get(), 0);
    click_text(&mut ui, "Home");
    assert_eq!(went.get(), 1);
}

#[test]
fn pages_can_be_gone_through() {
    let page_at = signal(4usize);
    let mut ui = start(move || column_of(pagination(page_at, || 20)));
    assert!(has_text("1") && has_text("20") && has_text("4") && has_text("6"));
    assert!(!has_text("8"));
    assert_eq!(count_text("…"), 2);
    click_text(&mut ui, "6");
    assert_eq!(page_at.get(), 5);
    assert!(has_text("7"));
    click_text(&mut ui, "›");
    assert_eq!(page_at.get(), 6);
    click_text(&mut ui, "1");
    assert_eq!(page_at.get(), 0);
    assert_eq!(count_text("…"), 1, "nothing is left out before it");
    click_text(&mut ui, "‹");
    assert_eq!(page_at.get(), 0, "it is the first");
    click_text(&mut ui, "20");
    click_text(&mut ui, "›");
    assert_eq!(page_at.get(), 19, "it is the last");
}

#[test]
fn few_pages_all_show() {
    let page_at = signal(0usize);
    let _ui = start(move || column_of(pagination(page_at, || 7)));
    for p in 1..=7 {
        assert!(has_text(&p.to_string()));
    }
    assert_eq!(count_text("…"), 0);
}
