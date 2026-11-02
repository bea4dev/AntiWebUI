//! The paint order of `z_index` and `absolute`, and hit testing that agrees with it.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use std::time::Instant;

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::ONE,
    });
    ui.update();
    ui
}

/// The red channel of each box's background in paint order; tests use it as a name. Scrollbars
/// have none.
fn order_in(scene: &Scene, out: &mut Vec<u32>) {
    for p in &scene.primitives {
        match p {
            Primitive::Quad(q) if q.background.r > 0.0 => {
                out.push((q.background.r * 255.0).round() as u32)
            }
            Primitive::Layer(l) => order_in(&l.content, out),
            _ => {}
        }
    }
}

fn order(ui: &mut Ui) -> Vec<u32> {
    ui.update();
    let mut out = Vec::new();
    order_in(&ui.paint(Instant::now()), &mut out);
    out
}

fn name(n: u32) -> Color {
    Color::new(n as f32 / 255.0, 0.0, 0.0, 1.0)
}

fn tile(n: u32) -> Div {
    div().h(40.0).w(40.0).bg(name(n))
}

fn hover(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

/// Reports which of the nodes is under the pointer.
fn watching(hovered: Signal<u32>, n: u32, d: Div) -> Div {
    d.on_hover(move |inside| {
        if inside {
            hovered.set(n);
        }
    })
}

#[test]
fn z_index_orders_siblings_around_the_ones_without() {
    let mut ui = mount(|| {
        div()
            .child(tile(1).z_index(2))
            .child(tile(2))
            .child(tile(3).z_index(-1))
            .child(tile(4).z_index(1))
            .child(tile(5))
    });
    assert_eq!(order(&mut ui), [3, 2, 5, 4, 1]);
}

#[test]
fn absolute_goes_above_what_follows_it_without_a_z_index() {
    let mut ui = mount(|| {
        div()
            .relative()
            .child(tile(1).absolute())
            .child(tile(2))
            .child(tile(3).absolute().z_index(0))
            .child(tile(4))
    });
    assert_eq!(order(&mut ui), [2, 4, 1, 3]);
}

#[test]
fn a_stacking_context_keeps_its_descendants_together() {
    let mut ui = mount(|| {
        div()
            // Its child's high number only counts inside of it.
            .child(tile(1).z_index(1).child(tile(2).z_index(100)))
            .child(tile(3).z_index(2))
            // Without a context, its child is ordered with the others above.
            .child(tile(4).absolute().child(tile(5).z_index(5)))
            .child(tile(6).z_index(3))
    });
    assert_eq!(order(&mut ui), [4, 1, 2, 3, 6, 5]);
}

#[test]
fn negative_z_goes_above_its_contexts_background() {
    let mut ui = mount(|| {
        div()
            .child(tile(1).z_index(0).child(tile(2).z_index(-1)).child(tile(3)))
            .child(tile(4).child(tile(5).z_index(-1)))
    });
    // The first is a context: 2 stays above its background. The other isn't: 5 goes to the root.
    assert_eq!(order(&mut ui), [5, 4, 1, 2, 3]);
}

#[test]
fn translucency_makes_a_stacking_context() {
    let mut ui = mount(|| {
        div()
            .child(tile(1).opacity(0.5).child(tile(2).z_index(9)))
            .child(tile(3).z_index(1))
    });
    assert_eq!(order(&mut ui), [1, 2, 3]);
    // And what is in it gets its opacity, wherever it is painted from.
    ui.update();
    let scene = ui.paint(Instant::now());
    let alpha: Vec<f32> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) => Some(q.background.a),
            _ => None,
        })
        .collect();
    assert_eq!(alpha, [0.5, 0.5, 1.0]);
}

#[test]
fn a_scroller_keeps_its_content_together_whether_or_not_it_overflows() {
    let big = signal(false);
    let mut ui = mount(move || {
        div()
            .child(
                div()
                    .h(100.0)
                    .overflow_y_scroll()
                    .child(tile(1).z_index(9))
                    .child(div().dyn_style(move |s| s.h(if big.get() { 500.0 } else { 1.0 }))),
            )
            .child(tile(2).z_index(1))
    });
    assert_eq!(order(&mut ui), [1, 2]);
    big.set(true);
    assert_eq!(order(&mut ui), [1, 2]);
}

