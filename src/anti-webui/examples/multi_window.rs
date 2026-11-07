//! More than one window: one on the same thread that shares signals, one on a thread of its own,
//! a modal dialog, asking before closing, and a title bar where the window draws its own frame.
//! See `docs/design/36-multi-window.md`.
//!
//! `cargo run --release -p anti-webui --example multi_window`
//!
//! `ANTIWEBUI_CSD=1` asks for a frame of the window's own even where the system has one.

use anti_webui::prelude::*;
use std::time::{Duration, Instant};

fn main() {
    let decorations = if std::env::var_os("ANTIWEBUI_CSD").is_some() {
        DecorationMode::Client
    } else {
        DecorationMode::Auto
    };
    App::new()
        .title("Windows")
        .size(560, 420)
        .decorations(decorations)
        .run(move || main_window(decorations))
        .unwrap();
}

/// Keeps a window's content below its title bar, which only shows with a frame of its own.
fn framed(title: impl Into<String>, content: Div) -> Div {
    page()
        .flex()
        .flex_col()
        .size_full()
        .child(title_bar(title.into()))
        .child(content.flex_1().min_h(0.0))
}

fn main_window(decorations: DecorationMode) -> Div {
    let count = signal(0);
    let unsaved = signal(false);
    let asking = signal(false);
    let progress = shared_signal(0.0f32);
    let opened = signal(0);
    let main = current_window();

    on_close_request(move |request| {
        if unsaved.get_untracked() {
            request.prevent();
            asking.set(true);
        }
    });
    // The dialog that asks, as a window that keeps input from this one.
    effect(move || {
        if !asking.get() {
            return;
        }
        open_window(
            WindowOptions::new()
                .title("Unsaved changes")
                .size(360, 160)
                .modal(true)
                .decorations(decorations),
            {
                let main = main.clone();
                move || {
                    on_cleanup(move || asking.set(false));
                    let dialog = current_window();
                    framed(
                        "Unsaved changes",
                        div()
                            .flex()
                            .flex_col()
                            .gap(12.0)
                            .p(16.0)
                            .child(label("Close the program without saving?"))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(8.0)
                                    .justify_end()
                                    .child(button("Cancel").on_click({
                                        let dialog = dialog.clone();
                                        move |_| dialog.close()
                                    }))
                                    .child(danger_button("Close").on_click(move |_| {
                                        unsaved.set(false);
                                        dialog.close();
                                        // The window that asked closes after its dialog, and
                                        // finds nothing unsaved now.
                                        main.request_close();
                                    })),
                            ),
                    )
                }
            },
        );
    });

    let content = div()
        .flex()
        .flex_col()
        .gap(12.0)
        .p(16.0)
        .child(heading("Windows"))
        .child(label(move || {
            format!(
                "Frame: {:?}, maximized: {}, windows opened: {}",
                window_decorations(),
                window_maximized(),
                opened.get()
            )
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(8.0)
                .items_center()
                .child(button("−").on_click(move |_| count.update(|c| *c -= 1)))
                .child(text(move || format!("Shared count: {}", count.get())))
                .child(button("+").on_click(move |_| count.update(|c| *c += 1))),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(8.0)
                .child(button("Open on this thread").on_click(move |_| {
                    opened.update(|n| *n += 1);
                    open_window(
                        WindowOptions::new()
                            .title("Same thread")
                            .size(360, 220)
                            .decorations(decorations),
                        move || same_thread_window(count),
                    );
                }))
                .child(button("Open on a thread of its own").on_click({
                    let progress = progress.clone();
                    move |_| {
                        opened.update(|n| *n += 1);
                        let progress = progress.clone();
                        spawn_window(
                            WindowOptions::new()
                                .title("Own thread")
                                .size(380, 240)
                                .decorations(decorations),
                            move || own_thread_window(progress),
                        );
                    }
                }))
                .child(button("Modal dialog").on_click(move |_| {
                    open_window(
                        WindowOptions::new()
                            .title("Modal")
                            .size(300, 140)
                            .modal(true)
                            .decorations(decorations),
                        || {
                            let me = current_window();
                            framed(
                                "Modal",
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(12.0)
                                    .p(16.0)
                                    .child(label("The main window takes no input now."))
                                    .child(primary_button("OK").on_click(move |_| me.close())),
                            )
                        },
                    );
                })),
        )
        .child(label(move || {
            format!("Work on the other thread: {:.0}%", progress.get() * 100.0)
        }))
        .child(checkbox(unsaved, "Unsaved changes (closing asks first)"));
    framed("Windows", content)
}

fn same_thread_window(count: Signal<i32>) -> Div {
    framed(
        "Same thread",
        div()
            .flex()
            .flex_col()
            .gap(12.0)
            .p(16.0)
            .child(label("This window shares the count with the first one."))
            .child(text(move || format!("Count: {}", count.get())).text_size(24.0))
            .child(button("+10").on_click(move |_| count.update(|c| *c += 10))),
    )
}

fn own_thread_window(progress: SharedSignal<f32>) -> Div {
    let busy = signal(false);
    let started = signal(Instant::now());
    let shown = signal(0.0f32);
    // A frame each refresh while the work goes on, as a heavy view would.
    fn tick(
        busy: Signal<bool>,
        started: Signal<Instant>,
        shown: Signal<f32>,
        progress: SharedSignal<f32>,
    ) {
        request_frame(move |_| {
            if !busy.get_untracked() {
                return;
            }
            // Holds this thread up, and only this one.
            let until = Instant::now() + Duration::from_millis(30);
            while Instant::now() < until {}
            let done = (started.get_untracked().elapsed().as_secs_f32() / 5.0).min(1.0);
            shown.set(done);
            progress.set(done);
            if done < 1.0 {
                tick(busy, started, shown, progress);
            } else {
                busy.set(false);
            }
        });
    }
    framed(
        "Own thread",
        div()
            .flex()
            .flex_col()
            .gap(12.0)
            .p(16.0)
            .child(label(
                "Heavy work here leaves the other windows smooth: drag the first one around while it runs.",
            ))
            .child(progress_bar(move || shown.get()))
            .child(button("Work for 5 seconds").on_click(move |_| {
                if !busy.get_untracked() {
                    busy.set(true);
                    started.set(Instant::now());
                    tick(busy, started, shown, progress.clone());
                }
            })),
    )
}
