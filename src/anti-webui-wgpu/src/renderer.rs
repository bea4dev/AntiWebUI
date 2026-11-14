//! Scene → GPU draw calls.

use crate::custom_shader::{self, CHUNK, CompiledShader, ShaderInstance};
use crate::effects::{
    self, BackdropInstance, EffectPipelines, Effects, EvalContext, INPUTS_SIZE, Missing,
    PoolContext, Slot, Source, Step, UNIFORM_STRIDE,
};
use crate::glyph_cache::{GlyphCache, PlacedGlyph};
use crate::path::{PathInstance, PathPass, TableRow};
use crate::shader_cache::ShaderCache;
use crate::video::{VideoInstance, VideoPass};
use anti_webui_core::damage::{
    Damage, backdrop_read, diff_content, expand_for_backdrops, primitive_bounds,
};
use anti_webui_core::geometry::{Affine, RectI};
use anti_webui_core::images::Bitmap;
use anti_webui_core::scene::{
    CanvasDraw, Clip, ImageDraw, Layer, LayerFilter, Primitive, Quad, Scene, VideoDraw,
};
use anti_webui_core::shader::{ShaderSource, preloads_from};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Weak};
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

/// A picture on its way to the screen.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PictureInstance {
    area: [f32; 4],
    rect: [f32; 4],
    shape: [f32; 4],
    shape_radii: [f32; 4],
    clip: [f32; 4],
    clip_radii: [f32; 4],
    params: [f32; 2],
}

/// A picture on the GPU, for as long as anything has its pixels.
struct GpuPicture {
    bitmap: Weak<Bitmap>,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    /// The first level of the bitmap in the texture: only those the GPU can take are.
    first: u32,
    /// The frame of an animation the texture holds.
    frame: u32,
}

enum Batch {
    Quads(Range<u32>),
    /// Solid instances, blended.
    Solids(Range<u32>),
    /// Solid instances of opaque colors, replacing what is there.
    Opaque(Range<u32>),
    Glyphs(Range<u32>),
    /// Rectangles of shapes.
    Paths(Range<u32>),
    /// Layer instances that sample the texture of the layer with this key.
    Layer {
        key: u64,
        /// Texels don't land on pixels.
        linear: bool,
        /// From what a filter made of it rather than the texture itself.
        filtered: bool,
        range: Range<u32>,
    },
    /// Boxes painted by the shader with this key, whose parameters are in this chunk.
    Shader {
        key: usize,
        chunk: u32,
        /// Which of the frame's bindings of inputs it reads.
        inputs: Option<u32>,
        range: Range<u32>,
    },
    /// What is behind a box, which is in this texture of the pool.
    Backdrop {
        tex: usize,
        range: Range<u32>,
    },
    /// Instances of the picture with this id.
    Picture {
        id: u64,
        sampler: u8,
        opaque: bool,
        range: Range<u32>,
    },
    /// Instances of the current frame of the stream with this id.
    Video {
        stream: u64,
        range: Range<u32>,
    },
    /// Picture instances of what the `gpu_canvas` with this id drew.
    Canvas {
        id: u64,
        range: Range<u32>,
    },
}

/// What a `gpu_canvas` is drawn into. See `docs/design/41-gpu-canvas.md`.
struct CanvasTexture {
    view: wgpu::TextureView,
    srgb_view: Option<wgpu::TextureView>,
    bind: wgpu::BindGroup,
    size: (u32, u32),
    /// The version of the canvas it is to show, once drawn.
    version: Option<u64>,
    /// Whether what it holds is to be shown: drawing it didn't fail.
    shown: bool,
}

/// What a filter made of a layer, kept until that changes.
struct FilterOutput {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    bind_linear: wgpu::BindGroup,
    view: wgpu::TextureView,
    size: (i32, i32),
    /// What it holds, in texels of the layer. Starts at its first texel.
    rect: RectI,
    /// The filter that made it, and whether what it holds is up to date.
    made_by: Option<Box<LayerFilter>>,
}

/// Where a window is drawn when something reads what is drawn: the surface can't be read. See
/// `docs/design/16-filters.md`.
struct Canvas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    size: (u32, u32),
}

/// What a primitive was given by the pictures made for it.
#[derive(Clone, Copy)]
enum PrimEval {
    Backdrop(Option<Slot>),
    Inputs(u32),
}

/// The pictures to make in a target. Drawing stops before each primitive that needs some: `segment`
/// is what comes after making them.
#[derive(Default)]
struct TargetEffects {
    by_primitive: HashMap<usize, (usize, PrimEval)>,
    steps: Vec<Vec<Step>>,
}

/// A texture holding a layer's content, `k` times bigger. A content pixel `(x, y)` lives at texel
/// `(kx mod width, ky mod height)`, so what is already drawn stays put as the layer scrolls in any
/// direction (see `docs/design/05-rendering.md`).
struct LayerCache {
    /// The target it is shown on, which decides when it is no longer in use.
    owner: u64,
    texture: wgpu::Texture,
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
    output: Option<FilterOutput>,
}

/// One rectangle to redraw in a layer's texture.
struct RasterOp {
    scissor: RectI,
    /// Index of the solid instance clearing `scissor`.
    clear: u32,
    /// What to draw before the first picture is made, and after each one.
    segments: Vec<Vec<Batch>>,
}

/// What a frame draws in a layer's texture.
struct Raster {
    key: u64,
    ops: Vec<RasterOp>,
    effects: Vec<Vec<Step>>,
    /// What a filter makes of the layer, afterwards.
    filter: Vec<Step>,
}

/// What a frame redraws in one layer: from the plan, in content coordinates.
struct LayerPlan {
    key: u64,
    dirty: Vec<RectI>,
    /// What to make of it: it changed, or the filter did.
    filter: Option<Box<LayerFilter>>,
}

