//! The shapes of the catalog, how they change into each other, and `Ink` on a box of the program's
//! own. Through the real renderer where pixels matter; skipped when no GPU adapter is available.

mod common;

use anti_webui::app::headless;
use anti_webui::core::Ui;
use anti_webui::core::event::PointerButton;
use anti_webui::core::scene::Scene;
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Renderer, wgpu::TextureFormat};
use anti_webui_md3e::ColorScheme;
use anti_webui_md3e::components::{Ink, MaterialShape, ShapeStyle, shape, styled_shape};
use common::*;
use std::cell::Cell;
use std::rc::Rc;

fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

fn rgb_of(c: Color) -> [u8; 3] {
    let p = c.premultiplied();
    [0, 1, 2].map(|i| (p[i] * 255.0).round() as u8)
}

const WHITE: [u8; 4] = [255, 255, 255, 255];

struct Pixels {
    gpu: Gpu,
    renderer: Renderer,
}

impl Pixels {
    fn new() -> Option<Self> {
        match Gpu::headless() {
            Ok(gpu) => {
                let renderer = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
                Some(Self { gpu, renderer })
            }
            Err(e) => {
                eprintln!("skipping: {e}");
                None
            }
        }
    }

    fn of(&mut self, scene: &Scene) -> Vec<u8> {
        self.gpu.render_to_rgba(&mut self.renderer, scene)
    }
}

/// The pixels of the square of `size` at the top left of a frame 1000 wide.
fn square(p: &[u8], size: u32) -> Vec<[u8; 4]> {
    (0..size)
        .flat_map(|y| (0..size).map(move |x| (x, y)))
        .map(|(x, y)| px(p, 1000, x, y))
        .collect()
}

fn painted(pixels: &[[u8; 4]]) -> usize {
    pixels.iter().filter(|p| **p != WHITE).count()
}

/// A shape that changes when the test says so.
fn changing(style: ShapeStyle) -> (Ui, Signal<MaterialShape>) {
    let slot = Rc::new(Cell::new(None));
    let out = slot.clone();
    let ui = start_ui(move || {
        let wanted = signal(MaterialShape::Circle);
        out.set(Some(wanted));
        div()
            .bg(Color::WHITE)
            .w(pct(100.0))
            .h(pct(100.0))
            .child(styled_shape(move || wanted.get(), style).w(96.0).h(96.0))
    });
    (ui, slot.get().expect("the view was made"))
}

#[test]
fn every_shape_fills_the_middle_of_its_box_and_no_two_look_alike() {
    let (w, h) = (34 * 50, 50);
    let Ok(p) = headless::render_rgba(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .flex()
            .flex_row()
            .gap(2.0)
            .children(MaterialShape::ALL.map(|s| shape(move || s)))
    }) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let primary = rgb_of(ColorScheme::light().primary);
    let mut seen: Vec<Vec<bool>> = Vec::new();
    for (i, s) in MaterialShape::ALL.iter().enumerate() {
        let x0 = i as u32 * 50;
        assert_eq!(&px(&p, w, x0 + 24, 24)[..3], primary, "{s:?}");
        // It stays in its box of 48.
        for y in 0..h {
            assert_eq!(px(&p, w, x0 + 48, y), WHITE, "{s:?}");
            assert_eq!(px(&p, w, x0 + 49, y), WHITE, "{s:?}");
        }
        let mask: Vec<bool> = (0..48)
            .flat_map(|y| (0..48).map(move |x| (x, y)))
            .map(|(x, y)| px(&p, w, x0 + x, y) != WHITE)
            .collect();
        for (j, other) in seen.iter().enumerate() {
            let differ = mask.iter().zip(other).filter(|(a, b)| a != b).count();
            assert!(
                differ > 20,
                "{s:?} and {:?} differ by {differ}",
                MaterialShape::ALL[j]
            );
        }
        seen.push(mask);
    }
}

#[test]
fn the_edge_is_smooth_on_a_scale_that_is_not_whole() {
    let (w, h) = (120, 120);
    let Ok(p) = headless::render_rgba(w, h, 1.8, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(shape(|| MaterialShape::Circle).w(60.0).h(60.0))
    }) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let primary = rgb_of(ColorScheme::light().primary);
    // 108 across: in the middle at 54, and partly painted where the edge goes through pixels.
    assert_eq!(&px(&p, w, 54, 54)[..3], primary);
    let between = (0..w)
        .flat_map(|x| (0..h).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let c = px(&p, w, x, y);
            c != WHITE && c[..3] != primary
        })
        .count();
    // The circle is about 2 pi 53 around: about a pixel deep all along it.
    assert!((200..800).contains(&between), "{between}");
}

