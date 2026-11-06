//! `svg` and its shapes as far as the tree goes: their size, where they are, what hits them, and
//! that they follow signals. See `docs/design/25-svg.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{Modifiers, PointerButton, WindowEvent};
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::Primitive;
use anti_webui_core::style::SvgPaint;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

fn mount<V: IntoView>(size: (i32, i32), scale: Scale, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(size.0, size.1),
        scale,
    });
    ui.update();
    ui
}

const ONE: Scale = Scale(120);

fn paths(ui: &mut Ui) -> Vec<anti_webui_core::scene::PathDraw> {
    ui.paint(Instant::now())
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Path(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

fn size_of(view: impl FnOnce(NodeRef) -> View, parent: impl FnOnce(View) -> Div) -> (i32, i32) {
    let r = node_ref();
    let _ui = mount((800, 600), ONE, || parent(view(r)));
    let rect = r.rect().unwrap();
    (rect.width, rect.height)
}

fn in_block(v: View) -> Div {
    div().w(400.0).child(v)
}

/// Each of these is what Chromium makes of the same thing.
#[test]
fn sizes_are_those_of_a_browser() {
    let vb = |r: NodeRef| svg().view_box(0.0, 0.0, 24.0, 12.0).node_ref(r).into_view();
    let none = |r: NodeRef| svg().node_ref(r).into_view();
    assert_eq!(size_of(none, in_block), (300, 150));
    assert_eq!(size_of(vb, in_block), (400, 200));
    assert_eq!(size_of(vb, |v| div().w(400.0).flex().child(v)), (400, 200));
    assert_eq!(
        size_of(none, |v| div().w(400.0).flex().child(v)),
        (300, 150)
    );
    let column = |v: View| div().w(400.0).flex().flex_col().items_start().child(v);
    assert_eq!(size_of(vb, column), (400, 200));
    assert_eq!(size_of(none, column), (300, 150));
    assert_eq!(
        size_of(|r| svg().w(500.0).node_ref(r).into_view(), in_block),
        (500, 150)
    );
    assert_eq!(
        size_of(|r| svg().h(50.0).node_ref(r).into_view(), in_block),
        (300, 50)
    );
    assert_eq!(
        size_of(
            |r| svg()
                .w(100.0)
                .view_box(0.0, 0.0, 24.0, 12.0)
                .node_ref(r)
                .into_view(),
            in_block
        ),
        (100, 50)
    );
    assert_eq!(
        size_of(
            |r| svg()
                .h(100.0)
                .view_box(0.0, 0.0, 24.0, 12.0)
                .node_ref(r)
                .into_view(),
            in_block
        ),
        (200, 100)
    );
    assert_eq!(
        size_of(
            |r| svg()
                .w(100.0)
                .p(10.0)
                .view_box(0.0, 0.0, 24.0, 12.0)
                .node_ref(r)
                .into_view(),
            in_block
        ),
        (100, 60)
    );
}

#[test]
fn a_view_box_maps_to_the_box_as_svg_says() {
    let r = node_ref();
    let mut ui = mount((800, 600), ONE, || {
        div().child(
            svg()
                .w(200.0)
                .h(100.0)
                .view_box(0.0, 0.0, 10.0, 10.0)
                .child(svg::rect(0.0, 0.0, 10.0, 10.0).node_ref(r)),
        )
    });
    // In the middle, as big as fits.
    let rect = r.rect().unwrap();
    assert!((49..=50).contains(&rect.x), "{rect:?}");
    assert!((100..=103).contains(&rect.width), "{rect:?}");
    let d = &paths(&mut ui)[0];
    assert_eq!((d.transform.a, d.transform.d), (10.0, 10.0));
    assert_eq!(d.transform.e, 50.0);
}

#[test]
fn the_shapes_follow_the_scale_of_the_window() {
    let mut ui = mount((800, 600), Scale(216), || {
        svg()
            .w(100.0)
            .h(100.0)
            .child(svg::circle(10.0, 10.0, 10.0).stroke(Color::BLACK))
    });
    let d = &paths(&mut ui);
    assert_eq!(d.len(), 2, "the inside and the outline");
    assert_eq!(d[0].transform.a, 1.8);
    assert!(d[1].stroke.is_some());
}

#[test]
fn groups_add_up_and_pass_on_what_is_inherited() {
    let mut ui = mount((800, 600), ONE, || {
        svg().w(100.0).h(100.0).child(
            svg::group()
                .transform(svg::Transform::translation(10.0, 20.0))
                .fill(Color::from_rgb(0x112233))
                .child(
                    svg::group()
                        .transform(svg::Transform::scaling(2.0, 2.0))
                        .child(svg::rect(1.0, 1.0, 2.0, 2.0)),
                ),
        )
    });
    let d = &paths(&mut ui)[0];
    assert_eq!(
        (d.transform.a, d.transform.e, d.transform.f),
        (2.0, 10.0, 20.0)
    );
    assert_eq!(
        d.paint,
        anti_webui_core::scene::PathPaint::Color(Color::from_rgb(0x112233))
    );
}

#[test]
fn current_color_goes_with_the_text_color() {
    let c = signal(Color::from_rgb(0xff0000));
    let mut ui = mount((800, 600), ONE, move || {
        svg()
            .w(100.0)
            .h(100.0)
            .dyn_style(move |s| s.text_color(c.get()))
            .child(svg::rect(0.0, 0.0, 10.0, 10.0).fill_current())
    });
    let color = |ui: &mut Ui| paths(ui)[0].paint.clone();
    assert_eq!(
        color(&mut ui),
        anti_webui_core::scene::PathPaint::Color(Color::from_rgb(0xff0000))
    );
    c.set(Color::from_rgb(0x00ff00));
    ui.update();
    assert_eq!(
        color(&mut ui),
        anti_webui_core::scene::PathPaint::Color(Color::from_rgb(0x00ff00))
    );
}

fn click_at(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed: true,
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.handle_event(WindowEvent::PointerButton {
        button: PointerButton::Primary,
        pressed: false,
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
        time: 0,
    });
    ui.update();
}

#[test]
fn only_what_is_painted_is_hit() {
    let circle = Rc::new(Cell::new(0));
    let outline = Rc::new(Cell::new(0));
    let around = Rc::new(Cell::new(0));
    let (a, b, c) = (circle.clone(), outline.clone(), around.clone());
    let mut ui = mount((800, 600), ONE, move || {
        svg()
            .w(400.0)
            .h(100.0)
            .on_click(move |_| c.set(c.get() + 1))
            .child(svg::circle(50.0, 50.0, 40.0).on_click(move |_| a.set(a.get() + 1)))
            .child(
                svg::circle(200.0, 50.0, 40.0)
                    .fill_none()
                    .stroke(Color::BLACK)
                    .stroke_width(6.0)
                    .on_click(move |_| b.set(b.get() + 1)),
            )
    });
    click_at(&mut ui, 50.0, 50.0);
    assert_eq!(circle.get(), 1, "inside");
    assert_eq!(around.get(), 1, "it goes up from there, as everywhere");
    click_at(&mut ui, 15.0, 15.0);
    assert_eq!(circle.get(), 1, "in its box but outside of it");
    assert_eq!(around.get(), 2, "so it goes to what is behind");
    click_at(&mut ui, 200.0, 50.0);
    assert_eq!(outline.get(), 0, "in the middle of what is only an outline");
    assert_eq!(around.get(), 3);
    click_at(&mut ui, 240.0, 50.0);
    assert_eq!(outline.get(), 1, "on the outline");
}

#[test]
fn hits_go_through_transforms() {
    let hit = Rc::new(Cell::new(0));
    let h = hit.clone();
    let mut ui = mount((800, 600), ONE, move || {
        svg().w(200.0).h(200.0).child(
            svg::group()
                .transform(svg::Transform::translation(100.0, 100.0))
                .child(
                    svg::rect(-40.0, -5.0, 80.0, 10.0)
                        .transform(svg::Transform::rotation(45.0))
                        .on_click(move |_| h.set(h.get() + 1)),
                ),
        )
    });
    click_at(&mut ui, 120.0, 120.0);
    assert_eq!(hit.get(), 1, "along it");
    click_at(&mut ui, 120.0, 80.0);
    assert_eq!(hit.get(), 1, "in its box, across from it");
}

#[test]
fn a_shape_follows_signals() {
    let r = signal(10.0f64);
    let at = signal(0.0f64);
    let shape = node_ref();
    let mut ui = mount((800, 600), ONE, move || {
        svg().w(400.0).h(400.0).child(
            svg::path(move || {
                use kurbo_shape::*;
                Circle::new((100.0, 100.0), r.get()).to_path(0.1)
            })
            .dyn_transform(move || svg::Transform::translation(at.get(), 0.0))
            .node_ref(shape),
        )
    });
    let before = shape.rect().unwrap();
    assert!((before.width - 20).abs() <= 4, "{before:?}");
    let id = paths(&mut ui)[0].geometry.id();
    r.set(50.0);
    ui.update();
    let after = shape.rect().unwrap();
    assert!((after.width - 100).abs() <= 4, "{after:?}");
    assert_ne!(paths(&mut ui)[0].geometry.id(), id);
    at.set(30.0);
    ui.update();
    assert_eq!(shape.rect().unwrap().x, after.x + 30);
    assert_eq!(paths(&mut ui)[0].transform.e, 30.0);
}

mod kurbo_shape {
    pub use anti_webui_core::svg::kurbo::{Circle, Shape};
}

#[test]
fn a_color_changes_without_layout() {
    let c = signal(Color::BLACK);
    let mut ui = mount((800, 600), ONE, move || {
        svg()
            .w(100.0)
            .h(100.0)
            .child(svg::rect(0.0, 0.0, 10.0, 10.0).dyn_style(move |s| s.fill(c.get())))
    });
    c.set(Color::WHITE);
    anti_webui_core::tree::with_doc(|d| {
        anti_webui_core::layout::resolve_styles(d);
        assert!(!d.dirty.layout, "a color change must not trigger layout");
    });
    ui.update();
    assert_eq!(
        paths(&mut ui)[0].paint,
        anti_webui_core::scene::PathPaint::Color(Color::WHITE)
    );
}

#[test]
fn hover_works_as_on_any_node() {
    let mut ui = mount((800, 600), ONE, move || {
        svg().w(100.0).h(100.0).child(
            svg::rect(10.0, 10.0, 20.0, 20.0)
                .fill(Color::BLACK)
                .hover(|s| s.fill(Color::WHITE)),
        )
    });
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(15.0, 15.0),
        modifiers: Modifiers::default(),
    });
    ui.update();
    assert_eq!(
        paths(&mut ui)[0].paint,
        anti_webui_core::scene::PathPaint::Color(Color::WHITE)
    );
}

