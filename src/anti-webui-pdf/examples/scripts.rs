//! Runs the JavaScript of a document, sets some of its fields the way an application would, and
//! prints what each field has and shows: to see what the scripts make of real forms. Dialogs are
//! printed and answered with OK, and what the scripts log goes to stderr.
//!
//! ```sh
//! cargo run --release -p anti-webui-pdf --features js --example scripts -- form.pdf [name=value ...] [--list]
//! ```
//!
//! `--list` prints the names and kinds of all of the fields first, and `--dump=out.ppm` writes the
//! picture of the first page at the end.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_pdf::{PdfAnswer, PdfFieldKind, PdfFieldValue, PdfZoom, pdf_view, pdf_viewer};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) && record.target().starts_with("anti_webui_pdf") {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

fn settle(ui: &mut Ui) {
    for _ in 0..6 {
        ui.update();
        ui.wait_for_images(Duration::from_secs(60));
    }
}

/// Writes the picture of the page that shows first.
fn dump(ui: &mut Ui, path: &str) {
    fn walk(scene: &Scene, out: &mut Vec<ImageDraw>) {
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

fn main() {
    let _ = log::set_logger(&Logger);
    log::set_max_level(log::LevelFilter::Debug);
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a PDF to run");
    let args: Vec<String> = args.collect();
    let list = args.iter().any(|a| a == "--list");
    let dump_to = args
        .iter()
        .find_map(|a| a.strip_prefix("--dump=").map(str::to_owned));
    let sets: Vec<(String, String)> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .filter_map(|a| a.split_once('=').map(|(n, v)| (n.to_owned(), v.to_owned())))
        .collect();

    let viewer = Rc::new(RefCell::new(None));
    let kept = viewer.clone();
    let started = Instant::now();
    let mut ui = Ui::new(move || {
        let v = pdf_viewer();
        *kept.borrow_mut() = Some(v);
        v.set_zoom(PdfZoom::FitWidth);
        pdf_view(path.as_str()).viewer(v).w_full().h_full()
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1200, 900),
        scale: Scale(100),
    });
    settle(&mut ui);
    let viewer = viewer.borrow().expect("it was made");
    let answer_dialogs = |ui: &mut Ui| {
        for _ in 0..100 {
            settle(ui);
            match viewer.dialog() {
                Some(dialog) => {
                    println!("dialog: {} / {}", dialog.title, dialog.message);
                    viewer.answer_dialog(PdfAnswer::Ok);
                }
                None => break,
            }
        }
    };
    answer_dialogs(&mut ui);
    println!(
        "opened in {:.0} ms, scripts: {}, changed by them: {}",
        started.elapsed().as_secs_f64() * 1000.0,
        viewer.has_scripts(),
        viewer.is_modified()
    );

    let fields = viewer.fields();
    if list {
        for field in &fields {
            println!("{} {:?}", field.name, field.kind);
        }
    }
    for (name, value) in &sets {
        let Some(field) = fields.iter().find(|f| &f.name == name) else {
            eprintln!("there is no field {name}");
            continue;
        };
        let value = match field.kind {
            PdfFieldKind::Check | PdfFieldKind::Radio => {
                PdfFieldValue::State((value != "Off").then(|| value.clone()))
            }
            PdfFieldKind::Combo { .. } | PdfFieldKind::List { .. } => {
                PdfFieldValue::Choice(vec![value.clone()])
            }
            _ => PdfFieldValue::Text(value.clone()),
        };
        let at = Instant::now();
        viewer.set_field(name, value);
        answer_dialogs(&mut ui);
        println!(
            "set {name} in {:.1} ms",
            at.elapsed().as_secs_f64() * 1000.0
        );
    }
    for field in &fields {
        let value = viewer.field_value(&field.name);
        let text = viewer.field_text(&field.name).unwrap_or_default();
        let value = match value {
            Some(PdfFieldValue::Text(t)) => t,
            Some(PdfFieldValue::State(s)) => s.unwrap_or_else(|| "Off".into()),
            Some(PdfFieldValue::Choice(c)) => c.join(", "),
            None => String::new(),
        };
        if (value.is_empty() && text.is_empty()) || field.kind == PdfFieldKind::Push {
            continue;
        }
        if text == value {
            println!("{} = {value:?}", field.name);
        } else {
            println!("{} = {value:?} shows {text:?}", field.name);
        }
    }
    if let Some(path) = dump_to {
        settle(&mut ui);
        dump(&mut ui, &path);
    }
}
