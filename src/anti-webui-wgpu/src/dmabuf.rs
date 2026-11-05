//! Display buffers that we allocate ourselves and share with the compositor as dmabufs (Linux,
//! Vulkan). See `docs/design/06-platform-wayland.md`.
//!
//! Each buffer is a `VkImage` with an explicit DRM format modifier whose memory is exported as
//! a dmabuf, wrapped as a `wgpu::Texture` for rendering. Around each frame the image's queue
//! family ownership moves between our queue and `VK_QUEUE_FAMILY_FOREIGN_EXT` (the compositor),
//! which is also where drivers resolve compression for external readers.

use crate::{Gpu, GpuError};
use ash::{ext, khr, vk};
use std::ffi::CStr;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Duration;
use wgpu::hal::api::Vulkan;

/// `DRM_FORMAT_ARGB8888`: B, G, R, A bytes in memory, which is Vulkan's `B8G8R8A8_UNORM`.
pub const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// "No explicit modifier"; we cannot allocate for it.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const VK_FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;
const WGPU_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

const REQUIRED_EXTENSIONS: [&CStr; 6] = [
    khr::external_memory_fd::NAME,
    ext::external_memory_dma_buf::NAME,
    ext::image_drm_format_modifier::NAME,
    khr::external_semaphore_fd::NAME,
    ext::queue_family_foreign::NAME,
    ext::physical_device_drm::NAME,
];

fn vk_err(what: &str) -> impl Fn(vk::Result) -> GpuError + '_ {
    move |e| GpuError::Dmabuf(format!("{what}: {e}"))
}

/// Linux `dev_t` encoding (glibc).
fn makedev(major: i64, minor: i64) -> u64 {
    let (major, minor) = (major as u64, minor as u64);
    ((major & 0xffff_f000) << 32)
        | ((major & 0xfff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0xff)
}

/// One plane of a dmabuf.
#[derive(Clone, Copy, Debug)]
pub struct DmabufPlane {
    pub offset: u32,
    pub stride: u32,
}

#[derive(Clone, Copy, Debug)]
struct ModifierInfo {
    modifier: u64,
    planes: u32,
    max_extent: (u32, u32),
}

/// Vulkan objects shared by all dmabuf images of a device.
pub struct DmabufContext {
    instance: ash::Instance,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family: u32,
    memory_fd: khr::external_memory_fd::Device,
    semaphore_fd: khr::external_semaphore_fd::Device,
    drm_modifier: ext::image_drm_format_modifier::Device,
    pool: vk::CommandPool,
    modifiers: Vec<ModifierInfo>,
    render_node: Option<PathBuf>,
    /// Images dropped while the GPU may still use them, with the fence that says when it is done.
    graveyard: Vec<Retired>,
}

struct Retired {
    _texture: wgpu::Texture,
    commands: [vk::CommandBuffer; 2],
    semaphores: [vk::Semaphore; 2],
    fence: vk::Fence,
}

/// A display buffer: a wgpu texture whose memory is a dmabuf.
pub struct DmabufImage {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// The dmabuf. Keep it open for implicit-sync fence operations.
    pub fd: OwnedFd,
    pub planes: Vec<DmabufPlane>,
    pub modifier: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    acquire: vk::CommandBuffer,
    release: vk::CommandBuffer,
    wait_semaphore: vk::Semaphore,
    signal_semaphore: vk::Semaphore,
    /// Signaled when the last release submission completed.
    fence: vk::Fence,
    /// Never handed to the compositor: still in the layout wgpu created it in.
    fresh: bool,
}

impl Gpu {
    /// Creates a Vulkan GPU context on the device the compositor uses (`main_device` is its
    /// `dev_t`), with the extensions needed for dmabuf presentation.
    pub fn for_dmabuf(main_device: u64) -> Result<(Gpu, DmabufContext), GpuError> {
        if std::env::var("WGPU_BACKEND").is_ok_and(|b| !b.to_lowercase().contains("vulkan")) {
            return Err(GpuError::Dmabuf("WGPU_BACKEND excludes Vulkan".into()));
        }
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
        for adapter in adapters {
            // SAFETY: only used to inspect and open the adapter.
            let Some(open) = (unsafe { open_for_dmabuf(&adapter, main_device) }) else {
                continue;
            };
            let open = open?;
            let limits =
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
            // SAFETY: `open` was created from this adapter with no extra features.
            let (device, queue) = unsafe {
                adapter.create_device_from_hal(
                    open,
                    &wgpu::DeviceDescriptor {
                        label: Some("anti-webui"),
                        required_features: crate::video_features(&adapter),
                        required_limits: limits,
                        ..Default::default()
                    },
                )
            }
            .map_err(GpuError::Device)?;
            let gpu = Gpu {
                instance,
                adapter,
                device,
                queue,
            };
            let ctx = DmabufContext::new(&gpu)?;
            return Ok((gpu, ctx));
        }
        Err(GpuError::Dmabuf(
            "no Vulkan adapter on the compositor's device with dmabuf support".into(),
        ))
    }
}

/// Opens `adapter` if it is `main_device` and supports what we need; `None` if it does not.
unsafe fn open_for_dmabuf(
    adapter: &wgpu::Adapter,
    main_device: u64,
) -> Option<Result<wgpu::hal::OpenDevice<Vulkan>, GpuError>> {
    // SAFETY: the guard is dropped before the adapter is used through wgpu again.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }?;
    let caps = hal.physical_device_capabilities();
    if !REQUIRED_EXTENSIONS
        .iter()
        .all(|e| caps.supports_extension(e))
    {
        return None;
    }
    let raw_instance = hal.shared_instance().raw_instance();
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    // SAFETY: the physical device belongs to this instance and supports the DRM extension.
    unsafe { raw_instance.get_physical_device_properties2(hal.raw_physical_device(), &mut props) };
    let matches = (drm.has_primary != 0
        && makedev(drm.primary_major, drm.primary_minor) == main_device)
        || (drm.has_render != 0 && makedev(drm.render_major, drm.render_minor) == main_device);
    if !matches {
        return None;
    }
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    let callback: Box<wgpu::hal::vulkan::CreateDeviceCallback<'_>> = Box::new(|args| {
        for e in [
            khr::external_semaphore_fd::NAME,
            ext::queue_family_foreign::NAME,
        ] {
            if !args.extensions.contains(&e) {
                args.extensions.push(e);
            }
        }
    });
    // SAFETY: the extensions were checked above; no features are removed.
    let open = unsafe {
        hal.open_with_callback(
            crate::video_features(adapter),
            &limits,
            &wgpu::MemoryHints::default(),
            Some(callback),
        )
    };
    Some(open.map_err(|e| GpuError::Dmabuf(format!("opening the device: {e}"))))
}

