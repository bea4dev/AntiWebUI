//! A PDF viewer.
//!
//! ```sh
//! cargo run --release -p anti-webui --example pdf --features pdf -- paper.pdf [percent]
//! ```
//!
//! Ctrl + the wheel zooms, and so do Ctrl + `+`, `-` and `0`. PageUp, PageDown, Home and End move.

use anti_webui::prelude::*;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: pdf <file.pdf> [percent]");
        return;
    };
    let zoom: Option<f32> = std::env::args().nth(2).and_then(|p| p.parse().ok());
    App::new()
        .title("PDF")
        .size(1100, 900)
        .run(move || {
            let viewer = pdf_viewer();
            if let Some(percent) = zoom {
                viewer.set_zoom(PdfZoom::Percent(percent / 100.0));
            }
            let bar = rgb(0x2b2d31);
            let shown = move || match viewer.state() {
                PdfState::Loading => "読み込み中".to_owned(),
                PdfState::Failed(e) => e.to_string(),
                PdfState::Ready => format!(
                    "{} / {} ページ  {:.0}%",
                    viewer.page() + 1,
                    viewer.page_count(),
                    viewer.percent() * 100.0
                ),
            };
            request_frame(move |_| viewer.focus());
            div()
                .flex()
                .flex_col()
                .w_full()
                .h_full()
                .bg(rgb(0x1e1f22))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(8.0)
                        .px(12.0)
                        .py(6.0)
                        .bg(bar)
                        .text_color(Color::WHITE)
                        .child(button("−").on_click(move |_| viewer.zoom_by(0.8)))
                        .child(button("+").on_click(move |_| viewer.zoom_by(1.25)))
                        .child(
                            button("幅に合わせる")
                                .on_click(move |_| viewer.set_zoom(PdfZoom::FitWidth)),
                        )
                        .child(button("全体").on_click(move |_| viewer.set_zoom(PdfZoom::FitPage)))
                        .child(text(shown)),
                )
                .child(pdf_view(path.clone()).viewer(viewer).flex_1().min_h(0.0))
        })
        .unwrap();
}
