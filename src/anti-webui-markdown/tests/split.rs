//! `markdown_split_editor` through `Ui`: the right side follows the source as it is edited and
//! scrolled, and the source follows the right side; code blocks are colored.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeKind, with_doc};
use anti_webui_markdown::{markdown_split_editor, markdown_view};
use std::time::{Duration, Instant};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1000, 600),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

/// Lets what other threads send come in until `until`.
fn settle(ui: &mut Ui, until: impl Fn() -> bool) {
    let started = Instant::now();
    while !until() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
        ui.poll_images();
        ui.update();
    }
    assert!(until(), "nothing came");
}

/// Every text shown, with the color of each of its spans.
fn spans() -> Vec<(String, Option<Color>)> {
    with_doc(|d| {
        let mut out = Vec::new();
        for (_, n) in d.nodes.iter() {
            let NodeKind::Text(t) = &n.kind else {
                continue;
            };
            match &t.rich {
                Some(rich) => {
                    for s in &rich.spans {
                        out.push((t.text[s.range.clone()].to_owned(), s.style.color));
                    }
                }
                None => out.push((t.text.clone(), None)),
            }
        }
        out
    })
}

fn has_text(s: &str) -> bool {
    with_doc(|d| {
        d.nodes
            .iter()
            .any(|(_, n)| matches!(&n.kind, NodeKind::Text(t) if t.text.contains(s)))
    })
}

#[test]
fn code_blocks_in_the_viewer_are_colored() {
    let mut ui = mount(|| markdown_view("```rust\nfn main() { let x = 1; }\n```\n").w(800.0));
    let keyword = anti_webui_code::CodeTheme::light()
        .style_of("keyword")
        .unwrap()
        .color;
    // The grammar is compiled on another thread, then the block is made again with its colors.
    settle(&mut ui, || {
        spans()
            .iter()
            .any(|(t, c)| t == "fn" && *c == keyword && c.is_some())
    });
}

/// A document of `n` sections, each a heading and a few lines.
fn long_doc(n: usize) -> String {
    (0..n)
        .map(|i| format!("## Section {i}\n\nSome text of section {i}.\nAnd more of it.\n\n"))
        .collect()
}

#[test]
fn typing_on_the_left_shows_on_the_right() {
    let source = std::rc::Rc::new(std::cell::Cell::new(None));
    let s2 = source.clone();
    let editor = editor_ref();
    let mut ui = mount(move || {
        let s = signal("# Title\n".to_owned());
        s2.set(Some(s));
        markdown_split_editor(s)
            .editor_ref(editor)
            .w(1000.0)
            .h(600.0)
    });
    assert!(has_text("Title"));
    editor.replace(7..7, " and more");
    ui.update();
    assert_eq!(source.get().unwrap().get_untracked(), "# Title and more\n");
    assert!(has_text("Title and more"));
}

