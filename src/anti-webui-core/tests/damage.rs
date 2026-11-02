//! Damage tracking on real documents: small changes must damage small areas.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::paint::CARET_BLINK;
use anti_webui_core::prelude::*;
use std::time::{Duration, Instant};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale: Scale(180),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
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

fn center(r: RectI) -> PointF {
    PointF::new(
        r.x as f32 + r.width as f32 / 2.0,
        r.y as f32 + r.height as f32 / 2.0,
    )
}

fn contains(outer: RectI, inner: RectI) -> bool {
    outer.intersect(&inner) == inner
}

const PARAGRAPH: &str = "The quick brown fox jumps over the lazy dog. \
    The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog.";

#[test]
fn unchanged_frames_have_no_damage() {
    let mut ui = mount(|| div().p(10.0).child(text(PARAGRAPH)));
    let now = Instant::now();
    let a = ui.paint(now);
    ui.update();
    let b = ui.paint(now + Duration::from_secs(1));
    assert!(damage::diff(&a, &b).is_empty());
}

#[test]
fn caret_blink_damages_only_the_caret() {
    let field = node_ref();
    let mut ui = mount(move || {
        div()
            .p(10.0)
            .gap(8.0)
            .flex_col()
            .child(text(PARAGRAPH))
            .child(
                text_input()
                    .w(300.0)
                    .node_ref(field)
                    .value(signal("hello".to_string())),
            )
    });
    let r = field.rect().unwrap();
    click(&mut ui, center(r));
    let now = Instant::now();
    let on = ui.paint(now);
    let off = ui.paint(now + CARET_BLINK + Duration::from_millis(10));
    let d = damage::diff(&on, &off);
    let b = d.bounds();
    assert!(!d.is_empty());
    assert!(contains(r, b), "{b:?} not inside {r:?}");
    assert!(b.width <= 8, "caret damage too wide: {b:?}");
}

#[test]
fn hover_damages_only_the_hovered_element() {
    let target = node_ref();
    let mut ui = mount(move || {
        div()
            .p(10.0)
            .gap(8.0)
            .flex_col()
            .child(text(PARAGRAPH))
            .child(
                div()
                    .w(120.0)
                    .h(32.0)
                    .bg(rgb(0xeeeeee))
                    .hover(|s| s.bg(rgb(0xdddddd)))
                    .node_ref(target),
            )
    });
    let now = Instant::now();
    let before = ui.paint(now);
    let r = target.rect().unwrap();
    ui.handle_event(WindowEvent::PointerMoved {
        position: center(r),
        modifiers: Modifiers::default(),
    });
    ui.update();
    let after = ui.paint(now);
    assert_eq!(damage::diff(&before, &after).rects(), &[r]);
}

#[test]
fn selection_damages_the_highlight_not_the_paragraph() {
    let para = node_ref();
    let mut ui = mount(move || {
        div()
            .p(10.0)
            .w(300.0)
            .child(div().node_ref(para).child(text(PARAGRAPH)))
    });
    let r = para.rect().unwrap();
    let now = Instant::now();
    let before = ui.paint(now);
    // Double-click a word on the first line.
    let p = PointF::new(r.x as f32 + 20.0, r.y as f32 + 5.0);
    for _ in 0..2 {
        click(&mut ui, p);
    }
    let after = ui.paint(now);
    let d = damage::diff(&before, &after);
    assert!(!d.is_empty());
    assert!(
        d.bounds().height * 2 < r.height,
        "{:?} vs {r:?}",
        d.bounds()
    );
}
