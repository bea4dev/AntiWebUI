//! `pdf_view` as far as the tree goes: real documents, drawn on real threads, without a GPU. See
//! `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{ClipboardKind as Clipboard, Key, KeyEvent, PointerButton};
use anti_webui_core::event::{Modifiers, PinchPhase, ScrollPhase, ScrollSource, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, PlatformRequest};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_pdf::{
    PdfEditing, PdfError, PdfOutlineItem, PdfState, PdfTarget, PdfViewer, PdfZoom, pdf_thumbnails,
    pdf_view, pdf_viewer,
};
use std::cell::RefCell;
use std::rc::Rc;
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
        modifiers: Modifiers::PRIMARY,
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
    let data = document((100, 200), &[RED; 20]);
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
    let (mut ui, viewer) = viewer_of((420, 600), &[RED; 20]);
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
fn an_encrypted_document_shows_and_says_it_cannot_be_edited() {
    let (mut ui, viewer, _) = encrypted_viewer();
    settle(&mut ui);
    viewer.unlock("hello");
    settle(&mut ui);
    assert_eq!(viewer.state(), PdfState::Ready);
    assert_eq!(viewer.editing(), PdfEditing::Encrypted);
    assert_eq!(viewer.edit_error(), None);
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

/// A document with pages of text in Helvetica, one point of each page's content being what it
/// says, with more for the catalog, each page and the rest of the objects.
struct TextDocument {
    pages: Vec<String>,
    /// For each page, what else its dictionary has.
    page_extras: Vec<String>,
    catalog_extra: String,
    /// More objects, numbered from 100.
    objects: Vec<String>,
}

impl TextDocument {
    fn new(pages: &[&str]) -> Self {
        Self {
            pages: pages.iter().map(|p| p.to_string()).collect(),
            page_extras: vec![String::new(); pages.len()],
            catalog_extra: String::new(),
            objects: Vec::new(),
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets: Vec<(usize, usize)> = Vec::new();
        let mut object = |out: &mut Vec<u8>, id: usize, body: &str| {
            offsets.push((id, out.len()));
            out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        };
        let kids: Vec<String> = (0..self.pages.len())
            .map(|i| format!("{} 0 R", 10 + i * 2))
            .collect();
        object(
            &mut out,
            1,
            &format!("<< /Type /Catalog /Pages 2 0 R {} >>", self.catalog_extra),
        );
        object(
            &mut out,
            2,
            &format!(
                "<< /Type /Pages /Count {} /Kids [{}] >>",
                self.pages.len(),
                kids.join(" ")
            ),
        );
        object(
            &mut out,
            3,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        );
        for (i, content) in self.pages.iter().enumerate() {
            object(
                &mut out,
                10 + i * 2,
                &format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents {} 0 R \
                     /Resources << /Font << /F1 3 0 R >> >> {} >>",
                    11 + i * 2,
                    self.page_extras[i]
                ),
            );
            object(
                &mut out,
                11 + i * 2,
                &format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                ),
            );
        }
        for (i, body) in self.objects.iter().enumerate() {
            object(&mut out, 100 + i, body);
        }
        let start = out.len();
        let size = offsets.iter().map(|o| o.0).max().unwrap_or(0) + 1;
        out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
        for (id, offset) in &offsets {
            out.extend_from_slice(format!("{id} 1\n{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }
}

/// A line of text, in 20 point Helvetica, `y` points from the bottom of the page.
fn line(x: u32, y: u32, text: &str) -> String {
    format!("BT /F1 20 Tf {x} {y} Td ({text}) Tj ET\n")
}

/// A viewer of 400x300 pages at 100% at a scale of 1, where a point is 4/3 of a pixel, in a box of
/// that size. It is kept from scrolling sideways with room to spare.
fn read(doc: &TextDocument) -> (Ui, PdfViewer) {
    read_with(doc, |view| view)
}

fn read_with(
    doc: &TextDocument,
    f: impl FnOnce(anti_webui_pdf::PdfView) -> anti_webui_pdf::PdfView,
) -> (Ui, PdfViewer) {
    let viewer = Rc::new(RefCell::new(None));
    let bytes = doc.build();
    let kept = viewer.clone();
    let mut ui = mount((560, 420), ONE, move || {
        let v = pdf_viewer();
        *kept.borrow_mut() = Some(v);
        v.set_zoom(PdfZoom::Percent(1.0));
        f(pdf_view(bytes).viewer(v).gap(0.0)).w_full().h_full()
    });
    settle(&mut ui);
    let v = viewer.borrow().expect("it was made");
    v.focus();
    ui.update();
    (ui, v)
}

/// Where a point of a page is in the box, for the first page.
fn at(x: f32, y: f32) -> PointF {
    PointF::new(x * 4.0 / 3.0, (300.0 - y) * 4.0 / 3.0)
}

fn move_to(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    // What the page says has to come for this to mean anything.
    settle(ui);
}

fn press(ui: &mut Ui, p: PointF, time: u32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    settle(ui);
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed: true,
        position: p,
        modifiers: Modifiers::default(),
        time,
    });
    settle(ui);
}

fn release(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed: false,
        position: p,
        modifiers: Modifiers::default(),
        time: 0,
    });
    settle(ui);
}

fn drag(ui: &mut Ui, from: PointF, to: PointF) {
    press(ui, from, 0);
    move_to(ui, to);
    release(ui, to);
}

fn control(ui: &mut Ui, key: &str) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Character(key.into()),
        text: None,
        modifiers: Modifiers::PRIMARY,
        pressed: true,
        repeat: false,
    }));
    settle(ui);
}

/// What was last put on a clipboard.
fn clipboard(ui: &mut Ui, kind: Clipboard) -> Option<String> {
    ui.take_requests().into_iter().rev().find_map(|r| match r {
        PlatformRequest::SetClipboard { kind: k, text } if k == kind => Some(text),
        _ => None,
    })
}

fn copied(ui: &mut Ui) -> Option<String> {
    control(ui, "c");
    clipboard(ui, Clipboard::Clipboard)
}

/// A letter of Helvetica at 20 points takes a little over 10 points on average, so these are in the
/// middle of the first and last letters.
const FOX: &str = "quick fox";

#[test]
fn dragging_selects_text_and_ctrl_c_copies_it() {
    let doc = TextDocument::new(&[&(line(20, 200, FOX) + &line(20, 170, "lazy dog"))]);
    let (mut ui, viewer) = read(&doc);
    assert!(!viewer.has_selection());
    drag(&mut ui, at(19.0, 205.0), at(300.0, 175.0));
    assert!(viewer.has_selection());
    // Letting go also puts it in the primary selection.
    assert_eq!(
        clipboard(&mut ui, Clipboard::Primary).as_deref(),
        Some("quick fox\nlazy dog")
    );
    assert_eq!(copied(&mut ui).as_deref(), Some("quick fox\nlazy dog"));
}

#[test]
fn the_selection_shows_on_top_of_the_page() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    let boxes = |ui: &mut Ui| {
        let mut count = 0;
        fn walk(scene: &Scene, count: &mut usize) {
            for p in &scene.primitives {
                match p {
                    Primitive::Quad(q) if q.background.a > 0.0 && q.background.a < 1.0 => {
                        *count += 1
                    }
                    Primitive::Layer(l) => walk(&l.content, count),
                    _ => {}
                }
            }
        }
        walk(&ui.paint(Instant::now()), &mut count);
        count
    };
    assert_eq!(boxes(&mut ui), 0);
    drag(&mut ui, at(19.0, 205.0), at(300.0, 205.0));
    assert!(viewer.has_selection());
    assert_eq!(boxes(&mut ui), 1);
    // A click on the page takes it away again.
    press(&mut ui, at(60.0, 205.0), 5000);
    release(&mut ui, at(60.0, 205.0));
    assert!(!viewer.has_selection());
    assert_eq!(boxes(&mut ui), 0);
}

#[test]
fn a_double_click_takes_a_word_and_a_triple_click_a_line() {
    let doc = TextDocument::new(&[&(line(20, 200, FOX) + &line(20, 170, "lazy dog"))]);
    let (mut ui, _viewer) = read(&doc);
    let p = at(30.0, 205.0);
    press(&mut ui, p, 1000);
    release(&mut ui, p);
    press(&mut ui, p, 1100);
    release(&mut ui, p);
    assert_eq!(copied(&mut ui).as_deref(), Some("quick"));
    press(&mut ui, p, 1200);
    release(&mut ui, p);
    assert_eq!(copied(&mut ui).as_deref(), Some("quick fox"));
}

