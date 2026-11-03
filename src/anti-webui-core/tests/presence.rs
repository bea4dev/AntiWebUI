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

/// Rows as tall as `ROW`, one for each key below `refs.len()`.
const ROW: i32 = 10;

fn list(
    items: Signal<Vec<usize>>,
    refs: Rc<Vec<NodeRef>>,
    builds: Rc<Cell<u32>>,
    f: impl FnOnce(ForEach<usize, usize>) -> ForEach<usize, usize>,
) -> Div {
    div().size_full().flex().flex_col().child(f(for_each(
        move || items.get(),
        |i| *i,
        move |i| {
            builds.set(builds.get() + 1);
            div().w(100.0).h(ROW as f32).node_ref(refs[i])
        },
    )))
}

fn fading_list(l: ForEach<usize, usize>) -> ForEach<usize, usize> {
    l.enter(|s| s.opacity(0.0))
        .exit(|s| s.opacity(0.0))
        .transition(Transition::ms(100).linear())
}

fn refs(n: usize) -> Rc<Vec<NodeRef>> {
    Rc::new((0..n).map(|_| node_ref()).collect())
}

/// The keys there are, from the top. Each one has to be where its place in the list says.
fn shown(refs: &[NodeRef]) -> Vec<usize> {
    let mut there: Vec<_> = (0..refs.len())
        .filter(|i| there(refs[*i]))
        .map(|i| (refs[i].rect().unwrap().y, i))
        .collect();
    there.sort();
    for (place, (y, i)) in there.iter().enumerate() {
        assert_eq!(*y, place as i32 * ROW, "{i} is out of place in {there:?}");
    }
    there.into_iter().map(|(_, i)| i).collect()
}

#[test]
fn what_goes_stays_behind_what_was_in_front_of_it_until_it_has_gone() {
    let items = signal(vec![0, 1, 2, 3, 4]);
    let r = refs(7);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), fading_list));
    window(&mut ui);
    assert_eq!(shown(&r), [0, 1, 2, 3, 4]);

    let start = Instant::now();
    items.set(vec![4, 5, 0, 2]);
    frame_at(&mut ui, start, 10);
    assert_eq!(shown(&r), [4, 5, 0, 1, 2, 3], "what is going moves along");
    run_out(&mut ui, start, 10);
    assert_eq!(shown(&r), [4, 5, 0, 2]);
    assert!(!there(r[1]) && !there(r[3]));
}

#[test]
fn what_goes_first_stays_first_and_what_goes_together_stays_in_order() {
    let items = signal(vec![0, 1, 2, 3, 4]);
    let r = refs(7);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), fading_list));
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![2, 5]);
    frame_at(&mut ui, start, 10);
    assert_eq!(shown(&r), [0, 1, 2, 3, 4, 5]);
    // What was left goes as well, one more time behind what is still there.
    items.set(vec![5]);
    frame_at(&mut ui, start, 20);
    assert_eq!(shown(&r), [0, 1, 2, 3, 4, 5]);
    items.set(vec![6]);
    frame_at(&mut ui, start, 30);
    assert_eq!(shown(&r), [0, 1, 2, 3, 4, 5, 6]);
    run_out(&mut ui, start, 30);
    assert_eq!(shown(&r), [6]);
}

#[test]
fn only_what_is_added_later_comes_unless_asked() {
    let items = signal(vec![0]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), fading_list));
    window(&mut ui);
    assert_eq!(opacity_of(r[0]), 1.0);
    assert!(!ui.animating());
    let start = Instant::now();
    items.set(vec![0, 1]);
    ui.update();
    assert_eq!(opacity_of(r[0]), 1.0);
    assert_eq!(opacity_of(r[1]), 0.0);
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(r[1]) - 0.5).abs() < SOON);
    run_out(&mut ui, start, 50);
    assert_eq!(opacity_of(r[1]), 1.0);

    let r = refs(1);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(signal(vec![0]), rr.clone(), builds.clone(), |l| {
            fading_list(l).appear()
        })
    });
    let start = Instant::now();
    window(&mut ui);
    assert!(ui.animating());
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(r[0]) - 0.5).abs() < SOON);
}

#[test]
fn it_goes_on_going_when_the_list_changes_again() {
    let items = signal(vec![0, 1]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), fading_list));
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    frame_at(&mut ui, start, 50);
    let reached = opacity_of(r[0]);
    assert!((reached - 0.5).abs() < SOON, "{reached}");
    items.set(vec![1, 2]);
    frame_at(&mut ui, start, 75);
    assert!(
        (opacity_of(r[0]) - 0.25).abs() < SOON,
        "{}",
        opacity_of(r[0])
    );
    assert_eq!(shown(&r), [0, 1, 2]);
    run_out(&mut ui, start, 75);
    assert_eq!(shown(&r), [1, 2]);
}

