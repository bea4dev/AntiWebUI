//! `markdown_editor` in a `Ui` without a window: what shows of the marks as the caret moves, and
//! how long a long document takes.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeId, NodeKind, with_doc};
use anti_webui_markdown::{MarkdownTheme, markdown_editor};
use std::time::{Duration, Instant};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1000, 900),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn editor(source: Signal<String>) -> impl IntoView {
    markdown_editor(source)
        .w(900.0)
        .h(800.0)
        .overflow_y_scroll()
        .p(16.0)
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

/// What each row shows: its text, or `<block>` for a row shown as a node.
fn shown() -> Vec<String> {
    let h = host();
    with_doc(|d| {
        d.nodes[h]
            .children
            .iter()
            .map(|&div| {
                let first = d.nodes[div].children.first().copied();
                match first.map(|c| &d.nodes[c].kind) {
                    Some(NodeKind::Text(t)) if t.row.is_some() => {
                        t.text.trim_end_matches(' ').to_owned()
                    }
                    _ => "<block>".to_owned(),
                }
            })
            .collect()
    })
}

fn select(at: usize) {
    let h = host();
    with_doc(|d| d.pending_focus = Some(Some(h)));
    // Through a press, as the user would: put the caret at `at`.
    with_doc(|d| {
        let e = d.nodes[h].editor_mut().unwrap();
        e.select(at, at);
    });
}

const DOC: &str = "# Title\n\nSome **bold** and `code` and [a link](https://x.y).\n\n- one\n- [ ] task\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n```rust\nfn main() {}\n```\n";

#[test]
fn marks_hide_away_from_the_caret_and_blocks_show_as_they_look() {
    let source = signal(DOC.to_owned());
    let mut ui = mount(move || editor(source));
    let rows = shown();
    assert_eq!(rows[0], "Title");
    assert_eq!(rows[2], "Some bold and code and a link.");
    assert_eq!(rows[4], " one");
    assert_eq!(rows[5], " task");
    assert_eq!(rows[7], "<block>", "the table");
    assert_eq!(rows[9], "<block>", "the rule");
    assert_eq!(rows[11], "", "the fences hide");
    assert_eq!(rows[12], "fn main() {}");

    // On the heading line, its mark shows.
    select(1);
    ui.update();
    named(&mut ui, NamedKey::ArrowRight);
    assert_eq!(shown()[0], "# Title");
    assert_eq!(shown()[2], "Some bold and code and a link.");
    // In the bold word, its marks show, and only those.
    let at = DOC.find("bold").unwrap();
    select(at + 1);
    named(&mut ui, NamedKey::ArrowRight);
    assert_eq!(shown()[0], "Title");
    assert_eq!(shown()[2], "Some **bold** and code and a link.");
    // In the table, its source.
    let at = DOC.find("| 1").unwrap();
    select(at + 1);
    named(&mut ui, NamedKey::ArrowRight);
    let rows = shown();
    assert_eq!(rows[7], "| a | b |");
    assert_eq!(rows[9], "| 1 | 2 |");
    assert_eq!(rows[11], "<block>", "the rule after it");
}

#[test]
fn enter_goes_on_with_a_list_and_ends_it_on_an_empty_item() {
    let source = signal("- one".to_owned());
    let mut ui = mount(move || editor(source));
    select(5);
    ui.update();
    named(&mut ui, NamedKey::Enter);
    typ(&mut ui, "two");
    assert_eq!(source.get_untracked(), "- one\n- two");
    named(&mut ui, NamedKey::Enter);
    named(&mut ui, NamedKey::Enter);
    assert_eq!(source.get_untracked(), "- one\n- two\n");
    key(&mut ui, Key::Character("z".into()), None, ctrl());
    assert_eq!(source.get_untracked(), "- one\n- two\n- ");
}

#[test]
fn a_task_box_ticks_the_task_in_the_text() {
    let source = signal("x\n\n- [ ] task".to_owned());
    let mut ui = mount(move || editor(source));
    // The box is the first node in the third line.
    let h = host();
    let rect = with_doc(|d| {
        let div = d.nodes[h].children[2];
        let text = d.nodes[div].children[0];
        let b = d.nodes[text].children[0];
        d.window_rect(b).unwrap()
    });
    let p = PointF::new(
        (rect.x + rect.width / 2) as f32,
        (rect.y + rect.height / 2) as f32,
    );
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
    assert_eq!(source.get_untracked(), "x\n\n- [x] task");
}

#[test]
fn time_a_long_document() {
    let path = std::env::var("MARKDOWN_FILE")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/examples/bench.md").into());
    let text = std::fs::read_to_string(path).unwrap();
    let lines = text.lines().count();
    let source = signal(text);
    let started = Instant::now();
    let mut ui = mount(move || editor(source));
    let first = started.elapsed();
    let h = host();
    with_doc(|d| d.pending_focus = Some(Some(h)));
    ui.update();
    // Typing in the middle, and moving the caret there.
    let middle = source.with_untracked(|s| s.len() / 2);
    let middle = source.with_untracked(|s| s[middle..].find('\n').unwrap() + middle + 1);
    select(middle);
    ui.update();
    // There first: the lines around it are laid out and shaped the first time they show.
    named(&mut ui, NamedKey::ArrowRight);
    named(&mut ui, NamedKey::ArrowLeft);
    let mut typing = Vec::new();
    for c in "hello world".chars() {
        let t = Instant::now();
        let c = c.to_string();
        key(
            &mut ui,
            Key::Character(c.clone()),
            Some(&c),
            Modifiers::default(),
        );
        typing.push(t.elapsed());
    }
    let mut moving = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        named(&mut ui, NamedKey::ArrowDown);
        moving.push(t.elapsed());
    }
    let avg = |v: &[Duration]| v.iter().sum::<Duration>() / v.len() as u32;
    let max = |v: &[Duration]| v.iter().max().copied().unwrap_or_default();
    eprintln!(
        "{lines} lines: first layout {first:?}; a keystroke {:?} (worst {:?}); a line down {:?} \
         (worst {:?})",
        avg(&typing),
        max(&typing),
        avg(&moving),
        max(&moving),
    );
}