/// A viewer with text of the application above it, as a toolbar has. It is 40 pixels high.
fn read_under_a_toolbar(doc: &TextDocument) -> (Ui, PdfViewer) {
    let viewer = Rc::new(RefCell::new(None));
    let bytes = doc.build();
    let kept = viewer.clone();
    let mut ui = mount((560, 460), ONE, move || {
        let v = pdf_viewer();
        *kept.borrow_mut() = Some(v);
        v.set_zoom(PdfZoom::Percent(1.0));
        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .child(div().h(40.0).flex_none().child(text("toolbar")))
            .child(pdf_view(bytes).viewer(v).gap(0.0).flex_1().min_h(0.0))
    });
    settle(&mut ui);
    let v = viewer.borrow().expect("it was made");
    (ui, v)
}

/// Whether the text of the application shows as selected.
fn application_text_is_selected(ui: &mut Ui) -> bool {
    control(ui, "c");
    ui.take_requests().iter().any(
        |r| matches!(r, PlatformRequest::SetClipboard { text, .. } if text.contains("toolbar")),
    )
}

#[test]
fn selecting_in_a_document_leaves_the_text_around_it_alone() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read_under_a_toolbar(&doc);
    let below = |p: PointF| PointF::new(p.x, p.y + 40.0);
    drag(&mut ui, below(at(19.0, 205.0)), below(at(300.0, 205.0)));
    assert!(viewer.has_selection());
    assert_eq!(copied(&mut ui).as_deref(), Some(FOX));
}

#[test]
fn ctrl_a_in_a_document_selects_only_the_document() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read_under_a_toolbar(&doc);
    let below = |p: PointF| PointF::new(p.x, p.y + 40.0);
    press(&mut ui, below(at(30.0, 205.0)), 0);
    release(&mut ui, below(at(30.0, 205.0)));
    control(&mut ui, "a");
    assert!(viewer.has_selection());
    assert_eq!(copied(&mut ui).as_deref(), Some(FOX));
}

#[test]
fn pressing_in_a_document_ends_the_selection_of_the_text_around_it() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read_under_a_toolbar(&doc);
    drag(&mut ui, PointF::new(1.0, 10.0), PointF::new(300.0, 10.0));
    assert!(application_text_is_selected(&mut ui));
    let below = |p: PointF| PointF::new(p.x, p.y + 40.0);
    press(&mut ui, below(at(300.0, 40.0)), 5000);
    release(&mut ui, below(at(300.0, 40.0)));
    assert!(!viewer.has_selection());
    assert!(!application_text_is_selected(&mut ui));
}

#[test]
fn ctrl_c_with_nothing_selected_in_a_document_copies_what_is_selected_around_it() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read_under_a_toolbar(&doc);
    viewer.focus();
    ui.update();
    drag(&mut ui, PointF::new(1.0, 10.0), PointF::new(300.0, 10.0));
    viewer.focus();
    ui.update();
    assert!(application_text_is_selected(&mut ui));
}

#[test]
fn shift_goes_on_from_where_selecting_started() {
    let doc = TextDocument::new(&[&(line(20, 200, FOX) + &line(20, 170, "lazy dog"))]);
    let (mut ui, _viewer) = read(&doc);
    drag(&mut ui, at(19.0, 205.0), at(70.0, 205.0));
    let selected = clipboard(&mut ui, Clipboard::Primary).expect("something was selected");
    assert!(
        selected.starts_with("qu") && !selected.contains("fox"),
        "{selected}"
    );
    let p = at(300.0, 175.0);
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed: true,
        position: p,
        modifiers: Modifiers {
            shift: true,
            ..Default::default()
        },
        time: 5000,
    });
    settle(&mut ui);
    release(&mut ui, p);
    assert_eq!(
        clipboard(&mut ui, Clipboard::Primary).as_deref(),
        Some("quick fox\nlazy dog")
    );
}

#[test]
fn selecting_past_the_edge_of_the_box_scrolls_and_goes_on() {
    let pages: Vec<String> = (0..6).map(|i| line(20, 200, &format!("page{i}"))).collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (mut ui, viewer) = read(&TextDocument::new(&pages));
    press(&mut ui, at(19.0, 205.0), 0);
    // Far below the box, where the pointer stays while it scrolls by itself.
    move_to(&mut ui, PointF::new(100.0, 2000.0));
    let start = Instant::now();
    for i in 0..40 {
        ui.animate(start + Duration::from_millis(i * 8));
        settle(&mut ui);
    }
    assert!(viewer.page() >= 3, "{}", viewer.page());
    release(&mut ui, PointF::new(100.0, 2000.0));
    let selected = clipboard(&mut ui, Clipboard::Primary).expect("something was selected");
    assert!(selected.starts_with("page0\npage1\npage2"), "{selected}");
}

#[test]
fn pressing_far_from_any_text_selects_nothing() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    drag(&mut ui, at(300.0, 40.0), at(350.0, 20.0));
    assert!(!viewer.has_selection());
}

#[test]
fn text_that_is_filled_and_stroked_is_there_once() {
    let content = "BT /F1 20 Tf 2 Tr 20 200 Td (twice) Tj ET\n";
    let (mut ui, viewer) = read(&TextDocument::new(&[content]));
    drag(&mut ui, at(19.0, 205.0), at(300.0, 205.0));
    assert!(viewer.has_selection());
    assert_eq!(copied(&mut ui).as_deref(), Some("twice"));
}

#[test]
fn ctrl_a_copies_every_page_even_if_they_have_not_been_read() {
    let pages: Vec<String> = (0..40)
        .map(|i| line(20, 200, &format!("page{i}")))
        .collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (mut ui, viewer) = read(&TextDocument::new(&pages));
    control(&mut ui, "a");
    assert!(viewer.has_selection());
    let all = copied(&mut ui).expect("all of it was read");
    let wanted: Vec<String> = (0..40).map(|i| format!("page{i}")).collect();
    assert_eq!(all, wanted.join("\n"));
}

#[test]
fn searching_finds_all_of_it_and_goes_around() {
    let pages: Vec<String> = (0..50)
        .map(|i| {
            if i % 10 == 3 {
                line(20, 200, "the needle") + &line(20, 170, "and a Needle")
            } else {
                line(20, 200, "hay")
            }
        })
        .collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (mut ui, viewer) = read(&TextDocument::new(&pages));
    assert!(viewer.search_done());
    viewer.search("needle");
    settle(&mut ui);
    assert!(viewer.search_done());
    assert_eq!(viewer.search_count(), 10);
    // It went to the first one, which is on another page, and shows it in the middle of the box.
    assert_eq!(viewer.search_index(), Some(0));
    settle(&mut ui);
    assert_eq!(viewer.page(), 3);
    viewer.find_next();
    settle(&mut ui);
    assert_eq!(viewer.search_index(), Some(1));
    viewer.find_next();
    settle(&mut ui);
    assert_eq!(viewer.search_index(), Some(2));
    assert_eq!(viewer.page(), 13);
    viewer.find_previous();
    viewer.find_previous();
    viewer.find_previous();
    settle(&mut ui);
    assert_eq!(viewer.search_index(), Some(9));
    assert_eq!(viewer.page(), 43);
    viewer.search("");
    settle(&mut ui);
    assert_eq!(viewer.search_count(), 0);
    assert_eq!(viewer.search_index(), None);
}

#[test]
fn going_to_the_only_one_found_shows_it_again() {
    let mut pages = vec![line(20, 200, "needle")];
    pages.extend((0..5).map(|_| line(20, 200, "hay")));
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (mut ui, viewer) = read(&TextDocument::new(&pages));
    viewer.search("needle");
    settle(&mut ui);
    viewer.go_to_page(4);
    settle(&mut ui);
    assert_eq!(viewer.page(), 4);
    viewer.find_next();
    settle(&mut ui);
    assert_eq!(viewer.page(), 0);
}

