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
    let contents: Vec<String> = colors
        .iter()
        .map(|(r, g, b)| format!("{r} {g} {b} rg 0 0 {} {} re f", size.0, size.1))
        .collect();
    document_of(size, &contents)
}

/// A document with one page for each drawing, in the operators of a PDF.
fn document_of(size: (u32, u32), contents: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let count = contents.len();
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
    for (i, content) in contents.iter().enumerate() {
        object(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents {} 0 R >>",
                size.0,
                size.1,
                4 + i * 2
            ),
        );
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

/// A page of 1000 points, red up to `split` from the left and blue past it, which at 400% is too
/// big for a picture of all of it: 5333 pixels each way, where one can be at most 4000.
fn big_page(split: u32, pages: usize) -> Vec<u8> {
    let content = format!(
        "1 0 0 rg 0 0 {split} 1000 re f 0 0 1 rg {split} 0 {} 1000 re f",
        1000 - split
    );
    document_of((1000, 1000), &vec![content; pages])
}

fn big_viewer(data: Vec<u8>) -> (Ui, PdfViewer) {
    let handed = std::rc::Rc::new(std::cell::Cell::new(None));
    let viewer = handed.clone();
    let ui = mount((420, 300), ONE, move || {
        let v = pdf_viewer();
        v.set_zoom(PdfZoom::Percent(4.0));
        viewer.set(Some(v));
        pdf_view(data).viewer(v).w_full().h_full().gap(0.0)
    });
    (ui, handed.get().unwrap())
}

/// The picture that shows pixels one to one, if any.
fn sharp(ui: &mut Ui) -> Option<ImageDraw> {
    pictures(ui).into_iter().find(|d| {
        (d.rect.width as u32, d.rect.height as u32) == (d.bitmap.width(), d.bitmap.height())
    })
}

#[test]
fn a_page_too_big_for_a_picture_is_sharp_where_it_shows() {
    let (mut ui, viewer) = big_viewer(big_page(100, 1));
    settle(&mut ui);
    let drawn = pictures(&mut ui);
    assert_eq!(drawn.len(), 2);
    // The one below all of the page is stretched, and takes no more than it may.
    assert_eq!((drawn[0].rect.width, drawn[0].rect.height), (5333, 5333));
    assert_eq!(
        (drawn[0].bitmap.width(), drawn[0].bitmap.height()),
        (4000, 4000)
    );
    // The one above it has the view and half of it around it, or as far as the page goes.
    let part = &drawn[1];
    assert_eq!((part.rect.x, part.rect.y), (0, 0));
    assert_eq!((part.bitmap.width(), part.bitmap.height()), (630, 450));
    assert_eq!((part.rect.width, part.rect.height), (630, 450));
    // 100 points in from the left are 533 pixels in, whatever part of it this is.
    assert_eq!(pixel(part, 520, 100), [255, 0, 0, 255]);
    assert_eq!(pixel(part, 546, 100), [0, 0, 255, 255]);
    assert_eq!(viewer.percent(), 4.0);
}

#[test]
fn scrolling_past_the_margin_gets_another_part() {
    let (mut ui, _viewer) = big_viewer(big_page(500, 1));
    settle(&mut ui);
    let before = sharp(&mut ui).expect("a sharp one");
    // Far enough to the right that it shows something else.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(100.0, 100.0),
        delta_x: 2600.0,
        delta_y: 0.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Touchpad,
        phase: ScrollPhase::Moved,
        time: 0,
    });
    settle(&mut ui);
    let after = sharp(&mut ui).expect("a sharp one");
    assert_ne!(before.bitmap.id(), after.bitmap.id());
    // It covers what shows, and sits where it is on the page: the color changes 2667 pixels in, a
    // little left of the view that starts 2600 in.
    let (x, y) = (after.rect.x, after.rect.y);
    assert!(x <= 0 && y <= 0 && after.rect.right() >= 420 && after.rect.bottom() >= 300);
    let split = 67 - x;
    assert_eq!(pixel(&after, split as u32 - 2, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&after, split as u32 + 2, 10), [0, 0, 255, 255]);
}

#[test]
fn a_part_goes_when_all_of_the_page_fits_in_a_picture_again() {
    let (mut ui, viewer) = big_viewer(big_page(100, 1));
    settle(&mut ui);
    assert_eq!(pictures(&mut ui).len(), 2);
    viewer.set_zoom(PdfZoom::Percent(1.0));
    // At once: what shows must not jump around while the new picture is being drawn.
    ui.update();
    assert_eq!(pictures(&mut ui).len(), 1);
    settle(&mut ui);
    let drawn = pictures(&mut ui);
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].bitmap.width(), 1333);
}

