//! How pictures take part in everything but the pixels. See `docs/design/22-images.md`.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{ImageDraw, Primitive, Scene};
use anti_webui_core::tree::with_doc;
use image::codecs::gif::GifEncoder;
use image::{Delay, Frame, ImageEncoder, RgbaImage, codecs::png::PngEncoder};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn mount_at<V: IntoView>(scale: Scale, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    mount_at(Scale::ONE, f)
}

fn load(ui: &mut Ui) {
    ui.wait_for_images(Duration::from_secs(30));
    ui.update();
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let pixels = vec![200u8; (w * h * 4) as usize];
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&pixels, w, h, image::ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn rgba(w: u32, h: u32) -> ImageSource {
    ImageSource::rgba(w, h, vec![200u8; (w * h * 4) as usize])
}

fn gif(colors: &[[u8; 4]], delay_ms: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut out);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        for c in colors {
            let picture = RgbaImage::from_pixel(8, 8, image::Rgba(*c));
            encoder
                .encode_frame(Frame::from_parts(
                    picture,
                    0,
                    0,
                    Delay::from_numer_denom_ms(delay_ms, 1),
                ))
                .unwrap();
        }
    }
    out
}

fn pictures(scene: &Scene) -> Vec<ImageDraw> {
    fn walk(scene: &Scene, out: &mut Vec<ImageDraw>) {
        for p in &scene.primitives {
            match p {
                Primitive::Image(d) => out.push(d.clone()),
                Primitive::Layer(l) => walk(&l.content, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(scene, &mut out);
    out
}

fn the_picture(ui: &mut Ui) -> ImageDraw {
    let scene = ui.paint(Instant::now());
    let mut all = pictures(&scene);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

fn size_of(place: NodeRef) -> (i32, i32) {
    let r = place.rect().expect("laid out");
    (r.width, r.height)
}

#[test]
fn a_picture_takes_no_room_until_its_size_is_known() {
    let place = node_ref();
    let mut ui = mount(|| div().child(img(png(120, 80)).node_ref(place)));
    let _ = ui.paint(Instant::now());
    assert_eq!(size_of(place), (0, 0));
    load(&mut ui);
    assert_eq!(size_of(place), (120, 80));
}

#[test]
fn a_box_with_a_size_doesnt_move_when_the_picture_comes() {
    let place = node_ref();
    let mut ui = mount(|| div().child(img(png(120, 80)).w(50.0).h(50.0).node_ref(place)));
    let _ = ui.paint(Instant::now());
    assert_eq!(size_of(place), (50, 50));
    load(&mut ui);
    assert_eq!(size_of(place), (50, 50));
}

#[test]
fn a_pixel_of_a_picture_is_a_logical_pixel() {
    let place = node_ref();
    let mut ui = mount_at(Scale::from_f64(1.8), || {
        div().child(img(png(21, 10)).node_ref(place))
    });
    load(&mut ui);
    assert_eq!(size_of(place), (38, 18));
}

#[test]
fn density_makes_the_box_smaller() {
    let place = node_ref();
    let mut ui = mount(|| div().child(img(png(120, 80)).density(2.0).node_ref(place)));
    load(&mut ui);
    assert_eq!(size_of(place), (60, 40));
}

/// As in a browser, unlike a `div`.
#[test]
fn a_picture_in_a_block_is_as_wide_as_it_is() {
    let place = node_ref();
    let mut ui = mount(|| div().w(400.0).child(img(png(120, 80)).node_ref(place)));
    load(&mut ui);
    assert_eq!(size_of(place), (120, 80));
}

/// `max-width: 100%`, as everyone writes for pictures.
#[test]
fn a_limit_keeps_the_proportions() {
    let place = node_ref();
    let mut ui = mount(|| {
        div()
            .w(60.0)
            .child(img(png(120, 80)).max_w(pct(100.0)).node_ref(place))
    });
    load(&mut ui);
    assert_eq!(size_of(place), (60, 40));
}

/// Compared with Chrome: a column stretches it across, and the height follows.
#[test]
fn a_picture_stretched_across_a_column_keeps_its_proportions() {
    let place = node_ref();
    let mut ui = mount(|| {
        div()
            .w(300.0)
            .flex()
            .flex_col()
            .child(img(png(120, 80)).node_ref(place))
    });
    load(&mut ui);
    assert_eq!(size_of(place), (300, 200));
}

/// Compared with Chrome: a row of a given height stretches it down, and the width follows.
#[test]
fn a_picture_stretched_down_a_row_keeps_its_proportions() {
    let place = node_ref();
    let mut ui = mount(|| {
        div()
            .h(160.0)
            .flex()
            .child(img(png(120, 80)).node_ref(place))
    });
    load(&mut ui);
    assert_eq!(size_of(place), (240, 160));
}

/// The other cases of a row, compared with Chrome.
#[test]
fn a_picture_in_a_row_goes_by_the_rules_of_css() {
    let size = |limit: Option<f32>, tune: fn(Img) -> Img| {
        let place = node_ref();
        let mut ui = mount(move || {
            let row = div().flex();
            let row = match limit {
                Some(w) => row.w(w),
                None => row,
            };
            row.child(tune(img(png(120, 80))).node_ref(place))
        });
        load(&mut ui);
        size_of(place)
    };
    // It doesn't shrink below its own size, unless it is allowed to.
    assert_eq!(size(Some(60.0), |i| i), (120, 80));
    assert_eq!(size(Some(60.0), |i| i.min_w(0.0)), (60, 40));
    assert_eq!(size(None, |i| i), (120, 80));
    assert_eq!(
        size(Some(400.0), |i| i.flex_grow(1.0).flex_basis(0.0).min_w(0.0)),
        (400, 267)
    );
}

#[test]
fn padding_and_border_are_left_out_of_the_proportions() {
    let place = node_ref();
    let mut ui = mount(|| {
        div().child(
            img(png(120, 80))
                .w(130.0)
                .p(10.0)
                .border(5.0)
                .node_ref(place),
        )
    });
    load(&mut ui);
    assert_eq!(size_of(place), (130, 97));
}

#[test]
fn a_ratio_gives_any_box_its_other_side() {
    let place = node_ref();
    let mut ui = mount(|| {
        div()
            .w(200.0)
            .child(div().aspect_ratio(2.0).node_ref(place))
    });
    let _ = ui.paint(Instant::now());
    assert_eq!(size_of(place), (200, 100));
}

#[test]
fn what_is_in_a_box_with_a_ratio_can_make_it_bigger() {
    let place = node_ref();
    let mut ui = mount(|| {
        div()
            .w(200.0)
            .child(div().aspect_ratio(4.0).node_ref(place).child(div().h(90.0)))
    });
    let _ = ui.paint(Instant::now());
    assert_eq!(size_of(place), (200, 90));
}

#[test]
fn what_lies_over_a_picture_has_no_say_in_its_size() {
    let (place, over) = (node_ref(), node_ref());
    let mut ui = mount(|| {
        div().child(
            img(png(100, 50))
                .node_ref(place)
                .child(div().h(500.0).w(500.0).node_ref(over)),
        )
    });
    load(&mut ui);
    assert_eq!(size_of(place), (100, 50));
    assert_eq!(size_of(over), (500, 500));
}

#[test]
fn those_who_ask_are_told_once() {
    let (sized, decoded, failed) = (
        Rc::new(Cell::new(None)),
        Rc::new(Cell::new(0)),
        Rc::new(Cell::new(0)),
    );
    let (a, b, c) = (sized.clone(), decoded.clone(), failed.clone());
    let mut ui = mount(move || {
        div().child(
            img(png(120, 80))
                .on_load(move |w, h| a.set(Some((w, h))))
                .on_decoded(move || b.set(b.get() + 1))
                .on_error(move |_| c.set(c.get() + 1)),
        )
    });
    load(&mut ui);
    for i in 0..3 {
        ui.animate(Instant::now() + Duration::from_millis(i));
        ui.update();
        let _ = ui.paint(Instant::now());
    }
    assert_eq!(sized.get(), Some((120.0, 80.0)));
    assert_eq!(decoded.get(), 1);
    assert_eq!(failed.get(), 0);
}

#[test]
fn something_that_cant_be_shown_says_so_and_takes_no_room() {
    let message = Rc::new(Cell::new(String::new()));
    let place = node_ref();
    let m = message.clone();
    let mut ui = mount(move || {
        div().child(
            img("/no/such/file.png")
                .node_ref(place)
                .on_error(move |e| m.set(e.to_owned())),
        )
    });
    load(&mut ui);
    assert!(message.take().contains("/no/such/file.png"));
    assert_eq!(size_of(place), (0, 0));
    assert!(pictures(&ui.paint(Instant::now())).is_empty());
}

/// A file that starts like a picture and goes wrong later is only found out when it is decoded.
#[test]
fn a_file_cut_short_says_so_when_it_is_decoded() {
    let mut file = png(64, 64);
    file.truncate(file.len() / 2);
    let failed = Rc::new(Cell::new(false));
    let f = failed.clone();
    let mut ui =
        mount(move || div().child(img(file).w(64.0).h(64.0).on_error(move |_| f.set(true))));
    load(&mut ui);
    assert!(failed.get());
}

#[test]
fn the_last_picture_stays_until_the_next_is_there() {
    let which = signal(0);
    let (a, b) = (rgba(40, 20), rgba(10, 10));
    let place = node_ref();
    let mut ui = mount(move || {
        div().child(
            img(move || {
                if which.get() == 0 {
                    a.clone()
                } else {
                    b.clone()
                }
            })
            .node_ref(place),
        )
    });
    load(&mut ui);
    let before = the_picture(&mut ui).bitmap.id();
    assert_eq!(size_of(place), (40, 20));
    which.set(1);
    ui.update();
    // Nothing that was decoded in the meantime has been taken in.
    assert_eq!(the_picture(&mut ui).bitmap.id(), before);
    assert_eq!(size_of(place), (40, 20));
    load(&mut ui);
    assert_ne!(the_picture(&mut ui).bitmap.id(), before);
    assert_eq!(size_of(place), (10, 10));
}

#[test]
fn a_picture_is_decoded_for_the_size_it_shows() {
    let side = signal(100.0f32);
    let mut ui = mount(move || {
        div().child(img(rgba(1600, 1200)).dyn_style(move |s| s.w(side.get()).h(side.get())))
    });
    load(&mut ui);
    let small = the_picture(&mut ui).bitmap;
    assert_eq!((small.width(), small.height()), (200, 150));

    // Not again for something smaller.
    side.set(50.0);
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).bitmap.id(), small.id());

    // Until it is there, what there is goes on showing.
    side.set(500.0);
    ui.update();
    assert_eq!(the_picture(&mut ui).bitmap.id(), small.id());
    load(&mut ui);
    let big = the_picture(&mut ui).bitmap;
    assert_eq!((big.width(), big.height()), (800, 600));
}

#[test]
fn the_scale_of_the_window_counts_for_the_size_it_shows() {
    let mut ui = mount_at(Scale::from_f64(2.0), || {
        div().child(img(rgba(1600, 1200)).w(100.0).h(100.0))
    });
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).bitmap.width(), 400);
}

/// What is scaled up later would show its pixels otherwise.
#[test]
fn a_transform_counts_for_the_size_it_shows() {
    let k = signal(1.0f32);
    let mut ui = mount(move || {
        div().child(
            img(rgba(1600, 1200))
                .w(100.0)
                .h(100.0)
                .dyn_style(move |s| s.scale(k.get())),
        )
    });
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).bitmap.width(), 200);
    k.set(4.0);
    load(&mut ui);
    // 400 high takes a picture 600 high, with a width that goes with that.
    assert_eq!(the_picture(&mut ui).bitmap.width(), 800);
}

