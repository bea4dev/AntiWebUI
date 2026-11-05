//! Shapes in the tree, and boxes that wrap.
//!
//! ```sh
//! cargo run --release -p anti-webui --example svg
//! ```
//!
//! Point at the boxes of the diagram to light them up, and click on them to add one. An `.svg` file
//! after the name shows it as shapes.

use anti_webui::prelude::svg::kurbo;
use anti_webui::prelude::*;
use std::f64::consts::TAU;

fn card(title: &str, child: impl IntoView) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(8.0)
        .p(12.0)
        .rounded(12.0)
        .bg(Color::WHITE)
        .shadow(BoxShadow {
            offset_x: 0.0,
            offset_y: 1.0,
            blur: 4.0,
            spread: 0.0,
            color: Color::from_rgba(0x00000030),
            inset: false,
        })
        .child(
            text(title)
                .text_size(13.0)
                .text_color(Color::from_rgb(0x555555)),
        )
        .child(child)
}

fn star(points: usize, outer: f64, inner: f64, turn: f64) -> kurbo::BezPath {
    let mut p = kurbo::BezPath::new();
    for i in 0..points * 2 {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = turn + i as f64 / (points * 2) as f64 * TAU;
        let at = (70.0 + r * a.cos(), 70.0 + r * a.sin());
        if i == 0 {
            p.move_to(at);
        } else {
            p.line_to(at);
        }
    }
    p.close_path();
    p
}

fn shapes() -> impl IntoView {
    svg()
        .w(180.0)
        .h(140.0)
        .child(svg::rounded_rect(8.0, 8.0, 70.0, 50.0, 10.0).fill(Color::from_rgb(0x3b82f6)))
        .child(
            svg::circle(125.0, 35.0, 28.0)
                .fill(Color::from_rgb(0xfbbf24))
                .stroke(Color::from_rgb(0x92400e))
                .stroke_width(3.0),
        )
        .child(
            svg::path("M10 130 C 40 70, 70 150, 100 90 S 150 80, 170 125")
                .fill_none()
                .stroke(Color::from_rgb(0xdc2626))
                .stroke_width(4.0)
                .line_cap(LineCap::Round),
        )
}

fn strokes() -> impl IntoView {
    let line = |y: f64| svg::line(10.0, y, 170.0, y).stroke_width(5.0);
    svg()
        .w(180.0)
        .h(140.0)
        .fill_none()
        .stroke(Color::from_rgb(0x334155))
        .child(line(20.0).dash_array([14.0, 8.0]))
        .child(line(45.0).dash_array([1.0, 10.0]).line_cap(LineCap::Round))
        .child(
            svg::polyline([(15.0, 125.0), (50.0, 65.0), (85.0, 125.0)])
                .stroke_width(10.0)
                .line_join(LineJoin::Miter),
        )
        .child(
            svg::polyline([(100.0, 125.0), (135.0, 65.0), (170.0, 125.0)])
                .stroke_width(10.0)
                .line_join(LineJoin::Round),
        )
}

fn gradients() -> impl IntoView {
    svg()
        .w(180.0)
        .h(140.0)
        .child(
            svg::rounded_rect(8.0, 8.0, 80.0, 124.0, 12.0).fill(
                Gradient::linear(0.0, 0.0, 0.0, 1.0)
                    .stop(0.0, Color::from_rgb(0xf472b6))
                    .stop(1.0, Color::from_rgb(0x6366f1)),
            ),
        )
        .child(
            svg::circle(135.0, 70.0, 38.0).fill(
                Gradient::radial(0.5, 0.5, 0.5)
                    .stop(0.0, Color::WHITE)
                    .stop(1.0, Color::from_rgb(0x059669)),
            ),
        )
}

/// A shape that changes every frame: each one is made again.
fn morphing() -> impl IntoView {
    let t = signal(0.0f64);
    set_ticker(t);
    svg().w(140.0).h(140.0).child(
        svg::path(move || {
            let t = t.get();
            star(
                7,
                55.0 + 10.0 * t.sin(),
                28.0 + 14.0 * (t * 1.3).cos(),
                t * 0.4,
            )
        })
        .fill(Color::from_rgb(0xa855f7)),
    )
}

