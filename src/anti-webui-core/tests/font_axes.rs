//! The axes of variable fonts (see `docs/design/17-font-axes.md`).

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::tree::with_doc;
use std::time::{Duration, Instant};

static FONT: &[u8] = include_bytes!("../../anti-webui-md3e/fonts/RobotoFlex.ttf");
const FAMILY: &str = "Axes";

// Where the axes are in Roboto Flex.
const OPSZ: usize = 0;
const WGHT: usize = 1;
const GRAD: usize = 2;
const WDTH: usize = 3;

fn window_at(ui: &mut Ui, scale: Scale) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 300),
        scale,
    });
    ui.update();
}

fn window(ui: &mut Ui) {
    window_at(ui, Scale::ONE);
    let _ = ui.paint(Instant::now());
}

fn font() {
    register_font_as(FONT, FAMILY).unwrap();
}

/// What the first run of glyphs is drawn with.
fn coords_of(scene: &Scene) -> Vec<i16> {
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.normalized_coords.clone()),
            _ => None,
        })
        .expect("some text")
}

fn glyph_xs(scene: &Scene) -> Vec<f32> {
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.glyphs.iter().map(|g| g.x).collect()),
            _ => None,
        })
        .expect("some text")
}

/// What a view looks like on its own, on a thread of its own: there is one `Ui` to a thread.
fn look<T: Send + 'static>(
    scale: Scale,
    view: impl FnOnce() -> Text + Send + 'static,
    f: impl FnOnce(&Scene) -> T + Send + 'static,
) -> T {
    std::thread::spawn(move || {
        font();
        let mut ui = Ui::new(view);
        window_at(&mut ui, scale);
        f(&ui.paint(Instant::now()))
    })
    .join()
    .unwrap()
}

fn coords(view: impl FnOnce() -> Text + Send + 'static) -> Vec<i16> {
    look(Scale::ONE, view, coords_of)
}

fn at(coords: &[i16], axis: usize) -> i16 {
    coords.get(axis).copied().unwrap_or(0)
}

fn hello() -> Text {
    text("Hello").font_family(FAMILY)
}

#[test]
fn an_axis_goes_to_the_font() {
    let c = coords(|| hello().font_axis(b"GRAD", 150.0));
    assert_eq!(at(&c, GRAD), 1 << 14, "the end of the axis: {c:?}");
    assert_eq!(at(&c, WGHT), 0);
    assert_eq!(at(&c, WDTH), 0);
}

#[test]
fn nothing_set_is_the_font_as_it_comes() {
    assert_eq!(coords(hello), Vec::<i16>::new());
}

#[test]
fn an_axis_the_font_does_not_have_changes_nothing() {
    assert_eq!(
        coords(|| hello().font_axis(b"ZZZZ", 1.0)),
        Vec::<i16>::new()
    );
}

#[test]
fn a_value_past_the_end_of_an_axis_is_the_end() {
    assert_eq!(
        coords(|| hello().font_axis(b"wght", 5000.0)),
        coords(|| hello().font_axis(b"wght", 1000.0)),
    );
    assert_eq!(
        coords(|| hello().font_axis(b"wght", -5000.0)),
        coords(|| hello().font_axis(b"wght", 100.0)),
    );
}

#[test]
fn saying_it_with_an_axis_beats_the_weight() {
    assert_eq!(
        coords(|| hello()
            .font_weight(FontWeight::BOLD)
            .font_axis(b"wght", 400.0)),
        Vec::<i16>::new(),
    );
}

#[test]
fn some_axes_make_text_wider_and_some_do_not() {
    let xs = |view: fn() -> Text| look(Scale::ONE, view, glyph_xs);
    let plain = xs(hello);
    assert_eq!(xs(|| hello().font_axis(b"GRAD", 150.0)), plain);
    let bold = xs(|| hello().font_axis(b"wght", 700.0));
    assert!(bold.last() > plain.last(), "{bold:?} {plain:?}");
}

#[test]
fn optical_size_follows_the_font_size() {
    assert_eq!(coords(|| hello().text_size(14.0)), Vec::<i16>::new());
    let big = coords(|| hello().text_size(32.0));
    let bigger = coords(|| hello().text_size(64.0));
    assert!(at(&big, OPSZ) > 0, "{big:?}");
    assert!(at(&bigger, OPSZ) > at(&big, OPSZ));
    let small = coords(|| hello().text_size(9.0));
    assert!(at(&small, OPSZ) < 0, "{small:?}");
    assert_eq!(
        at(&coords(|| hello().text_size(400.0)), OPSZ),
        1 << 14,
        "the end of the axis"
    );
}

