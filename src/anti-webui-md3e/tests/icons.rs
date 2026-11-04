//! Icons through the real renderer, using nothing but the public API. Skipped when no GPU adapter
//! is available.

use anti_webui::app::headless;
use anti_webui::core::Ui;
use anti_webui::core::event::WindowEvent;
use anti_webui::core::geometry::{Scale, SizeI};
use anti_webui::core::scene::{Primitive, Scene};
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use anti_webui_md3e::components::{icon, sized_icon};
use anti_webui_md3e::icons;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The Vulkan loader doesn't survive being loaded by one thread while another one unloads it, and
/// this file renders many times.
static GPU: Mutex<()> = Mutex::new(());

fn render(w: u32, h: u32, scale: f64, view: impl FnOnce() -> Div) -> Option<Vec<u8>> {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    match headless::render_rgba(w, h, scale, view) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().unwrap()
}

fn page() -> Div {
    div().size_full().bg(Color::WHITE).text_color(Color::BLACK)
}

/// Where something is painted on white: `[left, top, right, bottom]`.
fn ink_box(p: &[u8], w: u32) -> Option<[u32; 4]> {
    let mut out: Option<[u32; 4]> = None;
    for (i, c) in p.chunks_exact(4).enumerate() {
        if c[0] < 128 {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let b = out.get_or_insert([x, y, x + 1, y + 1]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
        }
    }
    out
}

fn ink(p: &[u8]) -> u64 {
    p.chunks_exact(4).map(|c| 255 - c[0] as u64).sum()
}

fn sized(ui: &mut Ui, scale: f64) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 200),
        scale: Scale::from_f64(scale),
    });
    ui.update();
}

