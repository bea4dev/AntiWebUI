//! The button family, using nothing but the public API. See `docs/design/19-buttons.md`.

use anti_webui::app::headless;
use anti_webui::core::Ui;
use anti_webui::core::event::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, WindowEvent};
use anti_webui::core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui::core::scene::{Primitive, Quad, Scene};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    ButtonSize, ButtonStyle, FabColor, FabSize, FabStyle, IconButtonStyle, IconButtonWidth, button,
    button_group, connected_button_group, extended_fab, fab, filled_icon_button, icon_button,
    icon_toggle_button, outlined_icon_button, styled_button, styled_extended_fab, styled_fab,
    styled_icon_button, styled_toggle_button, toggle_button, tonal_button, tonal_icon_button,
};
use anti_webui_md3e::typography::TextStyle;
use anti_webui_md3e::{ColorScheme, icons};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// See `tests/icons.rs`.
static GPU: Mutex<()> = Mutex::new(());

fn start_ui(view: impl FnOnce() -> Div + 'static) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 400),
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
    ui.update();
}

fn pointer(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

fn click(ui: &mut Ui, x: f32, y: f32) {
    pointer_at(ui, x, y);
    pointer(ui, x, y, true);
    pointer(ui, x, y, false);
}

fn run(ui: &mut Ui, start: Instant, to: u64) -> Scene {
    let mut scene = None;
    for t in (0..=to).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        scene = Some(ui.paint(now));
    }
    scene.expect("a frame")
}

fn settle(ui: &mut Ui) -> Scene {
    run(ui, Instant::now(), 1500)
}

/// The biggest box of the scene, which is the button when it stands alone.
fn the_box(scene: &Scene) -> Quad {
    boxes(scene)
        .into_iter()
        .max_by_key(|q| q.rect.width * q.rect.height)
        .expect("a box")
}

fn boxes(scene: &Scene) -> Vec<Quad> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) => Some(q.clone()),
            _ => None,
        })
        .collect()
}

fn rgb_of(c: Color) -> [u8; 3] {
    let p = c.premultiplied();
    [0, 1, 2].map(|i| (p[i] * 255.0).round() as u8)
}

/// The window rectangle of a view, as laid out.
fn rect_of(view: impl FnOnce() -> Div + 'static) -> RectI {
    let place = node_ref();
    let mut ui = start_ui(move || div().flex().child(view().node_ref(place)));
    let _ = ui.paint(Instant::now());
    place.rect().expect("laid out")
}

fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().unwrap()
}

fn render(w: u32, h: u32, view: impl FnOnce() -> Div) -> Option<Vec<u8>> {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    match headless::render_rgba(w, h, 1.0, view) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn enter(ui: &mut Ui, pressed: bool) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        text: None,
        modifiers: Modifiers::default(),
        pressed,
        repeat: false,
    }));
    ui.update();
}

fn tab(ui: &mut Ui) {
    let key = |pressed| {
        WindowEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Tab),
            text: None,
            modifiers: Modifiers::default(),
            pressed,
            repeat: false,
        })
    };
    ui.handle_event(key(true));
    ui.handle_event(key(false));
    ui.update();
}

// Buttons with icons

#[test]
fn an_icon_takes_its_size_and_a_gap_in_a_button() {
    let plain = on_thread(|| rect_of(|| button("Save")).width);
    let with = on_thread(|| {
        rect_of(|| styled_button("Save", ButtonStyle::filled().icon(icons::SAVE))).width
    });
    assert_eq!(with - plain, 20 + 8);
    let both = on_thread(|| {
        rect_of(|| {
            styled_button(
                "Save",
                ButtonStyle::filled()
                    .icon(icons::SAVE)
                    .trailing_icon(icons::ARROW_FORWARD),
            )
        })
        .width
    });
    assert_eq!(both - plain, 2 * (20 + 8));
}

#[test]
fn the_icon_of_a_button_follows_its_size() {
    for (size, icon, gap) in [
        (ButtonSize::ExtraSmall, 20, 4),
        (ButtonSize::Small, 20, 8),
        (ButtonSize::Medium, 24, 8),
        (ButtonSize::Large, 32, 12),
        (ButtonSize::ExtraLarge, 40, 16),
    ] {
        let plain = on_thread(move || {
            rect_of(move || styled_button("Save", ButtonStyle::filled().size(size))).width
        });
        let with = on_thread(move || {
            rect_of(move || {
                styled_button("Save", ButtonStyle::filled().size(size).icon(icons::SAVE))
            })
            .width
        });
        assert_eq!(with - plain, icon + gap, "{size:?}");
    }
}

// Toggle buttons