impl DmabufContext {
    fn new(gpu: &Gpu) -> Result<Self, GpuError> {
        // SAFETY: the raw handles are only used while `gpu` (and so the device) is alive; the
        // owner drops this context first.
        let hal = unsafe { gpu.device.as_hal::<Vulkan>() }
            .ok_or_else(|| GpuError::Dmabuf("not a Vulkan device".into()))?;
        let instance = hal.shared_instance().raw_instance().clone();
        let device = hal.raw_device().clone();
        let physical_device = hal.raw_physical_device();
        let queue_family = hal.queue_family_index();
        let queue = hal.raw_queue();
        drop(hal);

        let memory_fd = khr::external_memory_fd::Device::new(&instance, &device);
        let semaphore_fd = khr::external_semaphore_fd::Device::new(&instance, &device);
        let drm_modifier = ext::image_drm_format_modifier::Device::new(&instance, &device);

        // Sync files must be both exportable (our rendering) and importable (compositor reads).
        let mut sem_props = vk::ExternalSemaphoreProperties::default();
        // SAFETY: valid physical device and output structure.
        unsafe {
            instance.get_physical_device_external_semaphore_properties(
                physical_device,
                &vk::PhysicalDeviceExternalSemaphoreInfo::default()
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
                &mut sem_props,
            )
        };
        let needed = vk::ExternalSemaphoreFeatureFlags::EXPORTABLE
            | vk::ExternalSemaphoreFeatureFlags::IMPORTABLE;
        if !sem_props.external_semaphore_features.contains(needed) {
            return Err(GpuError::Dmabuf("sync file semaphores unsupported".into()));
        }

        let modifiers = query_modifiers(&instance, physical_device);
        if modifiers.is_empty() {
            return Err(GpuError::Dmabuf("no exportable DRM format modifier".into()));
        }

        let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
        // SAFETY: as above.
        unsafe { instance.get_physical_device_properties2(physical_device, &mut props) };
        let render_node = (drm.has_render != 0)
            .then(|| find_device_node(makedev(drm.render_major, drm.render_minor)))
            .flatten();

        // SAFETY: valid device and queue family.
        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }
        .map_err(vk_err("command pool"))?;

