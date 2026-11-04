//! Sheets, and what comes with them. See `docs/design/21-widgets.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const SHEET: Color = rgb(0x111111);
const SHEET_HOVER: Color = rgb(0x222222);
const SHEET_ACTIVE: Color = rgb(0x333333);
const SHEET_FOCUS: Color = rgb(0x444444);
const SHEET_RING: Color = rgb(0x555555);
const SHEET_OFF: Color = rgb(0x666666);
const OWN: Color = rgb(0xaa0000);
const OWN_HOVER: Color = rgb(0x00aa00);

fn sheet() -> Rc<Sheet> {
    Rc::new(
        Sheet::new(|s| s.bg(SHEET))
            .hover(|s| s.bg(SHEET_HOVER))
            .active(|s| s.bg(SHEET_ACTIVE))
            .focus(|s| s.bg(SHEET_FOCUS))
            .focus_visible(|s| s.bg(SHEET_RING))
            .disabled(|s| s.bg(SHEET_OFF)),
    )
}

fn start(view: impl FnOnce() -> Div + 'static) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let _ = ui.paint(Instant::now());
    ui
}

fn move_to(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

fn press(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

fn tab(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        text: None,
        modifiers: Modifiers::default(),
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

fn color_at(scene: &Scene, x: i32, y: i32) -> Option<Color> {
    scene.primitives.iter().rev().find_map(|p| match p {
        Primitive::Quad(q) if q.rect.contains(PointF::new(x as f32, y as f32)) => {
            Some(q.background)
        }
        _ => None,
    })
}

fn look(ui: &mut Ui) -> Option<Color> {
    ui.update();
    color_at(&ui.paint(Instant::now()), 10, 10)
}

fn a_box() -> Div {
    div().w(50.0).h(50.0)
}

#[test]
fn a_sheet_gives_each_state_its_look() {
    let shared = sheet();
    let mut ui = start(move || div().child(a_box().sheet(move || shared.clone())));
    assert_eq!(look(&mut ui), Some(SHEET.snapped()));
    move_to(&mut ui, 10.0, 10.0);
    assert_eq!(look(&mut ui), Some(SHEET_HOVER.snapped()));
    press(&mut ui, 10.0, 10.0, true);
    assert_eq!(look(&mut ui), Some(SHEET_ACTIVE.snapped()));
    press(&mut ui, 10.0, 10.0, false);
    assert_eq!(look(&mut ui), Some(SHEET_HOVER.snapped()));
    move_to(&mut ui, 300.0, 200.0);
    assert_eq!(look(&mut ui), Some(SHEET.snapped()));
}

#[test]
fn what_an_element_is_given_goes_above_its_sheet_in_every_state() {
    // As `style="background: red"` stays red under `button:hover` of a browser.
    let shared = sheet();
    let mut ui = start(move || div().child(a_box().bg(OWN).sheet(move || shared.clone())));
    assert_eq!(look(&mut ui), Some(OWN.snapped()));
    move_to(&mut ui, 10.0, 10.0);
    assert_eq!(look(&mut ui), Some(OWN.snapped()), "while hovered");
    press(&mut ui, 10.0, 10.0, true);
    assert_eq!(look(&mut ui), Some(OWN.snapped()), "while pressed");
}

#[test]
fn a_state_of_the_element_itself_goes_above_the_sheet_too() {
    let shared = sheet();
    let mut ui = start(move || {
        div().child(
            a_box()
                .bg(OWN)
                .hover(|s| s.bg(OWN_HOVER))
                .sheet(move || shared.clone()),
        )
    });
    move_to(&mut ui, 10.0, 10.0);
    assert_eq!(look(&mut ui), Some(OWN_HOVER.snapped()));
}

#[test]
fn a_sheet_can_change() {
    let other = signal(false);
    let (a, b) = (sheet(), Rc::new(Sheet::new(|s| s.bg(OWN))));
    let mut ui = start(move || {
        div().child(a_box().sheet(move || if other.get() { b.clone() } else { a.clone() }))
    });
    assert_eq!(look(&mut ui), Some(SHEET.snapped()));
    other.set(true);
    assert_eq!(look(&mut ui), Some(OWN.snapped()));
}

#[test]
fn the_focus_is_only_shown_when_it_came_from_the_keyboard() {
    let shared = sheet();
    let mut ui = start(move || div().child(a_box().focusable().sheet(move || shared.clone())));
    tab(&mut ui);
    assert_eq!(look(&mut ui), Some(SHEET_RING.snapped()));
    // Pressing it again elsewhere and coming back with the pointer doesn't show it.
    press(&mut ui, 300.0, 200.0, true);
    press(&mut ui, 300.0, 200.0, false);
    assert_eq!(look(&mut ui), Some(SHEET.snapped()));
    press(&mut ui, 10.0, 10.0, true);
    press(&mut ui, 10.0, 10.0, false);
    move_to(&mut ui, 300.0, 200.0);
    assert_eq!(look(&mut ui), Some(SHEET_FOCUS.snapped()));
}

#[test]
fn what_is_disabled_has_that_look_and_no_hover() {
    let off = signal(true);
    let shared = sheet();
    let mut ui = start(move || {
        div().child(
            a_box()
                .sheet(move || shared.clone())
                .disabled(move || off.get()),
        )
    });
    assert_eq!(look(&mut ui), Some(SHEET_OFF.snapped()));
    move_to(&mut ui, 10.0, 10.0);
    assert_eq!(look(&mut ui), Some(SHEET_OFF.snapped()));
    press(&mut ui, 10.0, 10.0, true);
    assert_eq!(look(&mut ui), Some(SHEET_OFF.snapped()));
    press(&mut ui, 10.0, 10.0, false);
    off.set(false);
    assert_eq!(look(&mut ui), Some(SHEET_HOVER.snapped()));
}

#[test]
fn what_is_disabled_does_not_react_without_a_look_of_its_own() {
    let plain = Rc::new(
        Sheet::new(|s| s.bg(SHEET))
            .hover(|s| s.bg(SHEET_HOVER))
            .active(|s| s.bg(SHEET_ACTIVE)),
    );
    let mut ui = start(move || div().child(a_box().sheet(move || plain.clone()).disabled(|| true)));
    move_to(&mut ui, 10.0, 10.0);
    assert_eq!(look(&mut ui), Some(SHEET.snapped()));
    press(&mut ui, 10.0, 10.0, true);
    assert_eq!(look(&mut ui), Some(SHEET.snapped()));
}

#[test]
fn what_is_in_something_disabled_is_disabled() {
    let off = signal(true);
    let clicks = Rc::new(Cell::new(0));
    let (shared, count) = (sheet(), clicks.clone());
    let mut ui = start(move || {
        div().disabled(move || off.get()).child(
            a_box()
                .focusable()
                .sheet(move || shared.clone())
                .on_click(move |_| count.set(count.get() + 1)),
        )
    });
    assert_eq!(look(&mut ui), Some(SHEET_OFF.snapped()));
    press(&mut ui, 10.0, 10.0, true);
    press(&mut ui, 10.0, 10.0, false);
    assert_eq!(clicks.get(), 0, "it can't be pressed");
    tab(&mut ui);
    assert_eq!(look(&mut ui), Some(SHEET_OFF.snapped()));
    off.set(false);
    assert_eq!(look(&mut ui), Some(SHEET_HOVER.snapped()));
    press(&mut ui, 10.0, 10.0, true);
    press(&mut ui, 10.0, 10.0, false);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn something_that_gets_disabled_loses_the_focus() {
    let off = signal(false);
    let focused = Rc::new(Cell::new(false));
    let seen = focused.clone();
    let mut ui = start(move || {
        div().child(
            a_box()
                .focusable()
                .on_focus_change(move |v| seen.set(v))
                .disabled(move || off.get()),
        )
    });
    tab(&mut ui);
    assert!(focused.get());
    off.set(true);
    ui.update();
    assert!(!focused.get());
}

#[test]
fn an_accent_goes_down_the_tree() {
    let accent = rgb(0xe8590c);
    let marks = Rc::new(Sheet::new(|s| s.bg(SHEET)).accent_background());
    let plain = Rc::new(Sheet::new(|s| s.bg(SHEET)));
    let off = signal(false);
    let mut ui = start(move || {
        div()
            .accent_color(accent)
            .child(
                a_box()
                    .sheet(move || marks.clone())
                    .disabled(move || off.get()),
            )
            .child(a_box().sheet(move || plain.clone()))
    });
    let scene = ui.paint(Instant::now());
    assert_eq!(color_at(&scene, 10, 10), Some(accent.snapped()));
    assert_eq!(
        color_at(&scene, 10, 60),
        Some(SHEET.snapped()),
        "only what asks"
    );
    off.set(true);
    assert_eq!(
        look(&mut ui),
        Some(SHEET.snapped()),
        "not while it is disabled"
    );
}

#[test]
fn an_accent_on_the_element_itself_counts_and_its_own_color_wins() {
    let accent = rgb(0xe8590c);
    let marks = Rc::new(Sheet::new(|s| s.bg(SHEET)).accent_background());
    let again = marks.clone();
    let mut ui = start(move || {
        div()
            .child(a_box().accent_color(accent).sheet(move || marks.clone()))
            .child(
                a_box()
                    .accent_color(accent)
                    .bg(OWN)
                    .sheet(move || again.clone()),
            )
    });
    let scene = ui.paint(Instant::now());
    assert_eq!(color_at(&scene, 10, 10), Some(accent.snapped()));
    assert_eq!(color_at(&scene, 10, 60), Some(OWN.snapped()));
}

#[test]
fn device_lengths_are_physical_pixels_at_any_scale() {
    let mut ui = Ui::new(|| {
        div().relative().size_full().child(
            div()
                .absolute()
                .top(Length::Device(1_000_003))
                .left(Length::Device(7))
                .w(Length::Device(11))
                .h(Length::Device(13))
                .bg(OWN),
        )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    use anti_webui_core::tree::with_doc;
    let found = with_doc(|d| {
        d.nodes
            .iter()
            .find(|(_, n)| n.computed.background == OWN)
            .and_then(|(id, _)| d.window_rect(id))
    })
    .expect("the box");
    assert_eq!(
        (found.x, found.y, found.width, found.height),
        (7, 1_000_003, 11, 13)
    );
}

#[test]
fn the_scale_of_the_window_can_be_followed() {
    let seen = Rc::new(Cell::new(Scale::ONE));
    let mut ui = {
        let seen = seen.clone();
        Ui::new(move || {
            effect(move || seen.set(window_scale()));
            div()
        })
    };
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    assert_eq!(seen.get(), Scale::from_f64(1.8));
}