#[test]
fn optical_size_is_in_logical_pixels() {
    let at_scale =
        |scale: Scale, size: f32| look(scale, move || hello().text_size(size), coords_of);
    assert_eq!(
        at_scale(Scale::from_f64(2.0), 16.0),
        at_scale(Scale::ONE, 16.0)
    );
    assert_ne!(
        at_scale(Scale::from_f64(2.0), 16.0),
        at_scale(Scale::ONE, 32.0)
    );
}

#[test]
fn setting_optical_size_beats_following_the_font_size() {
    assert_eq!(
        coords(|| hello().text_size(32.0).font_axis(b"opsz", 14.0)),
        Vec::<i16>::new()
    );
}

#[test]
fn a_child_keeps_the_axes_of_its_parent_that_it_does_not_set() {
    font();
    let mut ui = Ui::new(|| {
        div()
            .font_family(FAMILY)
            .font_axis(b"GRAD", 150.0)
            .font_axis(b"XTRA", 400.0)
            .child(
                text("Hello")
                    .font_axis(b"wdth", 50.0)
                    .font_axis(b"XTRA", 500.0),
            )
    });
    window(&mut ui);
    let c = coords_of(&ui.paint(Instant::now()));
    assert_eq!(at(&c, GRAD), 1 << 14, "from the parent: {c:?}");
    assert!(at(&c, WDTH) < 0, "its own: {c:?}");
    let xtra = 7;
    let own = coords(|| hello().font_axis(b"XTRA", 500.0));
    assert_eq!(at(&c, xtra), at(&own, xtra), "its own over the parent's");
}

#[test]
fn a_state_changes_only_the_axes_it_sets() {
    font();
    let on = signal(false);
    let mut ui = Ui::new(move || {
        hello().font_axis(b"wdth", 50.0).dyn_style(move |s| {
            if on.get() {
                s.font_axis(b"GRAD", 150.0)
            } else {
                s
            }
        })
    });
    window(&mut ui);
    on.set(true);
    ui.update();
    let c = coords_of(&ui.paint(Instant::now()));
    assert_eq!(at(&c, GRAD), 1 << 14, "{c:?}");
    assert!(at(&c, WDTH) < 0, "{c:?}");
}

#[test]
fn hovering_takes_an_axis_and_leaving_gives_it_back() {
    font();
    let mut ui = Ui::new(|| {
        div().size_full().child(
            div()
                .w(300.0)
                .font_family(FAMILY)
                .font_axis(b"wdth", 50.0)
                .hover(|s| s.font_axis(b"GRAD", 150.0))
                .child("Hello"),
        )
    });
    window(&mut ui);
    let now = Instant::now();
    let before = coords_of(&ui.paint(now));
    assert_eq!(at(&before, GRAD), 0);
    let moved = |x, y| WindowEvent::PointerMoved {
        position: anti_webui_core::geometry::PointF::new(x, y),
        modifiers: Modifiers::default(),
    };
    ui.handle_event(moved(5.0, 5.0));
    ui.update();
    let hovered = coords_of(&ui.paint(now));
    assert_eq!(at(&hovered, GRAD), 1 << 14, "{hovered:?}");
    assert_eq!(at(&hovered, WDTH), at(&before, WDTH));
    ui.handle_event(moved(700.0, 200.0));
    ui.update();
    assert_eq!(coords_of(&ui.paint(now)), before);
}

#[test]
fn glyphs_are_drawn_on_steps_of_an_axis() {
    font();
    let value = signal(20.0_f32);
    let mut ui = Ui::new(move || hello().dyn_style(move |s| s.font_axis(b"GRAD", value.get())));
    window(&mut ui);
    let mut seen = std::collections::HashSet::new();
    for v in 0..=1500 {
        value.set(v as f32 * 0.1);
        ui.update();
        let c = coords_of(&ui.paint(Instant::now()));
        assert!(c.iter().all(|c| c % 256 == 0), "{c:?}");
        seen.insert(c);
    }
    assert!(seen.len() >= 60, "{}", seen.len());
    assert!(seen.len() <= 65, "{}", seen.len());
}

