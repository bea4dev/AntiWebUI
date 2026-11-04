//! What the tests of the widgets share: driving a UI without a window, and looking at what it has.
#![allow(dead_code)]

use anti_webui_core::Ui;
use anti_webui_core::event::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, PointI, Scale, SizeI};
use anti_webui_core::platform::{PlatformRequest, PopupParams};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Quad, Scene};
use anti_webui_core::tree::{NodeId, NodeKind, with_doc};
use std::time::{Duration, Instant};

pub fn start_at(view: impl FnOnce() -> Div + 'static, scale: Scale) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1000, 800),
        scale,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let _ = ui.paint(Instant::now());
    ui
}

pub fn start(view: impl FnOnce() -> Div + 'static) -> Ui {
    start_at(view, Scale::ONE)
}

pub fn pointer_at(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

pub fn press_with(ui: &mut Ui, x: f32, y: f32, pressed: bool, button: PointerButton, m: Modifiers) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button,
        pressed,
        modifiers: m,
        time: 0,
    });
    ui.update();
}

pub fn pointer(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    press_with(
        ui,
        x,
        y,
        pressed,
        PointerButton::Primary,
        Modifiers::default(),
    );
}

pub fn click(ui: &mut Ui, x: f32, y: f32) {
    pointer_at(ui, x, y);
    pointer(ui, x, y, true);
    pointer(ui, x, y, false);
}

pub fn click_with(ui: &mut Ui, x: f32, y: f32, m: Modifiers) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: m,
    });
    press_with(ui, x, y, true, PointerButton::Primary, m);
    press_with(ui, x, y, false, PointerButton::Primary, m);
}

pub fn key_with(ui: &mut Ui, named: NamedKey, m: Modifiers) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(named),
            text: None,
            modifiers: m,
            pressed,
            repeat: false,
        }));
    }
    ui.update();
}

pub fn key(ui: &mut Ui, named: NamedKey) {
    key_with(ui, named, Modifiers::default());
}

pub fn typed(ui: &mut Ui, s: &str) {
    for c in s.chars() {
        let t = c.to_string();
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Character(t.clone()),
            text: Some(t),
            modifiers: Modifiers::default(),
            pressed: true,
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

pub fn settle(ui: &mut Ui) -> Scene {
    run(ui, 600)
}

/// Runs what is waiting for a time, as if it had come.
pub fn wait(ui: &mut Ui, milliseconds: u64) {
    ui.tick(Instant::now() + Duration::from_millis(milliseconds));
    ui.update();
}

/// The boxes of a scene and of the layers in it, with where they are in the window.
pub fn boxes(scene: &Scene) -> Vec<Quad> {
    let mut all = Vec::new();
    for p in &scene.primitives {
        match p {
            Primitive::Quad(q) => all.push(q.clone()),
            Primitive::Layer(l) => all.extend(boxes(&l.content).into_iter().map(|mut q| {
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

/// The node with this text.
pub fn text_node(s: &str) -> Option<NodeId> {
    with_doc(|d| {
        d.nodes.iter().find_map(|(id, n)| match &n.kind {
            NodeKind::Text(t) if t.text == s => Some(id),
            _ => None,
        })
    })
}

pub fn has_text(s: &str) -> bool {
    text_node(s).is_some()
}

/// How many text nodes start with this.
pub fn count_text(prefix: &str) -> usize {
    with_doc(|d| {
        d.nodes
            .iter()
            .filter(|(_, n)| matches!(&n.kind, NodeKind::Text(t) if t.text.starts_with(prefix)))
            .count()
    })
}

/// The middle of the node with this text, in window coordinates.
pub fn middle_of(s: &str) -> (f32, f32) {
    let id = text_node(s).unwrap_or_else(|| panic!("no text {s}"));
    let r = with_doc(|d| d.window_rect(id)).expect("a place");
    (
        r.x as f32 + r.width as f32 / 2.0,
        r.y as f32 + r.height as f32 / 2.0,
    )
}

pub fn click_text(ui: &mut Ui, s: &str) {
    let (x, y) = middle_of(s);
    click(ui, x, y);
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

pub fn popups_closed(ui: &mut Ui) -> usize {
    ui.update();
    ui.take_requests()
        .iter()
        .filter(|r| matches!(r, PlatformRequest::ClosePopup(_)))
        .count()
}

/// Plays the platform: below the anchor, at the size it asked for.
pub fn place(ui: &mut Ui, p: &PopupParams) {
    ui.handle_event(WindowEvent::PopupConfigured {
        id: p.id,
        origin: PointI::new(
            p.anchor.x - p.geometry.x,
            p.anchor.bottom() + p.gap - p.geometry.y,
        ),
        size: p.wanted,
    });
    ui.update();
}

/// A screen is at rest: nothing asks for frames.
pub fn assert_at_rest(ui: &mut Ui) {
    settle(ui);
    settle(ui);
    assert!(!ui.animating(), "still asking for frames");
}