#[test]
fn a_key_that_comes_back_midway_is_the_same_item_and_goes_where_the_list_says() {
    let items = signal(vec![0, 1, 2]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let b = builds.clone();
    let mut ui = Ui::new(move || list(items, rr.clone(), b.clone(), fading_list));
    window(&mut ui);
    let id = r[0].get();
    assert_eq!(builds.get(), 3);

    let start = Instant::now();
    items.set(vec![1, 2]);
    ui.update();
    frame_at(&mut ui, start, 50);
    let reached = opacity_of(r[0]);

    let back = Instant::now();
    items.set(vec![2, 1, 0]);
    ui.update();
    assert_eq!(shown(&r), [2, 1, 0]);
    assert!((opacity_of(r[0]) - reached).abs() < 1e-6);
    frame_at(&mut ui, back, 200);
    run_out(&mut ui, back, 200);
    assert_eq!(opacity_of(r[0]), 1.0);
    assert_eq!(r[0].get(), id);
    assert_eq!(builds.get(), 3, "nothing was built again");
    assert_eq!(shown(&r), [2, 1, 0], "and nothing waits to remove it");

    // Going again after that, it does go.
    let again = Instant::now();
    items.set(vec![2, 1]);
    ui.update();
    run_out(&mut ui, again, 0);
    assert_eq!(shown(&r), [2, 1]);

    // And what is built once it has gone is new.
    items.set(vec![2, 1, 0]);
    ui.update();
    assert_eq!(builds.get(), 4);
}

#[test]
fn each_goes_in_its_own_time() {
    let items = signal(vec![0, 1, 2]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), fading_list));
    window(&mut ui);
    items.set(vec![1, 2]);
    ui.update();
    // Taken after that one started: it is over by 100 ms from here, at the latest.
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    items.set(vec![2]);
    ui.update();
    frame_at(&mut ui, start, 120);
    frame_at(&mut ui, start, 128);
    assert_eq!(shown(&r), [1, 2]);
    run_out(&mut ui, start, 128);
    assert_eq!(shown(&r), [2]);
}

#[test]
fn without_exit_it_goes_at_once_and_without_enter_it_comes_at_once() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            l.enter(|s| s.opacity(0.0))
        })
    });
    window(&mut ui);
    items.set(vec![1]);
    ui.update();
    assert_eq!(shown(&r), [1]);
    assert!(!ui.animating());

    let items = signal(vec![0]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            l.exit(|s| s.opacity(0.0))
        })
    });
    window(&mut ui);
    items.set(vec![0, 1]);
    ui.update();
    assert_eq!(opacity_of(r[1]), 1.0);
    assert!(!ui.animating());

    // Nothing asked for at all.
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), |l| l));
    window(&mut ui);
    items.set(vec![1]);
    ui.update();
    assert_eq!(shown(&r), [1]);
    assert!(!ui.animating());
}

#[test]
fn what_is_going_goes_with_its_list_and_cleans_up() {
    let (items, on) = (signal(vec![0, 1]), signal(true));
    let r = refs(2);
    let cleaned = Rc::new(Cell::new(0));
    let (rr, c) = (r.clone(), cleaned.clone());
    let mut ui = Ui::new(move || {
        let (rr, c) = (rr.clone(), c.clone());
        div().size_full().child(show(
            move || on.get(),
            move || {
                let (rr, c) = (rr.clone(), c.clone());
                for_each(
                    move || items.get(),
                    |i| *i,
                    move |i| {
                        let c = c.clone();
                        on_cleanup(move || c.set(c.get() + 1));
                        div().w(10.0).h(10.0).node_ref(rr[i])
                    },
                )
                .exit(|s| s.opacity(0.0))
                .transition(Transition::ms(100).linear())
            },
        ))
    });
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1]);
    frame_at(&mut ui, start, 10);
    assert!(there(r[0]) && there(r[1]));
    assert_eq!(cleaned.get(), 0);
    on.set(false);
    ui.update();
    assert!(!there(r[0]) && !there(r[1]));
    assert_eq!(cleaned.get(), 2);
    // What waited for it finds nothing to do.
    run_out(&mut ui, start, 10);
    assert_eq!(cleaned.get(), 2);
}

