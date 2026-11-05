//! Moving the items of a list. See `docs/design/23-drag-and-drop.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::with_doc;
use anti_webui_widgets::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn moved(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

fn button(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

fn press(ui: &mut Ui, x: f32, y: f32) {
    moved(ui, x, y);
    button(ui, x, y, true);
}

/// Four rows 40 high, with 10 between them, or as high as `heights` says.
fn list(items: Signal<Vec<u32>>, heights: fn(u32) -> f32, refs: Rc<[NodeRef; 4]>) -> Div {
    sortable_list(
        items,
        |v| *v,
        move |v, _| {
            div()
                .w(100.0)
                .h(heights(v))
                .node_ref(refs[v as usize])
                .on_click(|_| {})
        },
    )
    .gap(10.0)
}

fn same(_: u32) -> f32 {
    40.0
}

fn refs() -> Rc<[NodeRef; 4]> {
    Rc::new([node_ref(), node_ref(), node_ref(), node_ref()])
}

fn ys(ui: &mut Ui, refs: &[NodeRef; 4]) -> Vec<i32> {
    ui.update();
    refs.iter().map(|r| r.rect().unwrap().y).collect()
}

fn settle(ui: &mut Ui) {
    let start = Instant::now();
    for i in 1..200 {
        ui.animate(start + Duration::from_millis(i * 8));
        ui.update();
    }
}

fn layouts_during(ui: &mut Ui, f: impl FnOnce(&mut Ui)) -> bool {
    let before: Vec<_> = with_doc(|d| d.nodes.iter().map(|(id, n)| (id, n.rect)).collect());
    f(ui);
    with_doc(|d| {
        before
            .iter()
            .any(|(id, r)| d.get(*id).is_some_and(|n| n.rect != *r))
    })
}

#[test]
fn others_make_room_and_the_list_changes_when_let_go() {
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, same, rr));
    assert_eq!(ys(&mut ui, &r), [0, 50, 100, 150]);
    press(&mut ui, 50.0, 20.0);
    moved(&mut ui, 50.0, 80.0);
    moved(&mut ui, 50.0, 90.0);
    assert_eq!(items.get_untracked(), [0, 1, 2, 3], "not until let go");
    settle(&mut ui);
    // What is held is at its place plus 70, what was after it went up.
    assert_eq!(ys(&mut ui, &r), [70, 0, 100, 150]);
    button(&mut ui, 50.0, 90.0, false);
    assert_eq!(items.get_untracked(), [1, 0, 2, 3]);
    settle(&mut ui);
    assert_eq!(ys(&mut ui, &r), [50, 0, 100, 150]);
}

#[test]
fn nothing_jumps_when_it_is_let_go() {
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, same, rr));
    press(&mut ui, 50.0, 20.0);
    moved(&mut ui, 50.0, 90.0);
    settle(&mut ui);
    let before = ys(&mut ui, &r);
    button(&mut ui, 50.0, 90.0, false);
    assert_eq!(ys(&mut ui, &r), before);
}

#[test]
fn moving_it_only_changes_transforms() {
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, same, rr));
    press(&mut ui, 50.0, 20.0);
    let moved_rects = layouts_during(&mut ui, |ui| {
        moved(ui, 50.0, 90.0);
        moved(ui, 50.0, 160.0);
        settle(ui);
    });
    assert!(!moved_rects, "the list stays laid out as it was");
}

#[test]
fn it_goes_up_as_well() {
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, same, rr));
    press(&mut ui, 50.0, 170.0);
    moved(&mut ui, 50.0, 40.0);
    settle(&mut ui);
    // Its middle is at 40, above the middle of the second row.
    assert_eq!(ys(&mut ui, &r), [0, 100, 150, 20]);
    button(&mut ui, 50.0, 40.0, false);
    assert_eq!(items.get_untracked(), [0, 3, 1, 2]);
    settle(&mut ui);
    assert_eq!(ys(&mut ui, &r), [0, 100, 150, 50]);
}

#[test]
fn rows_of_other_heights_go_where_they_fit() {
    fn varied(v: u32) -> f32 {
        [20.0, 60.0, 30.0, 40.0][v as usize]
    }
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, varied, rr));
    assert_eq!(ys(&mut ui, &r), [0, 30, 100, 140]);
    press(&mut ui, 50.0, 10.0);
    // Its middle goes past the middle of the second one.
    moved(&mut ui, 50.0, 70.0);
    button(&mut ui, 50.0, 70.0, false);
    assert_eq!(items.get_untracked(), [1, 0, 2, 3]);
    settle(&mut ui);
    assert_eq!(ys(&mut ui, &r), [70, 0, 100, 140]);
}

