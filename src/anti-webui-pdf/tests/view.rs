//! `pdf_view` as far as the tree goes: real documents, drawn on real threads, without a GPU. See
//! `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, ScrollPhase, ScrollSource, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_pdf::{PdfError, PdfState, PdfViewer, PdfZoom, pdf_view, pdf_viewer};
use std::time::{Duration, Instant};

const ONE: Scale = Scale(120);

/// A document with one page for each color, each `size` points and filled with that color.
fn document(size: (u32, u32), colors: &[(f32, f32, f32)]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let count = colors.len();
    let mut object = |out: &mut Vec<u8>, body: String| {
        offsets.push(out.len());
        let id = offsets.len();
        out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", 3 + i * 2)).collect();
    object(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into());
    object(
        &mut out,
        format!(
            "<< /Type /Pages /Count {count} /Kids [{}] >>",
            kids.join(" ")
        ),
    );
    for (i, (r, g, b)) in colors.iter().enumerate() {
        object(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents {} 0 R >>",
                size.0,
                size.1,
                4 + i * 2
            ),
        );
        let content = format!("{r} {g} {b} rg 0 0 {} {} re f", size.0, size.1);
        object(
            &mut out,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        );
    }
    let start = out.len();
    let total = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {total}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {total} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n").as_bytes(),
    );
    out
}

fn mount<V: IntoView>(size: (i32, i32), scale: Scale, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(size.0, size.1),
        scale,
    });
    ui.update();
    ui
}

/// Lets everything that is going on finish.
fn settle(ui: &mut Ui) {
    ui.wait_for_images(Duration::from_secs(30));
    // What arrived can ask for more, like pictures of pages that came into view.
    for _ in 0..5 {
        ui.update();
        ui.wait_for_images(Duration::from_secs(30));
    }
}

/// The pictures in a frame, with `rect` where they show in the window.
fn pictures(ui: &mut Ui) -> Vec<ImageDraw> {
    fn walk(scene: &Scene, at: (i32, i32), out: &mut Vec<ImageDraw>) {
        for p in &scene.primitives {
            match p {
                Primitive::Image(d) => {
                    let mut d = d.clone();
                    d.rect = d.rect.translate(at.0, at.1);
                    out.push(d);
                }
                Primitive::Layer(l) => {
                    walk(&l.content, (at.0 + l.origin.x, at.1 + l.origin.y), out);
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&ui.paint(Instant::now()), (0, 0), &mut out);
    out
}

fn pixel(d: &ImageDraw, x: u32, y: u32) -> [u8; 4] {
    let mut found = [0; 4];
    let width = d.bitmap.width();
    d.bitmap.with_levels(|level, _, pixels| {
        if level == 0 {
            let i = ((y * width + x) * 4) as usize;
            found.copy_from_slice(&pixels[i..i + 4]);
        }
    });
    found
}

fn viewer_of(size: (i32, i32), colors: &[(f32, f32, f32)]) -> (Ui, PdfViewer) {
    let data = document((100, 200), colors);
    let viewer = std::rc::Rc::new(std::cell::Cell::new(None));
    let handed = viewer.clone();
    let ui = mount(size, ONE, move || {
        let v = pdf_viewer();
        handed.set(Some(v));
        pdf_view(data).viewer(v).w_full().h_full().gap(10.0)
    });
    (ui, viewer.get().unwrap())
}

const RED: (f32, f32, f32) = (1.0, 0.0, 0.0);
const GREEN: (f32, f32, f32) = (0.0, 1.0, 0.0);
const BLUE: (f32, f32, f32) = (0.0, 0.0, 1.0);

#[test]
fn a_page_fills_its_box_pixel_for_pixel() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED]);
    settle(&mut ui);
    assert_eq!(viewer.state(), PdfState::Ready);
    assert_eq!(viewer.page_count(), 1);
    let drawn = pictures(&mut ui);
    assert_eq!(drawn.len(), 1);
    // The width of the box less the gaps, which makes the page twice as big as a point each.
    assert_eq!((drawn[0].rect.width, drawn[0].rect.height), (400, 800));
    assert_eq!(drawn[0].rect.x, 10);
    assert_eq!(
        (drawn[0].bitmap.width(), drawn[0].bitmap.height()),
        (400, 800)
    );
    assert!(drawn[0].bitmap.is_opaque());
    assert_eq!(pixel(&drawn[0], 200, 400), [255, 0, 0, 255]);
}

#[test]
fn the_scale_of_the_screen_changes_the_pixels_and_not_the_zoom() {
    let data = document((72, 72), &[RED]);
    let mut ui = mount((600, 600), Scale(216), move || {
        let v = pdf_viewer();
        v.set_zoom(PdfZoom::Percent(1.0));
        pdf_view(data).viewer(v).w_full().h_full()
    });
    settle(&mut ui);
    let drawn = pictures(&mut ui);
    // 72 points are 96 logical pixels, and 1.8 times that here.
    assert_eq!(drawn[0].bitmap.width(), 173);
    assert_eq!(drawn[0].rect.width, 173);
}

#[test]
fn only_what_shows_and_comes_next_is_drawn() {
    let colors = vec![GREEN; 500];
    let (mut ui, viewer) = viewer_of((420, 900), &colors);
    settle(&mut ui);
    assert_eq!(viewer.page_count(), 500);
    let drawn = pictures(&mut ui);
    // Two pages of 800 pixels show, and the next one is built without having been asked to show.
    assert!(drawn.len() <= 3, "{} pictures", drawn.len());
    assert!(!drawn.is_empty());
    assert_eq!(viewer.page(), 0);
}

