//! A Markdown editor that shows what it says while it is written, as Obsidian's live preview.
//!
//! ```sh
//! cargo run --release -p anti-webui --example markdown_editor --features markdown -- notes.md
//! ```
//!
//! Ctrl+S saves the file. Ctrl + click on a link prints it. Pictures are found next to the file.
//! `ANTIWEBUI_PERF_OVERLAY=1` shows how long frames take.

use anti_webui::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: markdown_editor <file.md>");
        return;
    };
    let started = Instant::now();
    let title = format!(
        "{} — Markdown",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    App::new()
        .title(&title)
        .size(1000, 900)
        .run(move || {
            let source = signal(std::fs::read_to_string(&path).unwrap_or_default());
            let saved = signal(source.get_untracked());
            let dir = path.parent().map(PathBuf::from).unwrap_or_default();
            request_frame(move |_| {
                eprintln!(
                    "first frame after {:.0} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            });
            let file = path.clone();
            let save = move || {
                let text = source.get_untracked();
                match std::fs::write(&file, &text) {
                    Ok(()) => saved.set(text),
                    Err(e) => eprintln!("could not save {}: {e}", file.display()),
                }
            };
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            div()
                .w_full()
                .h_full()
                .bg(Color::WHITE)
                .flex()
                .flex_col()
                .on_key_down(move |e| {
                    if e.modifiers.primary() && e.key.is_char('s') {
                        e.prevent_default();
                        save();
                    }
                })
                .child(
                    div()
                        .px(16.0)
                        .py(6.0)
                        .text_size(13.0)
                        .text_color(Color::from_rgb(0x59636e))
                        .border_b(1.0)
                        .border_color(Color::from_rgb(0xd1d9e0))
                        .child(text(move || {
                            let dirty = source.with(|s| saved.with(|t| s != t));
                            format!("{name}{}", if dirty { " ●" } else { "" })
                        })),
                )
                .child(
                    div().flex_1().min_h(0.0).flex().justify_center().child(
                        markdown_editor(source)
                            .on_link(|url| println!("link: {url}"))
                            .image(move |src| {
                                let p = PathBuf::from(src);
                                ImageSource::from(if p.is_absolute() { p } else { dir.join(p) })
                            })
                            .w_full()
                            .max_w(900.0)
                            .h_full()
                            .overflow_y_scroll()
                            .px(32.0)
                            .py(24.0),
                    ),
                )
        })
        .unwrap();
}
