//! Filling in forms and marking up documents: real documents, edited, drawn and written. See
//! `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_pdf::{
    PdfAnnotationKind, PdfEditing, PdfFieldChange, PdfFieldKind, PdfFieldValue, PdfTool, PdfView,
    PdfViewer, PdfZoom, pdf_view, pdf_viewer,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A document made of a page, the objects it has by number, and an index.
struct Document {
    catalog_extra: String,
    page_extra: String,
    content: String,
    objects: Vec<(usize, String)>,
    /// The index is a stream rather than a table.
    index_stream: bool,
}

impl Document {
    fn build(&self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut all: Vec<(usize, String)> = vec![
            (
                1,
                format!("<< /Type /Catalog /Pages 2 0 R {} >>", self.catalog_extra),
            ),
            (2, "<< /Type /Pages /Count 1 /Kids [10 0 R] >>".to_owned()),
            (
                3,
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            ),
            (
                10,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents 11 0 R \
                     /Resources << /Font << /F1 3 0 R >> >> {} >>",
                    self.page_extra
                ),
            ),
            (
                11,
                format!(
                    "<< /Length {} >>\nstream\n{}\nendstream",
                    self.content.len(),
                    self.content
                ),
            ),
        ];
        all.extend(self.objects.iter().cloned());
        let mut offsets = Vec::new();
        for (id, body) in &all {
            offsets.push((*id, out.len()));
            out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let start = out.len();
        let size = offsets.iter().map(|o| o.0).max().unwrap_or(0) + 2;
        if self.index_stream {
            let own = size - 1;
            offsets.push((own, start));
            offsets.sort_unstable();
            let mut index = Vec::new();
            let mut ranges = String::new();
            for (id, offset) in &offsets {
                ranges.push_str(&format!("{id} 1 "));
                index.push(1u8);
                index.extend_from_slice(&(*offset as u32).to_be_bytes());
                index.extend_from_slice(&[0, 0]);
            }
            out.extend_from_slice(
                format!(
                    "{own} 0 obj\n<< /Type /XRef /Size {size} /W [1 4 2] /Index [{ranges}] \
                     /Root 1 0 R /Length {} >>\nstream\n",
                    index.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&index);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        } else {
            out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
            for (id, offset) in &offsets {
                out.extend_from_slice(format!("{id} 1\n{offset:010} 00000 n \n").as_bytes());
            }
            out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\n").as_bytes());
        }
        out.extend_from_slice(format!("startxref\n{start}\n%%EOF\n").as_bytes());
        out
    }
}

/// A form with a text field at the top, a check box and two radio buttons below it, then a list that
/// opens, a field that is read only and one that goes over several lines.
fn form() -> Document {
    let box_on = "0 0 0 rg 2 2 16 16 re f";
    Document {
        catalog_extra: "/AcroForm << /Fields [100 0 R 101 0 R 102 0 R 105 0 R 106 0 R 107 0 R] \
                        /DR << /Font << /Helv 3 0 R >> >> /DA (/Helv 0 Tf 0 g) >>"
            .to_owned(),
        page_extra: "/Annots [100 0 R 101 0 R 103 0 R 104 0 R 105 0 R 106 0 R 107 0 R]".to_owned(),
        content: String::new(),
        objects: vec![
            (
                100,
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 240 200 270] \
                 /DA (/Helv 14 Tf 0 g) /F 4 >>"
                    .to_owned(),
            ),
            (
                101,
                "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /Rect [20 200 40 220] \
                 /V /Off /AS /Off /F 4 /AP << /N << /Yes 110 0 R /Off 111 0 R >> >> >>"
                    .to_owned(),
            ),
            (
                102,
                "<< /FT /Btn /Ff 32768 /T (pick) /Kids [103 0 R 104 0 R] /V /Off >>".to_owned(),
            ),
            (
                103,
                "<< /Type /Annot /Subtype /Widget /Parent 102 0 R /Rect [60 200 80 220] /AS /Off \
                 /F 4 /AP << /N << /A 110 0 R /Off 111 0 R >> >> >>"
                    .to_owned(),
            ),
            (
                104,
                "<< /Type /Annot /Subtype /Widget /Parent 102 0 R /Rect [90 200 110 220] /AS /Off \
                 /F 4 /AP << /N << /B 110 0 R /Off 111 0 R >> >> >>"
                    .to_owned(),
            ),
            (
                105,
                "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (color) \
                 /Rect [20 160 200 190] /DA (/Helv 12 Tf 0 g) /F 4 \
                 /Opt [[(r) (Red)] [(g) (Green)]] >>"
                    .to_owned(),
            ),
            (
                106,
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (locked) /Ff 1 /Rect [20 120 200 150] \
                 /DA (/Helv 12 Tf 0 g) /V (Fixed) /F 4 >>"
                    .to_owned(),
            ),
            (
                107,
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (story) /Ff 4096 \
                 /Rect [20 20 380 110] /DA (/Helv 0 Tf 0 g) /F 4 >>"
                    .to_owned(),
            ),
            (
                110,
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length {} >>\nstream\n{box_on}\nendstream",
                    box_on.len()
                ),
            ),
            (
                111,
                "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream"
                    .to_owned(),
            ),
        ],
        index_stream: false,
    }
}

