//! Pictures with what shows around them. See `docs/design/22-images.md`.

mod common;

use anti_webui_core::Ui;
use anti_webui_core::geometry::RectI;
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_widgets::*;
use common::start;
use std::time::{Duration, Instant};

fn solid(side: u32) -> ImageSource {
    ImageSource::rgba(side, side, [10, 20, 30, 255].repeat((side * side) as usize))
}

fn pictures(scene: &Scene) -> Vec<ImageDraw> {
    let mut out = Vec::new();
    for p in &scene.primitives {
        match p {
            Primitive::Image(d) => out.push(d.clone()),
            Primitive::Layer(l) => out.extend(pictures(&l.content)),
            _ => {}
        }
    }
    out
}

fn texts(scene: &Scene) -> usize {
    scene
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Glyphs(_) => 1,
            Primitive::Layer(l) => texts(&l.content),
            _ => 0,
        })
        .sum()
}

fn backgrounds(scene: &Scene) -> Vec<Color> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) => Some(q.background),
            _ => None,
        })
        .collect()
}

/// Waits for the pictures, and then for what happens when they arrive.
fn settle(ui: &mut Ui) -> Scene {
    ui.wait_for_images(Duration::from_secs(30));
    let mut now = Instant::now();
    let mut scene = ui.paint(now);
    for _ in 0..80 {
        now += Duration::from_millis(16);
        ui.animate(now);
        ui.update();
        scene = ui.paint(now);
    }
    scene
}

#[test]
fn image_view_shows_the_color_of_the_theme_until_the_picture_is_there() {
    let mut ui = start(|| div().child(image_view(solid(32), "x").w(32.0).h(32.0)));
    let first = ui.paint(Instant::now());
    assert!(pictures(&first).is_empty());
    assert!(backgrounds(&first).contains(&theme().surface_muted));
}

#[test]
fn image_view_fades_the_picture_in_over_that_color() {
    let mut ui = start(|| div().child(image_view(solid(32), "x").w(32.0).h(32.0)));
    ui.wait_for_images(Duration::from_secs(30));
    let at_first = pictures(&ui.paint(Instant::now()))[0].opacity;
    assert_eq!(at_first, 0.0);
    let scene = settle(&mut ui);
    assert_eq!(pictures(&scene)[0].opacity, 1.0);
    // The color goes on being there, under it.
    assert!(backgrounds(&scene).contains(&theme().surface_muted));
}

#[test]
fn image_view_fills_its_box_unless_told_otherwise() {
    let mut ui = start(|| {
        div()
            .child(image_view(solid(32), "x").w(100.0).h(50.0))
            .child(
                image_view(solid(33), "x")
                    .w(100.0)
                    .h(50.0)
                    .object_fit(ObjectFit::Contain),
            )
    });
    let scene = settle(&mut ui);
    let all = pictures(&scene);
    assert_eq!(all[0].rect, RectI::new(0, -25, 100, 100));
    assert_eq!(all[1].rect, RectI::new(25, 50, 50, 50));
}

#[test]
fn image_view_says_what_it_would_have_shown() {
    let mut ui = start(|| div().child(image_view("/no/such/file.png", "A cat").w(100.0).h(50.0)));
    assert_eq!(texts(&ui.paint(Instant::now())), 0);
    let scene = settle(&mut ui);
    assert!(pictures(&scene).is_empty());
    assert_eq!(texts(&scene), 1);
}

#[test]
fn image_view_takes_a_color_of_its_own_above_the_theme() {
    let red = rgb(0xff0000);
    let mut ui = start(move || div().child(image_view(solid(32), "x").w(32.0).h(32.0).bg(red)));
    let scene = settle(&mut ui);
    assert!(backgrounds(&scene).contains(&red));
    assert!(!backgrounds(&scene).contains(&theme().surface_muted));
}

#[test]
fn image_view_follows_the_theme() {
    let mut ui = start(|| div().child(image_view(solid(32), "x").w(32.0).h(32.0)));
    set_theme(Theme::dark());
    let scene = settle(&mut ui);
    assert!(backgrounds(&scene).contains(&Theme::dark().surface_muted));
    set_theme(Theme::light());
}

#[test]
fn an_avatar_has_the_first_letters_of_a_name() {
    let mut ui = start(|| div().child(avatar("ada lovelace byron")));
    let scene = ui.paint(Instant::now());
    let glyphs: usize = scene
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Glyphs(g) => g.glyphs.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(glyphs, 2);
    assert!(backgrounds(&scene).contains(&theme().accent_muted));
}

#[test]
fn an_avatar_shows_letters_until_the_picture_is_there_and_then_no_more() {
    let mut ui = start(|| div().child(image_avatar(solid(32), "Ada Lovelace")));
    assert_eq!(texts(&ui.paint(Instant::now())), 1);
    let scene = settle(&mut ui);
    assert_eq!(texts(&scene), 0);
    assert_eq!(pictures(&scene).len(), 1);
    assert_eq!(pictures(&scene)[0].shape.radii, [36.0; 4]);
}

#[test]
fn an_avatar_goes_on_showing_letters_if_the_picture_cant_be_shown() {
    let mut ui = start(|| div().child(image_avatar("/no/such/file.png", "Ada Lovelace")));
    let scene = settle(&mut ui);
    assert_eq!(texts(&scene), 1);
    assert!(pictures(&scene).is_empty());
}

#[test]
fn an_avatar_keeps_its_size_while_the_picture_is_big() {
    let place = node_ref();
    let mut ui = start(move || {
        div()
            .flex()
            .child(image_avatar(solid(300), "A").node_ref(place))
    });
    let _ = settle(&mut ui);
    let r = place.rect().unwrap();
    assert_eq!((r.width, r.height), (36, 36));
}

#[test]
fn a_picture_on_a_card_goes_to_its_corners_and_no_further() {
    let mut ui = start(|| {
        div().child(
            media_card(solid(64), "x")
                .w(200.0)
                .child(card_body().child(text("Below"))),
        )
    });
    let scene = settle(&mut ui);
    let d = &pictures(&scene)[0];
    assert_eq!(d.clip.radii, [7.0; 4], "the inside of the rounded border");
    assert_eq!(d.area().width, 198);
    assert_eq!(d.area().height, 160);
}
