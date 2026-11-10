use anti_webui_core::prelude::*;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "snapshot.png".into());
    let scale: f64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.5);
    let (w, h) = (
        (480.0 * scale).round() as u32,
        (320.0 * scale).round() as u32,
    );
    anti_webui_app::headless::render_png(&out, w, h, scale, || {
        div()
            .flex()
            .flex_col()
            .gap(12.0)
            .p(16.0)
            .h(pct(100.0))
            .bg(rgb(0xf6f8fa))
            .child(text("AntiWebUI — integer physical pixel rendering").text_size(20.0).bold())
            .child(text("The quick brown fox jumps over the lazy dog. 日本語の文章も正しく折り返されるかを確認します。絵文字 🎉👍🏽"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(8.0)
                    .children((0..3).map(|i| {
                        div()
                            .flex_1()
                            .h(40.0)
                            .bg(rgb(0xffffff))
                            .border(1.0)
                            .border_color(rgb(0xd0d7de))
                            .rounded(6.0)
                            .shadow_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(text(format!("Box {i}")))
                    })),
            )
            .child(div().h(1.0).bg(rgb(0xd0d7de)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(8.0)
                    .child(div().px(12.0).py(6.0).bg(rgb(0x1f883d)).rounded(6.0).text_color(Color::WHITE).child("Primary"))
                    .child(div().px(12.0).py(6.0).border(1.0).border_color(rgb(0xd0d7de)).rounded(6.0).child("Secondary"))
                    .child(div().px(12.0).py(6.0).border(2.0).border_color(rgb(0xcf222e)).rounded(999.0).child(text("Danger").underline())),
            )
    })
    .unwrap();
    println!("wrote {out} ({w}x{h})");
}
