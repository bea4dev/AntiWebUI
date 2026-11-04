//! How filters and backdrops take part in painting and damage (see `docs/design/16-filters.md`).

use anti_webui_core::Ui;
use anti_webui_core::damage::{self, Damage, backdrop_read, expand_for_backdrops};
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Layer, Primitive, Scene};
use std::sync::Arc;
use std::time::Instant;

fn sized(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 400),
        scale: Scale::ONE,
    });
    ui.update();
}

fn only_layer(scene: &Scene) -> &Layer {
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Layer(l) => Some(l),
            _ => None,
        })
        .expect("a layer")
}

#[test]
fn a_filter_makes_a_layer_that_reaches_as_far_as_it_does() {
    let mut ui = Ui::new(|| {
        div().child(
            div()
                .absolute()
                .left(100.0)
                .top(100.0)
                .w(50.0)
                .h(50.0)
                .bg(rgb(0))
                .blur(4.0),
        )
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let layer = only_layer(&scene);
    let filter = layer.filter.as_ref().expect("a filter");
    assert_eq!(filter.reach, 12, "three standard deviations");
    assert_eq!(layer.bounds, RectI::new(88, 88, 74, 74));
    assert_eq!(filter.rect, RectI::new(100, 100, 50, 50));
}

#[test]
fn what_a_filter_is_reaching_for_is_not_culled_away() {
    // The box is out of view, and its blur comes into it.
    let mut ui = Ui::new(|| {
        div().size_full().child(
            div()
                .absolute()
                .left(-30.0)
                .top(0.0)
                .w(30.0)
                .h(30.0)
                .bg(rgb(0))
                .blur(10.0),
        )
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let layer = only_layer(&scene);
    assert_eq!(layer.content.primitives.len(), 1);
}

#[test]
fn a_filter_reaching_further_than_a_screen_still_finds_it() {
    // The child is further than what is painted around the view for scrolling or moving, and
    // within what the filter of its parent reaches.
    let mut ui = Ui::new(|| {
        div().size_full().child(
            div()
                .absolute()
                .left(-530.0)
                .top(0.0)
                .w(560.0)
                .h(30.0)
                .blur(200.0)
                .child(div().w(30.0).h(30.0).bg(rgb(0))),
        )
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let layer = only_layer(&scene);
    assert_eq!(layer.content.primitives.len(), 1);
}

#[test]
fn changing_a_filter_paints_nothing_again() {
    let radius = signal(4.0_f32);
    let mut ui = Ui::new(move || {
        div().child(
            div()
                .w(50.0)
                .h(50.0)
                .bg(rgb(0))
                .filter(move || Image::content().blur(radius.get()))
                .child(text("in it")),
        )
    });
    sized(&mut ui);
    let first = ui.paint(Instant::now());
    let content = only_layer(&first).content.clone();

    radius.set(6.0);
    ui.update();
    let second = ui.paint(Instant::now());
    let layer = only_layer(&second);
    assert!(Arc::ptr_eq(&content, &layer.content), "the same content");
    assert_ne!(only_layer(&first).filter, layer.filter);
    assert!(!damage::diff(&first, &second).is_empty());
}

#[test]
fn the_blur_as_a_style_paints_nothing_again_either() {
    let radius = signal(4.0_f32);
    let mut ui = Ui::new(move || {
        div().child(
            div()
                .w(50.0)
                .h(50.0)
                .bg(rgb(0))
                .dyn_style(move |s| s.blur(radius.get()))
                .child(text("in it")),
        )
    });
    sized(&mut ui);
    let first = ui.paint(Instant::now());
    let content = only_layer(&first).content.clone();

    radius.set(6.0);
    ui.update();
    let second = ui.paint(Instant::now());
    assert!(Arc::ptr_eq(&content, &only_layer(&second).content));
    assert!(!damage::diff(&first, &second).is_empty());
}

#[test]
fn a_change_inside_is_seen_as_far_as_the_filter_reaches() {
    let color = signal(rgb(0));
    let mut ui = Ui::new(move || {
        div().child(
            div()
                .absolute()
                .left(100.0)
                .top(100.0)
                .w(100.0)
                .h(100.0)
                .blur(4.0)
                .child(
                    div()
                        .w(10.0)
                        .h(10.0)
                        .bg(rgb(0xffffff))
                        .dyn_style(move |s| s.bg(color.get())),
                ),
        )
    });
    sized(&mut ui);
    let first = ui.paint(Instant::now());
    color.set(rgb(0xff));
    ui.update();
    let second = ui.paint(Instant::now());
    let d = damage::diff(&first, &second);
    assert_eq!(d.bounds(), RectI::new(88, 88, 34, 34));
}

fn reader(left: f32) -> Div {
    div()
        .absolute()
        .left(left)
        .top(100.0)
        .w(50.0)
        .h(50.0)
        .backdrop_blur(4.0)
}

#[test]
fn a_change_within_what_a_backdrop_reads_takes_all_of_it() {
    let scene = |ui: &mut Ui| ui.paint(Instant::now());
    let mut ui = Ui::new(|| div().size_full().child(reader(100.0)));
    sized(&mut ui);
    let scene = scene(&mut ui);
    let read = scene
        .primitives
        .iter()
        .find_map(backdrop_read)
        .expect("something that reads");
    assert!(read.contains(PointF::new(90.0, 110.0)));
    assert!(read.width > 50 + 2 * 12, "{read:?}");

    // Next to the box, within what it reads.
    let mut near = Damage::new();
    near.add(RectI::new(read.x, read.y, 2, 2));
    expand_for_backdrops(&scene, &mut near);
    assert_eq!(near.bounds(), read);

    // Far enough.
    let mut far = Damage::new();
    far.add(RectI::new(read.right() + 5, read.y, 2, 2));
    expand_for_backdrops(&scene, &mut far);
    assert_eq!(far.bounds(), RectI::new(read.right() + 5, read.y, 2, 2));
}

#[test]
fn it_goes_on_from_one_backdrop_to_the_next() {
    let mut ui = Ui::new(|| {
        div()
            .size_full()
            .child(reader(10.0))
            .child(reader(70.0))
            .child(reader(130.0))
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let reads: Vec<RectI> = scene.primitives.iter().filter_map(backdrop_read).collect();
    assert_eq!(reads.len(), 3);
    let mut d = Damage::new();
    d.add(RectI::new(reads[0].x, reads[0].y, 2, 2));
    expand_for_backdrops(&scene, &mut d);
    assert!(d.bounds().right() >= reads[2].right(), "{:?}", d.bounds());
}

#[test]
fn nothing_is_added_where_no_change_touches() {
    let mut ui = Ui::new(|| div().size_full().child(reader(100.0)));
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let mut d = Damage::new();
    expand_for_backdrops(&scene, &mut d);
    assert!(d.is_empty());
}

#[test]
fn the_diff_says_it_too() {
    let color = signal(rgb(0));
    let mut ui = Ui::new(move || {
        div()
            .size_full()
            .child(
                div()
                    .absolute()
                    .left(90.0)
                    .top(120.0)
                    .w(5.0)
                    .h(5.0)
                    .dyn_style(move |s| s.bg(color.get())),
            )
            .child(reader(100.0))
    });
    sized(&mut ui);
    let first = ui.paint(Instant::now());
    let read = first
        .primitives
        .iter()
        .find_map(backdrop_read)
        .expect("something that reads");
    color.set(rgb(0xff));
    ui.update();
    let second = ui.paint(Instant::now());
    assert_eq!(damage::diff(&first, &second).bounds(), read);
}

#[test]
fn a_backdrop_of_a_layer_stays_outside_of_it_and_goes_with_it() {
    let mut ui = Ui::new(|| {
        div().size_full().child(
            div()
                .absolute()
                .left(100.0)
                .top(100.0)
                .w(50.0)
                .h(50.0)
                .bg(rgb(0))
                .scale(2.0)
                .opacity(0.5)
                .backdrop_blur(4.0),
        )
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let layer = only_layer(&scene);
    assert!(
        !layer
            .content
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Backdrop(_))),
        "not in the layer"
    );
    let backdrop = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Backdrop(b) => Some(b),
            _ => None,
        })
        .expect("in the scene");
    assert_eq!(backdrop.transform, layer.transform);
    assert_eq!(backdrop.opacity, layer.opacity);
    assert_eq!(
        backdrop.outer_rect(),
        layer.parent_rect(RectI::new(100, 100, 50, 50))
    );
}

#[test]
fn a_backdrop_comes_before_the_shadow_of_its_box() {
    let mut ui = Ui::new(|| {
        div().size_full().child(
            div()
                .w(50.0)
                .h(50.0)
                .shadow_md()
                .bg(rgba(0xffffff40))
                .backdrop_blur(4.0),
        )
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let kinds: Vec<u8> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Backdrop(_) => Some(0),
            Primitive::Shadow(_) => Some(1),
            Primitive::Quad(q) if !q.background.is_transparent() => Some(2),
            _ => None,
        })
        .collect();
    assert_eq!(kinds[0], 0, "{kinds:?}");
    assert_eq!(kinds[kinds.len() - 1], 2, "{kinds:?}");
}

#[test]
fn a_filter_makes_a_stacking_context() {
    // The child with a z-index stays in what is blurred, whatever comes after it.
    let mut ui = Ui::new(|| {
        div()
            .size_full()
            .child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .filter(|| Image::content().blur(2.0))
                    .child(div().w(10.0).h(10.0).z_index(10).bg(rgb(0))),
            )
            .child(div().w(10.0).h(10.0).bg(rgb(0xff)))
    });
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    let layer = only_layer(&scene);
    assert!(
        layer
            .content
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Quad(_)))
    );
}

#[test]
fn a_filter_that_comes_and_goes_makes_a_layer_and_stops() {
    let on = signal(false);
    let mut ui = Ui::new(move || {
        div().size_full().child(
            div()
                .w(50.0)
                .h(50.0)
                .bg(rgb(0))
                .filter(move || on.get().then(|| Image::content().blur(2.0))),
        )
    });
    sized(&mut ui);
    let has_layer = |s: &Scene| {
        s.primitives
            .iter()
            .any(|p| matches!(p, Primitive::Layer(_)))
    };
    assert!(!has_layer(&ui.paint(Instant::now())));
    on.set(true);
    ui.update();
    assert!(has_layer(&ui.paint(Instant::now())));
    on.set(false);
    ui.update();
    assert!(!has_layer(&ui.paint(Instant::now())));
}

#[test]
fn the_image_says_how_far_it_reaches() {
    let image = Image::content().blur(2.0).blur(3.0);
    assert_eq!(image.reach_px(1.0), 6 + 9);
    assert_eq!(image.reach_px(2.0), 12 + 18);
    assert!(image.read_px(1.0) > image.reach_px(1.0));
    assert!(!image.reads_backdrop());
    assert!(Image::backdrop().blur(1.0).reads_backdrop());
    assert_eq!(Image::content().blur(0.0), Image::content());
    static NONE: Shader<()> = Shader::new("none", "");
    let through = Image::backdrop().through(&NONE, ()).reach(5.0);
    assert_eq!(through.reach_px(2.0), 10);
    assert!(through.reads_backdrop());
}
