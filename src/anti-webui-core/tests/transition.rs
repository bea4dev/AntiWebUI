//! `transition` and `animated`: changes that take time.

use anti_webui_core::Ui;
use anti_webui_core::animation::request_frame;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::tree::with_doc;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn window(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale: Scale::ONE,
    });
    ui.update();
    let _ = ui.paint(Instant::now());
}

/// Goes to `at` milliseconds after `start`, like a frame does.
fn frame_at(ui: &mut Ui, start: Instant, at: u64) -> Scene {
    ui.animate(start + Duration::from_millis(at));
    ui.update();
    ui.paint(start + Duration::from_millis(at))
}

fn opacity_of(n: NodeRef) -> f32 {
    with_doc(|d| d.get(n.get().unwrap()).unwrap().computed.opacity)
}

fn computed<R>(n: NodeRef, f: impl FnOnce(&anti_webui_style::ComputedStyle) -> R) -> R {
    with_doc(|d| f(&d.get(n.get().unwrap()).unwrap().computed))
}

const SOON: f32 = 0.05;

#[test]
fn a_change_goes_through_the_values_between() {
    let open = signal(false);
    let calls = Rc::new(Cell::new(0));
    let counted = calls.clone();
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .w(50.0)
            .h(50.0)
            .node_ref(n)
            .transition(Transition::ms(100).linear())
            .dyn_style(move |s| {
                counted.set(counted.get() + 1);
                s.opacity(if open.get() { 1.0 } else { 0.0 })
            })
    });
    window(&mut ui);
    assert_eq!(opacity_of(n), 0.0, "what is new starts where it is going");
    assert!(!ui.animating());
    assert_eq!(calls.get(), 1);

    let start = Instant::now();
    open.set(true);
    ui.update();
    assert_eq!(opacity_of(n), 0.0, "it hasn't moved yet");
    assert!(ui.animating());

    frame_at(&mut ui, start, 25);
    assert!((opacity_of(n) - 0.25).abs() < SOON, "{}", opacity_of(n));
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "{}", opacity_of(n));
    assert!(ui.animating());
    frame_at(&mut ui, start, 500);
    assert_eq!(opacity_of(n), 1.0);
    assert!(!ui.animating(), "and nothing starts from the end");
    assert_eq!(calls.get(), 2, "the style is only asked where it is going");
}

#[test]
fn hovering_goes_there_and_leaving_comes_back_as_far_as_it_went() {
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .w(50.0)
            .h(50.0)
            .node_ref(n)
            .opacity(0.0)
            .hover(|s| s.opacity(1.0))
            .transition(Transition::ms(100).linear())
    });
    window(&mut ui);
    let move_to = |ui: &mut Ui, x: f32| {
        ui.handle_event(WindowEvent::PointerMoved {
            position: PointF::new(x, 10.0),
            modifiers: Modifiers::default(),
        });
        ui.update();
    };
    let start = Instant::now();
    move_to(&mut ui, 10.0);
    frame_at(&mut ui, start, 20);
    let reached = opacity_of(n);
    assert!((reached - 0.2).abs() < SOON, "{reached}");

    let back = Instant::now();
    move_to(&mut ui, 300.0);
    assert!(
        (opacity_of(n) - reached).abs() < 1e-6,
        "it goes on from where it is"
    );
    // It took about 20 ms to come, so about that to go back; 100 would still be near the start.
    frame_at(&mut ui, back, 10);
    assert!(opacity_of(n) < reached * 0.75, "{}", opacity_of(n));
    frame_at(&mut ui, back, 40);
    assert_eq!(opacity_of(n), 0.0);
    assert!(!ui.animating());
}