fn colors_of(view: impl FnOnce() -> Div + Send + 'static) -> Option<[u8; 3]> {
    on_thread(move || {
        let p = render(200, 80, || div().flex().child(view()))?;
        // Inside the box, away from its label.
        Some([
            p[(20 * 200 + 8) * 4],
            p[(20 * 200 + 8) * 4 + 1],
            p[(20 * 200 + 8) * 4 + 2],
        ])
    })
}

#[test]
fn a_toggle_button_has_the_colors_of_its_state() {
    let c = ColorScheme::light();
    type Case = (fn() -> ButtonStyle, Color, Color);
    let cases: [Case; 4] = [
        (ButtonStyle::filled, c.surface_container, c.primary),
        (ButtonStyle::tonal, c.secondary_container, c.secondary),
        (ButtonStyle::elevated, c.surface_container_low, c.primary),
        (ButtonStyle::outlined, Color::TRANSPARENT, c.inverse_surface),
    ];
    for (style, off, on) in cases {
        let at = |checked: bool| {
            colors_of(move || {
                div()
                    .bg(Color::WHITE)
                    .size_full()
                    .flex()
                    .child(styled_toggle_button("Toggle", signal(checked), style()))
            })
        };
        let Some(unchecked) = at(false) else {
            return;
        };
        let expected = if off.is_transparent() {
            [255, 255, 255]
        } else {
            rgb_of(off)
        };
        assert_eq!(unchecked, expected);
        assert_eq!(at(true).unwrap(), rgb_of(on));
    }
}

#[test]
fn a_toggle_button_changes_shape_with_its_state_and_bounces() {
    let checked = signal(false);
    let mut ui = start_ui(move || div().flex().child(toggle_button("Toggle", checked)));
    let round = the_box(&settle(&mut ui)).radii[0];
    assert_eq!(round, 20.0);
    pointer_at(&mut ui, 30.0, 20.0);
    pointer(&mut ui, 30.0, 20.0, true);
    let pressed = the_box(&settle(&mut ui)).radii[0];
    assert_eq!(pressed, 8.0);
    pointer(&mut ui, 30.0, 20.0, false);
    assert!(checked.get_untracked());
    // On its way to the shape of the checked state, it goes past it: rounded by 12 at the end.
    let start = Instant::now();
    let mut most = 0.0_f32;
    for t in (0..=400).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        most = most.max(the_box(&ui.paint(now)).radii[0]);
    }
    assert!(most > 12.2, "it should overshoot: {most}");
    assert_eq!(the_box(&settle(&mut ui)).radii[0], 12.0);
}

#[test]
fn pressing_a_checked_toggle_button_takes_the_shape_of_a_press() {
    let mut ui = start_ui(|| div().flex().child(toggle_button("Toggle", signal(true))));
    assert_eq!(the_box(&settle(&mut ui)).radii[0], 12.0);
    pointer_at(&mut ui, 30.0, 20.0);
    pointer(&mut ui, 30.0, 20.0, true);
    assert_eq!(the_box(&settle(&mut ui)).radii[0], 8.0);
}

#[test]
fn square_goes_the_other_way() {
    let mut ui = start_ui(|| {
        div().flex().child(styled_toggle_button(
            "Toggle",
            signal(false),
            ButtonStyle::filled().square(),
        ))
    });
    assert_eq!(the_box(&settle(&mut ui)).radii[0], 12.0);
    let on = on_thread(|| {
        let mut ui = start_ui(|| {
            div().flex().child(styled_toggle_button(
                "Toggle",
                signal(true),
                ButtonStyle::filled().square(),
            ))
        });
        the_box(&settle(&mut ui)).radii[0]
    });
    assert_eq!(on, 20.0);
}

#[test]
fn a_toggle_button_that_starts_checked_starts_in_shape_and_asks_for_nothing() {
    let mut ui = start_ui(|| div().flex().child(toggle_button("Toggle", signal(true))));
    let scene = ui.paint(Instant::now());
    assert_eq!(the_box(&scene).radii[0], 12.0);
    assert!(!ui.needs_redraw());
}

#[test]
fn an_outlined_toggle_button_stays_as_wide_when_checked() {
    let checked = signal(false);
    let place = node_ref();
    let mut ui = start_ui(move || {
        div()
            .flex()
            .child(styled_toggle_button("Toggle", checked, ButtonStyle::outlined()).node_ref(place))
    });
    let _ = settle(&mut ui);
    let before = place.rect().unwrap();
    checked.set(true);
    let _ = settle(&mut ui);
    assert_eq!(place.rect().unwrap(), before);
}

/// The value of an axis in the first glyphs of the scene.
fn fill_of(scene: &Scene) -> i16 {
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            // `FILL` comes first in the font.
            Primitive::Glyphs(g) if g.font.data.data().len() < 100_000 => {
                Some(g.normalized_coords.first().copied().unwrap_or(0))
            }
            _ => None,
        })
        .expect("an icon")
}

