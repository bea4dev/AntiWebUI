//! Real files through a real FFmpeg, when the system has one this knows. See
//! `docs/design/24-video.md`.

use anti_webui_core::video::{
    VideoBackend, VideoError, VideoEvent, VideoFrame, VideoLibraries, VideoPixels, VideoRequest,
    VideoSettings, VideoStream, VideoTiming,
};
use anti_webui_video::FfmpegBackend;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn data(name: &str) -> String {
    format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn has_ffmpeg() -> bool {
    let found = FfmpegBackend.availability(&VideoSettings::default());
    if let Err(e) = &found {
        eprintln!("skipped: {e}");
    }
    found.is_ok()
}

fn request(source: &str) -> VideoRequest {
    VideoRequest {
        source: source.into(),
        settings: VideoSettings::default(),
        render_node: None,
        waker: Arc::new(|| {}),
        playing: false,
        looped: false,
        muted: true,
        volume: 1.0,
    }
}

fn open(source: &str, playing: bool) -> Box<dyn VideoStream> {
    FfmpegBackend.open(VideoRequest {
        playing,
        ..request(source)
    })
}

/// Waits until a frame is there to show.
fn first_frame(stream: &mut dyn VideoStream) -> Arc<VideoFrame> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(frame) = stream.frame(Instant::now(), None) {
            return frame;
        }
        assert!(Instant::now() < end, "no frame came");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_event(
    stream: &mut dyn VideoStream,
    wanted: impl Fn(&VideoEvent) -> bool,
) -> VideoEvent {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        for event in stream.poll() {
            if wanted(&event) {
                return event;
            }
        }
        assert!(Instant::now() < end, "that didn't happen");
        stream.frame(Instant::now(), None);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The luma of the middle of a frame, and of its red-difference chroma, in memory.
fn middle(frame: &VideoFrame) -> (u8, u8) {
    let VideoPixels::Memory(planes) = &frame.pixels else {
        panic!("in memory")
    };
    let (w, h) = planes[0].size;
    let y = planes[0].bytes()[(h / 2 * planes[0].stride + w / 2) as usize];
    let (cw, ch) = planes[1].size;
    let chroma = planes.last().unwrap();
    let texel = chroma.format.texel_bytes();
    let v = chroma.bytes()[(ch / 2 * chroma.stride + cw / 2 * texel + texel - 1) as usize];
    (y, v)
}

fn is_red(frame: &VideoFrame) -> bool {
    middle(frame).1 > 200
}

fn is_blue(frame: &VideoFrame) -> bool {
    middle(frame).1 < 150
}

#[test]
fn says_how_big_and_how_long() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = open(&data("colors.webm"), false);
    let opened = wait_for_event(stream.as_mut(), |e| matches!(e, VideoEvent::Opened { .. }));
    let VideoEvent::Opened { size, duration } = opened else {
        unreachable!()
    };
    assert_eq!(size, (320, 240));
    assert!((duration.unwrap() - 2.0).abs() < 0.1, "{duration:?}");
}

#[test]
fn stays_on_the_first_frame_until_told_to_play() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = open(&data("colors.webm"), false);
    let frame = first_frame(stream.as_mut());
    assert!(is_red(&frame), "{:?}", middle(&frame));
    assert_eq!(frame.size, (320, 240));
    std::thread::sleep(Duration::from_millis(100));
    let same = stream.frame(Instant::now(), None).unwrap();
    assert_eq!(same.id, frame.id);
    assert_eq!(stream.current_time(Instant::now()), 0.0);
    // Nothing is due, so nothing keeps it awake.
    assert_eq!(stream.next_change(Instant::now(), None), None);
}

#[test]
fn colors_of_the_frames_are_where_they_should_be() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = open(&data("silent.webm"), false);
    first_frame(stream.as_mut());
    stream.seek(1.5);
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let frame = stream.frame(Instant::now(), None).unwrap();
        if is_blue(&frame) {
            break;
        }
        assert!(Instant::now() < end, "{:?}", middle(&frame));
        std::thread::sleep(Duration::from_millis(1));
    }
    let time = stream.current_time(Instant::now());
    assert!((time - 1.5).abs() < 0.05, "{time}");
}

#[test]
fn plays_to_the_end_in_about_the_time_it_takes() {
    if !has_ffmpeg() {
        return;
    }
    let start = Instant::now();
    let mut stream = open(&data("silent.webm"), true);
    let mut ids = Vec::new();
    let mut blue_at = None;
    let end = start + Duration::from_secs(10);
    loop {
        if stream.poll().contains(&VideoEvent::Ended) {
            break;
        }
        if let Some(frame) = stream.frame(
            Instant::now(),
            Some(VideoTiming {
                refresh: Duration::from_micros(16667),
                vsync: None,
            }),
        ) {
            if ids.last() != Some(&frame.id) {
                ids.push(frame.id);
            }
            if blue_at.is_none() && is_blue(&frame) {
                blue_at = Some(start.elapsed());
            }
        }
        assert!(Instant::now() < end, "it didn't end");
        std::thread::sleep(Duration::from_millis(16));
    }
    let took = start.elapsed().as_secs_f64();
    assert!((1.9..2.6).contains(&took), "took {took}");
    assert!(ids.len() >= 55, "only {} frames", ids.len());
    let blue = blue_at.expect("blue came").as_secs_f64();
    assert!((0.9..1.2).contains(&blue), "blue at {blue}");
    assert!((stream.current_time(Instant::now()) - 2.0).abs() < 0.1);
    // Nothing is left going on.
    assert_eq!(stream.next_change(Instant::now(), None), None);
}

