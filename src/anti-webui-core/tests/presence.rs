//! `show` with `enter` and `exit`: what comes and goes takes time, and stays around meanwhile.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::tree::with_doc;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn window(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::ONE,
    });
    ui.update();
    let _ = ui.paint(Instant::now());
}

fn frame_at(ui: &mut Ui, start: Instant, at: u64) -> Scene {
    ui.animate(start + Duration::from_millis(at));
    ui.update();
    ui.paint(start + Duration::from_millis(at))
}

/// Frames until nothing goes on anymore.
fn run_out(ui: &mut Ui, start: Instant, from: u64) {
    let mut at = from;
    while ui.animating() {
        at += 8;
        frame_at(ui, start, at);
        assert!(at < 5000, "never ends");
    }
}

fn there(n: NodeRef) -> bool {
    n.get().is_some_and(|id| with_doc(|d| d.get(id).is_some()))
}

fn opacity_of(n: NodeRef) -> f32 {
    with_doc(|d| d.get(n.get().unwrap()).unwrap().computed.opacity)
}

fn fading(open: Signal<bool>, n: NodeRef, builds: Rc<Cell<u32>>) -> Div {
    div().size_full().child(
        show(
            move || open.get(),
            move || {
                builds.set(builds.get() + 1);
                div().w(100.0).h(100.0).node_ref(n)
            },
        )
        .enter(|s| s.opacity(0.0))
        .exit(|s| s.opacity(0.0))
        .transition(Transition::ms(100).linear()),
    )
}

const SOON: f32 = 0.06;

#[test]
fn what_is_shown_from_the_start_does_not_animate_unless_asked() {
    let (open, n) = (signal(true), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    assert_eq!(opacity_of(n), 1.0);
    assert!(!ui.animating());

    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().child(
            show(|| true, move || div().w(10.0).h(10.0).node_ref(n))
                .enter(|s| s.opacity(0.0))
                .transition(Transition::ms(100).linear())
                .appear(),
        )
    });
    let start = Instant::now();
    window(&mut ui);
    assert!(ui.animating());
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "{}", opacity_of(n));
}

#[test]
fn it_comes_from_enter_and_ends_as_it_is() {
    let (open, n) = (signal(false), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    assert!(!there(n));

    let start = Instant::now();
    open.set(true);
    ui.update();
    assert_eq!(opacity_of(n), 0.0, "where it starts");
    let _ = ui.paint(start);
    assert!(ui.animating());
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "{}", opacity_of(n));
    run_out(&mut ui, start, 50);
    assert_eq!(opacity_of(n), 1.0);
    assert_eq!(builds.get(), 1);
}

