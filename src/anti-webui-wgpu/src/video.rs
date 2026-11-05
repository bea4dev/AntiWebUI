//! Frames of videos on the GPU: in memory they are copied to textures, in dmabufs they are read
//! where they are. See `docs/design/24-video.md`.

use anti_webui_core::scene::VideoDraw;
use anti_webui_core::video::{
    PlaneFormat, VideoColor, VideoFrame, VideoPixels, VideoTransfer, YuvMatrix,
};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

/// A frame on its way to the screen.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct VideoInstance {
    pub area: [f32; 4],
    pub rect: [f32; 4],
    pub shape: [f32; 4],
    pub shape_radii: [f32; 4],
    pub clip: [f32; 4],
    pub clip_radii: [f32; 4],
    /// The first three rows of what turns the numbers of a frame into red, green and blue.
    pub to_rgb: [[f32; 4]; 3],
    /// x: opacity, y: degrees clockwise, z: 3 with the chroma in two textures, w: transfer.
    pub params: [f32; 4],
    /// xy: the size of a texel of luma in the frame, z: how many are read for each pixel.
    pub sampling: [f32; 4],
}

/// What turns the numbers of a frame into red, green and blue.
pub(crate) fn to_rgb(color: VideoColor) -> [[f32; 4]; 3] {
    let (kr, kb) = match color.matrix {
        YuvMatrix::Bt601 => (0.299, 0.114),
        YuvMatrix::Bt709 => (0.2126, 0.0722),
        YuvMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let kg = 1.0 - kr - kb;
    let (y_scale, y_offset, c_scale) = if color.full_range {
        (1.0, 0.0, 1.0)
    } else {
        (255.0 / 219.0, 16.0 / 255.0, 255.0 / 224.0)
    };
    let c_offset = 128.0 / 255.0;
    let row = |cb: f32, cr: f32| {
        let (cb, cr) = (cb * c_scale, cr * c_scale);
        [
            y_scale,
            cb,
            cr,
            -y_offset * y_scale - c_offset * cb - c_offset * cr,
        ]
    };
    [
        row(0.0, 2.0 * (1.0 - kr)),
        row(-2.0 * kb * (1.0 - kb) / kg, -2.0 * kr * (1.0 - kr) / kg),
        row(2.0 * (1.0 - kb), 0.0),
    ]
}

fn texture_format(format: PlaneFormat, deep: bool) -> wgpu::TextureFormat {
    match (format, deep) {
        (PlaneFormat::R8, _) | (PlaneFormat::R16, false) => wgpu::TextureFormat::R8Unorm,
        (PlaneFormat::Rg8, _) | (PlaneFormat::Rg16, false) => wgpu::TextureFormat::Rg8Unorm,
        (PlaneFormat::R16, true) => wgpu::TextureFormat::R16Unorm,
        (PlaneFormat::Rg16, true) => wgpu::TextureFormat::Rg16Unorm,
    }
}

/// Where the frames of a stream go, while they come in memory.
struct Copied {
    textures: Vec<wgpu::Texture>,
    bind: wgpu::BindGroup,
    formats: Vec<PlaneFormat>,
    sizes: Vec<(u32, u32)>,
}

/// A stream on the GPU.
struct Stream {
    alive: Weak<()>,
    /// The last frame that was made ready.
    frame: u64,
    copied: Option<Copied>,
    /// The pictures of the decoder's pool that have been seen, by which dmabuf they are in.
    #[cfg(target_os = "linux")]
    imported: HashMap<(u64, u64), crate::dmabuf::ImportedFrame>,
    /// What to draw with.
    #[cfg(target_os = "linux")]
    showing: Option<(u64, u64)>,
}

/// What the renderer keeps for videos. Nothing of it costs anything until the first one shows.
pub(crate) struct VideoPass {
    globals_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    shader: wgpu::ShaderModule,
    pipelines: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Sampler)>,
    streams: HashMap<u64, Stream>,
    uploads: Vec<Arc<VideoFrame>>,
    /// What the frame being drawn reads, which has to stay until the GPU is through with it.
    held: Vec<Arc<VideoFrame>>,
    #[cfg(target_os = "linux")]
    importer: Option<crate::dmabuf::VideoImporter>,
    import_failed: bool,
    /// How many times a frame was read from where it is, for tests.
    imports: Arc<Mutex<u64>>,
}

impl VideoPass {
    pub fn new(
        globals_layout: wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        shader: wgpu::ShaderModule,
    ) -> Self {
        Self {
            globals_layout,
            format,
            blend,
            shader,
            pipelines: None,
            streams: HashMap::new(),
            uploads: Vec::new(),
            held: Vec::new(),
            #[cfg(target_os = "linux")]
            importer: None,
            import_failed: false,
            imports: Arc::default(),
        }
    }

    #[cfg(target_os = "linux")]
    pub fn set_importer(&mut self, importer: crate::dmabuf::VideoImporter) {
        self.importer = Some(importer);
    }

