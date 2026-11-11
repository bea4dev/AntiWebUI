//! Our own DXGI swapchain on Windows, so that `Present1` tells DWM which rectangles changed (see
//! "提示" in `docs/design/39-platform-windows.md`). wgpu's surface presents whole frames.
//!
//! Frames are drawn into a kept texture (`WindowRenderer::retained`); what changed is copied from
//! it into the back buffer. Buffers of a `FLIP_SEQUENTIAL` swapchain keep their pixels, so a buffer
//! last presented `k` frames ago gets the changes of those `k` frames (its buffer age).

use crate::{Gpu, Offscreen};
use anti_webui_core::damage::{Damage, DamageHistory};
use anti_webui_core::geometry::RectI;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wgpu::hal::api::Dx12;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Resource};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_PRESENT_PARAMETERS,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain3,
};
use windows::core::{IUnknown, Interface};

/// One shown, one DWM may still read, and one to draw: as many as wgpu's surface had.
const BUFFERS: u32 = 3;

thread_local! {
    /// The DirectComposition device of this thread's windows: one is enough for any number of
    /// windows, and making one for each popup would add to the wait for it.
    static DCOMP: RefCell<Option<IDCompositionDevice>> = const { RefCell::new(None) };
}

fn dcomp_device() -> windows::core::Result<IDCompositionDevice> {
    DCOMP.with(|d| {
        if let Some(device) = d.borrow().as_ref() {
            return Ok(device.clone());
        }
        // SAFETY: plain creation; no rendering device is needed to show a swapchain.
        let device: IDCompositionDevice = unsafe { DCompositionCreateDevice2(None::<&IUnknown>)? };
        *d.borrow_mut() = Some(device.clone());
        Ok(device)
    })
}

struct Buffer {
    texture: wgpu::Texture,
    /// The frame last copied into it; 0 if none.
    frame: u64,
}

/// A swapchain with its buffers as wgpu textures.
struct Chain {
    // The textures go before the swapchain whose buffers they are.
    buffers: Vec<Buffer>,
    swapchain: IDXGISwapChain3,
}

/// A swapchain on a DirectComposition visual of a window.
pub(crate) struct Swapchain {
    chain: Chain,
    /// Swapchains of earlier sizes, kept until the GPU is done with what was submitted before
    /// they were replaced.
    retired: Vec<(Chain, Arc<AtomicBool>)>,
    factory: IDXGIFactory2,
    queue: ID3D12CommandQueue,
    dcomp: IDCompositionDevice,
    visual: IDCompositionVisual,
    _target: IDCompositionTarget,
    format: wgpu::TextureFormat,
    alpha: DXGI_ALPHA_MODE,
    size: (u32, u32),
    /// Frames presented so far on `chain`.
    frame: u64,
    history: DamageHistory,
    last_age: Option<u64>,
}

fn fail(what: &'static str) -> impl FnOnce(windows::core::Error) -> String {
    move |e| format!("{what}: {e}")
}

fn dxgi_format(format: wgpu::TextureFormat) -> Option<DXGI_FORMAT> {
    match format {
        wgpu::TextureFormat::Bgra8Unorm => Some(DXGI_FORMAT_B8G8R8A8_UNORM),
        wgpu::TextureFormat::Rgba8Unorm => Some(DXGI_FORMAT_R8G8B8A8_UNORM),
        _ => None,
    }
}

