//! Transforms: what gets painted again, what moves, and what the pointer finds.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Layer, Primitive, Scene};
use anti_webui_core::text::selection;
use anti_webui_core::tree::{NodeKind, with_doc};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

fn window(ui: &mut Ui, w: i32, h: i32) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w, h),
        scale: Scale::ONE,
    });
    ui.update();
}

fn frame(ui: &mut Ui) -> Scene {
    ui.update();
    ui.paint(Instant::now())
}

fn layers(scene: &Scene) -> Vec<&Layer> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Layer(l) => Some(l),
            _ => None,
        })
        .collect()
}

fn hovered_by(ui: &mut Ui, x: f32, y: f32, n: NodeRef) -> bool {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
    with_doc(|d| d.state.hovered.first().copied() == n.get())
}

#[test]
fn only_a_transform_makes_a_layer() {
    let dx = signal(0.0_f32);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(50.0)
                .h(50.0)
                .bg(rgb(0xff0000))
                .dyn_style(move |s| s.translate_x(dx.get())),
        )
    });
    window(&mut ui, 200, 200);
    assert!(layers(&frame(&mut ui)).is_empty());
    dx.set(10.0);
    assert_eq!(layers(&frame(&mut ui)).len(), 1);
    dx.set(0.0);
    assert!(
        layers(&frame(&mut ui)).is_empty(),
        "back to painting it directly"
    );
}

#[test]
fn moving_paints_nothing_again_and_damages_only_where_it_was_and_is() {
    let dx = signal(10.0_f32);
    let block = node_ref();
    let sibling = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .bg(rgb(0xff0000))
                    .node_ref(block)
                    .dyn_style(move |s| s.translate_x(dx.get()))
                    .child(div().w(10.0).h(10.0).bg(rgb(0x00ff00))),
            )
            .child(div().w(30.0).h(30.0).node_ref(sibling))
    });
    window(&mut ui, 400, 200);
    let a = frame(&mut ui);
    let sibling_at = sibling.rect().unwrap();
    dx.set(100.0);
    let b = frame(&mut ui);
    assert!(
        Arc::ptr_eq(&layers(&a)[0].content, &layers(&b)[0].content),
        "the content is what it was"
    );
    assert_eq!(sibling.rect().unwrap(), sibling_at, "layout doesn't know");
    assert_eq!(block.rect().unwrap(), RectI::new(100, 0, 50, 50));
    let d = damage::diff(&a, &b);
    assert!(d.area() <= (55 * 55 * 2) as i64, "{d:?}");
    assert_eq!(d.bounds(), RectI::new(10, 0, 140, 50));
}

#[test]
fn fading_a_layer_is_compositing_it_as_one_image() {
    let fade = signal(1.0_f32);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(50.0)
                .h(50.0)
                .bg(rgb(0xff0000))
                .translate(1.0, 0.0)
                .dyn_style(move |s| s.opacity(fade.get())),
        )
    });
    window(&mut ui, 200, 200);
    let a = frame(&mut ui);
    fade.set(0.5);
    let b = frame(&mut ui);
    let (la, lb) = (layers(&a)[0], layers(&b)[0]);
    assert!(Arc::ptr_eq(&la.content, &lb.content));
    assert_eq!(lb.opacity, 0.5);
    assert_eq!(
        damage::diff(&a, &b).bounds(),
        lb.bounds_on_parent(),
        "it looks different"
    );
    let Primitive::Quad(q) = &lb.content.primitives[0] else {
        panic!("{:?}", lb.content.primitives[0]);
    };
    assert_eq!(q.background, rgb(0xff0000), "not faded by itself");
}

#[test]
fn a_change_inside_damages_only_that_part() {
    let color = signal(0xff0000_u32);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(200.0)
                .h(200.0)
                .bg(rgb(0xffffff))
                .translate(30.0, 0.0)
                .child(
                    div()
                        .w(10.0)
                        .h(10.0)
                        .dyn_style(move |s| s.bg(rgb(color.get()))),
                ),
        )
    });
    window(&mut ui, 400, 400);
    let a = frame(&mut ui);
    color.set(0x00ff00);
    let b = frame(&mut ui);
    assert_eq!(damage::diff(&a, &b).bounds(), RectI::new(30, 0, 10, 10));
}