#[test]
fn a_transform_around_what_is_in_a_scroller_counts_too() {
    let k = signal(1.0f32);
    let mut ui = mount(move || {
        div().child(
            div()
                .w(300.0)
                .h(300.0)
                .dyn_style(move |s| s.scale(k.get()))
                .child(
                    div()
                        .w(300.0)
                        .h(300.0)
                        .overflow_y_scroll()
                        .child(img(rgba(1600, 1200)).w(100.0).h(100.0))
                        .child(div().h(1000.0)),
                ),
        )
    });
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).bitmap.width(), 200);
    k.set(4.0);
    load(&mut ui);
    // 400 high takes a picture 600 high, with a width that goes with that.
    assert_eq!(the_picture(&mut ui).bitmap.width(), 800);
}

#[test]
fn square_pixels_are_kept_as_they_are() {
    let mut ui = mount(|| {
        div().child(
            img(rgba(64, 64))
                .w(8.0)
                .h(8.0)
                .image_rendering(ImageRendering::Pixelated),
        )
    });
    load(&mut ui);
    let d = the_picture(&mut ui);
    assert_eq!(d.bitmap.width(), 64);
    assert!(d.pixelated);
}

#[test]
fn what_is_far_from_the_screen_isnt_decoded() {
    let mut ui = mount(|| {
        div()
            .h_full()
            .overflow_y_scroll()
            .children((0..200).map(|_| {
                // Each of its own, so that they count.
                img(rgba(16, 16)).w(100.0).h(100.0).flex_none()
            }))
    });
    load(&mut ui);
    let (known, decoded, _) = with_doc(|d| d.images.get_mut().counts());
    assert_eq!(known, 200, "the sizes are read all the same");
    // The screen is 6 rows, and a screen around it 12 more.
    assert!(decoded <= 24, "{decoded}");
    assert!(decoded >= 6, "{decoded}");
}

