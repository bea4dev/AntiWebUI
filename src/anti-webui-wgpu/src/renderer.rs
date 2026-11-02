//! Scene → GPU draw calls.

use crate::glyph_cache::{GlyphCache, PlacedGlyph};
use anti_webui_core::damage::{Damage, diff_content, primitive_bounds};
use anti_webui_core::geometry::RectI;
use anti_webui_core::scene::{Clip, Primitive, Scene, ScrollLayer};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
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
    /// Glyph instances that sample the texture of the layer with this key.
    Layer(u64, Range<u32>),
}

/// A texture holding a scroll layer's content. A content pixel `(x, y)` lives at texel
/// `(x mod width, y mod height)`, so what is already drawn stays put as the layer scrolls in any
/// direction (see `docs/design/05-rendering.md`).
struct LayerCache {
    view: wgpu::TextureView,
    /// Binds the texture like the glyph atlas, for compositing.
    bind: wgpu::BindGroup,
    /// Globals for rendering into the texture.
    globals_bind: wgpu::BindGroup,
    size: (i32, i32),
    content: Arc<Scene>,
    /// Bounds of the content's primitives, to find those touching a rectangle quickly.
    bounds: Vec<RectI>,
    /// The content rectangle the texture holds.
    valid: RectI,
    used: bool,
}

/// One rectangle to redraw in a layer's texture.
struct RasterOp {
    scissor: RectI,
    /// Index of the quad instance clearing `scissor`.
    clear: u32,
    batches: Vec<Batch>,
}

/// What a frame redraws in one layer: from the plan, in content coordinates.
struct LayerPlan {
    key: u64,
    dirty: Vec<RectI>,
}

struct Prepared {
    quads: Vec<QuadInstance>,
    glyphs: Vec<GlyphInstance>,
    batches: Vec<Batch>,
    /// Redraws per layer, by key.
    rasters: Vec<(u64, Vec<RasterOp>)>,
}

/// Splits a content rectangle where it wraps around a layer texture of `size`, and calls `f` with
/// each part and its position in the texture.
fn torus_parts(r: RectI, size: (i32, i32), mut f: impl FnMut(RectI, (i32, i32))) {
    let mut y = r.y;
    while y < r.bottom() {
        let ty = y.rem_euclid(size.1);
        let h = (size.1 - ty).min(r.bottom() - y);
        let mut x = r.x;
        while x < r.right() {
            let tx = x.rem_euclid(size.0);
            let w = (size.0 - tx).min(r.right() - x);
            f(RectI::new(x, y, w, h), (tx, ty));
            x += w;
        }
        y += h;
    }
}

/// `outer` minus `inner`, where `inner` lies inside `outer` or is empty.
fn subtract(outer: RectI, inner: RectI, out: &mut Damage) {
    if inner.is_empty() {
        out.add(outer);
        return;
    }
    let (x0, x1) = (inner.x, inner.right());
    out.add(RectI::new(outer.x, outer.y, outer.width, inner.y - outer.y));
    out.add(RectI::new(
        outer.x,
        inner.bottom(),
        outer.width,
        outer.bottom() - inner.bottom(),
    ));
    out.add(RectI::new(outer.x, inner.y, x0 - outer.x, inner.height));
    out.add(RectI::new(x1, inner.y, outer.right() - x1, inner.height));
}

fn rect4(r: RectI) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.width as f32, r.height as f32]
}