#[test]
fn the_icon_of_a_toggle_fills_in_when_checked() {
    let checked = signal(false);
    let mut ui = start_ui(move || {
        div().flex().child(styled_toggle_button(
            "Toggle",
            checked,
            ButtonStyle::filled().icon(icons::FAVORITE),
        ))
    });
    assert_eq!(fill_of(&settle(&mut ui)), 0);
    checked.set(true);
    let start = Instant::now();
    let middle = fill_of(&run(&mut ui, start, 100));
    assert!(middle > 0 && middle < 0x4000, "{middle}");
    assert_eq!(fill_of(&settle(&mut ui)), 0x4000);
}

#[test]
fn the_icon_of_a_plain_button_does_not_fill_in() {
    let mut ui = start_ui(|| {
        div().flex().child(styled_button(
            "Save",
            ButtonStyle::filled().icon(icons::FAVORITE),
        ))
    });
    assert_eq!(fill_of(&settle(&mut ui)), 0);
}

// Icon buttons

#[test]
fn an_icon_button_has_the_size_of_the_tokens() {
    for (size, height, widths) in [
        (ButtonSize::ExtraSmall, 32, [28, 32, 40]),
        (ButtonSize::Small, 40, [32, 40, 52]),
        (ButtonSize::Medium, 56, [48, 56, 72]),
        (ButtonSize::Large, 96, [64, 96, 128]),
        (ButtonSize::ExtraLarge, 136, [104, 136, 184]),
    ] {
        for (width, expected) in [
            IconButtonWidth::Narrow,
            IconButtonWidth::Default,
            IconButtonWidth::Wide,
        ]
        .into_iter()
        .zip(widths)
        {
            let rect = on_thread(move || {
                rect_of(move || {
                    styled_icon_button(
                        icons::ADD,
                        IconButtonStyle::filled().size(size).width(width),
                    )
                })
            });
            assert_eq!(
                (rect.width, rect.height),
                (expected, height),
                "{size:?} {width:?}"
            );
        }
    }
}

#[test]
fn a_narrow_icon_button_is_as_round_as_its_narrow_side_allows() {
    let mut ui = start_ui(|| {
        div().flex().child(styled_icon_button(
            icons::ADD,
            IconButtonStyle::filled().width(IconButtonWidth::Narrow),
        ))
    });
    assert_eq!(the_box(&settle(&mut ui)).radii[0], 16.0);
}

#[test]
fn the_kinds_of_icon_button_differ_like_the_tokens_say() {
    let c = ColorScheme::light();
    type Case = (fn() -> Div, Option<Color>);
    let cases: [Case; 4] = [
        (|| icon_button(icons::ADD), None),
        (|| filled_icon_button(icons::ADD), Some(c.primary)),
        (
            || tonal_icon_button(icons::ADD),
            Some(c.secondary_container),
        ),
        (|| outlined_icon_button(icons::ADD), None),
    ];
    for (view, background) in cases {
        let scene = on_thread(move || {
            let mut ui = start_ui(move || div().flex().child(view()));
            boxes(&settle(&mut ui))
                .into_iter()
                .filter(|q| !q.background.is_transparent())
                .map(|q| rgb_of(q.background))
                .next()
        });
        assert_eq!(scene, background.map(rgb_of));
    }
}

#[test]
fn an_outlined_icon_button_has_a_border_and_a_standard_one_has_nothing() {
    let border = |view: fn() -> Div| {
        on_thread(move || {
            let mut ui = start_ui(move || div().flex().child(view()));
            boxes(&settle(&mut ui))
                .iter()
                .map(|q| q.border_widths)
                .next()
        })
    };
    assert_eq!(
        border(|| outlined_icon_button(icons::ADD)),
        Some([1, 1, 1, 1])
    );
    assert_eq!(border(|| icon_button(icons::ADD)), None);
}

#[test]
fn an_icon_toggle_takes_the_color_of_the_checked_state() {
    let c = ColorScheme::light();
    let at = |style: fn() -> IconButtonStyle, checked: bool| {
        on_thread(move || {
            let mut ui = start_ui(move || {
                div()
                    .flex()
                    .child(icon_toggle_button(icons::ADD, signal(checked), style()))
            });
            boxes(&settle(&mut ui))
                .first()
                .map(|q| rgb_of(q.background))
        })
    };
    assert_eq!(
        at(IconButtonStyle::filled, false),
        Some(rgb_of(c.surface_container))
    );
    assert_eq!(at(IconButtonStyle::filled, true), Some(rgb_of(c.primary)));
    assert_eq!(at(IconButtonStyle::tonal, true), Some(rgb_of(c.secondary)));
    assert_eq!(
        at(IconButtonStyle::outlined, true),
        Some(rgb_of(c.inverse_surface))
    );
    assert_eq!(at(IconButtonStyle::standard, true), None);
}