#[test]
fn what_follows_moves_up_as_a_row_closes() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            l.enter(|s| s.h(0.0))
                .exit(|s| s.h(0.0))
                .transition(Transition::ms(100).linear())
        })
    });
    window(&mut ui);
    let y = |i: usize| r[i].rect().unwrap().y;
    assert_eq!(y(1), ROW);
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!((y(1) - ROW / 2).abs() <= 1, "{}", y(1));
    run_out(&mut ui, start, 50);
    assert_eq!(y(1), 0);
    assert!(!there(r[0]));

    let start = Instant::now();
    items.set(vec![0, 1]);
    ui.update();
    assert_eq!(y(1), 0, "it opens from nothing");
    frame_at(&mut ui, start, 50);
    assert!((y(1) - ROW / 2).abs() <= 1, "{}", y(1));
    run_out(&mut ui, start, 50);
    assert_eq!(y(1), ROW);
}

#[test]
fn a_row_that_is_going_takes_no_pointer_and_no_keys() {
    let items = signal(vec![0]);
    let (under, field) = (node_ref(), node_ref());
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .relative()
            .child(div().absolute().w(100.0).h(100.0).node_ref(under))
            .child(
                for_each(
                    move || items.get(),
                    |i: &usize| *i,
                    move |_| {
                        div()
                            .absolute()
                            .w(100.0)
                            .h(100.0)
                            .child(text_input().w(100.0).node_ref(field))
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
        position: PointF::new(50.0, 90.0),
        modifiers: Modifiers::default(),
    });
    ui.update();
    let hovered = || with_doc(|d| d.state.hovered.first().copied());
    assert_ne!(hovered(), under.get());

    items.set(vec![]);
    ui.update();
    assert_eq!(hovered(), under.get());
    assert_eq!(focused(), None);
    assert!(there(field));
}

/// A row and what is shown in it can both come and go, and it is the same box.
#[test]
fn what_goes_is_as_whoever_makes_it_go_says() {
    let (items, inner) = (signal(vec![0]), signal(true));
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            for_each(
                move || items.get(),
                |i: &usize| *i,
                move |_| {
                    show(
                        move || inner.get(),
                        move || div().w(10.0).h(10.0).node_ref(n),
                    )
                    .exit(|s| s.opacity(0.0))
                    .transition(Transition::ms(100).linear())
                },
            )
            .exit(|s| s.opacity(0.5))
            .transition(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);

    let start = Instant::now();
    items.set(vec![]);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.75).abs() < SOON, "{}", opacity_of(n));
    run_out(&mut ui, start, 50);
    assert!(!there(n));

    let start = Instant::now();
    items.set(vec![0]);
    ui.update();
    inner.set(false);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "{}", opacity_of(n));
    run_out(&mut ui, start, 50);
    assert!(!there(n));
}

/// The same nodes first start going because of the row, and then because of what is in it.
#[test]
fn what_goes_later_goes_as_it_says_itself() {
    let (items, inner) = (signal(vec![0]), signal(true));
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            for_each(
                move || items.get(),
                |i: &usize| *i,
                move |_| {
                    show(
                        move || inner.get(),
                        move || div().w(10.0).h(10.0).node_ref(n),
                    )
                    .exit(|s| s.opacity(0.0))
                    .transition(Transition::ms(100).linear())
                },
            )
            .exit(|s| s.opacity(0.5))
            .transition(Transition::ms(1000).linear()),
        )
    });
    window(&mut ui);
    let id = n.get();
    items.set(vec![]);
    ui.update();
    items.set(vec![0]);
    ui.update();
    let start = Instant::now();
    run_out(&mut ui, start, 0);
    assert_eq!(n.get(), id, "still the same box");

    let start = Instant::now();
    items.set(vec![]);
    ui.update();
    inner.set(false);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!((opacity_of(n) - 0.5).abs() < SOON, "{}", opacity_of(n));
    run_out(&mut ui, start, 50);
    assert!(!there(n));
}

#[test]
fn a_key_given_twice_is_shown_once() {
    let items = signal(vec![0, 1, 0]);
    let r = refs(2);
    let builds = Rc::new(Cell::new(0));
    let (rr, b) = (r.clone(), builds.clone());
    let mut ui = Ui::new(move || list(items, rr.clone(), b.clone(), |l| l));
    window(&mut ui);
    assert_eq!(shown(&r), [0, 1]);
    assert_eq!(builds.get(), 2);
    items.set(vec![1]);
    ui.update();
    assert_eq!(shown(&r), [1]);
}