#[test]
fn a_shape_changes_into_another_like_a_spring_and_then_rests() {
    let Some(mut pixels) = Pixels::new() else {
        return;
    };
    let (mut ui, wanted) = changing(ShapeStyle::new());
    let circle = square(&pixels.of(&settle(&mut ui)), 96);
    assert!(!ui.animating(), "a shape that stays asks for no frames");

    wanted.set(MaterialShape::Square);
    ui.update();
    let mut frames = Vec::new();
    watch(&mut ui, 600, |scene| {
        frames.push(square(&pixels.of(scene), 96))
    });
    let changed = frames.windows(2).filter(|f| f[0] != f[1]).count();
    assert!(changed > 10, "it moves frame by frame: {changed}");
    // A square covers more of its box than a circle does, and it gets there.
    let last = frames.last().unwrap();
    assert!(painted(last) > painted(&circle) + 500);
    assert_ne!(px_of(last, 96, 10, 10), WHITE, "the corners are filled");
    assert_eq!(px_of(&circle, 96, 10, 10), WHITE);

    assert_at_rest(&mut ui);
    let square_at_rest = square(&pixels.of(&settle(&mut ui)), 96);
    // The same as a square that was there from the start.
    let mut only = start_ui(|| {
        div()
            .bg(Color::WHITE)
            .w(pct(100.0))
            .h(pct(100.0))
            .child(shape(|| MaterialShape::Square).w(96.0).h(96.0))
    });
    assert_eq!(square_at_rest, square(&pixels.of(&settle(&mut only)), 96));
}

fn px_of(pixels: &[[u8; 4]], size: u32, x: u32, y: u32) -> [u8; 4] {
    pixels[(y * size + x) as usize]
}

#[test]
fn a_change_on_the_way_goes_on_from_where_it_is() {
    let Some(mut pixels) = Pixels::new() else {
        return;
    };
    let (mut ui, wanted) = changing(ShapeStyle::new());
    settle(&mut ui);
    wanted.set(MaterialShape::Square);
    ui.update();
    let halfway = square(&pixels.of(&run(&mut ui, 64)), 96);
    wanted.set(MaterialShape::Heart);
    ui.update();
    // The next frame starts where the last left off: nothing jumps.
    let next = square(&pixels.of(&ui.paint(std::time::Instant::now())), 96);
    let jumped = halfway.iter().zip(&next).filter(|(a, b)| a != b).count();
    assert!(jumped < 10, "{jumped}");
    settle(&mut ui);
    assert_at_rest(&mut ui);
}

#[test]
fn a_shape_turns_with_its_rotation_and_rests_again() {
    let Some(mut pixels) = Pixels::new() else {
        return;
    };
    let slot = Rc::new(Cell::new(None));
    let out = slot.clone();
    let mut ui = start_ui(move || {
        let degrees = signal(0.0f32);
        out.set(Some(degrees));
        div().bg(Color::WHITE).w(pct(100.0)).h(pct(100.0)).child(
            styled_shape(
                || MaterialShape::Triangle,
                ShapeStyle::new().rotation(move || degrees.get()),
            )
            .w(96.0)
            .h(96.0),
        )
    });
    let degrees = slot.get().unwrap();
    let up = square(&pixels.of(&settle(&mut ui)), 96);
    assert!(!ui.animating());
    degrees.set(180.0);
    ui.update();
    let mut frames = 0;
    watch(&mut ui, 400, |_| frames += 1);
    let down = square(&pixels.of(&settle(&mut ui)), 96);
    assert_at_rest(&mut ui);
    // Upside down: the top row of the one is the bottom row of the other.
    let painted_in_row =
        |p: &[[u8; 4]], y: u32| (0..96).filter(|&x| px_of(p, 96, x, y) != WHITE).count();
    let (top, bottom) = (
        (4..20).map(|y| painted_in_row(&up, y)).sum::<usize>(),
        (76..92).map(|y| painted_in_row(&up, y)).sum::<usize>(),
    );
    assert!(bottom > top * 2, "{top} {bottom}");
    let (top, bottom) = (
        (4..20).map(|y| painted_in_row(&down, y)).sum::<usize>(),
        (76..92).map(|y| painted_in_row(&down, y)).sum::<usize>(),
    );
    assert!(top > bottom * 2, "{top} {bottom}");
}

#[test]
fn a_stroke_leaves_the_inside_empty() {
    let (w, h) = (100, 100);
    let Ok(p) = headless::render_rgba(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            styled_shape(
                || MaterialShape::Circle,
                ShapeStyle::new().stroke(4.0).color(|c| c.tertiary),
            )
            .w(96.0)
            .h(96.0),
        )
    }) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let tertiary = rgb_of(ColorScheme::light().tertiary);
    assert_eq!(px(&p, w, 48, 48), WHITE);
    // The circle is a little smaller than its box, as in Compose: its line is just inside 1.
    let row: Vec<u32> = (0..48)
        .filter(|&x| px(&p, w, x, 48)[..3] == tertiary)
        .collect();
    assert!(!row.is_empty() && row.len() <= 4, "{row:?}");
    assert!(row[0] <= 3, "{row:?}");
}

