//! How far the standard widgets' look can be changed, level by level, with the code next to what
//! it gives. Every row is bound to the same two signals, so they all move together: what they do
//! is the same, only the look differs.
//!
//! `cargo run --release -p anti-webui-md3e --example custom_levels`
//! `cargo run --release -p anti-webui-md3e --example custom_levels -- out.png` writes a PNG instead.

use anti_webui::prelude::*;
use anti_webui::widgets::{SliderOptions, SliderParts, SliderState, SwitchParts, SwitchState};
use anti_webui_md3e::components::{self as md3e, SliderStyle, SwitchStyle, sized_icon};
use anti_webui_md3e::motion::{default_spatial, fast_effects, fast_spatial};
use anti_webui_md3e::{color_scheme, icons};

const MONO: &str = "\"JetBrains Mono\", \"Cascadia Code\", \"DejaVu Sans Mono\", monospace";
const ORANGE: Color = rgb(0xea580c);
const ORANGE_DARK: Color = rgb(0xc2410c);
const GRAY: Color = rgb(0xd6d3d1);

/// A value that follows `target` like a spring.
fn follow(target: impl Fn() -> f32 + 'static, spring: Spring) -> Animated<f32> {
    let value = animated(untrack(&target));
    effect(move || {
        let to = target();
        untrack(|| value.spring_to(to, spring));
    });
    value
}

fn shadow(x: f32, y: f32, blur: f32, color: Color, inset: bool) -> BoxShadow {
    BoxShadow {
        offset_x: x,
        offset_y: y,
        blur,
        spread: 0.0,
        color,
        inset,
    }
}

struct Level {
    tag: &'static str,
    title: &'static str,
    note: &'static str,
    code: &'static str,
    samples: Vec<(&'static str, View)>,
}

fn sample(label: &'static str, view: impl IntoView) -> (&'static str, View) {
    (label, view.into_view())
}

fn level(l: Level) -> Div {
    let tag_color = if l.tag.contains("新") {
        rgb(0x7c3aed)
    } else if l.tag.contains("参考") {
        rgb(0x57534e)
    } else {
        rgb(0x15803d)
    };
    div()
        .flex()
        .flex_row()
        .gap(20.0)
        .p(16.0)
        .rounded(12.0)
        .bg(Color::WHITE)
        .border(1.0)
        .border_color(rgb(0xe7e5e4))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(8.0)
                .w(620.0)
                .flex_none()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(8.0)
                        .child(
                            div()
                                .px(8.0)
                                .py(2.0)
                                .rounded(6.0)
                                .bg(tag_color)
                                .text_color(Color::WHITE)
                                .text_size(12.0)
                                .child(l.tag),
                        )
                        .child(div().text_size(16.0).bold().child(l.title)),
                )
                .child(
                    div()
                        .text_size(13.0)
                        .text_color(rgb(0x57534e))
                        .child(l.note),
                )
                .child(
                    div()
                        .p(10.0)
                        .rounded(8.0)
                        .bg(rgb(0xf5f5f4))
                        .font_family(MONO)
                        .text_size(12.0)
                        .child(l.code),
                ),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .gap(14.0)
                .children(l.samples.into_iter().map(|(label, view)| {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(12.0)
                        .child(
                            div()
                                .w(72.0)
                                .flex_none()
                                .text_size(12.0)
                                .text_color(rgb(0x78716c))
                                .child(label),
                        )
                        .child(view)
                })),
        )
}

fn with_value(volume: Signal<f32>, slider: Div) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(12.0)
        .child(slider.w(240.0))
        .child(
            div()
                .w(32.0)
                .text_size(12.0)
                .text_color(rgb(0x78716c))
                .child(text(move || format!("{:.0}", volume.get()))),
        )
}

