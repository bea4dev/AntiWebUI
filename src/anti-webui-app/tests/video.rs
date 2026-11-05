//! Frames of videos on real pixels: in memory, from a real FFmpeg, and read where a decoder on the
//! GPU left them. See `docs/design/24-video.md`.

use anti_webui_app::headless::{render_driven_rgba, render_rgba};
use anti_webui_core::prelude::*;
use anti_webui_core::video::{
    MemoryPlane, PlaneFormat, VideoBackend, VideoColor, VideoEvent, VideoFrame, VideoPixels,
    VideoRequest, VideoStream, VideoTiming, VideoTransfer, YuvMatrix, set_video_backend,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

const W: u32 = 64;

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

#[track_caller]
fn close(a: [u8; 4], b: [u8; 4], tolerance: u8) {
    assert!(
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tolerance),
        "{a:?} is not {b:?}"
    );
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// Stands in for what decodes: always the same frame, of pixels it owns.
struct Still(Arc<VideoFrame>);

struct StillStream(Arc<VideoFrame>, bool);

impl VideoStream for StillStream {
    fn poll(&mut self) -> Vec<VideoEvent> {
        if std::mem::replace(&mut self.1, true) {
            return Vec::new();
        }
        let (w, h) = self.0.size;
        let size = if self.0.rotation.is_multiple_of(180) {
            (w, h)
        } else {
            (h, w)
        };
        vec![VideoEvent::Opened {
            size,
            duration: None,
        }]
    }
    fn frame(&mut self, _: Instant, _: Option<VideoTiming>) -> Option<Arc<VideoFrame>> {
        Some(self.0.clone())
    }
    fn next_change(&self, _: Instant, _: Option<VideoTiming>) -> Option<Instant> {
        None
    }
    fn current_time(&self, _: Instant) -> f64 {
        0.0
    }
    fn set_playing(&mut self, _: bool) {}
    fn seek(&mut self, _: f64) {}
    fn set_looped(&mut self, _: bool) {}
    fn set_muted(&mut self, _: bool) {}
    fn set_volume(&mut self, _: f32) {}
    fn use_memory(&mut self) {}
}

impl VideoBackend for Still {
    fn open(&self, _: VideoRequest) -> Box<dyn VideoStream> {
        Box::new(StillStream(self.0.clone(), false))
    }
    fn availability(&self, _: &VideoSettings) -> Result<String, VideoError> {
        Ok("still".into())
    }
    fn can_play(&self, _: &str, _: &VideoSettings) -> bool {
        true
    }
}

/// A plane and its pixels, which go along with the frame.
fn plane(
    format: PlaneFormat,
    size: (u32, u32),
    bytes: Vec<u8>,
    keep: &mut Vec<Vec<u8>>,
) -> MemoryPlane {
    let stride = size.0 * format.texel_bytes();
    assert_eq!(bytes.len() as u32, stride * size.1);
    // SAFETY: the pixels stay in `keep`, and a `Vec` that moves leaves its buffer where it is.
    let p = unsafe { MemoryPlane::new(format, size, stride, bytes.as_ptr()) };
    keep.push(bytes);
    p
}

fn frame(
    planes: impl FnOnce(&mut Vec<Vec<u8>>) -> Vec<MemoryPlane>,
    size: (u32, u32),
    color: VideoColor,
    rotation: u16,
) -> Arc<VideoFrame> {
    let mut keep = Vec::new();
    let planes = planes(&mut keep);
    let alive: &'static Arc<()> = Box::leak(Box::new(Arc::new(())));
    Arc::new(VideoFrame {
        id: 1,
        stream: 1,
        alive: Arc::downgrade(alive),
        size,
        pixels: VideoPixels::Memory(planes),
        color,
        rotation,
        keep: Box::new(keep),
    })
}

/// BT.709 limited, all of it red.
const Y: u8 = 63;
const CB: u8 = 102;
const CR: u8 = 240;

fn nv12_red() -> Arc<VideoFrame> {
    frame(
        |keep| {
            vec![
                plane(PlaneFormat::R8, (4, 4), vec![Y; 16], keep),
                plane(PlaneFormat::Rg8, (2, 2), [CB, CR].repeat(4), keep),
            ]
        },
        (4, 4),
        VideoColor::default(),
        0,
    )
}

/// Red on the left half and blue on the right.
fn two_colors(rotation: u16) -> Arc<VideoFrame> {
    let (red, blue) = ((63u8, 102u8, 240u8), (32u8, 240u8, 118u8));
    frame(
        |keep| {
            let y: Vec<u8> = (0..16)
                .flat_map(|_| (0..16).map(|x| if x < 8 { red.0 } else { blue.0 }))
                .collect();
            let c: Vec<u8> = (0..8)
                .flat_map(|_| {
                    (0..8).flat_map(|x| {
                        if x < 4 {
                            [red.1, red.2]
                        } else {
                            [blue.1, blue.2]
                        }
                    })
                })
                .collect();
            vec![
                plane(PlaneFormat::R8, (16, 16), y, keep),
                plane(PlaneFormat::Rg8, (8, 8), c, keep),
            ]
        },
        (16, 16),
        VideoColor::default(),
        rotation,
    )
}

fn draw(frame: Arc<VideoFrame>, view: impl FnOnce() -> Video) -> Vec<u8> {
    set_video_backend(Rc::new(Still(frame)));
    render_rgba(W, W, 1.0, || {
        div().bg(Color::WHITE).w(64.0).h(64.0).child(view())
    })
    .unwrap()
}

fn full() -> Video {
    video("still").w(64.0).h(64.0)
}

#[test]
fn nv12_is_red() {
    let pixels = draw(nv12_red(), full);
    for (x, y) in [(0, 0), (32, 32), (63, 63)] {
        close(pixel(&pixels, x, y), RED, 3);
    }
}

#[test]
fn i420_is_red() {
    let f = frame(
        |keep| {
            vec![
                plane(PlaneFormat::R8, (4, 4), vec![Y; 16], keep),
                plane(PlaneFormat::R8, (2, 2), vec![CB; 4], keep),
                plane(PlaneFormat::R8, (2, 2), vec![CR; 4], keep),
            ]
        },
        (4, 4),
        VideoColor::default(),
        0,
    );
    close(pixel(&draw(f, full), 32, 32), RED, 3);
}

#[test]
fn ten_bits_are_red() {
    let wide = |v: u8| (((v as u16) << 8) | v as u16).to_le_bytes();
    let f = frame(
        |keep| {
            vec![
                plane(PlaneFormat::R16, (4, 4), wide(Y).repeat(16), keep),
                plane(
                    PlaneFormat::Rg16,
                    (2, 2),
                    [wide(CB), wide(CR)].concat().repeat(4),
                    keep,
                ),
            ]
        },
        (4, 4),
        VideoColor::default(),
        0,
    );
    close(pixel(&draw(f, full), 32, 32), RED, 3);
}

#[test]
fn what_the_frame_says_about_color_counts() {
    // The same numbers are another red under another matrix.
    let with = |color| {
        let f = frame(
            |keep| {
                vec![
                    plane(PlaneFormat::R8, (4, 4), vec![81; 16], keep),
                    plane(PlaneFormat::Rg8, (2, 2), [90, 240].repeat(4), keep),
                ]
            },
            (4, 4),
            color,
            0,
        );
        pixel(&draw(f, full), 32, 32)
    };
    let bt601 = VideoColor {
        matrix: YuvMatrix::Bt601,
        ..Default::default()
    };
    close(with(bt601), RED, 3);
    let bt709 = with(VideoColor::default());
    // By the numbers of BT.709 the same red has about 25 of green in it.
    assert!((15..40).contains(&bt709[1]), "{bt709:?}");
    // Black is another number in full range.
    let black = |full_range| {
        let f = frame(
            |keep| {
                vec![
                    plane(PlaneFormat::R8, (4, 4), vec![16; 16], keep),
                    plane(PlaneFormat::Rg8, (2, 2), [128, 128].repeat(4), keep),
                ]
            },
            (4, 4),
            VideoColor {
                full_range,
                ..Default::default()
            },
            0,
        );
        pixel(&draw(f, full), 32, 32)
    };
    close(black(false), [0, 0, 0, 255], 2);
    close(black(true), [16, 16, 16, 255], 2);
}

#[test]
fn hdr_comes_down_to_what_the_screen_can_show() {
    let grey = |number: f32, transfer| {
        let (y, c) = ((16.0 + number * 219.0).round() as u8, 128);
        let f = frame(
            |keep| {
                vec![
                    plane(PlaneFormat::R8, (4, 4), vec![y; 16], keep),
                    plane(PlaneFormat::Rg8, (2, 2), [c, c].repeat(4), keep),
                ]
            },
            (4, 4),
            VideoColor {
                matrix: YuvMatrix::Bt2020,
                full_range: false,
                transfer,
            },
            0,
        );
        pixel(&draw(f, full), 32, 32)[0]
    };
    // 203 nits in PQ is about 0.58, and that is what white is called in SDR. Being below where
    // highlights are compressed... it is on the way there.
    let white = grey(0.58, VideoTransfer::Pq);
    assert!((215..=250).contains(&white), "{white}");
    // Brighter than that gets brighter, without blowing out.
    let bright = grey(0.75, VideoTransfer::Pq);
    assert!(bright > white, "{bright}");
    // 26 nits is dark in both.
    let dark = grey(0.3, VideoTransfer::Pq);
    assert!(dark < 110, "{dark}");
    // HLG has its white at about 0.75.
    let hlg = grey(0.75, VideoTransfer::Hlg);
    assert!((200..=255).contains(&hlg), "{hlg}");
    // In SDR the same numbers aren't touched.
    assert!(grey(0.5, VideoTransfer::Sdr).abs_diff(127) <= 2);
}

#[test]
fn turns_the_way_the_file_says() {
    let check = |rotation, at_a: [u8; 4], at_b: [u8; 4], a: (u32, u32), b: (u32, u32)| {
        let pixels = draw(two_colors(rotation), full);
        close(pixel(&pixels, a.0, a.1), at_a, 3);
        close(pixel(&pixels, b.0, b.1), at_b, 3);
    };
    let (left, right, top, bottom) = ((8, 32), (56, 32), (32, 8), (32, 56));
    check(0, RED, BLUE, left, right);
    // A quarter clockwise puts the left on top.
    check(90, RED, BLUE, top, bottom);
    check(180, BLUE, RED, left, right);
    check(270, BLUE, RED, top, bottom);
}

#[test]
fn is_cut_and_faded_like_anything_else() {
    let pixels = draw(nv12_red(), || full().rounded(20.0));
    close(pixel(&pixels, 0, 0), WHITE, 1);
    close(pixel(&pixels, 32, 32), RED, 3);
    let pixels = draw(nv12_red(), || full().opacity(0.5));
    close(pixel(&pixels, 32, 32), [255, 127, 127, 255], 3);
    // Fitted into its box, with what is behind it around it.
    let pixels = draw(two_colors(0), || {
        video("still")
            .w(64.0)
            .h(32.0)
            .object_fit(ObjectFit::Contain)
    });
    close(pixel(&pixels, 2, 16), WHITE, 1);
    close(pixel(&pixels, 20, 16), RED, 3);
    close(pixel(&pixels, 44, 16), BLUE, 3);
}

/// Only worth going on if the system has an FFmpeg this knows.
fn real() -> bool {
    anti_webui_video::install();
    match video_availability() {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipped: {e}");
            false
        }
    }
}

