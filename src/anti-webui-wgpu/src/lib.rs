//! The wgpu renderer for AntiWebUI scenes.

mod custom_shader;
#[cfg(target_os = "linux")]
pub mod dmabuf;
mod effects;
mod glyph_cache;
#[cfg(target_os = "linux")]
pub mod linux_sync;
mod renderer;

use anti_webui_core::scene::Scene;
pub use custom_shader::check_shader;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, RawWindowHandle,
};
pub use renderer::Renderer;
pub use wgpu;

#[derive(Debug)]
pub enum GpuError {
    Surface(wgpu::CreateSurfaceError),
    Adapter(wgpu::RequestAdapterError),
    Device(wgpu::RequestDeviceError),
    /// Our own display buffers are unavailable; use a surface instead.
    Dmabuf(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::Surface(e) => write!(f, "failed to create surface: {e}"),
            GpuError::Adapter(e) => write!(f, "no suitable GPU adapter: {e}"),
            GpuError::Device(e) => write!(f, "failed to create GPU device: {e}"),
            GpuError::Dmabuf(e) => write!(f, "dmabuf presentation unavailable: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// A display connection handed to wgpu (needed by the GL backend on Wayland).
#[derive(Debug)]
struct Display(RawDisplayHandle);

// SAFETY: the handle is only an identifier; the platform keeps the connection alive for the
// lifetime of the GPU context.
unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl HasDisplayHandle for Display {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: see above.
        Ok(unsafe { DisplayHandle::borrow_raw(self.0) })
    }
}

/// Shared GPU objects.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// A window surface and its renderer.
pub struct WindowRenderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

impl Gpu {
    /// Creates the GPU context together with the first window's surface.
    ///
    /// # Safety
    /// The display and window handles must stay valid until the returned objects are dropped.
    pub unsafe fn with_window(
        display: RawDisplayHandle,
        window: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<(Gpu, WindowRenderer), GpuError> {
        // Try the native APIs first and fall back to GL only if they have no adapter: GL drivers
        // are much heavier to load (Mesa's GL pulls in LLVM, for example).
        let explicit = std::env::var_os("WGPU_BACKEND").is_some();
        let attempts: &[wgpu::Backends] = if explicit {
            &[wgpu::Backends::all()]
        } else {
            &[wgpu::Backends::PRIMARY, wgpu::Backends::GL]
        };
        let mut last_error = None;
        let mut found = None;
        for &backends in attempts {
            let mut desc = wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
                Display(display),
            ));
            if !explicit {
                desc.backends = backends;
            }
            let instance = wgpu::Instance::new(desc);
            // SAFETY: forwarded to the caller.
            let surface = unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(display),
                    raw_window_handle: window,
                })
            };
            let surface = match surface {
                Ok(s) => s,
                Err(e) => {
                    last_error = Some(GpuError::Surface(e));
                    continue;
                }
            };
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                }));
            match adapter {
                Ok(adapter) => {
                    found = Some((instance, surface, adapter));
                    break;
                }
                Err(e) => last_error = Some(GpuError::Adapter(e)),
            }
        }
        let Some((instance, surface, adapter)) = found else {
            return Err(last_error.expect("at least one backend was tried"));
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("anti-webui"),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(GpuError::Device)?;
        let gpu = Gpu {
            instance,
            adapter,
            device,
            queue,
        };
        let window = gpu.configure_window(surface, width, height);
        Ok((gpu, window))
    }

    /// Creates a surface for another window, like a popup, with a renderer of its own.
    ///
    /// # Safety
    /// The display and window handles must stay valid until the returned object is dropped.
    pub unsafe fn add_window(
        &self,
        display: RawDisplayHandle,
        window: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<WindowRenderer, GpuError> {
        // SAFETY: forwarded to the caller.
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(display),
                    raw_window_handle: window,
                })
        }
        .map_err(GpuError::Surface)?;
        Ok(self.configure_window(surface, width, height))
    }

    /// Creates a GPU context without a window, for offscreen rendering.
    pub fn headless() -> Result<Gpu, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(GpuError::Adapter)?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(GpuError::Device)?;
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
        })
    }

    fn configure_window(
        &self,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> WindowRenderer {
        let caps = surface.get_capabilities(&self.adapter);
        // Non-sRGB formats blend in gamma space, like browsers do.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .unwrap_or(caps.formats[0]);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        // Popups have transparent corners and shadows; the window says it is opaque by other means.
        let alpha_mode = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Opaque,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m))
        .unwrap_or(caps.alpha_modes[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&self.device, &config);
        WindowRenderer {
            surface,
            config,
            renderer: Renderer::new(&self.device, format),
        }
    }

    /// Renders a scene offscreen and returns tightly packed RGBA8 pixels.
    pub fn render_to_rgba(&self, renderer: &mut Renderer, scene: &Scene) -> Vec<u8> {
        let target = Offscreen::new(self, renderer.format(), scene.width, scene.height);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        renderer.render(&self.device, &self.queue, &mut encoder, &target.view, scene);
        self.queue.submit([encoder.finish()]);
        target.read_rgba(self)
    }
}

/// An offscreen render target that keeps its pixels between frames.
pub struct Offscreen {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Offscreen {
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }

    /// Reads the pixels back as tightly packed RGBA8.
    pub fn read_rgba(&self, gpu: &Gpu) -> Vec<u8> {
        let (w, h) = (self.texture.width(), self.texture.height());
        let padded = (w * 4).div_ceil(256) * 256;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("readback buffer is mapped");
        let bgra = self.texture.format() == wgpu::TextureFormat::Bgra8Unorm;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let start = (row * padded) as usize;
            for px in data[start..start + (w * 4) as usize].chunks_exact(4) {
                if bgra {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                } else {
                    out.extend_from_slice(px);
                }
            }
        }
        out
    }
}

/// The outcome of presenting a frame.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameResult {
    Presented,
    /// Nothing was presented; try again on the next frame.
    Skipped,
}

impl WindowRenderer {
    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if self.config.width != w || self.config.height != h {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&gpu.device, &self.config);
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn renderer(&mut self) -> &mut Renderer {
        &mut self.renderer
    }

    /// Renders and presents a scene.
    pub fn present(&mut self, gpu: &Gpu, scene: &Scene) -> FrameResult {
        self.resize(gpu, scene.width, scene.height);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&gpu.device, &self.config);
                return FrameResult::Skipped;
            }
            _ => return FrameResult::Skipped,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer
            .render(&gpu.device, &gpu.queue, &mut encoder, &view, scene);
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(frame);
        FrameResult::Presented
    }
}
