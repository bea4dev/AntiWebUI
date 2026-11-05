//! Flex layout against what Chromium makes of the same boxes. The cases and its answers are in
//! `tests/data/flex_wrap.txt`; `tools/chrome_layout.py` writes the answers.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;

fn apply(mut d: Div, css: &str) -> Div {
    for decl in css.split(';').filter(|s| !s.is_empty()) {
        let (name, value) = decl.split_once(':').unwrap();
        let num = |v: &str| v.trim_end_matches("px").parse::<f32>().unwrap();
        let len = |v: &str| match v.strip_suffix('%') {
            Some(p) => pct(p.parse().unwrap()),
            None => Length::Px(num(v)),
        };
        d = match (name, value) {
            ("display", "flex") => d.flex(),
            ("display", "block") => d.block(),
            ("display", "none") => d.hidden(),
            ("position", "relative") => d.relative(),
            ("position", "absolute") => d.absolute(),
            ("flex-direction", "row") => d.flex_row(),
            ("flex-direction", "column") => d.flex_col(),
            ("flex-direction", "row-reverse") => d.flex_row_reverse(),
            ("flex-direction", "column-reverse") => d.flex_col_reverse(),
            ("flex-wrap", "wrap") => d.flex_wrap(),
            ("flex-wrap", "wrap-reverse") => d.flex_wrap_reverse(),
            ("align-content", "flex-start") => d.content_start(),
            ("align-content", "flex-end") => d.content_end(),
            ("align-content", "center") => d.content_center(),
            ("align-content", "space-between") => d.content_between(),
            ("align-content", "space-around") => d.content_around(),
            ("align-content", "space-evenly") => d.content_evenly(),
            ("align-items", "flex-start") => d.items_start(),
            ("align-items", "flex-end") => d.items_end(),
            ("align-items", "center") => d.items_center(),
            ("align-self", "flex-start") => d.self_start(),
            ("align-self", "flex-end") => d.self_end(),
            ("align-self", "center") => d.self_center(),
            ("justify-content", "flex-start") => d.justify_start(),
            ("justify-content", "flex-end") => d.justify_end(),
            ("justify-content", "center") => d.justify_center(),
            ("justify-content", "space-between") => d.justify_between(),
            ("justify-content", "space-around") => d.justify_around(),
            ("justify-content", "space-evenly") => d.justify_evenly(),
            ("flex", "none") => d.flex_shrink(0.0).flex_grow(0.0),
            ("width", v) => d.w(len(v)),
            ("height", v) => d.h(len(v)),
            ("min-width", v) => d.min_w(len(v)),
            ("max-width", v) => d.max_w(len(v)),
            ("min-height", v) => d.min_h(len(v)),
            ("max-height", v) => d.max_h(len(v)),
            ("flex-grow", v) => d.flex_grow(num(v)),
            ("flex-shrink", v) => d.flex_shrink(num(v)),
            ("flex-basis", "0") => d.flex_basis(Length::Px(0.0)),
            ("flex-basis", v) => d.flex_basis(len(v)),
            ("gap", v) => d.gap(num(v)),
            ("row-gap", v) => d.row_gap(num(v)),
            ("column-gap", v) => d.column_gap(num(v)),
            ("padding", v) => d.p(num(v)),
            ("border", v) => d.border(num(v.split(' ').next().unwrap())),
            ("margin-left", v) => d.ml(len(v)),
            ("margin-right", v) => d.mr(len(v)),
            ("margin-top", v) => d.mt(len(v)),
            ("margin-bottom", v) => d.mb(len(v)),
            ("left", v) => d.left(len(v)),
            ("top", v) => d.top(len(v)),
            other => panic!("not in the table: {other:?}"),
        };
    }
    d
}

struct Case {
    name: String,
    parent: String,
    boxed: String,
    items: Vec<String>,
    expected: Vec<[f32; 4]>,
}

fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for line in include_str!("data/flex_wrap.txt").lines() {
        if let Some(name) = line.strip_prefix("case ") {
            cases.push(Case {
                name: name.into(),
                parent: "display:block;width:400px;height:300px".into(),
                boxed: String::new(),
                items: Vec::new(),
                expected: Vec::new(),
            });
            continue;
        }
        let Some(case) = cases.last_mut() else {
            continue;
        };
        if let Some(v) = line.strip_prefix("parent ") {
            case.parent = v.into();
        } else if let Some(v) = line.strip_prefix("box ") {
            case.boxed = v.into();
        } else if let Some(v) = line.strip_prefix("item ") {
            case.items.push(v.into());
        } else if let Some(v) = line.strip_prefix("= ") {
            let n: Vec<f32> = v.split(' ').map(|s| s.parse().unwrap()).collect();
            case.expected.push([n[0], n[1], n[2], n[3]]);
        }
    }
    cases
}

/// Every case on its own document, on its own thread (the document is thread-local).
fn layout_of(case: &Case) -> Vec<RectI> {
    let (parent, boxed, items) = (case.parent.clone(), case.boxed.clone(), case.items.clone());
    std::thread::scope(|s| {
        s.spawn(move || {
            let parent_ref = node_ref();
            let box_ref = node_ref();
            let item_refs: Vec<NodeRef> = items.iter().map(|_| node_ref()).collect();
            let refs = item_refs.clone();
            let mut ui = Ui::new(move || {
                apply(div(), &parent).node_ref(parent_ref).child(
                    apply(div(), &boxed)
                        .node_ref(box_ref)
                        .children(items.iter().zip(&refs).map(|(item, r)| {
                            let (own, child) = match item.split_once(" > ") {
                                Some((own, child)) => (own, Some(child)),
                                None => (item.as_str(), None),
                            };
                            let d = apply(div(), own).node_ref(*r);
                            match child {
                                Some(c) => d.child(apply(div(), &format!("flex:none;{c}"))),
                                None => d,
                            }
                        })),
                )
            });
            ui.handle_event(WindowEvent::Resized {
                size: SizeI::new(800, 600),
                scale: Scale(120),
            });
            ui.update();
            let origin = parent_ref.rect().unwrap();
            std::iter::once(box_ref)
                .chain(item_refs)
                .map(|r| {
                    let rect = r.rect().unwrap_or_default();
                    RectI::new(
                        rect.x - origin.x,
                        rect.y - origin.y,
                        rect.width,
                        rect.height,
                    )
                })
                .collect()
        })
        .join()
        .unwrap()
    })
}

#[test]
fn same_as_chromium() {
    let mut wrong = Vec::new();
    let all = cases();
    assert!(all.len() > 60);
    for case in &all {
        let got = layout_of(case);
        // What isn't shown has no box to compare. Chromium has fractions where room is shared
        // out, and here it is whole pixels, so those can be off by what the sharing leaves over.
        let close = |g: i32, e: f32| {
            if e.fract() == 0.0 {
                g as f32 == e
            } else {
                (g as f32 - e).abs() <= 1.0
            }
        };
        let lines: Vec<String> = got
            .iter()
            .zip(&case.expected)
            .enumerate()
            .filter(|(_, (g, e))| {
                (e[2], e[3]) != (0.0, 0.0)
                    && !(close(g.x, e[0])
                        && close(g.y, e[1])
                        && close(g.width, e[2])
                        && close(g.height, e[3]))
            })
            .map(|(i, (g, e))| {
                format!(
                    "  [{i}] got {} {} {} {}, chromium {} {} {} {}",
                    g.x, g.y, g.width, g.height, e[0], e[1], e[2], e[3]
                )
            })
            .collect();
        if !lines.is_empty() || got.len() != case.expected.len() {
            wrong.push(format!("{}\n{}", case.name, lines.join("\n")));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} differ:\n{}",
        wrong.len(),
        all.len(),
        wrong.join("\n")
    );
}