#[test]
fn going_elsewhere_midway_goes_on_from_where_it_is() {
    let goal = signal(0.0_f32);
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .w(50.0)
            .h(50.0)
            .node_ref(n)
            .transition(Transition::ms(100).linear())
            .dyn_style(move |s| s.translate_x(goal.get()))
    });
    window(&mut ui);
    let start = Instant::now();
    goal.set(100.0);
    ui.update();
    frame_at(&mut ui, start, 50);
    let reached = computed(n, |c| c.translate_x);
    assert!((reached - 50.0).abs() < 5.0, "{reached}");

    let again = Instant::now();
    goal.set(200.0);
    ui.update();
    assert!((computed(n, |c| c.translate_x) - reached).abs() < 1e-3);
    frame_at(&mut ui, again, 50);
    let x = computed(n, |c| c.translate_x);
    let halfway = reached + (200.0 - reached) * 0.5;
    assert!((x - halfway).abs() < 8.0, "{x} vs {halfway}");
    frame_at(&mut ui, again, 200);
    assert_eq!(computed(n, |c| c.translate_x), 200.0);
}

#[test]
fn only_what_is_named_takes_time() {
    let on = signal(false);
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .w(50.0)
            .h(50.0)
            .node_ref(n)
            .transition_of([Prop::Opacity], Transition::ms(100).linear())
            .dyn_style(move |s| {
                if on.get() {
                    s.opacity(0.0).bg(rgb(0x0000ff))
                } else {
                    s.bg(rgb(0xff0000))
                }
            })
    });
    window(&mut ui);
    let start = Instant::now();
    on.set(true);
    ui.update();
    assert_eq!(computed(n, |c| c.background), rgb(0x0000ff));
    assert_eq!(opacity_of(n), 1.0);
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON);
}

#[test]
fn the_style_it_goes_to_decides_how_it_goes() {
    let on = signal(false);
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().w(50.0).h(50.0).node_ref(n).dyn_style(move |s| {
            if on.get() {
                s.opacity(0.0).transition(Transition::ms(100).linear())
            } else {
                s
            }
        })
    });
    window(&mut ui);
    let start = Instant::now();
    on.set(true);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "takes time going there");
    on.set(false);
    ui.update();
    assert_eq!(opacity_of(n), 1.0, "and none coming back");
    assert!(!ui.animating());
}

#[test]
fn something_new_or_hidden_has_nothing_to_go_from() {
    let hidden = signal(true);
    let goal = signal(0.0_f32);
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .w(50.0)
            .h(50.0)
            .node_ref(n)
            .opacity(0.5)
            .transition(Transition::ms(100))
            .dyn_style(move |s| {
                let s = s.translate_x(goal.get());
                if hidden.get() { s.hidden() } else { s }
            })
    });
    window(&mut ui);
    assert!(!ui.animating());
    goal.set(40.0);
    ui.update();
    assert_eq!(computed(n, |c| c.translate_x), 40.0, "not while hidden");
    assert!(!ui.animating());
    hidden.set(false);
    ui.update();
    assert!(!ui.animating(), "not when it appears");
    assert_eq!(computed(n, |c| c.translate_x), 40.0);
}

#[test]
fn a_transition_of_a_transform_paints_nothing_again() {
    let on = signal(false);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(100.0)
                .h(100.0)
                .bg(rgb(0xff0000))
                .translate_x(1.0)
                .transition(Transition::ms(100).linear())
                .dyn_style(move |s| s.scale(if on.get() { 2.0 } else { 1.0 }).rotate(0.0)),
        )
    });
    window(&mut ui);
    let start = Instant::now();
    on.set(true);
    ui.update();
    let content = |s: &Scene| {
        s.primitives
            .iter()
            .find_map(|p| match p {
                Primitive::Layer(l) => Some((l.content.clone(), l.transform)),
                _ => None,
            })
            .unwrap()
    };
    let a = frame_at(&mut ui, start, 25);
    let b = frame_at(&mut ui, start, 50);
    let ((ca, ma), (cb, mb)) = (content(&a), content(&b));
    assert!(Arc::ptr_eq(&ca, &cb), "the same content");
    assert_ne!(ma, mb, "shown differently");
}

