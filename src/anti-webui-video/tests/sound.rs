//! What comes out of the sound device, by way of a device that writes it to a file. This is alone in
//! its process since ALSA reads its configuration once.

use anti_webui_core::video::{VideoBackend, VideoEvent, VideoRequest, VideoSettings, VideoStream};
use anti_webui_video::FfmpegBackend;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn data(name: &str) -> String {
    format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn play(muted: bool, volume: f32) -> (Vec<i16>, f64) {
    let mut stream = FfmpegBackend.open(VideoRequest {
        source: data("colors.webm"),
        settings: VideoSettings::default(),
        render_node: None,
        waker: Arc::new(|| {}),
        playing: true,
        looped: false,
        muted,
        volume,
    });
    let start = Instant::now();
    run_to_end(stream.as_mut(), start);
    let took = start.elapsed().as_secs_f64();
    drop(stream);
    // What closes it runs after the stream is gone.
    std::thread::sleep(Duration::from_millis(300));
    let bytes = std::fs::read(output()).unwrap_or_default();
    let _ = std::fs::remove_file(output());
    let samples = bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    (samples, took)
}

fn run_to_end(stream: &mut dyn VideoStream, start: Instant) {
    loop {
        let events = stream.poll();
        if events.contains(&VideoEvent::Ended) {
            return;
        }
        assert!(
            !events.iter().any(|e| matches!(e, VideoEvent::Failed(_))),
            "{events:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(10), "it didn't end");
        stream.frame(Instant::now(), None);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn output() -> PathBuf {
    std::env::temp_dir().join(format!("anti-webui-sound-{}.raw", std::process::id()))
}

fn peak(samples: &[i16]) -> i32 {
    samples.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0)
}

#[test]
fn what_comes_out() {
    if FfmpegBackend
        .availability(&VideoSettings::default())
        .is_err()
    {
        eprintln!("skipped: no FFmpeg");
        return;
    }
    if !Path::new("/usr/share/alsa/alsa.conf").exists() {
        eprintln!("skipped: no ALSA");
        return;
    }
    let config = std::env::temp_dir().join(format!("anti-webui-sound-{}.conf", std::process::id()));
    std::fs::write(
        &config,
        format!(
            "pcm.nothing {{ type null }}\n\
             pcm.filed {{ type file slave.pcm \"nothing\" file \"{}\" format \"raw\" }}\n",
            output().display()
        ),
    )
    .unwrap();
    // SAFETY: nothing else in this process is running yet.
    unsafe {
        std::env::set_var("ALSA_CONFIG_PATH", &config);
        std::env::set_var("ANTIWEBUI_AUDIO_DEVICE", "filed");
    }

    let (samples, took) = play(false, 1.0);
    // A device that takes everything at once doesn't make it go any faster.
    assert!((1.8..2.8).contains(&took), "took {took}");
    let seconds = samples.len() as f64 / 2.0 / 48000.0;
    assert!((1.8..2.2).contains(&seconds), "{seconds} s of sound");
    let left: Vec<i16> = samples.iter().step_by(2).copied().collect();
    let right: Vec<i16> = samples.iter().skip(1).step_by(2).copied().collect();
    // Going through zero tells the pitch: 440 Hz goes up through it 440 times a second.
    let middle = &left[left.len() / 4..left.len() * 3 / 4];
    let rising = middle.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
    let hz = rising as f64 / (middle.len() as f64 / 48000.0);
    assert!((hz - 440.0).abs() < 5.0, "{hz} Hz");
    // One channel in, and the same on both sides.
    assert!(left.iter().zip(&right).all(|(l, r)| l.abs_diff(*r) <= 1));
    let loud = peak(&samples);
    assert!(loud > 1000, "{loud}");

    let (quiet, _) = play(false, 0.25);
    let quiet = peak(&quiet);
    assert!(
        (quiet as f64 / loud as f64 - 0.25).abs() < 0.05,
        "{quiet} of {loud}"
    );

    let (silent, took) = play(true, 1.0);
    assert_eq!(peak(&silent), 0);
    // It keeps going all the same, and the clock with it.
    assert!(silent.len() as f64 / 2.0 / 48000.0 > 1.8);
    assert!((1.8..2.8).contains(&took), "took {took}");
    let _ = std::fs::remove_file(&config);
}
