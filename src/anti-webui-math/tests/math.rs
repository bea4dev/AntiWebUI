//! `math` in a `Ui` without a window: where it goes among words and on a line of its own.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeId, NodeKind, with_doc};
use anti_webui_math::{ERROR_COLOR, check, math};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale: Scale(120),
    });
    ui.update();
    ui
}

/// The drawings, in the order of the document, where they are in the window.
fn drawings() -> Vec<(NodeId, RectI)> {
    with_doc(|d| {
        let mut out: Vec<(u32, NodeId)> = d
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.kind, NodeKind::Drawing(_)))
            .map(|(id, n)| (n.doc_order, id))
            .collect();
        out.sort();
        out.into_iter()
            .map(|(_, id)| (id, d.window_rect(id).unwrap()))
            .collect()
    })
}

#[test]
fn among_words_formulas_share_the_baseline() {
    let _ui = mount(|| {
        div().w(800.0).text_size(20.0).child(rich_text((
            "a ",
            inline(math("x")),
            " b ",
            inline(math(r"\frac{1}{2}")),
            " c ",
            inline(math(r"x_{i}")),
        )))
    });
    let d = drawings();
    assert_eq!(d.len(), 3);
    // KaTeX's 1.21 em of 20 px is 24.2 px: `x` is 0.431 em high and has no depth.
    let x = d[0].1;
    assert_eq!(x.height, (0.431f32 * 24.2).round() as i32);
    // Each one's baseline is its ascent below its top: all on one line.
    let baseline = x.y + x.height;
    let frac = d[1].1;
    let sub = d[2].1;
    assert!(frac.y < x.y && frac.y + frac.height > baseline);
    assert!(sub.y + sub.height > baseline);
    assert_eq!(x.y + x.height, baseline);
}

#[test]
fn a_formula_of_its_own_is_in_the_middle_with_its_number_at_the_end() {
    let _ui = mount(|| {
        div()
            .w(600.0)
            .text_size(20.0)
            .child(math("x = 1").display())
            .child(math(r"\tag{2} y = 2").display())
    });
    let d = drawings();
    assert_eq!(d.len(), 3);
    let (x, y, tag) = (d[0].1, d[1].1, d[2].1);
    assert!((x.x + x.width / 2 - 300).abs() <= 1, "{x:?}");
    assert!((y.x + y.width / 2 - 300).abs() <= 1, "{y:?}");
    assert_eq!(tag.x + tag.width, 600);
    // The same height, on the same line.
    assert_eq!((tag.y, tag.height), (y.y, y.height));
}

#[test]
fn too_wide_a_formula_starts_at_the_left_and_scrolls() {
    let wide = (0..40)
        .map(|i| format!("a_{{{i}}}"))
        .collect::<Vec<_>>()
        .join("+");
    let _ui = mount(move || div().w(300.0).text_size(20.0).child(math(wide).display()));
    let d = drawings();
    assert_eq!(d[0].1.x, 0);
    assert!(d[0].1.width > 300);
}

#[test]
fn what_cant_be_read_shows_as_written_in_red() {
    assert!(check(r"\frac{1}{", false).is_err());
    let _ui = mount(|| div().text_size(20.0).child(math(r"\frac{1}{")));
    assert!(drawings().is_empty());
    let red = with_doc(|d| {
        d.nodes.iter().any(|(_, n)| {
            matches!(&n.kind, NodeKind::Text(t) if t.text == r"\frac{1}{")
                && n.computed.color == ERROR_COLOR
        })
    });
    assert!(red);
}

#[test]
fn among_words_a_long_formula_breaks_after_its_operators() {
    let long = (0..30)
        .map(|i| format!("a_{{{i}}}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let _ui = mount(move || {
        let mut runs = vec![Run::Text("so ".into())];
        runs.extend(math(long).runs());
        runs.push(Run::Text(" and on.".into()));
        div().w(300.0).text_size(20.0).child(rich_text(runs))
    });
    let d = drawings();
    assert_eq!(d.len(), 30);
    // On several lines, each within the room, a part starting each line at the left.
    let lines: std::collections::BTreeSet<i32> = d.iter().map(|(_, r)| r.y + r.height).collect();
    assert!(lines.len() >= 3, "{lines:?}");
    assert!(d.iter().all(|(_, r)| r.x + r.width <= 300), "{d:?}");
    assert!(d.iter().filter(|(_, r)| r.x == 0).count() >= 2);
}

#[test]
fn numbers_go_at_the_right_end_or_with_leqno_at_the_left() {
    let tex = r"\begin{align} a &= b \\ c &= d \end{align}";
    let _ui = mount(move || {
        div()
            .w(600.0)
            .text_size(20.0)
            .child(math(tex).display())
            .child(math(tex).display().leqno().first_number(3))
    });
    let d = drawings();
    assert_eq!(d.len(), 4);
    let (body, tags, left_tags, left_body) = (d[0].1, d[1].1, d[2].1, d[3].1);
    assert_eq!(tags.x + tags.width, 600);
    assert!((body.x + body.width / 2 - 300).abs() <= 1, "{body:?}");
    assert_eq!(left_tags.x, 0);
    assert!((left_body.x + left_body.width / 2 - 300).abs() <= 1);
    assert_eq!((tags.y, tags.height), (body.y, body.height));
}

#[test]
fn a_reference_goes_where_it_refers_when_clicked() {
    use anti_webui_core::event::{PointerButton, WindowEvent as W};
    use anti_webui_core::geometry::PointF;
    let went = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let w = went.clone();
    let mut ui = mount(move || {
        div().text_size(20.0).child(
            math(r"\text{see } \eqref{eq:one}")
                .references(|k| (k == "eq:one").then(|| "1".into()))
                .on_reference(move |k| w.borrow_mut().push(k.to_owned())),
        )
    });
    let r = drawings()[0].1;
    let at = |x: i32| PointF::new(x as f32, (r.y + r.height / 2) as f32);
    let mut time = 0;
    for x in [r.x + 3, r.x + r.width - 6] {
        let modifiers = Default::default();
        ui.handle_event(W::PointerMoved {
            position: at(x),
            modifiers,
        });
        for pressed in [true, false] {
            time += 300;
            ui.handle_event(W::PointerButton {
                position: at(x),
                button: PointerButton::Primary,
                pressed,
                modifiers,
                time,
            });
        }
        ui.update();
    }
    // The words before it go nowhere; the number does.
    assert_eq!(*went.borrow(), vec!["eq:one".to_owned()]);
}