#[test]
fn scrolling_to_a_picture_decodes_it() {
    let far = node_ref();
    let mut ui = mount(|| {
        div()
            .h_full()
            .overflow_y_scroll()
            .child(div().h(5000.0).flex_none())
            .child(
                img(rgba(16, 16))
                    .w(100.0)
                    .h(100.0)
                    .flex_none()
                    .node_ref(far),
            )
    });
    load(&mut ui);
    assert!(pictures(&ui.paint(Instant::now())).is_empty());
    far.scroll_into_view();
    ui.update();
    load(&mut ui);
    assert_eq!(pictures(&ui.paint(Instant::now())).len(), 1);
}

#[test]
fn a_picture_nobody_shows_is_forgotten_when_it_is_gone_from_memory() {
    let on = signal(true);
    let mut ui = mount(move || {
        div().child(show(
            move || on.get(),
            || img(ImageSource::from(png(8, 8))).w(8.0).h(8.0),
        ))
    });
    ui.wait_for_images(Duration::from_secs(30));
    on.set(false);
    ui.update();
    update_settings(|s| s.images.cache_bytes = 0);
    let _ = ui.paint(Instant::now());
    let _ = ui.paint(Instant::now());
    assert_eq!(with_doc(|d| d.images.get_mut().counts()), (0, 0, 0));
}