/// A page with two lines of text on it.
fn text_page() -> Document {
    Document {
        catalog_extra: String::new(),
        page_extra: String::new(),
        content: "BT /F1 20 Tf 40 240 Td (The quick brown fox) Tj ET\n\
                  BT /F1 20 Tf 40 200 Td (jumps over the dog) Tj ET\n"
            .to_owned(),
        objects: Vec::new(),
        index_stream: false,
    }
}

fn open(bytes: Vec<u8>) -> (Ui, PdfViewer) {
    open_with(bytes, |view| view)
}

fn open_with(bytes: Vec<u8>, f: impl FnOnce(PdfView) -> PdfView) -> (Ui, PdfViewer) {
    let viewer = Rc::new(RefCell::new(None));
    let kept = viewer.clone();
    let mut ui = Ui::new(move || {
        let v = pdf_viewer();
        *kept.borrow_mut() = Some(v);
        v.set_zoom(PdfZoom::Percent(1.0));
        f(pdf_view(bytes).viewer(v).gap(0.0)).w_full().h_full()
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(560, 420),
        scale: Scale(120),
    });
    settle(&mut ui);
    let v = viewer.borrow().expect("it was made");
    v.focus();
    ui.update();
    (ui, v)
}

fn settle(ui: &mut Ui) {
    for _ in 0..8 {
        ui.update();
        ui.wait_for_images(Duration::from_secs(30));
    }
}

/// Where a point of the page is in the window, with 4/3 of a pixel to a point.
fn at(x: f32, y: f32) -> PointF {
    PointF::new(x * 4.0 / 3.0, (300.0 - y) * 4.0 / 3.0)
}

thread_local! {
    /// When things happen, far enough apart for no two clicks to be taken for one double click.
    static TIME: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn press(ui: &mut Ui, p: PointF, pressed: bool) {
    press_at(ui, p, pressed, TIME.get() + 10_000);
}

fn press_at(ui: &mut Ui, p: PointF, pressed: bool, time: u32) {
    TIME.set(time);
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed,
        position: p,
        modifiers: Modifiers::default(),
        time,
    });
    settle(ui);
}

fn click(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    settle(ui);
    press(ui, p, true);
    press(ui, p, false);
}

fn drag(ui: &mut Ui, from: PointF, to: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: from,
        modifiers: Modifiers::default(),
    });
    settle(ui);
    press(ui, from, true);
    ui.handle_event(WindowEvent::PointerMoved {
        position: to,
        modifiers: Modifiers::default(),
    });
    settle(ui);
    press(ui, to, false);
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

fn key(ui: &mut Ui, key: NamedKey) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(key),
        text: None,
        modifiers: Modifiers::default(),
        pressed: true,
        repeat: false,
    }));
    settle(ui);
}