#[test]
fn the_right_side_follows_the_source_and_back() {
    let editor = editor_ref();
    let mut ui = mount(move || {
        let s = signal(long_doc(200));
        markdown_split_editor(s)
            .editor_ref(editor)
            .w(1000.0)
            .h(600.0)
    });
    for _ in 0..3 {
        ui.update();
    }
    // The preview is the scrolling box that isn't the editor.
    let preview_scroll = || {
        with_doc(|d| {
            d.nodes
                .iter()
                .filter(|(_, n)| {
                    n.computed.overflow_y == anti_webui_core::style::Overflow::Scroll
                        && !matches!(n.kind, NodeKind::Editor(_))
                })
                .map(|(_, n)| n.scroll.y)
                .max()
                .unwrap_or(0)
        })
    };
    assert_eq!(preview_scroll(), 0);
    // Section 100 starts on line 500.
    editor.scroll_to_line(500.0);
    for _ in 0..3 {
        ui.update();
    }
    let top = editor.top_line().unwrap();
    assert!((top - 500.0).abs() < 1.0, "the editor is at line {top}");
    let followed = preview_scroll();
    assert!(followed > 0, "the right side followed");
    let shown = |s: &str| {
        with_doc(|d| {
            d.nodes.iter().any(|(id, n)| {
                matches!(&n.kind, NodeKind::Text(t) if t.text == s)
                    && d.visible_window_rect(id)
                        .is_some_and(|r| r.height > 0 && r.y < 300)
            })
        })
    };
    assert!(
        shown("Section 100"),
        "the heading of the line at the top shows"
    );

    // Scrolling the right side with the wheel moves the source.
    for _ in 0..10 {
        ui.handle_event(WindowEvent::Scroll {
            position: PointF::new(800.0, 300.0),
            delta_x: 0.0,
            delta_y: -3.0,
            modifiers: Modifiers::default(),
            source: ScrollSource::Wheel,
            phase: ScrollPhase::Single,
            time: 0,
        });
        ui.update();
    }
    for _ in 0..60 {
        ui.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(preview_scroll() < followed, "the right side scrolled up");
    let top = editor.top_line().unwrap();
    assert!(top < 499.0, "the source followed, to line {top}");
}

/// How long a keystroke takes, the source and the right side together:
/// `MARKDOWN_FILE=... cargo test --release -p anti-webui-markdown --test split time_typing --
/// --ignored --nocapture`. A 1600 × 900 window at scale 1.8, typing in a paragraph in the middle.
#[test]
#[ignore]
fn time_typing() {
    let doc = std::env::var("MARKDOWN_FILE")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| long_doc(2000));
    let at = {
        let mid = doc.len() / 2;
        mid + doc[mid..].find("\n\n").unwrap() + 2
    };
    let editor = editor_ref();
    let started = Instant::now();
    let mut ui = Ui::new(move || {
        let s = signal(doc);
        markdown_split_editor(s)
            .editor_ref(editor)
            .w_full()
            .h_full()
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(2880, 1620),
        scale: Scale(216),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let _ = ui.paint(Instant::now());
    let first = started.elapsed();
    settle(&mut ui, || true);
    editor.focus();
    editor.select(at, at);
    let line = editor.with(|e| e.line_of(at)).unwrap();
    editor.scroll_to_line(line as f64 - 10.0);
    for _ in 0..5 {
        ui.update();
        let _ = ui.paint(Instant::now());
    }
    let key = |ui: &mut Ui, k: Key, text: Option<&str>| {
        for pressed in [true, false] {
            ui.handle_event(WindowEvent::Key(KeyEvent {
                key: k.clone(),
                text: text.filter(|_| pressed).map(Into::into),
                modifiers: Modifiers::default(),
                pressed,
                repeat: false,
            }));
        }
        ui.update();
        let _ = ui.paint(Instant::now());
    };
    let mut times = Vec::new();
    for i in 0..60 {
        let t = Instant::now();
        if i % 10 == 9 {
            key(&mut ui, Key::Named(NamedKey::Enter), Some("\r"));
        } else {
            key(&mut ui, Key::Character("x".into()), Some("x"));
        }
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let rss = std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("RssAnon"))
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|k| k.parse::<u64>().ok())
        .unwrap_or(0);
    eprintln!(
        "first frame at {:.1} ms; a keystroke: {:.2} ms median, {:.2} ms at worst; {} MB anonymous",
        first.as_secs_f64() * 1000.0,
        times[times.len() / 2],
        times[times.len() - 1],
        rss / 1024
    );
}

/// The window rectangle of the text node that says exactly `s` and shows.
fn shown_rect(s: &str) -> Option<anti_webui_core::geometry::RectI> {
    with_doc(|d| {
        d.nodes.iter().find_map(|(id, n)| {
            matches!(&n.kind, NodeKind::Text(t) if t.text == s)
                .then(|| d.visible_window_rect(id))
                .flatten()
                .filter(|r| r.height > 0)
        })
    })
}

#[test]
fn the_caret_line_is_marked_on_the_right() {
    let editor = editor_ref();
    let mut ui = mount(move || {
        let s = signal(long_doc(200));
        markdown_split_editor(s)
            .editor_ref(editor)
            .w(1000.0)
            .h(600.0)
    });
    editor.focus();
    // Section 100's heading is line 500.
    let at = editor.with(|e| e.line(500).start).unwrap();
    editor.select(at, at);
    editor.scroll_to_line(498.0);
    for _ in 0..4 {
        ui.update();
    }
    let bar = Color::from_rgb(0x0969da).with_alpha(0.6);
    let bar_rect = || {
        with_doc(|d| {
            d.nodes.iter().find_map(|(id, n)| {
                (n.computed.background == bar)
                    .then(|| d.window_rect(id))
                    .flatten()
                    .filter(|r| r.height > 0)
            })
        })
    };
    let heading = shown_rect("Section 100").expect("the heading shows");
    let mark = bar_rect().expect("the bar shows");
    assert!(
        mark.y < heading.bottom() && mark.bottom() > heading.y,
        "the bar {mark:?} is along the heading {heading:?}"
    );
    // On the paragraph's second line, it is lower, along the paragraph.
    let at = editor.with(|e| e.line(503).start).unwrap();
    editor.select(at, at);
    for _ in 0..3 {
        ui.update();
    }
    let lower = bar_rect().unwrap();
    assert!(lower.y >= mark.bottom(), "{lower:?} is below {mark:?}");
}

#[test]
fn a_double_click_on_the_right_takes_the_source_there() {
    let editor = editor_ref();
    let mut ui = mount(move || {
        let s = signal(long_doc(200));
        markdown_split_editor(s)
            .editor_ref(editor)
            .w(1000.0)
            .h(600.0)
    });
    editor.scroll_to_line(600.0);
    for _ in 0..4 {
        ui.update();
    }
    let r = shown_rect("Section 120").expect("the heading shows on the right");
    let p = PointF::new(r.x as f32 + 4.0, (r.y + r.height / 2) as f32);
    for _ in 0..2 {
        for pressed in [true, false] {
            ui.handle_event(WindowEvent::PointerButton {
                position: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::default(),
                time: 0,
            });
        }
    }
    for _ in 0..3 {
        ui.update();
    }
    let (line, focused) = with_doc(|d| {
        let id = d.editors[0];
        let e = d.nodes[id].editor().unwrap();
        (e.line_of(e.focus), d.state.focused == Some(id))
    });
    assert_eq!(line, 600, "the caret is on the heading's line");
    assert!(focused, "the source has the keyboard");
}

/// How long frames take while the right side is scrolled with the wheel, from the top and then
/// after a link to a heading far down, and while the source is scrolled after that: `cargo test --release -p anti-webui-markdown --test split
/// time_scrolling_after_a_link -- --ignored --nocapture`. `LINK` is the text of the link.
#[test]
#[ignore]
fn time_scrolling_after_a_link() {
    let doc = include_str!("../examples/bench.md").to_owned();
    let link = std::env::var("LINK").unwrap_or_else(|_| "12. Chapter 12 の見出し".into());
    let editor = editor_ref();
    let mut ui = Ui::new(move || {
        let s = signal(doc);
        markdown_split_editor(s)
            .editor_ref(editor)
            .w_full()
            .h_full()
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(2880, 1620),
        scale: Scale(216),
    });
    ui.handle_event(WindowEvent::Focused(true));
    settle(&mut ui, || true);
    for _ in 0..5 {
        ui.update();
        let _ = ui.paint(Instant::now());
    }
    let frames = |ui: &mut Ui, notches: usize, x: f32| {
        let mut times = Vec::new();
        let mut path = Vec::new();
        for _ in 0..notches {
            path.push(format!("{:.0}", editor.top_line().unwrap_or(-1.0)));
            ui.handle_event(WindowEvent::Scroll {
                position: PointF::new(x, 800.0),
                delta_x: 0.0,
                delta_y: 1.0,
                modifiers: Modifiers::default(),
                source: ScrollSource::Wheel,
                phase: ScrollPhase::Single,
                time: 0,
            });
            for _ in 0..6 {
                let t = Instant::now();
                ui.update();
                let _ = ui.paint(Instant::now());
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                std::thread::sleep(Duration::from_millis(8));
            }
        }
        let slow: Vec<String> = times
            .iter()
            .enumerate()
            .filter(|(_, t)| **t > 3.0)
            .map(|(i, t)| format!("#{i} {t:.1}"))
            .collect();
        eprintln!("  slow frames: {}", slow.join(", "));
        eprintln!("  the source's top line at each notch: {}", path.join(" "));
        times.sort_by(f64::total_cmp);
        let n = times.len();
        format!(
            "median {:.2} ms, 90% {:.2} ms, worst {:.2} ms, over 8 ms: {}",
            times[n / 2],
            times[n * 9 / 10],
            times[n - 1],
            times.iter().filter(|&&t| t > 8.0).count()
        )
    };
    eprintln!("from the top: {}", frames(&mut ui, 40, 2200.0));
    let preview_text = |s: &str| {
        with_doc(|d| {
            d.nodes.iter().find_map(|(id, n)| {
                matches!(&n.kind, NodeKind::Text(t) if t.text.contains(s))
                    .then(|| d.visible_window_rect(id))
                    .flatten()
                    .filter(|r| r.height > 0 && r.x > 1500)
            })
        })
    };
    // Back to the top, where the table of contents is.
    for _ in 0..40 {
        ui.handle_event(WindowEvent::Scroll {
            position: PointF::new(2200.0, 800.0),
            delta_x: 0.0,
            delta_y: -1.0,
            modifiers: Modifiers::default(),
            source: ScrollSource::Wheel,
            phase: ScrollPhase::Single,
            time: 0,
        });
        ui.update();
        std::thread::sleep(Duration::from_millis(8));
    }
    for _ in 0..60 {
        ui.update();
        let _ = ui.paint(Instant::now());
        std::thread::sleep(Duration::from_millis(8));
    }
    // Down until the link shows.
    for _ in 0..20 {
        if preview_text(&link).is_some_and(|r| r.y > 100 && r.bottom() < 1500) {
            break;
        }
        ui.handle_event(WindowEvent::Scroll {
            position: PointF::new(2200.0, 800.0),
            delta_x: 0.0,
            delta_y: 1.0,
            modifiers: Modifiers::default(),
            source: ScrollSource::Wheel,
            phase: ScrollPhase::Single,
            time: 0,
        });
        for _ in 0..20 {
            ui.update();
            let _ = ui.paint(Instant::now());
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    let r = preview_text(&link).expect("the link shows");
    let p = PointF::new(r.x as f32 + 20.0, (r.y + r.height / 2) as f32);
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    let t = Instant::now();
    ui.update();
    let _ = ui.paint(Instant::now());
    eprintln!("the jump: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    for _ in 0..10 {
        ui.update();
        let _ = ui.paint(Instant::now());
        std::thread::sleep(Duration::from_millis(8));
    }
    eprintln!("editor at line {:?}", editor.top_line());
    eprintln!("after the link: {}", frames(&mut ui, 40, 2200.0));
    eprintln!("editor at line {:?}", editor.top_line());
    eprintln!("the source scrolled: {}", frames(&mut ui, 40, 700.0));
    eprintln!("editor at line {:?}", editor.top_line());
}
