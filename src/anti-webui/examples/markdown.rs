//! A Markdown viewer that follows its file: save the file in an editor and it shows the change.
//!
//! ```sh
//! cargo run --release -p anti-webui --example markdown --features markdown -- README.md
//! ```
//!
//! Pictures are found next to the file. Links to headings scroll to them, and other links are
//! printed. `:name:` with a picture `emoji/name.png` next to the file shows that picture.

use anti_webui::prelude::*;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: markdown <file.md>");
        return;
    };
    let started = Instant::now();
    App::new()
        .title("Markdown")
        .size(1000, 900)
        .run(move || {
            let read =
                |p: &PathBuf| std::fs::read_to_string(p).unwrap_or_else(|e| format!("`{e}`"));
            let source = signal(read(&path));
            let tx = ui_channel(move |s: String| source.set(s));
            let watched = path.clone();
            std::thread::spawn(move || {
                let modified = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                let mut last: Option<SystemTime> = modified(&watched);
                loop {
                    std::thread::sleep(Duration::from_millis(300));
                    let now = modified(&watched);
                    if now != last {
                        last = now;
                        tx.send(read(&watched));
                    }
                }
            });
            let dir = path.parent().map(PathBuf::from).unwrap_or_default();
            let emoji_dir = dir.join("emoji");
            let pictures = dir.clone();
            request_frame(move |_| {
                eprintln!(
                    "first frame after {:.0} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            });
            div()
                .w_full()
                .h_full()
                .bg(Color::WHITE)
                .overflow_y_scroll()
                .child(
                    div().flex().justify_center().child(
                        markdown_view(move || source.get())
                            .on_link(|url| println!("link: {url}"))
                            .image(move |src| {
                                let p = PathBuf::from(src);
                                ImageSource::from(if p.is_absolute() {
                                    p
                                } else {
                                    pictures.join(p)
                                })
                            })
                            .emoji(move |name| {
                                let p = emoji_dir.join(format!("{name}.png"));
                                p.exists().then(|| Emoji::Image(ImageSource::from(p)))
                            })
                            .w_full()
                            .max_w(980.0)
                            .p(32.0),
                    ),
                )
        })
        .unwrap();
}