#[test]
fn an_icon_toggle_does_not_bounce_and_its_icon_fills_in() {
    let checked = signal(false);
    let mut ui = start_ui(move || {
        div().flex().child(icon_toggle_button(
            icons::FAVORITE,
            checked,
            IconButtonStyle::filled(),
        ))
    });
    assert_eq!(the_box(&settle(&mut ui)).radii[0], 20.0);
    checked.set(true);
    let start = Instant::now();
    let mut least = f32::MAX;
    for t in (0..=400).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        least = least.min(the_box(&ui.paint(now)).radii[0]);
    }
    assert!(least >= 11.99, "it shouldn't go past 12: {least}");
    let scene = settle(&mut ui);
    assert_eq!(the_box(&scene).radii[0], 12.0);
    assert_eq!(fill_of(&scene), 0x4000);
}

fn glyph_color(scene: &Scene) -> Color {
    scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.color),
            _ => None,
        })
        .expect("some glyphs")
}

#[test]
fn what_is_disabled_has_the_colors_of_the_tokens() {
    let c = ColorScheme::light();
    let mut ui = start_ui(|| {
        div().flex().child(styled_button(
            "Save",
            ButtonStyle::filled().disabled_when(|| true),
        ))
    });
    let scene = settle(&mut ui);
    assert_eq!(
        the_box(&scene).background,
        c.on_surface.with_alpha(0.1).snapped()
    );
    assert_eq!(
        glyph_color(&scene),
        c.on_surface_variant.with_alpha(0.38).snapped()
    );
    let (background, content) = on_thread(|| {
        let mut ui = start_ui(|| {
            div().flex().child(styled_icon_button(
                icons::ADD,
                IconButtonStyle::filled().disabled_when(|| true),
            ))
        });
        let scene = settle(&mut ui);
        (the_box(&scene).background, glyph_color(&scene))
    });
    assert_eq!(background, c.on_surface.with_alpha(0.1).snapped());
    assert_eq!(content, c.on_surface.with_alpha(0.38).snapped());
}

#[test]
fn a_disabled_icon_button_does_not_react() {
    let clicks = Rc::new(Cell::new(0));
    let counted = clicks.clone();
    let mut ui = start_ui(move || {
        div().flex().child(
            styled_icon_button(icons::ADD, IconButtonStyle::filled().disabled_when(|| true))
                .on_click(move |_| counted.set(counted.get() + 1)),
        )
    });
    let _ = settle(&mut ui);
    click(&mut ui, 20.0, 20.0);
    assert_eq!(clicks.get(), 0);
}

// Floating action buttons

#[test]
fn a_fab_has_the_size_and_shape_of_the_tokens() {
    for (size, side, radius) in [
        (FabSize::Baseline, 56, 16.0),
        (FabSize::Medium, 80, 20.0),
        (FabSize::Large, 96, 28.0),
    ] {
        let (w, h, r) = on_thread(move || {
            let mut ui = start_ui(move || {
                div()
                    .flex()
                    .child(styled_fab(icons::ADD, FabStyle::new().size(size)))
            });
            let q = the_box(&settle(&mut ui));
            (q.rect.width, q.rect.height, q.radii[0])
        });
        assert_eq!((w, h, r), (side, side, radius), "{size:?}");
    }
}

#[test]
fn a_fab_has_the_color_it_is_given() {
    let c = ColorScheme::light();
    for (color, expected) in [
        (FabColor::PrimaryContainer, c.primary_container),
        (FabColor::SecondaryContainer, c.secondary_container),
        (FabColor::TertiaryContainer, c.tertiary_container),
        (FabColor::Primary, c.primary),
        (FabColor::Secondary, c.secondary),
        (FabColor::Tertiary, c.tertiary),
    ] {
        let got = on_thread(move || {
            let mut ui = start_ui(move || {
                div()
                    .flex()
                    .child(styled_fab(icons::ADD, FabStyle::new().color(color)))
            });
            rgb_of(the_box(&settle(&mut ui)).background)
        });
        assert_eq!(got, rgb_of(expected));
    }
}

fn shadow_blurs(scene: &Scene) -> Vec<f32> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Shadow(s) => Some(s.blur),
            _ => None,
        })
        .collect()
}

#[test]
fn a_fab_floats_higher_under_the_pointer_and_settles_when_pressed() {
    let mut ui = start_ui(|| div().flex().p(20.0).child(fab(icons::ADD)));
    let rest = shadow_blurs(&settle(&mut ui));
    assert_eq!(rest, [8.0, 3.0]);
    pointer_at(&mut ui, 40.0, 40.0);
    assert_eq!(shadow_blurs(&settle(&mut ui)), [10.0, 3.0]);
    pointer(&mut ui, 40.0, 40.0, true);
    assert_eq!(shadow_blurs(&settle(&mut ui)), rest);
}

#[test]
fn a_fab_at_rest_asks_for_no_frames() {
    let mut ui = start_ui(|| div().flex().child(fab(icons::ADD)));
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
}

