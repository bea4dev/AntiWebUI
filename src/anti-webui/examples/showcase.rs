//! A tour of the basic widgets, selectable text and text input.
//!
//! Run with `cargo run -p anti-webui --example showcase`
//! (add `--features dynamic_linking` for faster rebuilds).

use anti_webui::prelude::*;

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
            "These paragraphs are separate elements, yet one drag selects across all of them. \
             Double-click selects a word, triple-click a paragraph, and Ctrl+C copies.",
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
                        .dyn_style(|s| s.bg(theme().surface_muted))
                        .child(text("Left column: The quick brown fox jumps over the lazy dog.")),
                )
                .child(
                    div()
                        .flex_1()
                        .p(10.0)
                        .rounded(6.0)
                        .dyn_style(|s| s.bg(theme().surface_muted))
                        .child(text("Right column: いろはにほへと ちりぬるを わかよたれそ つねならむ 🎌")),
                ),
        )
        .child(
            text("Selected text is also published to the primary selection, so middle-click pastes it.")
                .dyn_style(|s| s.text_color(theme().text_muted)),
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
        "A multi-line input.\nIME works too: 日本語も入力できます。",
    ));
    let submitted = signal(String::new());
    div()
        .flex()
        .flex_col()
        .gap(8.0)
        .child(label("Name"))
        .child(
            text_field(name)
                .placeholder("Enter your name (Enter to submit)")
                .on_submit(move |v| submitted.set(v.to_owned())),
        )
        .child(text(move || {
            let n = name.get();
            if n.is_empty() {
                "Nothing entered yet".to_owned()
            } else {
                format!("Hello, {n}! ({} characters)", n.chars().count())
            }
        }))
        .child(show(
            move || !submitted.get().is_empty(),
            move || {
                text(move || format!("Submitted: {}", submitted.get()))
                    .dyn_style(|s| s.text_color(theme().success))
            },
        ))
        .child(label("Notes"))
        .child(text_area_field(note).rows(3))
}

fn controls_demo() -> Div {
    let agree = signal(true);
    let dark = signal(false);
    effect(move || {
        set_theme(if dark.get() {
            Theme::dark()
        } else {
            Theme::light()
        })
    });
    let volume = signal(40.0f32);
    div()
        .flex()
        .flex_col()
        .gap(12.0)
        .child(checkbox(agree, "I agree to the terms of service"))
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
            title: "Try fractional scaling on Wayland".into(),
            done: signal(true),
        },
        Todo {
            id: 2,
            title: "Try selecting text by dragging".into(),
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
                        .placeholder("New task")
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
                            s.strikethrough().text_color(theme().text_muted)
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
            format!("{done} / {} done", list.len())
        }))
}

fn scroll_demo() -> Div {
    scroll_view()
        .h(160.0)
        .border(1.0)
        .dyn_style(|s| s.border_color(theme().border))
        .rounded(6.0)
        .child(div().flex().flex_col().children((1..=40).map(|i| {
            div()
                .px(12.0)
                .py(6.0)
                .dyn_style(move |s| {
                    if i % 2 == 0 {
                        s.bg(theme().surface_muted)
                    } else {
                        s
                    }
                })
                .child(text(format!(
                    "Row {i} — selection keeps going while you scroll"
                )))
        })))
}

fn shadow_demo() -> Div {
    let tile = |name: &'static str| {
        div()
            .w(120.0)
            .h(72.0)
            .rounded(10.0)
            .dyn_style(|s| s.bg(theme().surface))
            .flex()
            .items_center()
            .justify_center()
            .child(label(name))
    };
    div()
        .flex()
        .flex_row()
        .gap(24.0)
        .p(12.0)
        .child(tile("shadow_sm").shadow_sm())
        .child(tile("shadow_md").shadow_md())
        .child(tile("shadow_lg").shadow_lg())
        .child(tile("inset").shadow(BoxShadow {
            offset_y: 2.0,
            blur: 8.0,
            color: rgba(0x00000040),
            inset: true,
            ..Default::default()
        }))
}

