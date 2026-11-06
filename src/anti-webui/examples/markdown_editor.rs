//! A Markdown editor that shows what it says while it is written, as Obsidian's live preview, or
//! with `--split` the source on the left and what it shows on the right.
//!
//! ```sh
//! cargo run --release -p anti-webui --example markdown_editor --features markdown -- notes.md
//! cargo run --release -p anti-webui --example markdown_editor --features markdown -- notes.md --split
//! ```
//!
//! Ctrl+S saves the file. Ctrl + click on a link prints it. Pictures are found next to the file.
//! `ANTIWEBUI_PERF_OVERLAY=1` shows how long frames take.

use anti_webui::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let split = args.iter().any(|a| a == "--split");
    let Some(path) = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
    else {
        eprintln!("usage: markdown_editor <file.md> [--split]");
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
                .child(if split {
                    let editor = editor_ref();
                    request_frame(move |_| editor.focus());
                    markdown_split_editor(source)
                        .editor_ref(editor)
                        .on_link(|url| println!("link: {url}"))
                        .image(move |src| picture(&dir, src))
                        .flex_1()
                        .min_h(0.0)
                        .into_view()
                } else {
                    div()
                        .flex_1()
                        .min_h(0.0)
                        .flex()
                        .justify_center()
                        .child(
                            markdown_editor(source)
                                .on_link(|url| println!("link: {url}"))
                                .image(move |src| picture(&dir, src))
                                .w_full()
                                .max_w(900.0)
                                .h_full()
                                .overflow_y_scroll()
                                .px(32.0)
                                .py(24.0),
                        )
                        .into_view()
                })
        })
        .unwrap();
}

/// A picture next to the file, unless its path is absolute.
fn picture(dir: &std::path::Path, src: &str) -> ImageSource {
    let p = PathBuf::from(src);
    ImageSource::from(if p.is_absolute() { p } else { dir.join(p) })
}