#[test]
fn what_is_out_of_the_box_is_cut() {
    let mut ui = mount((800, 600), ONE, || {
        svg()
            .w(10.0)
            .h(10.0)
            .child(svg::rect(0.0, 0.0, 100.0, 100.0))
    });
    let d = &paths(&mut ui)[0];
    assert_eq!(d.clip.rect, RectI::new(0, 0, 10, 10));
}

#[test]
fn what_is_scrolled_away_is_not_painted() {
    let mut ui = mount((100, 100), ONE, || {
        svg().w(100.0).h(4000.0).children(
            (0..100).map(|i| svg::rect(0.0, i as f64 * 40.0, 10.0, 10.0).fill(Color::BLACK)),
        )
    });
    assert!(paths(&mut ui).len() < 10);
}

#[test]
fn text_goes_where_it_is_told_and_scales() {
    let middle = node_ref();
    let start = node_ref();
    let _ui = mount((800, 600), ONE, move || {
        svg()
            .w(800.0)
            .h(600.0)
            .view_box(0.0, 0.0, 400.0, 300.0)
            .child(
                svg::group()
                    .child(
                        svg::text("hello")
                            .at(100.0, 100.0)
                            .text_size(10.0)
                            .node_ref(start),
                    )
                    .child(
                        svg::text("hello")
                            .at(100.0, 200.0)
                            .text_size(10.0)
                            .anchor(svg::TextAnchor::Middle)
                            .node_ref(middle),
                    ),
            )
    });
    let wide = |r: NodeRef| with_first_child(r, |rect| rect);
    let (a, b) = (wide(start), wide(middle));
    assert_eq!(a.x, 200, "the view box doubles everything");
    assert_eq!(a.width, b.width);
    assert!(a.width > 25, "{a:?}");
    assert_eq!(b.x, 200 - b.width / 2);
    assert!(a.y < 200 && a.y + a.height > 195, "{a:?}");
}

