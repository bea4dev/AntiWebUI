//! Takes pictures of a window of the compositor this runs under, with each source it has. Not run
//! by default: `cargo test -p anti-webui-wayland-shell --test capture_live -- --ignored`.
//! `ANTIWEBUI_CAPTURE_DUMP=dir` writes them there as PPM.

#![cfg(target_os = "linux")]

use anti_webui_wayland_shell::{CaptureOptions, CaptureSource, capture, toplevels};
use std::time::{Duration, Instant};

#[test]
#[ignore = "needs a compositor with windows"]
fn windows_can_be_pictured_with_each_source() {
    let all = toplevels().all();
    let window = toplevels()
        .active()
        .or_else(|| all.first().cloned())
        .expect("a window to picture");
    for source in [CaptureSource::Ext, CaptureSource::Hyprland] {
        let options = CaptureOptions::default().max_size(320, 320).prefer(source);
        let shot = capture(&window, options);
        if shot.source() != Some(source) {
            eprintln!("{source:?}: not here");
            continue;
        }
        let start = Instant::now();
        while shot.picture().is_none()
            && !shot.stopped()
            && start.elapsed() < Duration::from_secs(3)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let picture = shot.picture().expect("a picture");
        let (w, h) = (picture.width(), picture.height());
        eprintln!(
            "{source:?}: {:?} as {w}x{h} in {:?}",
            shot.size(),
            start.elapsed()
        );
        assert!(w <= 320 && h <= 320 && w > 0 && h > 0);
        if let Some(dir) = std::env::var_os("ANTIWEBUI_CAPTURE_DUMP") {
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            picture.with_levels(|_, _, pixels| {
                for px in pixels.chunks_exact(4) {
                    ppm.extend_from_slice(&px[..3]);
                }
            });
            let path = std::path::Path::new(&dir).join(format!("{source:?}.ppm"));
            std::fs::write(path, ppm).unwrap();
        }
    }
}