#[test]
fn a_transition_of_a_size_moves_what_is_next_to_it() {
    let wide = signal(false);
    let next = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .h(10.0)
                    .transition(Transition::ms(100).linear())
                    .dyn_style(move |s| s.w(if wide.get() { 100.0 } else { 0.0 })),
            )
            .child(div().w(10.0).h(10.0).node_ref(next))
    });
    window(&mut ui);
    let start = Instant::now();
    wide.set(true);
    ui.update();
    frame_at(&mut ui, start, 50);
    let x = next.rect().unwrap().x;
    assert!((x - 50).abs() <= 5, "{x}");
    frame_at(&mut ui, start, 500);
    assert_eq!(next.rect().unwrap().x, 100);
}

#[test]
fn what_can_not_go_between_just_changes() {
    let fixed = signal(false);
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .h(10.0)
            .node_ref(n)
            .transition(Transition::ms(100))
            .dyn_style(move |s| if fixed.get() { s.w(30.0) } else { s })
    });
    window(&mut ui);
    fixed.set(true);
    ui.update();
    assert_eq!(n.rect().unwrap().width, 30);
    assert!(!ui.animating(), "auto has no number to start from");
}

#[test]
fn text_inherits_the_color_on_its_way() {
    let dark = signal(false);
    let mut ui = Ui::new(move || {
        div()
            .transition(Transition::ms(100).linear())
            .dyn_style(move |s| {
                s.text_color(if dark.get() {
                    Color::BLACK
                } else {
                    Color::WHITE
                })
            })
            .child(text("a"))
    });
    window(&mut ui);
    let start = Instant::now();
    dark.set(true);
    ui.update();
    let scene = frame_at(&mut ui, start, 50);
    let Some(Primitive::Glyphs(run)) = scene
        .primitives
        .iter()
        .find(|p| matches!(p, Primitive::Glyphs(_)))
    else {
        panic!();
    };
    assert!(run.color.r > 0.3 && run.color.r < 0.7, "{:?}", run.color);
}

#[test]
fn it_ends_with_the_node() {
    let there = signal(true);
    let on = signal(false);
    let mut ui = Ui::new(move || {
        div().size_full().child(show(
            move || there.get(),
            move || {
                div()
                    .w(10.0)
                    .h(10.0)
                    .transition(Transition::ms(1000))
                    .dyn_style(move |s| s.opacity(if on.get() { 0.0 } else { 1.0 }))
            },
        ))
    });
    window(&mut ui);
    let start = Instant::now();
    on.set(true);
    ui.update();
    frame_at(&mut ui, start, 10);
    assert!(ui.animating());
    there.set(false);
    ui.update();
    assert!(!ui.animating());
    frame_at(&mut ui, start, 20);
}

// ---------------------------------------------------------------------------------------------

/// Frames every 8 ms from `start` until it stops moving. Returns how many it took.
fn run_out(ui: &mut Ui, start: Instant, from: u64) -> u64 {
    let mut at = from;
    while ui.animating() {
        at += 8;
        frame_at(ui, start, at);
        assert!(at < 20_000, "never stops");
    }
    at
}

#[test]
fn animated_moves_and_what_reads_it_runs_only_then() {
    let slot = Rc::new(Cell::new(None));
    let calls = Rc::new(Cell::new(0));
    let (s, c) = (slot.clone(), calls.clone());
    let mut ui = Ui::new(move || {
        let x = animated(0.0_f32);
        s.set(Some(x));
        let c = c.clone();
        div().w(10.0).h(10.0).dyn_style(move |st| {
            c.set(c.get() + 1);
            st.translate_x(x.get())
        })
    });
    window(&mut ui);
    let x = slot.get().unwrap();
    for _ in 0..3 {
        frame_at(&mut ui, Instant::now(), 0);
    }
    assert_eq!(calls.get(), 1);
    assert!(!ui.animating());

    let start = Instant::now();
    x.to(100.0, Transition::ms(100).linear());
    assert!(ui.animating());
    frame_at(&mut ui, start, 25);
    assert!((x.get_untracked() - 25.0).abs() < 5.0);
    frame_at(&mut ui, start, 50);
    assert!((x.get_untracked() - 50.0).abs() < 5.0);
    assert_eq!(calls.get(), 3);
    frame_at(&mut ui, start, 200);
    assert_eq!(x.get_untracked(), 100.0);
    assert!(!ui.animating());
    let done = calls.get();
    frame_at(&mut ui, start, 300);
    assert_eq!(calls.get(), done);
}

