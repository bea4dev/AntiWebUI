//! Pixel tests through the real renderer. Skipped when no GPU adapter is available.

use anti_webui_core::prelude::*;

fn render(w: u32, h: u32, scale: f64, view: impl FnOnce() -> Div) -> Option<Vec<u8>> {
    match anti_webui_app::headless::render_rgba(w, h, scale, view) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn px(p: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

#[test]
fn rounded_overflow_clips_children() {
    let (w, h) = (200, 200);
    let Some(p) = render(w, h, 1.0, || {
        div().bg(Color::WHITE).h(pct(100.0)).p(20.0).child(
            div()
                .w(120.0)
                .h(120.0)
                .rounded(30.0)
                .overflow_y_scroll()
                .child(div().h(400.0).bg(rgb(0xff0000))),
        )
    }) else {
        return;
    };
    // The corner of the scroll view's bounding box lies outside its rounded shape.
    assert_eq!(
        px(&p, w, 22, 22),
        [255, 255, 255, 255],
        "child leaked out of the rounded corner"
    );
    // The middle is covered by the child.
    assert_eq!(px(&p, w, 80, 80), [255, 0, 0, 255]);
    // Along the straight edge the child reaches the box edge exactly.
    assert_eq!(px(&p, w, 20, 80), [255, 0, 0, 255]);
    assert_eq!(px(&p, w, 19, 80), [255, 255, 255, 255]);
}

#[test]
fn borders_are_crisp_at_fractional_scale() {
    let (w, h) = (180, 90);
    let Some(p) = render(w, h, 1.5, || {
        div().bg(Color::WHITE).h(pct(100.0)).child(
            div()
                .ml(10.0)
                .mt(10.0)
                .w(33.0)
                .h(20.0)
                .border(1.0)
                .border_color(Color::BLACK),
        )
    }) else {
        return;
    };
    // At 1.5x: box at (15,15) size 50x30, 1px border → 2 physical pixels, fully opaque.
    for x in [15, 16, 63, 64] {
        assert_eq!(px(&p, w, x, 30), [0, 0, 0, 255], "x={x}");
    }
    assert_eq!(px(&p, w, 14, 30), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 17, 30), [255, 255, 255, 255]);
    assert_eq!(px(&p, w, 65, 30), [255, 255, 255, 255]);
}
