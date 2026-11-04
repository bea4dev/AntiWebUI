//! Blurs and backdrops to look at: a list scrolling under a glass header, panels to hover, a dialog
//! that scales in, and the shadow of a shape (see `docs/design/16-filters.md`).
//!
//! `cargo run --release -p anti-webui --example filters`

use anti_webui::prelude::*;

#[derive(ShaderParams, Clone, Copy)]
struct Tint {
    color: Color,
}

/// Keeps the shape of the input in one color.
static TINT: Shader<Tint> = Shader::new(
    "tint",
    "fn paint(f: Frag) -> vec4<f32> {
    return params.color * read0(f.px).a;
}
",
);

/// The second input over the first.
static OVER: Shader<()> = Shader::new(
    "over",
    "fn paint(f: Frag) -> vec4<f32> {
    let top = read1(f.px);
    return top + read0(f.px) * (1.0 - top.a);
}
",
);

#[derive(ShaderParams, Clone, Copy)]
struct Ripple {
    time: f32,
}

/// Bends what is behind it, and blurs it a little.
static RIPPLE: Shader<Ripple> = Shader::new(
    "ripple",
    "fn paint(f: Frag) -> vec4<f32> {
    let amount = 10.0 * aw_box.scale;
    let dx = sin(f.px.y * 0.04 / aw_box.scale + params.time * 3.0) * amount;
    let c = read0(f.px + vec2<f32>(dx, 0.0));
    return vec4<f32>(c.rgb * 0.85 + vec3<f32>(0.1), 1.0);
}
",
);

fn card(i: usize) -> Div {
    let colors = [0xe45756, 0x4c78a8, 0xf58518, 0x72b7b2, 0x54a24b, 0xeeca3b];
    div()
        .h(72.0)
        .flex_none()
        .m(8.0)
        .px(16.0)
        .flex()
        .items_center()
        .rounded(12.0)
        .bg(rgb(colors[i % colors.len()]))
        .text_color(rgb(0xffffff))
        .child(format!("Row {i}: scroll under the header and the panels"))
}

fn glass() -> Div {
    div()
        .rounded(16.0)
        .border(1.0)
        .border_color(rgba(0xffffff50))
        .bg(rgba(0xffffff28))
        .text_color(rgb(0xffffff))
        .p(16.0)
}

fn button(label: &str, f: impl Fn() + 'static) -> Div {
    div()
        .px(16.0)
        .py(8.0)
        .rounded(8.0)
        .bg(rgba(0xffffff30))
        .hover(|s| s.bg(rgba(0xffffff50)))
        .text_color(rgb(0xffffff))
        .on_click(move |_| f())
        .child(label.to_string())
}

fn main() {
    App::new()
        .title("filters")
        .size(900, 700)
        .run(|| {
            let dialog = signal(false);
            let time = animated_time();
            div()
                .size_full()
                .relative()
                .bg(rgb(0x20304a))
                .child(
                    div()
                        .size_full()
                        .overflow_y_scroll()
                        .pt(90.0)
                        .children((0..200).map(card)),
                )
                .child(
                    // The header reads the list going by.
                    glass()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(pct(100.0))
                        .h(72.0)
                        .rounded(0.0)
                        .backdrop_blur(18.0)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(12.0)
                        .child("backdrop_blur(18)")
                        .child(button("Open the dialog", move || dialog.set(true))),
                )
                .child(
                    glass()
                        .absolute()
                        .left(24.0)
                        .top(110.0)
                        .w(240.0)
                        .h(120.0)
                        .backdrop_blur(8.0)
                        .transition(Transition::ms(220).ease_out())
                        .hover(|s| s.backdrop_blur(32.0).scale(1.05))
                        .child("Hover: the radius and the scale move together"),
                )
                .child(
                    // Something that isn't a blur reads what is behind it too.
                    shader(move || {
                        Image::backdrop()
                            .blur(1.5)
                            .through(&RIPPLE, Ripple { time: time.get() })
                            .reach(10.0)
                    })
                    .absolute()
                    .left(290.0)
                    .top(110.0)
                    .w(240.0)
                    .h(120.0)
                    .rounded(16.0)
                    .text_color(rgb(0xffffff))
                    .p(16.0)
                    .child("A shader that bends what is behind"),
                )
                .child(
                    // The shadow takes the shape of what is painted, text and all.
                    div()
                        .absolute()
                        .left(560.0)
                        .top(120.0)
                        .text_color(rgb(0xffffff))
                        .text_size(40.0)
                        .child("Shadow")
                        .filter(|| {
                            let me = Image::content();
                            let shadow = me.clone().blur(4.0).through(
                                &TINT,
                                Tint {
                                    color: rgb(0x000000),
                                },
                            );
                            OVER.params(()).inputs([shadow, me]).reach(14.0)
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .left(560.0)
                        .top(190.0)
                        .text_color(rgb(0xffffff))
                        .transition(Transition::ms(180).ease_out())
                        .hover(|s| s.blur(3.0))
                        .child("Hover: this text blurs"),
                )
                .child(
                    show(
                        move || dialog.get(),
                        move || {
                            // Its own transform and opacity go with what it shows of what is behind.
                            glass()
                                .absolute()
                                .left(270.0)
                                .top(230.0)
                                .w(360.0)
                                .h(200.0)
                                .bg(rgba(0xffffff40))
                                .backdrop_blur(24.0)
                                .on_click(move |_| dialog.set(false))
                                .child("A dialog that scales in. Click to close.")
                        },
                    )
                    .enter(|s| s.opacity(0.0).scale(0.9))
                    .exit(|s| s.opacity(0.0).scale(0.9))
                    .transition(Transition::ms(220).ease_out()),
                )
        })
        .unwrap();
}

/// Seconds since it started, going on as long as something is listening.
fn animated_time() -> Signal<f32> {
    let time = signal(0.0_f32);
    fn tick(started: Option<std::time::Instant>, time: Signal<f32>) {
        request_frame(move |now| {
            let started = started.unwrap_or(now);
            time.set(now.saturating_duration_since(started).as_secs_f32());
            tick(Some(started), time);
        });
    }
    tick(None, time);
    time
}
