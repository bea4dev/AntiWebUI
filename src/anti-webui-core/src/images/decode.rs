//! Turning what a picture comes from into pixels. Nothing here touches the document: it all runs on
//! the worker threads. See `docs/design/22-images.md`.

use crate::images::{Decoded, Info};
use anti_webui_style::ImageSource;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::borrow::Cow;
use std::io::{BufRead, BufReader, Cursor, Seek};
use std::time::Duration;

/// Bigger than this doesn't fit in a texture everywhere, and nobody can see the difference.
const MAX_SIDE: u32 = 8192;
/// What all the frames of one animation may take. See "what isn't done" in the design document.
const MAX_ANIMATION_BYTES: usize = 256 << 20;

/// The size to decode a picture at, to show it at `wanted` physical pixels: the smallest of its
/// halves that is no smaller than that, or exactly `wanted` for what is drawn from a description.
pub(crate) fn planned_size(info: &Info, wanted: (u32, u32), pixelated: bool) -> (u32, u32) {
    let wanted = (wanted.0.max(1), wanted.1.max(1));
    if info.vector {
        let fit = (MAX_SIDE as f32 / wanted.0.max(wanted.1) as f32).min(1.0);
        return (
            ((wanted.0 as f32 * fit) as u32).max(1),
            ((wanted.1 as f32 * fit) as u32).max(1),
        );
    }
    let (w, h) = (info.pixels.0.max(1), info.pixels.1.max(1));
    let mut level = 0;
    while (w >> level) > MAX_SIDE || (h >> level) > MAX_SIDE {
        level += 1;
    }
    if !pixelated {
        while level < 30
            && (w >> (level + 1)).max(1) >= wanted.0
            && (h >> (level + 1)).max(1) >= wanted.1
            && (w >> (level + 1) > 0 || h >> (level + 1) > 0)
        {
            level += 1;
        }
    }
    ((w >> level).max(1), (h >> level).max(1))
}

fn bytes_of(source: &ImageSource) -> Result<Cow<'_, [u8]>, String> {
    match source {
        ImageSource::Path(path) => std::fs::read(path)
            .map(Cow::Owned)
            .map_err(|e| format!("{}: {e}", path.display())),
        ImageSource::Static(bytes) => Ok(Cow::Borrowed(bytes)),
        ImageSource::Bytes(bytes) => Ok(Cow::Borrowed(bytes)),
        ImageSource::Pixels(_) => Err("pixels have no file".into()),
    }
}

/// Whether what starts like this is a description of a picture rather than its pixels.
fn looks_like_svg(start: &[u8]) -> bool {
    let start = start.strip_prefix(b"\xef\xbb\xbf").unwrap_or(start);
    let text = String::from_utf8_lossy(start);
    let text = text.trim_start();
    // A compressed one starts like any other gzip stream, which is no picture otherwise.
    text.starts_with("<svg")
        || text.starts_with("<?xml")
        || text.starts_with("<!--")
        || text.starts_with("<!DOCTYPE")
        || start.starts_with(&[0x1f, 0x8b])
}

/// Finds out how big a picture is, reading no more of it than that takes.
pub(crate) fn probe(source: &ImageSource) -> Result<Info, String> {
    match source {
        ImageSource::Pixels(p) => Ok(Info {
            pixels: (p.width, p.height),
            natural: (p.width as f32, p.height as f32),
            vector: false,
        }),
        ImageSource::Path(path) => {
            let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut reader = BufReader::new(file);
            let start = reader
                .fill_buf()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if looks_like_svg(start) {
                let bytes = bytes_of(source)?;
                return probe_svg(&bytes);
            }
            probe_raster(reader).map_err(|e| format!("{}: {e}", path.display()))
        }
        ImageSource::Static(_) | ImageSource::Bytes(_) => {
            let bytes = bytes_of(source)?;
            if looks_like_svg(&bytes[..bytes.len().min(256)]) {
                return probe_svg(&bytes);
            }
            probe_raster(Cursor::new(&*bytes))
        }
    }
}

