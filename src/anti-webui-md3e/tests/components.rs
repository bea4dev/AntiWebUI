//! The components through the real renderer, using nothing but the public API. Skipped when no GPU
//! adapter is available.

use anti_webui::app::headless;
use anti_webui::core::Ui;
use anti_webui::core::event::{Modifiers, PointerButton, WindowEvent};
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonStyle, button, check_shaders, loading_indicator, outlined_button, styled_button,
    text_button, tonal_button, wavy_progress,
};
use anti_webui_md3e::{ColorScheme, set_color_scheme};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

fn render(w: u32, h: u32, scale: f64, view: impl FnOnce() -> Div) -> Option<Vec<u8>> {
    match headless::render_rgba(w, h, scale, view) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn rgb_of(c: Color) -> [u8; 3] {
    let p = c.premultiplied();
    [0, 1, 2].map(|i| (p[i] * 255.0).round() as u8)
}

#[test]
fn the_shaders_compile() {
    check_shaders().unwrap();
}

#[test]
fn a_button_has_the_size_and_color_of_its_kind() {
    let (w, h) = (300, 100);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .flex()
            .flex_row()
            .child(button("Save"))
    }) else {
        return;
    };
    let primary = rgb_of(ColorScheme::light().primary);
    // 40 high, starting at the top; a pill, so that its corner isn't painted.
    assert_eq!(&px(&p, w, 20, 1)[..3], primary);
    assert_eq!(&px(&p, w, 20, 39)[..3], primary);
    assert_eq!(px(&p, w, 20, 41), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 0, 0), [255, 255, 255, 255]);
}

#[test]
fn the_kinds_of_button_differ_like_the_tokens_say() {
    let (w, h) = (400, 60);
    let c = ColorScheme::light();
    let Some(p) = render(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .flex()
            .flex_row()
            .gap(4.0)
            .child(tonal_button("AAAA").w(90.0))
            .child(outlined_button("BBBB").w(90.0))
            .child(text_button("CCCC").w(90.0))
    }) else {
        return;
    };
    assert_eq!(&px(&p, w, 45, 4)[..3], rgb_of(c.secondary_container));
    // The outlined one is empty, with a line at its top.
    assert_eq!(&px(&p, w, 94 + 45, 0)[..3], rgb_of(c.outline_variant));
    assert_eq!(px(&p, w, 94 + 45, 4), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 188 + 45, 4), [255, 255, 255, 255]);
}

fn start_ui(view: impl FnOnce() -> Div + 'static, size: (i32, i32)) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(size.0, size.1),
        scale: Scale::ONE,
    });
    ui.update();
    ui
}

fn pointer_at(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
}

fn pointer(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
}

fn shaders(scene: &anti_webui::core::scene::Scene) -> usize {
    use anti_webui::core::scene::Primitive;
    scene
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Shader(_) => 1,
            Primitive::Layer(l) => shaders(&l.content),
            _ => 0,
        })
        .sum()
}

fn radius_of(scene: &anti_webui::core::scene::Scene) -> f32 {
    use anti_webui::core::scene::Primitive;
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Quad(q) if q.rect.width > 20 => Some(q.radii[0]),
            _ => None,
        })
        .expect("a box")
}

/// Runs the animations up to `at` from the start, a frame at a time.
fn run(ui: &mut Ui, start: Instant, from: u64, to: u64) -> anti_webui::core::scene::Scene {
    let mut scene = None;
    for t in (from..=to).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        scene = Some(ui.paint(now));
    }
    scene.expect("at least a frame")
}

#[test]
fn a_press_changes_the_shape_and_makes_a_ripple_that_goes_away() {
    let mut ui = start_ui(
        || {
            div()
                .flex()
                .flex_row()
                .h(pct(100.0))
                .child(button("Save").w(100.0))
        },
        (200, 100),
    );
    let start = Instant::now();
    let scene = ui.paint(start);
    assert_eq!(radius_of(&scene), 20.0);
    assert_eq!(shaders(&scene), 0, "nothing extra at rest");

    pointer_at(&mut ui, 30.0, 20.0);
    pointer(&mut ui, 30.0, 20.0, true);
    let scene = run(&mut ui, Instant::now(), 0, 400);
    assert!(
        (radius_of(&scene) - 8.0).abs() < 0.1,
        "{}",
        radius_of(&scene)
    );
    assert_eq!(shaders(&scene), 1);

    pointer(&mut ui, 30.0, 20.0, false);
    pointer_at(&mut ui, 180.0, 90.0);
    let scene = run(&mut ui, Instant::now(), 0, 1000);
    assert!(
        (radius_of(&scene) - 20.0).abs() < 0.1,
        "{}",
        radius_of(&scene)
    );
    assert_eq!(shaders(&scene), 0, "nothing extra when it is over");
    assert!(!ui.needs_redraw(), "and nothing goes on");
}