/// Lv4: a switch drawn like Material 3 Expressive's, on the standard switch's behavior.
fn md3e_like_switch(on: Signal<bool>) -> Div {
    switch_with(on, |s: SwitchState| {
        let moved = follow(move || if s.on() { 1.0 } else { 0.0 }, default_spatial());
        let size = follow(
            move || match (s.pressed(), s.on()) {
                (true, _) => 28.0,
                (false, true) => 24.0,
                (false, false) => 16.0,
            },
            fast_spatial(),
        );
        let shown = follow(move || if s.on() { 1.0 } else { 0.0 }, fast_effects());
        div()
            .flex_none()
            .w(52.0)
            .h(32.0)
            .rounded(16.0)
            .border(2.0)
            .flex()
            .flex_row()
            .items_center()
            .cursor(CursorIcon::Pointer)
            .dyn_style(move |st| {
                let c = color_scheme();
                let st = if s.on() {
                    st.bg(c.primary).border_color(c.primary)
                } else {
                    st.bg(c.surface_container_highest).border_color(c.outline)
                };
                if s.focus_visible() {
                    st.outline(3.0)
                        .outline_offset(2.0)
                        .outline_color(c.secondary)
                } else {
                    st
                }
            })
            .child(div().dyn_style(move |st| st.flex_grow(moved.get())))
            .child(
                div()
                    .flex_none()
                    .w(28.0)
                    .h(28.0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .dyn_style(move |st| {
                                let c = color_scheme();
                                let d = size.get();
                                st.w(d).h(d).rounded(d / 2.0).bg(if s.on() {
                                    c.on_primary
                                } else {
                                    c.outline
                                })
                            })
                            .child(sized_icon(icons::CHECK, 16.0).dyn_style(move |st| {
                                st.opacity(shown.get())
                                    .text_color(color_scheme().on_primary_container)
                            })),
                    ),
            )
    })
}

/// Lv4: a slider drawn like Material 3 Expressive's, on the standard slider's behavior.
fn md3e_like_slider(volume: Signal<f32>) -> Div {
    slider_with(
        volume,
        0.0,
        100.0,
        SliderOptions::new(),
        |s: SliderState| {
            let held = move || s.dragging() || s.focus_visible();
            let handle = follow(move || if held() { 2.0 } else { 4.0 }, fast_spatial());
            let bubble = follow(move || if held() { 1.0 } else { 0.0 }, fast_spatial());
            let bar = |grow: fn(SliderState) -> f32, color: fn() -> Color| {
                div()
                    .h(16.0)
                    .rounded(4.0)
                    .dyn_style(move |st| st.flex_grow(grow(s)).bg(color()))
            };
            div()
                .relative()
                .h(44.0)
                .flex()
                .flex_row()
                .items_center()
                .cursor(CursorIcon::Pointer)
                .child(bar(|s| s.fraction(), || color_scheme().primary))
                .child(div().w(6.0).flex_none())
                .child(
                    div()
                        .flex_none()
                        .h(44.0)
                        .rounded(2.0)
                        .dyn_style(move |st| st.w(handle.get()).bg(color_scheme().primary)),
                )
                .child(div().w(6.0).flex_none())
                .child(bar(
                    |s| 1.0 - s.fraction(),
                    || color_scheme().secondary_container,
                ))
                // The value above the handle, on a rail that puts its middle over the handle's.
                .child(
                    div()
                        .absolute()
                        .left(-12.0)
                        .right(-8.0)
                        .top(-40.0)
                        .h(36.0)
                        .flex()
                        .flex_row()
                        .child(div().dyn_style(move |st| st.flex_grow(s.fraction())))
                        .child(
                            div()
                                .flex_none()
                                .w(40.0)
                                .h(36.0)
                                .rounded(18.0)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(13.0)
                                .transform_origin(0.5, 1.0)
                                .dyn_style(move |st| {
                                    let c = color_scheme();
                                    let b = bubble.get();
                                    st.bg(c.inverse_surface)
                                        .text_color(c.inverse_on_surface)
                                        .opacity(b.clamp(0.0, 1.0))
                                        .scale(b.max(0.0))
                                })
                                .child(text(move || format!("{:.0}", s.value()))),
                        )
                        .child(div().dyn_style(move |st| st.flex_grow(1.0 - s.fraction()))),
                )
        },
    )
}