fn with_first_child(r: NodeRef, f: impl Fn(RectI) -> RectI) -> RectI {
    use anti_webui_core::tree::with_doc;
    with_doc(|d| {
        let id = r.get().unwrap();
        let child = d.get(id).unwrap().children[0];
        f(d.window_rect(child).unwrap())
    })
}

#[test]
fn nothing_needs_a_box_decided_up_front() {
    // A shape in `for_each` and `show` goes like the rest.
    let items = signal(vec![1, 2]);
    let mut ui = mount((800, 600), ONE, move || {
        svg().w(100.0).h(100.0).child(for_each(
            move || items.get(),
            |i| *i,
            |i| svg::rect(i as f64 * 10.0, 0.0, 5.0, 5.0),
        ))
    });
    assert_eq!(paths(&mut ui).len(), 2);
    items.set(vec![1, 2, 3]);
    ui.update();
    let d = paths(&mut ui);
    assert_eq!(d.len(), 3);
    assert!(d[2].bounds.x >= 28, "{:?}", d[2].bounds);
}

#[test]
fn inherited_defaults_are_those_of_svg() {
    let style = anti_webui_core::style::ComputedStyle::default();
    assert_eq!(style.fill_paint, SvgPaint::Color(Color::BLACK));
    assert_eq!(style.stroke_paint, SvgPaint::None);
    assert_eq!(style.stroke_width, 1.0);
}

