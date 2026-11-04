//! `request_frame`: code that runs once per frame.

use anti_webui_core::Ui;
use anti_webui_core::animation::request_frame;
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn ui() -> Ui {
    Ui::new(|| div().w(10.0).h(10.0))
}

#[test]
fn a_callback_makes_the_ui_want_a_frame_and_runs_once() {
    let mut ui = ui();
    ui.update();
    let _ = ui.paint(Instant::now());
    assert!(!ui.animating());
    assert!(!ui.needs_redraw());

    let runs = Rc::new(Cell::new(0));
    let counted = runs.clone();
    request_frame(move |_| counted.set(counted.get() + 1));
    assert!(ui.animating());
    assert!(ui.needs_redraw());
    assert_eq!(runs.get(), 0, "not before the frame");

    ui.animate(Instant::now());
    assert_eq!(runs.get(), 1);
    assert!(!ui.animating());
    ui.animate(Instant::now());
    assert_eq!(runs.get(), 1, "only once");
}

#[test]
fn asking_again_from_a_callback_waits_for_the_next_frame() {
    fn again(runs: Rc<Cell<u32>>) {
        request_frame(move |_| {
            runs.set(runs.get() + 1);
            again(runs);
        });
    }
    let mut ui = ui();
    let runs = Rc::new(Cell::new(0));
    again(runs.clone());
    for frame in 1..=3 {
        ui.animate(Instant::now());
        assert_eq!(runs.get(), frame);
        assert!(ui.animating());
    }
}

#[test]
fn callbacks_get_the_time_of_the_frame() {
    let mut ui = ui();
    let seen = Rc::new(Cell::new(None));
    let got = seen.clone();
    request_frame(move |now| got.set(Some(now)));
    let now = Instant::now() + Duration::from_secs(1);
    ui.animate(now);
    assert_eq!(seen.get(), Some(now));
}

#[test]
fn what_a_callback_changes_is_in_the_same_frame() {
    let width = signal(10.0_f32);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .h(10.0)
                .bg(rgb(0xff0000))
                .dyn_style(move |s| s.w(width.get())),
        )
    });
    ui.update();
    let _ = ui.paint(Instant::now());
    request_frame(move |_| width.set(20.0));
    ui.animate(Instant::now());
    ui.update();
    assert!(ui.needs_redraw());
}

#[test]
fn a_spring_toward_a_big_number_comes_to_rest() {
    let mut ui = Ui::new(div);
    let v = animated(24_290.0f32);
    v.spring_to(24_291.0, Spring::from_ratio(1500.0, 1.0));
    let start = Instant::now();
    for t in 0..2000 {
        ui.animate(start + std::time::Duration::from_millis(t * 8));
    }
    assert_eq!(v.get_untracked(), 24_291.0);
    assert!(!ui.animating());
}
