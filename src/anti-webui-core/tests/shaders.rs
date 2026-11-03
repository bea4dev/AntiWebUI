//! How shaders take part in the reactive system, painting and input (see
//! `docs/design/14-shaders.md`).

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

struct Level(f32);

impl ShaderParams for Level {
    const FIELDS: &'static [anti_webui_core::shader::ParamField] =
        &[anti_webui_core::shader::ParamField {
            name: "level",
            ty: anti_webui_core::shader::ParamType::F32,
        }];

    fn write(&self, out: &mut anti_webui_core::shader::ParamWriter) {
        anti_webui_core::shader::ParamValue::write(&self.0, out);
    }
}

static LEVEL: Shader<Level> = Shader::new("level", "");

fn sized(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 400),
        scale: Scale::ONE,
    });
    ui.update();
}

fn shaders_in(scene: &Scene, out: &mut Vec<f32>) {
    for p in &scene.primitives {
        match p {
            Primitive::Shader(s) => {
                let b = s.paint.params();
                out.push(f32::from_ne_bytes(b[..4].try_into().unwrap()));
            }
            Primitive::Layer(l) => shaders_in(&l.content, out),
            _ => {}
        }
    }
}

fn layers_in(scene: &Scene) -> Vec<Arc<Scene>> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Layer(l) => Some(l.content.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn it_runs_again_only_when_what_it_reads_changes() {
    let level = signal(1.0f32);
    let runs = Rc::new(Cell::new(0));
    let counted = runs.clone();
    let mut ui = Ui::new(move || {
        let counted = counted.clone();
        shader(move || {
            counted.set(counted.get() + 1);
            LEVEL.params(Level(level.get()))
        })
        .w(10.0)
        .h(10.0)
    });
    sized(&mut ui);
    let now = Instant::now();
    let scene = ui.paint(now);
    let mut seen = Vec::new();
    shaders_in(&scene, &mut seen);
    assert_eq!(seen, [1.0]);
    assert_eq!(runs.get(), 1);
    assert!(!ui.needs_redraw());

    level.set(2.0);
    assert_eq!(runs.get(), 2);
    assert!(ui.needs_redraw());
    ui.update();
    let scene = ui.paint(now);
    let mut seen = Vec::new();
    shaders_in(&scene, &mut seen);
    assert_eq!(seen, [2.0]);
}

#[test]
fn the_same_parameters_ask_for_nothing() {
    let level = signal(1.0f32);
    let mut ui = Ui::new(move || {
        shader(move || LEVEL.params(Level(if level.get() > 0.0 { 1.0 } else { 0.0 })))
            .w(10.0)
            .h(10.0)
    });
    sized(&mut ui);
    ui.paint(Instant::now());
    assert!(!ui.needs_redraw());
    level.set(5.0);
    assert!(!ui.needs_redraw());
}

#[test]
fn only_the_layers_it_is_in_paint_again() {
    let level = signal(1.0f32);
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .child(
                div().h(100.0).overflow_y_scroll().child(
                    div().h(500.0).child(
                        shader(move || LEVEL.params(Level(level.get())))
                            .w(10.0)
                            .h(10.0),
                    ),
                ),
            )
            .child(
                div()
                    .h(100.0)
                    .overflow_y_scroll()
                    .child(div().h(500.0).child(text("not this one"))),
            )
    });
    sized(&mut ui);
    let now = Instant::now();
    let before = layers_in(&ui.paint(now));
    assert_eq!(before.len(), 2);
    level.set(2.0);
    ui.update();
    let after = layers_in(&ui.paint(now));
    assert!(!Arc::ptr_eq(&before[0], &after[0]));
    assert!(Arc::ptr_eq(&before[1], &after[1]));
    let mut seen = Vec::new();
    shaders_in(&after[0], &mut seen);
    assert_eq!(seen, [2.0]);
}

#[test]
fn a_layer_that_fades_or_moves_around_it_sees_the_change() {
    let level = signal(1.0f32);
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .child(
                div().opacity(0.5).bg(rgb(0xff0000)).child(
                    shader(move || LEVEL.params(Level(level.get())))
                        .w(10.0)
                        .h(10.0),
                ),
            )
            .child(
                div().translate_x(3.0).child(
                    shader(move || LEVEL.params(Level(level.get())))
                        .w(10.0)
                        .h(10.0),
                ),
            )
    });
    sized(&mut ui);
    let now = Instant::now();
    assert_eq!(layers_in(&ui.paint(now)).len(), 2);
    level.set(2.0);
    ui.update();
    let scene = ui.paint(now);
    let mut seen = Vec::new();
    shaders_in(&scene, &mut seen);
    assert_eq!(seen, [2.0, 2.0]);
}