/// Lv4: a slider that is a row of lights, nothing like the standard one.
fn meter_slider(volume: Signal<f32>) -> Div {
    const N: usize = 12;
    slider_with(
        volume,
        0.0,
        100.0,
        SliderOptions::new(),
        |s: SliderState| {
            div()
                .h(32.0)
                .flex()
                .flex_row()
                .items_end()
                .gap(4.0)
                .rounded(4.0)
                .cursor(CursorIcon::Pointer)
                .dyn_style(move |st| {
                    if s.focus_visible() {
                        st.outline(2.0).outline_offset(3.0).outline_color(ORANGE)
                    } else {
                        st
                    }
                })
                .children((0..N).map(move |i| {
                    let t = i as f32 / (N - 1) as f32;
                    let lit = Color::new(0.13 + 0.8 * t, 0.77 - 0.55 * t, 0.37 - 0.2 * t, 1.0);
                    div()
                        .flex_1()
                        .h(10.0 + 22.0 * t)
                        .rounded(3.0)
                        .dyn_style(move |st| {
                            let on = (i as f32 + 0.5) / N as f32 <= s.fraction();
                            st.bg(if on { lit } else { rgb(0xe7e5e4) })
                        })
                }))
        },
    )
}

/// Lv4: a day and night switch, nothing like the standard one.
fn day_night_switch(on: Signal<bool>) -> Div {
    switch_with(on, |s: SwitchState| {
        let moved = follow(move || if s.on() { 1.0 } else { 0.0 }, default_spatial());
        div()
            .flex_none()
            .w(84.0)
            .h(34.0)
            .p(4.0)
            .rounded(17.0)
            .flex()
            .flex_row()
            .items_center()
            .cursor(CursorIcon::Pointer)
            .transition_of([Prop::Background], Transition::ms(250).ease_out())
            .dyn_style(move |st| {
                let st = st.bg(if s.on() { rgb(0x1e1b4b) } else { rgb(0x7dd3fc) });
                if s.focus_visible() {
                    st.outline(2.0).outline_offset(2.0).outline_color(ORANGE)
                } else {
                    st
                }
            })
            .child(div().dyn_style(move |st| st.flex_grow(moved.get())))
            .child(
                div()
                    .flex_none()
                    .w(26.0)
                    .h(26.0)
                    .rounded(13.0)
                    .transition_of(
                        [Prop::Background, Prop::Shadow],
                        Transition::ms(250).ease_out(),
                    )
                    .dyn_style(move |st| {
                        if s.on() {
                            st.bg(rgb(0xe2e8f0)).shadow(shadow(
                                -6.0,
                                -3.0,
                                0.0,
                                rgb(0x1e1b4b),
                                true,
                            ))
                        } else {
                            st.bg(rgb(0xfacc15)).shadow(shadow(
                                0.0,
                                0.0,
                                10.0,
                                rgba(0xfde04799),
                                false,
                            ))
                        }
                    }),
            )
    })
}

