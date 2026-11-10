//! A video with what it takes to control it.
//!
//! ```sh
//! cargo run -p anti-webui --example video --features video -- clip.mp4
//! ```
//!
//! `--software` after the name decodes on the CPU. The button shows the video alone on all of the
//! screen, and Escape or a double click brings the rest back.

use anti_webui::prelude::*;

fn clock(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "--:--".into();
    }
    let s = seconds as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn main() {
    let source = std::env::args().nth(1).unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../anti-webui-video/tests/data/colors.webm"
        )
        .into()
    });
    let software = std::env::args().any(|a| a == "--software");
    App::new()
        .title("Video")
        .size(960, 640)
        .settings(move |s| s.video.software_only = software)
        .run(move || {
            let player = video_player();
            let error = signal(None::<String>);
            let position = signal(0.0f32);
            let volume = signal(100.0f32);
            let muted = signal(false);
            let cover = signal(false);
            let screen = node_ref();
            // The slider follows the time, and the time follows the slider when that is moved.
            effect(move || position.set(player.current_time() as f32));
            effect(move || {
                let at = position.get() as f64;
                if (at - untrack(|| player.current_time())).abs() > 0.5 {
                    player.seek(at);
                }
            });
            effect(move || player.set_volume(volume.get() / 100.0));
            effect(move || player.set_muted(muted.get()));
            let status = match video_availability() {
                Ok(what) => what,
                Err(e) => e.to_string(),
            };
            div()
                .flex()
                .flex_col()
                .gap(12.0)
                .p(16.0)
                .w_full()
                .h_full()
                .child(
                    div()
                        .flex_grow(1.0)
                        .min_h(0.0)
                        .bg(Color::BLACK)
                        .rounded(12.0)
                        .child(
                            video(source.clone())
                                .player(player)
                                .node_ref(screen)
                                .on_click(move |e| {
                                    if e.click_count == 2 {
                                        if screen.is_fullscreen() {
                                            exit_fullscreen();
                                        } else {
                                            screen.request_fullscreen();
                                        }
                                    }
                                })
                                .autoplay(true)
                                .looped(true)
                                .w_full()
                                .h_full()
                                .rounded(12.0)
                                .dyn_style(move |s| {
                                    s.object_fit(if cover.get() {
                                        ObjectFit::Cover
                                    } else {
                                        ObjectFit::Contain
                                    })
                                })
                                .on_error(move |e| error.set(Some(e.to_string()))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(12.0)
                        .child(
                            button(move || {
                                if player.paused() {
                                    "Play"
                                } else {
                                    "Pause"
                                }
                            })
                            .on_click(move |_| {
                                if player.paused() {
                                    player.play();
                                } else {
                                    player.pause();
                                }
                            }),
                        )
                        .child(text(move || clock(player.current_time())))
                        .child(dyn_view(move || {
                            let end = player.duration();
                            slider(
                                position,
                                0.0,
                                if end.is_finite() { end as f32 } else { 1.0 },
                            )
                            .flex_grow(1.0)
                        }))
                        .child(text(move || clock(player.duration()))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(16.0)
                        .child(checkbox(muted, "Mute"))
                        .child(slider(volume, 0.0, 100.0).w(120.0))
                        .child(checkbox(cover, "cover"))
                        .child(button("Fullscreen").on_click(move |_| screen.request_fullscreen()))
                        .child(text(status))
                        .child(text(move || error.get().unwrap_or_default())),
                )
        })
        .unwrap();
}
