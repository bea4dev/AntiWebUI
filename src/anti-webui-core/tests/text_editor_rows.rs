//! `text_editor` with long texts: only the rows around the view have nodes, the others are gaps of
//! their height. See "見えている行だけ作る" in `docs/design/31-code-editor.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeKind, with_doc};
use std::cell::RefCell;
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

fn key_with(ui: &mut Ui, key: Key, text: Option<&str>, modifiers: Modifiers) {
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
        key_with(
            ui,
            Key::Character(c.clone()),
            Some(&c),
            Modifiers::default(),
        );
    }
}

fn click(ui: &mut Ui, p: PointF) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    ui.update();
}

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

/// An editor 300 high in a 600 × 400 window, scrolling itself.
fn editor(text: String, scroller: NodeRef) -> TextEditor {
    text_editor()
        .text(text)
        .node_ref(scroller)
        .w(500.0)
        .h(300.0)
        .overflow_y_scroll()
        .text_size(16.0)
}

/// The editor's children: rows (with children) and gaps (without), with their heights.
fn children() -> Vec<(bool, i32)> {
    with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .map(|&c| (!d.nodes[c].children.is_empty(), d.nodes[c].rect.height))
            .collect()
    })
}

/// The line under a point in the window.
fn line_at(r: EditorRef, p: PointF) -> usize {
    let at = r.offset_at(p).unwrap();
    r.with(|e| e.line_of(at)).unwrap()
}

#[test]
fn only_the_rows_near_the_view_have_nodes() {
    let scroller = node_ref();
    let _ui = mount(move || editor(lines(5000), scroller));
    let kids = children();
    let rows: Vec<i32> = kids.iter().filter(|k| k.0).map(|k| k.1).collect();
    assert!(rows.len() < 100, "{} rows have nodes", rows.len());
    // The gaps stand for the other rows, a line each (the last line is the empty one after the
    // last break).
    let line = rows[0];
    let all: i32 = kids.iter().map(|k| k.1).sum();
    assert_eq!(all, 5001 * line);
}

#[test]
fn scrolling_far_gives_the_rows_there_nodes() {
    let scroller = node_ref();
    let r = editor_ref();
    let mut ui = mount(move || editor(lines(5000), scroller).editor_ref(r));
    let line = children()[0].1;
    scroller.scroll_to(0.0, 2500.0 * line as f32 / 1.0);
    ui.update();
    let top = scroller.rect().unwrap();
    assert_eq!(
        line_at(r, PointF::new(top.x as f32 + 20.0, top.y as f32 + 1.0)),
        2500
    );
    // Not the rows at the start any more, but for the one with the caret.
    let kids = children();
    assert!(kids[0].0 && !kids[1].0);
    assert!(children().iter().filter(|k| k.0).count() < 100);
}

/// Long lines that wrap, of lengths the estimates don't know: coming back up, the rows above take
/// their real heights and what shows stays where it was.
#[test]
fn what_shows_does_not_jump_when_rows_above_get_nodes() {
    let text: String = (0..2000)
        .map(|i| format!("{i} {}\n", "word ".repeat(i * 7 % 40)))
        .collect();
    let scroller = node_ref();
    let r = editor_ref();
    let mut ui = mount(move || editor(text, scroller).editor_ref(r));
    scroller.scroll_to(0.0, 30000.0);
    ui.update();
    let top = scroller.rect().unwrap();
    let probe = PointF::new(top.x as f32 + 5.0, top.y as f32 + 150.0);
    let shown = line_at(r, probe);
    let at = scroller.scroll_offset().y;
    scroller.scroll_to(0.0, at - 120.0);
    ui.update();
    // The line that was in the middle moved down by what was scrolled.
    let p = PointF::new(probe.x, probe.y + 120.0);
    assert_eq!(line_at(r, p), shown);
}

#[test]
fn the_line_with_the_caret_has_its_row_however_far() {
    let scroller = node_ref();
    let r = editor_ref();
    let mut ui = mount(move || editor(lines(5000), scroller).editor_ref(r));
    click(&mut ui, PointF::new(5.0, 5.0));
    key_with(&mut ui, Key::Named(NamedKey::End), None, Modifiers::WORD);
    typ(&mut ui, "x");
    assert!(r.text().ends_with("line 4999\nx"));
    // It scrolled to the end, where the rows are.
    let top = scroller.rect().unwrap();
    let bottom = PointF::new(top.x as f32 + 5.0, (top.y + top.height) as f32 - 3.0);
    assert_eq!(line_at(r, bottom), 5000);
}

#[test]
fn lines_are_decorated_only_when_they_get_rows() {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let seen = asked.clone();
    let scroller = node_ref();
    let mut ui = mount(move || {
        editor(lines(5000), scroller).decorate_lines(move |_, range| {
            seen.borrow_mut().push(range.clone());
            range.map(|_| Rc::new(LineLook::default())).collect()
        })
    });
    let first: usize = asked.borrow().iter().map(|r| r.len()).sum();
    assert!(first < 100, "{first} lines decorated");
    asked.borrow_mut().clear();
    click(&mut ui, PointF::new(5.0, 5.0));
    typ(&mut ui, "x");
    let typed: usize = asked.borrow().iter().map(|r| r.len()).sum();
    assert!(
        typed < 100 * 3,
        "{typed} lines decorated for a click and a key"
    );
}

