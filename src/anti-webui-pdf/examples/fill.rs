//! Fills in all of the fields of a document and marks it up, the way a person would through the
//! viewer, and writes the result: to compare with what other programs make of it.
//!
//! ```sh
//! cargo run --release -p anti-webui-pdf --example fill -- in.pdf out.pdf [text] [dump.ppm]
//! ```
//!
//! Besides the fields and a highlight of all of the text, it draws a line, puts down text and a note
//! near the middle of the first page.

use anti_webui_core::Ui;
use anti_webui_core::event::{Key, KeyEvent, Modifiers, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_pdf::{
    PdfFieldKind, PdfFieldValue, PdfTool, PdfViewer, PdfZoom, pdf_view, pdf_viewer,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn settle(ui: &mut Ui) {
    for _ in 0..6 {
        ui.update();
        ui.wait_for_images(Duration::from_secs(60));
    }
}

fn dump(ui: &mut Ui, path: &str) {
    fn walk(scene: &Scene, out: &mut Vec<anti_webui_core::scene::ImageDraw>) {
        for p in &scene.primitives {
            match p {
                Primitive::Image(d) => out.push(d.clone()),
                Primitive::Layer(l) => walk(&l.content, out),
                _ => {}
            }
        }
    }
    let mut pictures = Vec::new();
    walk(&ui.paint(Instant::now()), &mut pictures);
    let Some(picture) = pictures.first() else {
        return;
    };
    let (width, height) = (picture.bitmap.width(), picture.bitmap.height());
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    picture.bitmap.with_levels(|level, _, pixels| {
        if level == 0 {
            for p in pixels.chunks_exact(4) {
                out.extend_from_slice(&p[..3]);
            }
        }
    });
    std::fs::write(path, out).expect("the picture can be written");
}

fn pointer(ui: &mut Ui, position: PointF, pressed: Option<bool>) {
    ui.handle_event(WindowEvent::PointerMoved {
        position,
        modifiers: Modifiers::default(),
    });
    if let Some(pressed) = pressed {
        ui.handle_event(WindowEvent::PointerButton {
            button: PointerButton::Primary,
            pressed,
            position,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    settle(ui);
}

fn click(ui: &mut Ui, position: PointF) {
    pointer(ui, position, Some(true));
    pointer(ui, position, Some(false));
}

fn typed(ui: &mut Ui, text: &str) {
    for c in text.chars() {
        let s = c.to_string();
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Character(s.clone()),
            text: Some(s),
            modifiers: Modifiers::default(),
            pressed: true,
            repeat: false,
        }));
    }
    settle(ui);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: fill <in.pdf> <out.pdf> [text] [dump.ppm]");
        return;
    };
    let text = args
        .next()
        .unwrap_or_else(|| "Filled by AntiWebUI".to_owned());
    let picture = args.next();
    let viewer: Rc<RefCell<Option<PdfViewer>>> = Rc::new(RefCell::new(None));
    let kept = viewer.clone();
    let mut ui = Ui::new(move || {
        let v = pdf_viewer();
        *kept.borrow_mut() = Some(v);
        v.set_zoom(PdfZoom::FitWidth);
        pdf_view(PathBuf::from(input)).viewer(v).w_full().h_full()
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(900, 1200),
        scale: Scale(120),
    });
    settle(&mut ui);
    let viewer = viewer.borrow().expect("it was made");
    println!(
        "editing: {:?}, signed: {}",
        viewer.editing(),
        viewer.is_signed()
    );
    let fields = viewer.fields();
    println!("{} fields", fields.len());
    let started = Instant::now();
    let mut filled = 0;
    for field in &fields {
        if field.read_only {
            continue;
        }
        let value = match field.kind {
            PdfFieldKind::Text => PdfFieldValue::Text(text.clone()),
            PdfFieldKind::Check | PdfFieldKind::Radio => {
                PdfFieldValue::State(field.states.first().cloned())
            }
            PdfFieldKind::Combo { .. } | PdfFieldKind::List { .. } => {
                let Some(first) = field.options.first() else {
                    continue;
                };
                PdfFieldValue::Choice(vec![first.0.clone()])
            }
            PdfFieldKind::Push | PdfFieldKind::Signature => continue,
        };
        filled += viewer.set_field(&field.name, value) as usize;
    }
    settle(&mut ui);
    println!("filled {filled} in {:?}", started.elapsed());
    if let Some(e) = viewer.edit_error() {
        println!("couldn't: {e}");
    }
    viewer.select_all();
    ui.update();
    viewer.highlight_selection();
    settle(&mut ui);
    println!(
        "{} annotations on the first page, {:?} in all",
        viewer.annotations(0).len(),
        started.elapsed()
    );
    viewer.set_tool(PdfTool::Ink);
    viewer.set_ink_width(3.0);
    settle(&mut ui);
    pointer(&mut ui, PointF::new(200.0, 200.0), Some(true));
    for i in 0..20 {
        let x = 200.0 + i as f32 * 20.0;
        let y = 200.0 + (i as f32 * 0.8).sin() * 40.0;
        pointer(&mut ui, PointF::new(x, y), None);
    }
    pointer(&mut ui, PointF::new(600.0, 200.0), Some(false));
    viewer.set_tool(PdfTool::FreeText);
    settle(&mut ui);
    click(&mut ui, PointF::new(200.0, 320.0));
    typed(&mut ui, &format!("{text}\nsecond line"));
    viewer.set_tool(PdfTool::Note);
    settle(&mut ui);
    click(&mut ui, PointF::new(700.0, 320.0));
    typed(&mut ui, "a note");
    viewer.set_tool(PdfTool::Select);
    click(&mut ui, PointF::new(5.0, 5.0));
    println!("annotations of the first page: {:?}", viewer.annotations(0));
    if let Some(path) = picture {
        dump(&mut ui, &path);
    }
    let saved = Instant::now();
    viewer.save(PathBuf::from(output));
    settle(&mut ui);
    println!("saved in {:?}: {:?}", saved.elapsed(), viewer.save_result());
}