#[test]
fn an_extended_fab_holds_its_icon_and_label() {
    let plain = on_thread(|| rect_of(|| fab(icons::ADD)));
    let (extended, label) = on_thread(|| {
        let places = [node_ref(), node_ref()];
        let mut ui = start_ui(move || {
            div()
                .flex()
                .flex_col()
                .items_start()
                .child(extended_fab(icons::ADD, "Compose").node_ref(places[0]))
                .child(
                    div()
                        .child(TextStyle::TITLE_MEDIUM.apply(text("Compose")))
                        .node_ref(places[1]),
                )
        });
        let _ = ui.paint(Instant::now());
        (places[0].rect().unwrap(), places[1].rect().unwrap())
    });
    assert_eq!(extended.height, plain.height);
    assert_eq!(extended.width, 16 + 24 + 8 + label.width + 16);
}

#[test]
fn an_extended_fab_closes_over_its_label_and_opens_again() {
    let open = signal(true);
    let place = node_ref();
    let mut ui = start_ui(move || {
        div().flex().child(
            styled_extended_fab(
                icons::ADD,
                "Compose",
                FabStyle::new().expanded_when(move || open.get()),
            )
            .node_ref(place),
        )
    });
    let _ = settle(&mut ui);
    let wide = place.rect().unwrap().width;
    open.set(false);
    let start = Instant::now();
    let _ = run(&mut ui, start, 24);
    let middle = place.rect().unwrap().width;
    assert!(middle > 56 && middle < wide, "{middle} of {wide}");
    let _ = settle(&mut ui);
    assert_eq!(place.rect().unwrap().width, 56);
    assert!(!ui.needs_redraw());
    open.set(true);
    let _ = settle(&mut ui);
    assert_eq!(place.rect().unwrap().width, wide);
}

#[test]
fn an_extended_fab_that_starts_closed_is_as_wide_as_a_plain_one() {
    for (size, side) in [
        (FabSize::Baseline, 56),
        (FabSize::Medium, 80),
        (FabSize::Large, 96),
    ] {
        let width = on_thread(move || {
            rect_of(move || {
                styled_extended_fab(
                    icons::ADD,
                    "Compose",
                    FabStyle::new().size(size).expanded_when(|| false),
                )
            })
            .width
        });
        assert_eq!(width, side, "{size:?}");
    }
}

#[test]
fn an_open_extended_fab_follows_a_label_that_changes() {
    let label = signal("A".to_string());
    let place = node_ref();
    let mut ui = start_ui(move || {
        div()
            .flex()
            .child(extended_fab(icons::ADD, move || label.get()).node_ref(place))
    });
    let _ = settle(&mut ui);
    let short = place.rect().unwrap().width;
    label.set("A much longer label".to_string());
    let _ = settle(&mut ui);
    assert!(place.rect().unwrap().width > short + 50);
}

// Button groups

struct Group {
    ui: Ui,
    places: [NodeRef; 3],
}

fn group_of_three() -> Group {
    let places = [node_ref(), node_ref(), node_ref()];
    let ui = start_ui(move || {
        div().flex().child(button_group([
            button("First").node_ref(places[0]),
            tonal_button("Second").node_ref(places[1]),
            button("Third").node_ref(places[2]),
        ]))
    });
    Group { ui, places }
}

impl Group {
    fn rects(&self) -> [RectI; 3] {
        self.places.map(|p| p.rect().unwrap())
    }
    fn widths(&self) -> [i32; 3] {
        self.rects().map(|r| r.width)
    }
    fn press(&mut self, i: usize) {
        let r = self.rects()[i];
        let (x, y) = ((r.x + r.width / 2) as f32, (r.y + r.height / 2) as f32);
        pointer_at(&mut self.ui, x, y);
        pointer(&mut self.ui, x, y, true);
    }
    fn release(&mut self, i: usize) {
        let r = self.rects()[i];
        let (x, y) = ((r.x + r.width / 2) as f32, (r.y + r.height / 2) as f32);
        pointer(&mut self.ui, x, y, false);
    }
}

#[test]
fn buttons_of_a_group_keep_a_gap() {
    let mut g = group_of_three();
    let _ = settle(&mut g.ui);
    let r = g.rects();
    assert_eq!(r[1].x - (r[0].x + r[0].width), 12);
    assert_eq!(r[2].x - (r[1].x + r[1].width), 12);
}