#[test]
fn what_goes_over_the_budget_goes_unless_it_shows() {
    update_settings(|s| s.images.cache_bytes = 0);
    let shown = signal(0usize);
    let (a, b) = (rgba(64, 64), rgba(64, 64));
    let mut ui = mount(move || {
        div().child(
            img(move || {
                if shown.get() == 0 {
                    a.clone()
                } else {
                    b.clone()
                }
            })
            .w(64.0)
            .h(64.0),
        )
    });
    load(&mut ui);
    // The scene that shows it is held on to, like a presenter does.
    let scene = ui.paint(Instant::now());
    let _ = ui.paint(Instant::now());
    assert_eq!(with_doc(|d| d.images.get_mut().counts()).1, 1);
    assert_eq!(pictures(&scene).len(), 1);
    shown.set(1);
    load(&mut ui);
    let second = ui.paint(Instant::now());
    drop(scene);
    let _ = ui.paint(Instant::now());
    let (_, held, bytes) = with_doc(|d| d.images.get_mut().counts());
    assert_eq!(held, 1);
    assert_eq!(bytes as u32, picture_bytes(64));
    assert_eq!(pictures(&second).len(), 1);
}

fn picture_bytes(mut side: u32) -> u32 {
    let mut total = 0;
    loop {
        total += side * side * 4;
        if side == 1 {
            return total;
        }
        side /= 2;
    }
}