#[test]
fn japanese_text_is_cut_only_between_letters() {
    // Blocks that end in letters of more than one byte, without a line break at the end.
    let doc = "# 見出しの終わり\n\n**太字**と`コード`と[リンク](https://例.jp)の段落の終わり\n\n- 項目の終わり\n- [ ] タスクの終わり\n\n> 引用の終わり\n\n| 名前 | 説明 |\n|---|---|\n| あ | い |\n\n最後の段落の終わり";
    let source = signal(doc.to_owned());
    let mut ui = mount(move || editor(source));
    assert_eq!(shown()[0], "見出しの終わり");
    select(0);
    ui.update();
    // Through every letter, which shows and hides the marks around it.
    for _ in 0..doc.chars().count() {
        named(&mut ui, NamedKey::ArrowRight);
    }
    typ(&mut ui, "。追記");
    assert!(source.get_untracked().ends_with("最後の段落の終わり。追記"));
    for _ in 0..12 {
        named(&mut ui, NamedKey::ArrowUp);
        named(&mut ui, NamedKey::End);
    }
}

#[test]
fn items_with_room_before_their_marks_get_their_dots_and_boxes() {
    let source = signal("text\n\n - one\n - [ ] task\n   * three".to_owned());
    let _ui = mount(move || editor(source));
    let rows = shown();
    // The mark is a node: what is left is the room before it and the text after it.
    assert_eq!(rows[2], "  one");
    assert_eq!(rows[3], "  task");
    let h = host();
    let nodes: Vec<usize> = with_doc(|d| {
        d.nodes[h].children[2..5]
            .iter()
            .map(|&div| d.nodes[d.nodes[div].children[0]].children.len())
            .collect()
    });
    assert_eq!(nodes, [1, 1, 1]);
}