#[test]
fn close_to_the_default_is_the_default() {
    assert_eq!(
        coords(|| hello().font_axis(b"GRAD", 0.01)),
        Vec::<i16>::new()
    );
}

#[test]
fn a_change_within_a_step_draws_nothing() {
    font();
    let value = signal(75.0_f32);
    let mut ui = Ui::new(move || hello().dyn_style(move |s| s.font_axis(b"GRAD", value.get())));
    window(&mut ui);
    let before = ui.paint(Instant::now());
    value.set(75.1);
    ui.update();
    let same = ui.paint(Instant::now());
    assert!(damage::diff(&before, &same).is_empty());
    value.set(120.0);
    ui.update();
    let other = ui.paint(Instant::now());
    assert!(!damage::diff(&same, &other).is_empty());
}

fn frame_at(ui: &mut Ui, start: Instant, at: u64) -> Scene {
    ui.animate(start + Duration::from_millis(at));
    ui.update();
    ui.paint(start + Duration::from_millis(at))
}

/// A text that takes on `state` when `on` is set, in `Transition::ms(100)`.
fn switching(
    on: Signal<bool>,
    base: impl Fn(Text) -> Text + 'static,
    state: impl Fn(Style) -> Style + 'static,
) -> Ui {
    font();
    let mut ui = Ui::new(move || {
        base(hello())
            .transition(Transition::ms(100).linear())
            .dyn_style(move |s| if on.get() { state(s) } else { s })
    });
    window(&mut ui);
    ui
}

#[test]
fn an_axis_that_was_not_set_starts_from_the_default_of_the_font() {
    let on = signal(false);
    let mut ui = switching(on, |t| t, |s| s.font_axis(b"wdth", 151.0));
    let start = Instant::now();
    on.set(true);
    ui.update();
    let first = coords_of(&ui.paint(start));
    assert_eq!(at(&first, WDTH), 0, "doesn't jump: {first:?}");
    let middle = at(&coords_of(&frame_at(&mut ui, start, 50)), WDTH);
    assert!(middle > 0 && middle < 1 << 14, "{middle}");
    let end = coords_of(&frame_at(&mut ui, start, 100));
    assert_eq!(at(&end, WDTH), 1 << 14);
    frame_at(&mut ui, start, 110);
    assert!(!ui.animating());
}

#[test]
fn an_axis_goes_back_to_not_being_set() {
    let on = signal(true);
    let mut ui = switching(on, |t| t, |s| s.font_axis(b"wdth", 151.0));
    let start = Instant::now();
    on.set(false);
    ui.update();
    let first = coords_of(&ui.paint(start));
    assert_eq!(at(&first, WDTH), 1 << 14, "{first:?}");
    let middle = at(&coords_of(&frame_at(&mut ui, start, 50)), WDTH);
    assert!(middle > 0 && middle < 1 << 14, "{middle}");
    assert_eq!(
        coords_of(&frame_at(&mut ui, start, 100)),
        Vec::<i16>::new(),
        "the font as it comes"
    );
    frame_at(&mut ui, start, 110);
    assert!(!ui.animating());
}

#[test]
fn the_weight_axis_starts_from_the_weight() {
    let bold = coords(|| hello().font_weight(FontWeight::BOLD));
    let on = signal(false);
    let mut ui = switching(
        on,
        |t| t.font_weight(FontWeight::BOLD),
        |s| s.font_axis(b"wght", 300.0),
    );
    let start = Instant::now();
    on.set(true);
    ui.update();
    let first = coords_of(&ui.paint(start));
    assert_eq!(at(&first, WGHT), at(&bold, WGHT), "{first:?}");
    // Just on its way, it is still heavy: the default of the font is 400, which has none of this.
    let soon = coords_of(&frame_at(&mut ui, start, 10));
    assert!(at(&soon, WGHT) > 0, "{soon:?}");
    let light = coords(|| hello().font_axis(b"wght", 300.0));
    assert_eq!(
        at(&coords_of(&frame_at(&mut ui, start, 100)), WGHT),
        at(&light, WGHT)
    );
}

