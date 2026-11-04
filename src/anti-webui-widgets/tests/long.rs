//! What is too long to build: lists, trees and tables.

mod common;

use anti_webui_core::Ui;
use anti_webui_core::event::{NamedKey, ScrollPhase, ScrollSource, WindowEvent};
use anti_webui_core::geometry::{PointF, Scale};
use anti_webui_core::prelude::*;
use anti_webui_widgets::*;
use common::*;
use std::time::Instant;

fn scroll(ui: &mut Ui, x: f32, y: f32, dx: f32, dy: f32) {
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(x, y),
        delta_x: dx,
        delta_y: dy,
        modifiers: Default::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let _ = ui.paint(Instant::now());
}

fn list(count: usize) -> Div {
    virtual_list(move || count, 20.0, |i| text(format!("Row {i}"))).h(200.0)
}

#[test]
fn a_list_only_has_the_rows_that_show() {
    let mut ui = start(|| div().w(300.0).flex().flex_col().child(list(1_000_000)));
    let _ = ui.paint(Instant::now());
    ui.update();
    let built = count_text("Row ");
    assert!(built >= 10, "only {built} rows");
    assert!(built <= 30, "{built} rows");
    assert!(has_text("Row 0"));
    assert!(!has_text("Row 100"));
}

#[test]
fn scrolling_builds_what_comes_and_drops_what_goes() {
    let mut ui = start(|| div().w(300.0).flex().flex_col().child(list(1_000_000)));
    scroll(&mut ui, 100.0, 100.0, 0.0, 100_000.0);
    ui.update();
    assert!(has_text("Row 5000"), "{}", count_text("Row "));
    assert!(!has_text("Row 0"));
    assert!(count_text("Row ") <= 30);
    // It shows where it should: the row is in the viewport.
    let (_, y) = middle_of("Row 5002");
    assert!((0.0..200.0).contains(&y), "at {y}");
}

#[test]
fn rows_touch_at_a_scale_where_rounding_adds_up() {
    // 21 logical pixels are 37.8 physical ones, which is 38.
    let scale = Scale::from_f64(1.8);
    let mut ui = start_at(
        || {
            div()
                .w(300.0)
                .flex()
                .flex_col()
                .child(virtual_list(|| 1_000_000, 21.0, |i| text(format!("Row {i}"))).h(200.0))
        },
        scale,
    );
    scroll(&mut ui, 100.0, 100.0, 0.0, 500_000.0);
    ui.update();
    let row = scale.px(21.0);
    let mut tops: Vec<(usize, i32)> = (0..40_000)
        .filter_map(|i| {
            let id = text_node(&format!("Row {i}"))?;
            let r = anti_webui_core::tree::with_doc(|d| d.window_rect(id))?;
            Some((i, r.y))
        })
        .collect();
    tops.sort();
    assert!(tops.len() >= 5, "{tops:?}");
    for pair in tops.windows(2) {
        assert_eq!(pair[1].1 - pair[0].1, row * (pair[1].0 - pair[0].0) as i32);
    }
}

#[test]
fn a_list_follows_its_count() {
    let count = signal(3usize);
    let mut ui =
        start(move || {
            div().w(300.0).flex().flex_col().child(
                virtual_list(move || count.get(), 20.0, |i| text(format!("Row {i}"))).h(200.0),
            )
        });
    assert_eq!(count_text("Row "), 3);
    count.set(5);
    ui.update();
    assert_eq!(count_text("Row "), 5);
    count.set(1);
    ui.update();
    assert_eq!(count_text("Row "), 1);
}

fn tree() -> Vec<TreeItem<&'static str>> {
    vec![
        tree_item("a", "Alpha")
            .child(tree_item("a1", "Alpha one").child(tree_item("a11", "Deep")))
            .child(tree_item("a2", "Alpha two")),
        tree_item("b", "Beta"),
    ]
}

fn start_tree(selected: Signal<Option<&'static str>>) -> Ui {
    start(move || {
        div()
            .w(300.0)
            .flex()
            .flex_col()
            .child(tree_view(tree(), selected).h(200.0))
    })
}

