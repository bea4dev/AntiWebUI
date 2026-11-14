//! `drawing`: its size, where its baseline goes among words, and what it paints. See
//! `docs/design/40-math.md`.

use anti_webui_core::Ui;
use anti_webui_core::drawing::{DrawingItem, Glyph, PathGeometry, static_font};
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::Primitive;
use anti_webui_core::style::FillRule;
use std::sync::Arc;
use std::time::Instant;

fn mount<V: IntoView>(scale: Scale, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale,
    });
    ui.update();
    ui
}

/// 4 em wide, 1 em above the baseline and 0.5 below, with a line along the baseline.
fn sample(ascent: f32) -> Arc<Drawing> {
    let mut d = Drawing::new(4.0, ascent, 0.5);
    d.items.push(DrawingItem::Rect {
        x: 0.0,
        y: -0.01,
        width: 4.0,
        height: 0.02,
        color: None,
    });
    Arc::new(d)
}

#[test]
fn it_is_as_big_as_its_drawing_at_the_text_size() {
    let r = node_ref();
    let _ui = mount(Scale(120), || {
        div()
            .text_size(20.0)
            .child(drawing(sample(1.0)).em_scale(1.5).node_ref(r))
    });
    // 1 em is 30 px: 4 em by 1.5 em.
    let rect = r.rect().unwrap();
    assert_eq!((rect.width, rect.height), (120, 45));

    // At 1.5 times, in whole device pixels.
    let r = node_ref();
    let _ui = mount(Scale(180), || {
        div()
            .text_size(20.0)
            .child(drawing(sample(1.0)).node_ref(r))
    });
    let rect = r.rect().unwrap();
    assert_eq!((rect.width, rect.height), (120, 45));
}

#[test]
fn among_words_its_baseline_is_theirs() {
    let (a, b) = (node_ref(), node_ref());
    let _ui = mount(Scale(120), || {
        rich_text((
            "x ",
            inline(drawing(sample(1.0)).node_ref(a)),
            " y ",
            inline(drawing(sample(2.5)).node_ref(b)),
        ))
        .text_size(20.0)
    });
    let (ra, rb) = (a.rect().unwrap(), b.rect().unwrap());
    // Their baselines, `ascent` below their tops, are on the same line.
    assert_eq!(ra.y + 20, rb.y + 50);
    assert_eq!((ra.height, rb.height), (30, 60));
}

#[test]
fn it_paints_in_the_text_color_unless_told_otherwise() {
    // Painting doesn't read the font: the renderer does.
    let font = static_font(b"a font", 0);
    let mut d = Drawing::new(2.0, 1.0, 0.2);
    d.items.push(DrawingItem::Glyphs {
        font: font.clone(),
        size: 1.0,
        color: None,
        glyphs: vec![Glyph {
            id: 3,
            x: 0.25,
            y: 0.0,
        }],
    });
    d.items.push(DrawingItem::Rect {
        x: 0.0,
        y: -0.5,
        width: 2.0,
        height: 0.01,
        color: Some(rgb(0xff0000)),
    });
    let mut square = kurbo::BezPath::new();
    square.move_to((0.0, -1.0));
    square.line_to((1.0, -1.0));
    square.line_to((1.0, 0.0));
    square.close_path();
    d.items.push(DrawingItem::Path {
        geometry: PathGeometry::new(square),
        rule: FillRule::EvenOdd,
        stroke: None,
        color: None,
    });
    let d = Arc::new(d);
    let r = node_ref();
    let mut ui = mount(Scale(120), || {
        div().p(10.0).child(
            drawing(d)
                .text_size(20.0)
                .text_color(rgb(0x0000ff))
                .node_ref(r),
        )
    });
    let rect = r.rect().unwrap();
    let baseline = rect.y + 20;
    let scene = ui.paint(Instant::now());
    let mut seen = (false, false, false);
    for p in &scene.primitives {
        match p {
            Primitive::Glyphs(g) => {
                assert_eq!(g.color, rgb(0x0000ff));
                assert_eq!(g.size, 20.0);
                assert_eq!(g.glyphs[0].x, rect.x as f32 + 5.0);
                assert_eq!(g.glyphs[0].y, baseline as f32);
                seen.0 = true;
            }
            // A line thinner than a pixel is a pixel, on whole pixels.
            Primitive::Quad(q) if q.background == rgb(0xff0000) => {
                assert_eq!(q.rect, RectI::new(rect.x, baseline - 10, 40, 1));
                seen.1 = true;
            }
            Primitive::Path(p) => {
                assert_eq!(
                    p.paint,
                    anti_webui_core::scene::PathPaint::Color(rgb(0x0000ff))
                );
                assert_eq!(p.rule, FillRule::EvenOdd);
                // From em on the baseline to the window.
                assert_eq!(p.transform.a, 20.0);
                assert_eq!(
                    (p.transform.e, p.transform.f),
                    (rect.x as f64, baseline as f64)
                );
                seen.2 = true;
            }
            _ => {}
        }
    }
    assert_eq!(seen, (true, true, true));
}

#[test]
fn a_text_of_nothing_but_a_box_is_as_wide_as_the_box() {
    let r = node_ref();
    let _ui = mount(Scale(120), || {
        div().flex().flex_row().child(
            div()
                .flex_none()
                .node_ref(r)
                .child(rich_text((inline(drawing(sample(1.0))),)).text_size(20.0)),
        )
    });
    assert_eq!(r.rect().unwrap().width, 80);
}