#[test]
fn escape_leaves_it_as_it_was() {
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, same, rr));
    press(&mut ui, 50.0, 20.0);
    moved(&mut ui, 50.0, 90.0);
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Escape),
        text: None,
        modifiers: Modifiers::default(),
        pressed: true,
        repeat: false,
    }));
    button(&mut ui, 50.0, 90.0, false);
    assert_eq!(items.get_untracked(), [0, 1, 2, 3]);
    settle(&mut ui);
    assert_eq!(ys(&mut ui, &r), [0, 50, 100, 150]);
}

#[test]
fn clicks_on_a_row_still_arrive() {
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let items = signal(vec![0u32, 1]);
    let mut ui = mount(|| {
        sortable_list(
            items,
            |v| *v,
            move |_, _| {
                let c = c.clone();
                div().w(100.0).h(40.0).on_click(move |_| c.set(c.get() + 1))
            },
        )
    });
    press(&mut ui, 50.0, 20.0);
    button(&mut ui, 50.0, 20.0, false);
    assert_eq!(clicks.get(), 1);
    press(&mut ui, 50.0, 20.0);
    moved(&mut ui, 50.0, 50.0);
    button(&mut ui, 50.0, 50.0, false);
    assert_eq!(clicks.get(), 1, "that was moving it");
}

#[test]
fn only_the_grip_moves_it_when_there_is_one() {
    let items = signal(vec![0u32, 1]);
    let mut ui = mount(|| {
        sortable_list(
            items,
            |v| *v,
            move |_, row| {
                div()
                    .flex()
                    .w(200.0)
                    .h(40.0)
                    .child(row.grip(div().w(40.0).h(40.0)))
                    .child(div().w(160.0).h(40.0))
            },
        )
    });
    press(&mut ui, 100.0, 20.0);
    moved(&mut ui, 100.0, 60.0);
    button(&mut ui, 100.0, 60.0, false);
    assert_eq!(items.get_untracked(), [0, 1], "not by the rest of it");
    press(&mut ui, 20.0, 20.0);
    moved(&mut ui, 20.0, 70.0);
    button(&mut ui, 20.0, 70.0, false);
    assert_eq!(items.get_untracked(), [1, 0]);
}

#[test]
fn alt_and_the_arrows_move_it() {
    let (items, r) = (signal(vec![0, 1, 2, 3]), refs());
    let (i, rr) = (items, r.clone());
    let mut ui = mount(|| list(i, same, rr));
    press(&mut ui, 50.0, 20.0);
    button(&mut ui, 50.0, 20.0, false);
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    let arrow = |ui: &mut Ui, key| {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(key),
            text: None,
            modifiers: alt,
            pressed: true,
            repeat: false,
        }));
        ui.update();
    };
    arrow(&mut ui, NamedKey::ArrowDown);
    assert_eq!(items.get_untracked(), [1, 0, 2, 3]);
    arrow(&mut ui, NamedKey::ArrowDown);
    assert_eq!(items.get_untracked(), [1, 2, 0, 3]);
    arrow(&mut ui, NamedKey::ArrowUp);
    assert_eq!(items.get_untracked(), [1, 0, 2, 3]);
    arrow(&mut ui, NamedKey::ArrowUp);
    arrow(&mut ui, NamedKey::ArrowUp);
    assert_eq!(items.get_untracked(), [0, 1, 2, 3], "and stops at the end");
}

#[test]
fn it_goes_across_too() {
    let items = signal(vec![0u32, 1, 2]);
    let mut ui = mount(|| sortable_row(items, |v| *v, |_, _| div().w(50.0).h(40.0)).gap(10.0));
    press(&mut ui, 20.0, 20.0);
    moved(&mut ui, 150.0, 20.0);
    button(&mut ui, 150.0, 20.0, false);
    assert_eq!(items.get_untracked(), [1, 2, 0]);
}

#[test]
fn it_stays_with_the_pointer_when_what_contains_it_scrolls() {
    let scroller = node_ref();
    let items = signal((0..40u32).collect::<Vec<_>>());
    let first = node_ref();
    let mut ui = mount(|| {
        div()
            .h(200.0)
            .overflow_y_scroll()
            .node_ref(scroller)
            .child(sortable_list(
                items,
                |v| *v,
                move |v, _| {
                    let el = div().w(100.0).h(40.0);
                    if v == 0 { el.node_ref(first) } else { el }
                },
            ))
    });
    press(&mut ui, 50.0, 20.0);
    moved(&mut ui, 50.0, 190.0);
    let start = Instant::now();
    for i in 1..60 {
        ui.animate(start + Duration::from_millis(i * 16));
        ui.update();
    }
    assert!(scroller.scroll_offset().y > 0.0);
    // The row is 170 down from where it was grabbed, whatever scrolled.
    assert_eq!(first.rect().unwrap().y, 170);
    button(&mut ui, 50.0, 190.0, false);
    assert!(items.get_untracked()[0] != 0);
}