#[test]
fn text_on_a_turned_page_is_selected_where_it_shows() {
    let mut doc = TextDocument::new(&[&line(20, 200, FOX)]);
    doc.page_extras[0] = "/Rotate 90".into();
    let (mut ui, viewer) = read(&doc);
    viewer.select_all();
    viewer.copy();
    settle(&mut ui);
    assert_eq!(
        clipboard(&mut ui, Clipboard::Clipboard).as_deref(),
        Some(FOX)
    );
    // It goes down the page now, 200 points in from the left and starting 20 from the top. A page
    // this way is 300 points wide and 400 tall, in the middle of the box.
    let left = (560.0 - 400.0) / 2.0;
    let point = |x: f32, y: f32| PointF::new(left + x * 4.0 / 3.0, y * 4.0 / 3.0);
    viewer.clear_selection();
    settle(&mut ui);
    drag(&mut ui, point(205.0, 19.0), point(205.0, 200.0));
    assert_eq!(copied(&mut ui).as_deref(), Some(FOX));
}

#[test]
fn searching_starts_from_the_page_that_shows() {
    let pages: Vec<String> = (0..10).map(|_| line(20, 200, "same")).collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (mut ui, viewer) = read(&TextDocument::new(&pages));
    viewer.go_to_page(6);
    settle(&mut ui);
    assert_eq!(viewer.page(), 6);
    viewer.search("same");
    settle(&mut ui);
    assert_eq!(viewer.search_count(), 10);
    assert_eq!(viewer.search_index(), Some(6));
    assert_eq!(viewer.page(), 6);
}

#[test]
fn searching_finds_what_a_line_breaks_up() {
    let content = line(20, 200, "some exam-") + &line(20, 170, "ple text") + &line(20, 140, "two");
    let (mut ui, viewer) = read(&TextDocument::new(&[&content]));
    for (query, count) in [
        ("example", 1),
        ("EXAMPLE TEXT", 1),
        ("texttwo", 1),
        ("text two", 1),
        ("ＳＯＭＥ", 1),
        ("none", 0),
    ] {
        viewer.search(query);
        settle(&mut ui);
        assert_eq!(viewer.search_count(), count, "{query}");
    }
}

#[test]
fn links_go_to_their_pages_and_say_where_else_they_go() {
    let mut doc = TextDocument::new(&[
        &line(20, 200, "contents"),
        &line(20, 200, "second"),
        &line(20, 200, "third"),
    ]);
    // A rectangle for the link to the page after it, one by name and one out of the document.
    doc.page_extras[0] = "/Annots [\
        << /Type /Annot /Subtype /Link /Rect [0 250 100 300] /Dest [12 0 R /XYZ 0 300 0] >> \
        << /Type /Annot /Subtype /Link /Rect [100 250 200 300] /Dest (third) >> \
        << /Type /Annot /Subtype /Link /Rect [200 250 300 300] \
           /A << /S /URI /URI (https://example.com/) >> >> ]"
        .into();
    doc.catalog_extra = "/Names << /Dests << /Names [(third) [14 0 R /Fit]] >> >>".into();
    let urls = Rc::new(RefCell::new(Vec::<String>::new()));
    let told = urls.clone();
    let (mut ui, viewer) = read_with(&doc, move |view| {
        view.on_link(move |url| told.borrow_mut().push(url.to_owned()))
    });

    // Not followed when it is only there. A move says what is there, once the page has been read,
    // and the one after it shows on the pointer.
    for _ in 0..3 {
        move_to(&mut ui, at(50.0, 280.0));
    }
    assert_eq!(viewer.page(), 0);
    let cursors = ui.take_requests();
    assert!(
        cursors.contains(&PlatformRequest::SetCursor(CursorIcon::Pointer)),
        "{cursors:?}"
    );

    let p = at(250.0, 280.0);
    press(&mut ui, p, 0);
    release(&mut ui, p);
    assert_eq!(*urls.borrow(), ["https://example.com/"]);
    assert_eq!(viewer.page(), 0);

    let p = at(150.0, 280.0);
    press(&mut ui, p, 5000);
    release(&mut ui, p);
    settle(&mut ui);
    assert_eq!(viewer.page(), 2);

    viewer.go_to_page(0);
    settle(&mut ui);
    let p = at(50.0, 280.0);
    press(&mut ui, p, 10000);
    release(&mut ui, p);
    settle(&mut ui);
    assert_eq!(viewer.page(), 1);
}

#[test]
fn the_outline_says_what_goes_where() {
    let mut doc = TextDocument::new(&[&line(20, 200, "a"), &line(20, 200, "b")]);
    doc.catalog_extra = "/Outlines 100 0 R".into();
    doc.objects = vec![
        "<< /Type /Outlines /First 101 0 R /Last 102 0 R /Count 2 >>".into(),
        "<< /Title (One) /Parent 100 0 R /Next 102 0 R /First 103 0 R /Dest [10 0 R /Fit] >>"
            .into(),
        // In UTF-16, going to a place on the page.
        "<< /Title <FEFF65E5672C> /Parent 100 0 R /A << /S /GoTo /D [12 0 R /XYZ 0 150 0] >> >>"
            .into(),
        "<< /Title (Inside) /Parent 101 0 R >>".into(),
    ];
    let (mut ui, viewer) = read(&doc);
    settle(&mut ui);
    let outline = viewer.outline();
    assert_eq!(
        *outline,
        vec![
            PdfOutlineItem {
                title: "One".into(),
                target: Some(PdfTarget { page: 0, top: None }),
                children: vec![PdfOutlineItem {
                    title: "Inside".into(),
                    target: None,
                    children: Vec::new(),
                }],
            },
            PdfOutlineItem {
                title: "日本".into(),
                target: Some(PdfTarget {
                    page: 1,
                    top: Some(150.0),
                }),
                children: Vec::new(),
            },
        ]
    );
}

#[test]
fn an_outline_that_goes_in_circles_ends() {
    let mut doc = TextDocument::new(&[&line(20, 200, "a")]);
    doc.catalog_extra = "/Outlines 100 0 R".into();
    doc.objects = vec![
        "<< /First 101 0 R >>".into(),
        "<< /Title (Again) /Next 101 0 R /First 101 0 R >>".into(),
    ];
    let (mut ui, viewer) = read(&doc);
    settle(&mut ui);
    assert_eq!(viewer.outline().len(), 1);
}

#[test]
fn pressing_on_the_scrollbar_does_not_select() {
    let pages: Vec<String> = (0..5).map(|_| line(20, 200, FOX)).collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (mut ui, viewer) = read(&TextDocument::new(&pages));
    // On the right edge of the box, within reach of the text that is nearest.
    let p = PointF::new(555.0, 120.0);
    press(&mut ui, p, 0);
    move_to(&mut ui, PointF::new(555.0, 200.0));
    release(&mut ui, PointF::new(555.0, 200.0));
    assert!(!viewer.has_selection());
}

/// A page that is red in its top half and green in its bottom half.
fn two_colors() -> Vec<u8> {
    let content = "1 0 0 rg 0 100 100 100 re f 0 1 0 rg 0 0 100 100 re f".to_owned();
    document_of((100, 200), &[content])
}

fn upright(ui: &mut Ui) -> ImageDraw {
    settle(ui);
    let drawn = pictures(ui);
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    drawn.into_iter().next().unwrap()
}

#[test]
fn turning_changes_the_box_and_draws_it_turned() {
    let data = two_colors();
    let viewer = Rc::new(std::cell::Cell::new(None));
    let handed = viewer.clone();
    let mut ui = mount((420, 420), ONE, move || {
        let v = pdf_viewer();
        handed.set(Some(v));
        v.set_zoom(PdfZoom::Percent(0.75));
        pdf_view(data).viewer(v).w_full().h_full().gap(0.0)
    });
    let viewer = viewer.get().unwrap();
    let d = upright(&mut ui);
    assert_eq!((d.rect.width, d.rect.height), (100, 200));
    assert_eq!(pixel(&d, 50, 20), [255, 0, 0, 255]);
    assert_eq!(pixel(&d, 50, 180), [0, 255, 0, 255]);

    // A quarter turn clockwise takes the top to the right.
    viewer.rotate_clockwise();
    let d = upright(&mut ui);
    assert_eq!(viewer.rotation(), 90);
    assert_eq!((d.rect.width, d.rect.height), (200, 100));
    assert_eq!(pixel(&d, 180, 50), [255, 0, 0, 255]);
    assert_eq!(pixel(&d, 20, 50), [0, 255, 0, 255]);

    viewer.rotate_clockwise();
    let d = upright(&mut ui);
    assert_eq!(viewer.rotation(), 180);
    assert_eq!((d.rect.width, d.rect.height), (100, 200));
    assert_eq!(pixel(&d, 50, 180), [255, 0, 0, 255]);

    viewer.rotate_counterclockwise();
    viewer.rotate_counterclockwise();
    viewer.rotate_counterclockwise();
    let d = upright(&mut ui);
    assert_eq!(viewer.rotation(), 270);
    assert_eq!(pixel(&d, 20, 50), [255, 0, 0, 255]);
    viewer.set_rotation(0);
    let d = upright(&mut ui);
    assert_eq!(pixel(&d, 50, 20), [255, 0, 0, 255]);
}