fn probe_raster<R: BufRead + Seek>(reader: R) -> Result<Info, String> {
    let mut decoder = ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())?;
    let (mut w, mut h) = decoder.dimensions();
    if turns_sideways(decoder.orientation().map_err(|e| e.to_string())?) {
        std::mem::swap(&mut w, &mut h);
    }
    Ok(Info {
        pixels: (w, h),
        natural: (w as f32, h as f32),
        vector: false,
    })
}

fn turns_sideways(orientation: image::metadata::Orientation) -> bool {
    use image::metadata::Orientation as O;
    matches!(
        orientation,
        O::Rotate90 | O::Rotate270 | O::Rotate90FlipH | O::Rotate270FlipH
    )
}

/// Decodes a picture at a size from [`planned_size`].
pub(crate) fn decode(source: &ImageSource, target: (u32, u32)) -> Result<Decoded, String> {
    if let ImageSource::Pixels(p) = source {
        let mut pixels = p.data.clone();
        let opaque = premultiply(&mut pixels);
        return Ok(still(pixels, (p.width, p.height), target, opaque));
    }
    let bytes = bytes_of(source)?;
    if looks_like_svg(&bytes[..bytes.len().min(256)]) {
        return decode_svg(&bytes, target);
    }
    decode_raster(&bytes, target)
}

fn decode_raster(bytes: &[u8], target: (u32, u32)) -> Result<Decoded, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    match reader.format() {
        Some(ImageFormat::Gif) => {
            let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            let loops = decoder.loop_count();
            if let Some(d) = animation(decoder.into_frames(), target, loops)? {
                return Ok(d);
            }
        }
        Some(ImageFormat::WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            if decoder.has_animation() {
                let loops = decoder.loop_count();
                if let Some(d) = animation(decoder.into_frames(), target, loops)? {
                    return Ok(d);
                }
            }
        }
        Some(ImageFormat::Png) => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            if decoder.is_apng().map_err(|e| e.to_string())? {
                let decoder = decoder.apng().map_err(|e| e.to_string())?;
                let loops = decoder.loop_count();
                if let Some(d) = animation(decoder.into_frames(), target, loops)? {
                    return Ok(d);
                }
            }
        }
        _ => {}
    }
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let profile = decoder.icc_profile().map_err(|e| e.to_string())?;
    let mut picture = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    picture.apply_orientation(orientation);
    let opaque = !picture.color().has_alpha();
    let picture = picture.into_rgba8();
    let size = picture.dimensions();
    let mut pixels = picture.into_raw();
    if let Some(profile) = profile {
        to_srgb(&mut pixels, &profile);
    }
    let opaque = premultiply(&mut pixels) || opaque;
    Ok(still(pixels, size, target, opaque))
}

/// What the frames of an animation come to, or `None` if there is only one and it is a still.
fn animation(
    frames: image::Frames<'_>,
    target: (u32, u32),
    loops: image::metadata::LoopCount,
) -> Result<Option<Decoded>, String> {
    let mut out = Vec::new();
    let mut total = 0;
    for frame in frames {
        let frame = frame.map_err(|e| e.to_string())?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = Duration::from_micros(numer as u64 * 1000 / denom.max(1) as u64);
        // What browsers do with delays that make no sense as an animation.
        let delay = if delay <= Duration::from_millis(10) {
            Duration::from_millis(100)
        } else {
            delay
        };
        let picture = frame.into_buffer();
        let size = picture.dimensions();
        let mut pixels = picture.into_raw();
        premultiply(&mut pixels);
        let (pixels, _) = shrink(pixels, size, target);
        total += pixels.len();
        out.push((pixels, delay));
        if total > MAX_ANIMATION_BYTES {
            break;
        }
    }
    if out.len() < 2 {
        return Ok(None);
    }
    let size = target;
    Ok(Some(Decoded {
        size,
        opaque: false,
        levels: Vec::new(),
        frames: out,
        loops: match loops {
            image::metadata::LoopCount::Infinite => None,
            image::metadata::LoopCount::Finite(n) => Some(n.get()),
        },
    }))
}

