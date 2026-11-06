//! `markdown_view` in a `Ui` without a window.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeId, NodeKind, with_doc};
use anti_webui_markdown::{Emoji, markdown_view};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SAMPLE: &str = include_str!("../examples/sample.md");

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(900, 3000),
        scale: Scale(120),
    });
    ui.update();
    ui
}

fn settle(ui: &mut Ui) {
    for _ in 0..4 {
        ui.update();
        ui.wait_for_images(Duration::from_secs(20));
    }
    ui.update();
}

/// The text nodes, in the order of the document.
fn texts() -> Vec<(NodeId, String)> {
    with_doc(|d| {
        let mut out: Vec<(u32, NodeId, String)> = d
            .nodes
            .iter()
            .filter(|(_, n)| n.visible)
            .filter_map(|(id, n)| match &n.kind {
                NodeKind::Text(t) => Some((n.doc_order, id, t.text.clone())),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(o, ..)| *o);
        out.into_iter().map(|(_, id, t)| (id, t)).collect()
    })
}

fn rect_of_text(prefix: &str) -> RectI {
    let id = texts()
        .into_iter()
        .find(|(_, t)| t.starts_with(prefix))
        .unwrap_or_else(|| panic!("no text {prefix:?}"))
        .0;
    with_doc(|d| d.window_rect(id).unwrap())
}

#[test]
fn a_document_shows_its_blocks() {
    let mut ui = mount(|| markdown_view(SAMPLE).w(800.0));
    settle(&mut ui);
    let all: Vec<String> = texts().into_iter().map(|(_, t)| t).collect();
    let has = |s: &str| all.iter().any(|t| t.contains(s));
    assert!(has("AntiWebUI Markdown"));
    assert!(has("これは 太字 と 斜体"), "{all:?}");
    assert!(has("入れ子の項目"));
    assert!(has("println!"));
    assert!(has("rich_text"));
    // The heading is bigger than the paragraph under it, and above it.
    let h1 = rect_of_text("AntiWebUI Markdown");
    let p = rect_of_text("これは");
    assert!(h1.height > p.height / 2 + 10, "{h1:?} {p:?}");
    assert!(p.y > h1.bottom());
    // A nested item is further in than its parent.
    assert!(rect_of_text("入れ子").x > rect_of_text("一つ目").x + 20);
    // The cells of a column are as wide as each other, and the columns don't overlap.
    let name = rect_of_text("名前");
    let mermaid = rect_of_text("Mermaid");
    let count = rect_of_text("数");
    assert!(count.x > name.right());
    assert!((mermaid.x - name.x).abs() <= 1);
}

#[test]
fn diagrams_are_drawn() {
    let mut ui = mount(|| markdown_view(SAMPLE).w(800.0));
    settle(&mut ui);
    let svgs = with_doc(|d| {
        d.nodes
            .iter()
            .filter(|(_, n)| matches!(n.kind, NodeKind::Svg(_)) && n.visible)
            .map(|(_, n)| n.rect)
            .collect::<Vec<_>>()
    });
    let all: Vec<String> = texts().into_iter().map(|(_, t)| t).collect();
    let failed: Vec<&String> = all.iter().filter(|t| t.starts_with("Mermaid:")).collect();
    assert_eq!(svgs.len(), 2, "both diagrams are made: {failed:?}");
    assert!(
        svgs.iter().all(|r| r.width > 100 && r.height > 100),
        "{svgs:?}"
    );
    let all: Vec<String> = texts().into_iter().map(|(_, t)| t).collect();
    assert!(
        all.iter().any(|t| t.contains("条件")),
        "the labels are text: {all:?}"
    );
}

#[test]
fn only_the_blocks_that_changed_are_made_again() {
    let source = signal(String::from("# One\n\nTwo\n\nThree\n"));
    let mut ui = mount(move || markdown_view(move || source.get()));
    let before = texts();
    source.set(String::from("# One\n\nTwo changed\n\nThree\n"));
    ui.update();
    let after = texts();
    assert_eq!(after.len(), 3);
    assert_eq!(after[0].0, before[0].0, "the heading stays");
    assert_ne!(after[1].0, before[1].0);
    assert_eq!(after[1].1, "Two changed");
    assert_eq!(after[2].0, before[2].0, "what is after it stays");
}

#[test]
fn links_go_to_headings_or_out() {
    let followed = Rc::new(RefCell::new(Vec::new()));
    let f = followed.clone();
    let mut ui = mount(move || {
        div().h(400.0).overflow_y_scroll().child(
            markdown_view(SAMPLE)
                .w(800.0)
                .on_link(move |url| f.borrow_mut().push(url.to_owned())),
        )
    });
    settle(&mut ui);
    // Clicking on the span itself goes through `rich_text`; here the paths after it are tested.
    let clicks: Vec<_> = with_doc(|d| {
        d.nodes
            .iter()
            .filter_map(|(_, n)| match &n.kind {
                NodeKind::Text(t) => t.rich.as_ref().map(|r| {
                    r.spans
                        .iter()
                        .filter_map(|s| {
                            s.on_click
                                .clone()
                                .map(|c| (t.text[s.range.clone()].to_owned(), c))
                        })
                        .collect::<Vec<_>>()
                }),
                _ => None,
            })
            .flatten()
            .collect()
    });
    for (label, click) in &clicks {
        if label == "外へのリンク" || label == "見出しへのリンク" {
            click();
        }
    }
    assert_eq!(*followed.borrow(), ["https://example.com"]);
}

#[test]
fn emoji_are_pictures_among_the_words() {
    let mut ui = mount(|| {
        markdown_view("Party :party: time and :unknown:")
            .emoji(|name| (name == "party").then(|| Emoji::Image(ImageSource::from("party.png"))))
    });
    settle(&mut ui);
    let (text, boxes) = with_doc(|d| {
        d.nodes
            .iter()
            .find_map(|(_, n)| match &n.kind {
                NodeKind::Text(t) if t.text.starts_with("Party") => {
                    Some((t.text.clone(), t.rich.as_ref().map_or(0, |r| r.boxes.len())))
                }
                _ => None,
            })
            .unwrap()
    });
    assert_eq!(text, "Party  time and :unknown:");
    assert_eq!(boxes, 1);
}

/// How long a long document takes to show, to compare with Chromium (run with `--ignored`). The
/// document is `MARKDOWN_FILE` if it is set, or the sample 50 times.
#[test]
#[ignore]
fn time_a_long_document() {
    let doc = std::env::var("MARKDOWN_FILE")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| SAMPLE.repeat(50));
    let bytes = doc.len();
    let started = Instant::now();
    let mut ui = Ui::new(move || markdown_view(doc).w(800.0));
    let built = started.elapsed();
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(900, 3000),
        scale: Scale(120),
    });
    ui.update();
    let laid_out = started.elapsed();
    let _ = ui.paint(Instant::now());
    let painted = started.elapsed();
    settle(&mut ui);
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    eprintln!(
        "{bytes} bytes: built in {:.1} ms, laid out at {:.1} ms, painted at {:.1} ms, with diagrams at {:.1} ms, {} nodes",
        ms(built),
        ms(laid_out),
        ms(painted),
        ms(started.elapsed()),
        with_doc(|d| d.nodes.len())
    );
}