#[test]
fn a_tree_shows_what_is_open_and_selects_what_is_pressed() {
    let selected = signal(None);
    let mut ui = start_tree(selected);
    assert!(has_text("Alpha"));
    assert!(!has_text("Alpha one"));
    click_text(&mut ui, "Beta");
    assert_eq!(selected.get(), Some("b"));
    // The chevron is at the start of the row.
    let (_, y) = middle_of("Alpha");
    click(&mut ui, 12.0, y);
    assert!(has_text("Alpha one"), "the chevron opens it");
    assert_eq!(selected.get(), Some("b"), "and doesn't select");
    click(&mut ui, 12.0, y);
    assert!(!has_text("Alpha one"));
}

#[test]
fn a_tree_goes_with_the_keys() {
    let selected = signal(Some("a"));
    let mut ui = start_tree(selected);
    click_text(&mut ui, "Alpha");
    key(&mut ui, NamedKey::ArrowRight);
    assert!(has_text("Alpha one"), "right opens");
    assert_eq!(selected.get(), Some("a"));
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(selected.get(), Some("a1"), "right again goes in");
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(selected.get(), Some("a2"));
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(selected.get(), Some("b"));
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(selected.get(), Some("b"), "the end");
    key(&mut ui, NamedKey::Home);
    assert_eq!(selected.get(), Some("a"));
    key(&mut ui, NamedKey::End);
    assert_eq!(selected.get(), Some("b"));
    key(&mut ui, NamedKey::ArrowUp);
    key(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(selected.get(), Some("a"), "left goes to the parent");
    key(&mut ui, NamedKey::ArrowLeft);
    assert!(!has_text("Alpha one"), "left closes");
    key(&mut ui, NamedKey::Enter);
    assert!(has_text("Alpha one"), "enter opens");
    key(&mut ui, NamedKey::Space);
    assert!(!has_text("Alpha one"), "space closes");
}

#[derive(Clone)]
struct Row {
    name: String,
    n: i32,
}

fn rows(count: i32) -> Vec<Row> {
    (0..count)
        .map(|n| Row {
            name: format!("Name {n}"),
            n: (n * 7) % 10,
        })
        .collect()
}

fn start_table(data: Signal<Vec<Row>>, selected: Signal<Option<usize>>) -> Ui {
    start(move || {
        div().w(500.0).flex().flex_col().child(
            table(
                data,
                vec![
                    column("Name", |r: &Row| r.name.clone())
                        .width(200.0)
                        .sort_by(|a: &Row, b: &Row| a.name.cmp(&b.name)),
                    column("N", |r: &Row| format!("N={}", r.n))
                        .width(100.0)
                        .sort_by(|a: &Row, b: &Row| a.n.cmp(&b.n)),
                    column("Plain", |_: &Row| "x".to_owned())
                        .width(100.0)
                        .fill(),
                ],
                selected,
            )
            .h(300.0),
        )
    })
}

/// What is in the column of names, from the top.
fn names_in_order() -> Vec<String> {
    let mut found: Vec<(i32, String)> = anti_webui_core::tree::with_doc(|d| {
        d.nodes
            .iter()
            .filter_map(|(id, n)| match &n.kind {
                anti_webui_core::tree::NodeKind::Text(t) if t.text.starts_with("Name ") => {
                    Some((d.window_rect(id)?.y, t.text.clone()))
                }
                _ => None,
            })
            .collect()
    });
    found.sort();
    found.into_iter().map(|f| f.1).collect()
}

#[test]
fn a_table_of_many_rows_only_builds_what_shows() {
    let mut ui = start_table(signal(rows(100_000)), signal(None));
    let _ = ui.paint(Instant::now());
    ui.update();
    let built = count_text("Name ");
    assert!((5..=40).contains(&built), "{built} rows");
}

#[test]
fn pressing_a_title_sorts_one_way_then_the_other_then_not() {
    let mut ui = start_table(signal(rows(10)), signal(None));
    let _ = ui.paint(Instant::now());
    ui.update();
    assert_eq!(names_in_order()[0], "Name 0");
    click_text(&mut ui, "N");
    let _ = ui.paint(Instant::now());
    // n = n * 7 % 10, so the smallest are 0 (0), 1 (3) and 2 (6).
    assert_eq!(names_in_order()[..2], ["Name 0", "Name 3"]);
    click_text(&mut ui, "N");
    let _ = ui.paint(Instant::now());
    assert_eq!(names_in_order()[0], "Name 7", "9 is the largest");
    click_text(&mut ui, "N");
    let _ = ui.paint(Instant::now());
    assert_eq!(names_in_order()[..2], ["Name 0", "Name 1"]);
}

#[test]
fn a_table_follows_its_rows() {
    let data = signal(rows(2));
    let mut ui = start_table(data, signal(None));
    assert_eq!(count_text("Name "), 2);
    data.update(|d| {
        d.push(Row {
            name: "Name new".to_owned(),
            n: 0,
        })
    });
    ui.update();
    assert!(has_text("Name new"));
}

#[test]
fn a_table_selects_the_row_as_it_is_in_the_data() {
    let selected = signal(None);
    let mut ui = start_table(signal(rows(10)), selected);
    let _ = ui.paint(Instant::now());
    click_text(&mut ui, "N");
    let _ = ui.paint(Instant::now());
    click_text(&mut ui, "Name 3");
    assert_eq!(selected.get(), Some(3), "not the second row it shows");
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(selected.get(), Some(6), "the next as it shows, n = 2");
}

#[test]
fn the_keys_go_through_a_table_and_keep_the_row_in_view() {
    let selected = signal(None);
    let mut ui = start_table(signal(rows(10_000)), selected);
    let _ = ui.paint(Instant::now());
    click_text(&mut ui, "Name 0");
    assert_eq!(selected.get(), Some(0));
    key(&mut ui, NamedKey::PageDown);
    let page = selected.get().unwrap();
    assert!(page > 5, "a page is {page}");
    key(&mut ui, NamedKey::PageUp);
    assert_eq!(selected.get(), Some(0));
    key(&mut ui, NamedKey::End);
    assert_eq!(selected.get(), Some(9999));
    let _ = ui.paint(Instant::now());
    ui.update();
    let (_, y) = middle_of("Name 9999");
    assert!((0.0..300.0).contains(&y), "at {y}");
    key(&mut ui, NamedKey::Home);
    assert_eq!(selected.get(), Some(0));
}

#[test]
fn dragging_the_edge_of_a_title_changes_the_width_of_the_column() {
    let mut ui = start_table(signal(rows(3)), signal(None));
    let _ = ui.paint(Instant::now());
    let (n_before, _) = middle_of("N=0");
    let id = text_node("Name 0").unwrap();
    let cell = anti_webui_core::tree::with_doc(|d| {
        let parent = d.get(id).unwrap().parent.unwrap();
        d.window_rect(parent).unwrap()
    });
    let edge = (cell.right() - 2) as f32;
    let (_, y) = middle_of("Name");
    pointer_at(&mut ui, edge, y);
    pointer(&mut ui, edge, y, true);
    pointer_at(&mut ui, edge + 50.0, y);
    pointer(&mut ui, edge + 50.0, y, false);
    let _ = ui.paint(Instant::now());
    let (n_after, _) = middle_of("N=0");
    assert_eq!(n_after, n_before + 50.0);
}

#[test]
fn a_wide_table_scrolls_its_titles_with_its_rows() {
    let mut ui = start(move || {
        div().w(300.0).flex().flex_col().child(
            table(
                signal(rows(3)),
                vec![
                    column("Name", |r: &Row| r.name.clone()).width(400.0),
                    column("N", |r: &Row| format!("N={}", r.n)).width(400.0),
                ],
                signal(None),
            )
            .h(200.0),
        )
    });
    let _ = ui.paint(Instant::now());
    let before = middle_of("Name").0;
    let cell = middle_of("Name 0").0;
    scroll(&mut ui, 150.0, 100.0, 120.0, 0.0);
    ui.update();
    let _ = ui.paint(Instant::now());
    ui.update();
    assert_eq!(middle_of("Name 0").0, cell - 120.0, "the rows moved");
    assert_eq!(
        middle_of("Name").0,
        before - 120.0,
        "and the titles with them"
    );
}
