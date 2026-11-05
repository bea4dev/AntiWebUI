//! Moving the items of a list, as it looks here. See `docs/design/23-drag-and-drop.md`.

use anti_webui::core::Ui;
use anti_webui::core::event::*;
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{sortable_list, sortable_row};
use std::time::{Duration, Instant};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn moved(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.update();
}

fn button(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

fn settle(ui: &mut Ui) {
    let start = Instant::now();
    for i in 1..300 {
        ui.animate(start + Duration::from_millis(i * 8));
        ui.update();
    }
}

#[test]
fn what_is_held_is_lifted_and_comes_back_down_when_let_go() {
    let items = signal(vec![0u32, 1, 2]);
    let first = node_ref();
    let mut ui = mount(|| {
        sortable_list(
            items,
            |v| *v,
            move |v, _| {
                let el = div().w(100.0).h(40.0);
                if v == 0 { el.node_ref(first) } else { el }
            },
        )
    });
    let shadows = |first: NodeRef| {
        let id = first.get().unwrap();
        // The row is around what was given.
        anti_webui::core::tree::with_doc(|d| {
            let parent = d.get(id).unwrap().parent.unwrap();
            d.get(parent).unwrap().computed.shadows.len()
        })
    };
    assert_eq!(shadows(first), 0);
    moved(&mut ui, 50.0, 20.0);
    button(&mut ui, 50.0, 20.0, true);
    moved(&mut ui, 50.0, 80.0);
    assert!(shadows(first) > 0);
    button(&mut ui, 50.0, 80.0, false);
    assert_eq!(items.get_untracked(), [1, 0, 2]);
    assert_eq!(shadows(first), 0);
    // It bounces home in the end.
    settle(&mut ui);
    assert_eq!(first.rect().unwrap().y, 40);
}

#[test]
fn it_goes_across_too() {
    let items = signal(vec![0u32, 1, 2]);
    let mut ui = mount(|| sortable_row(items, |v| *v, |_, _| div().w(50.0).h(40.0)));
    moved(&mut ui, 20.0, 20.0);
    button(&mut ui, 20.0, 20.0, true);
    moved(&mut ui, 150.0, 20.0);
    button(&mut ui, 150.0, 20.0, false);
    assert_eq!(items.get_untracked(), [1, 2, 0]);
}