        Ok(Self {
            instance,
            device,
            physical_device,
            queue,
            queue_family,
            memory_fd,
            semaphore_fd,
            drm_modifier,
            pool,
            modifiers,
            render_node,
            graveyard: Vec::new(),
        })
    }

    /// The DRM render node of the device, for syncobj operations.
    pub fn render_node(&self) -> Option<&std::path::Path> {
        self.render_node.as_deref()
    }

    /// Whether we can render to `ARGB8888` with any of `modifiers`.
    pub fn supports_any(&self, modifiers: &[u64]) -> bool {
        self.modifiers
            .iter()
            .any(|m| modifiers.contains(&m.modifier))
    }

    /// Allocates an `ARGB8888` image; the driver picks one of `modifiers` (the compositor's).
    pub fn allocate(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        modifiers: &[u64],
    ) -> Result<DmabufImage, GpuError> {
        self.reap();
        let candidates: Vec<u64> = self
            .modifiers
            .iter()
            .filter(|m| {
                modifiers.contains(&m.modifier)
                    && m.max_extent.0 >= width
                    && m.max_extent.1 >= height
            })
            .map(|m| m.modifier)
            .collect();
        if candidates.is_empty() {
            return Err(GpuError::Dmabuf(
                "no modifier shared with the compositor".into(),
            ));
        }
        let device = &self.device;
        let mut modifier_list = vk::ImageDrmFormatModifierListCreateInfoEXT::default()
            .drm_format_modifiers(&candidates);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(VK_FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external)
            .push_next(&mut modifier_list);
        // SAFETY: valid create info; destroyed on failure below or by the texture's drop.
        let image = unsafe { device.create_image(&info, None) }.map_err(vk_err("image"))?;

        let memory = match self.bind_exportable_memory(image) {
            Ok(m) => m,
            Err(e) => {
                // SAFETY: the image is unused.
                unsafe { device.destroy_image(image, None) };
                return Err(e);
            }
        };
        let destroy = |e: GpuError| {
            // SAFETY: the image and memory are unused.
            unsafe {
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
            e
        };

        // SAFETY: the memory was allocated exportable as a dmabuf.
        let fd = unsafe {
            self.memory_fd.get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
        }
        .map_err(vk_err("exporting memory"))
        .map_err(destroy)?;
        // SAFETY: the driver returned a new file descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let mut mod_props = vk::ImageDrmFormatModifierPropertiesEXT::default();
        // SAFETY: the image was created with DRM_FORMAT_MODIFIER tiling.
        unsafe {
            self.drm_modifier
                .get_image_drm_format_modifier_properties(image, &mut mod_props)
        }
        .map_err(vk_err("querying the modifier"))
        .map_err(destroy)?;
        let modifier = mod_props.drm_format_modifier;
        let plane_count = self
            .modifiers
            .iter()
            .find(|m| m.modifier == modifier)
            .map_or(1, |m| m.planes);
        let aspects = [
            vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_3_EXT,
        ];
        let planes = aspects[..plane_count.min(4) as usize]
            .iter()
            .map(|&aspect_mask| {
                // SAFETY: valid image and memory plane aspect for its modifier.
                let layout = unsafe {
                    device.get_image_subresource_layout(
                        image,
                        vk::ImageSubresource {
                            aspect_mask,
                            mip_level: 0,
                            array_layer: 0,
                        },
                    )
                };
                DmabufPlane {
                    offset: layout.offset as u32,
                    stride: layout.row_pitch as u32,
                }
            })
            .collect();

        let (acquire, release, wait_semaphore, signal_semaphore, fence) =
            self.create_sync_objects(image).map_err(destroy)?;

        let texture = self.wrap_texture(gpu, image, memory, width, height);
        let view = texture.create_view(&Default::default());
        Ok(DmabufImage {
            texture,
            view,
            fd,
            planes,
            modifier,
            format: DRM_FORMAT_ARGB8888,
            width,
            height,
            acquire,
            release,
            wait_semaphore,
            signal_semaphore,
            fence,
            fresh: true,
        })
    }

    fn bind_exportable_memory(&self, image: vk::Image) -> Result<vk::DeviceMemory, GpuError> {
        let device = &self.device;
        // SAFETY: valid image.
        let req = unsafe { device.get_image_memory_requirements(image) };
        // SAFETY: valid physical device.
        let mem_props = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical_device)
        };
        let type_index = mem_props
            .memory_types_as_slice()
            .iter()
            .enumerate()
            .position(|(i, t)| {
                req.memory_type_bits & (1 << i) != 0
                    && t.property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .or_else(|| (0..32).position(|i| req.memory_type_bits & (1 << i) != 0))
            .ok_or_else(|| GpuError::Dmabuf("no memory type for the image".into()))?;
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(type_index as u32)
            .push_next(&mut export)
            .push_next(&mut dedicated);
        // SAFETY: valid allocation info.
        let memory = unsafe { device.allocate_memory(&alloc, None) }.map_err(vk_err("memory"))?;
        // SAFETY: the memory satisfies the image's requirements.
        if let Err(e) = unsafe { device.bind_image_memory(image, memory, 0) } {
            // SAFETY: unused memory.
            unsafe { device.free_memory(memory, None) };
            return Err(vk_err("binding memory")(e));
        }
        Ok(memory)
    }

    #[allow(clippy::type_complexity)]
    fn create_sync_objects(
        &self,
        image: vk::Image,
    ) -> Result<
        (
            vk::CommandBuffer,
            vk::CommandBuffer,
            vk::Semaphore,
            vk::Semaphore,
            vk::Fence,
        ),
        GpuError,
    > {
        let device = &self.device;
        // SAFETY: valid pool.
        let cmds = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(2),
            )
        }
        .map_err(vk_err("command buffers"))?;
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        // Back from the compositor: contents are kept (no UNDEFINED), ready to render.
        let acquire = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .dst_queue_family_index(self.queue_family)
            .image(image)
            .subresource_range(range);
        // To the compositor, from the layout wgpu leaves render targets in.
        let release = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(self.queue_family)
            .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .image(image)
            .subresource_range(range);
        let record = |cmd: vk::CommandBuffer,
                      barrier: vk::ImageMemoryBarrier<'_>,
                      src: vk::PipelineStageFlags,
                      dst: vk::PipelineStageFlags|
         -> Result<(), GpuError> {
            // SAFETY: freshly allocated command buffer, recorded once.
            unsafe {
                device
                    .begin_command_buffer(
                        cmd,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::SIMULTANEOUS_USE),
                    )
                    .map_err(vk_err("recording"))?;
                device.cmd_pipeline_barrier(
                    cmd,
                    src,
                    dst,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
                device.end_command_buffer(cmd).map_err(vk_err("recording"))
            }
        };
        let result = (|| {
            record(
                cmds[0],
                acquire,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            )?;
            record(
                cmds[1],
                release,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            )?;
            let mut export = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            // SAFETY: valid create infos.
            unsafe {
                let wait = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(vk_err("semaphore"))?;
                let signal = device
                    .create_semaphore(
                        &vk::SemaphoreCreateInfo::default().push_next(&mut export),
                        None,
                    )
                    .map_err(vk_err("semaphore"))?;
                let fence = device
                    .create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                    .map_err(vk_err("fence"))?;
                Ok((wait, signal, fence))
            }
        })();
        match result {
            Ok((wait, signal, fence)) => Ok((cmds[0], cmds[1], wait, signal, fence)),
            Err(e) => {
                // SAFETY: never submitted.
                unsafe { device.free_command_buffers(self.pool, &cmds) };
                Err(e)
            }
        }
    }

    fn wrap_texture(
        &self,
        gpu: &Gpu,
        image: vk::Image,
        memory: vk::DeviceMemory,
        width: u32,
        height: u32,
    ) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let device = self.device.clone();
        let drop_callback: wgpu::hal::DropCallback = Box::new(move || {
            // SAFETY: wgpu calls this once the texture is no longer used; our own submissions
            // were finished before the texture was dropped (see `retire`).
            unsafe {
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
        });
        // SAFETY: the image was created on this device matching the descriptor below, and the
        // callback keeps it and its memory alive until wgpu is done with it.
        let hal_texture = unsafe {
            let hal = gpu.device.as_hal::<Vulkan>().expect("Vulkan device");
            hal.texture_from_raw(
                image,
                &wgpu::hal::TextureDescriptor {
                    label: Some("dmabuf"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: WGPU_FORMAT,
                    usage: wgpu::TextureUses::COLOR_TARGET,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: vec![],
                },
                Some(drop_callback),
                wgpu::hal::vulkan::TextureMemory::External,
            )
        };
        // SAFETY: as above; the image starts in the UNDEFINED layout.
        unsafe {
            gpu.device.create_texture_from_hal::<Vulkan>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("dmabuf"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: WGPU_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        }
    }

    /// Takes the image back from the compositor before rendering. `wait` is a sync file the GPU
    /// waits for first (pending compositor reads under implicit sync).
    ///
    /// Call before the `queue.submit` that renders to the image.
    pub fn begin_access(
        &mut self,
        image: &mut DmabufImage,
        wait: Option<OwnedFd>,
    ) -> Result<(), GpuError> {
        if image.fresh {
            // wgpu transitions it out of UNDEFINED itself.
            return Ok(());
        }
        let mut wait_semaphores = Vec::new();
        if let Some(fd) = wait {
            let raw = fd.into_raw_fd();
            // SAFETY: on success the driver owns `raw`; the semaphore is not in use (its last
            // wait completed before the image's previous release did).
            let r = unsafe {
                self.semaphore_fd.import_semaphore_fd(
                    &vk::ImportSemaphoreFdInfoKHR::default()
                        .semaphore(image.wait_semaphore)
                        .flags(vk::SemaphoreImportFlags::TEMPORARY)
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                        .fd(raw),
                )
            };
            match r {
                Ok(()) => wait_semaphores.push(image.wait_semaphore),
                Err(e) => {
                    // SAFETY: the import failed, so we still own it.
                    drop(unsafe { OwnedFd::from_raw_fd(raw) });
                    return Err(vk_err("importing a sync file")(e));
                }
            }
        }
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let cmds = [image.acquire];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&stages[..wait_semaphores.len()])
            .command_buffers(&cmds);
        // SAFETY: the queue is only used from this thread; submission order puts this before
        // wgpu's rendering.
        unsafe {
            self.device
                .queue_submit(self.queue, &[submit], vk::Fence::null())
        }
        .map_err(vk_err("submitting the acquire barrier"))
    }

    /// Hands the image to the compositor after rendering. Returns a sync file that signals
    /// when rendering is done, or `None` if it already has.
    ///
    /// Call after the `queue.submit` that rendered to the image.
    pub fn end_access(&mut self, image: &mut DmabufImage) -> Result<Option<OwnedFd>, GpuError> {
        let device = &self.device;
        // SAFETY: valid fence; it is normally long signaled (the compositor waited for it).
        unsafe {
            device
                .wait_for_fences(
                    &[image.fence],
                    true,
                    Duration::from_secs(1).as_nanos() as u64,
                )
                .map_err(vk_err("waiting for the previous release"))?;
            device
                .reset_fences(&[image.fence])
                .map_err(vk_err("resetting a fence"))?;
        }
        let cmds = [image.release];
        let signal = [image.signal_semaphore];
        let submit = vk::SubmitInfo::default()
            .command_buffers(&cmds)
            .signal_semaphores(&signal);
        // SAFETY: as in `begin_access`; this follows wgpu's submission.
        unsafe { device.queue_submit(self.queue, &[submit], image.fence) }
            .map_err(vk_err("submitting the release barrier"))?;
        image.fresh = false;
        // SAFETY: the semaphore has a pending signal operation.
        let fd = unsafe {
            self.semaphore_fd.get_semaphore_fd(
                &vk::SemaphoreGetFdInfoKHR::default()
                    .semaphore(image.signal_semaphore)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
            )
        }
        .map_err(vk_err("exporting a sync file"))?;
        // SAFETY: a non-negative result is a new file descriptor; -1 means already signaled.
        Ok((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Frees an image once the GPU is done with it.
    pub fn retire(&mut self, image: DmabufImage) {
        self.graveyard.push(Retired {
            _texture: image.texture,
            commands: [image.acquire, image.release],
            semaphores: [image.wait_semaphore, image.signal_semaphore],
            fence: image.fence,
        });
        self.reap();
    }

    fn reap(&mut self) {
        let device = &self.device;
        let pool = self.pool;
        self.graveyard.retain(|r| {
            // SAFETY: valid fence.
            let done = unsafe { device.get_fence_status(r.fence) }.unwrap_or(true);
            if done {
                // SAFETY: the last submission using these finished.
                unsafe {
                    device.free_command_buffers(pool, &r.commands);
                    for s in r.semaphores {
                        device.destroy_semaphore(s, None);
                    }
                    device.destroy_fence(r.fence, None);
                }
            }
            !done
        });
    }
}

impl Drop for DmabufContext {
    fn drop(&mut self) {
        // SAFETY: waits for all our submissions, then frees what is left.
        unsafe {
            let _ = self.device.queue_wait_idle(self.queue);
        }
        self.reap();
        // SAFETY: nothing uses the pool any more.
        unsafe { self.device.destroy_command_pool(self.pool, None) };
    }
}

impl DmabufImage {
    pub fn fd(&self) -> BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.fd.as_fd()
    }
}

/// Modifiers usable as `ARGB8888` color attachments and exportable as dmabufs.
fn query_modifiers(instance: &ash::Instance, pd: vk::PhysicalDevice) -> Vec<ModifierInfo> {
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut props = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: two-call query with valid structures.
    unsafe { instance.get_physical_device_format_properties2(pd, VK_FORMAT, &mut props) };
    let count = list.drm_format_modifier_count as usize;
    let mut entries = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
        .drm_format_modifier_properties(&mut entries);
    let mut props = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: as above.
    unsafe { instance.get_physical_device_format_properties2(pd, VK_FORMAT, &mut props) };
    let needed =
        vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND;
    entries
        .iter()
        .filter(|e| e.drm_format_modifier_tiling_features.contains(needed))
        .filter_map(|e| {
            let mut drm_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
                .drm_format_modifier(e.drm_format_modifier)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let mut ext_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let info = vk::PhysicalDeviceImageFormatInfo2::default()
                .format(VK_FORMAT)
                .ty(vk::ImageType::TYPE_2D)
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .push_next(&mut drm_info)
                .push_next(&mut ext_info);
            let mut ext_props = vk::ExternalImageFormatProperties::default();
            let mut out = vk::ImageFormatProperties2::default().push_next(&mut ext_props);
            // SAFETY: valid structures.
            unsafe { instance.get_physical_device_image_format_properties2(pd, &info, &mut out) }
                .ok()?;
            let extent = out.image_format_properties.max_extent;
            let exportable = ext_props
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE);
            exportable.then_some(ModifierInfo {
                modifier: e.drm_format_modifier,
                planes: e.drm_format_modifier_plane_count,
                max_extent: (extent.width, extent.height),
            })
        })
        .collect()
}

/// The `/dev/dri` node with device number `dev`.
fn find_device_node(dev: u64) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    std::fs::read_dir("/dev/dri")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.rdev() == dev))
}

