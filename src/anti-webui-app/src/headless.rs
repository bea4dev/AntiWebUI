//! Offscreen rendering for snapshot tests and screenshots.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::view::IntoView;
use anti_webui_wgpu::{Gpu, Renderer};
use std::path::Path;
use std::time::Instant;

/// Renders `view` at the given physical size and scale and returns RGBA8 pixels.
pub fn render_rgba<V: IntoView>(
    width: u32,
    height: u32,
    scale: f64,
    view: impl FnOnce() -> V,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    render_driven_rgba(width, height, scale, view, |_| {})
}

/// Like [`render_rgba`], but lets `drive` feed events to the UI before the frame is painted.
pub fn render_driven_rgba<V: IntoView>(
    width: u32,
    height: u32,
    scale: f64,
    view: impl FnOnce() -> V,
    drive: impl FnOnce(&mut Ui),
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let gpu = Gpu::headless()?;
    let mut renderer = Renderer::new(
        &gpu.device,
        anti_webui_wgpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(width as i32, height as i32),
        scale: Scale::from_f64(scale),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    drive(&mut ui);
    ui.update();
    let scene = ui.paint(Instant::now());
    Ok(gpu.render_to_rgba(&mut renderer, &scene))
}

/// Writes RGBA8 pixels as a PNG file.
pub fn write_png(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Renders `view` into a PNG file.
pub fn render_png<V: IntoView>(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    scale: f64,
    view: impl FnOnce() -> V,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = render_rgba(width, height, scale, view)?;
    write_png(path, width, height, &pixels)
}
