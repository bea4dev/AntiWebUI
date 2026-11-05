//! Nothing stays behind when a video goes away. This is alone in its process since it counts what
//! the whole process has.

use anti_webui_core::video::{VideoBackend, VideoRequest, VideoSettings, VideoStream};
use anti_webui_video::FfmpegBackend;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

fn descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}

fn settle(what: impl Fn() -> usize, at_most: usize) -> usize {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let now = what();
        if now <= at_most || Instant::now() > end {
            return now;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open(playing: bool) -> Box<dyn VideoStream> {
    FfmpegBackend.open(VideoRequest {
        source: format!("{}/tests/data/colors.webm", env!("CARGO_MANIFEST_DIR")),
        settings: VideoSettings::default(),
        render_node: None,
        waker: Arc::new(|| {}),
        playing,
        looped: false,
        muted: true,
        volume: 1.0,
    })
}

#[test]
fn nothing_stays_behind() {
    if FfmpegBackend
        .availability(&VideoSettings::default())
        .is_err()
    {
        eprintln!("skipped: no FFmpeg");
        return;
    }
    let (threads_before, descriptors_before) = (threads(), descriptors());
    let mut frames = Vec::new();
    for round in 0..30 {
        let mut stream = open(round % 2 == 0);
        let end = Instant::now() + Duration::from_secs(10);
        while let (None, true) = (stream.frame(Instant::now(), None), Instant::now() < end) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // Some go while busy, and some hold on to a frame for longer than the stream lives.
        if round % 3 == 0 {
            frames.extend(stream.frame(Instant::now(), None));
            stream.seek(1.0);
        }
    }
    drop(frames);
    let threads_after = settle(threads, threads_before);
    assert!(
        threads_after <= threads_before,
        "{threads_before} -> {threads_after} threads"
    );
    let after = settle(descriptors, descriptors_before);
    // ALSA and such may keep something open once they have been used, not one for each.
    assert!(
        after <= descriptors_before + 2,
        "{descriptors_before} -> {after} descriptors"
    );
}