fn moving_list(l: ForEach<usize, usize>) -> ForEach<usize, usize> {
    l.moves(Transition::ms(100).linear())
}

/// Where it is shown, and where its place is.
fn shown_and_place(n: NodeRef) -> (i32, i32) {
    with_doc(|d| {
        let id = n.get().unwrap();
        (d.window_rect(id).unwrap().y, d.get(id).unwrap().rect.y)
    })
}

#[test]
fn what_stays_goes_to_its_new_place_from_where_it_was() {
    let items = signal(vec![0, 1, 2]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), moving_list));
    window(&mut ui);
    assert_eq!(shown_and_place(r[2]), (2 * ROW, 2 * ROW));

    let start = Instant::now();
    items.set(vec![1, 2]);
    ui.update();
    assert!(!there(r[0]));
    assert_eq!(shown_and_place(r[2]), (2 * ROW, ROW), "it hasn't moved yet");
    assert_eq!(shown_and_place(r[1]), (ROW, 0));

    frame_at(&mut ui, start, 50);
    let (y, place) = shown_and_place(r[2]);
    assert_eq!(place, ROW);
    assert!((y - (ROW + ROW / 2)).abs() <= 1, "{y}");

    run_out(&mut ui, start, 50);
    assert_eq!(shown_and_place(r[2]), (ROW, ROW));
    let drawn_directly = with_doc(|d| !d.get(r[2].get().unwrap()).unwrap().transformed);
    assert!(drawn_directly, "and it ends up as it was");
}

#[test]
fn without_moves_nothing_glides() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), |l| l));
    window(&mut ui);
    items.set(vec![1]);
    ui.update();
    assert_eq!(shown_and_place(r[1]), (0, 0));
    assert!(!ui.animating());

    // Taking no time is the same as not being asked for.
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            l.moves(Transition::ms(0))
        })
    });
    window(&mut ui);
    items.set(vec![1]);
    ui.update();
    assert_eq!(shown_and_place(r[1]), (0, 0));
    assert!(!ui.animating());
}

#[test]
fn reordering_moves_both_ways_and_new_ones_are_just_there() {
    let items = signal(vec![0, 1]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), moving_list));
    window(&mut ui);
    items.set(vec![1, 2, 0]);
    ui.update();
    assert_eq!(shown_and_place(r[0]), (0, 2 * ROW));
    assert_eq!(shown_and_place(r[1]), (ROW, 0));
    assert_eq!(shown_and_place(r[2]), (ROW, ROW), "nothing to come from");
}

#[test]
fn changing_the_list_again_goes_on_from_where_it_is_shown() {
    let items = signal(vec![0, 1, 2]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), moving_list));
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1, 2]);
    ui.update();
    frame_at(&mut ui, start, 50);
    let (reached, _) = shown_and_place(r[2]);
    assert!(reached > ROW && reached < 2 * ROW, "{reached}");

    let again = Instant::now();
    items.set(vec![2]);
    ui.update();
    assert_eq!(shown_and_place(r[2]), (reached, 0), "no jump");
    // It takes all of the time again from there.
    frame_at(&mut ui, again, 50);
    let (y, _) = shown_and_place(r[2]);
    assert!((y - reached / 2).abs() <= 1, "{y} from {reached}");
    run_out(&mut ui, again, 50);
    assert_eq!(shown_and_place(r[2]), (0, 0));
}

#[test]
fn several_changes_before_a_frame_go_from_what_was_on_screen() {
    let items = signal(vec![0, 1, 2]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), moving_list));
    window(&mut ui);
    items.set(vec![1, 2]);
    items.set(vec![2]);
    ui.update();
    assert_eq!(shown_and_place(r[2]), (2 * ROW, 0));
}

#[test]
fn what_follows_what_went_moves_up_once_it_has_gone() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            fading_list(l).moves(Transition::ms(100).linear())
        })
    });
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert_eq!(
        shown_and_place(r[1]),
        (ROW, ROW),
        "its place is still there"
    );

    let mut at = 50;
    while there(r[0]) {
        at += 8;
        frame_at(&mut ui, start, at);
        assert!(at < 1000);
    }
    let gone = Instant::now();
    assert_eq!(shown_and_place(r[1]), (ROW, 0), "and then it moves up");
    frame_at(&mut ui, gone, 50);
    let (y, _) = shown_and_place(r[1]);
    assert!((y - ROW / 2).abs() <= 1, "{y}");
    run_out(&mut ui, gone, 50);
    assert_eq!(shown_and_place(r[1]), (0, 0));
}