#[test]
fn the_ripple_shows_where_it_was_pressed() {
    let (w, h) = (300u32, 80u32);
    let Ok(gpu) = anti_webui::wgpu::Gpu::headless() else {
        return;
    };
    let mut renderer = anti_webui::wgpu::Renderer::new(
        &gpu.device,
        anti_webui::wgpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let mut ui = start_ui(
        || {
            div()
                .bg(Color::WHITE)
                .h(pct(100.0))
                .flex()
                .flex_row()
                .child(button("").w(300.0))
        },
        (w as i32, h as i32),
    );
    pointer_at(&mut ui, 40.0, 20.0);
    pointer(&mut ui, 40.0, 20.0, true);
    // Early on it is still small: it shows at the left and not at the right, where there is
    // only what being under the pointer shows.
    let scene = run(&mut ui, Instant::now(), 0, 90);
    let p = gpu.render_to_rgba(&mut renderer, &scene);
    let primary = rgb_of(ColorScheme::light().primary);
    let (near, far) = (px(&p, w, 40, 20), px(&p, w, 280, 20));
    assert!(near[0] > far[0] + 5, "{near:?} {far:?}");
    assert!(far[0] > primary[0], "{far:?}");
}

#[test]
fn a_disabled_button_does_not_react() {
    let off = Rc::new(std::cell::Cell::new(0));
    let clicks = off.clone();
    let mut ui = start_ui(
        move || {
            let clicks = clicks.clone();
            div().flex().flex_row().h(pct(100.0)).child(
                styled_button("Save", ButtonStyle::filled().disabled_when(|| true))
                    .w(100.0)
                    .on_click(move |_| clicks.set(clicks.get() + 1)),
            )
        },
        (200, 100),
    );
    pointer_at(&mut ui, 30.0, 20.0);
    pointer(&mut ui, 30.0, 20.0, true);
    let scene = run(&mut ui, Instant::now(), 0, 300);
    assert_eq!(radius_of(&scene), 20.0);
    assert_eq!(shaders(&scene), 0);
    pointer(&mut ui, 30.0, 20.0, false);
    ui.update();
    assert_eq!(off.get(), 0);
}

#[test]
fn what_is_added_to_a_button_does_not_take_its_behavior_away() {
    let hovers = Rc::new(std::cell::Cell::new(0));
    let seen = hovers.clone();
    let mut ui = start_ui(
        move || {
            let seen = seen.clone();
            div().flex().flex_row().h(pct(100.0)).child(
                button("Save")
                    .w(100.0)
                    .on_hover(move |_| seen.set(seen.get() + 1)),
            )
        },
        (200, 100),
    );
    pointer_at(&mut ui, 30.0, 20.0);
    let scene = run(&mut ui, Instant::now(), 0, 200);
    assert_eq!(hovers.get(), 1);
    assert_eq!(shaders(&scene), 1, "the state layer still shows");
}

#[test]
fn switching_the_scheme_recolors_what_is_shown() {
    let mut ui = start_ui(
        || {
            div()
                .flex()
                .flex_row()
                .h(pct(100.0))
                .child(button("Save").w(100.0))
        },
        (200, 100),
    );
    use anti_webui::core::scene::Primitive;
    let background = |scene: &anti_webui::core::scene::Scene| {
        scene
            .primitives
            .iter()
            .find_map(|p| match p {
                Primitive::Quad(q) if q.rect.width > 20 => Some(q.background),
                _ => None,
            })
            .expect("a box")
    };
    let now = Instant::now();
    assert_eq!(background(&ui.paint(now)), ColorScheme::light().primary);
    set_color_scheme(ColorScheme::dark());
    ui.update();
    assert_eq!(background(&ui.paint(now)), ColorScheme::dark().primary);
    set_color_scheme(ColorScheme::light());
}

#[test]
fn the_progress_bar_shows_a_line_a_gap_and_a_track() {
    let (w, h) = (240, 20);
    let c = ColorScheme::light();
    // No wave at this progress: all is straight.
    let Some(p) = render(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(wavy_progress(|| 0.05))
    }) else {
        return;
    };
    let active = rgb_of(c.primary);
    let track = rgb_of(c.secondary_container);
    assert_eq!(&px(&p, w, 8, 5)[..3], active, "the line");
    assert_eq!(px(&p, w, 17, 5), [255, 255, 255, 255], "the gap");
    assert_eq!(&px(&p, w, 200, 5)[..3], track, "the track");
    assert_eq!(&px(&p, w, 238, 5)[..3], active, "the stop at the end");
    assert_eq!(
        px(&p, w, 200, 1),
        [255, 255, 255, 255],
        "as thick as it should be"
    );
}

#[test]
fn the_wave_leaves_the_middle_and_stays_in_its_box() {
    let (w, h) = (240, 12);
    let Some(p) = render(w, h, 1.0, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .child(wavy_progress(|| 0.5))
    }) else {
        return;
    };
    let active = rgb_of(ColorScheme::light().primary);
    // At a quarter of a wavelength the wave is at its lowest in y-down terms, 10 away from the
    // start, and it comes back up a half wavelength later.
    let painted = |x, y| px(&p, w, x, y)[..3] == active;
    assert!(painted(12, 7) || painted(12, 8), "down at 10");
    assert!(painted(32, 2) || painted(32, 1), "up at 30");
    assert!(!painted(12, 1) && !painted(32, 8));
}

