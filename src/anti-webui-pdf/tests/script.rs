//! The JavaScript of forms: typing, committing, working out and showing values, buttons, links,
//! dialogs and timers, on documents made here. See "JavaScript" in `docs/design/26-pdf.md`.
#![cfg(feature = "js")]

use anti_webui_core::Ui;
use anti_webui_core::event::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_pdf::{
    PdfAnswer, PdfButtons, PdfDialogKind, PdfFieldChange, PdfFieldValue, PdfView, PdfViewer,
    PdfZoom, pdf_view, pdf_viewer,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A literal string of a PDF.
fn string(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        if matches!(c, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(')');
    out
}

fn action(code: &str) -> String {
    format!("<< /S /JavaScript /JS {} >>", string(code))
}

/// A page of 400 by 300 points with what is on it, and a script that runs when it opens.
struct Document {
    fields: Vec<String>,
    /// Annotations that aren't fields.
    others: Vec<String>,
    order: Vec<usize>,
    opening: Vec<&'static str>,
}

impl Document {
    fn build(&self) -> Vec<u8> {
        let mut objects: Vec<(usize, String)> = Vec::new();
        let mut widgets = Vec::new();
        for (i, field) in self.fields.iter().enumerate() {
            objects.push((100 + i, field.clone()));
            widgets.push(format!("{} 0 R", 100 + i));
        }
        let mut annots = widgets.clone();
        for (i, other) in self.others.iter().enumerate() {
            objects.push((200 + i, other.clone()));
            annots.push(format!("{} 0 R", 200 + i));
        }
        let names: Vec<String> = self
            .opening
            .iter()
            .enumerate()
            .map(|(i, code)| format!("(s{i}) {}", action(code)))
            .collect();
        let order: Vec<String> = self
            .order
            .iter()
            .map(|i| format!("{} 0 R", 100 + i))
            .collect();
        let mut all: Vec<(usize, String)> = vec![
            (
                1,
                format!(
                    "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] /CO [{}] \
                     /DR << /Font << /Helv 3 0 R >> >> /DA (/Helv 0 Tf 0 g) >> \
                     /Names << /JavaScript << /Names [{}] >> >> >>",
                    widgets.join(" "),
                    order.join(" "),
                    names.join(" ")
                ),
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
                     /Resources << /Font << /F1 3 0 R >> >> /Annots [{}] >>",
                    annots.join(" ")
                ),
            ),
            (11, "<< /Length 0 >>\nstream\n\nendstream".to_owned()),
        ];
        all.extend(objects);
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (id, body) in &all {
            offsets.push((*id, out.len()));
            out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
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

fn text_field(name: &str, rect: [f32; 4], more: &str) -> String {
    format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [{} {} {} {}] \
         /DA (/Helv 12 Tf 0 g) /F 4 {more} >>",
        rect[0], rect[1], rect[2], rect[3]
    )
}

const A: [f32; 4] = [20.0, 240.0, 120.0, 270.0];
const B: [f32; 4] = [140.0, 240.0, 240.0, 270.0];
const TOTAL: [f32; 4] = [260.0, 240.0, 380.0, 270.0];
const PERCENT: [f32; 4] = [20.0, 190.0, 120.0, 220.0];
const HIDE: [f32; 4] = [140.0, 190.0, 240.0, 220.0];
const OUT: [f32; 4] = [260.0, 190.0, 380.0, 220.0];
const LINK: [f32; 4] = [20.0, 140.0, 120.0, 170.0];
const UPPER: [f32; 4] = [140.0, 140.0, 240.0, 170.0];
const NOTE: [f32; 4] = [260.0, 140.0, 380.0, 170.0];

/// Two amounts and their total in dollars, a percentage, a button that hides it, a field the
/// scripts write to, a link that runs a script, a field that takes capitals, and one that says what
/// checking the total saw.
fn invoice() -> Document {
    let money = format!(
        "/AA << /K {} /F {} >>",
        action(r#"AFNumber_Keystroke(2, 0, 0, 0, "$", true);"#),
        action(r#"AFNumber_Format(2, 0, 0, 0, "$", true);"#)
    );
    Document {
        fields: vec![
            text_field("a", A, &money),
            text_field("b", B, &money),
            text_field(
                "total",
                TOTAL,
                &format!(
                    "/Ff 1 /AA << /C {} /F {} /V {} >>",
                    action(r#"AFSimple_Calculate("SUM", new Array("a", "b"));"#),
                    action(r#"AFNumber_Format(2, 0, 0, 0, "$", true);"#),
                    action(r#"this.getField("note").value = "checked " + event.value;"#)
                ),
            ),
            text_field(
                "percent",
                PERCENT,
                &format!(
                    "/AA << /V {} >>",
                    action("AFRange_Validate(true, 0, true, 100);")
                ),
            ),
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (hide) \
                 /Rect [{} {} {} {}] /F 4 /A {} >>",
                HIDE[0],
                HIDE[1],
                HIDE[2],
                HIDE[3],
                action(r#"this.getField("percent").display = display.hidden;"#)
            ),
            text_field("out", OUT, ""),
            text_field(
                "upper",
                UPPER,
                &format!(
                    "/AA << /K {} >>",
                    action("event.change = event.change.toUpperCase();")
                ),
            ),
            text_field("note", NOTE, ""),
        ],
        others: vec![format!(
            "<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /A {} >>",
            LINK[0],
            LINK[1],
            LINK[2],
            LINK[3],
            action(r#"app.launchURL("https://example.com/js");"#)
        )],
        order: vec![2],
        opening: vec![r#"this.getField("out").value = "opened";"#],
    }
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

fn open(bytes: Vec<u8>) -> (Ui, PdfViewer) {
    open_with(bytes, |view| view)
}

fn settle(ui: &mut Ui) {
    for _ in 0..8 {
        ui.update();
        ui.wait_for_images(Duration::from_secs(30));
    }
}

/// Updates until something is so, or a second has gone.
fn until(ui: &mut Ui, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        settle(ui);
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Where the middle of a box of the page is in the window, with 4/3 of a pixel to a point.
fn middle(r: [f32; 4]) -> PointF {
    let (x, y) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    PointF::new(x * 4.0 / 3.0, (300.0 - y) * 4.0 / 3.0)
}

thread_local! {
    static TIME: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn click(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    settle(ui);
    for pressed in [true, false] {
        let time = TIME.get() + 10_000;
        TIME.set(time);
        ui.handle_event(WindowEvent::PointerButton {
            button: PointerButton::Primary,
            pressed,
            position: p,
            modifiers: Modifiers::default(),
            time,
        });
        settle(ui);
    }
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
        ui.update();
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

fn fill(ui: &mut Ui, field: [f32; 4], text: &str) {
    click(ui, middle(field));
    typed(ui, text);
    key(ui, NamedKey::Enter);
}

fn value(viewer: PdfViewer, name: &str) -> String {
    match viewer.field_value(name) {
        Some(PdfFieldValue::Text(text)) => text,
        other => panic!("{other:?}"),
    }
}

fn shown(viewer: PdfViewer, name: &str) -> String {
    viewer.field_text(name).expect("there is such a field")
}

#[test]
fn the_scripts_of_the_document_run_when_it_opens() {
    let (mut ui, viewer) = open(invoice().build());
    assert!(viewer.has_scripts());
    assert!(until(&mut ui, || value(viewer, "out") == "opened"));
    // What the document did by itself isn't something to undo.
    assert!(!viewer.can_undo());
}

#[test]
fn committing_works_out_the_total_and_shows_it_as_money() {
    let changes = Rc::new(RefCell::new(Vec::<PdfFieldChange>::new()));
    let seen = changes.clone();
    let (mut ui, viewer) = open_with(invoice().build(), move |view| {
        view.on_field_change(move |c| seen.borrow_mut().push(c.clone()))
    });
    fill(&mut ui, A, "1234.5");
    assert_eq!(value(viewer, "a"), "1234.5");
    assert_eq!(shown(viewer, "a"), "$1,234.50");
    assert_eq!(value(viewer, "total"), "1234.5");
    assert_eq!(shown(viewer, "total"), "$1,234.50");

    // What was worked out was checked too.
    assert_eq!(value(viewer, "note"), "checked 1234.5");

    fill(&mut ui, B, "0.25");
    assert_eq!(value(viewer, "total"), "1234.75");
    assert_eq!(shown(viewer, "total"), "$1,234.75");
    // Only what the person changed counts as a change.
    let names: Vec<String> = changes.borrow().iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["a", "b"]);

    // Going back takes back what was worked out with it.
    control(&mut ui, "z");
    assert_eq!(value(viewer, "b"), "");
    assert_eq!(value(viewer, "total"), "1234.5");
    assert_eq!(shown(viewer, "total"), "$1,234.50");
}

#[test]
fn a_number_field_takes_no_letters() {
    let (mut ui, viewer) = open(invoice().build());
    fill(&mut ui, A, "1x2.5y");
    assert_eq!(value(viewer, "a"), "12.5");
    assert_eq!(value(viewer, "total"), "12.5");
}

#[test]
fn a_script_can_change_what_is_typed() {
    let (mut ui, viewer) = open(invoice().build());
    fill(&mut ui, UPPER, "abc");
    assert_eq!(value(viewer, "upper"), "ABC");
}

#[test]
fn setting_a_field_from_the_application_works_out_the_rest() {
    let (mut ui, viewer) = open(invoice().build());
    viewer.set_field("a", PdfFieldValue::Text("7".into()));
    assert!(until(&mut ui, || value(viewer, "total") == "7"));
    assert_eq!(shown(viewer, "a"), "$7.00");
}

#[test]
fn a_value_out_of_range_asks_and_is_refused() {
    let (mut ui, viewer) = open(invoice().build());
    fill(&mut ui, PERCENT, "200");
    assert!(until(&mut ui, || viewer.dialog().is_some()));
    let dialog = viewer.dialog().unwrap();
    assert_eq!(dialog.kind, PdfDialogKind::Alert(PdfButtons::Ok));
    assert!(
        dialog.message.contains("less than or equal to 100"),
        "{}",
        dialog.message
    );
    viewer.answer_dialog(PdfAnswer::Ok);
    assert!(until(&mut ui, || value(viewer, "percent").is_empty()));
    assert_eq!(viewer.dialog(), None);

    fill(&mut ui, PERCENT, "50");
    assert_eq!(value(viewer, "percent"), "50");
}

#[test]
fn a_button_hides_a_field() {
    let (mut ui, viewer) = open(invoice().build());
    click(&mut ui, middle(HIDE));
    // A field that is hidden doesn't take a click.
    fill(&mut ui, PERCENT, "5");
    assert_eq!(value(viewer, "percent"), "");
    // It is hidden in the file too.
    let dir = std::env::temp_dir().join(format!("awui-pdf-js-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hidden.pdf");
    viewer.save(&path);
    settle(&mut ui);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    let (_, added) = text.split_once("%%EOF").unwrap();
    assert!(added.contains("/F 6"), "{added}");
}

#[test]
fn a_link_runs_its_script() {
    let opened = Rc::new(RefCell::new(Vec::<String>::new()));
    let seen = opened.clone();
    let (mut ui, _viewer) = open_with(invoice().build(), move |view| {
        view.on_link(move |url| seen.borrow_mut().push(url.to_owned()))
    });
    click(&mut ui, middle(LINK));
    assert!(until(&mut ui, || !opened.borrow().is_empty()));
    assert_eq!(*opened.borrow(), ["https://example.com/js"]);
}

#[test]
fn the_functions_of_acrobat_give_what_acrobat_does() {
    let checks = [
        r#"util.printf("%,0.2f|%d|%05.1f|%s|%,2.2f", 1234.567, 42.9, 3.14159, "x", 1234.5)"#,
        r#"util.printd("yyyy-mm-dd HH:MM", util.scand("mm/dd/yyyy h:MM tt", "02/29/2024 3:05 pm"))"#,
        r#"String(util.scand("mm/dd/yyyy", "02/30/2024"))"#,
        r#"util.printx("(999) 999-9999", "555 123 4567")"#,
        r#"util.printd("dddd, mmmm d", new Date(2024, 2, 5))"#,
        r#"(function () { event.value = "1234.5"; AFNumber_Format(2, 0, 0, 0, "\u20ac", false); return event.value; })()"#,
        r#"(function () { event.value = -5; AFNumber_Format(2, 0, 2, 0, "$", true); return event.value; })()"#,
        r#"(function () { event.value = "0.256"; AFPercent_Format(1, 0); return event.value; })()"#,
        r#"(function () { event.value = "2024-03-05"; AFDate_FormatEx("mmm d, yyyy"); return event.value; })()"#,
        r#"(function () { event.value = "5551234567"; AFSpecial_Format(2); return event.value; })()"#,
        r#"String(AFMakeNumber("1.234,5")) + " " + String(AFMakeNumber("abc"))"#,
        r#"typeof this.getField("a").value + " " + this.getField("a").value"#,
    ];
    let code = format!(
        "event.target = this.getField(\"out\"); this.getField(\"out\").value = [{}].join(\"\\n\");",
        checks.join(", ")
    );
    let mut document = invoice();
    document.fields[0] = text_field("a", A, "/V (12)");
    document.opening = vec![Box::leak(code.into_boxed_str())];
    let (mut ui, viewer) = open(document.build());
    assert!(until(&mut ui, || !value(viewer, "out").is_empty()));
    let got = value(viewer, "out");
    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(
        lines,
        [
            "1,234.57|42|003.1|x|1.234,50",
            "2024-02-29 15:05",
            "null",
            "(555) 123-4567",
            "Tuesday, March 5",
            "1,234.50€",
            "($5.00)",
            "25.6%",
            "Mar 5, 2024",
            "(555) 123-4567",
            "1234.5 null",
            "number 12",
        ]
    );
}

#[test]
fn a_timer_runs_later() {
    let mut document = invoice();
    document.opening = vec![r#"app.setTimeOut('this.getField("out").value = "later";', 20);"#];
    let (mut ui, viewer) = open(document.build());
    assert_eq!(value(viewer, "out"), "");
    assert!(until(&mut ui, || value(viewer, "out") == "later"));
}

#[test]
fn a_script_that_never_ends_is_stopped_and_the_rest_still_works() {
    let mut document = invoice();
    document.fields[5] = text_field(
        "out",
        OUT,
        &format!("/AA << /K {} >>", action("while (true) {}")),
    );
    let (mut ui, viewer) = open(document.build());
    click(&mut ui, middle(OUT));
    // The UI doesn't wait for it, neither for the key nor for committing.
    let start = Instant::now();
    for k in [Key::Character("x".into()), Key::Named(NamedKey::Enter)] {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            text: matches!(k, Key::Character(_)).then(|| "x".to_owned()),
            key: k,
            modifiers: Modifiers::default(),
            pressed: true,
            repeat: false,
        }));
        ui.update();
    }
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
    // Each of the two runs until it is stopped.
    std::thread::sleep(Duration::from_millis(4500));
    settle(&mut ui);
    fill(&mut ui, A, "3");
    assert!(until(&mut ui, || value(viewer, "total") == "3"));
}

#[test]
fn a_question_gets_what_was_written() {
    let mut document = invoice();
    document.fields[4] = format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (hide) \
         /Rect [{} {} {} {}] /F 4 /A {} >>",
        HIDE[0],
        HIDE[1],
        HIDE[2],
        HIDE[3],
        action(
            r#"var n = app.response("Your name?", "Name", "nobody"); this.getField("out").value = "hello " + n;"#
        )
    );
    let (mut ui, viewer) = open(document.build());
    click(&mut ui, middle(HIDE));
    assert!(until(&mut ui, || viewer.dialog().is_some()));
    assert_eq!(
        viewer.dialog().unwrap().kind,
        PdfDialogKind::Question {
            default: "nobody".into(),
            password: false
        }
    );
    viewer.answer_dialog(PdfAnswer::Text("Ada".into()));
    assert!(until(&mut ui, || value(viewer, "out") == "hello Ada"));
}

#[test]
fn a_field_that_gets_its_look_when_opening_shows_as_its_script_says() {
    let mut document = invoice();
    document.fields[0] = text_field(
        "a",
        A,
        &format!(
            "/V (1234.5) /AA << /F {} >>",
            action(r#"AFNumber_Format(2, 0, 0, 0, "$", true);"#)
        ),
    );
    document.opening.clear();
    let (mut ui, viewer) = open(document.build());
    assert!(until(&mut ui, || shown(viewer, "a") == "$1,234.50"));
    assert_eq!(value(viewer, "a"), "1234.5");
    // That isn't a change of the document.
    assert!(!viewer.is_modified());
    assert!(!viewer.can_undo());
}

#[test]
fn a_script_can_send_the_typing_on_to_another_field() {
    let mut document = invoice();
    document.fields[0] = text_field(
        "a",
        A,
        &format!(
            "/AA << /V {} >>",
            action(r#"this.getField("b").setFocus();"#)
        ),
    );
    let (mut ui, viewer) = open(document.build());
    fill(&mut ui, A, "5");
    typed(&mut ui, "7");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(value(viewer, "a"), "5");
    assert_eq!(value(viewer, "b"), "7");
}
