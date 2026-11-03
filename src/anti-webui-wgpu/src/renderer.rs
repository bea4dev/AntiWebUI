//! Scene → GPU draw calls.

use crate::glyph_cache::{GlyphCache, PlacedGlyph};
use anti_webui_core::damage::{Damage, diff_content, primitive_bounds};
use anti_webui_core::geometry::{Affine, RectI};
use anti_webui_core::scene::{Clip, Layer, Primitive, Quad, Scene};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use wgpu::util::DeviceExt;

const ATLAS_INITIAL: u32 = 1024;

/// A layer is never drawn bigger than this, however far it is scaled up.
const MAX_K: i32 = 4;
/// How big a rotated layer is drawn, so that filtering doesn't soften its text much.
const ROTATED_K: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// A rectangle of one premultiplied color on whole pixels.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SolidInstance {
    rect: [f32; 4],
    color: [f32; 4],
}

/// A box or part of a shadow; see the shader for what the fields mean for each.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadInstance {
    area: [f32; 4],
    shape: [f32; 4],
    background: [f32; 4],
    border_color: [f32; 4],
    border: [f32; 4],
    radii: [f32; 4],
    clip: [f32; 4],
    clip_radii: [f32; 4],
    cut_radii: [f32; 4],
    params: [f32; 2],
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

/// A part of a layer's texture shown through a transform.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerInstance {
    matrix: [f32; 4],
    offset: [f32; 2],
    size: [f32; 2],
    uv: [f32; 4],
    clip: [f32; 4],
    clip_radii: [f32; 4],
    opacity: f32,
}

enum Batch {
    Quads(Range<u32>),
    /// Solid instances, blended.
    Solids(Range<u32>),
    /// Solid instances of opaque colors, replacing what is there.
    Opaque(Range<u32>),
    Glyphs(Range<u32>),
    /// Layer instances that sample the texture of the layer with this key.
    Layer {
        key: u64,
        /// Texels don't land on pixels.
        linear: bool,
        range: Range<u32>,
    },
}

/// A texture holding a layer's content, `k` times bigger. A content pixel `(x, y)` lives at texel
/// `(kx mod width, ky mod height)`, so what is already drawn stays put as the layer scrolls in any
/// direction (see `docs/design/05-rendering.md`).
struct LayerCache {
    /// The target it is shown on, which decides when it is no longer in use.
    owner: u64,
    view: wgpu::TextureView,
    /// Binds the texture like the glyph atlas, for compositing.
    bind: wgpu::BindGroup,
    /// The same with bilinear filtering.
    bind_linear: wgpu::BindGroup,
    /// Globals for rendering into the texture.
    globals_bind: wgpu::BindGroup,
    size: (i32, i32),
    /// How many texels a content pixel takes. See `docs/design/12-transform.md`.
    k: i32,
    content: Arc<Scene>,
    /// Bounds of the content's primitives, to find those touching a rectangle quickly.
    bounds: Vec<RectI>,
    /// Which of the content's primitives are layers.
    nested: Vec<u32>,
    /// The content rectangle the texture holds.
    valid: RectI,
    used: bool,
}

/// One rectangle to redraw in a layer's texture.
struct RasterOp {
    scissor: RectI,
    /// Index of the solid instance clearing `scissor`.
    clear: u32,
    batches: Vec<Batch>,
}

/// What a frame redraws in one layer: from the plan, in content coordinates.
struct LayerPlan {
    key: u64,
    dirty: Vec<RectI>,
}

#[derive(Default)]
struct Instances {
    solids: Vec<SolidInstance>,
    quads: Vec<QuadInstance>,
    glyphs: Vec<GlyphInstance>,
    layers: Vec<LayerInstance>,
}

/// Where content goes in the texture being drawn: `k` times bigger, then moved.
#[derive(Clone, Copy)]
struct Placement {
    k: i32,
    dx: i32,
    dy: i32,
}