/// What goes doesn't wait to arrive anywhere.
#[test]
fn what_goes_is_gone_when_its_exit_is_over_even_if_it_is_moving() {
    let items = signal(vec![0, 1, 2]);
    let r = refs(3);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            fading_list(l).moves(Transition::ms(1000).linear())
        })
    });
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1, 2]);
    ui.update();
    frame_at(&mut ui, start, 10);
    items.set(vec![2]);
    ui.update();
    // The first row left, so the second is moving while it goes.
    let mut at = 10;
    while there(r[0]) {
        at += 8;
        frame_at(&mut ui, start, at);
        assert!(at < 500);
    }
    assert!(at < 500);
    frame_at(&mut ui, start, at + 8);
    frame_at(&mut ui, start, at + 16);
    assert!(!there(r[1]), "gone at {at} ms already");
}

/// Its own `translate` goes on as if nothing else was going on, and the other way around.
#[test]
fn moving_adds_to_what_it_does_on_its_own() {
    let (items, pushed) = (signal(vec![0, 1]), signal(false));
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().flex().flex_col().child(
            for_each(
                move || items.get(),
                |i: &usize| *i,
                move |i| {
                    let row = div().w(100.0).h(ROW as f32);
                    if i == 1 {
                        row.node_ref(n)
                            .transition(Transition::ms(100).linear())
                            .dyn_style(move |s| {
                                s.translate_x(if pushed.get() { 40.0 } else { 0.0 })
                            })
                    } else {
                        row
                    }
                },
            )
            .moves(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);
    let x = || with_doc(|d| d.window_rect(n.get().unwrap()).unwrap().x);

    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    assert_eq!(x(), 0, "moving doesn't start its transition");
    pushed.set(true);
    ui.update();
    frame_at(&mut ui, start, 50);
    assert!((x() - 20).abs() <= 3, "{}", x());
    let (y, _) = shown_and_place(n);
    assert!((y - ROW / 2).abs() <= 1, "{y}");
    run_out(&mut ui, start, 50);
    assert_eq!(x(), 40);
    assert_eq!(shown_and_place(n), (0, 0));
}

/// Rows that close themselves move what follows every frame, with no list change to go from.
#[test]
fn what_follows_a_closing_row_does_not_lag_behind_it() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || {
        list(items, rr.clone(), builds.clone(), |l| {
            l.exit(|s| s.h(0.0))
                .transition(Transition::ms(100).linear())
                .moves(Transition::ms(100).linear())
        })
    });
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    let mut at = 0;
    while ui.animating() {
        at += 8;
        frame_at(&mut ui, start, at);
        let (y, place) = shown_and_place(r[1]);
        assert_eq!(y, place, "at {at} ms");
        assert!(at < 1000);
    }
    assert_eq!(shown_and_place(r[1]), (0, 0));
}

#[test]
fn only_what_can_be_seen_moves() {
    let items = signal((0..200).collect::<Vec<usize>>());
    let r = refs(200);
    let rr = r.clone();
    let mut ui = Ui::new(move || {
        let rr = rr.clone();
        div().h(100.0).overflow_y_scroll().child(
            for_each(
                move || items.get(),
                |i: &usize| *i,
                move |i| div().w(100.0).h(ROW as f32).node_ref(rr[i]),
            )
            .moves(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);
    let moved = |i: usize| with_doc(|d| d.get(r[i].get().unwrap()).unwrap().computed.move_y != 0.0);
    items.update(|l| {
        l.remove(0);
    });
    ui.update();
    assert!(moved(1), "at the top");
    assert!(moved(9), "still in view");
    assert!(moved(10), "comes into view");
    assert!(!moved(11), "just out of view");
    assert!(!moved(199));
    assert_eq!(shown_and_place(r[11]).1, 10 * ROW);

    // And what leaves the view on the way.
    let start = Instant::now();
    run_out(&mut ui, start, 500);
    items.update(|l| l.insert(0, 0));
    ui.update();
    assert!(!moved(0), "it is new again, nothing to come from");
    assert!(moved(10), "was in view, and moves out of it");
    assert!(!moved(11), "wasn't, and isn't");
}

#[test]
fn what_is_moving_takes_the_pointer_where_it_is_shown() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), moving_list));
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    frame_at(&mut ui, start, 1);
    // Its place is at the top; it is still shown below that.
    let position = PointF::new(50.0, ROW as f32 + 5.0);
    ui.handle_event(WindowEvent::PointerMoved {
        position,
        modifiers: Modifiers::default(),
    });
    ui.update();
    let hovered = with_doc(|d| d.state.hovered.first().copied());
    assert_eq!(hovered, r[1].get());
}