fn glyph_runs(scene: &Scene) -> Vec<(u64, Vec<i16>)> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Glyphs(g) => Some((g.font.data.id(), g.normalized_coords.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn the_box_is_square_at_any_scale() {
    for (scale, size, expected) in [
        (1.0, 24.0, 24),
        (1.25, 24.0, 30),
        (1.5, 20.0, 30),
        (1.8, 24.0, 43),
        (2.0, 48.0, 96),
        (1.8, 20.0, 36),
    ] {
        let (width, height) = on_thread(move || {
            let id = Rc::new(Cell::new(None));
            let out = id.clone();
            let mut ui = Ui::new(move || {
                let view = sized_icon(icons::HOME, size).into_view();
                out.set(Some(view.0));
                view
            });
            sized(&mut ui, scale);
            let rect = with_doc(|d| d.get(id.get().unwrap()).unwrap().rect);
            (rect.width, rect.height)
        });
        assert_eq!((width, height), (expected, expected), "{scale} {size}");
    }
}

#[test]
fn it_is_not_squeezed_by_a_row_that_is_too_narrow() {
    let id = Rc::new(Cell::new(None));
    let out = id.clone();
    let mut ui = Ui::new(move || {
        let view = icon(icons::HOME).into_view();
        out.set(Some(view.0));
        div()
            .flex()
            .flex_row()
            .w(30.0)
            .child(view)
            .child(div().w(100.0))
    });
    sized(&mut ui, 1.0);
    assert_eq!(
        with_doc(|d| d.get(id.get().unwrap()).unwrap().rect.width),
        24
    );
}

#[test]
fn the_picture_sits_in_the_middle_of_the_box() {
    // `check_box_outline_blank` is a square around the center of the icon.
    for scale in [1.0, 1.25, 1.5, 1.8, 2.0] {
        let Some(p) = on_thread(move || {
            render(200, 200, scale, || {
                page().child(icon(icons::CHECK_BOX_OUTLINE_BLANK))
            })
        }) else {
            return;
        };
        let [left, top, right, bottom] = ink_box(&p, 200).expect("something painted");
        let size = (24.0_f64 * scale).round() as i64;
        let (left, top, right, bottom) = (left as i64, top as i64, right as i64, bottom as i64);
        assert!(
            (left - (size - right)).abs() <= 1 && (top - (size - bottom)).abs() <= 1,
            "scale {scale}: {left} {top} {right} {bottom} in {size}"
        );
    }
}

#[test]
fn it_takes_the_color_of_the_text_around_it() {
    let Some(p) = on_thread(|| {
        render(40, 40, 1.0, || {
            page()
                .text_color(rgb(0xff0000))
                .child(icon(icons::CHECK_BOX_OUTLINE_BLANK))
        })
    }) else {
        return;
    };
    let reddest = p
        .chunks_exact(4)
        .map(|c| (c[0], c[1]))
        .min_by_key(|c| c.1)
        .unwrap();
    assert_eq!(reddest, (255, 0));
}

#[test]
fn what_the_text_around_it_looks_like_does_not_matter() {
    // At 1.4 the box is a little wider than the glyph, so that alignment shows.
    for scale in [1.0, 1.4] {
        let plain = on_thread(move || render(84, 84, scale, || page().child(icon(icons::HOME))));
        let Some(plain) = plain else {
            return;
        };
        let dressed = on_thread(move || {
            render(84, 84, scale, || {
                page()
                    .bold()
                    .italic()
                    .underline()
                    .strikethrough()
                    .letter_spacing(-5.0)
                    .line_height(LineHeight::Px(60.0))
                    .text_size(40.0)
                    .text_align(TextAlign::Right)
                    .font_family("sans-serif")
                    .child(icon(icons::HOME))
            })
        })
        .unwrap();
        assert!(plain == dressed, "at {scale}");
    }
}

#[test]
fn axes_at_their_defaults_are_the_same_picture() {
    let plain = on_thread(|| render(60, 60, 1.0, || page().child(icon(icons::HOME))));
    let Some(plain) = plain else {
        return;
    };
    let same = on_thread(|| {
        render(60, 60, 1.0, || {
            page().child(
                icon(icons::HOME)
                    .font_axis(b"FILL", 0.0)
                    .font_axis(b"GRAD", 0.0)
                    .font_axis(b"opsz", 24.0)
                    .font_axis(b"wght", 400.0),
            )
        })
    })
    .unwrap();
    assert!(plain == same);
}

#[test]
fn the_axes_change_the_picture() {
    let draw =
        |view: fn() -> Div| on_thread(move || render(60, 60, 1.0, move || page().child(view())));
    let Some(outline) = draw(|| icon(icons::FAVORITE)) else {
        return;
    };
    let filled = draw(|| icon(icons::FAVORITE).font_axis(b"FILL", 1.0)).unwrap();
    let bold = draw(|| icon(icons::FAVORITE).font_weight(FontWeight::BOLD)).unwrap();
    let graded = draw(|| icon(icons::FAVORITE).font_axis(b"GRAD", 200.0)).unwrap();
    assert!(ink(&filled) > ink(&outline) * 2);
    assert!(ink(&bold) > ink(&outline));
    assert!(ink(&graded) > ink(&outline));
    assert!(bold != graded);
}

#[test]
fn the_optical_size_follows_the_size_of_the_icon() {
    let coords = |size: f32| {
        on_thread(move || {
            let mut ui = Ui::new(move || sized_icon(icons::HOME, size));
            sized(&mut ui, 1.8);
            glyph_runs(&ui.paint(Instant::now()))[0].1.clone()
        })
    };
    // 24 is where the font is by default, 20 below it, 48 at the top of the axis.
    let opsz = 2;
    assert!(coords(24.0).iter().all(|c| *c == 0));
    assert!(coords(20.0)[opsz] < 0);
    assert_eq!(coords(48.0)[opsz], 0x4000);
}

#[test]
fn fill_can_animate() {
    let filled = signal(false);
    let mut ui = Ui::new(move || {
        icon(icons::FAVORITE)
            .transition(Transition::ms(100).linear())
            .dyn_style(move |s| s.font_axis(b"FILL", if filled.get() { 1.0 } else { 0.0 }))
    });
    sized(&mut ui, 1.0);
    let fill = |ui: &mut Ui, now: Instant| {
        let scene = ui.paint(now);
        let coords = glyph_runs(&scene)[0].1.clone();
        coords.first().copied().unwrap_or(0)
    };
    let start = Instant::now();
    assert_eq!(fill(&mut ui, start), 0);
    filled.set(true);
    ui.update();
    assert_eq!(fill(&mut ui, start), 0);
    let later = |ms| start + Duration::from_millis(ms);
    ui.animate(later(50));
    ui.update();
    let middle = fill(&mut ui, later(50));
    assert!(middle > 0 && middle < 0x4000, "{middle}");
    ui.animate(later(100));
    ui.update();
    assert_eq!(fill(&mut ui, later(100)), 0x4000);
}

#[test]
fn it_cannot_be_selected_as_text() {
    let id = Rc::new(Cell::new(None));
    let out = id.clone();
    let mut ui = Ui::new(move || {
        let view = icon(icons::HOME).into_view();
        out.set(Some(view.0));
        view
    });
    sized(&mut ui, 1.0);
    let select = with_doc(|d| d.get(id.get().unwrap()).unwrap().computed.user_select);
    assert_eq!(select, UserSelect::None);
}

#[test]
fn the_same_icon_is_one_font_and_another_icon_is_another() {
    let mut ui = Ui::new(|| {
        div()
            .child(icon(icons::HOME))
            .child(icon(icons::HOME))
            .child(icon(icons::SEARCH))
    });
    sized(&mut ui, 1.0);
    let runs = glyph_runs(&ui.paint(Instant::now()));
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].0, runs[1].0);
    assert_ne!(runs[0].0, runs[2].0);
}

#[test]
fn an_icon_that_comes_later_leaves_what_is_on_screen_alone() {
    let mut ui = Ui::new(|| text("Some text"));
    sized(&mut ui, 1.0);
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
    let _ = icon(icons::DOWNLOAD);
    assert!(!ui.needs_redraw());
}

#[test]
fn the_same_icon_again_leaves_what_is_on_screen_alone() {
    // Two places, which a constant holding the font itself would give two addresses.
    let mut ui = Ui::new(|| icon(icons::HOME));
    sized(&mut ui, 1.0);
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
    let _ = icon(icons::HOME);
    assert!(!ui.needs_redraw());
}

#[test]
fn a_font_of_your_own_makes_an_icon_too() {
    static OWN: icons::Symbol = icons::Symbol {
        font: include_bytes!("../fonts/RobotoFlex.ttf"),
        family: "An Icon Font Of Your Own",
        ch: 'A',
    };
    let Some(p) = on_thread(|| render(60, 60, 1.0, || page().child(icon(&OWN)))) else {
        return;
    };
    assert!(ink(&p) > 0);
}
