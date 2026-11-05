//! How long it takes to go from changing a field to having the page drawn again.
//!
//! ```sh
//! cargo run --release -p anti-webui-pdf --example commit_time -- form.pdf [text]
//! ```

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_pdf::{PdfFieldKind, PdfFieldValue, PdfViewer, PdfZoom, pdf_view, pdf_viewer};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn main() {
    let path = std::env::args().nth(1).expect("a document");
    let text = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Hello".to_owned());
    let viewer: Rc<RefCell<Option<PdfViewer>>> = Rc::new(RefCell::new(None));
    let kept = viewer.clone();
    let mut ui = Ui::new(move || {
        let v = pdf_viewer();
        *kept.borrow_mut() = Some(v);
        v.set_zoom(PdfZoom::FitWidth);
        pdf_view(PathBuf::from(path)).viewer(v).w_full().h_full()
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1300, 900),
        scale: Scale(216),
    });
    let wait = |ui: &mut Ui| {
        for _ in 0..4 {
            ui.update();
            ui.wait_for_images(Duration::from_secs(60));
        }
    };
    wait(&mut ui);
    let viewer = viewer.borrow().expect("it was made");
    let field = viewer
        .fields()
        .into_iter()
        .find(|f| f.kind == PdfFieldKind::Text && !f.read_only)
        .expect("a text field");
    let mut times = Vec::new();
    for i in 0..20 {
        let started = Instant::now();
        viewer.set_field(&field.name, PdfFieldValue::Text(format!("{text} {i}")));
        wait(&mut ui);
        times.push(started.elapsed());
    }
    times.sort();
    println!(
        "first field, {} fields: median {:?}, worst {:?}",
        viewer.fields().len(),
        times[times.len() / 2],
        times[times.len() - 1]
    );
}