fn data(name: &str) -> String {
    format!(
        "{}/../anti-webui-video/tests/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn a_real_file_shows() {
    if !real() {
        return;
    }
    let source = data("silent.webm");
    let pixels = render_rgba(W, W, 1.0, || video(source).w(64.0).h(64.0)).unwrap();
    close(pixel(&pixels, 32, 32), RED, 8);
}

#[test]
fn a_real_file_turns() {
    if !real() {
        return;
    }
    // Red is on the left in the file, which says to turn it a quarter clockwise. `ffmpeg` also has
    // red on top then.
    let source = data("rotated.mp4");
    let size = Rc::new(RefCell::new(None));
    let s = size.clone();
    let pixels = render_rgba(W, W, 1.0, move || {
        video(source)
            .w(64.0)
            .h(64.0)
            .on_load(move |w, h, _| *s.borrow_mut() = Some((w, h)))
    })
    .unwrap();
    assert_eq!(*size.borrow(), Some((120.0, 160.0)));
    close(pixel(&pixels, 32, 12), RED, 12);
    close(pixel(&pixels, 32, 52), BLUE, 12);
}

#[test]
fn what_cant_be_opened_doesnt_get_in_the_way() {
    if !real() {
        return;
    }
    let heard = Rc::new(RefCell::new(None));
    let h = heard.clone();
    let pixels = render_driven_rgba(
        W,
        W,
        1.0,
        move || {
            div().bg(Color::WHITE).w(64.0).h(64.0).child(
                video("/nonexistent.webm")
                    .w(64.0)
                    .h(64.0)
                    .on_error(move |e| *h.borrow_mut() = Some(e.clone())),
            )
        },
        |_| {},
    )
    .unwrap();
    close(pixel(&pixels, 32, 32), WHITE, 1);
    assert!(matches!(*heard.borrow(), Some(VideoError::Source(_))));
}
