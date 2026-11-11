//! Pictures on real pixels: from a file or memory to the screen. See `docs/design/22-images.md`.

use anti_webui_app::headless::render_rgba;
use anti_webui_core::prelude::*;
use image::{ImageEncoder, codecs::png::PngEncoder};

const W: u32 = 64;

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

fn close(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tolerance)
}

#[track_caller]
fn assert_close(a: [u8; 4], b: [u8; 4], tolerance: u8) {
    assert!(close(a, b, tolerance), "{a:?} is not {b:?}");
}

/// Left half red, right half blue, with straight alpha.
fn two_colors(w: u32, h: u32, alpha: u8) -> Vec<u8> {
    (0..h)
        .flat_map(|_| {
            (0..w).flat_map(move |x| {
                if x < w / 2 {
                    [255, 0, 0, alpha]
                } else {
                    [0, 0, 255, alpha]
                }
            })
        })
        .collect()
}

fn png(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(pixels, w, h, image::ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn draw<V: IntoView>(view: impl FnOnce() -> V) -> Vec<u8> {
    render_rgba(W, W, 1.0, view).unwrap()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn a_png_shows_at_its_own_size() {
    let file = png(16, 8, &two_colors(16, 8, 255));
    let p = draw(|| div().flex().child(img(file).flex_none().self_start()));
    assert_close(pixel(&p, 1, 1), RED, 0);
    assert_close(pixel(&p, 14, 6), BLUE, 0);
    assert_close(pixel(&p, 17, 1), WHITE, 0);
    assert_close(pixel(&p, 1, 9), WHITE, 0);
}

/// Left half one color, right half the other.
fn sides(w: u32, h: u32) -> ImageSource {
    ImageSource::rgba(w, h, two_colors(w, h, 255))
}

#[test]
fn a_picture_can_be_cut_by_object_fit() {
    let p = draw(|| {
        div().child(
            img(sides(100, 20))
                .w(20.0)
                .h(20.0)
                .object_fit(ObjectFit::Cover)
                .object_position(pct(0.0), pct(0.0)),
        )
    });
    // 20 of 100 across is all on the red side.
    assert_close(pixel(&p, 18, 10), RED, 0);
    assert_close(pixel(&p, 21, 10), WHITE, 0);
}

#[test]
fn contain_leaves_the_rest_of_the_box_to_the_background() {
    let p = draw(|| {
        div().child(
            img(sides(20, 10))
                .w(20.0)
                .h(20.0)
                .object_fit(ObjectFit::Contain)
                .bg(rgb(0x00ff00)),
        )
    });
    assert_close(pixel(&p, 10, 1), [0, 255, 0, 255], 0);
    assert_close(pixel(&p, 2, 10), RED, 0);
    assert_close(pixel(&p, 17, 10), BLUE, 0);
    assert_close(pixel(&p, 10, 18), [0, 255, 0, 255], 0);
}

#[test]
fn a_picture_is_cut_to_its_rounded_shape() {
    let solid = || ImageSource::rgba(32, 32, [255u8, 0, 0, 255].repeat(32 * 32));
    let p = draw(|| div().child(img(solid()).w(32.0).h(32.0).rounded(16.0)));
    assert_close(pixel(&p, 0, 0), WHITE, 0);
    assert_close(pixel(&p, 16, 16), RED, 0);
    assert_close(pixel(&p, 16, 1), RED, 0);
    let edge = pixel(&p, 4, 4);
    assert!(edge[1] > 0 && edge[1] < 255, "antialiased: {edge:?}");
}

#[test]
fn a_picture_with_alpha_shows_what_is_behind_it() {
    let half = ImageSource::rgba(4, 4, [255u8, 0, 0, 128].repeat(16));
    let p = draw(|| div().child(img(half).w(4.0).h(4.0).bg(rgb(0x0000ff))));
    assert_close(pixel(&p, 2, 2), [128, 0, 127, 255], 1);
}

#[test]
fn a_transparent_edge_doesnt_go_dark_when_it_is_scaled() {
    // A red square in nothing with a black color of its own: what is straight alpha gets dark
    // around the edge without premultiplying.
    let mut pixels = vec![0u8; 8 * 8 * 4];
    for y in 0..8 {
        for x in 0..4 {
            pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let p = draw(|| div().child(img(ImageSource::rgba(8, 8, pixels)).w(64.0).h(64.0)));
    for x in 20..44 {
        let c = pixel(&p, x, 30);
        assert_eq!(c[0], 255, "at {x}: {c:?}");
        assert_eq!(c[1], c[2], "at {x}: {c:?}");
    }
}

#[test]
fn a_fine_pattern_doesnt_shimmer_when_it_gets_smaller() {
    let check: Vec<u8> = (0..256u32 * 256)
        .flat_map(|i| {
            let (x, y) = (i % 256, i / 256);
            if (x + y) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        })
        .collect();
    let p = draw(|| div().child(img(ImageSource::rgba(256, 256, check)).w(32.0).h(32.0)));
    for (x, y) in [(5, 5), (17, 9), (30, 30)] {
        assert_close(pixel(&p, x, y), [127, 127, 127, 255], 2);
    }
}

/// Compared with Chrome, which reads the smallest level no smaller than the box, even when the next
/// one down is nearer in size.
#[test]
fn a_picture_a_little_smaller_is_read_from_the_level_above() {
    let stripes: Vec<u8> = (0..256u32 * 256)
        .flat_map(|i| {
            if (i % 256) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        })
        .collect();
    let p = draw(|| div().child(img(ImageSource::rgba(256, 256, stripes)).w(40.0).h(40.0)));
    // Far smaller, the stripes are gone: this is decoded at 64, with nothing left of them.
    let furthest = (0..40)
        .map(|x| pixel(&p, x, 20)[0].abs_diff(127))
        .max()
        .unwrap();
    assert!(furthest <= 2, "{furthest}");
    let stripes: Vec<u8> = (0..64u32 * 64)
        .flat_map(|i| {
            if (i % 64) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        })
        .collect();
    let p = draw(|| div().child(img(ImageSource::rgba(64, 64, stripes)).w(40.0).h(40.0)));
    let furthest = (0..40)
        .map(|x| pixel(&p, x, 20)[0].abs_diff(127))
        .max()
        .unwrap();
    // A little smaller they stay: the next level down would be all gray.
    assert!(furthest > 30, "{furthest}");
}

#[test]
fn square_pixels_stay_square() {
    let p = draw(|| {
        div().child(
            img(sides(2, 2))
                .w(32.0)
                .h(32.0)
                .image_rendering(ImageRendering::Pixelated),
        )
    });
    assert_close(pixel(&p, 15, 16), RED, 0);
    assert_close(pixel(&p, 16, 16), BLUE, 0);
}

#[test]
fn a_background_repeats_without_seams_and_stays_in_its_box() {
    let tile = sides(4, 4);
    let p = draw(|| div().child(div().w(32.0).h(16.0).bg_image(tile)));
    for x in 0..32 {
        let want = if x % 4 < 2 { RED } else { BLUE };
        assert_close(pixel(&p, x, 8), want, 0);
    }
    assert_close(pixel(&p, 33, 8), WHITE, 0);
    assert_close(pixel(&p, 8, 17), WHITE, 0);
}

#[test]
fn a_background_that_doesnt_repeat_doesnt_bleed_from_the_other_side() {
    let p = draw(|| {
        div().child(
            div()
                .w(32.0)
                .h(32.0)
                .bg_image(sides(4, 4))
                .bg_size(BackgroundSize::Cover)
                .bg_repeat(BackgroundRepeat::NoRepeat),
        )
    });
    assert_close(pixel(&p, 0, 16), RED, 0);
    assert_close(pixel(&p, 31, 16), BLUE, 0);
}

#[test]
fn a_border_goes_over_the_picture_of_a_background() {
    let p = draw(|| {
        div().child(
            div()
                .w(32.0)
                .h(32.0)
                .border(4.0)
                .border_color(rgb(0x00ff00))
                .bg_image(sides(32, 32)),
        )
    });
    assert_close(pixel(&p, 1, 16), [0, 255, 0, 255], 0);
    assert_close(pixel(&p, 8, 16), RED, 0);
}

/// Bytes for the two sides, in a JPEG that says how it is turned.
fn turned_jpeg(orientation: u8) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    let (w, h) = (32u32, 16u32);
    let rgb: Vec<u8> = (0..h)
        .flat_map(|_| (0..w).flat_map(move |x| if x < w / 2 { [255, 0, 0] } else { [0, 0, 255] }))
        .collect();
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 100);
    // Little endian TIFF with one tag.
    let mut exif = vec![];
    exif.extend(b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0");
    exif.extend([orientation, 0, 0, 0, 0, 0, 0, 0]);
    encoder.set_exif_metadata(exif).unwrap();
    encoder
        .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

#[test]
fn exif_says_which_way_is_up() {
    // Turned a quarter clockwise, the left side ends up on top.
    let p = draw(|| div().child(img(turned_jpeg(6))));
    assert_close(pixel(&p, 8, 4), RED, 40);
    assert_close(pixel(&p, 8, 28), BLUE, 40);
    assert_close(pixel(&p, 18, 4), WHITE, 0);
    assert_close(pixel(&p, 4, 34), WHITE, 0);
}

#[test]
fn a_picture_in_another_color_space_comes_out_in_srgb() {
    let profile = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    let pixels = [100u8, 200, 100, 255].repeat(4 * 4);
    let mut file = Vec::new();
    let mut encoder = PngEncoder::new(&mut file);
    encoder.set_icc_profile(profile).unwrap();
    encoder
        .write_image(&pixels, 4, 4, image::ExtendedColorType::Rgba8)
        .unwrap();
    let plain = draw(|| div().child(img(png(4, 4, &pixels))));
    let wide = draw(|| div().child(img(file)));
    assert_close(pixel(&plain, 2, 2), [100, 200, 100, 255], 0);
    // Green of a wider space is more green than the same numbers here.
    let c = pixel(&wide, 2, 2);
    assert!(c[0] < 90 && c[1] > 200, "{c:?}");
}

const SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8" viewBox="0 0 16 8">
<rect width="8" height="8" fill="#f00"/><rect x="8" width="8" height="8" fill="#00f"/></svg>"##;

#[test]
fn svg_has_a_size_of_its_own_and_is_drawn_at_the_size_it_shows() {
    let p = draw(|| div().child(img(SVG)));
    assert_close(pixel(&p, 2, 2), RED, 0);
    assert_close(pixel(&p, 14, 6), BLUE, 0);
    assert_close(pixel(&p, 17, 2), WHITE, 0);
    let p = draw(|| div().child(img(SVG).w(64.0).h(32.0)));
    // Scaled up from pixels this would be blurred across several.
    assert_close(pixel(&p, 31, 16), RED, 0);
    assert_close(pixel(&p, 32, 16), BLUE, 0);
}

#[test]
fn svg_keeps_its_proportions_in_a_box_of_others_unless_it_says_not_to() {
    let p = draw(|| div().child(img(SVG).w(64.0).h(64.0).bg(rgb(0x00ff00))));
    assert_close(pixel(&p, 32, 4), [0, 255, 0, 255], 0);
    assert_close(pixel(&p, 4, 32), RED, 0);
    let stretching = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8" viewBox="0 0 16 8" preserveAspectRatio="none"><rect width="16" height="8" fill="#f00"/></svg>"##;
    let p = draw(|| div().child(img(stretching).w(64.0).h(64.0).bg(rgb(0x00ff00))));
    assert_close(pixel(&p, 32, 4), RED, 0);
}

/// Text comes from the fonts of the system, which are only read for this.
#[test]
fn svg_with_text_shows_it() {
    let with_text = br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><text x="2" y="24" font-size="24" font-family="sans-serif" fill="#000">HH</text></svg>"##;
    let p = draw(|| div().child(img(with_text)));
    let dark = (0..32)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .filter(|(x, y)| pixel(&p, *x, *y)[0] < 64)
        .count();
    assert!(dark > 50, "{dark}");
}

#[test]
fn svg_of_two_sizes_at_once_is_sharp_in_both() {
    let p = draw(|| {
        div()
            .child(img(SVG).w(16.0).h(8.0))
            .child(img(SVG).w(32.0).h(16.0))
    });
    assert_close(pixel(&p, 7, 4), RED, 0);
    assert_close(pixel(&p, 8, 4), BLUE, 0);
    assert_close(pixel(&p, 15, 16), RED, 0);
    assert_close(pixel(&p, 16, 16), BLUE, 0);
}

#[test]
fn a_picture_in_a_transformed_layer_is_drawn_with_it() {
    let p = draw(|| {
        div().child(
            img(sides(16, 16))
                .w(16.0)
                .h(16.0)
                .translate_x(20.0)
                .scale(2.0),
        )
    });
    // Twice as big around its middle, 8 further right.
    assert_close(pixel(&p, 14, 8), RED, 0);
    assert_close(pixel(&p, 40, 8), BLUE, 0);
    assert_close(pixel(&p, 10, 8), WHITE, 0);
}

#[test]
fn a_picture_fades_with_opacity() {
    let p = draw(|| div().child(img(sides(8, 8)).w(8.0).h(8.0).opacity(0.5)));
    assert_close(pixel(&p, 1, 1), [255, 127, 127, 255], 1);
}

#[test]
fn a_picture_fades_with_image_opacity_over_its_background() {
    let p = draw(|| {
        div().child(
            img(sides(8, 8))
                .w(8.0)
                .h(8.0)
                .bg(rgb(0x00ff00))
                .image_opacity(0.0),
        )
    });
    assert_close(pixel(&p, 1, 1), [0, 255, 0, 255], 0);
}

/// Opaque pictures skip blending, and that must make no difference.
#[test]
fn drawing_opaque_pictures_without_blending_comes_out_the_same() {
    let opaque = sides(16, 16);
    let same_with_alpha = {
        let mut pixels = two_colors(16, 16, 255);
        pixels[3] = 254;
        ImageSource::rgba(16, 16, pixels)
    };
    let a = draw(|| div().child(img(opaque).w(32.0).h(32.0)));
    let b = draw(|| div().child(img(same_with_alpha).w(32.0).h(32.0)));
    for y in 4..32 {
        for x in 0..32 {
            assert_close(pixel(&a, x, y), pixel(&b, x, y), 2);
        }
    }
}

mod on_the_gpu {
    use anti_webui_core::Ui;
    use anti_webui_core::event::WindowEvent;
    use anti_webui_core::geometry::{Scale, SizeI};
    use anti_webui_core::prelude::*;
    use anti_webui_wgpu::{Gpu, Renderer, wgpu::TextureFormat};
    use std::time::{Duration, Instant};

    fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
        let mut ui = Ui::new(f);
        ui.handle_event(WindowEvent::Resized {
            size: SizeI::new(64, 64),
            scale: Scale::ONE,
        });
        ui.handle_event(WindowEvent::Focused(true));
        ui.update();
        ui
    }

    fn solid(c: [u8; 4], side: u32) -> ImageSource {
        ImageSource::rgba(side, side, c.repeat((side * side) as usize))
    }

    #[test]
    fn what_is_on_the_gpu_goes_when_nothing_has_its_pixels() {
        update_settings(|s| s.images.cache_bytes = 0);
        let gpu = Gpu::headless().unwrap();
        let mut renderer = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        let on = signal(true);
        let mut ui = mount(move || {
            div().child(show(
                move || on.get(),
                || img(solid([1, 2, 3, 255], 32)).w(32.0).h(32.0),
            ))
        });
        ui.wait_for_images(Duration::from_secs(30));
        let scene = ui.paint(Instant::now());
        let _ = gpu.render_to_rgba(&mut renderer, &scene);
        assert_eq!(renderer.picture_counts().0, 1);
        // 32x32 and its levels.
        assert_eq!(renderer.picture_counts().1, 32 * 32 * 4 * 4 / 3);
        drop(scene);
        on.set(false);
        ui.update();
        let scene = ui.paint(Instant::now());
        let scene2 = ui.paint(Instant::now());
        let _ = gpu.render_to_rgba(&mut renderer, &scene2);
        drop(scene);
        assert_eq!(renderer.picture_counts().0, 0);
    }

    #[test]
    fn pixels_can_go_once_the_gpu_has_them() {
        let gpu = Gpu::headless().unwrap();
        let mut renderer = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        renderer.set_release_picture_pixels(true);
        let mut ui = mount(|| div().child(img(solid([255, 0, 0, 255], 16)).w(16.0).h(16.0)));
        ui.wait_for_images(Duration::from_secs(30));
        let scene = ui.paint(Instant::now());
        let first = gpu.render_to_rgba(&mut renderer, &scene);
        // Drawn again from the texture, with nothing left on this side.
        let again = gpu.render_to_rgba(&mut renderer, &scene);
        assert_eq!(first, again);
        assert_eq!(&first[0..4], &[255, 0, 0, 255]);
        let anti_webui_core::scene::Primitive::Image(d) = &scene
            .primitives
            .iter()
            .find(|p| matches!(p, anti_webui_core::scene::Primitive::Image(_)))
            .unwrap()
        else {
            unreachable!()
        };
        assert!(!d.bitmap.with_levels(|_, _, _| {}));
    }

    #[test]
    fn pixels_the_program_made_can_be_drawn_by_another_renderer_afterwards() {
        // A notification's picture shows in its popup and in the list, which are windows of their
        // own: the second renderer to draw it still has pixels to put on its GPU.
        use anti_webui_core::images::Bitmap;
        use std::sync::Arc;
        let gpu = Gpu::headless().unwrap();
        let bitmap = Arc::new(Bitmap::from_premultiplied(
            2,
            2,
            [0, 0, 255, 255].repeat(4),
            true,
        ));
        let draw = |renderer: &mut Renderer| {
            let bitmap = bitmap.clone();
            let mut ui = mount(move || {
                div().child(bitmap_view(move || Some(bitmap.clone())).w(16.0).h(16.0))
            });
            let scene = ui.paint(Instant::now());
            gpu.render_to_rgba(renderer, &scene)
        };
        let mut first = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        first.set_release_picture_pixels(true);
        let mut second = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        second.set_release_picture_pixels(true);
        assert_eq!(&draw(&mut first)[0..4], &[0, 0, 255, 255]);
        assert_eq!(&draw(&mut second)[0..4], &[0, 0, 255, 255]);
        assert!(bitmap.with_levels(|_, _, _| {}));
    }

    #[test]
    fn the_frames_of_an_animation_reach_the_screen() {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba, RgbaImage};
        let mut file = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut file);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for c in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(Frame::from_parts(
                        RgbaImage::from_pixel(8, 8, Rgba(c)),
                        0,
                        0,
                        Delay::from_numer_denom_ms(50, 1),
                    ))
                    .unwrap();
            }
        }
        let gpu = Gpu::headless().unwrap();
        let mut renderer = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        let mut ui = mount(|| div().child(img(file).w(8.0).h(8.0)));
        ui.wait_for_images(Duration::from_secs(30));
        let scene = ui.paint(Instant::now());
        let p = gpu.render_to_rgba(&mut renderer, &scene);
        assert!(p[0] > 240 && p[2] < 15, "{:?}", &p[..4]);
        let due = ui.next_deadline(Instant::now()).unwrap();
        ui.tick(due);
        ui.update();
        let scene = ui.paint(Instant::now());
        let p = gpu.render_to_rgba(&mut renderer, &scene);
        assert!(p[0] < 15 && p[2] > 240, "{:?}", &p[..4]);
    }

    /// The renderer's damaged path has to come to the same as drawing all of it.
    #[test]
    fn redrawing_part_of_a_window_with_a_picture_comes_out_the_same() {
        let gpu = Gpu::headless().unwrap();
        let which = signal(false);
        let (a, b) = (solid([255, 0, 0, 255], 32), solid([0, 0, 255, 255], 32));
        let mut ui = mount(move || {
            div().child(div().h(8.0).bg(rgb(0x00ff00))).child(
                img(move || if which.get() { b.clone() } else { a.clone() })
                    .w(32.0)
                    .h(32.0)
                    .rounded(8.0),
            )
        });
        ui.wait_for_images(Duration::from_secs(30));
        let first = ui.paint(Instant::now());
        which.set(true);
        ui.wait_for_images(Duration::from_secs(30));
        let second = ui.paint(Instant::now());

        let mut renderer = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        let target = anti_webui_wgpu::Offscreen::new(&gpu, renderer.format(), 64, 64);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &first);
        gpu.queue.submit([encoder.finish()]);
        let damage = anti_webui_core::damage::diff(&first, &second);
        assert!(damage.area() <= 32 * 32);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &second,
            &damage,
        );
        gpu.queue.submit([encoder.finish()]);
        let partly = target.read_rgba(&gpu);
        let mut other = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
        let whole = gpu.render_to_rgba(&mut other, &second);
        assert_eq!(partly, whole);
    }
}