fn control(ui: &mut Ui, c: &str) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Character(c.into()),
        text: None,
        modifiers: Modifiers::PRIMARY,
        pressed: true,
        repeat: false,
    }));
    settle(ui);
}

fn first_picture(ui: &mut Ui) -> ImageDraw {
    fn walk(scene: &Scene, out: &mut Vec<ImageDraw>) {
        for p in &scene.primitives {
            match p {
                Primitive::Image(d) => out.push(d.clone()),
                Primitive::Layer(l) => walk(&l.content, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&ui.paint(Instant::now()), &mut out);
    out.into_iter().next().expect("there is a picture")
}

/// How many pixels of a box of the page, in points, are as a test says.
fn count(ui: &mut Ui, area: [f32; 4], test: impl Fn([u8; 4]) -> bool) -> usize {
    let picture = first_picture(ui);
    let width = picture.bitmap.width();
    let mut found = 0;
    picture.bitmap.with_levels(|level, _, pixels| {
        if level != 0 {
            return;
        }
        let (x0, y0) = (at(area[0], area[3]).x as u32, at(area[0], area[3]).y as u32);
        let (x1, y1) = (at(area[2], area[1]).x as u32, at(area[2], area[1]).y as u32);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * width + x) * 4) as usize;
                if test([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]) {
                    found += 1;
                }
            }
        }
    });
    found
}

fn dark(p: [u8; 4]) -> bool {
    p[0] < 100 && p[1] < 100 && p[2] < 100
}

/// What is saved, by way of a real file.
fn saved(ui: &mut Ui, viewer: PdfViewer) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!(
        "awui-pdf-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.pdf");
    viewer.save(&path);
    settle(ui);
    assert_eq!(viewer.save_result(), Some(Ok(path.clone())));
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    bytes
}

const NAME: [f32; 4] = [20.0, 240.0, 200.0, 270.0];
const CHECK: [f32; 4] = [20.0, 200.0, 40.0, 220.0];
const RADIO_A: [f32; 4] = [60.0, 200.0, 80.0, 220.0];
const RADIO_B: [f32; 4] = [90.0, 200.0, 110.0, 220.0];