#[test]
fn a_page_that_fits_is_not_left_at_its_old_turn() {
    // It is square, so that its size doesn't say that it has to be drawn again.
    let content = "1 0 0 rg 0 100 200 100 re f 0 1 0 rg 0 0 200 100 re f".to_owned();
    let data = document_of((200, 200), &[content]);
    let viewer = Rc::new(std::cell::Cell::new(None));
    let handed = viewer.clone();
    let mut ui = mount((420, 420), ONE, move || {
        let v = pdf_viewer();
        handed.set(Some(v));
        v.set_zoom(PdfZoom::Percent(0.75));
        pdf_view(data).viewer(v).w_full().h_full().gap(0.0)
    });
    let viewer = viewer.get().unwrap();
    assert_eq!(pixel(&upright(&mut ui), 100, 20), [255, 0, 0, 255]);
    viewer.rotate_clockwise();
    assert_eq!(pixel(&upright(&mut ui), 20, 100), [0, 255, 0, 255]);
}

#[test]
fn fitting_goes_by_the_turned_page() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED]);
    settle(&mut ui);
    let upright = viewer.percent();
    viewer.rotate_clockwise();
    settle(&mut ui);
    // It is twice as wide as it is tall now, in a box as wide as it was.
    let ratio = viewer.percent() / upright;
    assert!((ratio - 0.5).abs() < 0.01, "{ratio}");
    let d = pictures(&mut ui).remove(0);
    assert_eq!(d.rect.width, 400);
}

#[test]
fn control_with_brackets_turns_the_pages() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    control(&mut ui, "]");
    assert_eq!(viewer.rotation(), 90);
    control(&mut ui, "[");
    control(&mut ui, "[");
    assert_eq!(viewer.rotation(), 270);
}

/// Where a point of the first page is in the box when turned a quarter turn clockwise, at the same
/// zoom as [`at`]. It takes 400 of the 560 pixels across then, in the middle.
fn at_turned(x: f32, y: f32) -> PointF {
    let (shown_x, shown_y) = (y, x);
    PointF::new(80.0 + shown_x * 4.0 / 3.0, shown_y * 4.0 / 3.0)
}

#[test]
fn text_is_selected_where_it_shows_when_turned() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    viewer.rotate_clockwise();
    settle(&mut ui);
    // Turning kept what was in the middle there.
    viewer.go_to_page(0);
    settle(&mut ui);
    drag(&mut ui, at_turned(19.0, 205.0), at_turned(300.0, 205.0));
    assert!(viewer.has_selection());
    assert_eq!(copied(&mut ui).as_deref(), Some(FOX));
}

#[test]
fn what_is_selected_stays_selected_and_moves_when_turning() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    drag(&mut ui, at(19.0, 205.0), at(300.0, 205.0));
    let selected = |ui: &mut Ui| {
        fn walk(scene: &Scene, at: (i32, i32), out: &mut Vec<(i32, i32, i32, i32)>) {
            for p in &scene.primitives {
                match p {
                    Primitive::Quad(q) if q.background == Color::from_rgba(0x0054b466) => {
                        out.push((
                            q.rect.x + at.0,
                            q.rect.y + at.1,
                            q.rect.width,
                            q.rect.height,
                        ))
                    }
                    Primitive::Layer(l) => {
                        walk(&l.content, (at.0 + l.origin.x, at.1 + l.origin.y), out)
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&ui.paint(Instant::now()), (0, 0), &mut out);
        out
    };
    let before = selected(&mut ui);
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(before[0].2 > before[0].3, "it goes across: {before:?}");
    viewer.rotate_clockwise();
    settle(&mut ui);
    assert!(viewer.has_selection());
    let after = selected(&mut ui);
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].3 > after[0].2, "it goes down: {after:?}");
    assert_eq!((after[0].2, after[0].3), (before[0].3, before[0].2));
}

#[test]
fn turning_keeps_what_shows_on_the_same_page() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED, GREEN, RED, GREEN, RED]);
    settle(&mut ui);
    viewer.go_to_page(3);
    settle(&mut ui);
    assert_eq!(viewer.page(), 3);
    viewer.rotate_clockwise();
    settle(&mut ui);
    assert_eq!(viewer.page(), 3);
    viewer.rotate_counterclockwise();
    settle(&mut ui);
    assert_eq!(viewer.page(), 3);
}

#[test]
fn a_place_in_a_turned_document_is_where_it_shows() {
    let doc = TextDocument::new(&[
        &line(20, 200, FOX),
        &line(20, 200, FOX),
        &line(20, 200, FOX),
    ]);
    let (mut ui, viewer) = read(&doc);
    viewer.set_rotation(180);
    settle(&mut ui);
    // 280 points down from the top is 20 from the top when it is upside down.
    viewer.go_to(PdfTarget {
        page: 1,
        top: Some(280.0),
    });
    settle(&mut ui);
    let drawn = pictures(&mut ui);
    let top_of_page = drawn
        .iter()
        .map(|d| d.rect.y)
        .find(|y| (-400..=0).contains(y))
        .expect("it shows");
    assert!(
        (top_of_page + (20.0f32 * 4.0 / 3.0) as i32).abs() <= 1,
        "{drawn:?}"
    );
}

fn pinch(ui: &mut Ui, p: PointF, scale: f32, phase: PinchPhase) {
    ui.handle_event(WindowEvent::Pinch {
        position: p,
        scale,
        modifiers: Modifiers::default(),
        phase,
    });
    settle(ui);
}

#[test]
fn pinching_zooms_and_keeps_what_is_under_the_pointer() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED, GREEN]);
    viewer.set_zoom(PdfZoom::Percent(1.0));
    settle(&mut ui);
    let p = PointF::new(210.0, 150.0);
    let d = pictures(&mut ui).remove(0);
    let (fx, fy) = (
        (p.x - d.rect.x as f32) / d.rect.width as f32,
        (p.y - d.rect.y as f32) / d.rect.height as f32,
    );
    pinch(&mut ui, p, 1.0, PinchPhase::Began);
    assert_eq!(viewer.percent(), 1.0);
    pinch(&mut ui, p, 1.2, PinchPhase::Moved);
    pinch(&mut ui, p, 1.5, PinchPhase::Moved);
    pinch(&mut ui, p, 1.0, PinchPhase::Ended);
    assert!(
        (viewer.percent() - 1.8).abs() < 0.001,
        "{}",
        viewer.percent()
    );
    let d = pictures(&mut ui).remove(0);
    let (gx, gy) = (
        (p.x - d.rect.x as f32) / d.rect.width as f32,
        (p.y - d.rect.y as f32) / d.rect.height as f32,
    );
    assert!(
        (fx - gx).abs() < 0.01 && (fy - gy).abs() < 0.01,
        "{fx},{fy} {gx},{gy}"
    );
    pinch(&mut ui, p, 1.0, PinchPhase::Began);
    pinch(&mut ui, p, 0.5, PinchPhase::Moved);
    assert!(
        (viewer.percent() - 0.9).abs() < 0.001,
        "{}",
        viewer.percent()
    );
}

/// A viewer with thumbnails in a column to the left of it, as tall as `height`.
fn with_thumbnails(pages: usize, height: i32) -> (Ui, PdfViewer, Signal<bool>) {
    let contents = vec![String::from("0 0 1 rg 0 0 100 100 re f"); pages];
    let data = document_of((100, 100), &contents);
    let handed = Rc::new(RefCell::new(None));
    let kept = handed.clone();
    let mut ui = mount((600, height), ONE, move || {
        let v = pdf_viewer();
        let open = signal(true);
        *kept.borrow_mut() = Some((v, open));
        div()
            .flex()
            .w_full()
            .h_full()
            .child(show(
                move || open.get(),
                move || {
                    pdf_thumbnails(v)
                        .thumbnail_width(100.0)
                        .w(150.0)
                        .h_full()
                        .flex_none()
                },
            ))
            .child(pdf_view(data).viewer(v).flex_1().h_full())
    });
    settle(&mut ui);
    let (v, open) = handed.borrow().expect("it was made");
    (ui, v, open)
}