#[test]
fn a_pressed_button_in_the_middle_takes_from_both_neighbors() {
    let mut g = group_of_three();
    let _ = settle(&mut g.ui);
    let before = g.widths();
    let end = g.rects()[2].x + before[2];
    g.press(1);
    let _ = settle(&mut g.ui);
    let during = g.widths();
    let expected = (before[1] as f32 * 0.15).round() as i32;
    assert!(
        (during[1] - before[1] - expected).abs() <= 2,
        "{before:?} {during:?}"
    );
    assert!(
        ((before[0] - during[0]) * 2 - expected).abs() <= 2,
        "{before:?} {during:?}"
    );
    assert!(
        ((before[2] - during[2]) * 2 - expected).abs() <= 2,
        "{before:?} {during:?}"
    );
    let r = g.rects();
    assert!(
        (r[2].x + r[2].width - end).abs() <= 2,
        "the row stays as long"
    );
    assert_eq!(r[1].x - (r[0].x + r[0].width), 12, "the gap stays");
    g.release(1);
    let _ = settle(&mut g.ui);
    assert_eq!(g.widths(), before);
    assert!(!g.ui.needs_redraw());
}

#[test]
fn a_pressed_button_at_the_end_takes_from_its_one_neighbor() {
    let mut g = group_of_three();
    let _ = settle(&mut g.ui);
    let before = g.widths();
    g.press(0);
    let _ = settle(&mut g.ui);
    let during = g.widths();
    let expected = (before[0] as f32 * 0.15).round() as i32;
    assert!(
        (during[0] - before[0] - expected).abs() <= 2,
        "{before:?} {during:?}"
    );
    assert!(
        (before[1] - during[1] - expected).abs() <= 2,
        "{before:?} {during:?}"
    );
    assert_eq!(during[2], before[2]);
}

#[test]
fn a_neighbor_gives_up_no_more_than_the_limit() {
    let places = [node_ref(), node_ref()];
    let mut ui = start_ui(move || {
        div().flex().child(button_group([
            button("A very long label on this button, over two hundred wide").node_ref(places[0]),
            button("B").node_ref(places[1]),
        ]))
    });
    let _ = settle(&mut ui);
    let before = places.map(|p| p.rect().unwrap().width);
    assert!(before[0] as f32 * 0.15 > 24.0, "{before:?}");
    let r = places[0].rect().unwrap();
    pointer_at(&mut ui, (r.x + 10) as f32, (r.y + 10) as f32);
    pointer(&mut ui, (r.x + 10) as f32, (r.y + 10) as f32, true);
    let _ = settle(&mut ui);
    assert_eq!(places[0].rect().unwrap().width - before[0], 24);
}

#[test]
fn a_button_alone_in_a_group_does_not_grow() {
    let place = node_ref();
    let mut ui = start_ui(move || {
        div()
            .flex()
            .child(button_group([button("Alone").node_ref(place)]))
    });
    let _ = settle(&mut ui);
    let before = place.rect().unwrap();
    pointer_at(&mut ui, 20.0, 20.0);
    pointer(&mut ui, 20.0, 20.0, true);
    let _ = settle(&mut ui);
    assert_eq!(place.rect().unwrap(), before);
}

#[test]
fn the_keyboard_presses_a_button_in_a_group_too() {
    let mut g = group_of_three();
    let _ = settle(&mut g.ui);
    let before = g.widths();
    tab(&mut g.ui);
    enter(&mut g.ui, true);
    let _ = settle(&mut g.ui);
    assert!(g.widths()[0] > before[0], "{:?}", g.widths());
    enter(&mut g.ui, false);
    let _ = settle(&mut g.ui);
    assert_eq!(g.widths(), before);
}

#[test]
fn a_key_held_while_the_focus_moves_on_does_not_stay_pressed() {
    let mut g = group_of_three();
    let _ = settle(&mut g.ui);
    let before = g.widths();
    tab(&mut g.ui);
    enter(&mut g.ui, true);
    tab(&mut g.ui);
    let _ = settle(&mut g.ui);
    assert_eq!(g.widths(), before);
}