fn text_of(value: Option<PdfFieldValue>) -> String {
    match value {
        Some(PdfFieldValue::Text(text)) => text,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_fields_of_a_form_are_read() {
    let (_ui, viewer) = open(form().build());
    assert_eq!(viewer.editing(), PdfEditing::Yes);
    let fields = viewer.fields();
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["name", "agree", "pick", "color", "locked", "story"]);
    assert_eq!(fields[0].kind, PdfFieldKind::Text);
    assert_eq!(fields[1].kind, PdfFieldKind::Check);
    assert_eq!(fields[1].states, ["Yes"]);
    assert_eq!(fields[2].kind, PdfFieldKind::Radio);
    assert_eq!(fields[2].states, ["A", "B"]);
    assert_eq!(fields[3].kind, PdfFieldKind::Combo { editable: false });
    assert_eq!(fields[3].options[1], ("g".to_owned(), "Green".to_owned()));
    assert!(fields[4].read_only);
    assert_eq!(
        viewer.field_value("locked"),
        Some(PdfFieldValue::Text("Fixed".to_owned()))
    );
}

#[test]
fn what_goes_in_a_field_shows_on_the_page_and_going_back_takes_it_away() {
    for index_stream in [false, true] {
        let doc = Document {
            index_stream,
            ..form()
        };
        let (mut ui, viewer) = open(doc.build());
        assert_eq!(count(&mut ui, NAME, dark), 0);
        assert!(!viewer.is_modified());
        assert!(viewer.set_field("name", PdfFieldValue::Text("Hello".to_owned())));
        settle(&mut ui);
        assert_eq!(viewer.edit_error(), None);
        assert!(count(&mut ui, NAME, dark) > 20);
        assert!(viewer.is_modified());
        assert!(viewer.can_undo());
        viewer.undo();
        settle(&mut ui);
        assert_eq!(count(&mut ui, NAME, dark), 0);
        assert!(!viewer.is_modified());
        viewer.redo();
        settle(&mut ui);
        assert!(count(&mut ui, NAME, dark) > 20);
        assert_eq!(
            viewer.field_value("name"),
            Some(PdfFieldValue::Text("Hello".to_owned()))
        );
    }
}

#[test]
fn a_click_turns_a_check_box_on_and_off() {
    let (mut ui, viewer) = open(form().build());
    assert_eq!(count(&mut ui, CHECK, dark), 0);
    click(&mut ui, at(30.0, 210.0));
    assert_eq!(
        viewer.field_value("agree"),
        Some(PdfFieldValue::State(Some("Yes".to_owned())))
    );
    assert!(count(&mut ui, CHECK, dark) > 300);
    click(&mut ui, at(30.0, 210.0));
    assert_eq!(
        viewer.field_value("agree"),
        Some(PdfFieldValue::State(None))
    );
    assert_eq!(count(&mut ui, CHECK, dark), 0);
}

#[test]
fn one_radio_button_turns_the_other_off() {
    let (mut ui, viewer) = open(form().build());
    click(&mut ui, at(70.0, 210.0));
    assert!(count(&mut ui, RADIO_A, dark) > 300);
    assert_eq!(count(&mut ui, RADIO_B, dark), 0);
    click(&mut ui, at(100.0, 210.0));
    assert_eq!(count(&mut ui, RADIO_A, dark), 0);
    assert!(count(&mut ui, RADIO_B, dark) > 300);
    assert_eq!(
        viewer.field_value("pick"),
        Some(PdfFieldValue::State(Some("B".to_owned())))
    );
}

#[test]
fn typing_in_a_field_commits_on_enter_and_escape_goes_back() {
    let changes = Rc::new(RefCell::new(Vec::<PdfFieldChange>::new()));
    let told = changes.clone();
    let (mut ui, viewer) = open_with(form().build(), move |view| {
        view.on_field_change(move |c| told.borrow_mut().push(c.clone()))
    });
    click(&mut ui, at(100.0, 255.0));
    typed(&mut ui, "Ada");
    // Nothing happens to the document until it is done.
    assert_eq!(text_of(viewer.field_value("name")), "");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(text_of(viewer.field_value("name")), "Ada");
    assert!(count(&mut ui, NAME, dark) > 10);
    assert_eq!(changes.borrow().len(), 1);
    assert_eq!(changes.borrow()[0].name, "name");

    click(&mut ui, at(100.0, 255.0));
    typed(&mut ui, " Lovelace");
    key(&mut ui, NamedKey::Escape);
    assert_eq!(text_of(viewer.field_value("name")), "Ada");
    assert_eq!(changes.borrow().len(), 1);
}

#[test]
fn clicking_elsewhere_commits_what_was_typed() {
    let (mut ui, viewer) = open(form().build());
    click(&mut ui, at(100.0, 255.0));
    typed(&mut ui, "Grace");
    click(&mut ui, at(300.0, 150.0));
    assert_eq!(text_of(viewer.field_value("name")), "Grace");
}

#[test]
fn tab_goes_on_to_the_next_field_that_takes_text() {
    let (mut ui, viewer) = open(form().build());
    click(&mut ui, at(100.0, 255.0));
    typed(&mut ui, "One");
    key(&mut ui, NamedKey::Tab);
    assert_eq!(text_of(viewer.field_value("name")), "One");
    // The list comes next, which only shows what it has. Picking is a click.
    key(&mut ui, NamedKey::Escape);
}

#[test]
fn a_field_that_is_read_only_takes_nothing() {
    let (mut ui, viewer) = open(form().build());
    click(&mut ui, at(100.0, 135.0));
    typed(&mut ui, "x");
    assert_eq!(text_of(viewer.field_value("locked")), "Fixed");
    assert!(!viewer.is_modified());
}

#[test]
fn saving_nothing_keeps_the_file_as_it_was() {
    let bytes = form().build();
    let (mut ui, viewer) = open(bytes.clone());
    assert_eq!(saved(&mut ui, viewer), bytes);
}

#[test]
fn what_is_saved_is_the_old_file_with_more_at_the_end_and_opens_again() {
    for index_stream in [false, true] {
        let doc = Document {
            index_stream,
            ..form()
        };
        let bytes = doc.build();
        let (mut ui, viewer) = open(bytes.clone());
        viewer.set_field("name", PdfFieldValue::Text("Saved".to_owned()));
        viewer.set_field("agree", PdfFieldValue::State(Some("Yes".to_owned())));
        viewer.set_field("pick", PdfFieldValue::State(Some("B".to_owned())));
        viewer.set_field("color", PdfFieldValue::Choice(vec!["g".to_owned()]));
        settle(&mut ui);
        let file = saved(&mut ui, viewer);
        assert!(file.starts_with(&bytes));
        assert!(file.len() > bytes.len());
        assert!(!viewer.is_modified());

        let (mut ui, again) = open(file);
        assert_eq!(text_of(again.field_value("name")), "Saved");
        assert_eq!(
            again.field_value("agree"),
            Some(PdfFieldValue::State(Some("Yes".to_owned())))
        );
        assert_eq!(
            again.field_value("pick"),
            Some(PdfFieldValue::State(Some("B".to_owned())))
        );
        assert_eq!(
            again.field_value("color"),
            Some(PdfFieldValue::Choice(vec!["g".to_owned()]))
        );
        assert!(count(&mut ui, NAME, dark) > 20);
        assert!(count(&mut ui, CHECK, dark) > 300);
        assert!(count(&mut ui, RADIO_B, dark) > 300);
    }
}

#[test]
fn a_document_saved_twice_does_not_pile_up() {
    let (mut ui, viewer) = open(form().build());
    viewer.set_field("name", PdfFieldValue::Text("One".to_owned()));
    settle(&mut ui);
    let first = saved(&mut ui, viewer);
    viewer.set_field("name", PdfFieldValue::Text("Two".to_owned()));
    settle(&mut ui);
    let second = saved(&mut ui, viewer);
    let sections = |file: &[u8]| file.windows(9).filter(|w| *w == b"startxref").count();
    assert_eq!(sections(&first), sections(&second));
}

#[test]
fn a_field_with_a_value_and_no_look_gets_one_when_opening() {
    let mut doc = form();
    doc.objects[0].1 = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 240 200 270] \
                        /DA (/Helv 14 Tf 0 g) /V (There) /F 4 >>"
        .to_owned();
    let bytes = doc.build();
    let (mut ui, viewer) = open(bytes.clone());
    assert!(count(&mut ui, NAME, dark) > 20);
    // That isn't something that was changed, and doesn't go into the file by itself.
    assert!(!viewer.is_modified());
    assert_eq!(saved(&mut ui, viewer), bytes);
}