#[test]
fn changes_take_effect_and_damage_what_moved() {
    let z = signal(0);
    let hovered = signal(0);
    let mut ui = mount(move || {
        div()
            .child(watching(
                hovered,
                1,
                tile(1)
                    .ml(10.0)
                    .dyn_style(move |s| if z.get() != 0 { s.z_index(z.get()) } else { s }),
            ))
            .child(watching(hovered, 2, tile(2).ml(10.0).mt(-20.0)))
            .child(tile(3).ml(200.0))
    });
    assert_eq!(order(&mut ui), [1, 2, 3]);
    hover(&mut ui, 20.0, 30.0);
    assert_eq!(hovered.get(), 2);

    let now = Instant::now();
    ui.update();
    let before = ui.paint(now);
    z.set(1);
    assert_eq!(order(&mut ui), [2, 3, 1]);
    let after = ui.paint(now);
    // Only the two that changed places; the far one stays.
    let d = damage::diff(&before, &after).bounds();
    assert!(d.right() <= 50 && d.bottom() <= 60, "{d:?}");
    assert!(!d.is_empty());
    // What is on top takes the input.
    hover(&mut ui, 20.0, 35.0);
    hover(&mut ui, 20.0, 30.0);
    assert_eq!(hovered.get(), 1);

    z.set(-1);
    assert_eq!(order(&mut ui), [1, 2, 3]);
    hover(&mut ui, 20.0, 45.0);
    hover(&mut ui, 20.0, 30.0);
    assert_eq!(hovered.get(), 2);
}

#[test]
fn hit_testing_finds_what_is_below_a_transparent_top() {
    let hovered = signal(0);
    let mut ui = mount(move || {
        // A stacking context, or its own box would cover what has a negative z.
        div()
            .z_index(0)
            .relative()
            .size_full()
            .child(watching(hovered, 1, tile(1).w(100.0).h(100.0)))
            .child(watching(
                hovered,
                2,
                tile(2).absolute().top(0.0).left(0.0).pointer_events_none(),
            ))
            .child(watching(
                hovered,
                3,
                tile(3).absolute().top(90.0).left(90.0).z_index(-1),
            ))
    });
    hover(&mut ui, 10.0, 10.0);
    assert_eq!(hovered.get(), 1, "pointer_events_none lets it through");
    hover(&mut ui, 120.0, 120.0);
    assert_eq!(
        hovered.get(),
        3,
        "below the normal flow, but nothing is there"
    );
    hover(&mut ui, 95.0, 95.0);
    assert_eq!(hovered.get(), 1, "the normal flow covers negative z");
}

#[test]
fn clips_still_apply_above_the_rest() {
    let hovered = signal(0);
    let mut ui = mount(move || {
        div()
            .child(div().h(20.0).w(100.0).overflow_clip().child(watching(
                hovered,
                1,
                tile(1).z_index(5).h(100.0),
            )))
            .child(watching(hovered, 2, tile(2)))
    });
    hover(&mut ui, 10.0, 10.0);
    assert_eq!(hovered.get(), 1);
    hover(&mut ui, 10.0, 30.0);
    assert_eq!(
        hovered.get(),
        2,
        "clipped away here, even though it is on top"
    );
    ui.update();
    let scene = ui.paint(Instant::now());
    let top = scene
        .primitives
        .iter()
        .rev()
        .find_map(|p| match p {
            Primitive::Quad(q) => Some(q.clip.rect.height),
            _ => None,
        })
        .unwrap();
    assert_eq!(top, 20);
}

#[test]
fn removed_nodes_leave_the_lists() {
    let shown = signal(true);
    let mut ui = mount(move || {
        div().child(tile(1)).child(show(
            move || shown.get(),
            || tile(2).absolute().child(tile(3).z_index(1)),
        ))
    });
    assert_eq!(order(&mut ui), [1, 2, 3]);
    shown.set(false);
    assert_eq!(order(&mut ui), [1]);
    shown.set(true);
    assert_eq!(order(&mut ui), [1, 2, 3]);
}