/// With a budget of nothing, what comes back into view has to be decoded again and is.
#[test]
fn what_was_thrown_away_comes_back() {
    update_settings(|s| s.images.cache_bytes = 0);
    let on = signal(true);
    let source = rgba(16, 16);
    let mut ui = mount(move || {
        let source = source.clone();
        div().child(
            div()
                .w(16.0)
                .h(16.0)
                .bg_image(source.clone())
                .dyn_style(move |s| if on.get() { s } else { s.no_bg_image() }),
        )
    });
    load(&mut ui);
    assert_eq!(pictures(&ui.paint(Instant::now())).len(), 1);
    on.set(false);
    ui.update();
    let _ = ui.paint(Instant::now());
    let _ = ui.paint(Instant::now());
    assert_eq!(with_doc(|d| d.images.get_mut().counts()).1, 0);
    on.set(true);
    load(&mut ui);
    assert_eq!(pictures(&ui.paint(Instant::now())).len(), 1);
}

#[test]
fn a_picture_arriving_damages_only_itself() {
    let mut ui = mount(|| {
        div()
            .child(div().h(100.0).bg(rgb(0x123456)))
            .child(img(rgba(64, 64)).w(64.0).h(64.0).flex_none())
    });
    let before = ui.paint(Instant::now());
    load(&mut ui);
    let after = ui.paint(Instant::now());
    let d = damage::diff(&before, &after);
    assert_eq!(d.bounds(), RectI::new(0, 100, 64, 64));
}

#[test]
fn nothing_changes_once_it_is_there() {
    let mut ui = mount(|| div().child(img(rgba(64, 64)).w(64.0).h(64.0)));
    load(&mut ui);
    let a = ui.paint(Instant::now());
    let b = ui.paint(Instant::now());
    assert!(damage::diff(&a, &b).is_empty());
    assert!(!ui.needs_redraw());
    assert!(!ui.animating());
    assert_eq!(ui.next_deadline(Instant::now()), None);
}

#[test]
fn only_what_a_scroller_holds_is_painted_again_when_a_picture_arrives() {
    let mut ui = mount(|| {
        div()
            .h_full()
            .overflow_y_scroll()
            .child(img(rgba(32, 32)).w(32.0).h(32.0).flex_none())
            .child(div().h(5000.0).flex_none().bg(rgb(0x808080)))
    });
    let before = ui.paint(Instant::now());
    load(&mut ui);
    let after = ui.paint(Instant::now());
    assert_eq!(pictures(&after).len(), 1);
    assert_eq!(
        damage::diff(&before, &after).bounds(),
        RectI::new(0, 0, 32, 32)
    );
}

fn fit_of(fit: ObjectFit, position: (Length, Length)) -> ImageDraw {
    let mut ui = mount(move || {
        div().child(
            img(rgba(400, 300))
                .w(120.0)
                .h(60.0)
                .object_fit(fit)
                .object_position(position.0, position.1),
        )
    });
    load(&mut ui);
    the_picture(&mut ui)
}