#[test]
fn a_part_stays_on_its_place_while_the_zoom_changes() {
    let (mut ui, viewer) = big_viewer(big_page(100, 1));
    settle(&mut ui);
    viewer.set_zoom(PdfZoom::Percent(8.0));
    ui.update();
    let drawn = pictures(&mut ui);
    // Twice as big as it was, until there is a new one.
    assert_eq!(drawn[0].rect.width, 10667);
    assert_eq!((drawn[1].rect.width, drawn[1].bitmap.width()), (1260, 630));
    settle(&mut ui);
    let part = sharp(&mut ui).expect("a sharp one");
    assert_eq!(part.rect.width as u32, part.bitmap.width());
}

#[test]
fn a_part_goes_when_nothing_of_it_shows_any_more() {
    let (mut ui, viewer) = big_viewer(big_page(100, 3));
    settle(&mut ui);
    let old = sharp(&mut ui).expect("a sharp one");
    viewer.go_to_page(2);
    // It is on another page now, and this one must not hold memory for a page that isn't there.
    settle(&mut ui);
    let weak = std::sync::Arc::downgrade(&old.bitmap);
    drop(old);
    ui.update();
    let _ = pictures(&mut ui);
    assert!(weak.upgrade().is_none());
    assert!(sharp(&mut ui).is_some());
}

#[test]
fn pictures_stay_within_the_budget_except_for_what_shows() {
    let data = document((100, 200), &vec![GREEN; 40]);
    let handed = std::rc::Rc::new(std::cell::Cell::new(None));
    let viewer = handed.clone();
    let mut ui = mount((420, 900), ONE, move || {
        let v = pdf_viewer();
        viewer.set(Some(v));
        // Two pages show, of 400 by 800 pixels each, and this leaves room for one more.
        pdf_view(data)
            .viewer(v)
            .w_full()
            .h_full()
            .gap(10.0)
            .cache_bytes(3 * 400 * 800 * 4 + 100)
    });
    let viewer = handed.get().unwrap();
    settle(&mut ui);
    let held = |ui: &mut Ui| {
        pictures(ui)
            .iter()
            .map(|d| std::sync::Arc::downgrade(&d.bitmap))
            .collect::<Vec<_>>()
    };
    let first = held(&mut ui);
    assert!(!first.is_empty());
    // Going through all of it, whatever is left behind goes.
    for page in (0..40).step_by(4) {
        viewer.go_to_page(page);
        settle(&mut ui);
    }
    ui.update();
    let _ = pictures(&mut ui);
    assert!(first.iter().all(|b| b.upgrade().is_none()));
}

#[test]
fn a_budget_too_small_for_anything_still_shows_what_shows_and_comes_to_rest() {
    let data = document((100, 200), &vec![GREEN; 20]);
    let mut ui = mount((420, 900), ONE, move || {
        pdf_view(data).w_full().h_full().gap(10.0).cache_bytes(1)
    });
    let start = Instant::now();
    settle(&mut ui);
    // If what is drawn had to go and be drawn again, it would still be busy after this long.
    assert!(start.elapsed() < Duration::from_secs(25));
    assert!(!pictures(&mut ui).is_empty());
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

/// One red page of 100 by 200 points, encrypted with the password `hello` by Ghostscript.
const ENCRYPTED: &[u8] = include_bytes!("encrypted.pdf");

fn encrypted_viewer() -> (
    Ui,
    PdfViewer,
    std::rc::Rc<std::cell::RefCell<Vec<PdfError>>>,
) {
    let (handed, errors) = (
        std::rc::Rc::new(std::cell::Cell::new(None)),
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
    );
    let (viewer, told) = (handed.clone(), errors.clone());
    let ui = mount((420, 900), ONE, move || {
        let v = pdf_viewer();
        viewer.set(Some(v));
        pdf_view(ENCRYPTED.to_vec())
            .viewer(v)
            .w_full()
            .h_full()
            .gap(10.0)
            .on_error(move |e| told.borrow_mut().push(e.clone()))
    });
    (ui, handed.get().unwrap(), errors)
}

#[test]
fn a_document_with_a_password_opens_with_it() {
    let (mut ui, viewer, errors) = encrypted_viewer();
    settle(&mut ui);
    assert_eq!(viewer.state(), PdfState::Failed(PdfError::NeedsPassword));
    assert_eq!(*errors.borrow(), [PdfError::NeedsPassword]);
    assert!(pictures(&mut ui).is_empty());

    viewer.unlock("hello");
    settle(&mut ui);
    assert_eq!(viewer.state(), PdfState::Ready);
    assert_eq!(viewer.page_count(), 1);
    let drawn = pictures(&mut ui);
    assert_eq!(pixel(&drawn[0], 100, 100), [255, 0, 0, 255]);
}

#[test]
fn the_wrong_password_says_so_and_another_can_be_tried() {
    let (mut ui, viewer, errors) = encrypted_viewer();
    settle(&mut ui);
    viewer.unlock("nope");
    settle(&mut ui);
    assert_eq!(viewer.state(), PdfState::Failed(PdfError::WrongPassword));
    assert_eq!(errors.borrow().last(), Some(&PdfError::WrongPassword));
    viewer.unlock("hello");
    settle(&mut ui);
    assert_eq!(viewer.state(), PdfState::Ready);
}
