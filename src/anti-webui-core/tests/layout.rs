//! Layout tests for the physical integer pixel model. Each test runs on its own thread, so the
//! thread-local document starts empty.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::with_doc;

fn mount<V: IntoView>(size: (i32, i32), scale: Scale, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(size.0, size.1),
        scale,
    });
    ui.update();
    ui
}

fn rect(r: NodeRef) -> RectI {
    r.rect().unwrap()
}

#[test]
fn same_spec_same_size_at_fractional_scale() {
    let refs: Vec<NodeRef> = (0..3).map(|_| node_ref()).collect();
    let r2 = refs.clone();
    let _ui = mount((400, 100), Scale(180), move || {
        div().flex().flex_row().children(
            r2.iter()
                .map(|r| div().w(33.0).h(10.0).border(1.0).node_ref(*r)),
        )
    });
    let rects: Vec<RectI> = refs.iter().map(|r| rect(*r)).collect();
    assert_eq!(
        rects.iter().map(|r| r.width).collect::<Vec<_>>(),
        vec![50, 50, 50]
    );
    assert_eq!(
        rects.iter().map(|r| r.x).collect::<Vec<_>>(),
        vec![0, 50, 100]
    );
    let borders = with_doc(|d| d.get(refs[1].get().unwrap()).unwrap().resolved.border);
    assert_eq!(
        (borders.top, borders.right, borders.bottom, borders.left),
        (2, 2, 2, 2)
    );
}

#[test]
fn flex_grow_fills_exactly() {
    let refs: Vec<NodeRef> = (0..3).map(|_| node_ref()).collect();
    let r2 = refs.clone();
    let _ui = mount((100, 50), Scale(150), move || {
        div()
            .flex()
            .flex_row()
            .w(pct(100.0))
            .children(r2.iter().map(|r| div().flex_grow(1.0).node_ref(*r)))
    });
    let rects: Vec<RectI> = refs.iter().map(|r| rect(*r)).collect();
    assert_eq!(
        rects.iter().map(|r| r.width).collect::<Vec<_>>(),
        vec![34, 33, 33]
    );
    assert_eq!(rects[2].right(), 100);
}

#[test]
fn column_with_gap_padding_and_center() {
    let a = node_ref();
    let b = node_ref();
    let _ui = mount((200, 200), Scale(120), move || {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .h(pct(100.0))
            .p(10.0)
            .gap(5.0)
            .child(div().w(50.0).h(20.0).node_ref(a))
            .child(div().w(30.0).h(20.0).node_ref(b))
    });
    // Content 20 + 5 + 20 = 45 inside 180 → lead (180-45)/2 = 67.
    assert_eq!(rect(a), RectI::new(75, 77, 50, 20));
    assert_eq!(rect(b), RectI::new(85, 102, 30, 20));
}

#[test]
fn absolute_and_block() {
    let a = node_ref();
    let b = node_ref();
    let _ui = mount((300, 300), Scale(120), move || {
        div()
            .relative()
            .h(100.0)
            .child(div().h(30.0).node_ref(a))
            .child(
                div()
                    .absolute()
                    .right(10.0)
                    .bottom(10.0)
                    .w(20.0)
                    .h(20.0)
                    .node_ref(b),
            )
    });
    assert_eq!(rect(a), RectI::new(0, 0, 300, 30));
    assert_eq!(rect(b), RectI::new(270, 70, 20, 20));
}

#[test]
fn text_measures_and_wraps() {
    let t = node_ref();
    let _ui = mount((120, 400), Scale(120), move || {
        div().w(120.0).child(
            div()
                .node_ref(t)
                .child(text("The quick brown fox jumps over the lazy dog")),
        )
    });
    let r = rect(t);
    assert_eq!(r.width, 120);
    assert!(r.height > 30, "text should wrap onto multiple lines: {r:?}");
}