/// Converts to sRGB, unless it turns out to be that already or can't be made sense of.
fn to_srgb(pixels: &mut [u8], profile: &[u8]) {
    use moxcms::{ColorProfile, Layout, TransformOptions};
    let Ok(from) = ColorProfile::new_from_slice(profile) else {
        return;
    };
    let Ok(transform) = from.create_transform_8bit(
        Layout::Rgba,
        &ColorProfile::new_srgb(),
        Layout::Rgba,
        TransformOptions::default(),
    ) else {
        return;
    };
    // Most profiles say what is assumed anyway: going through them would only cost time.
    let probe: Vec<u8> = (0..64u32)
        .flat_map(|i| {
            [
                (i & 3) as u8 * 85,
                ((i >> 2) & 3) as u8 * 85,
                ((i >> 4) & 3) as u8 * 85,
                255,
            ]
        })
        .collect();
    let mut result = vec![0; probe.len()];
    if transform.transform(&probe, &mut result).is_err() {
        return;
    }
    if probe.iter().zip(&result).all(|(a, b)| a.abs_diff(*b) <= 1) {
        return;
    }
    let source = pixels.to_vec();
    let _ = transform.transform(&source, pixels);
}

/// Multiplies the colors by their alpha. Returns whether everything was opaque.
fn premultiply(pixels: &mut [u8]) -> bool {
    let mut opaque = true;
    for p in pixels.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a == 255 {
            continue;
        }
        opaque = false;
        for c in &mut p[..3] {
            // Exact division by 255, rounded.
            let v = *c as u32 * a + 128;
            *c = ((v + (v >> 8)) >> 8) as u8;
        }
    }
    opaque
}