#[test]
fn going_to_a_page_scrolls_there_and_shows_it() {
    let colors: Vec<_> = (0..100).map(|i| if i == 50 { BLUE } else { RED }).collect();
    let (mut ui, viewer) = viewer_of((420, 600), &colors);
    settle(&mut ui);
    viewer.go_to_page(50);
    settle(&mut ui);
    assert_eq!(viewer.page(), 50);
    let drawn = pictures(&mut ui);
    let at_top = drawn.iter().find(|d| d.rect.y == 0).expect("at the top");
    assert_eq!(pixel(at_top, 10, 10), [0, 0, 255, 255]);
}

#[test]
fn a_new_zoom_shows_at_once_and_is_drawn_again() {
    let (mut ui, viewer) = viewer_of((420, 900), &[RED, GREEN]);
    settle(&mut ui);
    viewer.set_zoom(PdfZoom::Percent(1.0));
    ui.update();
    // In the frame after the zoom the box has its new size, with whatever picture there was.
    let stretched = pictures(&mut ui);
    assert_eq!(stretched[0].rect.width, 133);
    settle(&mut ui);
    let drawn = pictures(&mut ui);
    assert_eq!(drawn[0].rect.width, 133);
    assert_eq!(drawn[0].bitmap.width(), 133);
    assert_eq!(viewer.percent(), 1.0);
    assert!(matches!(viewer.zoom(), PdfZoom::Percent(_)));
}

#[test]
fn zooming_with_ctrl_keeps_what_is_under_the_pointer() {
    let colors = vec![RED; 20];
    let (mut ui, viewer) = viewer_of((420, 600), &colors);
    settle(&mut ui);
    let before = viewer.percent();
    let (x, y) = (210.0, 300.0);
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(x, y),
        delta_x: 0.0,
        delta_y: -200.0,
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        source: ScrollSource::Wheel,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let after = viewer.percent();
    assert!(after > before * 1.1, "{before} to {after}");
    // The middle of the first page is where the pointer was, so it should still be there.
    let drawn = pictures(&mut ui);
    let page = &drawn[0];
    let ratio = (y - page.rect.y as f32) / page.rect.height as f32;
    let expected = (300.0 - 10.0) / 800.0;
    assert!((ratio - expected).abs() < 0.01, "{ratio} and {expected}");
}

#[test]
fn a_box_getting_its_first_size_stays_at_the_top() {
    let data = document((100, 200), &vec![RED; 20]);
    let handed = std::rc::Rc::new(std::cell::Cell::new(None));
    let viewer = handed.clone();
    // As a window goes: the document is open before anyone knows how big the box is.
    let mut ui = Ui::new(move || {
        let v = pdf_viewer();
        viewer.set(Some(v));
        pdf_view(data).viewer(v).w_full().h_full().gap(10.0)
    });
    settle(&mut ui);
    let viewer = handed.get().unwrap();
    assert_eq!(viewer.page_count(), 20);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(420, 600),
        scale: ONE,
    });
    settle(&mut ui);
    assert_eq!(viewer.page(), 0);
    assert_eq!(pictures(&mut ui)[0].rect.y, 10);
}

#[test]
fn a_box_changing_size_keeps_what_is_at_its_top() {
    let (mut ui, viewer) = viewer_of((420, 600), &vec![RED; 20]);
    settle(&mut ui);
    viewer.go_to_page(5);
    settle(&mut ui);
    assert_eq!(viewer.page(), 5);
    // Fitting the width, the pages get bigger along with the box.
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(620, 600),
        scale: ONE,
    });
    settle(&mut ui);
    assert_eq!(viewer.page(), 5);
    let top = pictures(&mut ui)
        .into_iter()
        .find(|d| d.rect.width == 600 && d.rect.y <= 0 && d.rect.bottom() > 0)
        .expect("a page at the top");
    assert!(top.rect.y.abs() <= 2, "at {}", top.rect.y);
}

#[test]
fn what_is_not_a_pdf_says_so() {
    let (seen, handed) = (
        std::rc::Rc::new(std::cell::RefCell::new(None)),
        std::rc::Rc::new(std::cell::Cell::new(None)),
    );
    let (told, viewer) = (seen.clone(), handed.clone());
    let mut ui = mount((100, 100), ONE, move || {
        let v = pdf_viewer();
        viewer.set(Some(v));
        pdf_view(b"this isn't one".to_vec())
            .viewer(v)
            .on_error(move |e| *told.borrow_mut() = Some(e.clone()))
    });
    settle(&mut ui);
    assert_eq!(*seen.borrow(), Some(PdfError::Invalid));
    assert_eq!(
        handed.get().unwrap().state(),
        PdfState::Failed(PdfError::Invalid)
    );
}

#[test]
fn a_file_that_is_not_there_says_so() {
    let handed = std::rc::Rc::new(std::cell::Cell::new(None));
    let viewer = handed.clone();
    let mut ui = mount((100, 100), ONE, move || {
        let v = pdf_viewer();
        viewer.set(Some(v));
        pdf_view("/no/such/file.pdf").viewer(v)
    });
    settle(&mut ui);
    assert!(matches!(
        handed.get().unwrap().state(),
        PdfState::Failed(PdfError::Source(_))
    ));
}

#[test]
fn leaving_lets_the_threads_go() {
    let (ui, _viewer) = viewer_of((420, 600), &[RED, GREEN, BLUE]);
    drop(ui);
}