/// How many frames can be on their way to being read at the same time, which there is no point in
/// having many of: reading them comes right after.
const VIDEO_ACQUIRES: usize = 8;

/// Reads frames of videos where the decoder left them, which is in dmabufs. See
/// `docs/design/24-video.md`.
pub struct VideoImporter {
    instance: ash::Instance,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family: u32,
    memory_fd: khr::external_memory_fd::Device,
    pool: vk::CommandPool,
    /// What takes frames over from the decoder, used in turns.
    acquires: Vec<(vk::CommandBuffer, vk::Fence)>,
    next: usize,
}

/// The textures of one picture of the decoder's pool, which is used over and over.
pub struct ImportedFrame {
    pub bind: wgpu::BindGroup,
    images: Vec<vk::Image>,
    _textures: Vec<wgpu::Texture>,
}

impl DmabufContext {
    /// Something that can read what a decoder on this GPU leaves behind.
    pub fn video_importer(&self) -> Result<VideoImporter, GpuError> {
        let device = self.device.clone();
        // SAFETY: valid device and queue family.
        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(self.queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }
        .map_err(vk_err("command pool"))?;
        // SAFETY: from a valid pool.
        let buffers = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(VIDEO_ACQUIRES as u32),
            )
        }
        .map_err(vk_err("command buffers"))?;
        let mut acquires = Vec::with_capacity(VIDEO_ACQUIRES);
        for buffer in buffers {
            // SAFETY: valid device. Signaled, since nothing was submitted that it has to wait for.
            let fence = unsafe {
                device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .map_err(vk_err("fence"))?;
            acquires.push((buffer, fence));
        }
        Ok(VideoImporter {
            instance: self.instance.clone(),
            memory_fd: self.memory_fd.clone(),
            device,
            physical_device: self.physical_device,
            queue: self.queue,
            queue_family: self.queue_family,
            pool,
            acquires,
            next: 0,
        })
    }
}

