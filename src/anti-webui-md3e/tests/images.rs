//! Pictures in the components. See `docs/design/22-images.md`.

mod common;

use anti_webui::core::Ui;
use anti_webui::core::geometry::RectI;
use anti_webui::core::scene::{ImageDraw, Primitive, Scene};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    avatar, card_media, carousel, carousel_image, elevated_card, image, initials_avatar,
};
use common::*;
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

fn arrive(ui: &mut Ui) -> Scene {
    ui.wait_for_images(Duration::from_secs(30));
    settle(ui)
}

#[test]
fn an_image_is_a_surface_until_it_is_there() {
    let mut ui = start_ui(|| div().child(image(solid(32)).w(32.0).h(32.0)));
    let first = ui.paint(Instant::now());
    assert!(pictures(&first).is_empty());
    assert_eq!(colored(&first, C.surface_container_highest).len(), 1);
}

#[test]
fn an_image_fades_in_for_as_long_as_material_says() {
    let mut ui = start_ui(|| div().child(image(solid(32)).w(32.0).h(32.0)));
    ui.wait_for_images(Duration::from_secs(30));
    let start = Instant::now();
    let opacity_at = |ui: &mut Ui, t: u64| {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        pictures(&ui.paint(now))[0].opacity
    };
    assert_eq!(opacity_at(&mut ui, 0), 0.0);
    let middle = opacity_at(&mut ui, 150);
    assert!(middle > 0.2 && middle < 1.0, "{middle}");
    assert_eq!(opacity_at(&mut ui, 400), 1.0);
}

#[test]
fn an_image_that_cant_be_shown_has_an_icon() {
    let mut ui = start_ui(|| div().child(image("/no/such/file.png").w(64.0).h(64.0)));
    let scene = arrive(&mut ui);
    assert!(pictures(&scene).is_empty());
    let icons = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Glyphs(g) if g.glyphs.len() == 1))
        .count();
    assert_eq!(icons, 1);
}

#[test]
fn an_avatar_is_round_and_as_big_as_the_start_of_a_list_item() {
    let place = node_ref();
    let mut ui = start_ui(move || div().flex().child(avatar(solid(64), "Ada").node_ref(place)));
    let scene = arrive(&mut ui);
    let r = place.rect().unwrap();
    assert_eq!((r.width, r.height), (40, 40));
    assert_eq!(pictures(&scene)[0].shape.radii, [40.0; 4]);
}

#[test]
fn an_avatar_has_letters_when_there_is_no_picture() {
    let letters = |view: fn() -> Div| {
        let mut ui = start_ui(view);
        let scene = arrive(&mut ui);
        scene
            .primitives
            .iter()
            .map(|p| match p {
                Primitive::Glyphs(g) => g.glyphs.len(),
                _ => 0,
            })
            .sum::<usize>()
    };
    assert_eq!(
        letters(|| div().child(avatar("/no/such/file.png", "Ada Lovelace"))),
        2
    );
    assert_eq!(letters(|| div().child(initials_avatar("Ada Lovelace"))), 2);
    assert_eq!(letters(|| div().child(avatar(solid(8), "Ada Lovelace"))), 0);
}

#[test]
fn a_picture_on_a_card_goes_to_its_corners() {
    let mut ui = start_ui(|| div().child(elevated_card().w(200.0).child(card_media(solid(64)))));
    let scene = arrive(&mut ui);
    let d = &pictures(&scene)[0];
    assert_eq!(d.clip.radii, [12.0; 4]);
    assert_eq!(d.area(), RectI::new(0, 0, 200, 168));
}

/// As with the mask of Compose: what is narrower shows the middle of the picture.
#[test]
fn an_item_of_a_carousel_shows_the_middle_of_its_picture() {
    let mut ui = start_ui(|| {
        div().w(720.0).child(carousel(8, 240.0, |_| {
            carousel_image(solid(64)).object_fit(ObjectFit::Fill)
        }))
    });
    let scene = arrive(&mut ui);
    let all = pictures(&scene);
    assert!(all.len() >= 3);
    let mut narrow = 0;
    for d in &all {
        // Whatever its width, the middles line up.
        let (clip, rect) = (d.clip.rect, d.rect);
        let middle = |x: i32, width: i32| x * 2 + width;
        assert!(
            (middle(clip.x, clip.width) - middle(rect.x, rect.width)).abs() <= 2,
            "{clip:?} {rect:?}"
        );
        narrow += usize::from(clip.width < rect.width);
    }
    assert!(narrow >= 1, "some are cut");
}