#[test]
fn pausing_stops_the_clock_and_playing_at_the_end_starts_over() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = open(&data("silent.webm"), true);
    first_frame(stream.as_mut());
    std::thread::sleep(Duration::from_millis(200));
    stream.set_playing(false);
    let paused_at = stream.current_time(Instant::now());
    assert!(paused_at > 0.1, "{paused_at}");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(stream.current_time(Instant::now()), paused_at);
    stream.seek(1.8);
    stream.set_playing(true);
    wait_for_event(stream.as_mut(), |e| *e == VideoEvent::Ended);
    stream.set_playing(true);
    // It said it was over, and now it isn't.
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let frame = stream.frame(Instant::now(), None);
        if frame.is_some_and(|f| is_red(&f)) {
            break;
        }
        assert!(Instant::now() < end, "it didn't start over");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn looping_starts_over_without_saying_it_ended() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = FfmpegBackend.open(VideoRequest {
        playing: true,
        looped: true,
        ..request(&data("silent.webm"))
    });
    first_frame(stream.as_mut());
    stream.seek(1.8);
    let end = Instant::now() + Duration::from_secs(5);
    let mut was_blue = false;
    loop {
        assert!(!stream.poll().contains(&VideoEvent::Ended));
        if let Some(frame) = stream.frame(Instant::now(), None) {
            was_blue |= is_blue(&frame);
            if was_blue && is_red(&frame) {
                break;
            }
        }
        assert!(Instant::now() < end, "it didn't come around");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Nobody looks at it, as when it isn't on the screen, and sound has to keep going all the same.
#[test]
fn nobody_taking_frames_doesnt_hold_it_up() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = open(&data("silent.webm"), true);
    first_frame(stream.as_mut());
    std::thread::sleep(Duration::from_millis(2300));
    assert!(stream.queued() <= 4);
    assert!(stream.poll().contains(&VideoEvent::Ended));
}

#[test]
fn what_cant_be_opened_says_so() {
    if !has_ffmpeg() {
        return;
    }
    let mut stream = open("/nonexistent/video.webm", true);
    let event = wait_for_event(stream.as_mut(), |e| matches!(e, VideoEvent::Failed(_)));
    assert!(
        matches!(event, VideoEvent::Failed(VideoError::Source(_))),
        "{event:?}"
    );
    // Not a video at all.
    let mut stream = open(&format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR")), true);
    let event = wait_for_event(stream.as_mut(), |e| matches!(e, VideoEvent::Failed(_)));
    assert!(matches!(event, VideoEvent::Failed(_)), "{event:?}");
}

/// A directory without FFmpeg in it, and a system whose FFmpeg isn't allowed: what is left is an
/// error, not a crash.
#[test]
fn only_lgpl_refuses_a_gpl_build() {
    if !has_ffmpeg() {
        return;
    }
    let description = FfmpegBackend
        .availability(&VideoSettings::default())
        .unwrap();
    let mut settings = VideoSettings::default();
    settings.libraries = VideoLibraries::LgplOnly;
    let only_lgpl = FfmpegBackend.availability(&settings);
    if description.contains("(GPL") {
        let Err(VideoError::Unavailable(why)) = only_lgpl else {
            panic!("{only_lgpl:?}");
        };
        assert!(why.contains("LGPL"), "{why}");
        let mut stream = FfmpegBackend.open(VideoRequest {
            settings,
            ..request(&data("silent.webm"))
        });
        let event = wait_for_event(stream.as_mut(), |e| matches!(e, VideoEvent::Failed(_)));
        assert!(
            matches!(event, VideoEvent::Failed(VideoError::Unavailable(_))),
            "{event:?}"
        );
    } else {
        assert!(only_lgpl.is_ok(), "{only_lgpl:?}");
    }
}

#[test]
fn says_what_it_can_play() {
    if !has_ffmpeg() {
        return;
    }
    let settings = VideoSettings::default();
    assert!(FfmpegBackend.can_play("video/webm; codecs=\"vp9, opus\"", &settings));
    assert!(!FfmpegBackend.can_play("video/nonsense", &settings));
}

/// On whatever the sound device of the machine is, without being heard: the clock has to come out
/// right with sound as it does without.
#[test]
fn takes_as_long_with_sound_as_without() {
    if !has_ffmpeg() {
        return;
    }
    let start = Instant::now();
    let mut stream = open(&data("colors.webm"), true);
    wait_for_event(stream.as_mut(), |e| *e == VideoEvent::Ended);
    let took = start.elapsed().as_secs_f64();
    assert!((1.9..2.8).contains(&took), "took {took}");
}