impl VideoImporter {
    /// What tells a picture of the pool from the others, which is the dmabuf it is in.
    pub fn key_of(planes: &[anti_webui_core::video::DmabufPlane]) -> Option<(u64, u64)> {
        let first = planes.first()?;
        // SAFETY: an open descriptor, and an out parameter of the right type.
        let inode = unsafe {
            let mut stat = std::mem::zeroed::<libc::stat>();
            (libc::fstat(first.fd, &mut stat) == 0).then_some(stat.st_ino as u64)
        }?;
        Some((inode, first.offset))
    }

    fn formats(format: anti_webui_core::video::PlaneFormat) -> (vk::Format, wgpu::TextureFormat) {
        use anti_webui_core::video::PlaneFormat as P;
        match format {
            P::R8 => (vk::Format::R8_UNORM, wgpu::TextureFormat::R8Unorm),
            P::Rg8 => (vk::Format::R8G8_UNORM, wgpu::TextureFormat::Rg8Unorm),
            P::R16 => (vk::Format::R16_UNORM, wgpu::TextureFormat::R16Unorm),
            P::Rg16 => (vk::Format::R16G16_UNORM, wgpu::TextureFormat::Rg16Unorm),
        }
    }

    /// Whether the driver says this can be read with this modifier.
    fn can_import(&self, format: vk::Format, modifier: u64) -> bool {
        let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .push_next(&mut drm)
            .push_next(&mut external);
        let mut external_props = vk::ExternalImageFormatProperties::default();
        let mut props = vk::ImageFormatProperties2::default().push_next(&mut external_props);
        // SAFETY: valid physical device and structures.
        let done = unsafe {
            self.instance.get_physical_device_image_format_properties2(
                self.physical_device,
                &info,
                &mut props,
            )
        };
        done.is_ok()
            && external_props
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    }

