//! Scene → GPU draw calls.

use crate::glyph_cache::{GlyphCache, PlacedGlyph};
use anti_webui_core::damage::{Damage, primitive_bounds};
use anti_webui_core::geometry::RectI;
use anti_webui_core::scene::{Clip, Primitive, Scene};
use bytemuck::{Pod, Zeroable};
use std::ops::Range;
use wgpu::util::DeviceExt;

const ATLAS_INITIAL: u32 = 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadInstance {
    rect: [f32; 4],
    background: [f32; 4],
    border_color: [f32; 4],
    border: [f32; 4],
    radii: [f32; 4],
    clip: [f32; 4],
    params: [f32; 4],
    clip_radii: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    clip: [f32; 4],
    params: [f32; 4],
    clip_radii: [f32; 4],
}

enum Batch {
    Quads(Range<u32>),
    Glyphs(Range<u32>),
}

fn rect4(r: RectI) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.width as f32, r.height as f32]
}

/// Clip rectangle and radii for the GPU. Rectangular clips are cut to the screen so the
/// "infinite" clip stays small; rounded clips keep their own rectangle for the corner shape.
fn gpu_clip(clip: &Clip, width: u32, height: u32) -> ([f32; 4], [f32; 4]) {
    if clip.is_rounded() {
        (rect4(clip.rect), clip.radii)
    } else {
        let screen = RectI::new(0, 0, width as i32, height as i32);
        (rect4(clip.rect.intersect(&screen)), [0.0; 4])
    }
}

struct InstanceBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, label: &str) -> Self {
        let capacity = 4096;
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if data.len() as u64 > self.capacity {
            self.capacity = (data.len() as u64).next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, data);
    }
}

pub struct Renderer {
    format: wgpu::TextureFormat,
    quad_pipeline: wgpu::RenderPipeline,
    /// The quad pipeline without blending, for clearing damaged rectangles.
    clear_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    atlas_layout: wgpu::BindGroupLayout,
    atlas: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    glyphs: GlyphCache,
    quad_buffer: InstanceBuffer,
    glyph_buffer: InstanceBuffer,
    max_texture: u32,
}