#[test]
fn the_pointer_finds_what_is_shown_where_it_is_shown() {
    let spin = signal(0.0_f32);
    let zoom = signal(1.0_f32);
    let block = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .ml(100.0)
                .mt(100.0)
                .w(100.0)
                .h(20.0)
                .node_ref(block)
                .dyn_style(move |s| s.rotate(spin.get()).scale(zoom.get())),
        )
    });
    window(&mut ui, 400, 400);
    frame(&mut ui);
    assert!(hovered_by(&mut ui, 190.0, 110.0, block));
    assert!(!hovered_by(&mut ui, 150.0, 150.0, block));

    // Around its center (150, 110), standing up.
    spin.set(90.0);
    frame(&mut ui);
    assert!(hovered_by(&mut ui, 150.0, 150.0, block));
    assert!(!hovered_by(&mut ui, 190.0, 110.0, block));
    let r = block.rect().unwrap();
    assert_eq!((r.x, r.y, r.width, r.height), (140, 60, 20, 100));

    spin.set(0.0);
    zoom.set(2.0);
    frame(&mut ui);
    assert!(hovered_by(&mut ui, 55.0, 110.0, block), "twice as long");
    assert!(!hovered_by(&mut ui, 45.0, 110.0, block));
}

#[test]
fn something_flattened_away_is_not_there() {
    let zoom = signal(1.0_f32);
    let block = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(100.0)
                .h(100.0)
                .bg(rgb(0xff0000))
                .node_ref(block)
                .dyn_style(move |s| s.scale(zoom.get())),
        )
    });
    window(&mut ui, 200, 200);
    assert!(hovered_by(&mut ui, 50.0, 50.0, block));
    zoom.set(0.0);
    let scene = frame(&mut ui);
    assert!(layers(&scene).is_empty(), "{:?}", scene.primitives);
    assert!(!hovered_by(&mut ui, 50.0, 50.0, block));
    zoom.set(1.0);
    let scene = frame(&mut ui);
    assert!(
        scene
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Quad(_)))
    );
}

#[test]
fn moving_under_a_still_pointer_changes_what_is_hovered() {
    let dx = signal(0.0_f32);
    let hovered = Rc::new(Cell::new(false));
    let seen = hovered.clone();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(50.0)
                .h(50.0)
                .on_hover(move |h| seen.set(h))
                .dyn_style(move |s| s.translate(dx.get(), 1.0)),
        )
    });
    window(&mut ui, 300, 100);
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(25.0, 25.0),
        modifiers: Modifiers::default(),
    });
    frame(&mut ui);
    assert!(hovered.get());
    dx.set(100.0);
    frame(&mut ui);
    assert!(!hovered.get());
}

#[test]
fn clips_outside_apply_to_the_layer_as_a_whole() {
    let inner = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div().w(100.0).h(100.0).overflow_clip().child(
                div()
                    .w(100.0)
                    .h(100.0)
                    .bg(rgb(0xff0000))
                    .node_ref(inner)
                    .translate(60.0, 0.0),
            ),
        )
    });
    window(&mut ui, 400, 400);
    let scene = frame(&mut ui);
    let l = layers(&scene)[0];
    assert_eq!(l.clip.rect, RectI::new(0, 0, 100, 100));
    assert_eq!(l.bounds_on_parent(), RectI::new(60, 0, 40, 100));
    assert!(hovered_by(&mut ui, 90.0, 50.0, inner));
    assert!(!hovered_by(&mut ui, 110.0, 50.0, inner), "clipped away");
}

#[test]
fn a_translation_between_pixels_settles_on_whole_pixels() {
    let dx = signal(0.0_f32);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(50.0)
                .h(50.0)
                .bg(rgb(0xff0000))
                .dyn_style(move |s| s.translate_x(dx.get())),
        )
    });
    window(&mut ui, 200, 200);
    dx.set(10.25);
    let moving = frame(&mut ui);
    assert_eq!(
        layers(&moving)[0].transform.e,
        10.25,
        "smooth while it moves"
    );
    assert!(ui.animating(), "needs one more frame");
    let settled = frame(&mut ui);
    assert_eq!(layers(&settled)[0].transform.e, 10.0);
    assert!(!ui.animating());
    assert!(!damage::diff(&moving, &settled).is_empty());
    let again = frame(&mut ui);
    assert!(damage::diff(&settled, &again).is_empty());
    // Moving again goes back to where it really is.
    dx.set(10.5);
    assert_eq!(layers(&frame(&mut ui))[0].transform.e, 10.5);
}

#[test]
fn a_transform_in_scrolled_content_is_a_layer_in_the_layer() {
    let dx = signal(5.0_f32);
    let inside = node_ref();
    let scroller = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(200.0)
                .h(100.0)
                .overflow_y_scroll()
                .node_ref(scroller)
                .child(
                    div().h(500.0).child(
                        div()
                            .mt(50.0)
                            .w(40.0)
                            .h(40.0)
                            .bg(rgb(0xff0000))
                            .node_ref(inside)
                            .dyn_style(move |s| s.translate_x(dx.get())),
                    ),
                ),
        )
    });
    window(&mut ui, 300, 300);
    let a = frame(&mut ui);
    let (outer_a, inner_a) = (layers(&a)[0], layers(&layers(&a)[0].content)[0]);
    dx.set(30.0);
    let b = frame(&mut ui);
    let (outer_b, inner_b) = (layers(&b)[0], layers(&layers(&b)[0].content)[0]);
    assert!(
        !Arc::ptr_eq(&outer_a.content, &outer_b.content),
        "it holds the transform"
    );
    assert!(
        Arc::ptr_eq(&inner_a.content, &inner_b.content),
        "what moves stays"
    );
    assert_eq!(
        damage::diff(&a, &b).bounds(),
        RectI::new(5, 50, 65, 40),
        "only where it was and is"
    );

    // Where it shows follows the scrolling.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(10.0, 10.0),
        delta_x: 0.0,
        delta_y: 30.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    frame(&mut ui);
    assert_eq!(inside.rect().unwrap(), RectI::new(30, 20, 40, 40));
    assert!(hovered_by(&mut ui, 50.0, 40.0, inside));
}