    fn import_plane(
        &self,
        device: &wgpu::Device,
        plane: &anti_webui_core::video::DmabufPlane,
    ) -> Result<(wgpu::Texture, vk::Image), GpuError> {
        let (format, wgpu_format) = Self::formats(plane.format);
        if !device
            .features()
            .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
            && matches!(
                plane.format,
                anti_webui_core::video::PlaneFormat::R16
                    | anti_webui_core::video::PlaneFormat::Rg16
            )
        {
            return Err(GpuError::Dmabuf("no textures of 16 bits".into()));
        }
        if !self.can_import(format, plane.modifier) {
            return Err(GpuError::Dmabuf(format!(
                "can't read {format:?} with modifier {:#x}",
                plane.modifier
            )));
        }
        let layouts = [vk::SubresourceLayout::default()
            .offset(plane.offset)
            .row_pitch(plane.stride)];
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(plane.modifier)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: plane.size.0,
                height: plane.size.1,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut explicit)
            .push_next(&mut external);
        // SAFETY: valid device and structures.
        let image = unsafe { self.device.create_image(&info, None) }
            .map_err(vk_err("creating a video image"))?;
        // SAFETY: the descriptor is open while the frame is alive, which is now.
        let fd = unsafe { libc::dup(plane.fd) };
        let free_image = || {
            // SAFETY: nothing else has it yet.
            unsafe { self.device.destroy_image(image, None) };
        };
        if fd < 0 {
            free_image();
            return Err(GpuError::Dmabuf("can't duplicate a dmabuf".into()));
        }
        // SAFETY: a new descriptor that belongs to this until it is handed over below.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut fd_props = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: valid image, descriptor and output.
        let (requirements, known) = unsafe {
            let requirements = self.device.get_image_memory_requirements(image);
            let known = self.memory_fd.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                owned.as_raw_fd(),
                &mut fd_props,
            );
            (requirements, known)
        };
        let bits = requirements.memory_type_bits & fd_props.memory_type_bits;
        if known.is_err() || bits == 0 {
            free_image();
            return Err(GpuError::Dmabuf("no memory type for a dmabuf".into()));
        }
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let raw = owned.into_raw_fd();
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(raw);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(bits.trailing_zeros())
            .push_next(&mut dedicated)
            .push_next(&mut import);
        // SAFETY: valid structures. On success the driver owns the descriptor.
        let memory = match unsafe { self.device.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(e) => {
                // SAFETY: the import failed, so it is still ours.
                drop(unsafe { OwnedFd::from_raw_fd(raw) });
                free_image();
                return Err(vk_err("importing a video image")(e));
            }
        };
        // SAFETY: both are new, and made for each other.
        if let Err(e) = unsafe { self.device.bind_image_memory(image, memory, 0) } {
            // SAFETY: nothing else has them.
            unsafe { self.device.free_memory(memory, None) };
            free_image();
            return Err(vk_err("binding a video image")(e));
        }
        let size = wgpu::Extent3d {
            width: plane.size.0,
            height: plane.size.1,
            depth_or_array_layers: 1,
        };
        // SAFETY: the image was created as this says, and is new. wgpu owns it and its memory from
        // here on, and frees them when it is through with them.
        let texture = unsafe {
            let hal = device.as_hal::<Vulkan>().expect("Vulkan device");
            let hal_texture = hal.texture_from_raw(
                image,
                &wgpu::hal::TextureDescriptor {
                    label: Some("video"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu_format,
                    usage: wgpu::TextureUses::RESOURCE,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: vec![],
                },
                None,
                wgpu::hal::vulkan::TextureMemory::Dedicated(memory),
            );
            drop(hal);
            // `acquire` has it in this state before anything reads it. wgpu doing that itself would
            // start from "whatever was in there doesn't matter".
            device.create_texture_from_hal::<Vulkan>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("video"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu_format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::TextureUses::RESOURCE,
            )
        };
        Ok((texture, image))
    }

    /// Makes textures of the planes of a picture.
    pub fn import(
        &mut self,
        device: &wgpu::Device,
        planes: &[anti_webui_core::video::DmabufPlane],
        bind: impl FnOnce(&[wgpu::TextureView]) -> wgpu::BindGroup,
    ) -> Result<ImportedFrame, GpuError> {
        let mut textures = Vec::with_capacity(planes.len());
        let mut images = Vec::with_capacity(planes.len());
        for plane in planes {
            let (texture, image) = self.import_plane(device, plane)?;
            textures.push(texture);
            images.push(image);
        }
        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        Ok(ImportedFrame {
            bind: bind(&views),
            images,
            _textures: textures,
        })
    }

    /// Takes a picture over from the decoder, which has finished it by the time it gets here. Call
    /// before the `queue.submit` that reads it.
    pub fn acquire(&mut self, frame: &ImportedFrame) -> Result<(), GpuError> {
        let (buffer, fence) = self.acquires[self.next];
        self.next = (self.next + 1) % self.acquires.len();
        let barriers: Vec<vk::ImageMemoryBarrier> = frame
            .images
            .iter()
            .map(|&image| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                    .dst_queue_family_index(self.queue_family)
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
            })
            .collect();
        let cmds = [buffer];
        // SAFETY: the fence says when the buffer is free. The queue is only used from this thread,
        // and this goes in before what reads the images.
        unsafe {
            let device = &self.device;
            device
                .wait_for_fences(&[fence], true, Duration::from_secs(1).as_nanos() as u64)
                .map_err(vk_err("waiting for an acquire"))?;
            device
                .reset_fences(&[fence])
                .map_err(vk_err("resetting a fence"))?;
            device
                .reset_command_buffer(buffer, vk::CommandBufferResetFlags::empty())
                .map_err(vk_err("resetting a command buffer"))?;
            device
                .begin_command_buffer(
                    buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(vk_err("recording an acquire"))?;
            device.cmd_pipeline_barrier(
                buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
            device
                .end_command_buffer(buffer)
                .map_err(vk_err("recording an acquire"))?;
            device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&cmds)],
                    fence,
                )
                .map_err(vk_err("submitting an acquire"))
        }
    }
}

impl Drop for VideoImporter {
    fn drop(&mut self) {
        let fences: Vec<vk::Fence> = self.acquires.iter().map(|(_, f)| *f).collect();
        // SAFETY: waits until nothing uses these, and nothing else has them.
        unsafe {
            let _ = self.device.wait_for_fences(
                &fences,
                true,
                Duration::from_secs(1).as_nanos() as u64,
            );
            for fence in fences {
                self.device.destroy_fence(fence, None);
            }
            self.device.destroy_command_pool(self.pool, None);
        }
    }
}
