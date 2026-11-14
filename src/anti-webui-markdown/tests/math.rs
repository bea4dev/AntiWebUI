//! Formulas in `markdown_view` and `markdown_editor`. See `docs/design/40-math.md`.
#![cfg(feature = "math")]

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::{NodeKind, with_doc};
use anti_webui_markdown::{markdown_editor, markdown_view};

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(900, 1500),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

/// The formulas that show, in the order of the document.
fn formulas() -> Vec<RectI> {
    with_doc(|d| {
        let mut out: Vec<(u32, RectI)> = d
            .nodes
            .iter()
            .filter(|(_, n)| n.visible && matches!(n.kind, NodeKind::Drawing(_)))
            .map(|(id, n)| (n.doc_order, d.window_rect(id).unwrap()))
            .collect();
        out.sort_by_key(|(o, _)| *o);
        out.into_iter().map(|(_, r)| r).collect()
    })
}

fn texts() -> Vec<String> {
    with_doc(|d| {
        let mut out: Vec<(u32, String)> = d
            .nodes
            .iter()
            .filter(|(_, n)| n.visible)
            .filter_map(|(_, n)| match &n.kind {
                NodeKind::Text(t) => Some((n.doc_order, t.text.clone())),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(o, _)| *o);
        out.into_iter().map(|(_, t)| t).collect()
    })
}

const DOC: &str = "Energy $E = mc^2$ here.\n\n$$\n\\sum_{i=1}^n i\n$$\n\nBefore $$x^2$$ after.\n\n```math\na + b\n```\n\nBad $\\frac{1}{x$ one.\n";

#[test]
fn the_viewer_draws_formulas_where_they_are_written() {
    let _ui = mount(|| div().w(800.0).child(markdown_view(DOC)));
    let f = formulas();
    // `E =` and `mc^2`: among words, a line may break after the `=`.
    assert_eq!(f.len(), 5, "{f:?}");
    assert_eq!(f[0].y + f[0].height, f[1].y + f[1].height);
    // On lines of their own, in the middle.
    for r in &f[2..] {
        assert!((r.x + r.width / 2 - 400).abs() <= 1, "{r:?}");
    }
    // The text around one splits into lines of its own, without the spaces next to it.
    let t = texts();
    assert!(t.iter().any(|t| t == "Before"), "{t:?}");
    assert!(t.iter().any(|t| t == "after."), "{t:?}");
    // What can't be read shows as written, in the paragraph.
    assert!(t.iter().any(|t| t.contains(r"\frac{1}{x")), "{t:?}");
    // Each block is lower than the one before.
    assert!(f[1..].windows(2).all(|w| w[1].y > w[0].y));
}

#[test]
fn the_editor_shows_a_formula_until_the_caret_is_in_it() {
    let source = signal("Energy $E = mc^2$ here.\n\n$$\nx^2\n$$\n".to_owned());
    let mut ui = mount(move || markdown_editor(source).w(800.0).h(600.0));
    // The one among words in two parts, a line may break between.
    assert_eq!(formulas().len(), 3);
    let host = with_doc(|d| d.editors[0]);
    with_doc(|d| d.pending_focus = Some(Some(host)));
    with_doc(|d| d.nodes[host].editor_mut().unwrap().select(9, 9));
    ui.update();
    // The caret is in the first: it shows as written, and the other still as a formula.
    assert_eq!(formulas().len(), 1);
    assert!(texts().iter().any(|t| t.contains("$E = mc^2$")));
    // In the one on its own lines, that one shows as written.
    // As the editor tests do: put the caret, then move it, which the decorations follow.
    with_doc(|d| d.nodes[host].editor_mut().unwrap().select(27, 27));
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::ArrowRight),
            text: None,
            modifiers: Modifiers::default(),
            pressed,
            repeat: false,
        }));
    }
    ui.update();
    assert_eq!(formulas().len(), 2);
    let t = texts();
    assert!(t.iter().any(|t| t.trim_end() == "x^2"), "{t:?}");
}

#[test]
fn a_reference_goes_to_its_equation() {
    use anti_webui_core::geometry::PointF;
    let doc = "See $\\eqref{eq:far}$ and $\\eqref{eq:none}$.\n\n$$\n\\begin{equation} E = mc^2 \\label{eq:far} \\end{equation}\n$$\n";
    let followed = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let f = followed.clone();
    let mut ui = mount(move || {
        div()
            .w(800.0)
            .child(markdown_view(doc).on_link(move |url| f.borrow_mut().push(url.to_owned())))
    });
    let refs: Vec<RectI> = formulas().into_iter().take(2).collect();
    let mut time = 0;
    for r in refs {
        let at = PointF::new((r.x + r.width / 2) as f32, (r.y + r.height / 2) as f32);
        let modifiers = Modifiers::default();
        ui.handle_event(WindowEvent::PointerMoved {
            position: at,
            modifiers,
        });
        for pressed in [true, false] {
            time += 300;
            ui.handle_event(WindowEvent::PointerButton {
                position: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
                time,
            });
        }
        ui.update();
    }
    // The one that is in the document is scrolled to; the other is left to the program.
    assert_eq!(*followed.borrow(), vec!["#eq:none".to_owned()]);
}

#[test]
fn in_the_editor_a_formula_ends_its_line_where_it_says() {
    let source = signal("Two lines: $a = b \\\\ c = d$ and on.\n".to_owned());
    let _ui = mount(move || markdown_editor(source).w(800.0).h(600.0));
    let f = formulas();
    // `a =`, `b`, then `c =`, `d` on the next line, at its start.
    assert_eq!(f.len(), 4, "{f:?}");
    let bottom = |r: &RectI| r.y + r.height;
    assert_eq!(bottom(&f[0]), bottom(&f[1]));
    assert!(f[2].y > bottom(&f[1]) - 2, "{f:?}");
    assert!(f[2].x < f[0].x, "{f:?}");
    assert_eq!(bottom(&f[2]), bottom(&f[3]));
}