#[test]
fn it_stays_until_it_has_gone_and_then_ends_properly() {
    let (open, n) = (signal(true), node_ref());
    let cleaned = Rc::new(Cell::new(false));
    let c = cleaned.clone();
    let x = Rc::new(Cell::new(None));
    let slot = x.clone();
    let mut ui = Ui::new(move || {
        let (c, slot) = (c.clone(), slot.clone());
        div().size_full().child(
            show(
                move || open.get(),
                move || {
                    let c = c.clone();
                    on_cleanup(move || c.set(true));
                    let inner = signal(5);
                    slot.set(Some(inner));
                    div().w(100.0).h(100.0).node_ref(n)
                },
            )
            .exit(|s| s.opacity(0.0))
            .transition(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);
    let start = Instant::now();
    open.set(false);
    ui.update();
    assert!(there(n), "still here");
    assert_eq!(opacity_of(n), 1.0);
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "{}", opacity_of(n));
    assert!(!cleaned.get());
    assert_eq!(x.get().unwrap().get_untracked(), 5, "what it owns lives on");
    run_out(&mut ui, start, 50);
    assert!(!there(n));
    assert!(cleaned.get());
    assert!(!x.get().unwrap().is_alive());
}

/// A frame moves things on before it resolves styles: going away may not even have started then.
#[test]
fn it_is_not_taken_for_gone_before_it_has_started_going() {
    let (open, n) = (signal(true), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    let start = Instant::now();
    open.set(false);
    frame_at(&mut ui, start, 0);
    assert!(there(n));
    frame_at(&mut ui, start, 50);
    assert!(there(n));
    assert!(opacity_of(n) < 1.0);
    run_out(&mut ui, start, 50);
    assert!(!there(n));
}

#[test]
fn toggling_quickly_ends_up_as_the_last_says() {
    let (open, n) = (signal(true), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    let start = Instant::now();
    for i in 0..7 {
        open.update(|v| *v = !*v);
        frame_at(&mut ui, start, i * 3);
    }
    assert!(!open.get_untracked());
    run_out(&mut ui, start, 30);
    assert!(!there(n));
    assert_eq!(builds.get(), 1, "never rebuilt on the way");
    open.set(true);
    ui.update();
    run_out(&mut ui, Instant::now(), 0);
    assert_eq!(opacity_of(n), 1.0);
    assert_eq!(builds.get(), 2);
}

#[test]
fn coming_back_midway_keeps_what_it_is() {
    let (open, n) = (signal(true), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    let id = n.get();

    let start = Instant::now();
    open.set(false);
    ui.update();
    frame_at(&mut ui, start, 50);
    let reached = opacity_of(n);

    let back = Instant::now();
    open.set(true);
    ui.update();
    assert!(
        (opacity_of(n) - reached).abs() < 1e-6,
        "from where it got to"
    );
    // It went about half way, so it takes about half of the time.
    frame_at(&mut ui, back, 200);
    run_out(&mut ui, back, 200);
    assert_eq!(opacity_of(n), 1.0);
    assert_eq!(n.get(), id, "the same node");
    assert_eq!(builds.get(), 1);
    assert!(there(n), "and what waited for the exit doesn't remove it");
}

#[test]
fn going_midway_through_coming_goes_back_the_way_it_came() {
    let (open, n) = (signal(false), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    let start = Instant::now();
    open.set(true);
    ui.update();
    frame_at(&mut ui, start, 30);
    let reached = opacity_of(n);
    assert!(reached > 0.15, "{reached}");

    let back = Instant::now();
    open.set(false);
    ui.update();
    assert!((opacity_of(n) - reached).abs() < 1e-6);
    frame_at(&mut ui, back, 15);
    assert!(
        opacity_of(n) < reached * 0.75,
        "{} vs {reached}",
        opacity_of(n)
    );
    run_out(&mut ui, back, 15);
    assert!(!there(n));
}

#[test]
fn it_is_built_anew_after_it_has_gone() {
    let (open, n) = (signal(true), node_ref());
    let builds = Rc::new(Cell::new(0));
    let b = builds.clone();
    let mut ui = Ui::new(move || fading(open, n, b));
    window(&mut ui);
    let start = Instant::now();
    open.set(false);
    ui.update();
    run_out(&mut ui, start, 0);
    assert!(!there(n));
    let again = Instant::now();
    open.set(true);
    ui.update();
    assert_eq!(builds.get(), 2);
    assert_eq!(opacity_of(n), 0.0);
    frame_at(&mut ui, again, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON);
}

#[test]
fn what_is_going_takes_no_pointer_and_no_keys() {
    let open = signal(true);
    let (under, field) = (node_ref(), node_ref());
    let value = signal(String::new());
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .relative()
            .child(div().absolute().w(200.0).h(200.0).node_ref(under))
            .child(
                show(
                    move || open.get(),
                    move || {
                        div()
                            .absolute()
                            .w(200.0)
                            .h(200.0)
                            .child(text_input().w(200.0).node_ref(field).value(value))
                    },
                )
                .exit(|s| s.opacity(0.0))
                .transition(Transition::ms(100)),
            )
    });
    window(&mut ui);
    field.focus();
    ui.update();
    let focused = || with_doc(|d| d.state.focused);
    assert_eq!(focused(), field.get());
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(100.0, 100.0),
        modifiers: Modifiers::default(),
    });
    ui.update();
    let hovered = || with_doc(|d| d.state.hovered.first().copied());
    assert_ne!(hovered(), under.get());

    open.set(false);
    ui.update();
    assert_eq!(hovered(), under.get(), "what is under it takes the pointer");
    assert_eq!(focused(), None);
    assert!(there(field), "while it is still here");
}

#[test]
fn it_does_not_wait_when_there_is_nothing_to_see() {
    // Its parent goes.
    let (outer, open, n) = (signal(true), signal(true), node_ref());
    let builds = Rc::new(Cell::new(0));
    let mut ui = Ui::new(move || {
        let b = builds.clone();
        div().size_full().child(show(
            move || outer.get(),
            move || fading(open, n, b.clone()),
        ))
    });
    window(&mut ui);
    let start = Instant::now();
    open.set(false);
    ui.update();
    frame_at(&mut ui, start, 10);
    assert!(there(n));
    outer.set(false);
    ui.update();
    assert!(!there(n), "goes with its parent");
    run_out(&mut ui, start, 10);

    // It isn't shown.
    let (open, hidden, n) = (signal(true), signal(false), node_ref());
    let builds = Rc::new(Cell::new(0));
    let mut ui = Ui::new(move || {
        fading(open, n, builds.clone())
            .dyn_style(move |s| if hidden.get() { s.hidden() } else { s })
    });
    window(&mut ui);
    hidden.set(true);
    ui.update();
    let _ = ui.paint(Instant::now());
    let start = Instant::now();
    open.set(false);
    ui.update();
    frame_at(&mut ui, start, 1);
    frame_at(&mut ui, start, 2);
    assert!(!there(n), "gone at once");
    assert!(!ui.animating());

    // There is nothing to change.
    let (open, n) = (signal(true), node_ref());
    let mut ui = Ui::new(move || {
        div().child(show(move || open.get(), move || div().w(1.0).h(1.0).node_ref(n)).exit(|s| s))
    });
    window(&mut ui);
    let start = Instant::now();
    open.set(false);
    ui.update();
    frame_at(&mut ui, start, 1);
    frame_at(&mut ui, start, 2);
    assert!(!there(n));
    assert!(!ui.animating());
}

#[test]
fn only_enter_goes_at_once_and_only_exit_comes_at_once() {
    let (open, n) = (signal(true), node_ref());
    let mut ui = Ui::new(move || {
        div().child(
            show(move || open.get(), move || div().w(1.0).h(1.0).node_ref(n))
                .enter(|s| s.opacity(0.0)),
        )
    });
    window(&mut ui);
    open.set(false);
    ui.update();
    assert!(!there(n));

    let (open, n) = (signal(false), node_ref());
    let mut ui = Ui::new(move || {
        div().child(
            show(move || open.get(), move || div().w(1.0).h(1.0).node_ref(n))
                .exit(|s| s.opacity(0.0)),
        )
    });
    window(&mut ui);
    open.set(true);
    ui.update();
    assert_eq!(opacity_of(n), 1.0);
    assert!(!ui.animating());
}

#[test]
fn its_own_transition_is_its_own_again_afterwards() {
    let (open, big, n) = (signal(false), signal(false), node_ref());
    let mut ui = Ui::new(move || {
        div().child(
            show(
                move || open.get(),
                move || {
                    div()
                        .w(10.0)
                        .h(10.0)
                        .node_ref(n)
                        .transition(Transition::ms(1000).linear())
                        .dyn_style(move |s| s.opacity(if big.get() { 0.5 } else { 1.0 }))
                },
            )
            .enter(|s| s.opacity(0.0))
            .transition(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);
    let start = Instant::now();
    open.set(true);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!(
        (opacity_of(n) - 0.5).abs() < SOON,
        "coming takes what `show` says"
    );
    run_out(&mut ui, start, 50);
    let later = Instant::now();
    big.set(true);
    ui.update();
    frame_at(&mut ui, later, 100);
    assert!((opacity_of(n) - 0.95).abs() < SOON, "{}", opacity_of(n));
}

#[test]
fn it_goes_through_fragments_and_a_transform_ends_up_as_nothing() {
    let (open, a, b) = (signal(false), node_ref(), node_ref());
    let mut ui = Ui::new(move || {
        div().size_full().child(
            show(
                move || open.get(),
                move || {
                    fragment([
                        div().w(50.0).h(50.0).bg(rgb(0xff0000)).node_ref(a),
                        div().w(50.0).h(50.0).bg(rgb(0x00ff00)).node_ref(b),
                    ])
                },
            )
            .enter(|s| s.opacity(0.0).scale(0.5))
            .transition(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);
    let start = Instant::now();
    open.set(true);
    ui.update();
    let scene = frame_at(&mut ui, start, 50);
    assert!((opacity_of(a) - 0.5).abs() < SOON);
    assert!((opacity_of(b) - 0.5).abs() < SOON);
    let layers = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Layer(_)))
        .count();
    assert_eq!(layers, 2);
    run_out(&mut ui, start, 50);
    let scene = ui.paint(start + Duration::from_secs(1));
    assert!(
        !scene
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Layer(_))),
        "drawn directly once it arrived"
    );
}