/// The thumbnails that show, by being 100 pixels wide.
fn thumbnails(ui: &mut Ui) -> Vec<ImageDraw> {
    pictures(ui)
        .into_iter()
        .filter(|d| d.bitmap.width() == 100)
        .collect()
}

#[test]
fn thumbnails_are_drawn_at_their_own_size_for_what_shows() {
    let (mut ui, _viewer, _) = with_thumbnails(50, 400);
    let shown = thumbnails(&mut ui);
    // Each takes 100 pixels, 22 for the number and 10 between them, from 10 down.
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert_eq!((shown[0].rect.width, shown[0].rect.height), (100, 100));
    assert_eq!(pixel(&shown[0], 50, 50), [0, 0, 255, 255]);
    assert_eq!(shown[1].rect.y - shown[0].rect.y, 132);
}

#[test]
fn clicking_a_thumbnail_goes_to_its_page() {
    let (mut ui, viewer, _) = with_thumbnails(20, 400);
    let d = thumbnails(&mut ui)[2].clone();
    let p = PointF::new(d.rect.x as f32 + 50.0, d.rect.y as f32 + 50.0);
    press(&mut ui, p, 0);
    release(&mut ui, p);
    assert_eq!(viewer.page(), 2);
}

/// Where the thumbnail of the page being read is.
fn outlined(ui: &mut Ui) -> Option<i32> {
    fn walk(scene: &Scene, at: i32, found: &mut Option<i32>) {
        for p in &scene.primitives {
            match p {
                Primitive::Quad(q) if q.border_widths[0] > 0 => *found = Some(q.rect.y + at),
                Primitive::Layer(l) => walk(&l.content, at + l.origin.y, found),
                _ => {}
            }
        }
    }
    let mut found = None;
    walk(&ui.paint(Instant::now()), 0, &mut found);
    found
}

#[test]
fn thumbnails_keep_up_with_the_page_being_read() {
    let (mut ui, viewer, _) = with_thumbnails(100, 400);
    viewer.go_to_page(60);
    settle(&mut ui);
    settle(&mut ui);
    // The row of that page shows: it is the only one with an outline.
    let shown = thumbnails(&mut ui);
    assert!(!shown.is_empty());
    let scene_y = shown[0].rect.y;
    assert!(outlined(&mut ui).is_some_and(|y| (0..300).contains(&y)));
    // Scrolling it by hand stays.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(50.0, 200.0),
        delta_x: 0.0,
        delta_y: 2000.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    settle(&mut ui);
    let moved = thumbnails(&mut ui);
    assert_ne!(
        moved[0].rect.y, scene_y,
        "it stays where it was scrolled to"
    );
    assert_eq!(viewer.page(), 60);
}

#[test]
fn thumbnails_are_let_go_of_when_they_are_gone() {
    let (mut ui, _, open) = with_thumbnails(20, 400);
    assert!(!thumbnails(&mut ui).is_empty());
    open.set(false);
    settle(&mut ui);
    assert!(thumbnails(&mut ui).is_empty());
    open.set(true);
    settle(&mut ui);
    assert_eq!(thumbnails(&mut ui).len(), 3);
}

#[test]
fn thumbnails_turn_with_the_pages() {
    let (mut ui, viewer, _) = with_thumbnails(4, 400);
    viewer.rotate_clockwise();
    settle(&mut ui);
    settle(&mut ui);
    assert_eq!(thumbnails(&mut ui).len(), 3);
    assert_eq!(viewer.rotation(), 90);
}

/// A selection across the words of a line, ready to be pressed in the middle.
fn selected_fox() -> (Ui, PdfViewer) {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    drag(&mut ui, at(19.0, 205.0), at(300.0, 205.0));
    assert!(viewer.has_selection());
    ui.take_requests();
    (ui, viewer)
}

fn offered_text(ui: &mut Ui) -> Option<String> {
    ui.take_requests().into_iter().find_map(|r| match r {
        PlatformRequest::StartDrag(offer) => offer
            .items
            .iter()
            .find(|(mime, _)| mime.starts_with("text/plain"))
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    })
}

#[test]
fn what_is_selected_can_be_carried_to_another_program() {
    let (mut ui, viewer) = selected_fox();
    ui.set_platform_drag(true);
    let p = at(100.0, 205.0);
    press(&mut ui, p, 5000);
    // Pressing it doesn't change it.
    assert!(viewer.has_selection());
    // Not yet: it could still be a click.
    move_to(&mut ui, PointF::new(p.x + 2.0, p.y + 2.0));
    assert_eq!(offered_text(&mut ui), None);
    // It goes as soon as it has moved enough one way.
    move_to(&mut ui, PointF::new(p.x + 5.0, p.y));
    assert_eq!(offered_text(&mut ui).as_deref(), Some(FOX));
    assert!(viewer.has_selection());
    // It is over when the system says so, and the selection is still there.
    ui.handle_event(WindowEvent::DragSourceEnded(DragResult::Copied));
    settle(&mut ui);
    assert!(viewer.has_selection());
    assert_eq!(copied(&mut ui).as_deref(), Some(FOX));
}

#[test]
fn carrying_a_selection_only_lets_it_be_copied() {
    let (mut ui, _) = selected_fox();
    ui.set_platform_drag(true);
    let p = at(100.0, 205.0);
    press(&mut ui, p, 5000);
    move_to(&mut ui, PointF::new(p.x + 8.0, p.y));
    let actions = ui.take_requests().into_iter().find_map(|r| match r {
        PlatformRequest::StartDrag(offer) => Some(offer.actions),
        _ => None,
    });
    assert_eq!(actions, Some(DragActions::Copy));
}

#[test]
fn a_click_on_what_is_selected_lets_go_of_it() {
    let (mut ui, viewer) = selected_fox();
    let p = at(100.0, 205.0);
    press(&mut ui, p, 5000);
    assert!(viewer.has_selection());
    release(&mut ui, p);
    assert!(!viewer.has_selection());
}

#[test]
fn a_press_beside_what_is_selected_starts_over() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    // Only part of the line.
    drag(&mut ui, at(19.0, 205.0), at(45.0, 205.0));
    let first = copied(&mut ui).expect("something is selected");
    ui.set_platform_drag(true);
    ui.take_requests();
    let p = at(70.0, 205.0);
    press(&mut ui, p, 5000);
    assert!(!viewer.has_selection());
    move_to(&mut ui, at(90.0, 205.0));
    assert_eq!(offered_text(&mut ui), None);
    assert!(viewer.has_selection());
    assert_ne!(copied(&mut ui), Some(first));
}

#[test]
fn a_second_click_on_what_is_selected_still_takes_a_word() {
    let (mut ui, viewer) = selected_fox();
    ui.set_platform_drag(true);
    let p = at(100.0, 205.0);
    press(&mut ui, p, 5000);
    release(&mut ui, p);
    press(&mut ui, p, 5100);
    release(&mut ui, p);
    assert!(viewer.has_selection());
    let word = copied(&mut ui).expect("a word");
    assert!(!word.contains(' '), "{word}");
    assert!(FOX.contains(&word), "{word}");
}

#[test]
fn shift_on_what_is_selected_goes_on_selecting() {
    let (mut ui, viewer) = selected_fox();
    ui.set_platform_drag(true);
    let p = at(100.0, 205.0);
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed: true,
        position: p,
        modifiers: shift,
        time: 5000,
    });
    settle(&mut ui);
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(p.x + 20.0, p.y),
        modifiers: shift,
    });
    settle(&mut ui);
    assert_eq!(offered_text(&mut ui), None);
    assert!(viewer.has_selection());
}

#[test]
fn all_of_a_long_selection_is_carried_even_if_it_was_not_read_yet() {
    // More pages than keep their text, of which what doesn't show hasn't been read.
    let pages: Vec<String> = (0..60)
        .map(|i| line(20, 200, &format!("page{i}")))
        .collect();
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    let doc = TextDocument::new(&refs);
    let (mut ui, viewer) = read(&doc);
    let p = at(30.0, 205.0);
    press(&mut ui, p, 0);
    release(&mut ui, p);
    control(&mut ui, "a");
    assert!(viewer.has_selection());
    ui.set_platform_drag(true);
    ui.take_requests();
    press(&mut ui, p, 5000);
    move_to(&mut ui, PointF::new(p.x + 8.0, p.y));
    let text = offered_text(&mut ui).expect("it started");
    assert!(text.contains("page0") && text.contains("page59"), "{text}");
}