#[test]
fn ink_on_a_box_of_its_own_shows_under_the_pointer_and_then_goes() {
    let Some(mut pixels) = Pixels::new() else {
        return;
    };
    let mut ui = start_ui(|| {
        let ink = Ink::new(|c| c.on_surface, || false);
        let tile = div()
            .w(120.0)
            .h(56.0)
            .rounded(16.0)
            .bg(Color::WHITE)
            .focusable();
        div()
            .bg(Color::WHITE)
            .w(pct(100.0))
            .h(pct(100.0))
            .child(ink.paint(ink.listen(tile)))
    });
    let rest = settle(&mut ui);
    assert!(shaders(&rest).is_empty(), "nothing extra at rest");
    assert!(!ui.animating());

    pointer_at(&mut ui, 60.0, 28.0);
    let hovered = settle(&mut ui);
    assert_eq!(shaders(&hovered).len(), 1, "the state layer");
    let p = pixels.of(&hovered);
    let on_surface = ColorScheme::light().on_surface.premultiplied();
    // 8% of onSurface over white, in the middle and not in the cut off corner.
    let expected = (255.0 * (1.0 - 0.08) + on_surface[0] * 255.0 * 0.08).round() as i32;
    let middle = px(&p, 1000, 60, 28);
    assert!(
        (middle[0] as i32 - expected).abs() <= 2,
        "{middle:?} {expected}"
    );
    assert_eq!(px(&p, 1000, 1, 1), WHITE, "in the shape of the box");
    assert!(
        !ui.animating(),
        "a state layer that stays asks for no frames"
    );

    pointer(&mut ui, 60.0, 28.0, true);
    let pressed = run(&mut ui, 100);
    let p = pixels.of(&pressed);
    assert!(px(&p, 1000, 60, 28)[0] < middle[0], "a ripple");
    pointer(&mut ui, 60.0, 28.0, false);
    pointer_at(&mut ui, 600.0, 600.0);
    let gone = settle(&mut ui);
    assert!(shaders(&gone).is_empty());
    assert_at_rest(&mut ui);
}

/// A tile with ink of its own, 120 by 56 at the top left.
fn ink_tile() -> Div {
    let ink = Ink::new(|c| c.on_surface, || false);
    let tile = div().w(120.0).h(56.0).rounded(16.0).focusable();
    div().child(ink.paint(ink.listen(tile)))
}

#[test]
fn a_ripple_held_down_asks_for_no_frames_until_it_is_let_go() {
    let mut ui = start_ui(ink_tile);
    pointer_at(&mut ui, 60.0, 28.0);
    settle(&mut ui);
    pointer(&mut ui, 60.0, 28.0, true);
    assert!(ui.animating(), "it grows");
    let held = run(&mut ui, 400);
    assert_eq!(shaders(&held).len(), 1, "it is still drawn");
    assert!(!ui.animating(), "but asks for no frames once it has grown");
    pointer(&mut ui, 60.0, 28.0, false);
    assert!(ui.animating(), "it goes once let go");
    pointer_at(&mut ui, 600.0, 600.0);
    let gone = settle(&mut ui);
    assert!(shaders(&gone).is_empty());
    assert_at_rest(&mut ui);
}

#[test]
fn a_ripple_another_button_starts_goes_by_itself() {
    // Only the primary button makes the tile active, which is what lets a ripple go: one another
    // button starts, which opens a menu for one, used to stay and ask for frames for ever.
    for button in [PointerButton::Secondary, PointerButton::Middle] {
        let mut ui = start_ui(ink_tile);
        pointer_at(&mut ui, 60.0, 28.0);
        settle(&mut ui);
        pointer_button(&mut ui, 60.0, 28.0, true, button);
        assert!(ui.animating(), "{button:?} makes a ripple");
        // Its release may not even come here, when what it opened takes the pointer.
        settle(&mut ui);
        assert!(!ui.animating(), "{button:?}: the ripple has gone");
        pointer_button(&mut ui, 60.0, 28.0, false, button);
        pointer_at(&mut ui, 600.0, 600.0);
        let gone = settle(&mut ui);
        assert!(shaders(&gone).is_empty(), "{button:?}");
        assert_at_rest(&mut ui);
    }
}

#[test]
fn disabled_ink_shows_nothing() {
    let mut ui = start_ui(|| {
        let ink = Ink::new(|c| c.on_surface, || true);
        let tile = div().w(120.0).h(56.0).rounded(16.0).focusable();
        div().child(ink.paint(ink.listen(tile)))
    });
    pointer_at(&mut ui, 60.0, 28.0);
    pointer(&mut ui, 60.0, 28.0, true);
    let scene = run(&mut ui, 200);
    assert!(shaders(&scene).is_empty());
}
