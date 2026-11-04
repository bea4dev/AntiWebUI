//! Fonts that come with the application (see `docs/design/04-text.md`).

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::text::{ShapeStyle, TextLayout, with_text};
use std::time::Instant;

static FONT: &[u8] = include_bytes!("../../anti-webui-md3e/fonts/RobotoFlex.ttf");

fn glyph_font_size(scene: &Scene) -> Option<usize> {
    scene.primitives.iter().find_map(|p| match p {
        Primitive::Glyphs(g) => Some(g.font.data.data().len()),
        _ => None,
    })
}

fn sized(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(300, 100),
        scale: Scale::ONE,
    });
    ui.update();
}

#[test]
fn a_font_is_found_by_the_name_in_its_file() {
    assert_eq!(register_font(FONT).unwrap(), ["Roboto Flex"]);
}

#[test]
fn what_is_not_a_font_says_so() {
    assert_eq!(register_font(&b"not a font"[..]), Err(NotAFont));
    assert_eq!(register_font_as(&b""[..], "Nothing"), Err(NotAFont));
}

#[test]
fn text_uses_a_font_under_the_name_given_to_it() {
    assert_eq!(register_font_as(FONT, "My Font").unwrap(), ["My Font"]);
    let mut ui = Ui::new(|| text("Hello").font_family("My Font"));
    sized(&mut ui);
    let scene = ui.paint(Instant::now());
    assert_eq!(glyph_font_size(&scene), Some(FONT.len()));
}

#[test]
fn text_already_shown_takes_a_font_that_arrives_later() {
    let name = "A Font That Arrives Later";
    let mut ui = Ui::new(move || text("Hello").font_family(name));
    sized(&mut ui);
    let now = Instant::now();
    let before = ui.paint(now);
    assert_ne!(glyph_font_size(&before), Some(FONT.len()));
    assert!(!ui.needs_redraw());
    register_font_as(FONT, name).unwrap();
    assert!(ui.needs_redraw());
    ui.update();
    let after = ui.paint(now);
    assert_eq!(glyph_font_size(&after), Some(FONT.len()));
}

#[test]
fn text_takes_a_font_that_arrives_later_whatever_the_case_of_its_name() {
    let mut ui = Ui::new(|| text("Hello").font_family("a font in lower case, sans-serif"));
    sized(&mut ui);
    let now = Instant::now();
    let _ = ui.paint(now);
    register_font_as(FONT, "A Font In Lower Case").unwrap();
    assert!(ui.needs_redraw());
    ui.update();
    assert_eq!(glyph_font_size(&ui.paint(now)), Some(FONT.len()));
}

#[test]
fn text_takes_a_font_that_arrives_later_under_the_name_in_its_file() {
    let mut ui = Ui::new(|| text("Hello").font_family("Roboto Flex"));
    sized(&mut ui);
    let now = Instant::now();
    let _ = ui.paint(now);
    register_font(FONT).unwrap();
    assert!(ui.needs_redraw());
    ui.update();
    assert_eq!(glyph_font_size(&ui.paint(now)), Some(FONT.len()));
}

#[test]
fn a_font_that_no_text_asked_for_changes_nothing_on_screen() {
    let mut ui = Ui::new(|| text("Hello"));
    sized(&mut ui);
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
    register_font_as(FONT, "A Font Nothing Asked For").unwrap();
    assert!(!ui.needs_redraw());
}

#[test]
fn another_face_of_a_family_in_use_is_taken_up() {
    let name = "A Family In Use";
    register_font_as(FONT, name).unwrap();
    let mut ui = Ui::new(move || text("Hello").font_family(name));
    sized(&mut ui);
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
    register_font_as(FONT, name).unwrap();
    assert!(ui.needs_redraw());
}

#[test]
fn a_name_seen_only_by_an_earlier_font_still_counts() {
    // Nothing has shaped with it, but the family could be reached through a fallback.
    let name = "A Family Nothing Shaped With";
    register_font_as(FONT, name).unwrap();
    let mut ui = Ui::new(|| text("Hello"));
    sized(&mut ui);
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
    register_font_as(FONT, name).unwrap();
    assert!(ui.needs_redraw());
}

#[test]
fn the_weight_of_a_variable_font_goes_through_its_axes() {
    register_font_as(FONT, "Weighty").unwrap();
    let coords = |weight: u16| {
        let mut ui = Ui::new(move || {
            text("Hello")
                .font_family("Weighty")
                .font_weight(FontWeight(weight))
        });
        sized(&mut ui);
        let scene = ui.paint(Instant::now());
        scene.primitives.iter().find_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.normalized_coords.clone()),
            _ => None,
        })
    };
    assert_ne!(coords(400), coords(700));
}

#[test]
fn what_is_shaped_is_shaped_again_only_for_a_font_it_asked_for() {
    let family = "A Family Asked For";
    let style = ShapeStyle {
        family: family.into(),
        size: 14.0,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
        axes: Default::default(),
        optical_size: 14.0,
        line_height: None,
        letter_spacing: 0.0,
        wrap: true,
    };
    let mut layout = TextLayout::default();
    let mut shape = || with_text(|ts| layout.shape(ts, "Hello", 1, &style));
    assert!(shape());
    assert!(!shape());
    register_font_as(FONT, "A Family Nothing Asked For").unwrap();
    assert!(!shape());
    register_font_as(FONT, family).unwrap();
    assert!(shape());
}
