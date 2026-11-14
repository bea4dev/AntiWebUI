//! Our own display buffers on macOS: `IOSurface`s that Metal draws to and Core Animation shows (see
//! "提示" in `docs/design/38-platform-macos.md`).

use crate::{Gpu, GpuError, video_features};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
pub use objc2_io_surface::IOSurfaceRef;
use objc2_io_surface::{
    kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture,
    MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use wgpu::hal::api::Metal;

/// What buffers are drawn as: `Bgra8Unorm`, which Core Animation shows as it is.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// `'BGRA'`, the pixel format of `kCVPixelFormatType_32BGRA`.
const PIXEL_FORMAT: i32 = i32::from_be_bytes(*b"BGRA");

/// An `IOSurface`, and the texture that draws to it.
pub struct IoSurfaceImage {
    pub surface: CFRetained<IOSurfaceRef>,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Gpu {
    /// A GPU context of its own, for drawing to `IOSurface`s on Metal.
    pub fn for_iosurface() -> Result<Gpu, GpuError> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = wgpu::Backends::METAL;
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(GpuError::Adapter)?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("anti-webui"),
            required_features: video_features(&adapter),
            required_limits: crate::device_limits(&adapter),
            ..Default::default()
        }))
        .map_err(GpuError::Device)?;
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
            shader_cache: None,
        })
    }

    /// Makes an `IOSurface` of `width` × `height` and a texture of the GPU on it.
    pub fn allocate_iosurface(&self, width: u32, height: u32) -> Result<IoSurfaceImage, GpuError> {
        let fail = |what: &str| GpuError::IoSurface(what.into());
        let (w, h) = (width.max(1), height.max(1));
        let number = |n: i32| CFNumber::new_i32(n);
        let (wn, hn, bpe, pf) = (
            number(w as i32),
            number(h as i32),
            number(4),
            number(PIXEL_FORMAT),
        );
        // SAFETY: the keys are CF strings that live for the process.
        let keys: [&CFString; 4] = unsafe {
            [
                kIOSurfaceWidth,
                kIOSurfaceHeight,
                kIOSurfaceBytesPerElement,
                kIOSurfacePixelFormat,
            ]
        };
        let values: [&CFType; 4] = [&wn, &hn, &bpe, &pf];
        let props = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        // SAFETY: the dictionary has the keys `IOSurfaceCreate` takes, with values of their types.
        let surface = unsafe { IOSurfaceRef::new(props.as_opaque()) }
            .ok_or_else(|| fail("IOSurfaceCreate failed"))?;

        // SAFETY: the device is wgpu's own, and only used for what it says.
        let hal = unsafe { self.device.as_hal::<Metal>() }.ok_or_else(|| fail("not on Metal"))?;
        let device = hal.raw_device();
        let desc = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                w as usize,
                h as usize,
                false,
            )
        };
        desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        desc.setStorageMode(if device.hasUnifiedMemory() {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        });
        let raw: Retained<ProtocolObject<dyn MTLTexture>> = device
            .newTextureWithDescriptor_iosurface_plane(&desc, &surface, 0)
            .ok_or_else(|| fail("no texture on the IOSurface"))?;
        drop(hal);

        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        // SAFETY: the texture was made on this device with what the descriptor says.
        let texture = unsafe {
            let hal_texture = wgpu::hal::metal::Device::texture_from_raw(
                raw,
                FORMAT,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: w,
                    height: h,
                    depth: 1,
                },
                None,
            );
            self.device.create_texture_from_hal::<Metal>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("iosurface"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        let view = texture.create_view(&Default::default());
        Ok(IoSurfaceImage {
            surface,
            texture,
            view,
        })
    }

    /// Waits until what was submitted so far is scheduled on the GPU. After that, what Core
    /// Animation does with the `IOSurface`s comes after it, as Chromium relies on too.
    pub fn wait_until_scheduled(&self) {
        // SAFETY: an empty command buffer on wgpu's queue, which runs after what is on it.
        let Some(hal) = (unsafe { self.queue.as_hal::<Metal>() }) else {
            return;
        };
        if let Some(buffer) = hal.as_raw().commandBuffer() {
            buffer.commit();
            buffer.waitUntilScheduled();
        }
    }
}
