//! What the tests of the components share: driving a UI without a window, and looking at what it
//! paints. It uses nothing but the public API.
#![allow(dead_code)]

use anti_webui::core::Ui;
use anti_webui::core::event::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, WindowEvent};
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::core::platform::{PlatformRequest, PopupParams};
use anti_webui::core::scene::{GlyphRun, Primitive, Quad, Scene, ShaderDraw};
use anti_webui::prelude::*;
use anti_webui_md3e::ColorScheme;
use std::time::{Duration, Instant};

pub const C: ColorScheme = ColorScheme::light();

pub fn start_ui(view: impl FnOnce() -> Div + 'static) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1000, 800),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

pub fn pointer_at(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

pub fn pointer(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    pointer_button(ui, x, y, pressed, PointerButton::Primary);
}

pub fn pointer_button(ui: &mut Ui, x: f32, y: f32, pressed: bool, button: PointerButton) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

pub fn click(ui: &mut Ui, x: f32, y: f32) {
    pointer_at(ui, x, y);
    pointer(ui, x, y, true);
    pointer(ui, x, y, false);
}

pub fn key(ui: &mut Ui, named: NamedKey) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(named),
            text: None,
            modifiers: Modifiers::default(),
            pressed,
            repeat: false,
        }));
    }
    ui.update();
}

/// Plays frames for this many milliseconds, and gives the last.
pub fn run(ui: &mut Ui, milliseconds: u64) -> Scene {
    let start = Instant::now();
    let mut scene = None;
    for t in (0..=milliseconds).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        scene = Some(ui.paint(now));
    }
    scene.expect("a frame")
}

/// Plays frames for this many milliseconds, looking at each.
pub fn watch(ui: &mut Ui, milliseconds: u64, mut f: impl FnMut(&Scene)) {
    let start = Instant::now();
    for t in (0..=milliseconds).step_by(8) {
        let now = start + Duration::from_millis(t);
        ui.animate(now);
        ui.update();
        f(&ui.paint(now));
    }
}

pub fn settle(ui: &mut Ui) -> Scene {
    run(ui, 1500)
}

/// Runs what is waiting for a time, as if it had come.
pub fn wait(ui: &mut Ui, milliseconds: u64) {
    ui.tick(Instant::now() + Duration::from_millis(milliseconds));
    ui.update();
}

pub fn boxes(scene: &Scene) -> Vec<Quad> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) => Some(q.clone()),
            _ => None,
        })
        .collect()
}

/// The boxes of a scene and of the layers in it, which is where what fades or moves is, with where
/// they are in the window.
pub fn all_boxes(scene: &Scene) -> Vec<Quad> {
    let mut all = Vec::new();
    for p in &scene.primitives {
        match p {
            Primitive::Quad(q) => all.push(q.clone()),
            Primitive::Layer(l) => all.extend(all_boxes(&l.content).into_iter().map(|mut q| {
                q.rect = q.rect.translate(l.origin.x, l.origin.y);
                q
            })),
            _ => {}
        }
    }
    all
}

/// The boxes of a color.
pub fn colored(scene: &Scene, color: Color) -> Vec<Quad> {
    boxes(scene)
        .into_iter()
        .filter(|q| q.background == color.snapped())
        .collect()
}

pub fn shaders(scene: &Scene) -> Vec<ShaderDraw> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Shader(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

pub fn glyphs(scene: &Scene) -> Vec<GlyphRun> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.clone()),
            _ => None,
        })
        .collect()
}

pub fn popups_shown(ui: &mut Ui) -> Vec<PopupParams> {
    ui.update();
    ui.take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::ShowPopup(p) => Some(p),
            _ => None,
        })
        .collect()
}

/// A screen is at rest: nothing asks for frames.
pub fn assert_at_rest(ui: &mut Ui) {
    settle(ui);
    settle(ui);
    assert!(!ui.animating(), "still asking for frames");
}
