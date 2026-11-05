//! Plays a video in the whole window for a while and closes it, for `ANTIWEBUI_FRAME_STATS`. See
//! `docs/design/24-video.md`.
//!
//! ```sh
//! ANTIWEBUI_FRAME_STATS=frames.csv cargo run --release -p anti-webui --example bench_video \
//!     --features video -- clip.mp4 10 [--software]
//! ```

use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let source = args[1].clone();
    let seconds: f64 = args.get(2).map_or(10.0, |s| s.parse().expect("seconds"));
    let software = args.iter().any(|a| a == "--software");
    App::new()
        .title("Video")
        .size(960, 640)
        .settings(move |s| s.video.software_only = software)
        .run(move || {
            set_timeout(Duration::from_secs_f64(seconds), || {
                with_doc(|d| d.request(PlatformRequest::Close));
            });
            video(source)
                .autoplay(true)
                .looped(true)
                .muted(true)
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Contain)
                .bg(Color::BLACK)
        })
        .unwrap();
}