#[test]
fn alerts_footnotes_and_what_is_raised_show_as_they_look() {
    let doc =
        "> [!NOTE]\n> GitHub のアラート\n\n脚注[^1]。上付き^2^と下付き~x~。\n\n[^1]: これが脚注";
    let source = signal(doc.to_owned());
    let mut ui = mount(move || editor(source));
    let rows = shown();
    // The name of the alert is a node in place of `[!NOTE]`.
    assert_eq!(rows[0], "");
    assert_eq!(rows[1], "GitHub のアラート");
    assert_eq!(rows[3], "脚注1。上付き2と下付きx。");
    assert_eq!(rows[5], "1 これが脚注");
    let h = host();
    let aligns: Vec<VerticalAlign> = with_doc(|d| {
        let div = d.nodes[h].children[3];
        match &d.nodes[d.nodes[div].children[0]].kind {
            NodeKind::Text(t) => t
                .rich
                .as_ref()
                .unwrap()
                .spans
                .iter()
                .map(|s| s.align)
                .collect(),
            _ => Vec::new(),
        }
    });
    assert_eq!(
        aligns,
        [
            VerticalAlign::Super,
            VerticalAlign::Super,
            VerticalAlign::Sub
        ]
    );
    // With the caret at the raised `2`, its marks show, and only those.
    let at = doc.find("^2^").unwrap();
    select(at);
    named(&mut ui, NamedKey::ArrowRight);
    assert_eq!(shown()[3], "脚注1。上付き^2^と下付きx。");
    assert_eq!(shown()[0], "");
}

#[test]
fn a_done_task_looks_as_the_theme_says() {
    let struck = |theme: Option<MarkdownTheme>| {
        let source = signal("- [x] done".to_owned());
        let _ui = mount(move || {
            let e = markdown_editor(source).w(900.0).h(800.0);
            match theme {
                Some(t) => e.theme(t).into_view(),
                None => e.into_view(),
            }
        });
        let h = host();
        with_doc(|d| {
            let div = d.nodes[h].children[0];
            match &d.nodes[d.nodes[div].children[0]].kind {
                NodeKind::Text(t) => t
                    .rich
                    .as_ref()
                    .unwrap()
                    .spans
                    .iter()
                    .any(|s| s.style.strikethrough == Some(true)),
                _ => false,
            }
        })
    };
    // As on GitHub: only the box is ticked.
    assert!(!struck(None));
    let theme = MarkdownTheme {
        task_done: Style::default().strikethrough(),
        ..MarkdownTheme::default()
    };
    assert!(struck(Some(theme)));
}

#[test]
fn a_selection_over_a_table_leaves_it_as_it_looks() {
    let doc = "before\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nafter";
    let source = signal(doc.to_owned());
    let mut ui = mount(move || editor(source));
    select(1);
    ui.update();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    for _ in 0..4 {
        key(&mut ui, Key::Named(NamedKey::ArrowDown), None, shift);
    }
    let rows = shown();
    assert_eq!(rows[2], "<block>");
    let h = host();
    assert_eq!(
        with_doc(|d| d.nodes[h].editor().unwrap().selected_blocks().len()),
        1
    );
    // A caret in it shows its source.
    let at = doc.find("| 1").unwrap();
    select(at + 1);
    named(&mut ui, NamedKey::ArrowRight);
    assert_eq!(shown()[4], "| 1 | 2 |");
}

#[test]
fn down_goes_down_from_inside_code() {
    let doc = "some `code` here\nthe next line\nand the last";
    let source = signal(doc.to_owned());
    let mut ui = mount(move || editor(source));
    let caret = || {
        let h = host();
        with_doc(|d| d.nodes[h].editor().unwrap().focus)
    };
    for at in [
        doc.find("code").unwrap() + 1,
        doc.find('`').unwrap(),
        doc.find("` here").unwrap() + 1,
    ] {
        select(at);
        ui.update();
        named(&mut ui, NamedKey::ArrowRight);
        named(&mut ui, NamedKey::ArrowLeft);
        assert_eq!(caret(), at);
        named(&mut ui, NamedKey::ArrowDown);
        let line = source.with_untracked(|s| s[..caret()].matches('\n').count());
        assert_eq!(line, 1, "from {at}: at {}", caret());
        named(&mut ui, NamedKey::ArrowUp);
        assert_eq!(
            source.with_untracked(|s| s[..caret()].matches('\n').count()),
            0
        );
    }
}
