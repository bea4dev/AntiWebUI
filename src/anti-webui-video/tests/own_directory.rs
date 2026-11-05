//! FFmpeg next to a program goes before the system's. A copy of the system's libraries stands in for
//! one of its own, and this is alone in its process so that nothing else has loaded them before.

use anti_webui_core::video::{VideoBackend, VideoEvent, VideoRequest, VideoSettings};
use anti_webui_video::FfmpegBackend;
use std::sync::Arc;
use std::time::{Duration, Instant};

const LIBRARIES: [&str; 5] = [
    "libavutil.so.61",
    "libswresample.so.7",
    "libswscale.so.10",
    "libavcodec.so.63",
    "libavformat.so.63",
];

#[test]
fn looks_in_its_own_directory_first() {
    let system = std::path::Path::new("/usr/lib");
    if LIBRARIES.iter().any(|l| !system.join(l).exists()) {
        eprintln!("skipped: not the FFmpeg this copies");
        return;
    }
    let dir = std::env::temp_dir().join(format!("anti-webui-video-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for library in LIBRARIES {
        std::fs::copy(system.join(library), dir.join(library)).unwrap();
    }
    let mut settings = VideoSettings::default();
    settings.library_dir = Some(dir.clone());
    assert!(FfmpegBackend.availability(&settings).is_ok());
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    for library in LIBRARIES {
        assert!(
            maps.contains(dir.join(library).to_str().unwrap()),
            "{library} isn't the one in the directory"
        );
    }
    // And it plays.
    let mut stream = FfmpegBackend.open(VideoRequest {
        source: format!("{}/tests/data/silent.webm", env!("CARGO_MANIFEST_DIR")),
        settings,
        render_node: None,
        waker: Arc::new(|| {}),
        playing: false,
        looped: false,
        muted: true,
        volume: 1.0,
    });
    let end = Instant::now() + Duration::from_secs(10);
    while stream.frame(Instant::now(), None).is_none() {
        assert!(Instant::now() < end, "no frame came: {:?}", stream.poll());
        assert!(
            !stream
                .poll()
                .iter()
                .any(|e| matches!(e, VideoEvent::Failed(_)))
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(stream);
    let _ = std::fs::remove_dir_all(&dir);
}