#[test]
fn what_is_selected_gets_a_highlight_and_the_text_stays_readable() {
    let (mut ui, viewer) = open(text_page().build());
    drag(&mut ui, at(40.0, 248.0), at(240.0, 248.0));
    assert!(viewer.has_selection());
    viewer.clear_selection();
    drag(&mut ui, at(40.0, 248.0), at(240.0, 248.0));
    viewer.highlight_selection();
    settle(&mut ui);
    assert!(!viewer.has_selection());
    assert_eq!(viewer.annotations(0).len(), 1);
    assert_eq!(viewer.annotations(0)[0].1, PdfAnnotationKind::Highlight);
    let yellow = |p: [u8; 4]| p[0] > 200 && p[1] > 180 && p[2] < 60;
    let area = [40.0, 236.0, 230.0, 262.0];
    assert!(count(&mut ui, area, yellow) > 1000);
    // It multiplies with what is below, so that the text stays black.
    assert!(count(&mut ui, area, dark) > 100);

    viewer.undo();
    settle(&mut ui);
    assert_eq!(count(&mut ui, area, yellow), 0);
    assert!(viewer.annotations(0).is_empty());
}

#[test]
fn with_a_tool_selecting_is_marking() {
    let (mut ui, viewer) = open(text_page().build());
    viewer.set_tool(PdfTool::StrikeOut);
    drag(&mut ui, at(40.0, 248.0), at(240.0, 248.0));
    settle(&mut ui);
    assert_eq!(viewer.annotations(0)[0].1, PdfAnnotationKind::StrikeOut);
    assert_eq!(viewer.tool(), PdfTool::StrikeOut);
    let red = |p: [u8; 4]| p[0] > 180 && p[1] < 80 && p[2] < 80;
    assert!(count(&mut ui, [40.0, 236.0, 230.0, 262.0], red) > 50);
}