#[derive(Default)]
struct Instances {
    solids: Vec<SolidInstance>,
    quads: Vec<QuadInstance>,
    glyphs: Vec<GlyphInstance>,
    layers: Vec<LayerInstance>,
    shaders: Vec<ShaderInstance>,
    backdrops: Vec<BackdropInstance>,
    pictures: Vec<PictureInstance>,
    videos: Vec<VideoInstance>,
    paths: Vec<PathInstance>,
    path_table: Vec<TableRow>,
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
    /// What to draw in the window before the first picture is made, and after each one.
    segments: Vec<Vec<Batch>>,
    effects: Vec<Vec<Step>>,
    /// Redraws per layer.
    rasters: Vec<Raster>,
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
    picture_pipeline: wgpu::RenderPipeline,
    opaque_picture_pipeline: wgpu::RenderPipeline,
    picture_layout: wgpu::BindGroupLayout,
    /// For each of: square pixels, repeating across, repeating down.
    picture_samplers: Vec<wgpu::BindGroup>,
    pictures: HashMap<u64, GpuPicture>,
    /// What to hand to the GPU before drawing.
    picture_uploads: Vec<(Arc<Bitmap>, u32)>,
    /// Let go of the pixels of a picture once it is on the GPU.
    release_pixels: bool,
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
    shader_buffer: InstanceBuffer,
    backdrop_buffer: InstanceBuffer,
    picture_buffer: InstanceBuffer,
    video: VideoPass,
    video_buffer: InstanceBuffer,
    paths: PathPass,
    path_buffer: InstanceBuffer,
    /// Shaders written by users, by the address of their source.
    shaders: HashMap<usize, CompiledShader>,
    /// Compiled shaders saved for the next launch.
    shader_cache: Option<Arc<ShaderCache>>,
    /// What the frame being prepared needs and isn't compiled yet.
    missing_shader: Option<&'static ShaderSource>,
    /// How many of the shaders asked to be compiled ahead were seen to.
    preloaded: usize,
    shader_params_layout: wgpu::BindGroupLayout,
    /// What shaders read. Shaders without inputs read a transparent texture.
    inputs_layout: wgpu::BindGroupLayout,
    no_inputs: wgpu::BindGroup,
    empty_view: wgpu::TextureView,
    empty_source: wgpu::BindGroup,
    /// Doesn't go past the edge of what it reads.
    clamp_sampler: wgpu::Sampler,
    effects: Effects,
    effect_pipelines: Option<EffectPipelines>,
    /// A frame being prepared needs them.
    missing_effects: bool,
    effect_uniform_layout: wgpu::BindGroupLayout,
    effect_uniforms: Option<wgpu::Buffer>,
    effect_uniform_bind: Option<wgpu::BindGroup>,
    /// Windows that something reads, by target.
    canvases: HashMap<u64, Canvas>,
    /// What each `gpu_canvas` is drawn into, by its id.
    gpu_canvases: HashMap<u64, CanvasTexture>,
    /// The canvases to draw before this frame, with how many pixels make a logical one.
    canvas_draws: Vec<(u64, f32)>,
    /// Whether textures can be seen as sRGB, which OpenGL can't.
    srgb_views: bool,
    filters_run: usize,
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
        Self::with_cache(device, format, None)
    }

    /// With a cache of compiled shaders, which it saves after compiling more.
    pub(crate) fn with_cache(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shader_cache: Option<Arc<ShaderCache>>,
    ) -> Self {
        let pipeline_cache = shader_cache.as_ref().map(|c| &c.cache);
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
            cache: pipeline_cache,
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
                cache: pipeline_cache,
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
            cache: pipeline_cache,
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
            cache: pipeline_cache,
        });

        let picture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("picture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let picture_sampler_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("picture sampler"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                }],
            });
        let picture_samplers = (0..8u8)
            .map(|i| {
                let filter = if i & 1 != 0 {
                    wgpu::FilterMode::Nearest
                } else {
                    wgpu::FilterMode::Linear
                };
                let address = |repeats: bool| {
                    if repeats {
                        wgpu::AddressMode::Repeat
                    } else {
                        wgpu::AddressMode::ClampToEdge
                    }
                };
                let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("picture"),
                    address_mode_u: address(i & 2 != 0),
                    address_mode_v: address(i & 4 != 0),
                    mag_filter: filter,
                    min_filter: filter,
                    mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                    ..Default::default()
                });
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("picture sampler"),
                    layout: &picture_sampler_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    }],
                })
            })
            .collect();
        let picture_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("picture"),
                bind_group_layouts: &[
                    Some(&globals_layout),
                    Some(&picture_layout),
                    Some(&picture_sampler_layout),
                ],
                immediate_size: 0,
            });
        let picture_attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4, 6 => Float32x2
        ];
        let picture_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PictureInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &picture_attrs,
        })];
        let picture_pipeline_with = |label, entry, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&picture_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_picture"),
                    compilation_options: Default::default(),
                    buffers: &picture_buffers,
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: pipeline_cache,
            })
        };
        let picture_pipeline = picture_pipeline_with("picture", "fs_picture", &targets);
        let opaque_picture_pipeline = picture_pipeline_with(
            "opaque picture",
            "fs_opaque_picture",
            &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        );

        let inputs_layout = effects::inputs_layout(device);
        let effect_uniform_layout = effects::uniform_layout(device);
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("effects"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let empty = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nothing"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let empty_view = empty.create_view(&wgpu::TextureViewDescriptor::default());
        let empty_source = effects::source_bind(device, &atlas_layout, &empty_view, &clamp_sampler);
        let no_inputs_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("no inputs"),
            size: INPUTS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let no_inputs = inputs_bind(
            device,
            &inputs_layout,
            [&empty_view; 4],
            &clamp_sampler,
            &no_inputs_uniform,
        );

        if let Some(c) = &shader_cache {
            c.save_soon();
        }
        Self {
            format,
            quad_pipeline,
            solid_pipeline,
            opaque_pipeline,
            glyph_pipeline,
            layer_pipeline,
            picture_pipeline,
            opaque_picture_pipeline,
            picture_layout,
            picture_samplers,
            pictures: HashMap::new(),
            picture_uploads: Vec::new(),
            release_pixels: false,
            globals,
            globals_bind,
            paths: PathPass::new(
                globals_layout.clone(),
                format,
                blend,
                shader.clone(),
                device.limits().max_texture_dimension_2d,
            )
            .with_shader_cache(shader_cache.clone()),
            video: VideoPass::new(globals_layout.clone(), format, blend, shader.clone()),
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
            shader_buffer: InstanceBuffer::new(device, "shaders"),
            backdrop_buffer: InstanceBuffer::new(device, "backdrops"),
            picture_buffer: InstanceBuffer::new(device, "pictures"),
            video_buffer: InstanceBuffer::new(device, "videos"),
            path_buffer: InstanceBuffer::new(device, "paths"),
            shaders: HashMap::new(),
            shader_cache,
            missing_shader: None,
            preloaded: 0,
            shader_params_layout: custom_shader::params_layout(device),
            inputs_layout,
            no_inputs,
            empty_view,
            empty_source,
            clamp_sampler,
            effects: Effects::default(),
            effect_pipelines: None,
            missing_effects: false,
            effect_uniform_layout,
            effect_uniforms: None,
            effect_uniform_bind: None,
            canvases: HashMap::new(),
            gpu_canvases: HashMap::new(),
            canvas_draws: Vec::new(),
            srgb_views: device.adapter_info().backend != wgpu::Backend::Gl,
            filters_run: 0,
            max_texture: device.limits().max_texture_dimension_2d,
            layers: HashMap::new(),
            target: 0,
            slice_quads: true,
        }
    }

    /// Selects what the next frames are for. The layer textures of other targets stay as they
    /// are, however many frames go by.
    #[doc(hidden)]
    pub fn set_target(&mut self, target: u64) {
        self.target = target;
    }

    /// Frees what was kept for a target that is gone.
    #[doc(hidden)]
    pub fn forget_target(&mut self, target: u64) {
        self.layers.retain(|_, c| c.owner != target);
        self.canvases.remove(&target);
    }

    /// Lets go of the pixels of each picture once they are on the GPU, which is right when this is
    /// the only renderer that draws them. They are decoded again if anything needs them.
    pub fn set_release_picture_pixels(&mut self, on: bool) {
        self.release_pixels = on;
    }

    /// Lets frames of videos be read where the decoder left them.
    #[cfg(target_os = "linux")]
    #[doc(hidden)]
    pub fn set_video_importer(&mut self, importer: crate::dmabuf::VideoImporter) {
        self.video.set_importer(importer);
    }

    /// How many `gpu_canvas` textures are kept, for tests.
    #[doc(hidden)]
    pub fn canvas_textures(&self) -> usize {
        self.gpu_canvases.len()
    }

    /// How much the edges of shapes take, in bytes.
    #[doc(hidden)]
    pub fn path_store_used(&self) -> u32 {
        self.paths.store_used()
    }

    /// A frame of a video couldn't be read where it is, since this was last asked. Frames have to
    /// come in memory then: see [`anti_webui_core::Ui::video_use_memory`].
    #[doc(hidden)]
    pub fn take_video_import_failed(&mut self) -> bool {
        self.video.take_import_failed()
    }

    /// How many videos are on the GPU, and how many frames were read where they are, for tests.
    #[doc(hidden)]
    pub fn video_counts(&self) -> (usize, u64) {
        self.video.counts()
    }

    /// How many pictures are on the GPU, and what their textures take, for tests.
    #[doc(hidden)]
    pub fn picture_counts(&self) -> (usize, u64) {
        (
            self.pictures.len(),
            self.pictures
                .values()
                .map(|p| {
                    let size = p.texture.size();
                    let base = size.width as u64 * size.height as u64 * 4;
                    if p.texture.mip_level_count() > 1 {
                        base * 4 / 3
                    } else {
                        base
                    }
                })
                .sum(),
        )
    }

    /// Sends every box through the quad shader, which any drawing must match pixel for pixel.
    #[doc(hidden)]
    pub fn set_slice_quads(&mut self, on: bool) {
        self.slice_quads = on;
    }

    /// How many windows are drawn on a canvas, how many textures are kept for making pictures, and
    /// how many filters have run so far, for tests.
    #[doc(hidden)]
    pub fn effect_counts(&self) -> (usize, usize, usize) {
        (
            self.canvases.len(),
            self.effects.pool.len(),
            self.filters_run,
        )
    }

    /// The length of a side of the glyph atlas, for tests.
    #[doc(hidden)]
    pub fn atlas_size(&self) -> u32 {
        self.glyphs.size
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Forgets all layer textures, so the next frame redraws their content from scratch.
    #[doc(hidden)]
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
            // Copied from by what reads it.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
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
            texture,
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
            output: None,
        }
    }

    /// Makes sure a layer has where to keep what its filter makes of `rect`, in texels.
    fn prepare_output(&self, device: &wgpu::Device, cache: &mut LayerCache, rect: RectI) {
        let round_up = |v: i32| ((v.max(1) + 63) & !63).min(self.max_texture as i32);
        let needed = (round_up(rect.width), round_up(rect.height));
        let fits = cache.output.as_ref().is_some_and(|o| {
            o.size.0 >= needed.0
                && o.size.1 >= needed.1
                && (o.size.0 as i64 * o.size.1 as i64) <= 4 * needed.0 as i64 * needed.1 as i64
        });
        if !fits {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("filter output"),
                size: wgpu::Extent3d {
                    width: needed.0 as u32,
                    height: needed.1 as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            cache.output = Some(FilterOutput {
                bind: effects::source_bind(device, &self.atlas_layout, &view, &self.sampler),
                bind_linear: effects::source_bind(
                    device,
                    &self.atlas_layout,
                    &view,
                    &self.clamp_sampler,
                ),
                texture,
                view,
                size: needed,
                rect,
                made_by: None,
            });
        }
        let output = cache.output.as_mut().expect("just made");
        if output.rect != rect {
            output.rect = rect;
            output.made_by = None;
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
        // A filter shows `view`, made of all it reads around it.
        let shows = view;
        let view = match &layer.filter {
            Some(f) => view
                .outset(f.image.read_px(f.scale))
                .intersect(&layer.bounds.inset(f.reach, f.reach, f.reach, f.reach)),
            None => view,
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
        // too big. Not by much for a short row, which there can be a lot of.
        let round_up = |v: i32| {
            let step = if v <= 256 { 16 } else { 128 };
            ((v + step - 1) & !(step - 1)).min(self.max_texture as i32)
        };
        let needed = (round_up(view.width * k), round_up(view.height * k));
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
        expand_for_backdrops(&layer.content, &mut dirty);
        dirty.clip(view);
        let dirty = dirty.rects().to_vec();
        let nested = cache.nested.clone();
        for i in nested {
            if let Primitive::Layer(inner) = &layer.content.primitives[i as usize] {
                self.plan_layer(device, inner, view, Some(&dirty), k, plans);
            }
        }
        let mut filter = None;
        if layer.filter.is_some() {
            let mut cache = self.layers.remove(&layer.key).expect("kept above");
            self.prepare_output(device, &mut cache, shows.scaled(k));
            let output = cache.output.as_mut().expect("just prepared");
            if !dirty.is_empty() || output.made_by != layer.filter {
                output.made_by = layer.filter.clone();
                filter = layer.filter.clone();
            }
            self.layers.insert(layer.key, cache);
        } else if let Some(cache) = self.layers.get_mut(&layer.key) {
            cache.output = None;
        }
        if !dirty.is_empty() || filter.is_some() {
            plans.push(LayerPlan {
                key: layer.key,
                dirty,
                filter,
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
        let preloads = preloads_from(self.preloaded);
        self.preloaded += preloads.len();
        self.compile_shaders(device, &preloads);
        let mut cleared = false;
        let mut paths_cleared = false;
        loop {
            match self.try_prepare(device, scene, damage, plans) {
                Some(r) => return r,
                None => {
                    if let Some(source) = self.missing_shader.take() {
                        self.compile_shader(device, source);
                        continue;
                    }
                    if self.paths.take_full(paths_cleared) {
                        paths_cleared = true;
                        continue;
                    }
                    if std::mem::take(&mut self.missing_effects) {
                        self.effect_pipelines = Some(effects::create_pipelines(
                            device,
                            self.format,
                            &self.globals_layout,
                            &self.atlas_layout,
                            &self.effect_uniform_layout,
                            self.shader_cache.as_ref().map(|c| &c.cache),
                        ));
                        self.save_shaders();
                        continue;
                    }
                    let next = self.glyphs.size_when_full(cleared, self.max_texture);
                    if next != self.glyphs.size {
                        let (t, b) = create_atlas(device, &self.atlas_layout, &self.sampler, next);
                        self.atlas = t;
                        self.atlas_bind = b;
                    }
                    self.glyphs.clear(next);
                    cleared = true;
                }
            }
        }
    }

    /// Compiles those not compiled yet, on as many threads as there are cores: compiling one can
    /// take a few hundred milliseconds (FXC on DX12), and an app preloads several before its
    /// first frame.
    fn compile_shaders(&mut self, device: &wgpu::Device, sources: &[&'static ShaderSource]) {
        let mut todo: Vec<&'static ShaderSource> = Vec::new();
        for &source in sources {
            let key = shader_key(source);
            if !self.shaders.contains_key(&key) && !todo.iter().any(|&s| shader_key(s) == key) {
                todo.push(source);
            }
        }
        if todo.len() < 2 {
            for source in todo {
                self.compile_shader(device, source);
            }
            return;
        }
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(todo.len());
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (format, globals, params, inputs) = (
            self.format,
            &self.globals_layout,
            &self.shader_params_layout,
            &self.inputs_layout,
        );
        let cache = self.shader_cache.as_ref().map(|c| &c.cache);
        let compiled: Vec<(usize, CompiledShader)> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(&source) = todo.get(i) else {
                                return out;
                            };
                            let shader = custom_shader::compile(
                                device, source, format, globals, params, inputs, cache,
                            );
                            out.push((shader_key(source), shader));
                        }
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|w| w.join().expect("compiling a shader doesn't panic"))
                .collect()
        });
        self.shaders.extend(compiled);
        self.save_shaders();
    }

    /// Saves the compiled shaders for the next launch, if anything new was compiled.
    fn save_shaders(&self) {
        if let Some(c) = &self.shader_cache {
            c.save_soon();
        }
    }

    fn compile_shader(&mut self, device: &wgpu::Device, source: &'static ShaderSource) {
        let key = shader_key(source);
        if !self.shaders.contains_key(&key) {
            let compiled = custom_shader::compile(
                device,
                source,
                self.format,
                &self.globals_layout,
                &self.shader_params_layout,
                &self.inputs_layout,
                self.shader_cache.as_ref().map(|c| &c.cache),
            );
            self.shaders.insert(key, compiled);
            self.save_shaders();
        }
    }

    /// Plans the pictures of the primitives that need some and show in `touches`, in paint order.
    #[allow(clippy::too_many_arguments)]
    fn plan_effects(
        &mut self,
        device: &wgpu::Device,
        primitives: &[Primitive],
        bounds: impl Fn(usize) -> RectI,
        touches: impl Fn(&RectI) -> bool,
        k: i32,
        area: RectI,
        backdrop: Option<Source>,
        instances: &mut Instances,
    ) -> Option<TargetEffects> {
        let mut out = TargetEffects::default();
        for (i, prim) in primitives.iter().enumerate() {
            if !needs_pictures(prim) || !touches(&bounds(i)) {
                continue;
            }
            if self.effect_pipelines.is_none() {
                self.missing_effects = true;
                return None;
            }
            self.effects.release_results();
            let pool = PoolContext {
                source_layout: &self.atlas_layout,
                globals_layout: &self.globals_layout,
                sampler: &self.clamp_sampler,
                format: self.format,
                max_texture: self.max_texture,
            };
            let mut steps = Vec::new();
            let eval = match prim {
                Primitive::Backdrop(b) => {
                    let cx = EvalContext {
                        k,
                        scale: b.scale,
                        rect: b.rect.scaled(k),
                        border: [0; 4],
                        padding: [0; 4],
                        radii: b.radii,
                        content: None,
                        backdrop: backdrop.clone(),
                    };
                    let shows = b
                        .outer_rect()
                        .scaled(k)
                        .intersect(&b.clip.scaled(k).rect)
                        .intersect(&area);
                    let slots = self.effects.evaluate(
                        device,
                        &pool,
                        &[(b.image.clone(), shows)],
                        &cx,
                        &mut self.shaders,
                        &mut instances.shaders,
                        &mut steps,
                    );
                    PrimEval::Backdrop(self.got_slots(slots)?[0])
                }
                Primitive::Shader(s) => {
                    let cx = EvalContext {
                        k,
                        scale: s.scale,
                        rect: s.rect.scaled(k),
                        border: s.border,
                        padding: s.padding,
                        radii: s.radii,
                        content: None,
                        backdrop: backdrop.clone(),
                    };
                    let shows = s
                        .rect
                        .scaled(k)
                        .intersect(&s.clip.scaled(k).rect)
                        .intersect(&area)
                        .outset(s.paint.reach_px(s.scale) * k);
                    let roots: Vec<_> = s
                        .paint
                        .input_images()
                        .iter()
                        .map(|image| (image.clone(), shows))
                        .collect();
                    let slots = self.effects.evaluate(
                        device,
                        &pool,
                        &roots,
                        &cx,
                        &mut self.shaders,
                        &mut instances.shaders,
                        &mut steps,
                    );
                    let slots = self.got_slots(slots)?;
                    PrimEval::Inputs(self.effects.bind_inputs(&slots, &cx))
                }
                _ => unreachable!("only these need pictures"),
            };
            out.by_primitive.insert(i, (out.steps.len(), eval));
            out.steps.push(steps);
        }
        Some(out)
    }

    fn got_slots(
        &mut self,
        slots: Result<Vec<Option<Slot>>, Missing>,
    ) -> Option<Vec<Option<Slot>>> {
        match slots {
            Ok(slots) => Some(slots),
            Err(Missing::Shader(source)) => {
                self.missing_shader = Some(source);
                None
            }
        }
    }

    /// Plans what a layer's filter makes of it.
    fn plan_filter(
        &mut self,
        device: &wgpu::Device,
        filter: &LayerFilter,
        cache: &LayerCache,
        instances: &mut Instances,
    ) -> Option<Vec<Step>> {
        if self.effect_pipelines.is_none() {
            self.missing_effects = true;
            return None;
        }
        let output = cache.output.as_ref()?;
        self.effects.release_results();
        let k = cache.k;
        let cx = EvalContext {
            k,
            scale: filter.scale,
            rect: filter.rect.scaled(k),
            border: filter.border,
            padding: filter.padding,
            radii: filter.radii,
            content: Some(Source {
                texture: cache.texture.clone(),
                wrap: Some(cache.size),
                valid: cache.valid.scaled(k),
            }),
            backdrop: None,
        };
        let pool = PoolContext {
            source_layout: &self.atlas_layout,
            globals_layout: &self.globals_layout,
            sampler: &self.clamp_sampler,
            format: self.format,
            max_texture: self.max_texture,
        };
        let mut steps = Vec::new();
        let slots = self.effects.evaluate(
            device,
            &pool,
            &[(filter.image.clone(), output.rect)],
            &cx,
            &mut self.shaders,
            &mut instances.shaders,
            &mut steps,
        );
        let slot = self.got_slots(slots)?[0];
        match slot {
            Some(slot) => {
                let part = output.rect.intersect(&slot.rect).intersect(&RectI::new(
                    output.rect.x,
                    output.rect.y,
                    output.size.0,
                    output.size.1,
                ));
                if part != output.rect {
                    steps.push(Step::ClearOutput {
                        view: output.view.clone(),
                    });
                }
                if !part.is_empty() {
                    steps.push(Step::Output {
                        src: slot.tex,
                        src_at: ((part.x - slot.rect.x) as u32, (part.y - slot.rect.y) as u32),
                        to: output.texture.clone(),
                        at: (
                            (part.x - output.rect.x) as u32,
                            (part.y - output.rect.y) as u32,
                        ),
                        size: (part.width as u32, part.height as u32),
                    });
                }
            }
            None => steps.push(Step::ClearOutput {
                view: output.view.clone(),
            }),
        }
        Some(steps)
    }

    fn try_prepare(
        &mut self,
        device: &wgpu::Device,
        scene: &Scene,
        damage: Option<&[RectI]>,
        plans: &[LayerPlan],
    ) -> Option<Prepared> {
        for shader in self.shaders.values_mut() {
            shader.data.clear();
            shader.count = 0;
        }
        self.effects.begin_try();
        let mut out = Prepared {
            instances: Instances::default(),
            segments: Vec::new(),
            effects: Vec::new(),
            rasters: Vec::new(),
        };
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        let clear = scene.clear.premultiplied();
        for r in damage.unwrap_or_default() {
            out.instances.solids.push(solid_instance(*r, clear));
        }

        let layers = std::mem::take(&mut self.layers);
        let mut rasters = Vec::with_capacity(plans.len());
        let mut failed = false;
        for plan in plans {
            let cache = &layers[&plan.key];
            let mut ops = Vec::new();
            let target = RectI::new(0, 0, cache.size.0, cache.size.1);
            let k = cache.k;
            let content = cache.content.clone();
            let touched_dirty = |r: &RectI| plan.dirty.iter().any(|d| d.intersects(r));
            let Some(planned) = self.plan_effects(
                device,
                &content.primitives,
                |i| cache.bounds[i],
                touched_dirty,
                k,
                cache.valid.scaled(k),
                Some(Source {
                    texture: cache.texture.clone(),
                    wrap: Some(cache.size),
                    valid: cache.valid.scaled(k),
                }),
                &mut out.instances,
            ) else {
                failed = true;
                break;
            };
            for dirty in &plan.dirty {
                let mut ok = true;
                torus_parts(dirty.scaled(k), cache.size, |part, (tx, ty)| {
                    let scissor = RectI::new(tx, ty, part.width, part.height);
                    let index = out.instances.solids.len() as u32;
                    out.instances.solids.push(solid_instance(scissor, [0.0; 4]));
                    let mut segments: Vec<Vec<Batch>> = Vec::new();
                    segments.resize_with(planned.steps.len() + 1, Vec::new);
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
                    let mut segment = 0;
                    for (i, (prim, bounds)) in
                        content.primitives.iter().zip(&cache.bounds).enumerate()
                    {
                        let eval = planned.by_primitive.get(&i);
                        if let Some((seg, _)) = eval {
                            segment = seg + 1;
                        }
                        if !bounds.intersects(&touched) {
                            continue;
                        }
                        let placed_ok = self.push_primitive(
                            device,
                            prim,
                            at,
                            target,
                            &layers,
                            &mut out.instances,
                            &mut segments[segment],
                            &mut placed,
                            eval.map(|e| e.1),
                        );
                        ok &= placed_ok.is_some();
                    }
                    ops.push(RasterOp {
                        scissor,
                        clear: index,
                        segments,
                    });
                });
                failed |= !ok;
            }
            if failed {
                break;
            }
            let filter = match &plan.filter {
                Some(filter) => match self.plan_filter(device, filter, cache, &mut out.instances) {
                    Some(steps) => steps,
                    None => {
                        failed = true;
                        break;
                    }
                },
                None => Vec::new(),
            };
            self.effects.release_results();
            rasters.push(Raster {
                key: plan.key,
                ops,
                effects: planned.steps,
                filter,
            });
        }

        let screen = RectI::new(0, 0, scene.width as i32, scene.height as i32);
        let touches = |r: &RectI| damage.is_none_or(|d| d.iter().any(|x| x.intersects(r)));
        let canvas_source = self.canvases.get(&self.target).map(|c| Source {
            texture: c.texture.clone(),
            wrap: None,
            valid: screen,
        });
        let window = if failed {
            None
        } else {
            self.plan_effects(
                device,
                &scene.primitives,
                |i| primitive_bounds(&scene.primitives[i]).intersect(&screen),
                touches,
                1,
                screen,
                canvas_source,
                &mut out.instances,
            )
            .and_then(|planned| {
                out.segments.resize_with(planned.steps.len() + 1, Vec::new);
                let mut segment = 0;
                for (i, prim) in scene.primitives.iter().enumerate() {
                    let eval = planned.by_primitive.get(&i);
                    if let Some((seg, _)) = eval {
                        segment = seg + 1;
                    }
                    let bounds = primitive_bounds(prim).intersect(&screen);
                    if bounds.is_empty() || !touches(&bounds) {
                        continue;
                    }
                    self.push_primitive(
                        device,
                        prim,
                        Placement::NONE,
                        screen,
                        &layers,
                        &mut out.instances,
                        &mut out.segments[segment],
                        &mut placed,
                        eval.map(|e| e.1),
                    )?;
                }
                out.effects = planned.steps;
                Some(())
            })
        };
        self.layers = layers;
        window?;
        out.rasters = rasters;
        Some(out)
    }

    /// Adds the instances for a primitive, placed by `at`, to a pass rendering into `target`.
    /// Returns `None` if the glyph atlas is full.
    #[allow(clippy::too_many_arguments)]
    fn push_primitive(
        &mut self,
        device: &wgpu::Device,
        prim: &Primitive,
        at: Placement,
        target: RectI,
        layers: &HashMap<u64, LayerCache>,
        instances: &mut Instances,
        batches: &mut Vec<Batch>,
        placed: &mut Vec<PlacedGlyph>,
        eval: Option<PrimEval>,
    ) -> Option<()> {
        let Placement { k, dx, dy } = at;
        let Instances {
            solids,
            quads,
            glyphs,
            layers: layer_instances,
            shaders: shader_instances,
            backdrops,
            pictures,
            videos,
            paths,
            path_table,
        } = instances;
        match prim {
            Primitive::Image(d) => self.push_picture(device, d, at, target, pictures, batches),
            Primitive::Video(d) => self.push_video(device, d, at, target, videos, batches),
            Primitive::Canvas(d) => self.push_canvas(device, d, at, target, pictures, batches),
            Primitive::Path(d) => {
                let clip = gpu_clip(&d.clip.scaled(k).translated(dx, dy), target);
                let before = paths.len() as u32;
                self.paths
                    .push(device, d, k, dx, dy, target, clip, paths, path_table)?;
                let end = paths.len() as u32;
                if end > before {
                    match batches.last_mut() {
                        Some(Batch::Paths(r)) => r.end = end,
                        _ => batches.push(Batch::Paths(before..end)),
                    }
                }
            }
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
            Primitive::Backdrop(b) => {
                // It didn't get its picture because it doesn't show: nothing to draw.
                let Some(PrimEval::Backdrop(Some(slot))) = eval else {
                    return Some(());
                };
                let m = Affine::translation(dx as f64, dy as f64)
                    .then_apply(&Affine::scaling(k as f64, k as f64))
                    .then_apply(&b.transform);
                let (x, y) = m.apply(b.rect.x as f64, b.rect.y as f64);
                let (clip, clip_radii) = gpu_clip(&b.clip.scaled(k).translated(dx, dy), target);
                let size = self.effects.texture_size(slot.tex);
                backdrops.push(BackdropInstance {
                    matrix: [m.a as f32, m.b as f32, m.c as f32, m.d as f32],
                    offset: [x as f32, y as f32],
                    size: [b.rect.width as f32, b.rect.height as f32],
                    radii: b.radii,
                    clip,
                    clip_radii,
                    origin: [(slot.rect.x + dx) as f32, (slot.rect.y + dy) as f32],
                    limit: [slot.rect.width as f32, slot.rect.height as f32],
                    texel: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
                    opacity: b.opacity,
                });
                let end = backdrops.len() as u32;
                batches.push(Batch::Backdrop {
                    tex: slot.tex,
                    range: end - 1..end,
                });
            }
            Primitive::Shader(s) => {
                let source = s.paint.source();
                let key = shader_key(source);
                let Some(compiled) = self.shaders.get_mut(&key) else {
                    self.missing_shader = Some(source);
                    return None;
                };
                if compiled.pipeline.is_none() {
                    return Some(());
                }
                let inputs = match eval {
                    Some(PrimEval::Inputs(inputs)) => Some(inputs),
                    // It takes inputs and doesn't show now, or it would have got them.
                    _ if !s.paint.input_images().is_empty() => return Some(()),
                    _ => None,
                };
                let f = k as f32;
                let rect = s.rect.scaled(k).translate(dx, dy);
                let clip = s.clip.scaled(k).translated(dx, dy);
                let area = rect.intersect(&clip.rect).intersect(&target);
                if area.is_empty() {
                    return Some(());
                }
                let (chunk, index) = compiled.push(s.paint.params());
                let (clip, clip_radii) = gpu_clip(&clip, target);
                shader_instances.push(ShaderInstance {
                    area: rect4(area),
                    rect: rect4(rect),
                    rect_radii: s.radii.map(|r| r * f),
                    clip,
                    clip_radii,
                    radii: s.radii,
                    border: s.border,
                    padding: s.padding,
                    size: [s.rect.width, s.rect.height],
                    misc: [f, s.scale, s.opacity],
                    index,
                });
                let end = shader_instances.len() as u32;
                match batches.last_mut() {
                    Some(Batch::Shader {
                        key: last,
                        chunk: same,
                        inputs: same_inputs,
                        range,
                    }) if *last == key && *same == chunk && *same_inputs == inputs => {
                        range.end = end
                    }
                    _ => batches.push(Batch::Shader {
                        key,
                        chunk,
                        inputs,
                        range: end - 1..end,
                    }),
                }
            }
        }
        Some(())
    }

    /// Adds what shows a picture, and makes sure the GPU is going to have it.
    fn push_picture(
        &mut self,
        device: &wgpu::Device,
        d: &ImageDraw,
        at: Placement,
        target: RectI,
        pictures: &mut Vec<PictureInstance>,
        batches: &mut Vec<Batch>,
    ) {
        let Placement { k, dx, dy } = at;
        let rect = d.rect.scaled(k).translate(dx, dy);
        let shape = d.shape.scaled(k).translated(dx, dy);
        let clip = d.clip.scaled(k).translated(dx, dy);
        let placed = ImageDraw {
            rect,
            shape,
            clip,
            ..d.clone()
        };
        let area = placed.area().intersect(&target);
        if area.is_empty() || rect.is_empty() {
            return;
        }
        let id = d.bitmap.id();
        if !self.pictures.contains_key(&id) {
            let Some(picture) = self.create_picture(device, &d.bitmap) else {
                return;
            };
            self.pictures.insert(id, picture);
            self.picture_uploads.push((d.bitmap.clone(), d.frame));
        }
        let picture = self.pictures.get_mut(&id).expect("just made sure");
        if picture.frame != d.frame {
            picture.frame = d.frame;
            self.picture_uploads.push((d.bitmap.clone(), d.frame));
        }
        // As in Chrome: the smallest level that is no smaller than the box, read with bilinear
        // filtering. See `docs/design/22-images.md` for how this was found out.
        let tile = (d.tile.0 * k as f32, d.tile.1 * k as f32);
        let shrink = (d.bitmap.width() as f32 / tile.0).max(d.bitmap.height() as f32 / tile.1);
        let lod = (shrink.log2().floor() - picture.first as f32).max(0.0);
        let sampler = u8::from(d.pixelated) | u8::from(d.repeat.0) << 1 | u8::from(d.repeat.1) << 2;
        let opaque = d.covers_all();
        let (clip_rect, clip_radii) = gpu_clip(&clip, target);
        pictures.push(PictureInstance {
            area: rect4(area),
            rect: [rect.x as f32, rect.y as f32, tile.0, tile.1],
            shape: rect4(shape.rect),
            shape_radii: shape.radii,
            clip: clip_rect,
            clip_radii,
            params: [lod, d.opacity],
        });
        let end = pictures.len() as u32;
        match batches.last_mut() {
            Some(Batch::Picture {
                id: last,
                sampler: same,
                opaque: same_opaque,
                range,
            }) if *last == id && *same == sampler && *same_opaque == opaque => range.end = end,
            _ => batches.push(Batch::Picture {
                id,
                sampler,
                opaque,
                range: end - 1..end,
            }),
        }
    }

    /// Adds what shows what a `gpu_canvas` drew, and has it drawn first if it isn't yet.
    fn push_canvas(
        &mut self,
        device: &wgpu::Device,
        d: &CanvasDraw,
        at: Placement,
        target: RectI,
        pictures: &mut Vec<PictureInstance>,
        batches: &mut Vec<Batch>,
    ) {
        let Placement { k, dx, dy } = at;
        let rect = d.rect.scaled(k).translate(dx, dy);
        let shape = d.shape.scaled(k).translated(dx, dy);
        let clip = d.clip.scaled(k).translated(dx, dy);
        let area = rect
            .intersect(&shape.rect)
            .intersect(&clip.rect)
            .intersect(&target);
        if area.is_empty() || rect.is_empty() {
            return;
        }
        // As many pixels as it shows on, unless that is more than a texture can have.
        let size = (
            (rect.width as u32).min(self.max_texture),
            (rect.height as u32).min(self.max_texture),
        );
        if self.gpu_canvases.get(&d.id).is_none_or(|c| c.size != size) {
            let texture = self.create_canvas_texture(device, size);
            self.gpu_canvases.insert(d.id, texture);
        }
        let canvas = self.gpu_canvases.get_mut(&d.id).expect("just made sure");
        if canvas.version != Some(d.version) {
            canvas.version = Some(d.version);
            let scale = d.scale * k as f32 * size.0 as f32 / rect.width as f32;
            self.canvas_draws.push((d.id, scale));
        }
        let (clip_rect, clip_radii) = gpu_clip(&clip, target);
        pictures.push(PictureInstance {
            area: rect4(area),
            rect: rect4(rect),
            shape: rect4(shape.rect),
            shape_radii: shape.radii,
            clip: clip_rect,
            clip_radii,
            params: [0.0, d.opacity],
        });
        let end = pictures.len() as u32;
        batches.push(Batch::Canvas {
            id: d.id,
            range: end - 1..end,
        });
    }

    fn create_canvas_texture(&self, device: &wgpu::Device, size: (u32, u32)) -> CanvasTexture {
        let srgb = [crate::canvas::SRGB_FORMAT];
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpu_canvas"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::canvas::CANVAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: if self.srgb_views { &srgb } else { &[] },
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let srgb_view = self.srgb_views.then(|| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(crate::canvas::SRGB_FORMAT),
                ..Default::default()
            })
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu_canvas"),
            layout: &self.picture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        CanvasTexture {
            view,
            srgb_view,
            bind,
            size,
            version: None,
            shown: false,
        }
    }

    /// Has the canvases that changed draw, before anything that shows them.
    fn draw_canvases(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for (id, scale) in std::mem::take(&mut self.canvas_draws) {
            let Some(canvas) = self.gpu_canvases.get_mut(&id) else {
                continue;
            };
            let target = crate::canvas::Target {
                view: &canvas.view,
                srgb_view: canvas.srgb_view.as_ref(),
                size: canvas.size,
                scale,
            };
            canvas.shown = crate::canvas::render(id, device, queue, &target);
        }
    }

    /// Adds what shows a frame of a video, and makes sure the GPU is going to have it.
    fn push_video(
        &mut self,
        device: &wgpu::Device,
        d: &VideoDraw,
        at: Placement,
        target: RectI,
        videos: &mut Vec<VideoInstance>,
        batches: &mut Vec<Batch>,
    ) {
        let Placement { k, dx, dy } = at;
        let rect = d.rect.scaled(k).translate(dx, dy);
        let shape = d.shape.scaled(k).translated(dx, dy);
        let clip = d.clip.scaled(k).translated(dx, dy);
        let area = rect
            .intersect(&shape.rect)
            .intersect(&clip.rect)
            .intersect(&target);
        if area.is_empty() || rect.is_empty() {
            return;
        }
        self.video.prepare(device, &d.frame);
        let (clip_rect, clip_radii) = gpu_clip(&clip, target);
        videos.push(VideoPass::instance(
            d,
            rect4(area),
            rect4(rect),
            (rect4(shape.rect), shape.radii),
            (clip_rect, clip_radii),
        ));
        let end = videos.len() as u32;
        let stream = d.frame.stream;
        match batches.last_mut() {
            Some(Batch::Video {
                stream: last,
                range,
            }) if *last == stream => range.end = end,
            _ => batches.push(Batch::Video {
                stream,
                range: end - 1..end,
            }),
        }
    }

    fn create_picture(&self, device: &wgpu::Device, bitmap: &Arc<Bitmap>) -> Option<GpuPicture> {
        let (w, h) = (bitmap.width(), bitmap.height());
        let mut first = 0;
        while (w >> first).max(h >> first) > self.max_texture {
            first += 1;
        }
        let levels = bitmap.level_count().saturating_sub(first).max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picture"),
            size: wgpu::Extent3d {
                width: (w >> first).max(1),
                height: (h >> first).max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("picture"),
            layout: &self.picture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        Some(GpuPicture {
            bitmap: Arc::downgrade(bitmap),
            texture,
            bind,
            first,
            frame: 0,
        })
    }

    /// Hands to the GPU what the frame needs and it doesn't have.
    fn upload_pictures(&mut self, queue: &wgpu::Queue) {
        for (bitmap, frame) in std::mem::take(&mut self.picture_uploads) {
            let Some(picture) = self.pictures.get(&bitmap.id()) else {
                continue;
            };
            let (first, texture) = (picture.first, &picture.texture);
            let write = |level: u32, (w, h): (u32, u32), pixels: &[u8]| {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: level,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            };
            if let Some(pixels) = bitmap.frame(frame) {
                write(0, (bitmap.width(), bitmap.height()), pixels);
                continue;
            }
            let levels = bitmap.level_count();
            bitmap.with_levels(|level, size, pixels| {
                if level >= first {
                    write(level - first, size, pixels);
                } else if levels == 1 {
                    // It has no level that fits, and something coarse beats nothing.
                    let (w, h) = ((size.0 >> first).max(1), (size.1 >> first).max(1));
                    write(0, (w, h), &every_nth_pixel(pixels, size, first));
                }
            });
            if self.release_pixels {
                bitmap.release_levels();
            }
        }
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
        // What is older than the last frame adds to it, and can touch what reads what is drawn.
        let mut damage = damage.clone();
        expand_for_backdrops(scene, &mut damage);
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
        // Something reads what is drawn in the window: it goes where that is possible.
        let reads = scene.primitives.iter().any(|p| backdrop_read(p).is_some());
        let size = (scene.width.max(1), scene.height.max(1));
        let mut damage = damage;
        if reads {
            if self.effect_pipelines.is_none() {
                self.effect_pipelines = Some(effects::create_pipelines(
                    device,
                    self.format,
                    &self.globals_layout,
                    &self.atlas_layout,
                    &self.effect_uniform_layout,
                    self.shader_cache.as_ref().map(|c| &c.cache),
                ));
                self.save_shaders();
            }
            if self
                .canvases
                .get(&self.target)
                .is_none_or(|c| c.size != size)
            {
                let canvas = self.create_canvas(device, size);
                self.canvases.insert(self.target, canvas);
                // It holds nothing yet.
                damage = None;
            }
        } else {
            self.canvases.remove(&self.target);
        }
        self.pictures.retain(|_, p| p.bitmap.strong_count() > 0);
        self.gpu_canvases.retain(|id, _| crate::canvas::alive(*id));
        self.video.begin_frame(queue);
        self.paths.begin_frame();
        self.paths.prepare_many(device, &scene.primitives);
        let plans = self.plan_layers(device, scene, damage);
        let Prepared {
            instances,
            segments,
            effects,
            rasters,
        } = self.prepare(device, scene, damage, &plans);
        self.draw_canvases(device, queue);
        self.upload_pictures(queue);
        self.video.upload(device, queue);
        self.paths.upload(device, queue, &instances.path_table);
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
        self.backdrop_buffer
            .upload(device, queue, bytemuck::cast_slice(&instances.backdrops));
        self.picture_buffer
            .upload(device, queue, bytemuck::cast_slice(&instances.pictures));
        if !instances.paths.is_empty() {
            self.path_buffer
                .upload(device, queue, bytemuck::cast_slice(&instances.paths));
        }
        if !instances.videos.is_empty() {
            self.video_buffer
                .upload(device, queue, bytemuck::cast_slice(&instances.videos));
        }
        if !instances.shaders.is_empty() {
            self.shader_buffer
                .upload(device, queue, bytemuck::cast_slice(&instances.shaders));
            for shader in self.shaders.values_mut() {
                shader.upload(device, queue, &self.shader_params_layout);
            }
        }
        self.upload_effects(device, queue);

        for raster in &rasters {
            let cache = &self.layers[&raster.key];
            if !raster.ops.is_empty() {
                for segment in 0..=raster.effects.len() {
                    if segment > 0 {
                        self.run_steps(encoder, &raster.effects[segment - 1]);
                    }
                    let mut pass = begin_pass(encoder, "layer", &cache.view, wgpu::LoadOp::Load);
                    pass.set_bind_group(0, &cache.globals_bind, &[]);
                    for op in &raster.ops {
                        let r = op.scissor;
                        pass.set_scissor_rect(
                            r.x as u32,
                            r.y as u32,
                            r.width as u32,
                            r.height as u32,
                        );
                        if segment == 0 {
                            pass.set_pipeline(&self.opaque_pipeline);
                            pass.set_vertex_buffer(0, self.solid_buffer.buffer.slice(..));
                            pass.draw(0..4, op.clear..op.clear + 1);
                        }
                        self.draw_batches(&mut pass, &op.segments[segment]);
                    }
                }
            }
            if !raster.filter.is_empty() {
                self.filters_run += 1;
                self.run_steps(encoder, &raster.filter);
            }
        }

        let c = scene.clear.premultiplied();
        let canvas = reads.then(|| &self.canvases[&self.target]);
        let drawn_on = canvas.map_or(view, |c| &c.view);
        for segment in 0..segments.len() {
            if segment > 0 {
                self.run_steps(encoder, &effects[segment - 1]);
            }
            let load = match damage {
                None if segment == 0 => wgpu::LoadOp::Clear(wgpu::Color {
                    r: c[0] as f64,
                    g: c[1] as f64,
                    b: c[2] as f64,
                    a: c[3] as f64,
                }),
                _ => wgpu::LoadOp::Load,
            };
            let mut pass = begin_pass(encoder, "anti-webui", drawn_on, load);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            let Some(damage) = damage else {
                self.draw_batches(&mut pass, &segments[segment]);
                continue;
            };
            // Each rectangle is cleared and then repainted; overlapping rectangles just repeat work.
            for (i, r) in damage.iter().enumerate() {
                pass.set_scissor_rect(r.x as u32, r.y as u32, r.width as u32, r.height as u32);
                if segment == 0 {
                    pass.set_pipeline(&self.opaque_pipeline);
                    pass.set_vertex_buffer(0, self.solid_buffer.buffer.slice(..));
                    pass.draw(0..4, i as u32..i as u32 + 1);
                }
                self.draw_batches(&mut pass, &segments[segment]);
            }
        }
        if let Some(canvas) = canvas {
            let load = match damage {
                Some(_) => wgpu::LoadOp::Load,
                None => wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            };
            let pipelines = self.effect_pipelines.as_ref();
            if let Some(show) = pipelines.map(|p| &p.show) {
                let mut pass = begin_pass(encoder, "show", view, load);
                pass.set_pipeline(show);
                pass.set_bind_group(0, &canvas.bind, &[]);
                match damage {
                    Some(damage) => {
                        for r in damage {
                            pass.set_scissor_rect(
                                r.x as u32,
                                r.y as u32,
                                r.width as u32,
                                r.height as u32,
                            );
                            pass.draw(0..3, 0..1);
                        }
                    }
                    None => pass.draw(0..3, 0..1),
                }
            }
        }
        self.effects.end_frame();
    }

    fn create_canvas(&self, device: &wgpu::Device, size: (u32, u32)) -> Canvas {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Canvas {
            bind: effects::source_bind(device, &self.atlas_layout, &view, &self.sampler),
            texture,
            view,
            size,
        }
    }

    /// Hands the frame's uniforms to the GPU, and binds what shaders read.
    fn upload_effects(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.effects.binds.clear();
        if self.effects.uniforms.is_empty() {
            return;
        }
        let needed = self.effects.uniforms.len() as u64;
        if self
            .effect_uniforms
            .as_ref()
            .is_none_or(|b| b.size() < needed)
        {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("effect uniforms"),
                size: needed.next_power_of_two().max(UNIFORM_STRIDE as u64 * 16),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.effect_uniform_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("effect uniforms"),
                layout: &self.effect_uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(effects::BLUR_UNIFORM_SIZE),
                    }),
                }],
            }));
            self.effect_uniforms = Some(buffer);
        }
        let buffer = self.effect_uniforms.as_ref().expect("just made");
        queue.write_buffer(buffer, 0, &self.effects.uniforms);
        let binds = self
            .effects
            .inputs
            .iter()
            .map(|input| {
                let views = input
                    .textures
                    .map(|t| t.map_or(&self.empty_view, |t| self.effects.view(t)));
                inputs_bind(
                    device,
                    &self.inputs_layout,
                    views,
                    &self.clamp_sampler,
                    buffer,
                )
            })
            .collect();
        self.effects.binds = binds;
    }

    /// Runs what makes pictures. No pass is open.
    fn run_steps(&self, encoder: &mut wgpu::CommandEncoder, steps: &[Step]) {
        let extent = |size: (u32, u32)| wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let copy_of = |texture, at: (u32, u32)| wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: at.0,
                y: at.1,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        };
        for step in steps {
            match step {
                Step::Clear { tex } => {
                    let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
                    begin_pass(encoder, "clear", self.effects.view(*tex), clear);
                }
                Step::ClearOutput { view } => {
                    let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
                    begin_pass(encoder, "clear", view, clear);
                }
                Step::Copy {
                    from,
                    src,
                    dst,
                    at,
                    size,
                } => encoder.copy_texture_to_texture(
                    copy_of(from, *src),
                    copy_of(self.effects.texture(*dst), *at),
                    extent(*size),
                ),
                Step::Output {
                    src,
                    src_at,
                    to,
                    at,
                    size,
                } => encoder.copy_texture_to_texture(
                    copy_of(self.effects.texture(*src), *src_at),
                    copy_of(to, *at),
                    extent(*size),
                ),
                Step::Gauss {
                    src,
                    dst,
                    size,
                    uniform,
                } => {
                    let (Some(pipelines), Some(uniforms)) =
                        (&self.effect_pipelines, &self.effect_uniform_bind)
                    else {
                        continue;
                    };
                    let mut pass =
                        begin_pass(encoder, "blur", self.effects.view(*dst), wgpu::LoadOp::Load);
                    pass.set_scissor_rect(0, 0, size.0, size.1);
                    pass.set_pipeline(&pipelines.gauss);
                    pass.set_bind_group(0, uniforms, &[*uniform]);
                    pass.set_bind_group(1, self.effects.source_bind(*src), &[]);
                    pass.draw(0..3, 0..1);
                }
                Step::Blur {
                    down,
                    src,
                    lower,
                    dst,
                    size,
                    uniform,
                } => {
                    let (Some(pipelines), Some(uniforms)) =
                        (&self.effect_pipelines, &self.effect_uniform_bind)
                    else {
                        continue;
                    };
                    let mut pass =
                        begin_pass(encoder, "blur", self.effects.view(*dst), wgpu::LoadOp::Load);
                    pass.set_scissor_rect(0, 0, size.0, size.1);
                    pass.set_pipeline(if *down {
                        &pipelines.down
                    } else {
                        &pipelines.up
                    });
                    pass.set_bind_group(0, uniforms, &[*uniform]);
                    pass.set_bind_group(1, self.effects.source_bind(*src), &[]);
                    let lower = lower.map_or(&self.empty_source, |t| self.effects.source_bind(t));
                    pass.set_bind_group(2, lower, &[]);
                    pass.draw(0..3, 0..1);
                }
                Step::Shader {
                    dst,
                    size,
                    key,
                    chunk,
                    instance,
                    inputs,
                } => {
                    let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
                    let mut pass = begin_pass(encoder, "shader", self.effects.view(*dst), clear);
                    pass.set_scissor_rect(0, 0, size.0, size.1);
                    pass.set_bind_group(0, self.effects.globals_bind(*dst), &[]);
                    self.draw_batches(
                        &mut pass,
                        &[Batch::Shader {
                            key: *key,
                            chunk: *chunk,
                            inputs: Some(*inputs),
                            range: *instance..*instance + 1,
                        }],
                    );
                }
            }
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
                Batch::Layer {
                    key,
                    linear,
                    filtered,
                    range,
                } => {
                    let Some(cache) = self.layers.get(key) else {
                        continue;
                    };
                    pass.set_pipeline(&self.layer_pipeline);
                    let bind = match (cache.output.as_ref().filter(|_| *filtered), *linear) {
                        (Some(output), true) => &output.bind_linear,
                        (Some(output), false) => &output.bind,
                        (None, true) => &cache.bind_linear,
                        (None, false) => &cache.bind,
                    };
                    pass.set_bind_group(1, bind, &[]);
                    pass.set_vertex_buffer(0, self.layer_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
                Batch::Backdrop { tex, range } => {
                    let Some(pipelines) = &self.effect_pipelines else {
                        continue;
                    };
                    pass.set_pipeline(&pipelines.backdrop);
                    pass.set_bind_group(1, self.effects.source_bind(*tex), &[]);
                    pass.set_vertex_buffer(0, self.backdrop_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
                Batch::Picture {
                    id,
                    sampler,
                    opaque,
                    range,
                } => {
                    let Some(picture) = self.pictures.get(id) else {
                        continue;
                    };
                    pass.set_pipeline(if *opaque {
                        &self.opaque_picture_pipeline
                    } else {
                        &self.picture_pipeline
                    });
                    pass.set_bind_group(1, &picture.bind, &[]);
                    pass.set_bind_group(2, &self.picture_samplers[*sampler as usize], &[]);
                    pass.set_vertex_buffer(0, self.picture_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
                Batch::Canvas { id, range } => {
                    let Some(canvas) = self.gpu_canvases.get(id).filter(|c| c.shown) else {
                        continue;
                    };
                    pass.set_pipeline(&self.picture_pipeline);
                    pass.set_bind_group(1, &canvas.bind, &[]);
                    pass.set_bind_group(2, &self.picture_samplers[0], &[]);
                    pass.set_vertex_buffer(0, self.picture_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
                Batch::Paths(range) => {
                    let Some((pipeline, bind)) = self.paths.draw_state() else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(1, bind, &[]);
                    pass.set_vertex_buffer(0, self.path_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
                Batch::Video { stream, range } => {
                    let Some((pipeline, bind)) = self.video.draw_state(*stream) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(1, bind, &[]);
                    pass.set_vertex_buffer(0, self.video_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
                Batch::Shader {
                    key,
                    chunk,
                    inputs,
                    range,
                } => {
                    let Some(shader) = self.shaders.get(key) else {
                        continue;
                    };
                    let (Some(pipeline), Some(bind)) = (&shader.pipeline, &shader.bind) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(1, bind, &[*chunk * CHUNK as u32]);
                    match inputs {
                        Some(i) => {
                            let uniform = self.effects.inputs[*i as usize].uniform;
                            pass.set_bind_group(2, &self.effects.binds[*i as usize], &[uniform]);
                        }
                        None => pass.set_bind_group(2, &self.no_inputs, &[0]),
                    }
                    pass.set_vertex_buffer(0, self.shader_buffer.buffer.slice(..));
                    pass.draw(0..4, range.clone());
                }
            }
        }
    }
}

/// Identifies a shader: its source lives in a `static`.
/// What is left of pixels when only each `2^skip`-th one of each `2^skip`-th row stays. It is as
/// fast as it gets, which is what a picture too big for the GPU needs.
fn every_nth_pixel(pixels: &[u8], (w, h): (u32, u32), skip: u32) -> Vec<u8> {
    let (kept_w, kept_h) = ((w >> skip).max(1) as usize, (h >> skip).max(1) as usize);
    let (w, step) = (w as usize, 1usize << skip);
    let mut out = Vec::with_capacity(kept_w * kept_h * 4);
    for row in pixels.chunks_exact(w * 4).step_by(step).take(kept_h) {
        for pixel in row.chunks_exact(4).step_by(step).take(kept_w) {
            out.extend_from_slice(pixel);
        }
    }
    out
}

fn shader_key(source: &'static ShaderSource) -> usize {
    std::ptr::from_ref(source) as usize
}

/// Whether a primitive needs pictures made before it is drawn.
fn needs_pictures(p: &Primitive) -> bool {
    match p {
        Primitive::Backdrop(_) => true,
        Primitive::Shader(s) => !s.paint.input_images().is_empty(),
        _ => false,
    }
}

fn begin_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
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
    })
}

fn inputs_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    views: [&wgpu::TextureView; 4],
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let view = |binding: u32, view| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shader inputs"),
        layout,
        entries: &[
            view(0, views[0]),
            view(1, views[1]),
            view(2, views[2]),
            view(3, views[3]),
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniforms,
                    offset: 0,
                    size: wgpu::BufferSize::new(INPUTS_SIZE),
                }),
            },
        ],
    })
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
    let valid = cache.valid.scaled(cache.k);
    let linear = !m.is_whole_translation();
    if layer.filter.is_some() {
        // What the filter made, which doesn't wrap around.
        let Some(output) = &cache.output else {
            return;
        };
        let r = output.rect;
        let inset = if linear { 0.5 } else { 0.0 };
        let (x, y) = m.apply(r.x as f64 + inset as f64, r.y as f64 + inset as f64);
        let (width, height) = (r.width as f32 - inset * 2.0, r.height as f32 - inset * 2.0);
        let (tw, th) = (output.size.0 as f32, output.size.1 as f32);
        instances.push(LayerInstance {
            matrix: [m.a as f32, m.b as f32, m.c as f32, m.d as f32],
            offset: [x as f32, y as f32],
            size: [width, height],
            uv: [
                inset / tw,
                inset / th,
                (inset + width) / tw,
                (inset + height) / th,
            ],
            clip,
            clip_radii,
            opacity: layer.opacity,
        });
        batches.push(Batch::Layer {
            key: layer.key,
            linear,
            filtered: true,
            range: start..instances.len() as u32,
        });
        return;
    }
    // Filtering at the very edge would read a texel further, which holds whatever was there before.
    // Half a texel in, the edge is all it reads, and it is transparent or not shown.
    let inset = if linear { 0.5 } else { 0.0 };
    torus_parts(valid, cache.size, |part, (tx, ty)| {
        let left = if part.x == valid.x { inset } else { 0.0 };
        let top = if part.y == valid.y { inset } else { 0.0 };
        let right = if part.right() == valid.right() {
            inset
        } else {
            0.0
        };
        let bottom = if part.bottom() == valid.bottom() {
            inset
        } else {
            0.0
        };
        let (width, height) = (
            part.width as f32 - left - right,
            part.height as f32 - top - bottom,
        );
        let (tx, ty) = (tx as f32 + left, ty as f32 + top);
        let (x, y) = m.apply(part.x as f64 + left as f64, part.y as f64 + top as f64);
        instances.push(LayerInstance {
            matrix: [m.a as f32, m.b as f32, m.c as f32, m.d as f32],
            offset: [x as f32, y as f32],
            size: [width, height],
            uv: [tx / w, ty / h, (tx + width) / w, (ty + height) / h],
            clip,
            clip_radii,
            opacity: layer.opacity,
        });
    });
    batches.push(Batch::Layer {
        key: layer.key,
        linear,
        filtered: false,
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

    #[test]
    fn a_picture_too_big_for_the_gpu_keeps_every_nth_pixel() {
        let pixels: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        // Half of 4 by 3 is 2 by 1, which is every other pixel of the first row.
        let kept = every_nth_pixel(&pixels, (4, 3), 1);
        assert_eq!(kept, [0, 1, 2, 3, 8, 9, 10, 11]);
        assert_eq!(every_nth_pixel(&pixels, (4, 3), 3), &pixels[..4]);
    }

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