/// The same numbers come out of the pixels of Chrome: see `docs/design/22-images.md`.
#[test]
fn object_fit_puts_the_picture_where_css_says() {
    let middle = (pct(50.0), pct(50.0));
    let at = |fit, position| fit_of(fit, position).rect;
    assert_eq!(at(ObjectFit::Fill, middle), RectI::new(0, 0, 120, 60));
    assert_eq!(at(ObjectFit::Contain, middle), RectI::new(20, 0, 80, 60));
    assert_eq!(at(ObjectFit::Cover, middle), RectI::new(0, -15, 120, 90));
    assert_eq!(
        at(ObjectFit::None, middle),
        RectI::new(-140, -120, 400, 300)
    );
    assert_eq!(at(ObjectFit::ScaleDown, middle), RectI::new(20, 0, 80, 60));
    let start = (pct(0.0), pct(0.0));
    assert_eq!(at(ObjectFit::Cover, start), RectI::new(0, 0, 120, 90));
    let end = (pct(100.0), pct(100.0));
    assert_eq!(at(ObjectFit::None, end), RectI::new(-280, -240, 400, 300));
    let offset = (px(-20.0), px(-30.0));
    assert_eq!(at(ObjectFit::None, offset), RectI::new(-20, -30, 400, 300));
}

#[test]
fn a_picture_goes_no_further_than_its_content_box() {
    let d = fit_of(ObjectFit::None, (pct(50.0), pct(50.0)));
    assert_eq!(d.area(), RectI::new(0, 0, 120, 60));
}

#[test]
fn the_corners_of_a_picture_are_those_of_the_content_box() {
    let mut ui = mount(|| {
        div().child(
            img(rgba(64, 64))
                .w(100.0)
                .h(100.0)
                .rounded(20.0)
                .border(4.0)
                .p(6.0),
        )
    });
    load(&mut ui);
    let d = the_picture(&mut ui);
    assert_eq!(d.shape.rect, RectI::new(10, 10, 80, 80));
    assert_eq!(d.shape.radii, [10.0; 4]);
}

#[test]
fn one_picture_alone_fades_without_a_layer_and_with_a_background_with_one() {
    let layers = |bg: bool| {
        let mut ui = mount(move || {
            let image = img(rgba(16, 16)).w(16.0).h(16.0).opacity(0.5);
            div().child(if bg { image.bg(rgb(0xff0000)) } else { image })
        });
        load(&mut ui);
        let scene = ui.paint(Instant::now());
        scene
            .primitives
            .iter()
            .filter(|p| matches!(p, Primitive::Layer(_)))
            .count()
    };
    assert_eq!(layers(false), 0);
    assert_eq!(layers(true), 1);
}

/// In a window nothing else may come along to paint: what arrives has to ask for it.
#[test]
fn what_arrives_for_a_background_asks_for_a_frame() {
    let mut ui = mount(|| div().child(div().w(16.0).h(16.0).bg_image(rgba(16, 16))));
    let _ = ui.paint(Instant::now());
    assert!(pictures(&ui.paint(Instant::now())).is_empty());
    let until = Instant::now() + Duration::from_secs(30);
    // Its size first, and then a frame to say what size it is needed at.
    while !ui.needs_redraw() {
        assert!(Instant::now() < until);
        ui.poll_images();
        std::thread::sleep(Duration::from_millis(1));
    }
    ui.update();
    let _ = ui.paint(Instant::now());
    // Then its pixels, without anyone doing anything else.
    while !ui.needs_redraw() {
        assert!(Instant::now() < until);
        ui.poll_images();
        std::thread::sleep(Duration::from_millis(1));
    }
    ui.update();
    assert_eq!(pictures(&ui.paint(Instant::now())).len(), 1);
}

#[test]
fn what_arrives_for_a_picture_asks_for_a_frame() {
    let mut ui = mount(|| div().child(img(rgba(16, 16)).w(16.0).h(16.0)));
    let _ = ui.paint(Instant::now());
    let until = Instant::now() + Duration::from_secs(30);
    for _ in 0..2 {
        while !ui.needs_redraw() {
            assert!(Instant::now() < until);
            ui.poll_images();
            std::thread::sleep(Duration::from_millis(1));
        }
        ui.update();
        let _ = ui.paint(Instant::now());
    }
    assert_eq!(pictures(&ui.paint(Instant::now())).len(), 1);
}