#[test]
fn a_saved_highlight_reads_back_and_can_be_removed() {
    let (mut ui, viewer) = open(text_page().build());
    drag(&mut ui, at(40.0, 248.0), at(240.0, 248.0));
    viewer.highlight_selection();
    settle(&mut ui);
    let file = saved(&mut ui, viewer);
    let text = String::from_utf8_lossy(&file);
    assert!(text.contains("/Subtype /Highlight"));
    assert!(text.contains("/QuadPoints"));

    let (mut ui, again) = open(file);
    let found = again.annotations(0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, PdfAnnotationKind::Highlight);
    let yellow = |p: [u8; 4]| p[0] > 200 && p[1] > 180 && p[2] < 60;
    let area = [40.0, 236.0, 230.0, 262.0];
    assert!(count(&mut ui, area, yellow) > 1000);
    // A click picks it, even though it is on text, and the key takes it away.
    click(&mut ui, at(100.0, 248.0));
    assert_eq!(again.selected_annotation(), Some(found[0].0));
    key(&mut ui, NamedKey::Delete);
    assert!(again.annotations(0).is_empty());
    assert_eq!(count(&mut ui, area, yellow), 0);
    let file = saved(&mut ui, again);
    let (_ui, third) = open(file);
    assert!(third.annotations(0).is_empty());
}

#[test]
fn a_line_can_be_drawn_moved_and_taken_away() {
    let (mut ui, viewer) = open(text_page().build());
    viewer.set_tool(PdfTool::Ink);
    viewer.set_ink_width(4.0);
    drag(&mut ui, at(40.0, 100.0), at(200.0, 100.0));
    let lines = viewer.annotations(0);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].1, PdfAnnotationKind::Ink);
    let red = |p: [u8; 4]| p[0] > 180 && p[1] < 80 && p[2] < 80;
    let old = [40.0, 90.0, 200.0, 110.0];
    assert!(count(&mut ui, old, red) > 100);

    viewer.set_tool(PdfTool::Select);
    ui.update();
    click(&mut ui, at(100.0, 100.0));
    assert_eq!(viewer.selected_annotation(), Some(lines[0].0));
    drag(&mut ui, at(100.0, 100.0), at(100.0, 40.0));
    settle(&mut ui);
    let new = [40.0, 30.0, 200.0, 50.0];
    assert_eq!(count(&mut ui, old, red), 0);
    assert!(count(&mut ui, new, red) > 100);
    control(&mut ui, "z");
    assert!(count(&mut ui, old, red) > 100);
    control(&mut ui, "y");
    assert!(count(&mut ui, new, red) > 100);

    click(&mut ui, at(100.0, 40.0));
    key(&mut ui, NamedKey::Delete);
    assert_eq!(count(&mut ui, new, red), 0);
    assert!(viewer.annotations(0).is_empty());
}

