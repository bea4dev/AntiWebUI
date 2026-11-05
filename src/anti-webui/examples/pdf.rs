//! A PDF viewer.
//!
//! ```sh
//! cargo run --release -p anti-webui --example pdf --features pdf -- paper.pdf [percent]
//! ```
//!
//! Ctrl + the wheel and pinching zoom, and so do Ctrl + `+`, `-` and `0`. Ctrl + `]` and `[` turn the
//! pages. PageUp, PageDown, Home and End move. Dragging selects text, a double click takes a word and
//! a triple click a line, Ctrl + C copies it, and dragging what is selected takes it to another
//! program. Ctrl + F goes to the search box, where Enter goes to the next one found.
//!
//! Fields can be filled in by clicking on them, and the buttons with colors mark up what is selected
//! or draw on the page. Clicking on an annotation picks it, which can then be moved or removed with
//! Delete. Ctrl + Z goes back, Ctrl + Y forward, and Ctrl + S writes `<name>-edited.pdf` next to the
//! document.
//!
//! F11 puts the window on all of the screen. F5 shows the document one page at a time there, where
//! the arrows, Space, a click and the wheel go through the pages, a number and Enter go to a page,
//! `B` and `W` blank the screen, and Escape ends it.

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
            let thumbnails = signal(false);
            let search_box = node_ref();
            let saved_as = {
                let path = std::path::PathBuf::from(&path);
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                path.with_file_name(format!("{stem}-edited.pdf"))
            };
            let save = {
                let saved_as = saved_as.clone();
                move || viewer.save(saved_as.clone())
            };
            let tool_button = move |name: &'static str, tool: PdfTool| {
                button(name)
                    .on_click(move |_| {
                        viewer.set_tool(if viewer.tool() == tool {
                            PdfTool::Select
                        } else {
                            tool
                        })
                    })
                    .dyn_style(move |s| {
                        if viewer.tool() == tool {
                            s.outline(2.0).outline_color(Color::from_rgb(0x7cacf8))
                        } else {
                            s
                        }
                    })
            };
            request_frame(move |_| viewer.focus());
            div()
                .flex()
                .flex_col()
                .w_full()
                .h_full()
                .bg(rgb(0x1e1f22))
                .on_key_down({
                    let save = save.clone();
                    move |e| {
                        if e.modifiers.ctrl && matches!(&e.key, Key::Character(c) if c == "f") {
                            search_box.focus();
                            e.stop_propagation();
                        }
                        if e.modifiers.ctrl && matches!(&e.key, Key::Character(c) if c == "s") {
                            save();
                            e.stop_propagation();
                        }
                        if matches!(e.key, Key::Named(NamedKey::F(5))) {
                            viewer.present();
                        }
                        if matches!(e.key, Key::Named(NamedKey::F(11))) {
                            set_window_fullscreen(!window_fullscreen());
                        }
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
                        .child(
                            button("サムネイル").on_click(move |_| thumbnails.update(|t| *t = !*t)),
                        )
                        .child(button("−").on_click(move |_| viewer.zoom_by(0.8)))
                        .child(button("+").on_click(move |_| viewer.zoom_by(1.25)))
                        .child(
                            button("幅に合わせる")
                                .on_click(move |_| viewer.set_zoom(PdfZoom::FitWidth)),
                        )
                        .child(button("全体").on_click(move |_| viewer.set_zoom(PdfZoom::FitPage)))
                        .child(button("回転").on_click(move |_| viewer.rotate_clockwise()))
                        .child(button("プレゼン").on_click(move |_| viewer.present()))
                        .child(
                            button(move || {
                                if window_fullscreen() {
                                    "全画面を解除"
                                } else {
                                    "全画面"
                                }
                            })
                            .on_click(move |_| set_window_fullscreen(!window_fullscreen())),
                        )
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
                        .items_center()
                        .gap(8.0)
                        .px(12.0)
                        .pb(6.0)
                        .bg(bar)
                        .text_color(Color::WHITE)
                        .select_none()
                        .child(tool_button("蛍光ペン", PdfTool::Highlight))
                        .child(tool_button("下線", PdfTool::Underline))
                        .child(tool_button("取り消し線", PdfTool::StrikeOut))
                        .child(tool_button("インク", PdfTool::Ink))
                        .child(tool_button("テキスト", PdfTool::FreeText))
                        .child(tool_button("付箋", PdfTool::Note))
                        .child(button("消す").on_click(move |_| viewer.delete_annotation()))
                        .child(button("元に戻す").on_click(move |_| viewer.undo()))
                        .child(button("やり直す").on_click(move |_| viewer.redo()))
                        .child(button("保存").on_click({
                            let save = save.clone();
                            move |_| save()
                        }))
                        .child(text(move || {
                            let state = match viewer.editing() {
                                PdfEditing::Yes | PdfEditing::NotYet => "",
                                PdfEditing::Encrypted => "暗号化されているので編集できません",
                                PdfEditing::Broken => "壊れているので編集できません",
                            };
                            let saving = match viewer.save_result() {
                                Some(Ok(path)) => format!("{} に保存しました", path.display()),
                                Some(Err(e)) => format!("保存できません: {e}"),
                                None => String::new(),
                            };
                            let modified = if viewer.is_modified() {
                                "変更あり"
                            } else {
                                ""
                            };
                            format!("{state} {modified} {saving}")
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_h(0.0)
                        .child(show(
                            move || thumbnails.get(),
                            move || {
                                pdf_thumbnails(viewer)
                                    .w(140.0)
                                    .h_full()
                                    .flex_none()
                                    .bg(bar)
                                    .text_color(Color::WHITE)
                            },
                        ))
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
