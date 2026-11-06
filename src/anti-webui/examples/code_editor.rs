//! A code editor with the colors of tree-sitter grammars.
//!
//! ```sh
//! cargo run --release -p anti-webui --example code_editor --features code -- src/main.rs
//! ```
//!
//! The language comes from the file's extension. Ctrl+S saves, Ctrl+F finds, Ctrl+H replaces,
//! Ctrl+/ comments lines, Ctrl+Shift+[ and ] fold and unfold. `--light` for the light colors.
//! `ANTIWEBUI_PERF_OVERLAY=1` shows how long frames take.

use anti_webui::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let light = args.iter().any(|a| a == "--light");
    let Some(path) = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
    else {
        eprintln!("usage: code_editor [--light] <file>");
        return;
    };
    let started = Instant::now();
    let language = CodeLanguage::from_path(&path);
    let title = format!(
        "{} — {}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        language.name()
    );
    App::new()
        .title(&title)
        .size(1100, 900)
        .run(move || {
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            request_frame(move |_| {
                eprintln!(
                    "first frame after {:.0} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            });
            let theme = if light {
                CodeTheme::light()
            } else {
                CodeTheme::dark()
            };
            let editor = editor_ref();
            let dirty = signal(false);
            let file = path.clone();
            let save = Rc::new(move || match std::fs::write(&file, editor.text()) {
                Ok(()) => dirty.set(false),
                Err(e) => eprintln!("could not save {}: {e}", file.display()),
            });
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let (bg, fg, line) = (theme.background, theme.line_numbers, theme.line_numbers);
            div()
                .w_full()
                .h_full()
                .bg(bg)
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
                        .py(5.0)
                        .text_size(12.0)
                        .text_color(fg)
                        .border_b(1.0)
                        .border_color(line.with_alpha(0.3))
                        .child(text(move || {
                            format!(
                                "{name}{} · {}",
                                if dirty.get() { " ●" } else { "" },
                                language.name()
                            )
                        })),
                )
                .child(
                    code_editor(source)
                        .language(language)
                        .theme(theme)
                        .editor_ref(editor)
                        .on_change(move |_| dirty.set_if_changed(true))
                        .flex_1()
                        .min_h(0.0),
                )
        })
        .unwrap();
}