#[test]
fn a_file_makes_shapes() {
    let data =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" viewBox="0 0 2 1">
        <rect width="1" height="1" fill="#ff0000"/>
        <circle cx="1.5" cy="0.5" r="0.5" fill="#00ff00" stroke="#000" stroke-width="0.1"/>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || svg::parse(data).unwrap());
    assert_eq!(paths(&mut ui).len(), 3);
}

#[test]
fn a_file_with_what_cannot_be_drawn_is_one_picture() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
        <filter id="f"><feGaussianBlur stdDeviation="1"/></filter>
        <rect width="10" height="10" fill="#ff0000" filter="url(#f)"/>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || svg::parse(data).unwrap());
    assert_eq!(paths(&mut ui).len(), 0);
}

#[test]
fn a_file_that_cannot_be_read_says_so() {
    assert!(svg::parse(b"not svg").is_err());
}

#[test]
fn text_can_be_measured_up_front() {
    let measured = std::cell::Cell::new((0.0, 0.0));
    let r = node_ref();
    let _ui = mount((800, 600), ONE, || {
        measured.set(svg::measure_text(
            "Some label",
            &Style::default().text_size(20.0),
        ));
        div()
            .flex()
            .items_start()
            .child(div().node_ref(r).child(text("Some label").text_size(20.0)))
    });
    let (w, h) = measured.get();
    let rect = r.rect().unwrap();
    assert_eq!((w as i32, h as i32), (rect.width, rect.height));
}

fn texts(ui: &mut Ui) -> Vec<(anti_webui_core::scene::GlyphRun, usize)> {
    ui.paint(Instant::now())
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Glyphs(g) => Some((g.clone(), g.glyphs.len())),
            _ => None,
        })
        .collect()
}

/// What Mermaid writes: HTML in a `foreignObject`, styled by a sheet with `!important` and classes.
#[test]
fn html_labels_become_text() {
    let data = br##"<svg id="m" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <style>#m{color:#112233}#m .box{color:#ff0000}#m p{margin:0}</style>
        <g transform="translate(10 20)">
            <foreignObject width="100" height="24">
                <div xmlns="http://www.w3.org/1999/xhtml" class="box" style="line-height:1.5;text-align:center">
                    <span><p>Label</p></span>
                </div>
            </foreignObject>
        </g>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    let runs = texts(&mut ui);
    assert_eq!(runs.len(), 1, "the text is there");
    assert_eq!(runs[0].1, 5);
    assert_eq!(runs[0].0.color, Color::from_rgb(0xff0000));
    // In the middle of its box, moved along with what is around it.
    let middle = runs[0].0.glyphs.first().unwrap().x;
    assert!(middle > 20.0 && middle < 60.0, "{middle}");
}

#[test]
fn backgrounds_of_html_labels_are_painted_and_empty_ones_leave_nothing() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <foreignObject width="100" height="24">
            <div xmlns="http://www.w3.org/1999/xhtml" style="background-color:rgba(10,20,30,0.5)">Hi</div>
        </foreignObject>
        <foreignObject width="50" height="24"><div xmlns="http://www.w3.org/1999/xhtml"><p></p></div></foreignObject>
        <foreignObject width="0" height="0"><div xmlns="http://www.w3.org/1999/xhtml">Gone</div></foreignObject>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    assert_eq!(paths(&mut ui).len(), 1, "the one background");
    assert_eq!(texts(&mut ui).len(), 1, "one label has text");
}

#[test]
fn a_line_break_in_html_makes_two_lines() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <foreignObject width="200" height="50">
            <div xmlns="http://www.w3.org/1999/xhtml"><p>one<br/>two</p><p>three</p></div>
        </foreignObject>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    let runs = texts(&mut ui);
    assert_eq!(runs.len(), 3, "a run for each line");
    assert!(runs[0].0.glyphs[0].y < runs[1].0.glyphs[0].y);
    assert!(runs[1].0.glyphs[0].y < runs[2].0.glyphs[0].y);
}