/// One step down: half the size, with each new pixel taking two neighbours on an even side and
/// three (1, 2, 1) on an odd side so that nothing is left out, like Skia's mipmaps.
fn halved(src: &[u8], (w, h): (u32, u32)) -> (Vec<u8>, (u32, u32)) {
    let (dw, dh) = ((w / 2).max(1), (h / 2).max(1));
    let row = |y: u32| &src[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
    // Horizontally first, in sums four times as big as the bytes.
    let sums = |y: u32, out: &mut [u32]| {
        let r = row(y);
        if w == 1 {
            for (o, v) in out.iter_mut().zip(r) {
                *o = *v as u32 * 4;
            }
        } else if w % 2 == 0 {
            for (o, v) in out.chunks_exact_mut(4).zip(r.chunks_exact(8)) {
                for c in 0..4 {
                    o[c] = (v[c] as u32 + v[c + 4] as u32) * 2;
                }
            }
        } else {
            for (x, o) in out.chunks_exact_mut(4).enumerate() {
                let i = x * 8;
                for c in 0..4 {
                    o[c] = r[i + c] as u32 + r[i + c + 4] as u32 * 2 + r[i + c + 8] as u32;
                }
            }
        }
    };
    let width = (dw * 4) as usize;
    let mut out = vec![0u8; width * dh as usize];
    let mut a = vec![0u32; width];
    let mut b = vec![0u32; width];
    let mut c = vec![0u32; width];
    for (y, dst) in out.chunks_exact_mut(width).enumerate() {
        let y = y as u32;
        if h == 1 {
            sums(0, &mut a);
            for (d, v) in dst.iter_mut().zip(&a) {
                *d = ((v + 2) / 4) as u8;
            }
        } else if h % 2 == 0 {
            sums(y * 2, &mut a);
            sums(y * 2 + 1, &mut b);
            for (d, (p, q)) in dst.iter_mut().zip(a.iter().zip(&b)) {
                *d = ((p + q + 4) >> 3) as u8;
            }
        } else {
            sums(y * 2, &mut a);
            sums(y * 2 + 1, &mut b);
            sums(y * 2 + 2, &mut c);
            for (d, (p, (q, r))) in dst.iter_mut().zip(a.iter().zip(b.iter().zip(&c))) {
                *d = ((p + q * 2 + r + 8) >> 4) as u8;
            }
        }
    }
    (out, (dw, dh))
}

/// Halves until it is at `target`.
fn shrink(mut pixels: Vec<u8>, mut size: (u32, u32), target: (u32, u32)) -> (Vec<u8>, (u32, u32)) {
    while (size.0 > target.0 || size.1 > target.1) && (size.0 > 1 || size.1 > 1) {
        (pixels, size) = halved(&pixels, size);
    }
    (pixels, size)
}

/// A picture at `target` with what is needed to show it smaller.
fn still(pixels: Vec<u8>, size: (u32, u32), target: (u32, u32), opaque: bool) -> Decoded {
    let (pixels, size) = shrink(pixels, size, target);
    let mut levels = vec![pixels];
    let mut at = size;
    while at.0 > 1 || at.1 > 1 {
        let (next, next_size) = halved(levels.last().expect("there is the first"), at);
        levels.push(next);
        at = next_size;
    }
    Decoded {
        size,
        opaque,
        levels,
        frames: Vec::new(),
        loops: None,
    }
}

#[cfg(feature = "svg")]
fn svg_tree(bytes: &[u8]) -> Result<resvg::usvg::Tree, String> {
    resvg::usvg::Tree::from_data(bytes, &crate::svg_file::options_for(bytes))
        .map_err(|e| e.to_string())
}

#[cfg(feature = "svg")]
fn probe_svg(bytes: &[u8]) -> Result<Info, String> {
    let size = svg_tree(bytes)?.size();
    Ok(Info {
        pixels: (size.width().ceil() as u32, size.height().ceil() as u32),
        natural: (size.width(), size.height()),
        vector: true,
    })
}

#[cfg(feature = "svg")]
fn decode_svg(bytes: &[u8], target: (u32, u32)) -> Result<Decoded, String> {
    use resvg::tiny_skia::{Pixmap, Transform};
    let tree = svg_tree(bytes)?;
    let mut pixmap = Pixmap::new(target.0, target.1).ok_or("nothing to draw on")?;
    let size = tree.size();
    let (sx, sy) = (
        target.0 as f32 / size.width(),
        target.1 as f32 / size.height(),
    );
    // A box of other proportions gets the whole picture in the middle, unless it says otherwise. A
    // box that only differs by the rounding of its sides isn't one: that would leave a blurred edge.
    let k = sx.min(sy);
    let left = (
        target.0 as f32 - size.width() * k,
        target.1 as f32 - size.height() * k,
    );
    let stretches = bytes
        .windows(26)
        .any(|w| w == b"preserveAspectRatio=\"none\"");
    let transform = if stretches || (left.0 <= 1.0 && left.1 <= 1.0) {
        Transform::from_scale(sx, sy)
    } else {
        Transform::from_scale(k, k).post_translate(left.0 * 0.5, left.1 * 0.5)
    };
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // It comes out with premultiplied alpha, like everything here.
    Ok(still(pixmap.take(), target, target, false))
}

#[cfg(not(feature = "svg"))]
fn probe_svg(_: &[u8]) -> Result<Info, String> {
    Err("built without the feature `svg`".into())
}

#[cfg(not(feature = "svg"))]
fn decode_svg(_: &[u8], _: (u32, u32)) -> Result<Decoded, String> {
    Err("built without the feature `svg`".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(w: u32, h: u32) -> Info {
        Info {
            pixels: (w, h),
            natural: (w as f32, h as f32),
            vector: false,
        }
    }

    #[test]
    fn a_picture_is_decoded_at_the_smallest_half_that_is_big_enough() {
        let i = info(4000, 3000);
        assert_eq!(planned_size(&i, (4000, 3000), false), (4000, 3000));
        assert_eq!(planned_size(&i, (2001, 1), false), (4000, 3000));
        assert_eq!(planned_size(&i, (2000, 1500), false), (2000, 1500));
        assert_eq!(planned_size(&i, (1000, 751), false), (2000, 1500));
        assert_eq!(planned_size(&i, (200, 150), false), (250, 187));
        assert_eq!(planned_size(&i, (1, 1), false), (1, 1));
        // Bigger than the picture takes all there is.
        assert_eq!(planned_size(&i, (9000, 9000), false), (4000, 3000));
    }

    #[test]
    fn a_picture_with_square_pixels_stays_as_it_is() {
        assert_eq!(planned_size(&info(64, 64), (8, 8), true), (64, 64));
    }

    #[test]
    fn a_picture_too_big_for_a_texture_comes_down_to_one() {
        assert_eq!(
            planned_size(&info(20000, 100), (20000, 100), true),
            (5000, 25)
        );
    }

    #[test]
    fn what_is_drawn_from_a_description_is_drawn_at_the_size_it_shows() {
        let i = Info {
            vector: true,
            ..info(24, 24)
        };
        assert_eq!(planned_size(&i, (37, 41), false), (37, 41));
    }

    #[test]
    fn premultiplying_is_exact() {
        for c in 0..=255u32 {
            for a in 0..=255u32 {
                let mut p = [c as u8, 0, 0, a as u8];
                premultiply(&mut p);
                assert_eq!(p[0] as u32, (c * a * 2 + 255) / 510, "{c} {a}");
            }
        }
    }

    #[test]
    fn opaque_pixels_are_found_out() {
        assert!(premultiply(&mut [9, 9, 9, 255, 1, 1, 1, 255]));
        assert!(!premultiply(&mut [9, 9, 9, 255, 1, 1, 1, 254]));
    }

    fn solid(w: u32, h: u32, p: [u8; 4]) -> Vec<u8> {
        p.iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect()
    }

    #[test]
    fn halving_keeps_a_solid_color_whatever_the_size() {
        for (w, h) in [
            (8, 8),
            (7, 8),
            (8, 7),
            (7, 5),
            (1, 9),
            (9, 1),
            (2, 3),
            (3, 2),
        ] {
            let (out, size) = halved(&solid(w, h, [10, 200, 77, 255]), (w, h));
            assert_eq!(size, ((w / 2).max(1), (h / 2).max(1)));
            assert_eq!(out, solid(size.0, size.1, [10, 200, 77, 255]), "{w}x{h}");
        }
    }

    #[test]
    fn halving_averages_neighbours() {
        let src = [0, 0, 0, 255, 100, 100, 100, 255];
        assert_eq!(halved(&src, (2, 1)).0, [50, 50, 50, 255]);
        let src = [0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255];
        assert_eq!(halved(&src, (3, 1)).0, [100, 100, 100, 255]);
    }

    #[test]
    fn a_still_goes_down_to_one_pixel() {
        let d = still(solid(5, 3, [1, 2, 3, 255]), (5, 3), (5, 3), true);
        let sizes: Vec<usize> = d.levels.iter().map(Vec::len).collect();
        assert_eq!(sizes, [5 * 3 * 4, 2 * 4, 4]);
    }

    #[test]
    fn something_that_is_no_picture_is_an_error() {
        let source = ImageSource::Static(b"this is not a picture");
        assert!(probe(&source).is_err());
        assert!(decode(&source, (1, 1)).is_err());
    }

    #[test]
    fn a_missing_file_says_which() {
        let source = ImageSource::from("/nowhere/at/all.png");
        assert!(probe(&source).unwrap_err().contains("/nowhere/at/all.png"));
    }
}