#[test]
fn what_is_given_to_a_button_in_a_group_still_works() {
    let clicks = Rc::new(Cell::new(0));
    let counted = clicks.clone();
    let mut ui = start_ui(move || {
        div().flex().child(button_group([
            button("One").on_click(move |_| counted.set(counted.get() + 1)),
            button("Two"),
        ]))
    });
    let _ = settle(&mut ui);
    click(&mut ui, 20.0, 20.0);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn an_icon_button_grows_in_a_group_too() {
    let places = [node_ref(), node_ref()];
    let mut ui = start_ui(move || {
        div().flex().child(button_group([
            filled_icon_button(icons::ADD).node_ref(places[0]),
            tonal_icon_button(icons::EDIT).node_ref(places[1]),
        ]))
    });
    let _ = settle(&mut ui);
    let before = places.map(|p| p.rect().unwrap().width);
    pointer_at(&mut ui, 20.0, 20.0);
    pointer(&mut ui, 20.0, 20.0, true);
    let _ = settle(&mut ui);
    assert_eq!(places[0].rect().unwrap().width, before[0] + 6);
    assert_eq!(places[1].rect().unwrap().width, before[1] - 6);
}

// Connected groups

fn connected(selected: Signal<usize>) -> Ui {
    start_ui(move || {
        div().flex().child(connected_button_group(
            ["Day", "Week", "Month"],
            ButtonStyle::filled(),
            move |i| selected.get() == i,
            move |i| selected.set(i),
        ))
    })
}

#[test]
fn connected_buttons_touch_and_their_corners_tell_their_place() {
    let mut ui = connected(signal(9));
    let scene = settle(&mut ui);
    let b = boxes(&scene);
    assert_eq!(b.len(), 3);
    assert_eq!(b[0].radii, [20.0, 8.0, 8.0, 20.0]);
    assert_eq!(b[1].radii, [8.0; 4]);
    assert_eq!(b[2].radii, [8.0, 20.0, 20.0, 8.0]);
    assert_eq!(b[1].rect.x - (b[0].rect.x + b[0].rect.width), 2);
}

#[test]
fn a_connected_button_tightens_its_inner_corners_when_pressed_and_rounds_all_when_selected() {
    let selected = signal(9);
    let mut ui = connected(selected);
    let first = boxes(&settle(&mut ui))[0].rect;
    let (x, y) = ((first.x + 10) as f32, (first.y + 10) as f32);
    pointer_at(&mut ui, x, y);
    pointer(&mut ui, x, y, true);
    assert_eq!(boxes(&settle(&mut ui))[0].radii, [20.0, 4.0, 4.0, 20.0]);
    pointer(&mut ui, x, y, false);
    assert_eq!(selected.get_untracked(), 0);
    assert_eq!(boxes(&settle(&mut ui))[0].radii, [20.0; 4]);
}

#[test]
fn a_connected_group_of_one_is_a_pill() {
    let mut ui = start_ui(|| {
        div().flex().child(connected_button_group(
            ["Only"],
            ButtonStyle::tonal(),
            |_| false,
            |_| {},
        ))
    });
    assert_eq!(boxes(&settle(&mut ui))[0].radii, [20.0; 4]);
}

#[test]
fn a_page_of_all_of_them_at_rest_asks_for_no_frames() {
    let mut ui = start_ui(|| {
        div()
            .flex()
            .flex_col()
            .child(styled_button("A", ButtonStyle::filled().icon(icons::ADD)))
            .child(toggle_button("B", signal(true)))
            .child(icon_button(icons::ADD))
            .child(icon_toggle_button(
                icons::ADD,
                signal(true),
                IconButtonStyle::filled(),
            ))
            .child(fab(icons::ADD))
            .child(extended_fab(icons::ADD, "C"))
            .child(button_group([button("D"), button("E")]))
            .child(connected_button_group(
                ["F", "G"],
                ButtonStyle::tonal(),
                |_| true,
                |_| {},
            ))
    });
    let _ = ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
}

// What moves, frame by frame

fn at_scale(view: impl FnOnce() -> Div + 'static) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1000, 400),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    ui
}

fn watch(ui: &mut Ui, milliseconds: u64, mut f: impl FnMut(&Scene)) {
    let start = Instant::now();
    for t in (0..=milliseconds).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        f(&ui.paint(now));
    }
}

fn biggest_step(values: &[i32]) -> i32 {
    values
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .max()
        .unwrap_or(0)
}

#[test]
fn an_extended_fab_opens_in_one_movement() {
    for label in ["Compose", "A longer label than that"] {
        let open = signal(true);
        let place = node_ref();
        let mut ui = at_scale(move || {
            div().flex().child(
                styled_extended_fab(
                    icons::EDIT,
                    label,
                    FabStyle::new().expanded_when(move || open.get()),
                )
                .node_ref(place),
            )
        });
        settle(&mut ui);
        let full = place.rect().unwrap().width;
        open.set(false);
        settle(&mut ui);
        let closed = place.rect().unwrap().width;
        open.set(true);
        let mut widths = Vec::new();
        watch(&mut ui, 800, |_| widths.push(place.rect().unwrap().width));
        assert_eq!(*widths.last().unwrap(), full);
        // Springs are fastest in the middle, and a frame covers less than a third of the way.
        let step = biggest_step(&widths);
        assert!(
            step < (full - closed) / 3,
            "{label}: a step of {step} between {closed} and {full}: {widths:?}"
        );
        assert!(widths.iter().any(|w| *w > full), "it goes past: {widths:?}");
    }
}

#[test]
fn an_extended_fab_that_starts_closed_opens_in_one_movement_too() {
    let open = signal(false);
    let place = node_ref();
    let mut ui = at_scale(move || {
        div().flex().child(
            styled_extended_fab(
                icons::EDIT,
                "Compose",
                FabStyle::new().expanded_when(move || open.get()),
            )
            .node_ref(place),
        )
    });
    settle(&mut ui);
    let closed = place.rect().unwrap().width;
    open.set(true);
    let mut widths = Vec::new();
    watch(&mut ui, 800, |_| widths.push(place.rect().unwrap().width));
    let full = *widths.last().unwrap();
    assert!(biggest_step(&widths) < (full - closed) / 3, "{widths:?}");
}