/// `<text>` goes through the same fonts as other text, rather than being outlines.
#[test]
fn text_in_a_file_is_text() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <text x="100" y="50" text-anchor="middle" dominant-baseline="central" style="font-size:16px" fill="#0000ff"><tspan x="100" dy="0">Hello</tspan><tspan x="100" dy="20">World</tspan></text>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    assert_eq!(paths(&mut ui).len(), 0, "no outlines");
    let runs = texts(&mut ui);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].0.color, Color::from_rgb(0x0000ff));
    assert!(runs[1].0.glyphs[0].y > runs[0].0.glyphs[0].y + 10.0);
}

/// What has its letters turned one by one stays as outlines, which get that right.
#[test]
fn text_with_letters_on_their_own_is_outlines() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <text x="10" y="50" rotate="10" font-size="16">abc</text>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    assert!(!paths(&mut ui).is_empty());
    assert_eq!(texts(&mut ui).len(), 0);
}

/// What every Mermaid diagram has: an arrow in a marker, whose box clips it but cuts nothing off.
#[test]
fn a_marker_does_not_make_it_a_picture() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <defs><marker id="a" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto"><path d="M0 0 L10 5 L0 10 Z"/></marker></defs>
        <path d="M10 10 L100 80" fill="none" stroke="#000" marker-end="url(#a)"/>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    let d = paths(&mut ui);
    assert_eq!(d.len(), 2, "the line and the arrow");
    assert!(d[1].quads.is_some(), "its box still cuts it");
}

#[test]
fn a_clip_path_that_is_not_a_rectangle_makes_it_a_picture() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <clipPath id="c"><circle cx="50" cy="50" r="40"/></clipPath>
        <rect width="100" height="100" clip-path="url(#c)"/>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    assert_eq!(paths(&mut ui).len(), 0);
}

/// What can't be shapes is one picture, with the labels still over it.
#[test]
fn labels_stay_when_it_is_a_picture() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <filter id="f"><feGaussianBlur stdDeviation="1"/></filter>
        <rect width="10" height="10" fill="#ff0000" filter="url(#f)"/>
        <g transform="translate(5 5)"><foreignObject width="100" height="24"><div xmlns="http://www.w3.org/1999/xhtml">Hi</div></foreignObject></g>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    assert_eq!(texts(&mut ui).len(), 1);
}

/// What a sheet says of `rect` isn't for the label, which `usvg` sees as one.
#[test]
fn a_sheet_about_rectangles_leaves_labels_alone() {
    let data = br##"<svg id="m" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <style>#m .edge rect{opacity:0.5;fill:#f00}</style>
        <g class="edge"><foreignObject width="100" height="24">
            <div xmlns="http://www.w3.org/1999/xhtml" style="color:#000">Label</div>
        </foreignObject></g>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    let runs = texts(&mut ui);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0.color, Color::BLACK, "not faded");
}

/// Mermaid's sheets pick markers by how their `id` ends, which `usvg` doesn't know.
#[test]
fn a_sheet_can_pick_by_how_an_attribute_ends() {
    let data = br##"<svg id="m" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <style>#m .marker{fill:#000000}#m [id$="-hollowEnd"],#m .hollow{fill:#00ff00!important}</style>
        <defs><marker id="m-hollowEnd" class="marker" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto"><path d="M0 0 L10 5 L0 10 Z"/></marker></defs>
        <path d="M10 10 L100 80" fill="none" stroke="#000" marker-end="url(#m-hollowEnd)"/>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    let d = paths(&mut ui);
    assert_eq!(d.len(), 2, "the line and the arrow");
    assert_eq!(
        d[1].paint,
        anti_webui_core::scene::PathPaint::Color(Color::from_rgb(0x00ff00))
    );
}

/// The names of Mermaid's classes are `bolder` than what is around them.
#[test]
fn labels_can_be_bolder() {
    let data = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
        <g style="font-weight: bolder"><foreignObject width="100" height="24">
            <div xmlns="http://www.w3.org/1999/xhtml">Label</div>
        </foreignObject></g>
        <g transform="translate(0 40)"><foreignObject width="100" height="24">
            <div xmlns="http://www.w3.org/1999/xhtml">Label</div>
        </foreignObject></g>
    </svg>"##;
    let mut ui = mount((800, 600), ONE, || {
        svg::parse(data).unwrap().w(200.0).h(100.0)
    });
    let runs = texts(&mut ui);
    assert_eq!(runs.len(), 2);
    let (bold, normal) = (&runs[0].0, &runs[1].0);
    assert!(
        bold.embolden != normal.embolden
            || bold.normalized_coords != normal.normalized_coords
            || bold.glyphs.last().map(|g| g.x) != normal.glyphs.last().map(|g| g.x),
        "the first is bold"
    );
}
