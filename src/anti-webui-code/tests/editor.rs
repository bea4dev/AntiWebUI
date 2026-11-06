//! `code_editor` through `Ui`: colors that come from the worker, the keys, folding and finding.

use anti_webui_code::{CodeTheme, Language, code_editor};
use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeKind, with_doc};
use std::time::{Duration, Instant};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn press(ui: &mut Ui, key: Key, text: Option<&str>, modifiers: Modifiers) {
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
        press(
            ui,
            Key::Character(c.clone()),
            Some(&c),
            Modifiers::default(),
        );
    }
}

fn named(ui: &mut Ui, k: NamedKey) {
    let text = matches!(k, NamedKey::Enter).then_some("\r");
    press(ui, Key::Named(k), text, Modifiers::default());
}

fn ctrl(shift: bool) -> Modifiers {
    Modifiers {
        ctrl: true,
        shift,
        ..Modifiers::default()
    }
}

/// Clicks at the start of the editor's first line.
fn click_start(ui: &mut Ui) {
    let r = with_doc(|d| {
        let h = d.editors[0];
        let row = d.nodes[h].children[0];
        let t = d.nodes[row].children[0];
        d.nodes[t].abs
    });
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: PointF::new(r.x as f32 + 1.0, r.y as f32 + 2.0),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    ui.update();
}

/// The colors of the spans of each row that has nodes.
fn colors() -> Vec<Vec<(String, Option<Color>)>> {
    with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .filter_map(|&row| {
                let t = *d.nodes[row].children.first()?;
                let NodeKind::Text(t) = &d.nodes[t].kind else {
                    return None;
                };
                let rich = t.rich.as_ref()?;
                Some(
                    rich.spans
                        .iter()
                        .map(|s| (t.text[s.range.clone()].to_owned(), s.style.color))
                        .collect(),
                )
            })
            .collect()
    })
}

/// Waits for what the worker sends.
fn settle(ui: &mut Ui, until: impl Fn() -> bool) {
    let started = Instant::now();
    while !until() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
        ui.poll_images();
        ui.update();
    }
    assert!(until(), "nothing came from the worker");
}

fn keyword_color() -> Option<Color> {
    CodeTheme::dark().style_of("keyword").unwrap().color
}

fn colored(word: &str) -> bool {
    colors()
        .iter()
        .flatten()
        .any(|(t, c)| t == word && *c == keyword_color())
}

#[test]
fn colors_come_and_follow_the_edits() {
    let r = editor_ref();
    let mut ui = mount(move || {
        code_editor("fn main() {}\n")
            .language(Language::Rust)
            .editor_ref(r)
            .w(600.0)
            .h(400.0)
    });
    settle(&mut ui, || colored("fn"));
    click_start(&mut ui);
    typ(&mut ui, "pub ");
    // The new word is colored too, right away or once parsed again.
    settle(&mut ui, || colored("pub"));
    assert_eq!(r.text(), "pub fn main() {}\n");
}

#[test]
fn enter_indents_and_brackets_go_on_lines_of_their_own() {
    let r = editor_ref();
    let mut ui = mount(move || {
        code_editor("fn f() {}")
            .language(Language::Rust)
            .editor_ref(r)
            .w(600.0)
            .h(400.0)
    });
    click_start(&mut ui);
    press(
        &mut ui,
        Key::Named(NamedKey::End),
        None,
        Modifiers::default(),
    );
    named(&mut ui, NamedKey::ArrowLeft);
    named(&mut ui, NamedKey::Enter);
    typ(&mut ui, "x");
    assert_eq!(r.text(), "fn f() {\n    x\n}");
    // Undo takes the whole Enter back in one step.
    press(&mut ui, Key::Character("z".into()), None, ctrl(false));
    press(&mut ui, Key::Character("z".into()), None, ctrl(false));
    assert_eq!(r.text(), "fn f() {}");
}

#[test]
fn ctrl_slash_comments_lines() {
    let r = editor_ref();
    let mut ui = mount(move || {
        code_editor("a\nb")
            .language(Language::Python)
            .editor_ref(r)
            .w(600.0)
            .h(400.0)
    });
    click_start(&mut ui);
    press(&mut ui, Key::Character("a".into()), None, ctrl(false));
    press(&mut ui, Key::Character("/".into()), None, ctrl(false));
    assert_eq!(r.text(), "# a\n# b");
    press(&mut ui, Key::Character("/".into()), None, ctrl(false));
    assert_eq!(r.text(), "a\nb");
}