#[test]
fn the_loading_indicator_is_round_about_the_middle_and_moves() {
    let (w, h) = (48, 48);
    let Ok(gpu) = anti_webui::wgpu::Gpu::headless() else {
        return;
    };
    let mut renderer = anti_webui::wgpu::Renderer::new(
        &gpu.device,
        anti_webui::wgpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let mut ui = start_ui(
        || {
            div()
                .bg(Color::WHITE)
                .h(pct(100.0))
                .child(loading_indicator())
        },
        (w, h),
    );
    let start = Instant::now();
    let a = run(&mut ui, start, 0, 16);
    let first = gpu.render_to_rgba(&mut renderer, &a);
    let b = run(&mut ui, start, 24, 900);
    let later = gpu.render_to_rgba(&mut renderer, &b);
    let primary = rgb_of(ColorScheme::light().primary);
    assert_eq!(&px(&first, w as u32, 24, 24)[..3], primary);
    assert_eq!(&px(&later, w as u32, 24, 24)[..3], primary);
    assert_ne!(first, later);
    // It stays inside of its 48 by 48: the corners are left alone.
    for p in [&first, &later] {
        for (x, y) in [(0, 0), (47, 0), (0, 47), (47, 47)] {
            assert_eq!(px(p, w as u32, x, y), [255, 255, 255, 255]);
        }
    }
    assert!(ui.needs_redraw(), "it goes on");
}

#[test]
fn the_label_uses_the_font_that_comes_with_the_crate() {
    let mut ui = start_ui(
        || div().flex().flex_row().h(pct(100.0)).child(button("Save")),
        (300, 100),
    );
    use anti_webui::core::scene::Primitive;
    let scene = ui.paint(Instant::now());
    let run = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => Some(g),
            _ => None,
        })
        .expect("a label");
    // The file in the crate, whole.
    let file = include_bytes!("../fonts/RobotoFlex.ttf");
    assert_eq!(run.font.data.data().len(), file.len());
    // Roboto Flex is a variable font, and its weight goes through the axes.
    assert!(!run.normalized_coords.is_empty());
    assert!(run.normalized_coords.iter().any(|c| *c != 0));
    assert_eq!(run.size, 14.0);
}

#[test]
fn it_shows_on_a_scale_that_is_not_whole() {
    let (w, h) = (300, 100);
    let Some(p) = render(w, h, 1.8, || {
        div()
            .bg(Color::WHITE)
            .h(pct(100.0))
            .flex()
            .flex_row()
            .child(button("Save").w(100.0))
    }) else {
        return;
    };
    let primary = rgb_of(ColorScheme::light().primary);
    // 180 wide and 72 high.
    assert_eq!(&px(&p, w, 90, 70)[..3], primary);
    assert_eq!(px(&p, w, 90, 73), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 182, 36), [255, 255, 255, 255]);
}

#[test]
fn a_page_of_buttons_at_rest_asks_for_no_frames() {
    let mut ui = start_ui(
        || {
            div()
                .flex()
                .flex_row()
                .h(pct(100.0))
                .child(button("A"))
                .child(tonal_button("B"))
                .child(wavy_progress(|| 0.05))
        },
        (400, 100),
    );
    ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
}

#[test]
fn the_keyboard_presses_a_button_too() {
    use anti_webui::core::event::{Key, KeyEvent, NamedKey};
    let clicks = Rc::new(std::cell::Cell::new(0));
    let seen = clicks.clone();
    let mut ui = start_ui(
        move || {
            let seen = seen.clone();
            div().flex().flex_row().h(pct(100.0)).child(
                button("Save")
                    .w(100.0)
                    .on_click(move |_| seen.set(seen.get() + 1)),
            )
        },
        (200, 100),
    );
    let key = |ui: &mut Ui, key: NamedKey, pressed: bool| {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(key),
            text: None,
            modifiers: Modifiers::default(),
            pressed,
            repeat: false,
        }));
    };
    key(&mut ui, NamedKey::Tab, true);
    let scene = run(&mut ui, Instant::now(), 0, 100);
    assert_eq!(shaders(&scene), 1, "focus shows in the state layer");
    key(&mut ui, NamedKey::Enter, true);
    let scene = run(&mut ui, Instant::now(), 0, 100);
    assert_eq!(clicks.get(), 1);
    use anti_webui::core::scene::Primitive;
    let params = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Shader(s) => Some(s.paint.params().to_vec()),
            _ => None,
        })
        .expect("the layer");
    // The first ripple comes after the color and the state, at a multiple of 16: x is negative.
    let x = f32::from_ne_bytes(params[32..36].try_into().unwrap());
    assert_eq!(x, -1.0, "it starts from the center");
    key(&mut ui, NamedKey::Enter, false);
    let scene = run(&mut ui, Instant::now(), 0, 600);
    let params = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Shader(s) => Some(s.paint.params().to_vec()),
            _ => None,
        })
        .expect("focus still shows");
    let strength = f32::from_ne_bytes(params[44..48].try_into().unwrap());
    assert_eq!(strength, 0.0, "the ripple is over");
}