impl Swapchain {
    /// Makes a swapchain of `size` and shows it in the window.
    ///
    /// # Safety
    /// The window must outlive the swapchain.
    pub(crate) unsafe fn new(
        gpu: &Gpu,
        hwnd: HWND,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        premultiplied: bool,
    ) -> Result<Self, String> {
        dxgi_format(format).ok_or("no DXGI format for the surface's")?;
        // SAFETY: only the queue is taken, which lives as long as the device.
        let queue = unsafe { gpu.device.as_hal::<Dx12>() }
            .ok_or("not on DX12")?
            .raw_queue()
            .clone();
        // SAFETY: plain creation.
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
            .map_err(fail("CreateDXGIFactory2"))?;
        let alpha = if premultiplied {
            DXGI_ALPHA_MODE_PREMULTIPLIED
        } else {
            DXGI_ALPHA_MODE_IGNORE
        };
        let chain = make_chain(gpu, &factory, &queue, format, alpha, size)?;
        let dcomp = dcomp_device().map_err(fail("DCompositionCreateDevice2"))?;
        // SAFETY: the caller keeps the window; the objects are kept with the swapchain.
        let (target, visual) = unsafe {
            let target = dcomp
                .CreateTargetForHwnd(hwnd, false)
                .map_err(fail("CreateTargetForHwnd"))?;
            let visual = dcomp.CreateVisual().map_err(fail("CreateVisual"))?;
            visual
                .SetContent(&chain.swapchain)
                .map_err(fail("SetContent"))?;
            target.SetRoot(&visual).map_err(fail("SetRoot"))?;
            dcomp.Commit().map_err(fail("Commit"))?;
            (target, visual)
        };
        Ok(Self {
            chain,
            retired: Vec::new(),
            factory,
            queue,
            dcomp,
            visual,
            _target: target,
            format,
            alpha,
            size,
            frame: 0,
            history: DamageHistory::default(),
            last_age: None,
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// How many frames old the last buffer copied into was; `None` if it held nothing.
    pub(crate) fn last_age(&self) -> Option<u64> {
        self.last_age
    }

    /// Shows a new swapchain of `size` in place of this one. `ResizeBuffers` needs every reference
    /// to the buffers gone first, so it would wait for the GPU to finish what uses them.
    pub(crate) fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) -> Result<(), String> {
        let chain = make_chain(
            gpu,
            &self.factory,
            &self.queue,
            self.format,
            self.alpha,
            size,
        )?;
        // SAFETY: the visual and the swapchain are ours.
        unsafe {
            self.visual
                .SetContent(&chain.swapchain)
                .map_err(fail("SetContent"))?;
            self.dcomp.Commit().map_err(fail("Commit"))?;
        }
        let old = std::mem::replace(&mut self.chain, chain);
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        gpu.queue
            .on_submitted_work_done(move || flag.store(true, Ordering::Release));
        self.retired.push((old, done));
        self.size = size;
        self.frame = 0;
        Ok(())
    }

    /// Copies what changed from the frame in `kept` (of `extent`, at the top left) into the next
    /// buffer, submits `encoder` with that, and presents the buffer with `damage` as what changed
    /// since the last frame. `whole` says that the last frame can't be built on: everything is
    /// copied and presented as changed.
    pub(crate) fn present(
        &mut self,
        gpu: &Gpu,
        mut encoder: wgpu::CommandEncoder,
        kept: &Offscreen,
        extent: (u32, u32),
        damage: &Damage,
        whole: bool,
    ) -> Result<(), String> {
        self.retired
            .retain(|(_, done)| !done.load(Ordering::Acquire));
        let bounds = RectI::new(0, 0, extent.0 as i32, extent.1 as i32);
        let whole = whole || self.frame == 0;
        if whole {
            for b in &mut self.chain.buffers {
                b.frame = 0;
            }
            self.history.clear();
        }
        let frame = self.frame + 1;
        // SAFETY: plain query.
        let index = unsafe { self.chain.swapchain.GetCurrentBackBufferIndex() } as usize;
        let buffer = self
            .chain
            .buffers
            .get_mut(index)
            .ok_or("no such back buffer")?;
        let age = (buffer.frame > 0).then(|| frame - buffer.frame);
        let mut copy = match age {
            Some(1) => Some(damage.clone()),
            Some(age) => self.history.last(age as usize - 1).map(|mut d| {
                d.extend(damage);
                d
            }),
            None => None,
        }
        .unwrap_or_else(|| Damage::full(extent.0, extent.1));
        copy.clip(bounds);
        for r in copy.rects() {
            let origin = wgpu::Origin3d {
                x: r.x as u32,
                y: r.y as u32,
                z: 0,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &kept.texture,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &buffer.texture,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: r.width as u32,
                    height: r.height as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &buffer.texture,
                selector: None,
                state: wgpu::TextureUses::PRESENT,
            }),
        );
        gpu.queue.submit([encoder.finish()]);
        buffer.frame = frame;
        self.frame = frame;
        self.last_age = age;
        self.history.push(damage.clone());

        // No rectangles means the whole buffer.
        let mut dirty: Vec<RECT> = Vec::new();
        if !whole {
            let mut changed = damage.clone();
            changed.clip(bounds);
            dirty.extend(changed.rects().iter().map(|r| RECT {
                left: r.x,
                top: r.y,
                right: r.x + r.width,
                bottom: r.y + r.height,
            }));
        }
        let params = DXGI_PRESENT_PARAMETERS {
            DirtyRectsCount: dirty.len() as u32,
            pDirtyRects: if dirty.is_empty() {
                std::ptr::null_mut()
            } else {
                dirty.as_mut_ptr()
            },
            pScrollRect: std::ptr::null_mut(),
            pScrollOffset: std::ptr::null_mut(),
        };
        // SAFETY: the copies into the buffer were submitted on the swapchain's queue just before;
        // `dirty` outlives the call. Interval 0: DWM shows the latest at its next composition, and
        // we pace ourselves with the vertical blank.
        unsafe { self.chain.swapchain.Present1(0, DXGI_PRESENT(0), &params) }
            .ok()
            .map_err(fail("Present1"))
    }
}

/// A swapchain for composition, with its buffers wrapped for wgpu to copy into.
fn make_chain(
    gpu: &Gpu,
    factory: &IDXGIFactory2,
    queue: &ID3D12CommandQueue,
    format: wgpu::TextureFormat,
    alpha: DXGI_ALPHA_MODE,
    size: (u32, u32),
) -> Result<Chain, String> {
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: size.0,
        Height: size.1,
        Format: dxgi_format(format).ok_or("no DXGI format for the surface's")?,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: BUFFERS,
        Scaling: DXGI_SCALING_STRETCH,
        // Keeps each buffer's pixels between presents, which buffer age needs.
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: alpha,
        Flags: 0,
    };
    // SAFETY: the queue is the one wgpu submits to; `desc` is filled in above.
    let swapchain: IDXGISwapChain3 =
        unsafe { factory.CreateSwapChainForComposition(queue, &desc, None) }
            .map_err(fail("CreateSwapChainForComposition"))?
            .cast()
            .map_err(fail("IDXGISwapChain3"))?;
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let mut buffers = Vec::with_capacity(BUFFERS as usize);
    for i in 0..BUFFERS {
        // SAFETY: the swapchain has `BUFFERS` buffers.
        let resource: ID3D12Resource =
            unsafe { swapchain.GetBuffer(i) }.map_err(fail("GetBuffer"))?;
        // SAFETY: the resource is a 2D texture of this size and format, with one mip level and one
        // sample, in the state `PRESENT` until it is first presented.
        let texture = unsafe {
            let hal = wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                format,
                wgpu::TextureDimension::D2,
                extent,
                1,
                1,
            );
            gpu.device.create_texture_from_hal::<Dx12>(
                hal,
                &wgpu::TextureDescriptor {
                    label: Some("swapchain"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::TextureUses::PRESENT,
            )
        };
        buffers.push(Buffer { texture, frame: 0 });
    }
    Ok(Chain { buffers, swapchain })
}