fn page() -> Div {
    let on = signal(true);
    let volume = signal(40.0f32);
    let pink = signal(false);
    effect(move || {
        set_theme(if pink.get() {
            Theme {
                accent: rgb(0xdb2777),
                accent_hover: rgb(0xbe185d),
                accent_pressed: rgb(0x9d174d),
                success: rgb(0xdb2777),
                focus_ring: rgba(0xdb277799),
                ..Theme::light()
            }
        } else {
            Theme::light()
        });
    });

    let levels = [
        Level {
            tag: "Lv0 今",
            title: "そのまま",
            note: "標準の見た目。",
            code: "primary_button(\"保存\")\nswitch(on)\nslider(volume, 0.0, 100.0)",
            samples: vec![
                sample("ボタン", primary_button("保存")),
                sample("スイッチ", switch(on)),
                sample("スライダー", with_value(volume, slider(volume, 0.0, 100.0))),
            ],
        },
        Level {
            tag: "Lv1 今",
            title: "テーマ（色だけ、アプリ全体）",
            note: "色 39 個を差し替える。窓の中の標準部品すべてに効き、一部だけには効かない。寸法や形は変わらない。右のスイッチで Lv0 の行も一緒に変わる。",
            code: "set_theme(Theme {\n    accent: rgb(0xdb2777),\n    success: rgb(0xdb2777),\n    ..Theme::light()\n})",
            samples: vec![sample(
                "テーマ",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(10.0)
                    .child(switch(pink))
                    .child(div().text_size(13.0).child("ピンクのテーマ")),
            )],
        },
        Level {
            tag: "Lv2 今",
            title: "外側の箱にスタイル（CSS で要素に書くのと同じ）",
            note: "返ってくる Div に続けて書く。テーマより強く、ホバー中も押下中も勝つ。ボタンは中身を自分で渡すのでほぼ何でもできるが、スイッチのつまみやスライダーの溝のような中の部品には届かない。",
            code: "primary_button(\"保存\")\n    .h(40.0).px(24.0).rounded(20.0).bg(ORANGE).border_color(ORANGE)\n    .hover(|s| s.bg(ORANGE_DARK))\n\nswitch(on).w(56.0).h(30.0).rounded(8.0)\n    .dyn_style(move |s| s.bg(if on.get() { ORANGE } else { GRAY }))\n    // つまみは白い 16px の丸のまま\n\nslider(volume, 0.0, 100.0)\n    .h(32.0).px(10.0).bg(rgb(0xfff7ed)).rounded(16.0)\n    // 溝とつまみは標準のまま",
            samples: vec![
                sample(
                    "ボタン",
                    primary_button("保存")
                        .h(40.0)
                        .px(24.0)
                        .rounded(20.0)
                        .bg(ORANGE)
                        .border_color(ORANGE)
                        .hover(|s| s.bg(ORANGE_DARK)),
                ),
                sample(
                    "スイッチ",
                    switch(on)
                        .w(56.0)
                        .h(30.0)
                        .rounded(8.0)
                        .dyn_style(move |s| s.bg(if on.get() { ORANGE } else { GRAY })),
                ),
                sample(
                    "スライダー",
                    with_value(
                        volume,
                        slider(volume, 0.0, 100.0)
                            .h(32.0)
                            .px(10.0)
                            .bg(rgb(0xfff7ed))
                            .rounded(16.0),
                    ),
                ),
            ],
        },
        Level {
            tag: "Lv3 新",
            title: "部品ごとにスタイル（CSS の ::part や ::-webkit-slider-thumb に当たる）",
            note: "標準の見た目のまま、名前の付いた部品を受け取って書き足す。部品は Div そのものなので .hover() や .child() も使え、状態（押下中、ドラッグ中）も読める。部品の数と並びは変えられない。",
            code: "styled_switch(on, SwitchParts::new()\n    .track(|t, s| t.w(56.0).h(30.0).p(3.0).rounded(8.0)\n        .dyn_style(move |st| st.bg(if s.on() { ORANGE } else { GRAY })))\n    .thumb(|t, s| t.w(24.0).h(24.0).rounded(5.0)\n        .dyn_style(move |st| st.scale(if s.pressed() { 0.8 } else { 1.0 }))))\n\nstyled_slider(volume, 0.0, 100.0, SliderParts::new()\n    .track(|t, _| t.h(8.0).rounded(4.0))\n    .fill(|f, _| f.rounded(4.0).bg(ORANGE))\n    .thumb(|t, s| t.w(22.0).h(22.0).rounded(6.0).border(0.0).bg(ORANGE)\n        .dyn_style(move |st| st.scale(if s.dragging() { 1.25 } else { 1.0 }))))",
            samples: vec![
                sample(
                    "スイッチ",
                    styled_switch(
                        on,
                        SwitchParts::new()
                            .track(|t, s| {
                                t.w(56.0)
                                    .h(30.0)
                                    .p(3.0)
                                    .rounded(8.0)
                                    .dyn_style(move |st| st.bg(if s.on() { ORANGE } else { GRAY }))
                            })
                            .thumb(|t, s| {
                                t.w(24.0)
                                    .h(24.0)
                                    .rounded(5.0)
                                    .transition_of([Prop::Scale], Transition::ms(120).ease_out())
                                    .dyn_style(move |st| {
                                        st.scale(if s.pressed() { 0.8 } else { 1.0 })
                                    })
                            }),
                    ),
                ),
                sample(
                    "スライダー",
                    with_value(
                        volume,
                        styled_slider(
                            volume,
                            0.0,
                            100.0,
                            SliderParts::new()
                                .track(|t, _| t.h(8.0).rounded(4.0))
                                .fill(|f, _| f.rounded(4.0).bg(ORANGE))
                                .thumb(|t, s| {
                                    t.w(22.0)
                                        .h(22.0)
                                        .rounded(6.0)
                                        .border(0.0)
                                        .bg(ORANGE)
                                        .transition_of(
                                            [Prop::Scale],
                                            Transition::ms(120).ease_out(),
                                        )
                                        .dyn_style(move |st| {
                                            st.scale(if s.dragging() { 1.25 } else { 1.0 })
                                        })
                                }),
                        ),
                    ),
                ),
            ],
        },
        Level {
            tag: "Lv4 新",
            title: "見た目を丸ごと（動きだけ借りる）",
            note: "部品は状態（値、押下、ドラッグ、ホバー、キーボードのフォーカス）を渡すだけで、見た目の木は全部自分で組む。ばね、アイコン、吹き出しも自由。ポインタ、キー（矢印、PageUp/Down、Home/End）、フォーカスは標準と同じ。md3e もこの形で載せてある。",
            code: "switch_with(on, |s: SwitchState| {\n    // s.on() / s.pressed() / s.hovered() / s.focus_visible() を読んで\n    // 木を自由に組む（ばねで動くつまみ、中のアイコンなど）\n    div()...\n})\n\nslider_with(volume, 0.0, 100.0, SliderOptions::new(), |s: SliderState| {\n    // s.fraction() / s.dragging() / s.focus_visible() を読む\n    div()...\n})\n// 全部のコードはこのファイルの md3e_like_switch などにある",
            samples: vec![
                sample("md3e 風", md3e_like_switch(on)),
                sample("md3e 風", with_value(volume, md3e_like_slider(volume))),
                sample("自由", day_night_switch(on)),
                sample("自由", with_value(volume, meter_slider(volume))),
            ],
        },
        Level {
            tag: "参考 今",
            title: "本物の md3e（標準の動きの上に載せてある）",
            note: "Lv4 の md3e 風と比べるためのもの。中身は switch_with と slider_with に MD3E の見た目（シェーダー、ばね、リップル）を渡したもの。同じシグナルにつないである。",
            code: "md3e::styled_switch(on, SwitchStyle::new().on_icon(icons::CHECK))\n\nmd3e::styled_slider(volume, SliderStyle::new().range(0.0..=100.0))",
            samples: vec![
                sample(
                    "md3e",
                    md3e::styled_switch(on, SwitchStyle::new().on_icon(icons::CHECK)),
                ),
                sample(
                    "md3e",
                    with_value(
                        volume,
                        md3e::styled_slider(volume, SliderStyle::new().range(0.0..=100.0)),
                    ),
                ),
            ],
        },
    ];

    div()
        .size_full()
        .overflow_y_scroll()
        .bg(rgb(0xfafaf9))
        .text_color(rgb(0x1c1917))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(12.0)
                .p(20.0)
                .child(div().text_size(20.0).bold().child("見た目をどこまで変えられるか"))
                .child(
                    div()
                        .text_size(13.0)
                        .text_color(rgb(0x57534e))
                        .child("どの行も同じ on と volume につないである。動きは全部同じで、見た目だけが違う。"),
                )
                .children(levels.into_iter().map(level)),
        )
}

fn main() {
    if let Some(out) = std::env::args().nth(1).filter(|a| a.ends_with(".png")) {
        let scale = 1.8;
        let (w, h) = (1240.0, 1780.0);
        anti_webui::app::headless::render_png(
            &out,
            (w * scale) as u32,
            (h * scale) as u32,
            scale,
            page,
        )
        .expect("write the PNG");
        return;
    }
    App::new()
        .title("Custom levels")
        .size(1240, 880)
        .run(page)
        .unwrap();
}
