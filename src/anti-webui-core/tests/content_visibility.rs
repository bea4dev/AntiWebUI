//! `content-visibility`: what is far from the screen isn't laid out, and nothing jumps when it is.
//! See `docs/design/29-content-visibility.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::with_doc;

const ONE: Scale = Scale(120);

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: ONE,
    });
    ui.update();
    ui
}

fn laid_out(r: NodeRef) -> bool {
    let id = r.get().unwrap();
    with_doc(|d| d.nodes[id].visible)
}

/// Blocks of wide letters, which are more lines than the average letter makes of them, in a
/// scroller as tall as the window. Gives the scroller and the text of each block.
fn blocks(n: usize) -> (NodeRef, Vec<NodeRef>, impl FnOnce() -> Div) {
    let scroller = node_ref();
    let texts: Vec<NodeRef> = (0..n).map(|_| node_ref()).collect();
    let refs = texts.clone();
    let view = move || {
        div()
            .w(400.0)
            .h(300.0)
            .overflow_y_scroll()
            .node_ref(scroller)
            .children(refs.iter().enumerate().map(|(i, r)| {
                div()
                    .content_visibility_auto()
                    .estimate_contents()
                    .pb(8.0)
                    .child(div().node_ref(*r).child(text("WWWWW ".repeat(10 + i % 7))))
                    .into_view()
            }))
    };
    (scroller, texts, view)
}

#[test]
fn only_what_is_near_the_screen_is_laid_out() {
    let (_, texts, view) = blocks(60);
    let _ui = mount(view);
    assert!(laid_out(texts[0]));
    assert!(!laid_out(texts[59]));
}

#[test]
fn scrolling_lays_out_what_comes_near() {
    let (scroller, texts, view) = blocks(60);
    let mut ui = mount(view);
    scroller.scroll_to(0.0, 1e6);
    ui.update();
    assert!(laid_out(texts[59]));
    assert!(!laid_out(texts[0]), "far above now");
}

/// Coming back up, the blocks above take their real heights, which aren't what was estimated, and
/// what shows stays where it was.
#[test]
fn what_shows_does_not_jump() {
    let (scroller, texts, view) = blocks(60);
    let mut ui = mount(view);
    scroller.scroll_to(0.0, 1e6);
    ui.update();
    let shown = texts
        .iter()
        .copied()
        .find(|r| laid_out(*r) && r.rect().unwrap().y >= 0)
        .unwrap();
    let before = shown.rect().unwrap().y;
    let at = scroller.scroll_offset().y;
    scroller.scroll_to(0.0, at - 200.0);
    ui.update();
    // It moves down by as much as was scrolled, whatever the blocks above turned out to be.
    assert_eq!(shown.rect().unwrap().y, before + 200);
    // Something above came near the screen and was laid out.
    assert!(
        texts
            .iter()
            .any(|r| laid_out(*r) && r.rect().unwrap().bottom() < -100)
    );
}

#[test]
fn a_block_keeps_its_size_when_it_goes_away() {
    let (scroller, texts, view) = blocks(60);
    let mut ui = mount(view);
    let parent = |r: NodeRef| {
        let id = r.get().unwrap();
        with_doc(|d| d.nodes[d.nodes[id].parent.unwrap()].rect.height)
    };
    let real = parent(texts[0]);
    scroller.scroll_to(0.0, 1e6);
    ui.update();
    assert!(!laid_out(texts[0]));
    assert_eq!(parent(texts[0]), real);
}

#[test]
fn scrolling_to_something_skipped_lays_it_out_first() {
    let (scroller, texts, view) = blocks(60);
    let mut ui = mount(view);
    assert!(!laid_out(texts[40]));
    texts[40].scroll_into_view();
    ui.update();
    assert!(laid_out(texts[40]));
    let r = texts[40].rect().unwrap();
    assert!(r.y >= 0 && r.bottom() <= 300, "{r:?}");
    assert!(scroller.scroll_offset().y > 0.0);
}

#[test]
fn hidden_contents_are_never_laid_out() {
    let (inner, outer) = (node_ref(), node_ref());
    let _ui = mount(move || {
        div().child(
            div()
                .content_visibility_hidden()
                .contain_intrinsic_size(100.0, 40.0)
                .p(2.0)
                .node_ref(outer)
                .child(div().node_ref(inner).child(text("Hidden"))),
        )
    });
    assert!(!laid_out(inner));
    assert_eq!(outer.rect().unwrap().height, 44);
}

#[test]
fn an_estimate_is_near_the_real_size() {
    let (scroller, texts, view) = blocks(60);
    let mut ui = mount(view);
    let parent = |r: NodeRef| {
        let id = r.get().unwrap();
        with_doc(|d| d.nodes[d.nodes[id].parent.unwrap()].rect.height)
    };
    let estimated = parent(texts[59]);
    scroller.scroll_to(0.0, 1e6);
    ui.update();
    let real = parent(texts[59]);
    assert!(
        (estimated - real).abs() * 2 <= real,
        "estimated {estimated}, really {real}"
    );
}

/// Selecting all of it takes in what is skipped, which is then laid out, so that it can be copied.
#[test]
fn selecting_all_lays_out_all() {
    let (_, texts, view) = blocks(60);
    let mut ui = mount(view);
    assert!(!laid_out(texts[59]));
    with_doc(|d| d.state.selection = anti_webui_core::text::selection::select_all(d));
    ui.update();
    assert!(texts.iter().all(|r| laid_out(*r)));
    let copied = with_doc(|d| {
        let sel = d.state.selection.unwrap();
        anti_webui_core::text::selection::selected_text(d, &sel)
    });
    assert_eq!(
        copied.matches("WWWWW").count(),
        (0..60).map(|i| 10 + i % 7).sum::<usize>()
    );
}