#[test]
fn an_animation_nothing_shows_any_more_doesnt_wake_anything_up() {
    let on = signal(true);
    let file = gif(&[RED, BLUE], 50);
    let source: ImageSource = file.into();
    let mut ui = mount(move || {
        let source = source.clone();
        div().child(div().w(8.0).h(8.0).dyn_style(move |s| {
            if on.get() {
                s.bg_image(source.clone())
            } else {
                s
            }
        }))
    });
    load(&mut ui);
    assert!(ui.next_deadline(Instant::now()).is_some());
    on.set(false);
    ui.update();
    let now = Instant::now();
    let _ = ui.paint(now);
    assert_eq!(ui.next_deadline(now), None);
}

#[test]
fn a_backgrounds_picture_changes_with_hover() {
    let (a, b) = (rgba(8, 8), rgba(16, 16));
    let mut ui = mount(move || {
        div().child(
            div()
                .w(100.0)
                .h(100.0)
                .bg_image(a)
                .hover(move |s| s.bg_image(b.clone())),
        )
    });
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).bitmap.width(), 8);
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(50.0, 50.0),
        modifiers: Modifiers::default(),
    });
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).bitmap.width(), 16);
}

fn background_of(
    size: BackgroundSize,
    position: (Length, Length),
    repeat: BackgroundRepeat,
) -> ImageDraw {
    let mut ui = mount(move || {
        div().child(
            div()
                .w(100.0)
                .h(50.0)
                .bg_image(rgba(16, 16))
                .bg_size(size)
                .bg_position(position.0, position.1)
                .bg_repeat(repeat),
        )
    });
    load(&mut ui);
    the_picture(&mut ui)
}

#[test]
fn a_background_goes_where_css_says() {
    let start = (pct(0.0), pct(0.0));
    let r = BackgroundRepeat::NoRepeat;
    let at = |size, position| background_of(size, position, r).rect;
    assert_eq!(at(BackgroundSize::Auto, start), RectI::new(0, 0, 16, 16));
    assert_eq!(
        at(BackgroundSize::Cover, (pct(50.0), pct(50.0))),
        RectI::new(0, -25, 100, 100)
    );
    assert_eq!(
        at(BackgroundSize::Contain, (pct(50.0), pct(50.0))),
        RectI::new(25, 0, 50, 50)
    );
    assert_eq!(
        at(BackgroundSize::Size(pct(50.0), AUTO), start),
        RectI::new(0, 0, 50, 50)
    );
    assert_eq!(
        at(BackgroundSize::Auto, (px(10.0), px(7.0))),
        RectI::new(10, 7, 16, 16)
    );
}

#[test]
fn what_repeats_touches_all_the_box_and_what_doesnt_only_its_place() {
    let start = (pct(0.0), pct(0.0));
    let of = |r| background_of(BackgroundSize::Auto, start, r).area();
    assert_eq!(of(BackgroundRepeat::Repeat), RectI::new(0, 0, 100, 50));
    assert_eq!(of(BackgroundRepeat::RepeatX), RectI::new(0, 0, 100, 16));
    assert_eq!(of(BackgroundRepeat::RepeatY), RectI::new(0, 0, 16, 50));
    assert_eq!(of(BackgroundRepeat::NoRepeat), RectI::new(0, 0, 16, 16));
}