#[test]
fn only_the_text_shows_with_the_pointer_and_not_the_page() {
    let (mut ui, _) = selected_fox();
    let p = at(100.0, 205.0);
    press(&mut ui, p, 5000);
    let before = pictures(&mut ui).len();
    move_to(&mut ui, PointF::new(p.x + 8.0, p.y));
    let drawn = pictures(&mut ui);
    assert_eq!(drawn.len(), before + 1, "{drawn:?}");
    let d = drawn
        .iter()
        .find(|d| !d.bitmap.is_opaque())
        .expect("what is carried isn't opaque");
    let mut there = (0u64, 0u64);
    d.bitmap.with_levels(|level, _, pixels| {
        if level == 0 {
            for px in pixels.chunks_exact(4) {
                if px[3] == 0 {
                    there.0 += 1;
                } else {
                    there.1 += 1;
                }
                assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3], "{px:?}");
            }
        }
    });
    let share = there.1 as f64 / (there.0 + there.1) as f64;
    assert!(there.0 > 0 && there.1 > 0, "{there:?}");
    assert!(share < 0.5, "{share}");
    // The pointer stays on what was pressed, as far along it as it was.
    let moved = PointF::new(p.x + 8.0, p.y);
    assert!(
        d.rect.x as f32 <= moved.x && d.rect.right() as f32 >= moved.x,
        "{d:?} {moved:?}"
    );
    assert!(
        d.rect.y as f32 <= moved.y && d.rect.bottom() as f32 >= moved.y,
        "{d:?} {moved:?}"
    );
}

#[test]
fn what_is_selected_can_be_dropped_in_an_input_next_to_it() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let bytes = doc.build();
    let handed = Rc::new(RefCell::new(None));
    let kept = handed.clone();
    let mut ui = mount((560, 460), ONE, move || {
        let v = pdf_viewer();
        let value = signal(String::new());
        *kept.borrow_mut() = Some((v, value));
        v.set_zoom(PdfZoom::Percent(1.0));
        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .child(text_input().value(value).h(40.0).flex_none().w_full())
            .child(pdf_view(bytes).viewer(v).gap(0.0).flex_1().min_h(0.0))
    });
    settle(&mut ui);
    let (viewer, value) = handed.borrow().expect("it was made");
    let below = |p: PointF| PointF::new(p.x, p.y + 40.0);
    drag(&mut ui, below(at(19.0, 205.0)), below(at(300.0, 205.0)));
    assert!(viewer.has_selection());
    let p = below(at(100.0, 205.0));
    press(&mut ui, p, 5000);
    move_to(&mut ui, PointF::new(p.x, p.y - 100.0));
    let input = PointF::new(50.0, 20.0);
    move_to(&mut ui, input);
    release(&mut ui, input);
    assert_eq!(value.get_untracked(), FOX);
    assert!(viewer.has_selection());
}

#[test]
fn what_is_selected_shows_turned_when_carried_from_a_turned_page() {
    let doc = TextDocument::new(&[&line(20, 200, FOX)]);
    let (mut ui, viewer) = read(&doc);
    viewer.rotate_clockwise();
    settle(&mut ui);
    // Turning kept what was in the middle there.
    viewer.go_to_page(0);
    settle(&mut ui);
    drag(&mut ui, at_turned(19.0, 205.0), at_turned(300.0, 205.0));
    assert!(viewer.has_selection());
    let p = at_turned(60.0, 205.0);
    press(&mut ui, p, 5000);
    move_to(&mut ui, PointF::new(p.x, p.y + 8.0));
    let drawn = pictures(&mut ui);
    let d = drawn
        .iter()
        .find(|d| !d.bitmap.is_opaque())
        .expect("what is carried isn't opaque");
    assert!(d.bitmap.height() > d.bitmap.width(), "{d:?}");
}

#[test]
fn pages_stay_as_they_are_while_pinching_and_get_drawn_when_it_is_over() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED, GREEN]);
    viewer.set_zoom(PdfZoom::Percent(1.0));
    settle(&mut ui);
    let p = PointF::new(210.0, 150.0);
    let first = pictures(&mut ui).remove(0);
    let (width, id) = (first.bitmap.width(), first.bitmap.id());
    pinch(&mut ui, p, 1.0, PinchPhase::Began);
    pinch(&mut ui, p, 1.5, PinchPhase::Moved);
    let d = pictures(&mut ui).remove(0);
    assert_eq!(d.bitmap.id(), id, "it stays");
    assert_eq!(d.rect.width as f32, (width as f32 * 1.5).round());
    // It is more than twice as big as what there is now.
    pinch(&mut ui, p, 1.5, PinchPhase::Moved);
    let d = pictures(&mut ui).remove(0);
    assert_ne!(d.bitmap.id(), id, "it was too far off");
    assert_eq!(d.bitmap.width() as i32, d.rect.width);
    let again = d.bitmap.id();
    pinch(&mut ui, p, 1.2, PinchPhase::Moved);
    assert_eq!(pictures(&mut ui).remove(0).bitmap.id(), again);
    pinch(&mut ui, p, 1.0, PinchPhase::Ended);
    let d = pictures(&mut ui).remove(0);
    assert_eq!(d.bitmap.width() as i32, d.rect.width, "it is sharp again");
}

#[test]
fn nothing_ahead_is_drawn_while_pinching() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED, GREEN, RED, GREEN, RED, GREEN]);
    viewer.set_zoom(PdfZoom::Percent(1.0));
    settle(&mut ui);
    let p = PointF::new(210.0, 150.0);
    pinch(&mut ui, p, 1.0, PinchPhase::Began);
    pinch(&mut ui, p, 0.7, PinchPhase::Moved);
    pinch(&mut ui, p, 0.7, PinchPhase::Moved);
    // More pages show now, and those that have nothing are all that is drawn.
    for d in pictures(&mut ui) {
        assert_ne!(d.bitmap.width(), 0);
    }
    pinch(&mut ui, p, 1.0, PinchPhase::Cancelled);
    for d in pictures(&mut ui) {
        assert_eq!(d.bitmap.width() as i32, d.rect.width, "{d:?}");
    }
}

#[test]
fn what_is_selected_goes_on_a_surface_of_its_own_when_carried_to_another_program() {
    let (mut ui, _) = selected_fox();
    ui.set_platform_drag(true);
    let p = at(60.0, 205.0);
    press(&mut ui, p, 5000);
    move_to(&mut ui, PointF::new(p.x + 8.0, p.y));
    let icon = ui
        .paint_frame(Instant::now())
        .drag_icon
        .expect("it has one");
    assert!(icon.logical.width > 0 && icon.logical.height > 0);
    // The pointer is on what it was pressed on.
    assert!(
        icon.hotspot.x > 0 && icon.hotspot.x < icon.logical.width,
        "{icon:?}"
    );
    assert!(
        icon.hotspot.y >= 0 && icon.hotspot.y < icon.logical.height,
        "{icon:?}"
    );
}

/// Plays the platform: the window goes on all of the screen or comes back.
fn window_went(ui: &mut Ui, on: bool, size: (i32, i32)) {
    ui.handle_event(WindowEvent::Fullscreen(on));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(size.0, size.1),
        scale: ONE,
    });
    ui.update();
}

fn slide_key(ui: &mut Ui, key: Key) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key,
        text: None,
        modifiers: Modifiers::default(),
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

fn named(ui: &mut Ui, key: NamedKey) {
    slide_key(ui, Key::Named(key));
}

const SCREEN: (i32, i32) = (800, 600);

/// A document of pages 100 by 200 points, presenting from the page that showed.
fn presenting(colors: &[(f32, f32, f32)], page: usize) -> (Ui, PdfViewer) {
    let (mut ui, viewer) = viewer_of((420, 300), colors);
    settle(&mut ui);
    viewer.go_to_page(page);
    settle(&mut ui);
    viewer.present();
    window_went(&mut ui, true, SCREEN);
    settle(&mut ui);
    assert!(viewer.is_presenting());
    (ui, viewer)
}