    /// A frame couldn't be read where it is, since this was last asked.
    pub fn take_import_failed(&mut self) -> bool {
        std::mem::take(&mut self.import_failed)
    }

    /// How many streams are on the GPU, and how many frames were read from where they are.
    pub fn counts(&self) -> (usize, u64) {
        (
            self.streams.len(),
            *self.imports.lock().expect("no panics in here"),
        )
    }

    fn ensure_pipeline(&mut self, device: &wgpu::Device) {
        if self.pipelines.is_some() {
            return;
        }
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video"),
            entries: &[
                texture(0),
                texture(1),
                texture(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video"),
            bind_group_layouts: &[Some(&self.globals_layout), Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
            10 => Float32x4
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_video"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<VideoInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_video"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: self.blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        self.pipelines = Some((pipeline, layout, sampler));
    }

    /// Drops what no stream needs any more, and lets go of what the GPU is through with: what the
    /// last frame read is done once the submission before this one is.
    pub fn begin_frame(&mut self, queue: &wgpu::Queue) {
        self.streams.retain(|_, s| s.alive.strong_count() > 0);
        if !self.held.is_empty() {
            let held = std::mem::take(&mut self.held);
            queue.on_submitted_work_done(move || drop(held));
        }
    }

    /// Makes sure the GPU is going to have a frame.
    pub fn prepare(&mut self, device: &wgpu::Device, frame: &Arc<VideoFrame>) {
        self.ensure_pipeline(device);
        let stream = self.streams.entry(frame.stream).or_insert_with(|| Stream {
            alive: frame.alive.clone(),
            frame: 0,
            copied: None,
            #[cfg(target_os = "linux")]
            imported: HashMap::new(),
            #[cfg(target_os = "linux")]
            showing: None,
        });
        if stream.frame != frame.id {
            stream.frame = frame.id;
            self.uploads.push(frame.clone());
        }
    }

    /// Hands to the GPU what the frame needs. Call before anything is drawn.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for frame in std::mem::take(&mut self.uploads) {
            match &frame.pixels {
                VideoPixels::Memory(_) => self.upload_memory(device, queue, &frame),
                #[cfg(unix)]
                VideoPixels::Dmabuf(_) => self.import(device, &frame),
            }
        }
    }

    fn upload_memory(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &VideoFrame) {
        let VideoPixels::Memory(planes) = &frame.pixels else {
            return;
        };
        let deep = device
            .features()
            .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
        let formats: Vec<PlaneFormat> = planes.iter().map(|p| p.format).collect();
        let sizes: Vec<(u32, u32)> = planes.iter().map(|p| p.size).collect();
        let Some(stream) = self.streams.get_mut(&frame.stream) else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            stream.showing = None;
        }
        let (_, layout, sampler) = self.pipelines.as_ref().expect("made in prepare");
        if stream
            .copied
            .as_ref()
            .is_none_or(|c| c.formats != formats || c.sizes != sizes)
        {
            let textures: Vec<wgpu::Texture> = planes
                .iter()
                .map(|p| {
                    device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("video"),
                        size: wgpu::Extent3d {
                            width: p.size.0,
                            height: p.size.1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: texture_format(p.format, deep),
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    })
                })
                .collect();
            let views: Vec<wgpu::TextureView> = textures
                .iter()
                .map(|t| t.create_view(&Default::default()))
                .collect();
            let bind = bind_group(device, layout, sampler, &views);
            stream.copied = Some(Copied {
                textures,
                bind,
                formats,
                sizes,
            });
        }
        let copied = stream.copied.as_ref().expect("just made sure");
        let mut narrowed = Vec::new();
        for (plane, texture) in planes.iter().zip(&copied.textures) {
            let wide = matches!(plane.format, PlaneFormat::R16 | PlaneFormat::Rg16);
            let (bytes, stride): (&[u8], u32) = if wide && !deep {
                // Without textures that hold more, the high bits are what there is room for.
                let row = plane.size.0 * plane.format.texel_bytes();
                narrowed.clear();
                for line in plane.bytes().chunks(plane.stride as usize) {
                    narrowed.extend(line[..row as usize].iter().skip(1).step_by(2));
                }
                (&narrowed, row / 2)
            } else {
                (plane.bytes(), plane.stride)
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(plane.size.1),
                },
                wgpu::Extent3d {
                    width: plane.size.0,
                    height: plane.size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[cfg(unix)]
    fn import(&mut self, _: &wgpu::Device, _: &Arc<VideoFrame>) {
        self.import_failed = true;
    }

    #[cfg(target_os = "linux")]
    fn import(&mut self, device: &wgpu::Device, frame: &Arc<VideoFrame>) {
        let (Some(importer), VideoPixels::Dmabuf(planes)) = (&mut self.importer, &frame.pixels)
        else {
            self.import_failed = true;
            return;
        };
        let Some(stream) = self.streams.get_mut(&frame.stream) else {
            return;
        };
        let (_, layout, sampler) = self.pipelines.as_ref().expect("made in prepare");
        let Some(key) = crate::dmabuf::VideoImporter::key_of(planes) else {
            self.import_failed = true;
            return;
        };
        let imported = match stream.imported.entry(key) {
            std::collections::hash_map::Entry::Occupied(known) => known.into_mut(),
            std::collections::hash_map::Entry::Vacant(new) => {
                let imported = importer.import(device, planes, |views| {
                    bind_group(device, layout, sampler, views)
                });
                let Ok(imported) = imported else {
                    self.import_failed = true;
                    return;
                };
                new.insert(imported)
            }
        };
        if importer.acquire(imported).is_err() {
            self.import_failed = true;
            return;
        }
        stream.showing = Some(key);
        self.held.push(frame.clone());
        *self.imports.lock().expect("no panics in here") += 1;
    }

    /// What to set to draw frames of a stream, unless there is nothing yet to draw.
    pub fn draw_state(&self, stream: u64) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup)> {
        let (pipeline, _, _) = self.pipelines.as_ref()?;
        let stream = self.streams.get(&stream)?;
        #[cfg(target_os = "linux")]
        if let Some(key) = &stream.showing {
            return Some((pipeline, &stream.imported.get(key)?.bind));
        }
        Some((pipeline, &stream.copied.as_ref()?.bind))
    }

    /// The numbers for drawing a frame, besides where.
    pub fn instance(
        d: &VideoDraw,
        area: [f32; 4],
        rect: [f32; 4],
        shape: ([f32; 4], [f32; 4]),
        clip: ([f32; 4], [f32; 4]),
    ) -> VideoInstance {
        let f = &d.frame;
        let three = match &f.pixels {
            VideoPixels::Memory(planes) => planes.len() == 3,
            #[cfg(unix)]
            VideoPixels::Dmabuf(planes) => planes.len() == 3,
        };
        let transfer = match f.color.transfer {
            VideoTransfer::Sdr => 0.0,
            VideoTransfer::Pq => 1.0,
            VideoTransfer::Hlg => 2.0,
        };
        // Past twice as small, reading one place between texels leaves some of them out.
        let turned = !f.rotation.is_multiple_of(180);
        let (w, h) = if turned { (f.size.1, f.size.0) } else { f.size };
        let shrink = (w as f32 / rect[2].max(1.0)).max(h as f32 / rect[3].max(1.0));
        VideoInstance {
            area,
            rect,
            shape: shape.0,
            shape_radii: shape.1,
            clip: clip.0,
            clip_radii: clip.1,
            to_rgb: to_rgb(f.color),
            params: [
                d.opacity,
                f.rotation as f32,
                if three { 3.0 } else { 2.0 },
                transfer,
            ],
            sampling: [
                1.0 / f.size.0.max(1) as f32,
                1.0 / f.size.1.max(1) as f32,
                if shrink > 2.0 { shrink / 2.0 } else { 0.0 },
                0.0,
            ],
        }
    }
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    views: &[wgpu::TextureView],
) -> wgpu::BindGroup {
    // With the chroma in one texture it goes in both of the places for it.
    let last = views.last().expect("a frame has planes");
    let second = views.get(1).unwrap_or(last);
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("video"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(second),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(last),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &[[f32; 4]; 3], y: f32, cb: f32, cr: f32) -> [f32; 3] {
        let row = |r: [f32; 4]| r[0] * y + r[1] * cb + r[2] * cr + r[3];
        [row(m[0]), row(m[1]), row(m[2])]
    }

    fn close(a: [f32; 3], b: [f32; 3]) {
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 0.01, "{a:?} isn't {b:?}");
        }
    }

    /// Well-known colors, from the definitions of each standard.
    #[test]
    fn colors_come_out_right() {
        let limited = |matrix| {
            to_rgb(VideoColor {
                matrix,
                full_range: false,
                transfer: VideoTransfer::Sdr,
            })
        };
        let v = |x: f32| x / 255.0;
        // Black and white.
        for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
            close(
                apply(&limited(matrix), v(16.0), v(128.0), v(128.0)),
                [0.0; 3],
            );
            close(
                apply(&limited(matrix), v(235.0), v(128.0), v(128.0)),
                [1.0; 3],
            );
        }
        // Red in BT.709 and BT.601.
        close(
            apply(&limited(YuvMatrix::Bt709), v(63.0), v(102.0), v(240.0)),
            [1.0, 0.0, 0.0],
        );
        close(
            apply(&limited(YuvMatrix::Bt601), v(81.0), v(90.0), v(240.0)),
            [1.0, 0.0, 0.0],
        );
        // Full range, as in a JPEG.
        let full = to_rgb(VideoColor {
            matrix: YuvMatrix::Bt601,
            full_range: true,
            transfer: VideoTransfer::Sdr,
        });
        close(apply(&full, 0.0, v(128.0), v(128.0)), [0.0; 3]);
        close(apply(&full, 1.0, v(128.0), v(128.0)), [1.0; 3]);
    }
}