/// Chrome draws the picture of a background under the border.
#[test]
fn a_background_goes_between_its_color_and_its_border() {
    let mut ui = mount(|| {
        div().child(
            div()
                .w(100.0)
                .h(50.0)
                .border(2.0)
                .bg(rgb(0x00ff00))
                .bg_image(rgba(16, 16)),
        )
    });
    load(&mut ui);
    let scene = ui.paint(Instant::now());
    let order: Vec<&str> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) if q.border_widths == [0; 4] => Some("color"),
            Primitive::Quad(_) => Some("border"),
            Primitive::Image(_) => Some("picture"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["color", "picture", "border"]);
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn an_animation_goes_through_its_frames_on_timers_alone() {
    let mut ui = mount(|| div().child(img(gif(&[RED, BLUE], 50)).w(8.0).h(8.0)));
    load(&mut ui);
    assert_eq!(the_picture(&mut ui).frame, 0);
    assert!(!ui.animating(), "no frames on end between frames");
    let due = ui.next_deadline(Instant::now()).expect("the next frame");
    ui.tick(due);
    assert!(ui.needs_redraw());
    ui.update();
    assert_eq!(the_picture(&mut ui).frame, 1);
    let due = ui.next_deadline(due).expect("the next frame");
    ui.tick(due);
    ui.update();
    assert_eq!(the_picture(&mut ui).frame, 0);
}

#[test]
fn a_frame_changing_damages_only_the_picture() {
    let mut ui = mount(|| {
        div()
            .child(div().h(50.0).bg(rgb(0x123456)))
            .child(img(gif(&[RED, BLUE], 50)).w(8.0).h(8.0))
    });
    load(&mut ui);
    let a = ui.paint(Instant::now());
    let due = ui.next_deadline(Instant::now()).unwrap();
    ui.tick(due);
    ui.update();
    let b = ui.paint(Instant::now());
    assert_eq!(damage::diff(&a, &b).bounds(), RectI::new(0, 50, 8, 8));
}

#[test]
fn an_animation_out_of_view_stops_and_goes_on_when_it_comes_back() {
    let far = node_ref();
    let near = node_ref();
    let mut ui = mount(|| {
        div()
            .h_full()
            .overflow_y_scroll()
            .node_ref(far)
            .child(
                img(gif(&[RED, BLUE], 50))
                    .w(8.0)
                    .h(8.0)
                    .flex_none()
                    .node_ref(near),
            )
            .child(div().h(5000.0).flex_none())
    });
    load(&mut ui);
    assert!(ui.next_deadline(Instant::now()).is_some());
    far.scroll_to(0.0, 3000.0);
    ui.update();
    let now = Instant::now();
    let _ = ui.paint(now);
    assert_eq!(ui.next_deadline(now), None, "nothing to wake up for");
    far.scroll_to(0.0, 0.0);
    ui.update();
    let _ = ui.paint(now);
    assert!(ui.next_deadline(now).is_some());
}

/// Those that aren't for animating are shown like browsers do.
#[test]
fn delays_of_nothing_count_as_a_tenth_of_a_second() {
    let mut ui = mount(|| div().child(img(gif(&[RED, BLUE], 0)).w(8.0).h(8.0)));
    load(&mut ui);
    let now = Instant::now();
    let due = ui.next_deadline(now).unwrap();
    let wait = due.saturating_duration_since(now);
    assert!(wait > Duration::from_millis(50), "{wait:?}");
    assert!(wait <= Duration::from_millis(100), "{wait:?}");
}

#[test]
fn an_animation_that_plays_once_stops_on_its_last_frame() {
    let file = {
        let mut out = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut out);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Finite(1))
                .unwrap();
            for c in [RED, BLUE] {
                encoder
                    .encode_frame(Frame::from_parts(
                        RgbaImage::from_pixel(8, 8, image::Rgba(c)),
                        0,
                        0,
                        Delay::from_numer_denom_ms(50, 1),
                    ))
                    .unwrap();
            }
        }
        out
    };
    let mut ui = mount(|| div().child(img(file).w(8.0).h(8.0)));
    load(&mut ui);
    let mut now = Instant::now();
    for _ in 0..5 {
        match ui.next_deadline(now) {
            Some(due) => {
                ui.tick(due);
                ui.update();
                now = due;
            }
            None => break,
        }
    }
    assert_eq!(ui.next_deadline(now), None);
    assert_eq!(the_picture(&mut ui).frame, 1);
}