#[test]
fn edits_are_told_with_their_lines() {
    let told = Rc::new(RefCell::new(Vec::new()));
    let seen = told.clone();
    let mut ui = mount(move || {
        text_editor()
            .text("ab\ncdef\ng")
            .w(300.0)
            .text_size(16.0)
            .on_edit(move |e| seen.borrow_mut().push(*e))
    });
    let r = with_doc(|d| d.nodes[d.editors[0]].abs);
    click(&mut ui, PointF::new(r.x as f32 + 1.0, r.y as f32 + 1.0));
    key_with(
        &mut ui,
        Key::Named(NamedKey::ArrowDown),
        None,
        Modifiers::default(),
    );
    key_with(
        &mut ui,
        Key::Named(NamedKey::End),
        None,
        Modifiers::default(),
    );
    key_with(
        &mut ui,
        Key::Named(NamedKey::Enter),
        Some("\r"),
        Modifiers::default(),
    );
    let e = told.borrow()[0];
    assert_eq!((e.start, e.old_end, e.new_end), (7, 7, 8));
    assert_eq!(e.start_point, (1, 4));
    assert_eq!(e.old_end_point, (1, 4));
    assert_eq!(e.new_end_point, (2, 0));
}

/// The number of each row with nodes, from its line number's text.
fn numbers() -> Vec<String> {
    with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .filter_map(|&c| {
                let g = *d.nodes[c].children.last()?;
                let t = *d.nodes[g].children.first()?;
                match &d.nodes[t].kind {
                    NodeKind::Text(t) => Some(t.text.clone()),
                    _ => None,
                }
            })
            .collect()
    })
}

#[test]
fn line_numbers_follow_the_lines() {
    let mut ui = mount(move || {
        text_editor()
            .text("a\nb\nc")
            .w(300.0)
            .text_size(16.0)
            .gutter(Gutter {
                digits: 2,
                ..Gutter::default()
            })
    });
    assert_eq!(numbers(), ["1", "2", "3"]);
    let r = with_doc(|d| d.nodes[d.editors[0]].abs);
    // On the line number: the start of its line.
    click(&mut ui, PointF::new(r.x as f32 + 2.0, r.y as f32 + 2.0));
    key_with(
        &mut ui,
        Key::Named(NamedKey::Enter),
        Some("\r"),
        Modifiers::default(),
    );
    assert_eq!(numbers(), ["1", "2", "3", "4"]);
    let texts: Vec<String> = with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .filter_map(|&c| match &d.nodes[*d.nodes[c].children.first()?].kind {
                NodeKind::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect()
    });
    assert_eq!(texts, [" ", "a ", "b ", "c "]);
    // The text starts after the numbers.
    let x = with_doc(|d| {
        let h = d.editors[0];
        let row = d.nodes[h].children[1];
        d.nodes[d.nodes[row].children[0]].abs.x - d.nodes[row].abs.x
    });
    assert!(x > 10, "the text is {x} from the left");
}

#[test]
fn some_lines_can_fold_into_one_row() {
    let r = editor_ref();
    let mut ui = mount(move || {
        text_editor()
            .text(lines(30))
            .editor_ref(r)
            .w(300.0)
            .h(380.0)
            .overflow_y_scroll()
            .text_size(16.0)
            .decorate_lines(|_, range| range.map(|_| Rc::new(LineLook::default())).collect())
            .blocks(|_| {
                vec![(
                    3,
                    Row::Block {
                        lines: 4,
                        style: None,
                        widget: Widget::new(1, || text("folded")),
                    },
                )]
            })
    });
    // Lines 3 to 6 are one row, of the 31 lines.
    assert_eq!(children().iter().filter(|k| k.0).count() + 3, 31);
    let p = with_doc(|d| d.nodes[d.editors[0]].abs);
    click(&mut ui, PointF::new(p.x as f32 + 2.0, p.y as f32 + 2.0));
    for _ in 0..3 {
        key_with(
            &mut ui,
            Key::Named(NamedKey::ArrowDown),
            None,
            Modifiers::default(),
        );
    }
    assert_eq!(r.with(|e| e.line_of(e.focus)).unwrap(), 3);
    key_with(
        &mut ui,
        Key::Named(NamedKey::ArrowDown),
        None,
        Modifiers::default(),
    );
    assert_eq!(r.with(|e| e.line_of(e.focus)).unwrap(), 7);
}

/// Far down, with the estimates above learned again all at once (the lines wrap at another
/// width): the line at the top stays there. Before, the rows were chosen by where the view was
/// among the old estimates, so the rows that showed went and nothing held the view.
#[test]
fn the_line_at_the_top_stays_when_the_estimates_change() {
    let text: String = (0..3000)
        .map(|i| format!("{i} {}\n", "word ".repeat(i * 37 % 90)))
        .collect();
    let scroller = node_ref();
    let r = editor_ref();
    let width = signal(500.0f32);
    let mut ui = mount(move || {
        editor(text, scroller)
            .editor_ref(r)
            .dyn_style(move |s| s.w(width.get()))
    });
    r.scroll_to_line(2000.0);
    for _ in 0..3 {
        ui.update();
    }
    let before = r.top_line().unwrap();
    assert!((before - 2000.0).abs() < 1.0, "at line {before}");
    width.set(380.0);
    for _ in 0..4 {
        ui.update();
    }
    let after = r.top_line().unwrap();
    assert!(
        (after - before).abs() < 1.0,
        "the top went from line {before} to {after}"
    );
}
