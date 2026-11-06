//! `rich_text`: spans with styles of their own and nodes among the words, in one text node.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, PlatformRequest};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::text::selection;
use anti_webui_core::tree::{NodeId, NodeKind, with_doc};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

fn mount<V: IntoView>(width: i32, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(width, 400),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

/// The text nodes whose text starts with `prefix`.
fn texts(prefix: &str) -> Vec<NodeId> {
    with_doc(|d| {
        let mut ids: Vec<NodeId> = d
            .nodes
            .iter()
            .filter(|(_, n)| matches!(&n.kind, NodeKind::Text(t) if t.text.starts_with(prefix)))
            .map(|(id, _)| id)
            .collect();
        ids.sort_by_key(|id| d.nodes[*id].doc_order);
        ids
    })
}

fn rect_of(id: NodeId) -> RectI {
    with_doc(|d| d.window_rect(id).unwrap())
}

fn move_to(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
}

fn click(ui: &mut Ui, p: PointF) {
    move_to(ui, p);
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

fn last_cursor(ui: &mut Ui) -> Option<CursorIcon> {
    ui.take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::SetCursor(c) => Some(c),
            _ => None,
        })
        .last()
}

fn quads(scene: &Scene, out: &mut Vec<(RectI, Color, [f32; 4])>) {
    for p in &scene.primitives {
        match p {
            Primitive::Quad(q) => out.push((q.rect, q.background, q.radii)),
            Primitive::Layer(l) => quads(&l.content, out),
            _ => {}
        }
    }
}

#[test]
fn a_paragraph_is_one_text_node_with_its_spans() {
    let _ui = mount(600, || {
        div().child(rich_text((
            "plain ",
            span("big").text_size(28.0),
            " and ",
            span("bold").bold(),
        )))
    });
    let ids = texts("plain");
    assert_eq!(ids.len(), 1);
    let (content, spans) = with_doc(|d| {
        let NodeKind::Text(t) = &d.nodes[ids[0]].kind else {
            unreachable!()
        };
        let rich = t.rich.as_ref().unwrap();
        (
            t.text.clone(),
            rich.spans
                .iter()
                .map(|s| s.range.clone())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(content, "plain big and bold");
    assert_eq!(spans, [6..9, 14..18]);
}

#[test]
fn a_bigger_span_makes_its_line_taller() {
    let _ui = mount(600, || {
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(text("plain big and bold"))
            .child(rich_text((
                "plain ",
                span("big").text_size(28.0),
                " and bold",
            )))
    });
    let [plain, rich] = texts("plain")[..] else {
        panic!()
    };
    assert!(rect_of(rich).height > rect_of(plain).height + 8);
    assert!(rect_of(rich).width > rect_of(plain).width + 10);
}

#[test]
fn padding_makes_room_on_the_sides_of_a_span() {
    let _ui = mount(600, || {
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(rich_text(("ab", span("cd"), "ef")))
            .child(rich_text(("ab", span("cd").px(10.0), "ef")))
    });
    let [without, with] = texts("ab")[..] else {
        panic!()
    };
    assert_eq!(rect_of(with).width, rect_of(without).width + 20);
}

#[test]
fn nodes_go_among_the_words_and_wrap_with_them() {
    let pic = node_ref();
    let _ui = mount(600, move || {
        div().child(rich_text((
            "before ",
            inline(div().w(20.0).h(30.0).node_ref(pic)),
            " after",
        )))
    });
    let id = texts("before")[0];
    let text = rect_of(id);
    let r = pic.rect().unwrap();
    assert_eq!((r.width, r.height), (20, 30));
    assert!(r.x > text.x + 20, "after the first word: {r:?} in {text:?}");
    assert!(text.height >= 30, "the line holds it: {text:?}");
    // On the baseline: its bottom is where the text of the line sits.
    assert!(r.bottom() <= text.bottom() && r.bottom() >= text.bottom() - 8);

    // Too narrow for both words and the box: the box goes to a line of its own.
    let pic = node_ref();
    let _ui = mount(600, move || {
        div().w(60.0).child(rich_text((
            "before ",
            inline(div().w(50.0).h(10.0).node_ref(pic)),
            " after",
        )))
    });
    let text = rect_of(texts("before")[0]);
    let r = pic.rect().unwrap();
    assert!(r.y > text.y + 5, "on the second line: {r:?} in {text:?}");
    assert!(r.x - text.x < 5, "at the start of it: {r:?} in {text:?}");
}

#[test]
fn a_node_among_the_words_is_copied_as_its_alt() {
    let _ui = mount(600, || {
        div().child(rich_text((
            "party ",
            inline(div().w(16.0).h(16.0)).alt(":party:"),
            " time",
        )))
    });
    let copied = with_doc(|d| {
        let sel = selection::select_all(d).unwrap();
        selection::selected_text(d, &sel)
    });
    assert_eq!(copied, "party :party: time");
}

#[test]
fn a_span_is_clicked_and_shows_a_hand() {
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let mut ui = mount(600, move || {
        div().child(rich_text((
            "go to ",
            span("the link")
                .text_color(Color::from_rgb(0x0969da))
                .hover(|s| s.underline())
                .on_click(move || c.set(c.get() + 1)),
            " now",
        )))
    });
    let id = texts("go to")[0];
    let r = rect_of(id);
    let start = with_doc(|d| {
        let NodeKind::Text(t) = &d.nodes[id].kind else {
            unreachable!()
        };
        t.layout.caret_rect(6, Default::default(), 1).x
    });
    let on_link = PointF::new((r.x + start + 10) as f32, (r.y + r.height / 2) as f32);
    let off_link = PointF::new((r.x + 2) as f32, (r.y + r.height / 2) as f32);

    move_to(&mut ui, on_link);
    assert_eq!(last_cursor(&mut ui), Some(CursorIcon::Pointer));
    let hovered = with_doc(|d| d.state.hovered_span);
    assert_eq!(hovered, Some((id, 0)));
    move_to(&mut ui, off_link);
    assert_eq!(last_cursor(&mut ui), Some(CursorIcon::Text));

    click(&mut ui, off_link);
    assert_eq!(clicks.get(), 0);
    click(&mut ui, on_link);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn the_background_of_a_span_is_as_tall_as_its_font() {
    let code = Color::from_rgb(0xeff1f3);
    let mut ui = mount(600, move || {
        div().child(
            rich_text(("use ", span("code").bg(code).rounded(4.0).px(3.0), " here"))
                .line_height(LineHeight::Relative(3.0)),
        )
    });
    let text = rect_of(texts("use")[0]);
    let mut found = Vec::new();
    quads(&ui.paint(Instant::now()), &mut found);
    let (r, _, radii) = found
        .into_iter()
        .find(|(_, c, _)| *c == code)
        .expect("the background is painted");
    assert_eq!(radii, [4.0; 4]);
    assert!(r.height < text.height / 2, "{r:?} in a line of {text:?}");
    assert!(r.x > text.x && r.right() < text.right());
}

#[test]
fn a_node_among_the_words_takes_clicks() {
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let pic = node_ref();
    let mut ui = mount(600, move || {
        div().child(rich_text((
            "see ",
            inline(
                div()
                    .w(30.0)
                    .h(20.0)
                    .node_ref(pic)
                    .on_click(move |_| c.set(c.get() + 1)),
            ),
            " here",
        )))
    });
    let r = pic.rect().unwrap();
    click(&mut ui, PointF::new((r.x + 5) as f32, (r.y + 5) as f32));
    assert_eq!(clicks.get(), 1);
}
