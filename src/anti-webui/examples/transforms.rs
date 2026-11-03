//! Transforms and animations by hand: things that keep moving, cards that grow under the pointer,
//! `transition`, `animated`, `enter` and `exit` (of a list too), and transformed rows in a list, to check that clicks and scrolling
//! land where things are shown (see `docs/design/12-transform.md`, `docs/design/13-animation.md`).
//!
//! `cargo run --release -p anti-webui --example transforms`

use anti_webui::prelude::*;
use std::time::Instant;

/// Keeps `t` at the seconds since the first frame.
fn tick(t: Signal<f32>, start: Option<Instant>) {
    request_frame(move |now| {
        let start = start.unwrap_or(now);
        t.set(now.saturating_duration_since(start).as_secs_f32());
        tick(t, Some(start));
    });
}

fn card(label: &'static str) -> Div {
    div()
        .w(190.0)
        .h(80.0)
        .p(10.0)
        .rounded(10.0)
        .border(1.0)
        .border_color(rgb(0x3355aa))
        .bg(rgb(0xddeeff))
        .shadow_md()
        .child(text(label))
        .child(text("日本語のテキスト abc 0123"))
}

fn button(label: &'static str) -> Div {
    div()
        .px(12.0)
        .py(4.0)
        .rounded(6.0)
        .bg(rgb(0xe8e8e8))
        .hover(|s| s.bg(rgb(0xd0d0d0)))
        .child(text(label))
}

fn main() {
    App::new()
        .title("transforms")
        .size(900, 1060)
        .run(|| {
            let t = signal(0.0_f32);
            tick(t, None);
            let list = signal(vec![0, 1, 2, 3]);
            let next = signal(4);
            let clicks = signal(0);
            let tween = animated(0.0_f32);
            let spring = animated(0.0_f32);
            let flip = signal(false);
            let panel = signal(false);
            let menu = signal(false);
            let typed = signal(String::new());
            let wave = move |speed: f32| (t.get() * speed).sin();
            div()
                .size_full()
                .bg(Color::WHITE)
                .p(24.0)
                .flex()
                .flex_col()
                .gap(36.0)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(40.0)
                        .child(
                            card("translate").dyn_style(move |s| s.translate_x(wave(1.5) * 20.0)),
                        )
                        .child(card("rotate + scale").dyn_style(move |s| {
                            s.rotate(t.get() * 40.0).scale(1.0 + wave(2.0) * 0.2)
                        }))
                        .child(card("opacity").dyn_style(move |s| {
                            s.translate_y(wave(1.0) * 6.0)
                                .opacity(0.5 + wave(3.0) * 0.5)
                        })),
                )
                .child(
                    div()
                        .mt(30.0)
                        .flex()
                        .flex_row()
                        .gap(40.0)
                        .child(card("hover: grows").hover(|s| s.scale(1.1)))
                        .child(card("hover: tilts").hover(|s| s.rotate(5.0)))
                        .child(
                            card("rotated: click me")
                                .rotate(-8.0)
                                .hover(|s| s.bg(rgb(0xffeecc)))
                                .on_click(move |_| clicks.update(|c| *c += 1))
                                .child(text(move || format!("clicks: {}", clicks.get()))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(40.0)
                        .child(
                            card("transition: hover")
                                .hover(|s| s.scale(1.08).bg(rgb(0xffeecc)).shadow_lg())
                                .transition(Transition::ms(180).ease_out()),
                        )
                        .child(
                            card("animated: click")
                                .dyn_style(move |s| s.translate_x(tween.get()))
                                .on_click(move |_| {
                                    let to = if tween.get_untracked() > 30.0 {
                                        0.0
                                    } else {
                                        60.0
                                    };
                                    tween.to(to, Transition::ms(300).ease_in_out());
                                }),
                        )
                        .child(
                            card("spring: click, again midway")
                                .dyn_style(move |s| s.translate_y(spring.get()))
                                .on_click(move |_| {
                                    let to = if flip.get_untracked() { 0.0 } else { 40.0 };
                                    flip.set(!flip.get_untracked());
                                    spring.spring_to(
                                        to,
                                        Spring {
                                            stiffness: 300.0,
                                            damping: 12.0,
                                            mass: 1.0,
                                        },
                                    );
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(16.0)
                        .items_center()
                        .child(
                            card("enter / exit: click, again midway")
                                .on_click(move |_| panel.update(|v| *v = !*v)),
                        )
                        .child(
                            show(
                                move || panel.get(),
                                move || {
                                    // Text stays while it goes and when it comes back midway.
                                    card("in and out").child(text_input().w(160.0).value(typed))
                                },
                            )
                            .enter(|s| s.opacity(0.0).translate_x(-20.0))
                            .exit(|s| s.opacity(0.0).scale(0.9))
                            .transition(Transition::ms(600).ease_out()),
                        )
                        .child(
                            card("menu: click").on_click(move |_| menu.set(true)).child(
                                show(
                                    move || menu.get(),
                                    move || {
                                        popup().on_dismiss(move || menu.set(false)).child(
                                            card("click to close")
                                                .on_click(move |_| menu.set(false)),
                                        )
                                    },
                                )
                                .enter(|s| s.opacity(0.0).scale(0.9))
                                .exit(|s| s.opacity(0.0))
                                .transition(Transition::ms(150).ease_out()),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(16.0)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(8.0)
                                .child(button("add").on_click(move |_| {
                                    let n = next.get_untracked();
                                    next.set(n + 1);
                                    list.update(|l| l.insert(l.len() / 2, n));
                                }))
                                .child(button("shuffle").on_click(move |_| {
                                    list.update(|l| l.rotate_left(1));
                                }))
                                .child(button("replace all").on_click(move |_| {
                                    let n = next.get_untracked();
                                    next.set(n + 3);
                                    list.set(vec![n, n + 1, n + 2]);
                                })),
                        )
                        .child(
                            div().w(260.0).h(170.0).overflow_y_scroll().child(
                                for_each(
                                    move || list.get(),
                                    |i| *i,
                                    move |i| {
                                        div()
                                            .w(240.0)
                                            .h(30.0)
                                            .mb(4.0)
                                            .px(8.0)
                                            .rounded(6.0)
                                            .overflow_hidden()
                                            .bg(rgb(0xddeeff))
                                            .hover(|s| s.bg(rgb(0xbbd4ff)))
                                            .on_click(move |_| {
                                                list.update(|l| l.retain(|v| *v != i));
                                            })
                                            .child(text(format!("item {i}: click to remove")))
                                    },
                                )
                                .enter(|s| s.opacity(0.0).translate_x(-20.0))
                                .exit(|s| s.opacity(0.0).translate_x(20.0))
                                .transition(Transition::ms(300).ease_out())
                                .moves(Transition::ms(600).ease_out()),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(0.0))
                        .rounded(8.0)
                        .border(1.0)
                        .overflow_y_scroll()
                        .children((0..200).map(|i| {
                            let row = div()
                                .h(28.0)
                                .px(8.0)
                                .hover(|s| s.bg(rgb(0xeef2ff)))
                                .child(text(format!("row {i}")));
                            if i % 5 == 0 {
                                row.bg(rgb(0xeeffee))
                                    .dyn_style(move |s| s.translate_x(10.0 + wave(2.0) * 10.0))
                            } else {
                                row
                            }
                        })),
                )
        })
        .unwrap();
}