#[test]
fn what_is_moving_goes_with_its_list() {
    let (items, on) = (signal(vec![0, 1]), signal(true));
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(show(
            move || on.get(),
            move || {
                for_each(
                    move || items.get(),
                    |i: &usize| *i,
                    move |_| div().w(10.0).h(10.0).node_ref(n),
                )
                .moves(Transition::ms(100).linear())
            },
        ))
    });
    window(&mut ui);
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    on.set(false);
    ui.update();
    assert!(!there(n));
    run_out(&mut ui, start, 0);
    assert!(!ui.animating());
}

/// A row with a `transition` of its own is one that style resolution looks at for transitions.
#[test]
fn a_row_with_a_transition_of_its_own_gets_there_too() {
    let (items, pushed) = (signal(vec![0, 1]), signal(false));
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().flex().flex_col().child(
            for_each(
                move || items.get(),
                |i: &usize| *i,
                move |i| {
                    let row = div().w(100.0).h(ROW as f32);
                    if i == 1 {
                        row.node_ref(n)
                            .transition(Transition::ms(200).linear())
                            .dyn_style(move |s| {
                                s.translate_x(if pushed.get() { 40.0 } else { 0.0 })
                            })
                    } else {
                        row
                    }
                },
            )
            .moves(Transition::ms(50).linear()),
        )
    });
    window(&mut ui);
    let x = || with_doc(|d| d.window_rect(n.get().unwrap()).unwrap().x);

    // Nothing else going on: it mustn't stop halfway.
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    frame_at(&mut ui, start, 25);
    assert!(ui.animating(), "it is under way");
    let (y, _) = shown_and_place(n);
    assert!((y - ROW / 2).abs() <= 1, "{y}");
    frame_at(&mut ui, start, 40);
    let (y, _) = shown_and_place(n);
    assert!(y < ROW / 2, "{y}");
    run_out(&mut ui, start, 40);
    assert_eq!(shown_and_place(n), (0, 0));

    // Its own transition goes on when moving is over before it.
    items.set(vec![0, 1]);
    let start = Instant::now();
    ui.update();
    pushed.set(true);
    ui.update();
    frame_at(&mut ui, start, 100);
    assert_eq!(shown_and_place(n), (ROW, ROW), "there");
    assert!((x() - 20).abs() <= 3, "{}", x());
    frame_at(&mut ui, start, 150);
    assert!((x() - 30).abs() <= 3, "{}", x());
    run_out(&mut ui, start, 150);
    assert_eq!(x(), 40);
}

#[test]
fn moving_paints_nothing_again() {
    let items = signal(vec![0, 1]);
    let r = refs(2);
    let (rr, builds) = (r.clone(), Rc::new(Cell::new(0)));
    let mut ui = Ui::new(move || list(items, rr.clone(), builds.clone(), moving_list));
    window(&mut ui);
    let layer = |scene: &Scene| {
        scene.primitives.iter().find_map(|p| match p {
            Primitive::Layer(l) => Some((l.content.clone(), l.transform.f)),
            _ => None,
        })
    };
    let start = Instant::now();
    items.set(vec![1]);
    ui.update();
    let (first, from) = layer(&frame_at(&mut ui, start, 20)).unwrap();
    let (second, to) = layer(&frame_at(&mut ui, start, 40)).unwrap();
    assert_ne!(from, to, "it does move");
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

/// What wasn't shown wasn't anywhere: it has nothing to come from.
#[test]
fn what_was_hidden_is_just_there() {
    let (items, hidden) = (signal(vec![0, 1, 2]), signal(false));
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().flex().flex_col().child(
            for_each(
                move || items.get(),
                |i: &usize| *i,
                move |i| {
                    let row = div().w(100.0).h(ROW as f32);
                    if i == 2 {
                        row.node_ref(n)
                            .dyn_style(move |s| if hidden.get() { s.hidden() } else { s })
                    } else {
                        row
                    }
                },
            )
            .moves(Transition::ms(100).linear()),
        )
    });
    window(&mut ui);
    hidden.set(true);
    ui.update();
    let _ = ui.paint(Instant::now());
    batch(|| {
        items.set(vec![1, 2]);
        hidden.set(false);
    });
    ui.update();
    assert_eq!(shown_and_place(n), (ROW, ROW));
}