/// Clip rectangle and radii for the GPU. Rectangular clips are cut to the target so the
/// "infinite" clip stays small; rounded clips keep their own rectangle for the corner shape.
fn gpu_clip(clip: &Clip, target: RectI) -> ([f32; 4], [f32; 4]) {
    if clip.is_rounded() {
        (rect4(clip.rect), clip.radii)
    } else {
        (rect4(clip.rect.intersect(&target)), [0.0; 4])
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
    globals_layout: wgpu::BindGroupLayout,
    atlas_layout: wgpu::BindGroupLayout,
    atlas: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    glyphs: GlyphCache,
    quad_buffer: InstanceBuffer,
    glyph_buffer: InstanceBuffer,
    max_texture: u32,
    layers: HashMap<u64, LayerCache>,
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
            globals_layout,
            atlas_layout,
            atlas,
            atlas_bind,
            sampler,
            glyphs: GlyphCache::new(ATLAS_INITIAL),
            quad_buffer: InstanceBuffer::new(device, "quads"),
            glyph_buffer: InstanceBuffer::new(device, "glyphs"),
            max_texture: device.limits().max_texture_dimension_2d,
            layers: HashMap::new(),
        }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Forgets all layer textures, so the next frame redraws their content from scratch.
    pub fn clear_layer_caches(&mut self) {
        self.layers.clear();
    }

    fn create_layer_cache(
        &self,
        device: &wgpu::Device,
        layer: &ScrollLayer,
        size: (i32, i32),
    ) -> LayerCache {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scroll layer"),
            size: wgpu::Extent3d {
                width: size.0 as u32,
                height: size.1 as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scroll layer"),
            layout: &self.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scroll layer globals"),
            contents: bytemuck::bytes_of(&Globals {
                viewport: [size.0 as f32, size.1 as f32],
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scroll layer globals"),
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        LayerCache {
            view,
            bind,
            globals_bind,
            size,
            content: layer.content.clone(),
            bounds: layer
                .content
                .primitives
                .iter()
                .map(primitive_bounds)
                .collect(),
            valid: RectI::default(),
            used: true,
        }
    }

    /// Updates the layer caches for this frame and works out what to redraw in their textures.
    /// Layers outside of `damage` are left as they are; their content is caught up with later.
    fn plan_layers(
        &mut self,
        device: &wgpu::Device,
        scene: &Scene,
        damage: Option<&[RectI]>,
    ) -> Vec<LayerPlan> {
        let screen = RectI::new(0, 0, scene.width as i32, scene.height as i32);
        for cache in self.layers.values_mut() {
            cache.used = false;
        }
        let mut plans = Vec::new();
        for prim in &scene.primitives {
            let Primitive::Layer(layer) = prim else {
                continue;
            };
            if let Some(cache) = self.layers.get_mut(&layer.key) {
                cache.used = true;
            }
            let visible = layer.clip.rect.intersect(&screen);
            if visible.is_empty()
                || damage.is_some_and(|d| !d.iter().any(|r| r.intersects(&visible)))
            {
                continue;
            }
            // Round up so that small resizes keep the texture, and shrink only when it is far
            // too big.
            let needed = (
                ((visible.width + 127) & !127).min(self.max_texture as i32),
                ((visible.height + 127) & !127).min(self.max_texture as i32),
            );
            let fits = self.layers.get(&layer.key).is_some_and(|c| {
                c.size.0 >= needed.0
                    && c.size.1 >= needed.1
                    && (c.size.0 as i64 * c.size.1 as i64) <= 2 * needed.0 as i64 * needed.1 as i64
            });
            if !fits {
                let cache = self.create_layer_cache(device, layer, needed);
                self.layers.insert(layer.key, cache);
            }
            let cache = self.layers.get_mut(&layer.key).expect("just inserted");

            let view = visible.translate(-layer.origin.x, -layer.origin.y);
            let kept = cache.valid.intersect(&view);
            let mut dirty = Damage::new();
            if !Arc::ptr_eq(&cache.content, &layer.content) {
                if !kept.is_empty() {
                    let changed = diff_content(&cache.content, &layer.content);
                    for r in changed.rects() {
                        dirty.add(r.intersect(&kept));
                    }
                }
                cache.content = layer.content.clone();
                cache.bounds = layer
                    .content
                    .primitives
                    .iter()
                    .map(primitive_bounds)
                    .collect();
            }
            subtract(view, kept, &mut dirty);
            cache.valid = view;
            if !dirty.is_empty() {
                plans.push(LayerPlan {
                    key: layer.key,
                    dirty: dirty.rects().to_vec(),
                });
            }
        }
        self.layers.retain(|_, c| c.used);
        plans
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
        plans: &[LayerPlan],
    ) -> Prepared {
        loop {
            match self.try_prepare(scene, damage, plans) {
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
        plans: &[LayerPlan],
    ) -> Option<Prepared> {
        let mut out = Prepared {
            quads: Vec::new(),
            glyphs: Vec::new(),
            batches: Vec::new(),
            rasters: Vec::new(),
        };
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        let clear = scene.clear.premultiplied();
        for r in damage.unwrap_or_default() {
            out.quads.push(clear_instance(*r, clear));
        }

        let layers = std::mem::take(&mut self.layers);
        let rasters = plans.iter().map(|plan| {
            let cache = &layers[&plan.key];
            let mut ops = Vec::new();
            let target = RectI::new(0, 0, cache.size.0, cache.size.1);
            let mut failed = false;
            for dirty in &plan.dirty {
                torus_parts(*dirty, cache.size, |part, (tx, ty)| {
                    let scissor = RectI::new(tx, ty, part.width, part.height);
                    let index = out.quads.len() as u32;
                    out.quads.push(clear_instance(scissor, [0.0; 4]));
                    let mut batches = Vec::new();
                    for (prim, bounds) in cache.content.primitives.iter().zip(&cache.bounds) {
                        if !bounds.intersects(&part) {
                            continue;
                        }
                        let placed_ok = self.push_primitive(
                            prim,
                            (tx - part.x, ty - part.y),
                            target,
                            &mut out.quads,
                            &mut out.glyphs,
                            &mut batches,
                            &mut placed,
                        );
                        failed |= placed_ok.is_none();
                    }
                    ops.push(RasterOp {
                        scissor,
                        clear: index,
                        batches,
                    });
                });
            }
            (!failed).then_some((plan.key, ops))
        });
        let rasters: Option<Vec<_>> = rasters.collect();

        let screen = RectI::new(0, 0, scene.width as i32, scene.height as i32);
        let window = rasters.as_ref().and_then(|_| {
            for prim in &scene.primitives {
                let bounds = primitive_bounds(prim).intersect(&screen);
                if bounds.is_empty()
                    || damage.is_some_and(|d| !d.iter().any(|r| r.intersects(&bounds)))
                {
                    continue;
                }
                if let Primitive::Layer(layer) = prim {
                    if let Some(cache) = layers.get(&layer.key) {
                        push_layer(layer, cache, screen, &mut out.glyphs, &mut out.batches);
                    }
                    continue;
                }
                self.push_primitive(
                    prim,
                    (0, 0),
                    screen,
                    &mut out.quads,
                    &mut out.glyphs,
                    &mut out.batches,
                    &mut placed,
                )?;
            }
            Some(())
        });
        self.layers = layers;
        window?;
        out.rasters = rasters?;
        Some(out)
    }

    /// Adds the instances for a primitive, moved by `delta`, to a pass rendering into `target`.
    /// Returns `None` if the glyph atlas is full.
    #[allow(clippy::too_many_arguments)]
    fn push_primitive(
        &mut self,
        prim: &Primitive,
        delta: (i32, i32),
        target: RectI,
        quads: &mut Vec<QuadInstance>,
        glyphs: &mut Vec<GlyphInstance>,
        batches: &mut Vec<Batch>,
        placed: &mut Vec<PlacedGlyph>,
    ) -> Option<()> {
        let (dx, dy) = delta;
        match prim {
            Primitive::Quad(q) => {
                let (clip, clip_radii) = gpu_clip(&q.clip.translated(dx, dy), target);
                let b = q.border_widths;
                quads.push(QuadInstance {
                    rect: rect4(q.rect.translate(dx, dy)),
                    background: q.background.premultiplied(),
                    border_color: q.border_color.premultiplied(),
                    border: [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32],
                    radii: q.radii,
                    clip,
                    params: [0.0; 4],
                    clip_radii,
                });
                push_batch(batches, true, quads.len() as u32 - 1, quads.len() as u32);
            }
            Primitive::Shadow(s) => {
                let (clip, clip_radii) = gpu_clip(&s.clip.translated(dx, dy), target);
                quads.push(QuadInstance {
                    rect: rect4(s.rect.translate(dx, dy)),
                    background: s.color.premultiplied(),
                    border_color: [0.0; 4],
                    border: [0.0; 4],
                    radii: s.radii,
                    clip,
                    params: [1.0, s.blur, 0.0, 0.0],
                    clip_radii,
                });
                push_batch(batches, true, quads.len() as u32 - 1, quads.len() as u32);
            }
            Primitive::Glyphs(run) => {
                placed.clear();
                self.glyphs.place_run(run, placed)?;
                let color = run.color.premultiplied();
                let (clip, clip_radii) = gpu_clip(&run.clip.translated(dx, dy), target);
                let start = glyphs.len() as u32;
                for p in placed.iter() {
                    glyphs.push(GlyphInstance {
                        rect: [
                            p.rect[0] + dx as f32,
                            p.rect[1] + dy as f32,
                            p.rect[2],
                            p.rect[3],
                        ],
                        uv: p.uv,
                        color,
                        clip,
                        params: [if p.color { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                        clip_radii,
                    });
                }
                if !placed.is_empty() {
                    push_batch(batches, false, start, glyphs.len() as u32);
                }
            }
            // Layers don't nest; the painter draws inner scrollers as content.
            Primitive::Layer(_) => {}
        }
        Some(())
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
        let plans = self.plan_layers(device, scene, damage);
        let Prepared {
            quads,
            glyphs,
            batches,
            rasters,
        } = self.prepare(device, scene, damage, &plans);
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

        for (key, ops) in &rasters {
            let cache = &self.layers[key];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scroll layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &cache.globals_bind, &[]);
            for op in ops {
                let r = op.scissor;
                pass.set_scissor_rect(r.x as u32, r.y as u32, r.width as u32, r.height as u32);
                pass.set_pipeline(&self.clear_pipeline);
                pass.set_vertex_buffer(0, self.quad_buffer.buffer.slice(..));
                pass.draw(0..4, op.clear..op.clear + 1);
                self.draw_batches(&mut pass, &op.batches);
            }
        }

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
                Batch::Layer(key, r) => {
                    let Some(cache) = self.layers.get(key) else {
                        continue;
                    };
                    pass.set_pipeline(&self.glyph_pipeline);
                    pass.set_bind_group(1, &cache.bind, &[]);
                    pass.set_vertex_buffer(0, self.glyph_buffer.buffer.slice(..));
                    pass.draw(0..4, r.clone());
                }
            }
        }
    }
}

fn clear_instance(r: RectI, color: [f32; 4]) -> QuadInstance {
    QuadInstance {
        rect: rect4(r),
        background: color,
        border_color: [0.0; 4],
        border: [0.0; 4],
        radii: [0.0; 4],
        clip: rect4(r),
        params: [0.0; 4],
        clip_radii: [0.0; 4],
    }
}

/// Adds the textured quads that show a layer. They use the glyph pipeline as "color glyphs":
/// premultiplied texels, clipped in the shader.
fn push_layer(
    layer: &ScrollLayer,
    cache: &LayerCache,
    screen: RectI,
    glyphs: &mut Vec<GlyphInstance>,
    batches: &mut Vec<Batch>,
) {
    let visible = layer.clip.rect.intersect(&screen);
    let view = visible.translate(-layer.origin.x, -layer.origin.y);
    let (clip, clip_radii) = gpu_clip(&layer.clip, screen);
    let (w, h) = (cache.size.0 as f32, cache.size.1 as f32);
    let start = glyphs.len() as u32;
    torus_parts(view, cache.size, |part, (tx, ty)| {
        let (tx, ty) = (tx as f32, ty as f32);
        glyphs.push(GlyphInstance {
            rect: rect4(part.translate(layer.origin.x, layer.origin.y)),
            uv: [
                tx / w,
                ty / h,
                (tx + part.width as f32) / w,
                (ty + part.height as f32) / h,
            ],
            color: [1.0; 4],
            clip,
            params: [1.0, 0.0, 0.0, 0.0],
            clip_radii,
        });
    });
    batches.push(Batch::Layer(layer.key, start..glyphs.len() as u32));
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