fn overlay_demo() -> Div {
    let fruit = signal(1u32);
    let in_page = signal(false);
    let tip = signal(false);
    let bubble = signal(false);
    let context = signal(None::<RectI>);
    let fruits = [
        "Apple",
        "Orange",
        "Grape",
        "Peach",
        "Pear",
        "Persimmon",
        "Strawberry",
        "Watermelon",
        "Melon",
        "Banana",
        "Kiwi",
        "Pineapple",
        "Cherry",
        "Mango",
    ];
    div()
        .flex()
        .flex_col()
        .gap(12.0)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(12.0)
                .child(
                    select(
                        fruit,
                        fruits
                            .iter()
                            .enumerate()
                            .map(|(i, f)| (i as u32, f.to_string()))
                            .collect(),
                    )
                    .w(180.0),
                )
                .child(text(move || {
                    format!("Selected: {}", fruits[fruit.get() as usize])
                })),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(12.0)
                .child(
                    // Stays in the window and is clipped like anything else.
                    div()
                        .relative()
                        .child(
                            button("In window (z_index)")
                                .on_click(move |_| in_page.update(|v| *v = !*v)),
                        )
                        .child(show(
                            move || in_page.get(),
                            || {
                                div()
                                    .absolute()
                                    .top(pct(100.0))
                                    .left(0.0)
                                    .mt(4.0)
                                    .w(200.0)
                                    .p(12.0)
                                    .z_index(10)
                                    .rounded(8.0)
                                    .dyn_style(|s| s.bg(theme().surface))
                                    .border(1.0)
                                    .dyn_style(|s| s.border_color(theme().border))
                                    .shadow_lg()
                                    .child(text("Drawn above the elements behind it"))
                            },
                        )),
                )
                .child(
                    div()
                        .child(button("Tooltip").on_hover(move |inside| tip.set(inside)))
                        .child(show(
                            move || tip.get(),
                            || {
                                popup()
                                    .kind(PopupKind::Tooltip)
                                    .placement(Placement::AboveStart)
                                    .gap(6)
                                    .child(
                                        div()
                                            .px(8.0)
                                            .py(4.0)
                                            .rounded(4.0)
                                            .bg(rgb(0x1f2328))
                                            .text_color(Color::WHITE)
                                            .text_size(12.0)
                                            .shadow_md()
                                            .child(text("Input passes through to what is below")),
                                    )
                            },
                        )),
                )
                .child(
                    div()
                        .child(
                            button("Bubble (subsurface)")
                                .on_click(move |_| bubble.update(|v| *v = !*v)),
                        )
                        .child(show(
                            move || bubble.get(),
                            move || {
                                popup()
                                    .kind(PopupKind::Bubble)
                                    .placement(Placement::RightStart)
                                    .gap(8)
                                    .child(
                                        card()
                                            .shadow_lg()
                                            .w(220.0)
                                            .child(text("Stays until you close it"))
                                            .child(
                                                button("Close")
                                                    .on_click(move |_| bubble.set(false)),
                                            ),
                                    )
                            },
                        )),
                ),
        )
        .child(
            div()
                .p(16.0)
                .rounded(6.0)
                .dyn_style(|s| s.bg(theme().surface_muted))
                .child(text("Right-click here to open a menu"))
                .on_pointer_down(move |e| {
                    if e.button == Some(PointerButton::Secondary) {
                        let (x, y) = (e.position.x as i32, e.position.y as i32);
                        context.set(Some(RectI::new(x, y, 1, 1)));
                    }
                })
                .child(dyn_view(move || {
                    context.get().map(|at| {
                        popup()
                            .anchor_rect(at)
                            .on_dismiss(move || context.set(None))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .p(4.0)
                                    .w(160.0)
                                    .rounded(8.0)
                                    .border(1.0)
                                    .dyn_style(|s| s.border_color(theme().border))
                                    .dyn_style(|s| s.bg(theme().surface))
                                    .shadow_lg()
                                    .children(["Copy", "Paste", "Delete"].map(|name| {
                                        div()
                                            .px(8.0)
                                            .py(5.0)
                                            .rounded(4.0)
                                            .cursor(CursorIcon::Pointer)
                                            .hover(|s| s.bg(theme().hover))
                                            .on_click(move |_| context.set(None))
                                            .child(text(name))
                                    })),
                            )
                    })
                })),
        )
        // Painted after what comes later only because of its `z_index`.
        .child(
            div()
                .relative()
                .h(70.0)
                .child(div().flex().flex_row().children((0..4).map(|i| {
                    div()
                        .w(90.0)
                        .h(60.0)
                        .ml(if i == 0 { 0.0 } else { -24.0 })
                        .mt(i as f32 * 3.0)
                        .rounded(8.0)
                        .bg([rgb(0xffd7d5), rgb(0xd8f3dc), rgb(0xcfe2ff), rgb(0xfff3bf)][i])
                        .border(1.0)
                        .dyn_style(|s| s.border_color(theme().border_strong))
                        .shadow_sm()
                        .z_index(4 - i as i32)
                        .hover(|s| s.z_index(9))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(label(format!("z {}", 4 - i)))
                }))),
        )
}

/// The whole showcase page. Also used by the `bench_slider` example.
pub fn root() -> Div {
    page().child(
        scroll_view().h_full().child(
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
                            text("A lightweight native UI framework drawn with wgpu")
                                .dyn_style(|s| s.text_color(theme().text_muted)),
                        ),
                )
                .child(section("Selectable text", selection_demo()))
                .child(section("Counter", counter_demo()))
                .child(section("Text input & IME", input_demo()))
                .child(section("Controls", controls_demo()))
                .child(section("Keyed list", todo_demo()))
                .child(section("Scroll view", scroll_demo()))
                .child(section("Shadows", shadow_demo()))
                .child(section("Overlays", overlay_demo())),
        ),
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