#[test]
fn a_scroller_in_a_transform_is_part_of_its_content() {
    let scroller = node_ref();
    let row = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(100.0)
                .h(50.0)
                .overflow_y_scroll()
                .node_ref(scroller)
                .translate(100.0, 0.0)
                .child(div().h(40.0))
                .child(div().h(40.0).node_ref(row).bg(rgb(0xff0000))),
        )
    });
    window(&mut ui, 300, 300);
    let scene = frame(&mut ui);
    assert_eq!(layers(&scene).len(), 1);
    assert!(layers(&layers(&scene)[0].content).is_empty());
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(150.0, 10.0),
        delta_x: 0.0,
        delta_y: 30.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    frame(&mut ui);
    assert_eq!(row.rect().unwrap(), RectI::new(100, 10, 100, 40));
    assert!(hovered_by(&mut ui, 150.0, 20.0, row));
}

#[test]
fn transforms_add_up() {
    let block = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div().w(100.0).h(100.0).translate(100.0, 0.0).child(
                div()
                    .w(100.0)
                    .h(100.0)
                    .scale(2.0)
                    .transform_origin(0.0, 0.0)
                    .child(div().ml(10.0).mt(10.0).w(10.0).h(10.0).node_ref(block)),
            ),
        )
    });
    window(&mut ui, 500, 500);
    frame(&mut ui);
    assert_eq!(block.rect().unwrap(), RectI::new(120, 20, 20, 20));
    assert!(hovered_by(&mut ui, 125.0, 25.0, block));
}

#[test]
fn a_transform_makes_a_stacking_context() {
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div().w(100.0).h(100.0).translate(1.0, 0.0).child(
                div()
                    .w(10.0)
                    .h(10.0)
                    .absolute()
                    .z_index(5)
                    .bg(rgb(0x00ff00)),
            ),
        )
    });
    window(&mut ui, 200, 200);
    let scene = frame(&mut ui);
    let l = layers(&scene)[0];
    assert!(
        l.content
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Quad(_))),
        "what is above in the transform stays in it"
    );
}

#[test]
fn selecting_text_goes_through_the_transform() {
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .ml(100.0)
                .w(100.0)
                .h(20.0)
                .scale_x(2.0)
                .transform_origin(0.0, 0.0)
                .child(text("abcdefghij")),
        )
    });
    window(&mut ui, 500, 100);
    frame(&mut ui);
    let found = |x: f32| {
        with_doc(|d| selection::hit_text_point(d, PointF::new(x, 5.0)).map(|t| t.offset)).unwrap()
    };
    let in_text = |x: f32| {
        with_doc(|d| {
            let id = d
                .preorder()
                .into_iter()
                .find(|i| d.get(*i).unwrap().is_text())
                .unwrap();
            let NodeKind::Text(t) = &d.get(id).unwrap().kind else {
                unreachable!()
            };
            t.layout.hit(x, 5.0).0
        })
    };
    for local in [3.0, 15.0, 30.0, 45.0] {
        // Twice as far from where the text starts.
        assert_eq!(found(100.0 + local * 2.0), in_text(local), "{local}");
    }
    assert!(
        in_text(30.0) > 0 && in_text(30.0) != found(130.0),
        "the test tells them apart"
    );
}

#[test]
fn a_field_in_a_transform_puts_the_caret_where_it_is_clicked() {
    let value = signal("abcdefghij".to_string());
    let field = node_ref();
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .translate(200.0, 50.0)
                .w(200.0)
                .child(text_input().w(200.0).node_ref(field).value(value)),
        )
    });
    window(&mut ui, 600, 200);
    frame(&mut ui);
    let r = field.rect().unwrap();
    assert_eq!((r.x, r.y), (200, 50));
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: PointF::new(r.right() as f32 - 1.0, r.y as f32 + 5.0),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    frame(&mut ui);
    let focus = with_doc(|d| d.get(field.get().unwrap()).unwrap().edit().unwrap().focus);
    assert_eq!(focus, 10, "past the end of the text");
}
