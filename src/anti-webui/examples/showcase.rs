//! A tour of the basic widgets, selectable text and text input.
//!
//! Run with `cargo run -p anti-webui --example showcase`
//! (add `--features dynamic_linking` for faster rebuilds).

use anti_webui::prelude::*;
use anti_webui::widgets::theme;

#[derive(Clone)]
struct Todo {
    id: u32,
    title: String,
    done: Signal<bool>,
}

fn section(title: &str, body: impl IntoView) -> Div {
    card().child(heading(title.to_owned())).child(body)
}

fn selection_demo() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(8.0)
        .child(text(
            "ここに並んだ文章は、別々の要素に分かれていてもドラッグで一度に選択できます。\
             ダブルクリックで単語、トリプルクリックで段落を選択し、Ctrl+C でコピーできます。",
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(12.0)
                .child(
                    div()
                        .flex_1()
                        .p(10.0)
                        .rounded(6.0)
                        .bg(theme::SURFACE_MUTED)
                        .child(text("Left column: The quick brown fox jumps over the lazy dog.")),
                )
                .child(
                    div()
                        .flex_1()
                        .p(10.0)
                        .rounded(6.0)
                        .bg(theme::SURFACE_MUTED)
                        .child(text("右の列: いろはにほへと ちりぬるを わかよたれそ つねならむ 🎌")),
                ),
        )
        .child(
            text("Selected text is also published to the primary selection, so middle-click pastes it.")
                .text_color(theme::TEXT_MUTED),
        )
}

fn counter_demo() -> Div {
    let count = signal(0);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0)
        .child(button("−1").on_click(move |_| count.update(|c| *c -= 1)))
        .child(
            text(move || format!("{}", count.get()))
                .text_size(18.0)
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(primary_button("+1").on_click(move |_| count.update(|c| *c += 1)))
        .child(button("Reset").on_click(move |_| count.set(0)))
}

fn input_demo() -> Div {
    let name = signal(String::new());
    let note = signal(String::from(
        "複数行の入力欄です。\nIME（日本語入力）も使えます。",
    ));
    let submitted = signal(String::new());
    div()
        .flex()
        .flex_col()
        .gap(8.0)
        .child(label("Name"))
        .child(
            text_field(name)
                .placeholder("お名前を入力（Enter で送信）")
                .on_submit(move |v| submitted.set(v.to_owned())),
        )
        .child(text(move || {
            let n = name.get();
            if n.is_empty() {
                "まだ入力されていません".to_owned()
            } else {
                format!("こんにちは、{n} さん！ ({} 文字)", n.chars().count())
            }
        }))
        .child(show(
            move || !submitted.get().is_empty(),
            move || {
                text(move || format!("送信された値: {}", submitted.get()))
                    .text_color(theme::SUCCESS)
            },
        ))
        .child(label("Notes"))
        .child(text_area_field(note).rows(3))
}

fn controls_demo() -> Div {
    let agree = signal(true);
    let dark = signal(false);
    let volume = signal(40.0f32);
    div()
        .flex()
        .flex_col()
        .gap(12.0)
        .child(checkbox(agree, "利用規約に同意する"))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0)
                .child(switch(dark))
                .child(text(move || {
                    if dark.get() {
                        "Switch: ON"
                    } else {
                        "Switch: OFF"
                    }
                })),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(12.0)
                .child(slider(volume, 0.0, 100.0).w(200.0))
                .child(text(move || format!("{:.0}%", volume.get())).w(40.0)),
        )
        .child(
            primary_button("Agree to continue")
                .self_start()
                .dyn_style(move |s| if agree.get() { s } else { s.opacity(0.5) })
                .disabled(move || !agree.get()),
        )
}

fn todo_demo() -> Div {
    let next_id = signal(3u32);
    let todos = signal(vec![
        Todo {
            id: 1,
            title: "Wayland で fractional scaling を試す".into(),
            done: signal(true),
        },
        Todo {
            id: 2,
            title: "テキスト選択をドラッグで試す".into(),
            done: signal(false),
        },
    ]);
    let draft = signal(String::new());
    let add = move || {
        let title = draft.get_untracked().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let id = next_id.get_untracked();
        next_id.set(id + 1);
        todos.update(|t| {
            t.push(Todo {
                id,
                title,
                done: signal(false),
            })
        });
        draft.set(String::new());
    };
    div()
        .flex()
        .flex_col()
        .gap(8.0)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(8.0)
                .child(
                    text_field(draft)
                        .flex_1()
                        .placeholder("新しいタスク")
                        .on_submit(move |_| add()),
                )
                .child(primary_button("Add").on_click(move |_| add())),
        )
        .child(for_each(
            move || todos.get(),
            |t| t.id,
            move |todo| {
                let id = todo.id;
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(8.0)
                    .py(4.0)
                    .child(checkbox(todo.done, ""))
                    .child(text(todo.title.clone()).flex_1().dyn_style(move |s| {
                        if todo.done.get() {
                            s.strikethrough().text_color(theme::TEXT_MUTED)
                        } else {
                            s
                        }
                    }))
                    .child(
                        button("Delete")
                            .on_click(move |_| todos.update(|t| t.retain(|x| x.id != id))),
                    )
            },
        ))
        .child(label(move || {
            let list = todos.get();
            let done = list.iter().filter(|t| t.done.get()).count();
            format!("{done} / {} 完了", list.len())
        }))
}

fn scroll_demo() -> Div {
    scroll_view()
        .h(160.0)
        .border(1.0)
        .border_color(theme::BORDER)
        .rounded(6.0)
        .child(div().flex().flex_col().children((1..=40).map(|i| {
            div()
                .px(12.0)
                .py(6.0)
                .dyn_style(move |s| {
                    if i % 2 == 0 {
                        s.bg(theme::SURFACE_MUTED)
                    } else {
                        s
                    }
                })
                .child(text(format!(
                    "Row {i} — スクロールしても選択を続けられます"
                )))
        })))
}

/// The whole showcase page. Also used by the `bench_slider` example.
pub fn root() -> Div {
    scroll_view().h_full().bg(theme::SURFACE_MUTED).child(
        div()
            .flex()
            .flex_col()
            .gap(16.0)
            .p(24.0)
            .max_w(860.0)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(4.0)
                    .child(
                        text("AntiWebUI")
                            .text_size(28.0)
                            .font_weight(FontWeight::BOLD),
                    )
                    .child(
                        text("wgpu で描画する軽量ネイティブ UI フレームワーク")
                            .text_color(theme::TEXT_MUTED),
                    ),
            )
            .child(section("Selectable text", selection_demo()))
            .child(section("Counter", counter_demo()))
            .child(section("Text input & IME", input_demo()))
            .child(section("Controls", controls_demo()))
            .child(section("Keyed list", todo_demo()))
            .child(section("Scroll view", scroll_demo())),
    )
}

#[allow(dead_code)]
fn main() {
    App::new()
        .title("AntiWebUI Showcase")
        .size(900, 760)
        .run(root)
        .unwrap();
}
