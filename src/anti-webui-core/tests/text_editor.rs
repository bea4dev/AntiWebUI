//! `text_editor`: lines as nodes of their own, decorations, and editing through `Ui`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeId, NodeKind, with_doc};
use std::rc::Rc;

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn click(ui: &mut Ui, p: PointF, time: u32) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time,
        });
    }
    ui.update();
}

fn key(ui: &mut Ui, key: Key, text: Option<&str>, modifiers: Modifiers) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: key.clone(),
            text: text.filter(|_| pressed).map(Into::into),
            modifiers,
            pressed,
            repeat: false,
        }));
    }
    ui.update();
}

fn typ(ui: &mut Ui, s: &str) {
    for c in s.chars() {
        let c = c.to_string();
        key(
            ui,
            Key::Character(c.clone()),
            Some(&c),
            Modifiers::default(),
        );
    }
}

fn named(ui: &mut Ui, k: NamedKey) {
    key(ui, Key::Named(k), None, Modifiers::default());
}

fn ctrl() -> Modifiers {
    Modifiers::PRIMARY
}

fn host() -> NodeId {
    with_doc(|d| d.editors[0])
}

/// What each row shows, and its text node.
fn rows() -> Vec<(String, Option<NodeId>)> {
    with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .map(|&div| {
                let text = d.nodes[div]
                    .children
                    .first()
                    .copied()
                    .filter(|&c| d.nodes[c].is_text());
                let shown = match text.map(|t| &d.nodes[t].kind) {
                    Some(NodeKind::Text(t)) => t.text.trim_end_matches(' ').to_owned(),
                    _ => "<block>".to_owned(),
                };
                (shown, text)
            })
            .collect()
    })
}

fn selection() -> (usize, usize) {
    let h = host();
    with_doc(|d| {
        let e = d.nodes[h].editor().unwrap();
        (e.anchor, e.focus)
    })
}

fn editor(source: Signal<String>) -> TextEditor {
    text_editor()
        .value(source)
        .w(500.0)
        .h(300.0)
        .overflow_y_scroll()
        .text_size(16.0)
}

#[test]
fn typing_goes_into_the_text_and_each_line_is_a_row() {
    let source = signal(String::new());
    let mut ui = mount(move || editor(source));
    click(&mut ui, PointF::new(20.0, 10.0), 0);
    typ(&mut ui, "abc");
    named(&mut ui, NamedKey::Enter);
    typ(&mut ui, "def");
    assert_eq!(source.get_untracked(), "abc\ndef");
    let r = rows();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].0, "abc");
    assert_eq!(r[1].0, "def");
    key(&mut ui, Key::Character("z".into()), None, ctrl());
    assert_eq!(source.get_untracked(), "abc\n");
    key(&mut ui, Key::Character("z".into()), None, ctrl());
    assert_eq!(source.get_untracked(), "abc");
    assert_eq!(rows().len(), 1);
}

#[test]
fn a_new_text_from_the_signal_shows() {
    let source = signal("one\ntwo".to_owned());
    let mut ui = mount(move || editor(source));
    assert_eq!(rows().len(), 2);
    source.set("one\nzwei\ndrei".into());
    ui.update();
    let r = rows();
    assert_eq!(
        r.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["one", "zwei", "drei"]
    );
}

#[test]
fn up_and_down_keep_to_where_they_started_across() {
    let source = signal("abcdefgh\nab\nabcdefgh".to_owned());
    let mut ui = mount(move || editor(source));
    click(&mut ui, PointF::new(1.0, 10.0), 0);
    for _ in 0..6 {
        named(&mut ui, NamedKey::ArrowRight);
    }
    assert_eq!(selection(), (6, 6));
    named(&mut ui, NamedKey::ArrowDown);
    // The short line ends before.
    assert_eq!(selection().1, 11);
    named(&mut ui, NamedKey::ArrowDown);
    assert_eq!(selection().1, 12 + 6);
    named(&mut ui, NamedKey::ArrowUp);
    named(&mut ui, NamedKey::ArrowUp);
    assert_eq!(selection().1, 6);
    named(&mut ui, NamedKey::End);
    assert_eq!(selection().1, 8);
    named(&mut ui, NamedKey::Home);
    assert_eq!(selection().1, 0);
}

/// Hides `**` unless the caret is in the line, and makes the rest bold.
fn bold_marks(cx: &DecorateContext) -> Vec<Row> {
    let caret_line = cx.line_of(cx.focus);
    let bold = Rc::new(Style::default().bold());
    (0..cx.line_count())
        .map(|i| {
            let r = cx.line(i);
            let line = &cx.text[r];
            let mut look = LineLook::default();
            if let Some(a) = line.find("**")
                && let Some(len) = line[a + 2..].find("**")
            {
                let b = a + 2 + len;
                look.marks.push((a + 2..b, bold.clone()));
                if i != caret_line {
                    look.replace.push((a..a + 2, None));
                    look.replace.push((b..b + 2, None));
                }
            }
            Row::Line(Rc::new(look))
        })
        .collect()
}

