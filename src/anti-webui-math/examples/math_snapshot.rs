//! Draws formulas, one per line, into a PNG to compare with KaTeX in a browser:
//! `math_snapshot formulas.txt out.png [scale] [first] [count]`.

use anti_webui_core::prelude::*;
use anti_webui_math::math;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let source = std::fs::read_to_string(&args[1]).expect("a file of formulas");
    let out = &args[2];
    let scale: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let first: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
    let count: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(13);
    let formulas: Vec<(usize, String)> = source
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .enumerate()
        .map(|(i, f)| (i + 1, f))
        .skip(first - 1)
        .take(count)
        .collect();
    let (w, h) = ((1000.0 * scale) as u32, (1900.0 * scale) as u32);
    anti_webui_app::headless::render_png(out, w, h, scale, move || {
        div()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x000000))
            .h(pct(100.0))
            .children(formulas.into_iter().map(|(n, f)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .border_b(1.0)
                    .border_color(rgb(0xcccccc))
                    .child(
                        text(n.to_string())
                            .w(32.0)
                            .text_size(14.0)
                            .text_color(rgb(0x888888)),
                    )
                    // KaTeX at 40 px: its 1.21 em of 33.06 px.
                    .child(div().flex_1().text_size(33.0579).child(math(f).display()))
            }))
    })
    .expect("a picture");
}