#[test]
fn it_counts_as_something_that_overlaps_when_fading() {
    // A box with just a shader fades through its instance, and with a background it is a layer.
    let mut alone = Ui::new(|| {
        shader(|| LEVEL.params(Level(1.0)))
            .w(10.0)
            .h(10.0)
            .opacity(0.5)
    });
    sized(&mut alone);
    assert_eq!(layers_in(&alone.paint(Instant::now())).len(), 0);
    drop(alone);
    let mut together = Ui::new(|| {
        shader(|| LEVEL.params(Level(1.0)))
            .bg(rgb(0xff0000))
            .w(10.0)
            .h(10.0)
            .opacity(0.5)
    });
    sized(&mut together);
    assert_eq!(layers_in(&together.paint(Instant::now())).len(), 1);
}

fn press(ui: &mut Ui, x: f32, y: f32) {
    let position = PointF::new(x, y);
    ui.handle_event(WindowEvent::PointerMoved {
        position,
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::PointerButton {
        position,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

fn release(ui: &mut Ui, x: f32, y: f32) {
    let position = PointF::new(x, y);
    ui.handle_event(WindowEvent::PointerMoved {
        position,
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::PointerButton {
        position,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
        time: 10,
    });
    ui.update();
}

#[test]
fn being_pressed_is_told_also_when_released_elsewhere() {
    let log = Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = log.clone();
    let mut ui = Ui::new(move || {
        let seen = seen.clone();
        div().h(pct(100.0)).child(
            div()
                .w(100.0)
                .h(100.0)
                .on_active(move |a| seen.borrow_mut().push(a))
                .child(div().w(50.0).h(50.0)),
        )
    });
    sized(&mut ui);
    press(&mut ui, 25.0, 25.0);
    assert_eq!(*log.borrow(), [true]);
    release(&mut ui, 300.0, 300.0);
    assert_eq!(*log.borrow(), [true, false]);
    press(&mut ui, 300.0, 300.0);
    release(&mut ui, 300.0, 300.0);
    assert_eq!(*log.borrow(), [true, false]);
}

#[test]
fn listeners_of_a_kind_add_up() {
    let log = Rc::new(std::cell::RefCell::new(Vec::new()));
    let (a, b, c, d) = (log.clone(), log.clone(), log.clone(), log.clone());
    let mut ui = Ui::new(move || {
        let (a, b, c, d) = (a.clone(), b.clone(), c.clone(), d.clone());
        div()
            .w(100.0)
            .h(100.0)
            .on_click(move |_| a.borrow_mut().push("first"))
            .on_click(move |_| b.borrow_mut().push("second"))
            .on_hover(move |_| c.borrow_mut().push("hover 1"))
            .on_hover(move |_| d.borrow_mut().push("hover 2"))
    });
    sized(&mut ui);
    press(&mut ui, 25.0, 25.0);
    release(&mut ui, 25.0, 25.0);
    assert_eq!(*log.borrow(), ["hover 1", "hover 2", "first", "second"]);
}

#[test]
fn focus_shows_when_it_comes_from_the_keyboard() {
    use anti_webui_core::event::{Key, KeyEvent, NamedKey};
    let log = Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = log.clone();
    let mut ui = Ui::new(move || {
        let seen = seen.clone();
        div()
            .w(100.0)
            .h(100.0)
            .focusable()
            .on_focus_visible(move |v| seen.borrow_mut().push(v))
    });
    sized(&mut ui);
    // A click focuses it, but there is nothing to show.
    press(&mut ui, 25.0, 25.0);
    release(&mut ui, 25.0, 25.0);
    assert!(log.borrow().is_empty());
    // Tab goes around to it again.
    let tab = |ui: &mut Ui, shift: bool| {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers {
                shift,
                ..Default::default()
            },
            text: None,
            pressed: true,
            repeat: false,
        }));
        ui.update();
    };
    press(&mut ui, 300.0, 300.0);
    release(&mut ui, 300.0, 300.0);
    assert_eq!(*log.borrow(), [false]);
    tab(&mut ui, false);
    assert_eq!(*log.borrow(), [false, true]);
}