#[test]
fn animated_goes_elsewhere_from_where_it_is_and_set_and_stop_end_it() {
    let (x, _) = create_root(|| animated(0.0_f32));
    let mut ui = Ui::new(div);
    window(&mut ui);
    let start = Instant::now();
    x.to(100.0, Transition::ms(100).linear());
    frame_at(&mut ui, start, 50);
    let reached = x.get_untracked();

    let again = Instant::now();
    x.to(0.0, Transition::ms(100).linear());
    frame_at(&mut ui, again, 50);
    assert!(
        (x.get_untracked() - reached * 0.5).abs() < 6.0,
        "{}",
        x.get_untracked()
    );

    x.stop();
    let stopped = x.get_untracked();
    frame_at(&mut ui, again, 90);
    assert_eq!(x.get_untracked(), stopped);
    assert!(!ui.animating());

    x.to(100.0, Transition::ms(100).linear());
    x.set(7.0);
    assert_eq!(x.get_untracked(), 7.0);
    frame_at(&mut ui, again, 150);
    assert_eq!(x.get_untracked(), 7.0, "what was going on is over");
    assert!(!ui.animating());
}

#[test]
fn something_reading_two_moving_values_runs_once_per_frame() {
    let slot = Rc::new(Cell::new(None));
    let calls = Rc::new(Cell::new(0));
    let (s, c) = (slot.clone(), calls.clone());
    let mut ui = Ui::new(move || {
        let (a, b) = (animated(0.0_f32), animated(0.0_f32));
        s.set(Some((a, b)));
        let c = c.clone();
        div().dyn_style(move |st| {
            c.set(c.get() + 1);
            st.translate(a.get(), b.get())
        })
    });
    window(&mut ui);
    let (a, b) = slot.get().unwrap();
    let start = Instant::now();
    a.to(100.0, Transition::ms(100));
    b.to(100.0, Transition::ms(100));
    let before = calls.get();
    frame_at(&mut ui, start, 30);
    assert_eq!(calls.get(), before + 1);
}

#[test]
fn a_spring_gets_there_and_rests_exactly() {
    let (x, _) = create_root(|| animated(0.0_f32));
    let mut ui = Ui::new(div);
    window(&mut ui);
    let start = Instant::now();
    x.spring_to(100.0, Spring::default());
    frame_at(&mut ui, start, 100);
    let early = x.get_untracked();
    assert!(early > 20.0 && early < 100.0, "{early}");
    let end = run_out(&mut ui, start, 100);
    assert_eq!(x.get_untracked(), 100.0);
    assert!(end < 2000, "took {end} ms");
}

#[test]
fn a_bouncy_spring_goes_past_and_comes_back() {
    let (x, _) = create_root(|| animated(0.0_f32));
    let mut ui = Ui::new(div);
    window(&mut ui);
    let start = Instant::now();
    x.spring_to(
        100.0,
        Spring {
            stiffness: 300.0,
            damping: 8.0,
            mass: 1.0,
        },
    );
    let mut most = 0.0_f32;
    let mut at = 0;
    while ui.animating() {
        at += 8;
        frame_at(&mut ui, start, at);
        most = most.max(x.get_untracked());
        assert!(at < 20_000);
    }
    assert!(most > 110.0, "{most}");
    assert_eq!(x.get_untracked(), 100.0);
}