#[test]
fn marks_hide_until_the_caret_comes_to_them() {
    let source = signal("plain\nsome **bold** text".to_owned());
    let mut ui = mount(move || editor(source).decorate(bold_marks));
    click(&mut ui, PointF::new(1.0, 10.0), 0);
    assert_eq!(rows()[1].0, "some bold text");
    named(&mut ui, NamedKey::ArrowDown);
    assert_eq!(rows()[1].0, "some **bold** text");
    named(&mut ui, NamedKey::ArrowUp);
    assert_eq!(rows()[1].0, "some bold text");
    // The first row was only shown again, not made again.
    let first = rows()[0].1;
    named(&mut ui, NamedKey::ArrowDown);
    assert_eq!(rows()[0].1, first);
}

#[test]
fn a_click_on_what_is_shown_goes_to_its_place_in_the_text() {
    let source = signal("x\n**bold** end".to_owned());
    let mut ui = mount(move || editor(source).decorate(bold_marks));
    // Far right on the second line: its end, past the hidden marks.
    let h = host();
    let y = with_doc(|d| {
        let div = d.nodes[h].children[1];
        let r = d.window_rect(div).unwrap();
        (r.y + r.height / 2) as f32
    });
    click(&mut ui, PointF::new(480.0, y), 0);
    assert_eq!(selection(), (14, 14));
    // Now the marks show; at the very left is the start of the line.
    click(&mut ui, PointF::new(1.0, y), 1000);
    assert_eq!(selection(), (2, 2));
}

#[test]
fn the_pre_edit_shows_in_its_line_and_commits_into_the_text() {
    let source = signal("ab\ncd".to_owned());
    let mut ui = mount(move || editor(source));
    click(&mut ui, PointF::new(1.0, 10.0), 0);
    named(&mut ui, NamedKey::ArrowRight);
    ui.handle_event(WindowEvent::Ime(ImeUpdate {
        preedit: Some(("にほ".into(), None)),
        commit: None,
        delete_surrounding: None,
    }));
    ui.update();
    assert_eq!(rows()[0].0, "aにほb");
    assert_eq!(source.get_untracked(), "ab\ncd");
    ui.handle_event(WindowEvent::Ime(ImeUpdate {
        preedit: None,
        commit: Some("日本".into()),
        delete_surrounding: None,
    }));
    ui.update();
    assert_eq!(rows()[0].0, "a日本b");
    assert_eq!(source.get_untracked(), "a日本b\ncd");
}

#[test]
fn some_lines_can_show_as_one_node() {
    let source = signal("a\nb\nc\nd".to_owned());
    let _ui = mount(move || {
        editor(source).decorate(|cx| {
            let plain = Rc::new(LineLook::default());
            let mut rows = vec![Row::Line(plain.clone())];
            rows.push(Row::Block {
                lines: 2,
                style: None,
                widget: Widget::new(7, || div().h(40.0)),
            });
            rows.extend((3..cx.line_count()).map(|_| Row::Line(plain.clone())));
            rows
        })
    });
    let r = rows();
    assert_eq!(
        r.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["a", "<block>", "d"]
    );
}

#[test]
fn selecting_across_lines_marks_each_of_them() {
    let source = signal("one\ntwo\nthree".to_owned());
    let mut ui = mount(move || editor(source));
    click(&mut ui, PointF::new(1.0, 10.0), 0);
    named(&mut ui, NamedKey::ArrowRight);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    key(&mut ui, Key::Named(NamedKey::ArrowDown), None, shift);
    key(&mut ui, Key::Named(NamedKey::ArrowDown), None, shift);
    let r = rows();
    let marked: Vec<Option<std::ops::Range<usize>>> = with_doc(|d| {
        r.iter()
            .map(|(_, t)| match &d.nodes[t.unwrap()].kind {
                NodeKind::Text(t) => t.row.as_ref().unwrap().selected.clone(),
                _ => None,
            })
            .collect()
    });
    // The first and the second include their breaks; the last ends at the caret.
    assert_eq!(marked, [Some(1..4), Some(0..4), Some(0..1)]);
    key(&mut ui, Key::Character("x".into()), None, ctrl());
    assert_eq!(source.get_untracked(), "ohree");
}

#[test]
fn a_keystroke_in_a_long_text_is_quick_and_touches_one_row() {
    let text: String = (0..5000)
        .map(|i| format!("line {i} with some words in it\n"))
        .collect();
    let source = signal(text);
    let started = std::time::Instant::now();
    let mut ui = mount(move || editor(source));
    let built = started.elapsed();
    click(&mut ui, PointF::new(1.0, 10.0), 0);
    let before = rows();
    let started = std::time::Instant::now();
    typ(&mut ui, "x");
    let typed = started.elapsed();
    let after = rows();
    let changed = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| a.0 != b.0 || a.1 != b.1)
        .count();
    assert_eq!(changed, 1);
    eprintln!("5000 lines: built in {built:?}, a keystroke in {typed:?}");
}