/// The one picture that shows, and what color it is.
fn the_slide(ui: &mut Ui) -> (ImageDraw, [u8; 4]) {
    let drawn = pictures(ui);
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    let color = pixel(&drawn[0], 5, 5);
    (drawn[0].clone(), color)
}

const SHOWS_RED: [u8; 4] = [255, 0, 0, 255];
const SHOWS_GREEN: [u8; 4] = [0, 255, 0, 255];
const SHOWS_BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn presenting_shows_one_page_in_the_middle_of_the_screen_on_black() {
    let (mut ui, viewer) = presenting(&[RED, GREEN, BLUE], 1);
    assert_eq!(viewer.page(), 1);
    let (slide, color) = the_slide(&mut ui);
    assert_eq!(color, SHOWS_GREEN);
    // As high as the screen, and half as wide as that, with whole pixels on the screen.
    assert_eq!(
        slide.rect,
        anti_webui_core::geometry::RectI::new(250, 0, 300, 600)
    );
    assert_eq!((slide.bitmap.width(), slide.bitmap.height()), (300, 600));
}

#[test]
fn the_next_pages_are_there_before_they_are_asked_for() {
    let (mut ui, _viewer) = presenting(&[RED, GREEN, BLUE, RED], 0);
    named(&mut ui, NamedKey::ArrowRight);
    // Right away, with nothing to wait for.
    let (slide, color) = the_slide(&mut ui);
    assert_eq!(color, SHOWS_GREEN);
    assert_eq!((slide.bitmap.width(), slide.bitmap.height()), (300, 600));
    settle(&mut ui);
    named(&mut ui, NamedKey::ArrowLeft);
    let (_, color) = the_slide(&mut ui);
    assert_eq!(color, SHOWS_RED);
}

#[test]
fn a_page_that_isnt_drawn_yet_leaves_the_one_before_it() {
    let colors: Vec<_> = (0..30).map(|i| if i == 29 { BLUE } else { RED }).collect();
    let (mut ui, viewer) = presenting(&colors, 0);
    named(&mut ui, NamedKey::End);
    assert_eq!(viewer.page(), 29);
    let (slide, color) = the_slide(&mut ui);
    assert_eq!(color, SHOWS_RED);
    assert_eq!(slide.rect.height, 600);
    settle(&mut ui);
    let (_, color) = the_slide(&mut ui);
    assert_eq!(color, SHOWS_BLUE);
}

#[test]
fn a_page_that_takes_too_long_goes_ahead_without_its_picture() {
    let colors = vec![RED; 30];
    let (mut ui, viewer) = presenting(&colors, 0);
    named(&mut ui, NamedKey::End);
    // Whether it is drawn by then or not, the wait ends.
    std::thread::sleep(Duration::from_millis(350));
    ui.tick(Instant::now());
    ui.update();
    assert_eq!(viewer.page(), 29);
    let shown = pictures(&mut ui);
    assert!(shown.len() <= 1, "{shown:?}");
    settle(&mut ui);
    assert_eq!(the_slide(&mut ui).0.rect.height, 600);
}

#[test]
fn keys_go_through_the_pages_and_stop_at_the_ends() {
    let (mut ui, viewer) = presenting(&[RED; 12], 0);
    for key in [
        NamedKey::ArrowRight,
        NamedKey::ArrowDown,
        NamedKey::PageDown,
        NamedKey::Space,
        NamedKey::Enter,
    ] {
        let before = viewer.page();
        named(&mut ui, key);
        assert_eq!(viewer.page(), before + 1, "{key:?}");
    }
    slide_key(&mut ui, Key::Character("n".into()));
    assert_eq!(viewer.page(), 6);
    for key in [
        NamedKey::ArrowLeft,
        NamedKey::ArrowUp,
        NamedKey::PageUp,
        NamedKey::Backspace,
    ] {
        let before = viewer.page();
        named(&mut ui, key);
        assert_eq!(viewer.page(), before - 1, "{key:?}");
    }
    slide_key(&mut ui, Key::Character("p".into()));
    assert_eq!(viewer.page(), 1);
    named(&mut ui, NamedKey::Home);
    named(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(viewer.page(), 0);
    named(&mut ui, NamedKey::End);
    named(&mut ui, NamedKey::ArrowRight);
    assert_eq!(viewer.page(), 11);
    // A number and Enter go to that page, counting from 1.
    slide_key(&mut ui, Key::Character("1".into()));
    slide_key(&mut ui, Key::Character("0".into()));
    named(&mut ui, NamedKey::Enter);
    assert_eq!(viewer.page(), 9);
}

fn mouse(ui: &mut Ui, button: PointerButton, modifiers: Modifiers) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            button,
            pressed,
            position: PointF::new(400.0, 300.0),
            modifiers,
            time: 0,
        });
        ui.update();
    }
}

#[test]
fn clicks_go_through_the_pages_and_select_nothing() {
    let (mut ui, viewer) = presenting(&[RED; 5], 0);
    mouse(&mut ui, PointerButton::Primary, Modifiers::default());
    assert_eq!(viewer.page(), 1);
    assert!(!viewer.has_selection());
    mouse(&mut ui, PointerButton::Primary, Modifiers::default());
    assert_eq!(viewer.page(), 2);
    mouse(&mut ui, PointerButton::Secondary, Modifiers::default());
    assert_eq!(viewer.page(), 1);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    mouse(&mut ui, PointerButton::Primary, shift);
    assert_eq!(viewer.page(), 0);
}

fn scrolled(ui: &mut Ui, delta_y: f32, source: ScrollSource, phase: ScrollPhase) {
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(400.0, 300.0),
        delta_x: 0.0,
        delta_y,
        modifiers: Modifiers::default(),
        source,
        phase,
        time: 0,
    });
    ui.update();
}

#[test]
fn a_notch_of_a_wheel_is_a_page() {
    let (mut ui, viewer) = presenting(&[RED; 5], 0);
    scrolled(&mut ui, 1.0, ScrollSource::Wheel, ScrollPhase::Single);
    assert_eq!(viewer.page(), 1);
    scrolled(&mut ui, 1.0, ScrollSource::Wheel, ScrollPhase::Single);
    assert_eq!(viewer.page(), 2);
    scrolled(&mut ui, -1.0, ScrollSource::Wheel, ScrollPhase::Single);
    assert_eq!(viewer.page(), 1);
}

#[test]
fn a_swipe_is_a_page_however_long_it_is() {
    let (mut ui, viewer) = presenting(&[RED; 5], 0);
    let finger = ScrollSource::Touchpad;
    scrolled(&mut ui, 5.0, finger, ScrollPhase::Began);
    assert_eq!(viewer.page(), 0);
    for _ in 0..50 {
        scrolled(&mut ui, 30.0, finger, ScrollPhase::Moved);
    }
    assert_eq!(viewer.page(), 1);
    scrolled(&mut ui, 0.0, finger, ScrollPhase::Lifted { fling: false });
    for _ in 0..50 {
        scrolled(&mut ui, 30.0, finger, ScrollPhase::Momentum);
    }
    assert_eq!(viewer.page(), 1);
    scrolled(&mut ui, 5.0, finger, ScrollPhase::Began);
    scrolled(&mut ui, 100.0, finger, ScrollPhase::Moved);
    assert_eq!(viewer.page(), 2);
}

#[test]
fn zooming_does_nothing_while_presenting() {
    let (mut ui, viewer) = presenting(&[RED; 5], 0);
    let zoom = viewer.zoom();
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(400.0, 300.0),
        delta_x: 0.0,
        delta_y: 0.0,
        modifiers: Modifiers::PRIMARY,
        source: ScrollSource::Touchpad,
        phase: ScrollPhase::Moved,
        time: 0,
    });
    ui.handle_event(WindowEvent::Pinch {
        position: PointF::new(400.0, 300.0),
        scale: 2.0,
        modifiers: Modifiers::default(),
        phase: PinchPhase::Moved,
    });
    ui.update();
    assert_eq!(viewer.zoom(), zoom);
    assert_eq!(the_slide(&mut ui).0.rect.height, 600);
}