/// How many rows have nodes and show something.
fn shown_rows() -> usize {
    with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .filter(|&&c| !d.nodes[c].children.is_empty() && d.nodes[c].rect.height > 0)
            .count()
    })
}

#[test]
fn folding_hides_the_lines_inside() {
    let r = editor_ref();
    let mut ui = mount(move || {
        code_editor("fn a() {\n    x;\n    y;\n}\nfn b() {}")
            .language(Language::Rust)
            .editor_ref(r)
            .w(600.0)
            .h(400.0)
    });
    assert_eq!(shown_rows(), 5);
    click_start(&mut ui);
    press(&mut ui, Key::Character("[".into()), None, ctrl(true));
    assert_eq!(shown_rows(), 3, "the two lines inside are hidden");
    press(&mut ui, Key::Character("]".into()), None, ctrl(true));
    assert_eq!(shown_rows(), 5);
}

#[test]
fn ctrl_f_finds_and_enter_goes_to_the_next() {
    let r = editor_ref();
    let mut ui = mount(move || {
        code_editor("one two\ntwo three two")
            .editor_ref(r)
            .w(600.0)
            .h(400.0)
    });
    click_start(&mut ui);
    press(&mut ui, Key::Character("f".into()), None, ctrl(false));
    // The find field has the focus.
    typ(&mut ui, "two");
    assert_eq!(r.with(|e| e.selection_range()).unwrap(), (4, 7));
    named(&mut ui, NamedKey::Enter);
    assert_eq!(r.with(|e| e.selection_range()).unwrap(), (8, 11));
    named(&mut ui, NamedKey::Enter);
    assert_eq!(r.with(|e| e.selection_range()).unwrap(), (18, 21));
    // The matches are marked in the text.
    let marked = with_doc(|d| {
        let h = d.editors[0];
        d.nodes[h]
            .children
            .iter()
            .filter_map(|&row| {
                let t = *d.nodes[row].children.first()?;
                let NodeKind::Text(t) = &d.nodes[t].kind else {
                    return None;
                };
                Some(
                    t.rich
                        .as_ref()?
                        .spans
                        .iter()
                        .filter(|s| s.style.background.is_some())
                        .count(),
                )
            })
            .sum::<usize>()
    });
    assert_eq!(marked, 3);
    named(&mut ui, NamedKey::Escape);
    // Typing goes into the text again.
    typ(&mut ui, "X");
    assert_eq!(r.text(), "one two\ntwo three X");
}

/// How fast a long file is: `CODE_FILE=sqlite3.c cargo test --release -p anti-webui-code --test
/// editor time_a_long_file -- --ignored --nocapture`. A 1000 × 900 editor at scale 1.8.
#[test]
#[ignore]
fn time_a_long_file() {
    let path = std::env::var("CODE_FILE").expect("CODE_FILE");
    let source = std::fs::read_to_string(&path).unwrap();
    let lang = Language::from_path(&path);
    let started = Instant::now();
    let numbers = std::env::var("NO_NUMBERS").is_err();
    let mut ui = Ui::new(move || {
        code_editor(source)
            .language(lang)
            .line_numbers(numbers)
            .w(1000.0)
            .h(900.0)
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1800, 1620),
        scale: Scale(216),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let built = started.elapsed();
    settle(&mut ui, || {
        colors().iter().flatten().any(|(_, c)| c.is_some())
    });
    let colored = started.elapsed();
    click_start(&mut ui);
    let time = |ui: &mut Ui, n: usize, f: &dyn Fn(&mut Ui)| {
        let mut t: Vec<Duration> = (0..n)
            .map(|_| {
                let s = Instant::now();
                f(ui);
                // What the worker sent comes in too.
                ui.poll_images();
                ui.update();
                s.elapsed()
            })
            .collect();
        t.sort();
        (t[n / 2], t[n - 1])
    };
    let pages = time(&mut ui, 30, &|ui| named(ui, NamedKey::PageDown));
    let keys = time(&mut ui, 40, &|ui| typ(ui, "x"));
    let enters = time(&mut ui, 20, &|ui| named(ui, NamedKey::Enter));
    let rss = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("RssAnon"))
        .unwrap_or_default()
        .to_owned();
    eprintln!(
        "{path}: built {built:?}, colored {colored:?}; page down {:?} (max {:?}), key {:?} (max {:?}), enter {:?} (max {:?}); {rss}",
        pages.0, pages.1, keys.0, keys.1, enters.0, enters.1
    );
}
