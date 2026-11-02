//! Measures the cost of dragging the showcase slider: one pointer move followed by a frame.
//!
//! `cargo run --release -p anti-webui --example bench_slider`

#[path = "showcase.rs"]
mod showcase;

use anti_webui::core::Ui;
use anti_webui::core::event::*;
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::core::tree::{NodeKind, with_doc};
use std::time::{Duration, Instant};

fn main() {
    let mut ui = Ui::new(showcase::root);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1620, 1616),
        scale: Scale(216),
    });
    ui.update();
    let _ = ui.paint(Instant::now());

    // The slider is the parent of the node whose text is "40%"'s sibling; find it by its label.
    let label = with_doc(|d| {
        d.nodes.iter().find_map(|(id, n)| match &n.kind {
            NodeKind::Text(t) if t.text == "40%" => Some(id),
            _ => None,
        })
    })
    .expect("slider label");
    let slider = with_doc(|d| {
        let row = d.get(label).unwrap().parent.unwrap();
        let row = if d.get(row).unwrap().children.len() < 2 {
            d.get(row).unwrap().parent.unwrap()
        } else {
            row
        };
        d.get(row).unwrap().children[0]
    });
    let rect = with_doc(|d| d.get(slider).unwrap().abs);
    let y = rect.y as f32 + rect.height as f32 / 2.0;
    let m = Modifiers::default();
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(rect.x as f32 + 5.0, y),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: m,
        time: 0,
    });

    let steps = 200;
    let (mut t_event, mut t_update, mut t_paint) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    for i in 0..steps {
        let x = rect.x as f32 + (i % 100) as f32 / 100.0 * rect.width as f32;
        let t0 = Instant::now();
        ui.handle_event(WindowEvent::PointerMoved {
            position: PointF::new(x, y),
            modifiers: m,
        });
        let t1 = Instant::now();
        ui.update();
        let t2 = Instant::now();
        let scene = ui.paint(Instant::now());
        let t3 = Instant::now();
        std::hint::black_box(scene);
        t_event += t1 - t0;
        t_update += t2 - t1;
        t_paint += t3 - t2;
    }
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / steps as f64;
    let nodes = with_doc(|d| d.nodes.len());
    println!(
        "{nodes} nodes; per drag step: event {:.3} ms, update {:.3} ms, paint {:.3} ms",
        per(t_event),
        per(t_update),
        per(t_paint)
    );

    // A 1000 Hz mouse delivers ~16 moves per 60 Hz frame.
    let t0 = Instant::now();
    for i in 0..16 {
        let x = rect.x as f32 + i as f32 / 16.0 * rect.width as f32;
        ui.handle_event(WindowEvent::PointerMoved {
            position: PointF::new(x, y),
            modifiers: m,
        });
    }
    ui.update();
    std::hint::black_box(ui.paint(Instant::now()));
    println!(
        "16 moves + one frame: {:.3} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
}