#[test]
fn the_screen_can_go_black_or_white_and_come_back() {
    let (mut ui, viewer) = presenting(&[RED; 5], 0);
    let on_top = |ui: &mut Ui| {
        let scene = ui.paint(Instant::now());
        let mut quads = Vec::new();
        fn walk(scene: &Scene, out: &mut Vec<Color>) {
            for p in &scene.primitives {
                match p {
                    Primitive::Quad(q) => out.push(q.background),
                    Primitive::Layer(l) => walk(&l.content, out),
                    _ => {}
                }
            }
        }
        walk(&scene, &mut quads);
        quads.last().copied()
    };
    slide_key(&mut ui, Key::Character("b".into()));
    assert_eq!(on_top(&mut ui), Some(Color::BLACK));
    // Whatever comes next only brings back what was there.
    named(&mut ui, NamedKey::ArrowRight);
    assert_eq!(viewer.page(), 0);
    assert_ne!(on_top(&mut ui), Some(Color::BLACK));
    slide_key(&mut ui, Key::Character("w".into()));
    assert_eq!(on_top(&mut ui), Some(Color::WHITE));
    mouse(&mut ui, PointerButton::Primary, Modifiers::default());
    assert_eq!(viewer.page(), 0);
    assert_eq!(the_slide(&mut ui).1, SHOWS_RED);
}

#[test]
fn escape_goes_back_to_the_same_place_on_the_same_page() {
    let colors = vec![RED; 20];
    let (mut ui, viewer) = viewer_of((420, 300), &colors);
    settle(&mut ui);
    viewer.set_zoom(PdfZoom::Percent(1.5));
    ui.update();
    viewer.go_to(PdfTarget {
        page: 3,
        top: Some(50.0),
    });
    settle(&mut ui);
    let before: Vec<_> = pictures(&mut ui).iter().map(|d| d.rect).collect();
    let zoom = viewer.zoom();
    let page = viewer.page();

    viewer.present();
    window_went(&mut ui, true, SCREEN);
    settle(&mut ui);
    assert_eq!(viewer.page(), page);
    named(&mut ui, NamedKey::Escape);
    let asked = ui.take_requests();
    assert!(
        asked.contains(&PlatformRequest::SetFullscreen(false)),
        "{asked:?}"
    );
    assert!(viewer.is_presenting());
    window_went(&mut ui, false, (420, 300));
    settle(&mut ui);

    assert!(!viewer.is_presenting());
    assert_eq!(viewer.zoom(), zoom);
    assert_eq!(viewer.page(), page);
    let after: Vec<_> = pictures(&mut ui).iter().map(|d| d.rect).collect();
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(&before) {
        assert!(
            (a.x - b.x).abs() <= 1 && (a.y - b.y).abs() <= 1,
            "{after:?} {before:?}"
        );
    }
}

#[test]
fn ending_on_another_page_goes_to_that_page() {
    let (mut ui, viewer) = presenting(&[RED; 20], 0);
    slide_key(&mut ui, Key::Character("9".into()));
    named(&mut ui, NamedKey::Enter);
    settle(&mut ui);
    viewer.stop_presenting();
    window_went(&mut ui, false, (420, 300));
    settle(&mut ui);
    assert!(!viewer.is_presenting());
    assert_eq!(viewer.page(), 8);
    // It reads on from there as it did before.
    let drawn = pictures(&mut ui);
    let rects: Vec<_> = drawn.iter().map(|d| d.rect).collect();
    assert!(rects.iter().any(|r| r.y >= 0 && r.y <= 10), "{rects:?}");
}

#[test]
fn the_system_ending_it_goes_back_too() {
    let (mut ui, viewer) = presenting(&[RED; 5], 2);
    window_went(&mut ui, false, (420, 300));
    settle(&mut ui);
    assert!(!viewer.is_presenting());
    assert_eq!(viewer.page(), 2);
}

#[test]
fn pages_of_other_sizes_each_fit_the_screen() {
    // Each page says its own size, which is what the second one takes.
    let mut data = document((100, 200), &[RED, GREEN]);
    let box_of_first = b"/MediaBox [0 0 100 200]";
    let second = data
        .windows(box_of_first.len())
        .rposition(|w| w == box_of_first)
        .unwrap();
    let wide = b"/MediaBox [0 0 400 100]";
    data[second..second + wide.len()].copy_from_slice(wide);
    let viewer = Rc::new(std::cell::Cell::new(None));
    let handed = viewer.clone();
    let mut ui = mount((420, 300), ONE, move || {
        let v = pdf_viewer();
        handed.set(Some(v));
        pdf_view(data).viewer(v).w_full().h_full()
    });
    settle(&mut ui);
    let viewer = viewer.get().unwrap();
    viewer.present();
    window_went(&mut ui, true, SCREEN);
    settle(&mut ui);
    let (slide, _) = the_slide(&mut ui);
    assert_eq!(
        slide.rect,
        anti_webui_core::geometry::RectI::new(250, 0, 300, 600)
    );
    named(&mut ui, NamedKey::ArrowRight);
    settle(&mut ui);
    let (slide, _) = the_slide(&mut ui);
    assert_eq!(
        slide.rect,
        anti_webui_core::geometry::RectI::new(0, 200, 800, 200)
    );
}

#[test]
fn links_are_followed_while_presenting_and_the_pointer_goes_away_by_itself() {
    let mut doc = TextDocument::new(&[
        &line(20, 200, "first"),
        &line(20, 200, "second"),
        &line(20, 200, "third"),
    ]);
    doc.page_extras[0] = "/Annots [\
        << /Type /Annot /Subtype /Link /Rect [0 0 300 300] /Dest [14 0 R /Fit] >> ]"
        .into();
    let (mut ui, viewer) = read_with(&doc, |view| view);
    viewer.present();
    window_went(&mut ui, true, (420, 420));
    settle(&mut ui);
    ui.take_requests();
    let p = PointF::new(210.0, 210.0);
    move_to(&mut ui, p);
    settle(&mut ui);
    move_to(&mut ui, p);
    settle(&mut ui);
    let asked = ui.take_requests();
    assert!(
        asked.contains(&PlatformRequest::SetCursor(CursorIcon::Pointer)),
        "{asked:?}"
    );
    press(&mut ui, p, 0);
    release(&mut ui, p);
    settle(&mut ui);
    assert_eq!(viewer.page(), 2);
}

#[test]
fn the_pointer_goes_away_when_it_stays_where_it_is() {
    let (mut ui, _viewer) = presenting(&[RED; 5], 0);
    let p = PointF::new(400.0, 300.0);
    move_to(&mut ui, p);
    ui.take_requests();
    ui.tick(Instant::now() + Duration::from_secs(4));
    ui.update();
    let asked = ui.take_requests();
    assert!(
        asked.contains(&PlatformRequest::SetCursor(CursorIcon::None)),
        "{asked:?}"
    );
    // Turning a page by the keyboard doesn't bring it back, and moving it does.
    named(&mut ui, NamedKey::ArrowRight);
    let asked = ui.take_requests();
    assert!(
        !asked.contains(&PlatformRequest::SetCursor(CursorIcon::Default)),
        "{asked:?}"
    );
    move_to(&mut ui, PointF::new(401.0, 300.0));
    ui.update();
    let asked = ui.take_requests();
    assert!(
        asked.contains(&PlatformRequest::SetCursor(CursorIcon::Default)),
        "{asked:?}"
    );
}

#[test]
fn a_window_already_on_all_of_the_screen_stays_there_after_presenting() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED; 5]);
    window_went(&mut ui, true, SCREEN);
    settle(&mut ui);
    ui.take_requests();
    viewer.present();
    ui.update();
    assert!(viewer.is_presenting());
    settle(&mut ui);
    assert_eq!(the_slide(&mut ui).0.rect.height, 600);
    named(&mut ui, NamedKey::Escape);
    assert!(!viewer.is_presenting());
    let asked = ui.take_requests();
    assert!(
        !asked
            .iter()
            .any(|r| matches!(r, PlatformRequest::SetFullscreen(_))),
        "{asked:?}"
    );
}

#[test]
fn stopping_before_it_has_started_keeps_it_from_starting() {
    let (mut ui, viewer) = viewer_of((420, 300), &[RED; 5]);
    settle(&mut ui);
    viewer.present();
    viewer.stop_presenting();
    let asked = ui.take_requests();
    assert!(
        asked.contains(&PlatformRequest::SetFullscreen(false)),
        "{asked:?}"
    );
    // The window may still get there on its way back.
    window_went(&mut ui, true, SCREEN);
    settle(&mut ui);
    assert!(!viewer.is_presenting());
}
