//! A PDF viewer.
//!
//! ```sh
//! cargo run --release -p anti-webui --example pdf --features pdf -- paper.pdf [percent]
//! ```
//!
//! Ctrl + the wheel zooms, and so do Ctrl + `+`, `-` and `0`. PageUp, PageDown, Home and End move.
//! Dragging selects text, a double click takes a word and a triple click a line, and Ctrl + C copies
//! it. Ctrl + F goes to the search box, where Enter goes to the next one found.

use anti_webui::prelude::*;

/// The items of an outline with how deep each is, in the order they show.
fn flatten(items: &[PdfOutlineItem], depth: usize, out: &mut Vec<(usize, PdfOutlineItem)>) {
    for item in items {
        out.push((depth, item.clone()));
        flatten(&item.children, depth + 1, out);
    }
}

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
            let query = signal(String::new());
            let found = move || {
                if query.with(String::is_empty) {
                    return String::new();
                }
                let count = viewer.search_count();
                let at = viewer.search_index().map_or(0, |i| i + 1);
                let more = if viewer.search_done() { "" } else { "+" };
                format!("{at} / {count}{more}")
            };
            let contents = signal(false);
            let search_box = node_ref();
            request_frame(move |_| viewer.focus());
            div()
                .flex()
                .flex_col()
                .w_full()
                .h_full()
                .bg(rgb(0x1e1f22))
                .on_key_down(move |e| {
                    if e.modifiers.ctrl && matches!(&e.key, Key::Character(c) if c == "f") {
                        search_box.focus();
                        e.stop_propagation();
                    }
                })
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
                        .select_none()
                        .child(button("目次").on_click(move |_| contents.update(|c| *c = !*c)))
                        .child(button("−").on_click(move |_| viewer.zoom_by(0.8)))
                        .child(button("+").on_click(move |_| viewer.zoom_by(1.25)))
                        .child(
                            button("幅に合わせる")
                                .on_click(move |_| viewer.set_zoom(PdfZoom::FitWidth)),
                        )
                        .child(button("全体").on_click(move |_| viewer.set_zoom(PdfZoom::FitPage)))
                        .child(text(shown))
                        .child(div().flex_1())
                        .child(
                            text_input()
                                .w(200.0)
                                .placeholder("検索")
                                .value(query)
                                .node_ref(search_box)
                                .on_input(move |q| viewer.search(q))
                                .on_submit(move |_| viewer.find_next()),
                        )
                        .child(text(found))
                        .child(button("↑").on_click(move |_| viewer.find_previous()))
                        .child(button("↓").on_click(move |_| viewer.find_next())),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_h(0.0)
                        .child(show(
                            move || contents.get(),
                            move || {
                                div()
                                    .w(280.0)
                                    .h_full()
                                    .flex_none()
                                    .overflow_scroll()
                                    .bg(bar)
                                    .text_color(Color::WHITE)
                                    .child(for_each(
                                        move || {
                                            let mut items = Vec::new();
                                            flatten(&viewer.outline(), 0, &mut items);
                                            items.into_iter().enumerate().collect::<Vec<_>>()
                                        },
                                        |(i, _)| *i,
                                        move |(_, (depth, item))| {
                                            div()
                                                .pl(8.0 + depth as f32 * 16.0)
                                                .pr(8.0)
                                                .py(4.0)
                                                .cursor(CursorIcon::Pointer)
                                                .on_click(move |_| {
                                                    if let Some(target) = item.target {
                                                        viewer.go_to(target);
                                                    }
                                                })
                                                .child(text(item.title.clone()))
                                        },
                                    ))
                            },
                        ))
                        .child(
                            pdf_view(path.clone())
                                .viewer(viewer)
                                .on_link(|url| {
                                    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
                                })
                                .flex_1()
                                .min_h(0.0),
                        ),
                )
        })
        .unwrap();
}
