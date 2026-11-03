//! Fonts that come with the application (see `docs/design/04-text.md`).

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
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