#[test]
fn reactive_text_updates_layout() {
    let t = node_ref();
    let s = signal(String::from("a"));
    let mut ui = mount((400, 400), Scale(120), move || {
        div()
            .flex()
            .flex_row()
            .child(div().node_ref(t).child(text(move || s.get())))
    });
    let w1 = rect(t).width;
    s.set("a much longer piece of text".into());
    ui.update();
    let w2 = rect(t).width;
    assert!(w2 > w1 * 5, "{w1} -> {w2}");
}

#[test]
fn shrink_respects_min_content() {
    let a = node_ref();
    let b = node_ref();
    let _ui = mount((100, 100), Scale(120), move || {
        div()
            .flex()
            .flex_row()
            .w(100.0)
            .child(div().w(80.0).node_ref(a))
            .child(div().w(80.0).node_ref(b))
    });
    // Empty boxes have zero min-content, so both shrink equally (weights = shrink × basis).
    assert_eq!(rect(a).width + rect(b).width, 100);
    assert_eq!(rect(a).width, 50);
}

/// Every node's rectangle, plus each text node's painted line count.
fn snapshot() -> Vec<(RectI, usize)> {
    with_doc(|d| {
        let mut v: Vec<_> = d
            .preorder()
            .into_iter()
            .map(|id| {
                let n = d.get(id).unwrap();
                let lines = match &n.kind {
                    anti_webui_core::tree::NodeKind::Text(t) => t.layout.layout.len(),
                    _ => 0,
                };
                (n.abs, lines)
            })
            .collect();
        v.shrink_to_fit();
        v
    })
}

#[test]
fn incremental_layout_matches_full_layout() {
    let width = signal(300.0f32);
    let label = signal(String::from("short"));
    let show_extra = signal(false);
    let items = signal(vec![1, 2, 3]);
    let mut ui = mount((800, 600), Scale(150), move || {
        div()
            .flex()
            .flex_col()
            .gap(4.0)
            .p(8.0)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(6.0)
                    .child(div().dyn_style(move |s| s.w(width.get())).bg(rgb(0xeeeeee)).h(20.0))
                    .child(div().flex_1().child(text("A paragraph that wraps when the flexible column becomes narrow enough to force it.")))
                    .child(div().child(text(move || label.get()))),
            )
            .child(show(move || show_extra.get(), || div().h(30.0).child("extra")))
            .child(for_each(move || items.get(), |i| *i, |i| div().p(2.0).child(text(format!("row {i}")))))
            .child(div().relative().h(40.0).child(div().absolute().dyn_style(move |s| s.left(width.get() / 3.0)).w(10.0).h(10.0)))
    });
    let steps: Vec<Box<dyn Fn()>> = vec![
        Box::new(move || width.set(500.0)),
        Box::new(move || label.set("a considerably longer label".into())),
        Box::new(move || show_extra.set(true)),
        Box::new(move || items.set(vec![3, 1, 4, 5])),
        Box::new(move || width.set(120.0)),
        Box::new(move || show_extra.set(false)),
        Box::new(move || label.set("x".into())),
    ];
    for step in steps {
        step();
        ui.update();
        let incremental = snapshot();
        with_doc(|d| d.invalidate_all_layout());
        ui.update();
        let full = snapshot();
        assert_eq!(incremental, full);
    }
}

#[test]
fn paint_only_changes_skip_layout() {
    let color = signal(rgb(0xff0000));
    let width = signal(10.0f32);
    let mut ui = mount((400, 300), Scale(120), move || {
        div()
            .dyn_style(move |s| s.bg(color.get()).w(width.get()))
            .child(text("hello"))
    });
    color.set(rgb(0x00ff00));
    with_doc(|d| {
        anti_webui_core::layout::resolve_styles(d);
        assert!(!d.dirty.layout, "a color change must not trigger layout");
    });
    width.set(20.0);
    with_doc(|d| {
        anti_webui_core::layout::resolve_styles(d);
        assert!(d.dirty.layout, "a width change must trigger layout");
    });
    ui.update();
}