fn button(ui: &mut Ui, p: PointF, pressed: bool, time: u32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time,
    });
    ui.update();
}

fn move_to(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
    ui.update();
}

/// Presses at `from`, goes to `to` and lets go there.
fn drag(ui: &mut Ui, from: PointF, to: PointF, time: u32) {
    button(ui, from, true, time);
    for i in 1..=4 {
        let t = i as f32 / 4.0;
        move_to(
            ui,
            PointF::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t),
        );
    }
    button(ui, to, false, time + 100);
}

/// A point in the window on the text of row `i`, `x` from its left.
fn on_row(i: usize, x: f32) -> PointF {
    let h = host();
    with_doc(|d| {
        let div = d.nodes[h].children[i];
        let r = d.window_rect(div).unwrap();
        PointF::new(r.x as f32 + x, (r.y + r.height / 2) as f32)
    })
}

fn select_first_word(ui: &mut Ui) {
    click(ui, on_row(0, 1.0), 0);
    let both = Modifiers::WORD.with_shift();
    key(ui, Key::Named(NamedKey::ArrowRight), None, both);
}

#[test]
fn selected_text_can_be_moved_in_one_step_of_undo() {
    let source = signal("alpha beta\ngamma".to_owned());
    let mut ui = mount(move || editor(source));
    select_first_word(&mut ui);
    assert_eq!(selection(), (0, 5));
    drag(&mut ui, on_row(0, 10.0), on_row(1, 400.0), 10_000);
    assert_eq!(source.get_untracked(), " beta\ngammaalpha");
    let (a, b) = selection();
    assert_eq!(&source.get_untracked()[a.min(b)..a.max(b)], "alpha");
    key(&mut ui, Key::Character("z".into()), None, ctrl());
    assert_eq!(source.get_untracked(), "alpha beta\ngamma");
}

#[test]
fn a_click_on_the_selection_puts_the_caret_there() {
    let source = signal("alpha beta".to_owned());
    let mut ui = mount(move || editor(source));
    select_first_word(&mut ui);
    let p = on_row(0, 10.0);
    button(&mut ui, p, true, 10_000);
    button(&mut ui, p, false, 10_050);
    let (a, b) = selection();
    assert_eq!(a, b);
    assert!(a > 0 && a < 5);
    assert_eq!(source.get_untracked(), "alpha beta");
}

#[test]
fn text_goes_between_an_editor_and_an_input() {
    let source = signal("alpha beta".to_owned());
    let field = signal(String::from("xyz"));
    let mut ui = mount(move || {
        div()
            .flex()
            .flex_col()
            .child(editor(source).h(100.0))
            .child(text_input().value(field).w(300.0).h(40.0))
    });
    select_first_word(&mut ui);
    let input = with_doc(|d| d.edits[0]);
    let end_of_input = with_doc(|d| {
        let r = d.window_rect(input).unwrap();
        PointF::new((r.right() - 4) as f32, (r.y + r.height / 2) as f32)
    });
    // Out of the editor: it moves.
    drag(&mut ui, on_row(0, 10.0), end_of_input, 10_000);
    assert_eq!(field.get_untracked(), "xyzalpha");
    assert_eq!(source.get_untracked(), " beta");
    // And back: it is selected in the input now, after `xyz`.
    let on_alpha = with_doc(|d| {
        let r = d.window_rect(input).unwrap();
        PointF::new(r.x as f32 + 50.0, (r.y + r.height / 2) as f32)
    });
    drag(&mut ui, on_alpha, on_row(0, 400.0), 20_000);
    assert_eq!(source.get_untracked(), " betaalpha");
    assert_eq!(field.get_untracked(), "xyz");
}

#[test]
fn rows_shown_as_one_node_are_marked_when_selected() {
    let source = signal("a\nb\nc\nd".to_owned());
    let mut ui = mount(move || {
        editor(source).decorate(|cx| {
            let plain = Rc::new(LineLook::default());
            let mut rows = vec![Row::Line(plain.clone())];
            rows.push(Row::Block {
                lines: 2,
                style: None,
                widget: Widget::new(7, || div().h(40.0)),
            });
            rows.extend((3..cx.line_count()).map(|_| Row::Line(plain.clone())));
            rows
        })
    });
    let marked = || {
        let h = host();
        with_doc(|d| d.nodes[h].editor().unwrap().selected_blocks().len())
    };
    click(&mut ui, on_row(0, 1.0), 0);
    assert_eq!(marked(), 0);
    key(&mut ui, Key::Character("a".into()), None, ctrl());
    assert_eq!(marked(), 1);
}