fn set_ticker(t: Signal<f64>) {
    fn tick(t: Signal<f64>) {
        t.update(|v| *v += 0.03);
        set_timeout(std::time::Duration::from_millis(16), move || tick(t));
    }
    tick(t);
}

fn diagram() -> impl IntoView {
    let boxes = signal(6usize);
    let (bw, bh) = (92.0, 40.0);
    let place = |i: usize| (10.0 + (i % 4) as f64 * 124.0, 10.0 + (i / 4) as f64 * 76.0);
    let label = Style::default().text_size(13.0);
    svg().w(510.0).h(320.0).child(dyn_view(move || {
        let n = boxes.get();
        let label = label.clone();
        svg::group()
            .child(
                svg::group()
                    .fill_none()
                    .stroke(Color::from_rgb(0x64748b))
                    .stroke_width(1.5)
                    .children((1..n).map(|i| {
                        // From the one before it along a row, and from the row above to the next.
                        let mut p = kurbo::BezPath::new();
                        if i % 4 == 0 {
                            let (ax, ay) = place(i - 4);
                            let (bx, by) = place(i);
                            let (a, b) = ((ax + bw * 0.5, ay + bh), (bx + bw * 0.5, by));
                            p.move_to(a);
                            p.curve_to((a.0, a.1 + 18.0), (b.0, b.1 - 18.0), b);
                        } else {
                            let (ax, ay) = place(i - 1);
                            let (bx, by) = place(i);
                            p.move_to((ax + bw, ay + bh * 0.5));
                            p.line_to((bx, by + bh * 0.5));
                        }
                        svg::path(p).dash_array(if i % 3 == 0 {
                            vec![5.0, 4.0]
                        } else {
                            Vec::new()
                        })
                    })),
            )
            .children((0..n).map(move |i| {
                let (x, y) = place(i);
                let style = label.clone();
                svg::group()
                    .child(
                        svg::rounded_rect(x, y, bw, bh, 8.0)
                            .fill(Color::from_rgb(0xdbeafe))
                            .stroke(Color::from_rgb(0x2563eb))
                            .hover(|s| s.fill(Color::from_rgb(0xfde68a)))
                            .transition(Transition::ms(120))
                            .cursor(CursorIcon::Pointer)
                            .on_click(move |_| boxes.update(|n| *n = (*n + 1).min(16))),
                    )
                    .child(
                        svg::text(format!("Step {}", i + 1))
                            .style(&style)
                            .at((x + bw * 0.5) as f32, (y + bh * 0.5) as f32)
                            .anchor(svg::TextAnchor::Middle)
                            .baseline(svg::TextBaseline::Middle)
                            .fill(Color::from_rgb(0x1e3a8a))
                            .pointer_events_none(),
                    )
            }))
    }))
}

fn main() {
    let file = std::env::args().nth(1);
    App::new()
        .title("SVG")
        .size(1100, 800)
        .run(move || {
            div()
                .size_full()
                .overflow_y_scroll()
                .bg(Color::from_rgb(0xeef2f7))
                .child(
                    // More cards than fit in a row: they go on to the next.
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(16.0)
                        .p(16.0)
                        .items_start()
                        .child(card("Shapes", shapes()))
                        .child(card("Outlines", strokes()))
                        .child(card("Gradients", gradients()))
                        .child(card("Changing every frame", morphing()))
                        .child(card("Boxes and arrows", diagram()))
                        .children(file.iter().map(|path| {
                            let data = std::fs::read(path).unwrap_or_default();
                            let shown = match svg::parse(&data) {
                                Ok(s) => s.into_view(),
                                Err(e) => text(format!("{path}: {e}")).into_view(),
                            };
                            card("From the file", shown)
                        })),
                )
        })
        .unwrap();
}