/// Presses a button of a group at a scale that doesn't round evenly, and gives the left edge of
/// each button and the first glyph of each label, for each frame.
fn pressed_group(at: f32) -> Vec<Vec<(i32, f32)>> {
    let colors = [rgb(0x101010), rgb(0x202020), rgb(0x303030)];
    let mut ui = at_scale(move || {
        div().flex().child(button_group(colors.map(|bg| {
            styled_button("Word", ButtonStyle::filled()).dyn_style(move |s| s.bg(bg))
        })))
    });
    settle(&mut ui);
    pointer_at(&mut ui, at, 36.0);
    pointer(&mut ui, at, 36.0, true);
    let mut frames = Vec::new();
    let mut record = |scene: &Scene| {
        let mut row = Vec::new();
        for color in colors {
            let q = boxes(scene)
                .into_iter()
                .find(|q| q.background == color.snapped())
                .expect("a button");
            let glyph = scene
                .primitives
                .iter()
                .find_map(|p| match p {
                    Primitive::Glyphs(g)
                        if q.rect
                            .contains(PointF::new(g.glyphs[0].x, g.glyphs[0].y - 1.0)) =>
                    {
                        Some(g.glyphs[0].x)
                    }
                    _ => None,
                })
                .expect("a label");
            row.push((q.rect.x, glyph));
            row.push((q.rect.right(), glyph));
        }
        frames.push(row);
    };
    watch(&mut ui, 500, &mut record);
    pointer(&mut ui, at, 36.0, false);
    watch(&mut ui, 500, &mut record);
    frames
}

#[test]
fn what_a_button_of_a_group_takes_is_exactly_what_the_others_give() {
    for at in [20.0, 200.0, 330.0] {
        let frames = pressed_group(at);
        let first_left = frames[0][0].0;
        let last_right = frames[0][5].0;
        let gap = frames[0][2].0 - frames[0][1].0;
        for frame in &frames {
            assert_eq!(frame[0].0, first_left, "the left end stays");
            assert_eq!(frame[5].0, last_right, "the right end stays");
            assert_eq!(frame[2].0 - frame[1].0, gap);
            assert_eq!(frame[4].0 - frame[3].0, gap);
        }
    }
}

#[test]
fn the_label_of_a_button_pressed_between_two_others_keeps_still() {
    let frames = pressed_group(200.0);
    let moved = frames[0][2].0 != frames[60][2].0;
    assert!(moved, "the button got wider");
    assert!(frames.iter().all(|f| f[2].1 == frames[0][2].1));
}

#[test]
fn the_labels_of_a_group_do_not_shake_on_the_way() {
    // Up to where the spring turns around, what is next to a pressed button goes one way.
    let frames = pressed_group(200.0);
    let peak = (0..frames.len().min(60))
        .max_by_key(|&i| frames[i][3].0)
        .unwrap_or(0);
    let peak = (0..=peak.min(60))
        .min_by_key(|&i| frames[i][1].0)
        .unwrap_or(0);
    for i in 1..=peak {
        assert!(
            frames[i][0].1 <= frames[i - 1][0].1 + 0.01,
            "frame {i}: {} after {}",
            frames[i][0].1,
            frames[i - 1][0].1
        );
    }
}

#[test]
fn pressing_a_button_of_a_group_again_on_its_way_back_does_not_jump() {
    let place = [node_ref(), node_ref()];
    let mut ui = at_scale(move || {
        div().flex().child(button_group([
            button("Word").node_ref(place[0]),
            button("Word").node_ref(place[1]),
        ]))
    });
    settle(&mut ui);
    pointer_at(&mut ui, 20.0, 36.0);
    pointer(&mut ui, 20.0, 36.0, true);
    settle(&mut ui);
    pointer(&mut ui, 20.0, 36.0, false);
    run(&mut ui, Instant::now(), 16);
    let before = place[0].rect().unwrap().width;
    pointer(&mut ui, 20.0, 36.0, true);
    // Ask it right away, before the press moves anything. What it takes stays what it was.
    let start = Instant::now();
    ui.animate(start);
    ui.update();
    let after = place[0].rect().unwrap().width;
    assert!((after - before).abs() <= 2, "{before} to {after}");
    let full = settle(&mut ui);
    let _ = full;
    let width = place[0].rect().unwrap().width;
    pointer(&mut ui, 20.0, 36.0, false);
    settle(&mut ui);
    pointer(&mut ui, 20.0, 36.0, true);
    settle(&mut ui);
    assert_eq!(
        place[0].rect().unwrap().width,
        width,
        "the same as the first time"
    );
}