fn create_atlas(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    size: u32,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyph atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (texture, bind)
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("anti-webui shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::bytes_of(&Globals {
                viewport: [1.0, 1.0],
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let (atlas, atlas_bind) = create_atlas(device, &atlas_layout, &sampler, ATLAS_INITIAL);

        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let targets = [Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let quad_attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
            4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4
        ];
        let quad_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &quad_attrs,
        })];
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        };
        let quad_pipeline_with = |label, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&quad_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_quad"),
                    compilation_options: Default::default(),
                    buffers: &quad_buffers,
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_quad"),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let quad_pipeline = quad_pipeline_with("quad", &targets);
        let clear_pipeline = quad_pipeline_with(
            "clear",
            &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        );

        let glyph_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph"),
            bind_group_layouts: &[Some(&globals_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });
        let glyph_attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4
        ];
        let glyph_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &glyph_attrs,
        })];
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph"),
            layout: Some(&glyph_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_glyph"),
                compilation_options: Default::default(),
                buffers: &glyph_buffers,
            },
            primitive,
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_glyph"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            format,
            quad_pipeline,
            clear_pipeline,
            glyph_pipeline,
            globals,
            globals_bind,
            atlas_layout,
            atlas,
            atlas_bind,
            sampler,
            glyphs: GlyphCache::new(ATLAS_INITIAL),
            quad_buffer: InstanceBuffer::new(device, "quads"),
            glyph_buffer: InstanceBuffer::new(device, "glyphs"),
            max_texture: device.limits().max_texture_dimension_2d,
        }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Builds instance data; grows the atlas and retries if glyphs did not fit.
    ///
    /// With `damage`, the first quad instances clear the damaged rectangles (one each) and
    /// primitives outside of them are dropped.
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        scene: &Scene,
        damage: Option<&[RectI]>,
    ) -> (Vec<QuadInstance>, Vec<GlyphInstance>, Vec<Batch>) {
        loop {
            match self.try_prepare(scene, damage) {
                Some(r) => return r,
                None => {
                    let next = (self.glyphs.size * 2).min(self.max_texture);
                    self.glyphs.clear(next);
                    let (t, b) = create_atlas(device, &self.atlas_layout, &self.sampler, next);
                    self.atlas = t;
                    self.atlas_bind = b;
                }
            }
        }
    }

    fn try_prepare(
        &mut self,
        scene: &Scene,
        damage: Option<&[RectI]>,
    ) -> Option<(Vec<QuadInstance>, Vec<GlyphInstance>, Vec<Batch>)> {
        let mut quads = Vec::new();
        let mut glyphs = Vec::new();
        let mut batches: Vec<Batch> = Vec::new();
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        let clear = scene.clear.premultiplied();
        for r in damage.unwrap_or_default() {
            quads.push(QuadInstance {
                rect: rect4(*r),
                background: clear,
                border_color: [0.0; 4],
                border: [0.0; 4],
                radii: [0.0; 4],
                clip: rect4(*r),
                params: [0.0; 4],
                clip_radii: [0.0; 4],
            });
        }
        for prim in &scene.primitives {
            if let Some(damage) = damage {
                let bounds = primitive_bounds(prim);
                if !damage.iter().any(|r| r.intersects(&bounds)) {
                    continue;
                }
            }
            match prim {
                Primitive::Quad(q) => {
                    let screen = RectI::new(0, 0, scene.width as i32, scene.height as i32);
                    let visible = q.clip.rect.intersect(&screen);
                    if visible.is_empty() || q.rect.intersect(&visible).is_empty() {
                        continue;
                    }
                    let (clip, clip_radii) = gpu_clip(&q.clip, scene.width, scene.height);
                    let b = q.border_widths;
                    quads.push(QuadInstance {
                        rect: rect4(q.rect),
                        background: q.background.premultiplied(),
                        border_color: q.border_color.premultiplied(),
                        border: [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32],
                        radii: q.radii,
                        clip,
                        params: [0.0; 4],
                        clip_radii,
                    });
                    push_batch(
                        &mut batches,
                        true,
                        quads.len() as u32 - 1,
                        quads.len() as u32,
                    );
                }
                Primitive::Shadow(s) => {
                    let (clip, clip_radii) = gpu_clip(&s.clip, scene.width, scene.height);
                    quads.push(QuadInstance {
                        rect: rect4(s.rect),
                        background: s.color.premultiplied(),
                        border_color: [0.0; 4],
                        border: [0.0; 4],
                        radii: s.radii,
                        clip,
                        params: [1.0, s.blur, 0.0, 0.0],
                        clip_radii,
                    });
                    push_batch(
                        &mut batches,
                        true,
                        quads.len() as u32 - 1,
                        quads.len() as u32,
                    );
                }
                Primitive::Glyphs(run) => {
                    placed.clear();
                    self.glyphs.place_run(run, &mut placed)?;
                    let color = run.color.premultiplied();
                    let (clip, clip_radii) = gpu_clip(&run.clip, scene.width, scene.height);
                    let start = glyphs.len() as u32;
                    for p in &placed {
                        glyphs.push(GlyphInstance {
                            rect: p.rect,
                            uv: p.uv,
                            color,
                            clip,
                            params: [if p.color { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                            clip_radii,
                        });
                    }
                    if !placed.is_empty() {
                        push_batch(&mut batches, false, start, glyphs.len() as u32);
                    }
                }
            }
        }
        Some((quads, glyphs, batches))
    }

    /// Records the scene into `view`, clearing it first.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        scene: &Scene,
    ) {
        self.record(device, queue, encoder, view, scene, None);
    }

    /// Records only the damaged part of the scene into `view`, which must hold the previous
    /// frame's pixels outside of `damage`. `view` must be the size of the scene.
    pub fn render_damaged(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        scene: &Scene,
        damage: &Damage,
    ) {
        let screen = RectI::new(0, 0, scene.width as i32, scene.height as i32);
        let rects: Vec<RectI> = damage
            .rects()
            .iter()
            .map(|r| r.intersect(&screen))
            .filter(|r| !r.is_empty())
            .collect();
        if rects.len() == 1 && rects[0] == screen {
            self.record(device, queue, encoder, view, scene, None);
        } else if !rects.is_empty() {
            self.record(device, queue, encoder, view, scene, Some(&rects));
        }
    }

    fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        scene: &Scene,
        damage: Option<&[RectI]>,
    ) {
        let (quads, glyphs, batches) = self.prepare(device, scene, damage);
        for (g, pixels) in self.glyphs.uploads.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: g.x,
                        y: g.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(g.width * 4),
                    rows_per_image: Some(g.height),
                },
                wgpu::Extent3d {
                    width: g.width,
                    height: g.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                viewport: [scene.width.max(1) as f32, scene.height.max(1) as f32],
                _pad: [0.0; 2],
            }),
        );
        self.quad_buffer
            .upload(device, queue, bytemuck::cast_slice(&quads));
        self.glyph_buffer
            .upload(device, queue, bytemuck::cast_slice(&glyphs));

        let c = scene.clear.premultiplied();
        let load = match damage {
            Some(_) => wgpu::LoadOp::Load,
            None => wgpu::LoadOp::Clear(wgpu::Color {
                r: c[0] as f64,
                g: c[1] as f64,
                b: c[2] as f64,
                a: c[3] as f64,
            }),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("anti-webui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.globals_bind, &[]);
        let Some(damage) = damage else {
            self.draw_batches(&mut pass, &batches);
            return;
        };
        // Each rectangle is cleared and then repainted; overlapping rectangles just repeat work.
        for (i, r) in damage.iter().enumerate() {
            pass.set_scissor_rect(r.x as u32, r.y as u32, r.width as u32, r.height as u32);
            pass.set_pipeline(&self.clear_pipeline);
            pass.set_vertex_buffer(0, self.quad_buffer.buffer.slice(..));
            pass.draw(0..4, i as u32..i as u32 + 1);
            self.draw_batches(&mut pass, &batches);
        }
    }

    fn draw_batches(&self, pass: &mut wgpu::RenderPass<'_>, batches: &[Batch]) {
        for batch in batches {
            match batch {
                Batch::Quads(r) => {
                    pass.set_pipeline(&self.quad_pipeline);
                    pass.set_vertex_buffer(0, self.quad_buffer.buffer.slice(..));
                    pass.draw(0..4, r.clone());
                }
                Batch::Glyphs(r) => {
                    pass.set_pipeline(&self.glyph_pipeline);
                    pass.set_bind_group(1, &self.atlas_bind, &[]);
                    pass.set_vertex_buffer(0, self.glyph_buffer.buffer.slice(..));
                    pass.draw(0..4, r.clone());
                }
            }
        }
    }
}

fn push_batch(batches: &mut Vec<Batch>, quad: bool, start: u32, end: u32) {
    match (batches.last_mut(), quad) {
        (Some(Batch::Quads(r)), true) | (Some(Batch::Glyphs(r)), false) => r.end = end,
        _ => batches.push(if quad {
            Batch::Quads(start..end)
        } else {
            Batch::Glyphs(start..end)
        }),
    }
}
