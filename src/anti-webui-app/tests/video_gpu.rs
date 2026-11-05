//! From a decoder on the GPU to the pixels without a copy. On what can't do that there is nothing to
//! see here. This is alone in its process since it makes a GPU instance and lets it go, which the
//! Vulkan loader doesn't take well next to others coming up. See `docs/design/24-video.md`.
#![cfg(target_os = "linux")]

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_wgpu::wgpu;
use std::cell::RefCell;
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;
use std::time::{Duration, Instant};

const W: u32 = 64;
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

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

fn data(name: &str) -> String {
    format!(
        "{}/../anti-webui-video/tests/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
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

#[test]
fn frames_are_read_where_the_decoder_left_them() {
    if !real() {
        return;
    }
    let node = "/dev/dri/renderD128";
    let Ok(meta) = std::fs::metadata(node) else {
        eprintln!("skipped: no {node}");
        return;
    };
    let Ok((gpu, ctx)) = anti_webui_wgpu::Gpu::for_dmabuf(meta.rdev()) else {
        eprintln!("skipped: no dmabufs on {node}");
        return;
    };
    let mut renderer = anti_webui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    renderer.set_video_importer(ctx.video_importer().unwrap());

    let player = Rc::new(RefCell::new(None));
    let p = player.clone();
    let source = data("silent.webm");
    let mut ui = Ui::new(move || {
        let handle = video_player();
        *p.borrow_mut() = Some(handle);
        video(source).player(handle).w(64.0).h(64.0)
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(W as i32, W as i32),
        scale: Scale::from_f64(1.0),
    });
    ui.set_video_render_node(Some(node.into()));
    ui.update();
    assert!(ui.wait_for_images(Duration::from_secs(10)));
    let draw = |ui: &mut Ui, renderer: &mut anti_webui_wgpu::Renderer| {
        let now = Instant::now();
        ui.animate(now);
        ui.update();
        let scene = ui.paint(now);
        gpu.render_to_rgba(renderer, &scene)
    };
    let pixels = draw(&mut ui, &mut renderer);
    let (streams, imports) = renderer.video_counts();
    if renderer.take_video_import_failed() || imports == 0 {
        // This GPU's decoder doesn't do this: the memory path is what the others are about.
        eprintln!("skipped: no frames were read from the GPU ({streams} streams)");
        return;
    }
    close(pixel(&pixels, 32, 32), RED, 8);

    // What was read is what the file has, and frames go by without running out of pictures.
    let player = player.borrow().unwrap();
    player.seek(1.5);
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let pixels = draw(&mut ui, &mut renderer);
        if pixel(&pixels, 32, 32)[2] > 200 {
            close(pixel(&pixels, 32, 32), BLUE, 12);
            break;
        }
        assert!(Instant::now() < end, "blue didn't come");
        std::thread::sleep(Duration::from_millis(2));
    }
    player.seek(0.0);
    player.play();
    let before = renderer.video_counts().1;
    let end = Instant::now() + Duration::from_secs(5);
    while !player.ended() && Instant::now() < end {
        draw(&mut ui, &mut renderer);
        ui.poll_images();
        std::thread::sleep(Duration::from_millis(8));
    }
    assert!(player.ended(), "it played to the end");
    let read = renderer.video_counts().1 - before;
    assert!(read >= 30, "only {read} frames");
    assert!(!renderer.take_video_import_failed());
}