#[test]
fn text_can_be_put_on_a_page_found_and_changed() {
    let (mut ui, viewer) = open(text_page().build());
    viewer.set_tool(PdfTool::FreeText);
    click(&mut ui, at(40.0, 120.0));
    typed(&mut ui, "Remember");
    click(&mut ui, at(300.0, 20.0));
    assert_eq!(viewer.tool(), PdfTool::Select);
    let found = viewer.annotations(0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, PdfAnnotationKind::FreeText);
    assert_eq!(found[0].2, "Remember");
    let area = [40.0, 90.0, 200.0, 125.0];
    assert!(count(&mut ui, area, dark) > 30);

    viewer.search("remember");
    settle(&mut ui);
    assert_eq!(viewer.search_count(), 1);
    viewer.search("");
    settle(&mut ui);

    // Going to it twice opens it again.
    let time = TIME.get() + 10_000;
    press_at(&mut ui, at(60.0, 115.0), true, time);
    press_at(&mut ui, at(60.0, 115.0), false, time + 10);
    press_at(&mut ui, at(60.0, 115.0), true, time + 20);
    press_at(&mut ui, at(60.0, 115.0), false, time + 30);
    typed(&mut ui, "!");
    click(&mut ui, at(300.0, 20.0));
    assert_eq!(viewer.annotations(0)[0].2, "Remember!");
}

#[test]
fn a_note_opens_for_writing_and_keeps_what_was_written() {
    let (mut ui, viewer) = open(text_page().build());
    viewer.set_tool(PdfTool::Note);
    click(&mut ui, at(300.0, 100.0));
    assert_eq!(viewer.tool(), PdfTool::Select);
    let found = viewer.annotations(0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, PdfAnnotationKind::Note);
    assert_eq!(viewer.selected_annotation(), Some(found[0].0));
    typed(&mut ui, "Why?");
    click(&mut ui, at(40.0, 40.0));
    assert_eq!(viewer.annotations(0)[0].2, "Why?");
    let file = saved(&mut ui, viewer);
    let (_ui, again) = open(file);
    assert_eq!(again.annotations(0)[0].2, "Why?");
    assert_eq!(again.annotations(0)[0].1, PdfAnnotationKind::Note);
}

#[test]
fn text_that_no_font_of_the_document_has_gets_a_font_of_the_system() {
    let (mut ui, viewer) = open(form().build());
    viewer.set_field("name", PdfFieldValue::Text("日本語".to_owned()));
    settle(&mut ui);
    assert_eq!(viewer.edit_error(), None);
    let file = saved(&mut ui, viewer);
    let text = String::from_utf8_lossy(&file);
    if !text.contains("/FontFile") {
        eprintln!("no font with these characters on this system");
        return;
    }
    assert!(text.contains("/Identity-H"));
    assert!(text.contains("/ToUnicode"));
    assert!(count(&mut ui, NAME, dark) > 30);
    let (mut ui, again) = open(file);
    assert_eq!(text_of(again.field_value("name")), "日本語");
    assert!(count(&mut ui, NAME, dark) > 30);
}

#[test]
fn a_field_on_a_page_that_is_turned_is_where_it_shows() {
    let doc = Document {
        page_extra: format!("{} /Rotate 90", form().page_extra),
        ..form()
    };
    let (mut ui, viewer) = open(doc.build());
    // A quarter turn clockwise makes what is up in the document go to the right of the window. The
    // page is 400 pixels wide then, in the middle of 560.
    let shown = |x: f32, y: f32| PointF::new(80.0 + y * 4.0 / 3.0, x * 4.0 / 3.0);
    click(&mut ui, shown(30.0, 210.0));
    assert_eq!(
        viewer.field_value("agree"),
        Some(PdfFieldValue::State(Some("Yes".to_owned())))
    );
}

#[test]
fn a_field_in_a_view_that_is_turned_is_where_it_shows() {
    let (mut ui, viewer) = open(form().build());
    viewer.rotate_clockwise();
    settle(&mut ui);
    // Turning keeps what is in the middle there.
    viewer.go_to_page(0);
    settle(&mut ui);
    // What was at the bottom left is at the top left, on a page in the middle of the window.
    let shown = |x: f32, y: f32| PointF::new(80.0 + y * 4.0 / 3.0, x * 4.0 / 3.0);
    click(&mut ui, shown(30.0, 210.0));
    assert_eq!(
        viewer.field_value("agree"),
        Some(PdfFieldValue::State(Some("Yes".to_owned())))
    );
    click(&mut ui, shown(100.0, 255.0));
    typed(&mut ui, "Turned");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(text_of(viewer.field_value("name")), "Turned");
}