#[test]
fn the_optical_size_axis_starts_from_the_font_size() {
    let big = coords(|| hello().text_size(32.0));
    let on = signal(false);
    let mut ui = switching(on, |t| t.text_size(32.0), |s| s.font_axis(b"opsz", 8.0));
    let start = Instant::now();
    on.set(true);
    ui.update();
    let first = coords_of(&ui.paint(start));
    assert_eq!(at(&first, OPSZ), at(&big, OPSZ), "{first:?}");
    // Just on its way, it is still big: the default of the font is 14, which has none of this.
    let soon = coords_of(&frame_at(&mut ui, start, 10));
    assert!(at(&soon, OPSZ) > 0, "{soon:?}");
    let end = coords_of(&frame_at(&mut ui, start, 100));
    assert!(at(&end, OPSZ) < 0, "{end:?}");
}

#[test]
fn turning_back_on_the_way_does_not_jump() {
    let on = signal(false);
    let mut ui = switching(on, |t| t, |s| s.font_axis(b"wdth", 151.0));
    let start = Instant::now();
    on.set(true);
    ui.update();
    let _ = ui.paint(start);
    let there = at(&coords_of(&frame_at(&mut ui, start, 60)), WDTH);
    // The way back starts now, in real time.
    let turned = Instant::now();
    on.set(false);
    ui.update();
    let back = at(&coords_of(&ui.paint(turned)), WDTH);
    assert!((back - there).abs() <= 256, "{there} {back}");
    let later = at(&coords_of(&frame_at(&mut ui, turned, 30)), WDTH);
    assert!(later < back && later > 0, "{back} {later}");
    assert_eq!(
        coords_of(&frame_at(&mut ui, turned, 100)),
        Vec::<i16>::new()
    );
}

#[test]
fn the_weight_goes_through_the_values_between() {
    let on = signal(false);
    let mut ui = switching(on, |t| t, |s| s.font_weight(FontWeight(800)));
    let start = Instant::now();
    on.set(true);
    ui.update();
    let _ = ui.paint(start);
    let middle = at(&coords_of(&frame_at(&mut ui, start, 50)), WGHT);
    let end = at(&coords_of(&frame_at(&mut ui, start, 100)), WGHT);
    assert!(middle > 0 && middle < end, "{middle} {end}");
    assert_eq!(
        end,
        at(&coords(|| hello().font_weight(FontWeight(800))), WGHT)
    );
}

#[test]
fn only_what_is_named_goes_through_values() {
    font();
    let on = signal(false);
    let mut ui = Ui::new(move || {
        hello()
            .transition_of([Prop::FontAxes], Transition::ms(100).linear())
            .dyn_style(move |s| {
                if on.get() {
                    s.font_axis(b"GRAD", 150.0).font_weight(FontWeight::BOLD)
                } else {
                    s
                }
            })
    });
    window(&mut ui);
    let start = Instant::now();
    on.set(true);
    ui.update();
    let c = coords_of(&ui.paint(start));
    assert_eq!(at(&c, GRAD), 0, "still to go: {c:?}");
    assert!(at(&c, WGHT) > 0, "already there: {c:?}");
}

#[test]
fn the_box_grows_with_the_text_on_the_way() {
    font();
    let on = signal(false);
    let n = node_ref();
    let mut ui = Ui::new(move || {
        div().flex().size_full().items_start().child(
            div()
                .node_ref(n)
                .font_family(FAMILY)
                .text_size(30.0)
                .transition(Transition::ms(100).linear())
                .dyn_style(move |s| {
                    if on.get() {
                        s.font_axis(b"wght", 900.0)
                    } else {
                        s
                    }
                })
                .child("Some words"),
        )
    });
    window(&mut ui);
    let width = || with_doc(|d| d.get(n.get().unwrap()).unwrap().rect.width);
    let start = Instant::now();
    let before = width();
    on.set(true);
    ui.update();
    let _ = ui.paint(start);
    frame_at(&mut ui, start, 50);
    let middle = width();
    frame_at(&mut ui, start, 100);
    let end = width();
    assert!(before < middle && middle < end, "{before} {middle} {end}");
}

#[test]
fn text_being_edited_has_its_axes_too() {
    font();
    let mut ui = Ui::new(|| {
        text_input()
            .font_family(FAMILY)
            .font_axis(b"GRAD", 150.0)
            .value(signal("Hello".to_string()))
    });
    window(&mut ui);
    let c = coords_of(&ui.paint(Instant::now()));
    assert_eq!(at(&c, GRAD), 1 << 14, "{c:?}");
}