impl Placement {
    const NONE: Placement = Placement { k: 1, dx: 0, dy: 0 };
}

struct Prepared {
    instances: Instances,
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

/// The parts of `outer` outside of `hole`, which may stick out of it: at most four rectangles.
fn around_hole(outer: RectI, hole: RectI, mut f: impl FnMut(RectI)) {
    let hole = hole.intersect(&outer);
    if hole.is_empty() {
        f(outer);
        return;
    }
    let parts = [
        RectI::new(outer.x, outer.y, outer.width, hole.y - outer.y),
        RectI::new(
            outer.x,
            hole.bottom(),
            outer.width,
            outer.bottom() - hole.bottom(),
        ),
        RectI::new(outer.x, hole.y, hole.x - outer.x, hole.height),
        RectI::new(
            hole.right(),
            hole.y,
            outer.right() - hole.right(),
            hole.height,
        ),
    ];
    for part in parts {
        if !part.is_empty() {
            f(part);
        }
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

/// A box drawn in parts, so that most of its pixels skip the quad shader.
#[derive(Debug, PartialEq)]
enum Piece {
    /// One premultiplied color.
    Solid(RectI, [f32; 4]),
    /// Needs the quad shader, to draw this area of the box.
    Shaded(RectI),
}

/// How many pixels of one color make it worth drawing a box in parts.
const MIN_SOLID_PIXELS: i64 = 4096;

/// Splits a box into pieces that don't overlap and cover everything it draws. What the quad
/// shader computes for a solid piece is exactly its color, since the pixels are whole and away
/// from any corner (see `docs/design/11-gpu-fill.md`).
fn slice_quad(q: &Quad, rect: RectI, mut f: impl FnMut(Piece)) {
    let bg = q.background.premultiplied();
    let [top, right, bottom, left] = q.border_widths;
    let half = (rect.width as f32 * 0.5).min(rect.height as f32 * 0.5);
    let [tl, tr, br, bl] = q.radii.map(|r| r.min(half).max(0.0).ceil() as i32);
    let bordered = q.border_widths != [0; 4];
    let inner_top = top.max(tl).max(tr);
    let inner_right = right.max(tr).max(br);
    let inner_bottom = bottom.max(br).max(bl);
    let inner_left = left.max(tl).max(bl);
    let inner = rect.inset(inner_top, inner_right, inner_bottom, inner_left);
    if inner.is_empty() {
        f(Piece::Shaded(rect));
        return;
    }
    let mut solid = |r: RectI, color: [f32; 4]| {
        if !r.is_empty() && color != [0.0; 4] {
            f(Piece::Solid(r, color));
        }
    };
    let (x, y) = (rect.x, rect.y);
    let bands = [
        RectI::new(x, y, rect.width, inner.y - y),
        RectI::new(
            x,
            inner.bottom(),
            rect.width,
            rect.bottom() - inner.bottom(),
        ),
        RectI::new(x, inner.y, inner.x - x, inner.height),
        RectI::new(
            inner.right(),
            inner.y,
            rect.right() - inner.right(),
            inner.height,
        ),
    ];
    match (q.radii != [0.0; 4], bordered) {
        (false, false) => solid(rect, bg),
        (false, true) => {
            let c = q.border_color.premultiplied();
            let ring = std::array::from_fn(|i| c[i] + bg[i] * (1.0 - c[3]));
            solid(inner, bg);
            for band in bands {
                solid(band, ring);
            }
        }
        (true, false) => {
            let middle = RectI::new(x, inner.y, rect.width, inner.height);
            let tops = RectI::new(x + tl, y, rect.width - tl - tr, inner.y - y);
            let bottoms = RectI::new(
                x + bl,
                inner.bottom(),
                rect.width - bl - br,
                rect.bottom() - inner.bottom(),
            );
            if middle.area() + tops.area() + bottoms.area() < MIN_SOLID_PIXELS {
                f(Piece::Shaded(rect));
                return;
            }
            solid(middle, bg);
            solid(tops, bg);
            solid(bottoms, bg);
            for corner in [
                RectI::new(x, y, tl, tops.height),
                RectI::new(tops.right(), y, tr, tops.height),
                RectI::new(x, bottoms.y, bl, bottoms.height),
                RectI::new(bottoms.right(), bottoms.y, br, bottoms.height),
            ] {
                if !corner.is_empty() {
                    f(Piece::Shaded(corner));
                }
            }
        }
        (true, true) => {
            if inner.area() < MIN_SOLID_PIXELS {
                f(Piece::Shaded(rect));
                return;
            }
            solid(inner, bg);
            for band in bands {
                if !band.is_empty() {
                    f(Piece::Shaded(band));
                }
            }
        }
    }
}

/// Whether every pixel of `r` is fully inside the clip, so that it needs no clipping in the shader.
fn inside_clip_corners(r: RectI, clip: &Clip) -> bool {
    let inset = clip.radii.iter().fold(0.0_f32, |a, b| a.max(*b)).ceil() as i32;
    let safe = clip.rect.inset(inset, inset, inset, inset);
    r.x >= safe.x && r.y >= safe.y && r.right() <= safe.right() && r.bottom() <= safe.bottom()
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
    solid_pipeline: wgpu::RenderPipeline,
    /// The solid pipeline without blending, for opaque colors and clearing damaged rectangles.
    opaque_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    layer_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    globals_layout: wgpu::BindGroupLayout,
    atlas_layout: wgpu::BindGroupLayout,
    atlas: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    /// Wraps around, since a layer's texture does.
    linear_sampler: wgpu::Sampler,
    glyphs: GlyphCache,
    solid_buffer: InstanceBuffer,
    quad_buffer: InstanceBuffer,
    glyph_buffer: InstanceBuffer,
    layer_buffer: InstanceBuffer,
    max_texture: u32,
    layers: HashMap<u64, LayerCache>,
    /// The target being rendered to, when one renderer serves several.
    target: u64,
    /// Off: every box goes through the quad shader, to compare in tests.
    slice_quads: bool,
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
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
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
            4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4,
            9 => Float32x2
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
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
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
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        let solid_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let solid_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SolidInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &solid_attrs,
        })];
        let solid_pipeline_with = |label, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&quad_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_solid"),
                    compilation_options: Default::default(),
                    buffers: &solid_buffers,
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_solid"),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let solid_pipeline = solid_pipeline_with("solid", &targets);
        let opaque_pipeline = solid_pipeline_with(
            "opaque",
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
        let layer_attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4, 6 => Float32
        ];
        let layer_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LayerInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &layer_attrs,
        })];
        let layer_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer"),
            layout: Some(&glyph_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_layer"),
                compilation_options: Default::default(),
                buffers: &layer_buffers,
            },
            primitive,
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_layer"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            format,
            quad_pipeline,
            solid_pipeline,
            opaque_pipeline,
            glyph_pipeline,
            layer_pipeline,
            globals,
            globals_bind,
            globals_layout,
            atlas_layout,
            atlas,
            atlas_bind,
            sampler,
            linear_sampler,
            glyphs: GlyphCache::new(ATLAS_INITIAL),
            solid_buffer: InstanceBuffer::new(device, "solids"),
            quad_buffer: InstanceBuffer::new(device, "quads"),
            glyph_buffer: InstanceBuffer::new(device, "glyphs"),
            layer_buffer: InstanceBuffer::new(device, "layers"),
            max_texture: device.limits().max_texture_dimension_2d,
            layers: HashMap::new(),
            target: 0,
            slice_quads: true,
        }
    }

    /// Selects what the next frames are for. The layer textures of other targets stay as they
    /// are, however many frames go by.
    pub fn set_target(&mut self, target: u64) {
        self.target = target;
    }

    /// Frees what was kept for a target that is gone.
    pub fn forget_target(&mut self, target: u64) {
        self.layers.retain(|_, c| c.owner != target);
    }

    /// Sends every box through the quad shader, which any drawing must match pixel for pixel.
    #[doc(hidden)]
    pub fn set_slice_quads(&mut self, on: bool) {
        self.slice_quads = on;
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
        layer: &Layer,
        size: (i32, i32),
        k: i32,
    ) -> LayerCache {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer"),
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
        let bind_with = |sampler: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("layer"),
                layout: &self.atlas_layout,
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
            })
        };
        let bind = bind_with(&self.sampler);
        let bind_linear = bind_with(&self.linear_sampler);
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("layer globals"),
            contents: bytemuck::bytes_of(&Globals {
                viewport: [size.0 as f32, size.1 as f32],
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer globals"),
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let (bounds, nested) = index_content(&layer.content);
        LayerCache {
            owner: self.target,
            view,
            bind,
            bind_linear,
            globals_bind,
            size,
            k,
            content: layer.content.clone(),
            bounds,
            nested,
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
        let target = self.target;
        for cache in self.layers.values_mut().filter(|c| c.owner == target) {
            cache.used = false;
        }
        let mut plans = Vec::new();
        for prim in &scene.primitives {
            if let Primitive::Layer(layer) = prim {
                self.plan_layer(device, layer, screen, damage, 1, &mut plans);
            }
        }
        self.layers.retain(|_, c| c.used || c.owner != target);
        plans
    }

    /// Plans a layer in a scene that shows `region` and redraws `within`, at `parent_k` texels per
    /// pixel. What is inside of it comes first in `plans`: it has to be drawn before it is used.
    fn plan_layer(
        &mut self,
        device: &wgpu::Device,
        layer: &Layer,
        region: RectI,
        within: Option<&[RectI]>,
        parent_k: i32,
        plans: &mut Vec<LayerPlan>,
    ) {
        let shown = layer.bounds_on_parent().intersect(&region);
        let matrix = layer.matrix();
        let inverse = matrix.inverse();
        let Some(inverse) = inverse.filter(|_| {
            !shown.is_empty() && within.is_none_or(|d| d.iter().any(|r| r.intersects(&shown)))
        }) else {
            self.keep_layer(layer.key);
            return;
        };
        let view = if layer.is_pixel_aligned() {
            shown.translate(-matrix.e.round() as i32, -matrix.f.round() as i32)
        } else {
            // The pixel around it stays transparent: filtering fades the edge into it.
            inverse
                .bounds_of(shown)
                .outset(1)
                .intersect(&layer.bounds.outset(1))
        };
        let wanted = (matrix.max_scale() * parent_k as f64 - 0.01).ceil() as i32;
        let wanted = if matrix.is_axis_aligned() {
            wanted
        } else {
            wanted.max(ROTATED_K)
        };
        let k = self
            .layers
            .get(&layer.key)
            .map_or(1, |c| c.k)
            .max(wanted)
            .clamp(1, MAX_K);
        // A bigger texture isn't possible: rather than failing, show less sharp.
        let largest = view.width.max(view.height).max(1) as i64;
        let k = k.min((self.max_texture as i64 / largest).max(1) as i32);
        // Round up so that small resizes keep the texture, and shrink only when it is far
        // too big.
        let needed = (
            ((view.width * k + 127) & !127).min(self.max_texture as i32),
            ((view.height * k + 127) & !127).min(self.max_texture as i32),
        );
        let fits = self.layers.get(&layer.key).is_some_and(|c| {
            c.k == k
                && c.size.0 >= needed.0
                && c.size.1 >= needed.1
                && (c.size.0 as i64 * c.size.1 as i64) <= 2 * needed.0 as i64 * needed.1 as i64
        });
        if !fits {
            let cache = self.create_layer_cache(device, layer, needed, k);
            self.layers.insert(layer.key, cache);
        }
        let cache = self.layers.get_mut(&layer.key).expect("just inserted");
        cache.used = true;

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
            (cache.bounds, cache.nested) = index_content(&layer.content);
        }
        subtract(view, kept, &mut dirty);
        cache.valid = view;
        let dirty = dirty.rects().to_vec();
        let nested = cache.nested.clone();
        for i in nested {
            if let Primitive::Layer(inner) = &layer.content.primitives[i as usize] {
                self.plan_layer(device, inner, view, Some(&dirty), k, plans);
            }
        }
        if !dirty.is_empty() {
            plans.push(LayerPlan {
                key: layer.key,
                dirty,
            });
        }
    }

    /// Marks a layer and what is inside of it as still in use, although not touched this frame.
    fn keep_layer(&mut self, key: u64) {
        let Some(cache) = self.layers.get_mut(&key) else {
            return;
        };
        cache.used = true;
        let content = cache.content.clone();
        let nested = cache.nested.clone();
        for i in nested {
            if let Primitive::Layer(inner) = &content.primitives[i as usize] {
                self.keep_layer(inner.key);
            }
        }
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
            instances: Instances::default(),
            batches: Vec::new(),
            rasters: Vec::new(),
        };
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        let clear = scene.clear.premultiplied();
        for r in damage.unwrap_or_default() {
            out.instances.solids.push(solid_instance(*r, clear));
        }

        let layers = std::mem::take(&mut self.layers);
        let rasters = plans.iter().map(|plan| {
            let cache = &layers[&plan.key];
            let mut ops = Vec::new();
            let target = RectI::new(0, 0, cache.size.0, cache.size.1);
            let k = cache.k;
            let mut failed = false;
            for dirty in &plan.dirty {
                torus_parts(dirty.scaled(k), cache.size, |part, (tx, ty)| {
                    let scissor = RectI::new(tx, ty, part.width, part.height);
                    let index = out.instances.solids.len() as u32;
                    out.instances.solids.push(solid_instance(scissor, [0.0; 4]));
                    let mut batches = Vec::new();
                    let at = Placement {
                        k,
                        dx: tx - part.x,
                        dy: ty - part.y,
                    };
                    let touched = RectI::new(
                        part.x.div_euclid(k),
                        part.y.div_euclid(k),
                        part.width / k + 2,
                        part.height / k + 2,
                    );
                    for (prim, bounds) in cache.content.primitives.iter().zip(&cache.bounds) {
                        if !bounds.intersects(&touched) {
                            continue;
                        }
                        let placed_ok = self.push_primitive(
                            prim,
                            at,
                            target,
                            &layers,
                            &mut out.instances,
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
                self.push_primitive(
                    prim,
                    Placement::NONE,
                    screen,
                    &layers,
                    &mut out.instances,
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

    /// Adds the instances for a primitive, placed by `at`, to a pass rendering into `target`.
    /// Returns `None` if the glyph atlas is full.
    #[allow(clippy::too_many_arguments)]
    fn push_primitive(
        &mut self,
        prim: &Primitive,
        at: Placement,
        target: RectI,
        layers: &HashMap<u64, LayerCache>,
        instances: &mut Instances,
        batches: &mut Vec<Batch>,
        placed: &mut Vec<PlacedGlyph>,
    ) -> Option<()> {
        let Placement { k, dx, dy } = at;
        let Instances {
            solids,
            quads,
            glyphs,
            layers: layer_instances,
        } = instances;
        match prim {
            Primitive::Quad(q) => {
                let scaled;
                let q = if k == 1 {
                    q
                } else {
                    scaled = q.scaled(k);
                    &scaled
                };
                let rect = q.rect.translate(dx, dy);
                let clip = q.clip.translated(dx, dy);
                let visible = rect.intersect(&clip.rect).intersect(&target);
                if visible.is_empty() {
                    return Some(());
                }
                let (gpu_clip, clip_radii) = gpu_clip(&clip, target);
                let b = q.border_widths;
                let shape = QuadInstance {
                    area: rect4(visible),
                    shape: rect4(rect),
                    background: q.background.premultiplied(),
                    border_color: q.border_color.premultiplied(),
                    border: [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32],
                    radii: q.radii,
                    clip: gpu_clip,
                    clip_radii,
                    cut_radii: [0.0; 4],
                    params: [0.0; 2],
                };
                if !self.slice_quads || (clip.is_rounded() && !inside_clip_corners(visible, &clip))
                {
                    quads.push(shape);
                    push_batch(batches, BatchKind::Quads, quads.len() as u32);
                    return Some(());
                }
                slice_quad(q, rect, |piece| match piece {
                    Piece::Solid(r, color) => {
                        let r = r.intersect(&visible);
                        if !r.is_empty() {
                            solids.push(solid_instance(r, color));
                            let kind = if color[3] >= 1.0 {
                                BatchKind::Opaque
                            } else {
                                BatchKind::Solids
                            };
                            push_batch(batches, kind, solids.len() as u32);
                        }
                    }
                    Piece::Shaded(r) => {
                        let r = r.intersect(&visible);
                        if !r.is_empty() {
                            quads.push(QuadInstance {
                                area: rect4(r),
                                ..shape
                            });
                            push_batch(batches, BatchKind::Quads, quads.len() as u32);
                        }
                    }
                });
            }
            Primitive::Shadow(s) => {
                let scaled;
                let s = if k == 1 {
                    s
                } else {
                    scaled = s.scaled(k);
                    &scaled
                };
                let (clip, clip_radii) = gpu_clip(&s.clip.translated(dx, dy), target);
                let visible = s
                    .bounds()
                    .translate(dx, dy)
                    .intersect(&s.clip.translated(dx, dy).rect)
                    .intersect(&target);
                around_hole(visible, s.hole().translate(dx, dy), |area| {
                    quads.push(QuadInstance {
                        area: rect4(area),
                        shape: rect4(s.rect.translate(dx, dy)),
                        background: s.color.premultiplied(),
                        border_color: [0.0; 4],
                        border: rect4(s.cut.translate(dx, dy)),
                        radii: s.radii,
                        clip,
                        clip_radii,
                        cut_radii: s.cut_radii,
                        params: [if s.inset { 2.0 } else { 1.0 }, s.blur * 0.5],
                    });
                    push_batch(batches, BatchKind::Quads, quads.len() as u32);
                });
            }
            Primitive::Glyphs(run) => {
                placed.clear();
                self.glyphs.place_run(run, k, placed)?;
                let color = run.color.premultiplied();
                let (clip, clip_radii) = gpu_clip(&run.clip.scaled(k).translated(dx, dy), target);
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
                    push_batch(batches, BatchKind::Glyphs, glyphs.len() as u32);
                }
            }
            Primitive::Layer(layer) => {
                if let Some(cache) = layers.get(&layer.key) {
                    push_layer(layer, cache, at, target, layer_instances, batches);
                }
            }
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
            instances,
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
        self.solid_buffer
            .upload(device, queue, bytemuck::cast_slice(&instances.solids));
        self.quad_buffer
            .upload(device, queue, bytemuck::cast_slice(&instances.quads));
        self.glyph_buffer
            .upload(device, queue, bytemuck::cast_slice(&instances.glyphs));
        self.layer_buffer
            .upload(device, queue, bytemuck::cast_slice(&instances.layers));

        for (key, ops) in &rasters {
            let cache = &self.layers[key];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layer"),
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
                pass.set_pipeline(&self.opaque_pipeline);
                pass.set_vertex_buffer(0, self.solid_buffer.buffer.slice(..));
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
            pass.set_pipeline(&self.opaque_pipeline);
            pass.set_vertex_buffer(0, self.solid_buffer.buffer.slice(..));
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
                Batch::Solids(r) => {
                    pass.set_pipeline(&self.solid_pipeline);
                    pass.set_vertex_buffer(0, self.solid_buffer.buffer.slice(..));
                    pass.draw(0..4, r.clone());
                }
                Batch::Opaque(r) => {
                    pass.set_pipeline(&self.opaque_pipeline);
                    pass.set_vertex_buffer(0, self.solid_buffer.buffer.slice(..));
                    pass.draw(0..4, r.clone());
                }
                Batch::Glyphs(r) => {
                    pass.set_pipeline(&self.glyph_pipeline);
                    pass.set_bind_group(1, &self.atlas_bind, &[]);
                    pass.set_vertex_buffer(0, self.glyph_buffer.buffer.slice(..));
                    pass.draw(0..4, r.clone());
                }
                Batch::Layer { key, linear, range } => {
                    let Some(cache) = self.layers.get(key) else {
                        continue;
                    };
                    pass.set_pipeline(&self.layer_pipeline);
                    let bind = if *linear {
                        &cache.bind_linear
                    } else {
                        &cache.bind
                    };
                    pass.set_bind_group(1, bind, &[]);
                    pass.set_vertex_buffer(0, self.layer_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
            }
        }
    }
}

fn solid_instance(r: RectI, color: [f32; 4]) -> SolidInstance {
    SolidInstance {
        rect: rect4(r),
        color,
    }
}

/// Adds the textured quads that show what a layer's texture holds.
fn push_layer(
    layer: &Layer,
    cache: &LayerCache,
    at: Placement,
    target: RectI,
    instances: &mut Vec<LayerInstance>,
    batches: &mut Vec<Batch>,
) {
    let (clip, clip_radii) = gpu_clip(&layer.clip.scaled(at.k).translated(at.dx, at.dy), target);
    let (k, parent) = (cache.k as f64, at.k as f64);
    // From texels to the target.
    let m = Affine::translation(at.dx as f64, at.dy as f64)
        .then_apply(&Affine::scaling(parent, parent))
        .then_apply(&layer.matrix())
        .then_apply(&Affine::scaling(1.0 / k, 1.0 / k));
    let (w, h) = (cache.size.0 as f32, cache.size.1 as f32);
    let start = instances.len() as u32;
    torus_parts(cache.valid.scaled(cache.k), cache.size, |part, (tx, ty)| {
        let (tx, ty) = (tx as f32, ty as f32);
        let (x, y) = m.apply(part.x as f64, part.y as f64);
        instances.push(LayerInstance {
            matrix: [m.a as f32, m.b as f32, m.c as f32, m.d as f32],
            offset: [x as f32, y as f32],
            size: [part.width as f32, part.height as f32],
            uv: [
                tx / w,
                ty / h,
                (tx + part.width as f32) / w,
                (ty + part.height as f32) / h,
            ],
            clip,
            clip_radii,
            opacity: layer.opacity,
        });
    });
    batches.push(Batch::Layer {
        key: layer.key,
        linear: !m.is_whole_translation(),
        range: start..instances.len() as u32,
    });
}

/// The bounds of a layer's primitives, and which of them are layers.
fn index_content(content: &Scene) -> (Vec<RectI>, Vec<u32>) {
    let bounds = content.primitives.iter().map(primitive_bounds).collect();
    let nested = content
        .primitives
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p, Primitive::Layer(_)))
        .map(|(i, _)| i as u32)
        .collect();
    (bounds, nested)
}

#[derive(Clone, Copy)]
enum BatchKind {
    Quads,
    Solids,
    Opaque,
    Glyphs,
}

/// Adds the instance just pushed, which ends at `end`, to the last batch if it is of that kind.
fn push_batch(batches: &mut Vec<Batch>, kind: BatchKind, end: u32) {
    match (batches.last_mut(), kind) {
        (Some(Batch::Quads(r)), BatchKind::Quads)
        | (Some(Batch::Solids(r)), BatchKind::Solids)
        | (Some(Batch::Opaque(r)), BatchKind::Opaque)
        | (Some(Batch::Glyphs(r)), BatchKind::Glyphs) => r.end = end,
        _ => {
            let r = end - 1..end;
            batches.push(match kind {
                BatchKind::Quads => Batch::Quads(r),
                BatchKind::Solids => Batch::Solids(r),
                BatchKind::Opaque => Batch::Opaque(r),
                BatchKind::Glyphs => Batch::Glyphs(r),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(rect: RectI, border: [i32; 4], radii: [f32; 4], bg: f32) -> Quad {
        Quad {
            rect,
            background: anti_webui_core::prelude::Color::new(0.2, 0.4, 0.8, bg),
            border_widths: border,
            border_color: anti_webui_core::prelude::Color::new(1.0, 0.0, 0.0, 0.5),
            radii,
            clip: Clip::NONE,
        }
    }

    #[test]
    fn a_box_is_cut_into_pieces_that_cover_it_once() {
        let radii = [
            [0.0; 4],
            [12.0; 4],
            [1.5; 4],
            [0.0, 30.0, 0.0, 5.0],
            [40.0, 0.0, 0.0, 0.0],
            [1000.0; 4],
        ];
        let borders = [[0; 4], [1; 4], [3, 0, 7, 2], [50, 50, 50, 50]];
        for (w, h) in [(300, 200), (200, 300), (64, 64), (10, 200), (2, 2)] {
            let rect = RectI::new(7, 11, w, h);
            for radii in radii {
                for border in borders {
                    let q = quad(rect, border, radii, 0.5);
                    let mut count = vec![0_u32; (w * h) as usize];
                    slice_quad(&q, rect, |piece| {
                        let (Piece::Solid(r, _) | Piece::Shaded(r)) = piece;
                        for y in r.y..r.bottom() {
                            for x in r.x..r.right() {
                                let i = (y - rect.y) * w + (x - rect.x);
                                count[i as usize] += 1;
                            }
                        }
                    });
                    assert!(
                        count.iter().all(|c| *c == 1),
                        "{w}x{h} radii {radii:?} border {border:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_small_rounded_box_is_not_worth_cutting() {
        let rect = RectI::new(0, 0, 40, 40);
        let mut n = 0;
        slice_quad(&quad(rect, [0; 4], [8.0; 4], 0.5), rect, |_| n += 1);
        assert_eq!(n, 1);
    }

    #[test]
    fn a_box_without_a_background_has_no_middle() {
        let rect = RectI::new(0, 0, 400, 300);
        let mut pieces = Vec::new();
        slice_quad(&quad(rect, [2; 4], [0.0; 4], 0.0), rect, |p| pieces.push(p));
        assert_eq!(pieces.len(), 4);
        assert!(
            !pieces
                .iter()
                .any(|p| matches!(p, Piece::Solid(r, _) if r.area() > 10000))
        );
    }

    fn pieces(outer: RectI, hole: RectI) -> Vec<RectI> {
        let mut out = Vec::new();
        around_hole(outer, hole, |r| out.push(r));
        out
    }

    #[test]
    fn the_area_around_a_hole_covers_the_rest_once() {
        let outer = RectI::new(10, 10, 100, 80);
        for hole in [
            RectI::new(30, 30, 40, 20),
            RectI::new(0, 0, 40, 40),
            RectI::new(90, 70, 100, 100),
            RectI::new(10, 10, 100, 80),
            RectI::new(0, 40, 500, 10),
            RectI::new(300, 300, 10, 10),
            RectI::new(30, 30, 0, 20),
        ] {
            let parts = pieces(outer, hole);
            assert!(parts.len() <= 4);
            for y in 0..120 {
                for x in 0..130 {
                    let p = anti_webui_core::geometry::PointF::new(x as f32, y as f32);
                    let n = parts.iter().filter(|r| r.contains(p)).count();
                    let want = outer.contains(p) && !hole.contains(p);
                    assert_eq!(n, want as usize, "{hole:?} at {x},{y}");
                }
            }
        }
    }
}