#[test]
fn what_is_found_stays_found_when_something_is_changed() {
    let (mut ui, viewer) = open(text_page().build());
    viewer.search("fox");
    settle(&mut ui);
    assert_eq!(viewer.search_count(), 1);
    assert_eq!(viewer.search_index(), Some(0));
    viewer.set_tool(PdfTool::Ink);
    drag(&mut ui, at(40.0, 100.0), at(200.0, 100.0));
    settle(&mut ui);
    assert_eq!(viewer.search_count(), 1);
    assert_eq!(viewer.search_index(), Some(0));
}

#[test]
fn a_form_can_be_put_back_as_it_was() {
    let (mut ui, viewer) = open(form().build());
    viewer.set_field("name", PdfFieldValue::Text("Gone".to_owned()));
    viewer.set_field("agree", PdfFieldValue::State(Some("Yes".to_owned())));
    settle(&mut ui);
    viewer.reset_fields();
    settle(&mut ui);
    assert_eq!(text_of(viewer.field_value("name")), "");
    assert_eq!(count(&mut ui, CHECK, dark), 0);
    // Going back brings it all back at once.
    viewer.undo();
    settle(&mut ui);
    assert_eq!(text_of(viewer.field_value("name")), "Gone");
    assert!(count(&mut ui, CHECK, dark) > 300);
}

#[test]
fn a_list_that_opens_picks_what_is_clicked() {
    let (mut ui, viewer) = open(form().build());
    click(&mut ui, at(100.0, 175.0));
    // The list is below the field, a line at a time.
    let below = |row: f32| {
        let p = at(40.0, 160.0);
        PointF::new(p.x, p.y + 12.0 + row * 22.0)
    };
    click(&mut ui, below(1.0));
    assert_eq!(
        viewer.field_value("color"),
        Some(PdfFieldValue::Choice(vec!["g".to_owned()]))
    );
    assert!(count(&mut ui, [20.0, 160.0, 200.0, 190.0], dark) > 10);
}

#[test]
fn a_highlight_on_a_page_that_is_turned_keeps_to_the_lines() {
    let doc = Document {
        page_extra: "/Rotate 90".to_owned(),
        ..text_page()
    };
    let (mut ui, viewer) = open(doc.build());
    viewer.select_all();
    settle(&mut ui);
    viewer.highlight_selection();
    settle(&mut ui);
    assert_eq!(viewer.edit_error(), None);
    // What is at y in the document is `y` from the left in the picture, and x is down it.
    let yellow = |p: [u8; 4]| p[0] > 200 && p[1] > 180 && p[2] < 60;
    assert!(count(&mut ui, [240.0, 60.0, 255.0, 240.0], yellow) > 300);
    assert_eq!(count(&mut ui, [222.0, 60.0, 234.0, 240.0], yellow), 0);
}

#[test]
fn a_field_that_was_changed_takes_xfa_out_of_the_form() {
    let mut doc = form();
    doc.catalog_extra = "/AcroForm 120 0 R".to_owned();
    doc.objects.push((
        120,
        "<< /Fields [100 0 R 101 0 R 102 0 R 105 0 R 106 0 R 107 0 R] /XFA 121 0 R \
         /DR << /Font << /Helv 3 0 R >> >> >>"
            .to_owned(),
    ));
    doc.objects
        .push((121, "<< /Length 5 >>\nstream\n<xdp>\nendstream".to_owned()));
    let bytes = doc.build();
    let (mut ui, viewer) = open(bytes.clone());
    viewer.set_field("name", PdfFieldValue::Text("Hello".to_owned()));
    settle(&mut ui);
    let file = saved(&mut ui, viewer);
    let added = String::from_utf8_lossy(&file[bytes.len()..]).into_owned();
    assert!(added.contains("120 0 obj"));
    assert!(added.contains("/Fields"));
    assert!(!added.contains("/XFA"));
}