#[test]
fn a_spring_moves_the_same_at_any_frame_rate() {
    let mut ui = Ui::new(div);
    window(&mut ui);
    let at_rate = |ui: &mut Ui, step: u64| {
        let (x, _) = create_root(|| animated(0.0_f32));
        let start = Instant::now();
        x.spring_to(100.0, Spring::default());
        let mut at = 0;
        while at < 240 {
            at += step;
            frame_at(ui, start, at);
        }
        x.stop();
        x.get_untracked()
    };
    let (fast, slow) = (at_rate(&mut ui, 4), at_rate(&mut ui, 16));
    assert!((fast - slow).abs() < 1.5, "{fast} vs {slow}");
}

#[test]
fn a_spring_keeps_the_speed_it_has() {
    let mut ui = Ui::new(div);
    window(&mut ui);
    let (x, _) = create_root(|| animated(0.0_f32));
    let start = Instant::now();
    x.spring_to(1000.0, Spring::default());
    frame_at(&mut ui, start, 100);
    let reached = x.get_untracked();
    let again = Instant::now();
    // Going back: moving at speed, it first goes on further forwards.
    x.spring_to(0.0, Spring::default());
    frame_at(&mut ui, again, 16);
    assert!(
        x.get_untracked() > reached,
        "{} vs {reached}",
        x.get_untracked()
    );
}

#[test]
fn a_tween_hands_its_speed_to_a_spring() {
    let mut ui = Ui::new(div);
    window(&mut ui);
    let (x, _) = create_root(|| animated(0.0_f32));
    let start = Instant::now();
    x.to(1000.0, Transition::ms(200).linear());
    frame_at(&mut ui, start, 50);
    frame_at(&mut ui, start, 100);
    let reached = x.get_untracked();
    let again = Instant::now();
    x.spring_to(0.0, Spring::default());
    frame_at(&mut ui, again, 16);
    assert!(
        x.get_untracked() > reached,
        "{} vs {reached}",
        x.get_untracked()
    );
}

#[test]
fn it_stops_with_its_owner() {
    let there = signal(true);
    let slot = Rc::new(Cell::new(None));
    let s = slot.clone();
    let mut ui = Ui::new(move || {
        let s = s.clone();
        div().child(show(
            move || there.get(),
            move || {
                let x = animated(0.0_f32);
                s.set(Some(x));
                x.to(100.0, Transition::ms(1000));
                div().dyn_style(move |st| st.opacity(x.get() / 100.0))
            },
        ))
    });
    window(&mut ui);
    let start = Instant::now();
    frame_at(&mut ui, start, 10);
    assert!(ui.animating());
    there.set(false);
    ui.update();
    frame_at(&mut ui, start, 20);
    assert!(!ui.animating(), "no panic, and nothing goes on");
}

#[test]
fn colors_go_through_without_darkening() {
    let (c, _) = create_root(|| animated(Color::TRANSPARENT));
    let mut ui = Ui::new(div);
    window(&mut ui);
    let start = Instant::now();
    c.to(Color::new(0.0, 0.0, 1.0, 1.0), Transition::ms(100).linear());
    frame_at(&mut ui, start, 50);
    let v = c.get_untracked();
    assert!(v.a > 0.3 && v.a < 0.7, "{v:?}");
    assert!((v.b - 1.0).abs() < 1e-3, "{v:?}");
}

#[test]
fn request_frame_callbacks_see_each_others_values() {
    let x = signal(0);
    let seen = Rc::new(Cell::new(0));
    let got = seen.clone();
    let mut ui = Ui::new(div);
    window(&mut ui);
    request_frame(move |_| x.set(5));
    request_frame(move |_| got.set(x.get()));
    ui.animate(Instant::now());
    assert_eq!(seen.get(), 5);
}
