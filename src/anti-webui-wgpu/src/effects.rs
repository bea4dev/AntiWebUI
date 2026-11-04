//! Making pictures out of what is drawn: blurs, shaders with inputs, and the textures for it. See
//! `docs/design/16-filters.md`.

use crate::custom_shader::{CompiledShader, ShaderInstance};
use anti_webui_core::geometry::RectI;
use anti_webui_core::shader::{
    Image, ImageNode, MAX_INPUTS, ShaderSource, blur_levels, blur_reach,
};
use bytemuck::{Pod, Zeroable};
use std::collections::{HashMap, HashSet};

/// Uniforms are bound at offsets of this: what every backend can do.
pub(crate) const UNIFORM_STRIDE: usize = 256;
/// A texture that wasn't used for this many frames in a row is freed.
const POOL_KEEP_FRAMES: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurUniform {
    texel: [f32; 2],
    limit: [f32; 2],
    amount: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct InputUniform {
    origin: [f32; 2],
    texel: [f32; 2],
    limit: [f32; 2],
    k: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GaussUniform {
    texel: [f32; 2],
    limit: [f32; 2],
    step: [f32; 2],
    _pad: [f32; 2],
    weights: [f32; 8],
}

/// How much of a uniform a blur binds.
pub(crate) const BLUR_UNIFORM_SIZE: u64 = std::mem::size_of::<GaussUniform>() as u64;

/// The weights of a gaussian at each distance, adding up to 1 with both sides.
fn gauss_weights(sigma: f32) -> [f32; 8] {
    let mut weights = [0.0; 8];
    let reach = (sigma * 3.0).ceil().min(7.0) as usize;
    for (i, w) in weights.iter_mut().enumerate().take(reach + 1) {
        *w = (-((i * i) as f32) / (2.0 * sigma * sigma)).exp();
    }
    let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    weights.map(|w| w / total)
}

/// The size of what a shader's inputs take in the uniform buffer.
pub(crate) const INPUTS_SIZE: u64 = (std::mem::size_of::<InputUniform>() * MAX_INPUTS) as u64;

/// What is behind a box, shown in the box.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct BackdropInstance {
    pub matrix: [f32; 4],
    pub offset: [f32; 2],
    pub size: [f32; 2],
    pub radii: [f32; 4],
    pub clip: [f32; 4],
    pub clip_radii: [f32; 4],
    pub origin: [f32; 2],
    pub limit: [f32; 2],
    pub texel: [f32; 2],
    pub opacity: f32,
}

struct PoolTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Binds the texture to be read, without going past its edges.
    source: wgpu::BindGroup,
    globals: wgpu::BindGroup,
    size: (u32, u32),
    busy: bool,
    idle: u32,
}

/// Bind group layouts and samplers the textures of the pool are made for.
pub(crate) struct PoolContext<'a> {
    pub source_layout: &'a wgpu::BindGroupLayout,
    pub globals_layout: &'a wgpu::BindGroupLayout,
    pub sampler: &'a wgpu::Sampler,
    pub format: wgpu::TextureFormat,
    pub max_texture: u32,
}

#[derive(Default)]
pub(crate) struct Pool {
    textures: Vec<PoolTexture>,
}

impl Pool {
    fn acquire(&mut self, device: &wgpu::Device, cx: &PoolContext, w: u32, h: u32) -> usize {
        let (w, h) = (w.max(1), h.max(1));
        let area = |s: (u32, u32)| s.0 as u64 * s.1 as u64;
        let best = self
            .textures
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.busy && t.size.0 >= w && t.size.1 >= h)
            .filter(|(_, t)| area(t.size) <= 16 * area((w, h)).max(64 * 64))
            .min_by_key(|(_, t)| area(t.size))
            .map(|(i, _)| i);
        let i = best.unwrap_or_else(|| {
            // Some room, since what is blurred often changes size as it moves.
            let round = |v: u32| v.next_multiple_of(64).min(cx.max_texture);
            self.textures
                .push(create_pool_texture(device, cx, round(w), round(h)));
            self.textures.len() - 1
        });
        self.textures[i].busy = true;
        self.textures[i].idle = 0;
        i
    }

    fn release(&mut self, tex: usize) {
        self.textures[tex].busy = false;
    }

    fn release_all(&mut self) {
        for t in &mut self.textures {
            t.busy = false;
        }
    }

    /// Frees what hasn't been used for a while. Nothing may refer to the textures by index anymore.
    fn end_frame(&mut self, used: &HashSet<usize>) {
        let mut i = 0;
        self.textures.retain_mut(|t| {
            if !used.contains(&i) {
                t.idle += 1;
            }
            i += 1;
            t.idle < POOL_KEEP_FRAMES
        });
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }
}

fn create_pool_texture(device: &wgpu::Device, cx: &PoolContext, w: u32, h: u32) -> PoolTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("effect"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: cx.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let source = source_bind(device, cx.source_layout, &view, cx.sampler);
    let buffer = wgpu::util::DeviceExt::create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("effect globals"),
            contents: bytemuck::cast_slice(&[w as f32, h as f32, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        },
    );
    let globals = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("effect globals"),
        layout: cx.globals_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    PoolTexture {
        texture,
        view,
        source,
        globals,
        size: (w, h),
        busy: false,
        idle: 0,
    }
}

pub(crate) fn source_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("effect source"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// Where a picture can come from: a texture that holds `valid`, in the coordinates pictures are
/// made in. A layer's texture wraps around.
#[derive(Clone)]
pub(crate) struct Source {
    pub texture: wgpu::Texture,
    pub wrap: Option<(i32, i32)>,
    pub valid: RectI,
}

/// What pictures are made in: `k` texels per pixel, and the box of the node.
pub(crate) struct EvalContext {
    pub k: i32,
    pub scale: f32,
    /// The box, in texels.
    pub rect: RectI,
    pub border: [i32; 4],
    pub padding: [i32; 4],
    pub radii: [f32; 4],
    pub content: Option<Source>,
    pub backdrop: Option<Source>,
}

/// A picture ready to be read: `rect` is what its texture holds, from its first texel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Slot {
    pub tex: usize,
    pub rect: RectI,
}

pub(crate) enum Step {
    Clear {
        tex: usize,
    },
    Copy {
        from: wgpu::Texture,
        src: (u32, u32),
        dst: usize,
        at: (u32, u32),
        size: (u32, u32),
    },
    Gauss {
        src: usize,
        dst: usize,
        size: (u32, u32),
        uniform: u32,
    },
    Blur {
        down: bool,
        src: usize,
        lower: Option<usize>,
        dst: usize,
        size: (u32, u32),
        uniform: u32,
    },
    Shader {
        dst: usize,
        size: (u32, u32),
        key: usize,
        chunk: u32,
        instance: u32,
        inputs: u32,
    },
    /// The result of a filter goes where it is kept.
    Output {
        src: usize,
        src_at: (u32, u32),
        to: wgpu::Texture,
        at: (u32, u32),
        size: (u32, u32),
    },
    ClearOutput {
        view: wgpu::TextureView,
    },
}

/// What a shader reads, for a frame.
pub(crate) struct InputBinding {
    pub textures: [Option<usize>; MAX_INPUTS],
    pub uniform: u32,
}

/// Something the pictures need that has to be made first.
pub(crate) enum Missing {
    Shader(&'static ShaderSource),
}

fn align_down(v: i32, to: i32) -> i32 {
    v.div_euclid(to) * to
}

/// Calls `f` with the parts of `r` in the texture of `source`, and where they go relative to `r`.
fn source_parts(source: &Source, r: RectI, mut f: impl FnMut((u32, u32), (u32, u32), (u32, u32))) {
    let r = r.intersect(&source.valid);
    if r.is_empty() {
        return;
    }
    let mut y = r.y;
    while y < r.bottom() {
        let (ty, h) = match source.wrap {
            Some((_, wh)) => {
                let ty = y.rem_euclid(wh);
                (ty, (wh - ty).min(r.bottom() - y))
            }
            None => (y, r.bottom() - y),
        };
        let mut x = r.x;
        while x < r.right() {
            let (tx, w) = match source.wrap {
                Some((ww, _)) => {
                    let tx = x.rem_euclid(ww);
                    (tx, (ww - tx).min(r.right() - x))
                }
                None => (x, r.right() - x),
            };
            f(
                (tx as u32, ty as u32),
                ((x) as u32, (y) as u32),
                (w as u32, h as u32),
            );
            x += w;
        }
        y += h;
    }
}

fn post_order(image: &Image, seen: &mut HashSet<usize>, order: &mut Vec<Image>) {
    if !seen.insert(image.id()) {
        return;
    }
    match image.node() {
        ImageNode::Content | ImageNode::Backdrop => {}
        ImageNode::Blur { input, .. } => post_order(input, seen, order),
        ImageNode::Shader(paint) => {
            for input in paint.input_images() {
                post_order(input, seen, order);
            }
        }
    }
    order.push(image.clone());
}

/// As much of `r` as fits in a texture: far from what is shown, the rest makes little difference.
fn limited(r: RectI, max_texture: u32) -> RectI {
    let max = max_texture as i32;
    RectI::new(r.x, r.y, r.width.min(max), r.height.min(max))
}

/// The sizes of the textures a blur goes through.
fn level_size(rect: RectI, level: u32) -> (u32, u32) {
    let shrink = |v: i32| ((v as u32).div_ceil(1 << level)).max(1);
    (shrink(rect.width), shrink(rect.height))
}

/// The textures in use and what the passes need to read, for a frame.
#[derive(Default)]
pub(crate) struct Effects {
    pub pool: Pool,
    /// Uniforms of the frame, `UNIFORM_STRIDE` apart.
    pub uniforms: Vec<u8>,
    pub inputs: Vec<InputBinding>,
    /// Made once a frame knows its textures.
    pub binds: Vec<wgpu::BindGroup>,
    /// The textures that hold results of the current evaluation.
    live: Vec<usize>,
    used: HashSet<usize>,
}

impl Effects {
    pub fn begin_try(&mut self) {
        self.pool.release_all();
        self.uniforms.clear();
        self.inputs.clear();
        self.live.clear();
        self.used.clear();
    }

    pub fn end_frame(&mut self) {
        self.pool.end_frame(&self.used);
    }

    /// The results of the last evaluation aren't needed anymore: what reads them is drawn before
    /// anything that comes next.
    pub fn release_results(&mut self) {
        for tex in std::mem::take(&mut self.live) {
            self.pool.release(tex);
        }
    }

    fn uniform(&mut self, bytes: &[u8]) -> u32 {
        let at = self.uniforms.len();
        self.uniforms.resize(at + UNIFORM_STRIDE, 0);
        self.uniforms[at..at + bytes.len()].copy_from_slice(bytes);
        at as u32
    }

    fn acquire(&mut self, device: &wgpu::Device, cx: &PoolContext, size: (u32, u32)) -> usize {
        let tex = self.pool.acquire(device, cx, size.0, size.1);
        self.used.insert(tex);
        self.live.push(tex);
        tex
    }

    fn free(&mut self, tex: usize) {
        self.live.retain(|t| *t != tex);
        self.pool.release(tex);
    }

    pub fn texture_size(&self, tex: usize) -> (u32, u32) {
        self.pool.textures[tex].size
    }

    pub fn view(&self, tex: usize) -> &wgpu::TextureView {
        &self.pool.textures[tex].view
    }

    pub fn texture(&self, tex: usize) -> &wgpu::Texture {
        &self.pool.textures[tex].texture
    }

    pub fn source_bind(&self, tex: usize) -> &wgpu::BindGroup {
        &self.pool.textures[tex].source
    }

    pub fn globals_bind(&self, tex: usize) -> &wgpu::BindGroup {
        &self.pool.textures[tex].globals
    }

    /// What a shader needs to read `slots`. Returns the index of what to bind.
    pub fn bind_inputs(&mut self, slots: &[Option<Slot>], cx: &EvalContext) -> u32 {
        let mut values = [InputUniform::default(); MAX_INPUTS];
        let mut textures = [None; MAX_INPUTS];
        for (i, slot) in slots.iter().take(MAX_INPUTS).enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            let size = self.texture_size(slot.tex);
            values[i] = InputUniform {
                origin: [
                    (cx.rect.x - slot.rect.x) as f32,
                    (cx.rect.y - slot.rect.y) as f32,
                ],
                texel: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
                limit: [slot.rect.width as f32, slot.rect.height as f32],
                k: cx.k as f32,
                _pad: 0.0,
            };
            textures[i] = Some(slot.tex);
        }
        let uniform = self.uniform(bytemuck::cast_slice(&values));
        self.inputs.push(InputBinding { textures, uniform });
        self.inputs.len() as u32 - 1
    }

    /// Plans making each picture, for at least the rectangle given with it. Pictures that can't be
    /// made have no slot, and read as transparent.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        device: &wgpu::Device,
        pool: &PoolContext,
        roots: &[(Image, RectI)],
        cx: &EvalContext,
        shaders: &mut HashMap<usize, CompiledShader>,
        instances: &mut Vec<ShaderInstance>,
        steps: &mut Vec<Step>,
    ) -> Result<Vec<Option<Slot>>, Missing> {
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        for (image, _) in roots {
            post_order(image, &mut seen, &mut order);
        }
        // From the roots down: what each one needs of what it is made from.
        let mut needs: HashMap<usize, RectI> = HashMap::new();
        let mut need = |image: &Image, r: RectI| {
            needs
                .entry(image.id())
                .and_modify(|n| *n = n.union(&r))
                .or_insert(r);
        };
        for (image, r) in roots {
            need(image, *r);
        }
        let k = cx.k as f32;
        for image in order.iter().rev() {
            let r = needs[&image.id()];
            let mut need = |image: &Image, r: RectI| {
                needs
                    .entry(image.id())
                    .and_modify(|n| *n = n.union(&r))
                    .or_insert(r);
            };
            match image.node() {
                ImageNode::Content | ImageNode::Backdrop => {}
                ImageNode::Blur { input, radius } => {
                    let sigma = radius * cx.scale * k;
                    let (levels, _) = blur_levels(sigma);
                    let grid = 1 << levels;
                    let wide = r.outset(blur_reach(sigma));
                    // On a grid that stays where it is as the rectangle moves, so that what is
                    // blurred doesn't shimmer.
                    let (x, y) = (align_down(wide.x, grid), align_down(wide.y, grid));
                    need(input, RectI::new(x, y, wide.right() - x, wide.bottom() - y));
                }
                ImageNode::Shader(paint) => {
                    let wide = r.outset(paint.reach_px(cx.scale) * cx.k);
                    for input in paint.input_images() {
                        need(input, wide);
                    }
                }
            }
        }
        // From the leaves up: make them.
        let mut done: HashMap<usize, Option<Slot>> = HashMap::new();
        for image in &order {
            let r = limited(needs[&image.id()], pool.max_texture);
            let slot = match image.node() {
                ImageNode::Content => self.read_content(device, pool, cx, r, steps),
                ImageNode::Backdrop => self.read_backdrop(device, pool, cx, r, steps),
                ImageNode::Blur { input, radius } => {
                    let sigma = radius * cx.scale * k;
                    done[&input.id()].map(|input| self.blur(device, pool, input, sigma, steps))
                }
                ImageNode::Shader(paint) => {
                    let source = paint.source();
                    let key = std::ptr::from_ref(source) as usize;
                    let Some(compiled) = shaders.get_mut(&key) else {
                        return Err(Missing::Shader(source));
                    };
                    if compiled.pipeline.is_none() || r.is_empty() {
                        None
                    } else {
                        let slots: Vec<Option<Slot>> =
                            paint.input_images().iter().map(|i| done[&i.id()]).collect();
                        let inputs = self.bind_inputs(&slots, cx);
                        let size = (r.width as u32, r.height as u32);
                        let tex = self.acquire(device, pool, size);
                        let (chunk, index) = compiled.push(paint.params());
                        let f =
                            |r: RectI| [r.x as f32, r.y as f32, r.width as f32, r.height as f32];
                        let area = RectI::new(0, 0, r.width, r.height);
                        instances.push(ShaderInstance {
                            area: f(area),
                            rect: f(cx.rect.translate(-r.x, -r.y)),
                            rect_radii: [0.0; 4],
                            clip: f(area),
                            clip_radii: [0.0; 4],
                            radii: cx.radii,
                            border: cx.border,
                            padding: cx.padding,
                            size: [cx.rect.width / cx.k, cx.rect.height / cx.k],
                            misc: [k, cx.scale, 1.0],
                            index,
                        });
                        steps.push(Step::Shader {
                            dst: tex,
                            size,
                            key,
                            chunk,
                            instance: instances.len() as u32 - 1,
                            inputs,
                        });
                        Some(Slot { tex, rect: r })
                    }
                }
            };
            done.insert(image.id(), slot);
        }
        Ok(roots.iter().map(|(image, _)| done[&image.id()]).collect())
    }

    /// What the node painted, with nothing around it.
    fn read_content(
        &mut self,
        device: &wgpu::Device,
        pool: &PoolContext,
        cx: &EvalContext,
        r: RectI,
        steps: &mut Vec<Step>,
    ) -> Option<Slot> {
        let source = cx.content.as_ref()?;
        let r = limited(r, pool.max_texture);
        if r.is_empty() {
            return None;
        }
        let tex = self.acquire(device, pool, (r.width as u32, r.height as u32));
        steps.push(Step::Clear { tex });
        source_parts(source, r, |src, pos, size| {
            steps.push(Step::Copy {
                from: source.texture.clone(),
                src,
                dst: tex,
                at: ((pos.0 as i32 - r.x) as u32, (pos.1 as i32 - r.y) as u32),
                size,
            });
        });
        Some(Slot { tex, rect: r })
    }

    /// What is drawn so far. There is nothing past the edges of the target, so what reads there
    /// gets the color at the edge.
    fn read_backdrop(
        &mut self,
        device: &wgpu::Device,
        pool: &PoolContext,
        cx: &EvalContext,
        r: RectI,
        steps: &mut Vec<Step>,
    ) -> Option<Slot> {
        let source = cx.backdrop.as_ref()?;
        let r = limited(r.intersect(&source.valid), pool.max_texture);
        if r.is_empty() {
            return None;
        }
        let tex = self.acquire(device, pool, (r.width as u32, r.height as u32));
        source_parts(source, r, |src, pos, size| {
            steps.push(Step::Copy {
                from: source.texture.clone(),
                src,
                dst: tex,
                at: ((pos.0 as i32 - r.x) as u32, (pos.1 as i32 - r.y) as u32),
                size,
            });
        });
        Some(Slot { tex, rect: r })
    }

    /// Goes down one level more than the blur takes, comes back up, and mixes the last level with
    /// what it was on the way down. See `docs/design/16-filters.md`.
    fn blur(
        &mut self,
        device: &wgpu::Device,
        pool: &PoolContext,
        input: Slot,
        sigma: f32,
        steps: &mut Vec<Step>,
    ) -> Slot {
        let (levels, amount) = blur_levels(sigma);
        let rect = input.rect;
        if levels == 0 {
            let size = level_size(rect, 0);
            let mut src = input.tex;
            for step in [[1.0, 0.0], [0.0, 1.0]] {
                let dst = self.acquire(device, pool, size);
                let texture = self.texture_size(src);
                let uniform = self.uniform(bytemuck::bytes_of(&GaussUniform {
                    texel: [1.0 / texture.0 as f32, 1.0 / texture.1 as f32],
                    limit: [size.0 as f32, size.1 as f32],
                    step,
                    _pad: [0.0; 2],
                    weights: gauss_weights(sigma),
                }));
                steps.push(Step::Gauss {
                    src,
                    dst,
                    size,
                    uniform,
                });
                if src != input.tex {
                    self.free(src);
                }
                src = dst;
            }
            return Slot { tex: src, rect };
        }
        let mut chain = vec![input.tex];
        for level in 0..levels {
            let size = level_size(rect, level + 1);
            let dst = self.acquire(device, pool, size);
            let src = chain[level as usize];
            let uniform = self.blur_uniform(src, level_size(rect, level), 1.0);
            steps.push(Step::Blur {
                down: true,
                src,
                lower: None,
                dst,
                size,
                uniform,
            });
            chain.push(dst);
            // The level before the last is mixed in on the way up; those above it are done.
            if level >= 1 && level + 1 < levels {
                self.free(src);
            }
        }
        let mut current = chain[levels as usize];
        for level in (0..levels).rev() {
            let size = level_size(rect, level);
            let dst = self.acquire(device, pool, size);
            let last = level + 1 == levels;
            let uniform = self.blur_uniform(
                current,
                level_size(rect, level + 1),
                if last { amount } else { 1.0 },
            );
            steps.push(Step::Blur {
                down: false,
                src: current,
                lower: (last && amount < 1.0).then_some(chain[level as usize]),
                dst,
                size,
                uniform,
            });
            self.free(current);
            if last && level >= 1 {
                self.free(chain[level as usize]);
            }
            current = dst;
        }
        Slot { tex: current, rect }
    }

    fn blur_uniform(&mut self, src: usize, limit: (u32, u32), amount: f32) -> u32 {
        let size = self.texture_size(src);
        self.uniform(bytemuck::bytes_of(&BlurUniform {
            texel: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
            limit: [limit.0 as f32, limit.1 as f32],
            amount,
            _pad: [0.0; 3],
        }))
    }
}

/// The pipelines for the above, which are made when something first needs them.
pub(crate) struct EffectPipelines {
    pub gauss: wgpu::RenderPipeline,
    pub down: wgpu::RenderPipeline,
    pub up: wgpu::RenderPipeline,
    pub show: wgpu::RenderPipeline,
    pub backdrop: wgpu::RenderPipeline,
}

pub(crate) fn create_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    globals: &wgpu::BindGroupLayout,
    source: &wgpu::BindGroupLayout,
    uniform: &wgpu::BindGroupLayout,
) -> EffectPipelines {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("effects"),
        source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
    });
    let primitive = wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleStrip,
        ..Default::default()
    };
    let replace = [Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let full = |label: &str,
                layouts: &[Option<&wgpu::BindGroupLayout>],
                entry: &str|
     -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: layouts,
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive,
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &replace,
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let blur = [Some(uniform), Some(source), Some(source)];
    let gauss = full("blur gauss", &[Some(uniform), Some(source)], "fs_gauss");
    let down = full("blur down", &blur, "fs_down");
    let up = full("blur up", &blur, "fs_up");
    let show = full("show", &[Some(source)], "fs_show");

    let blend = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    let targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState {
            color: blend,
            alpha: blend,
        }),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x2, 7 => Float32x2, 8 => Float32x2, 9 => Float32
    ];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("backdrop"),
        bind_group_layouts: &[Some(globals), Some(source)],
        immediate_size: 0,
    });
    let backdrop = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("backdrop"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_backdrop"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<BackdropInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attributes,
            })],
        },
        primitive,
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_backdrop"),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    });
    EffectPipelines {
        gauss,
        down,
        up,
        show,
        backdrop,
    }
}

/// The layout of what shaders read: `read0` to `read3`.
pub(crate) fn inputs_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
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
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shader inputs"),
        entries: &[
            texture(0),
            texture(1),
            texture(2),
            texture(3),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(INPUTS_SIZE),
                },
                count: None,
            },
        ],
    })
}

/// The layout of the uniforms of a blur.
pub(crate) fn uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("effect uniforms"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(BLUR_UNIFORM_SIZE),
            },
            count: None,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weights_of_a_small_blur_add_up() {
        // Smaller than that, there is not much left of it between whole texels.
        for sigma in [0.5, 1.0, 1.68] {
            let w = gauss_weights(sigma);
            let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
            assert!((total - 1.0).abs() < 1e-5, "{sigma}: {total}");
            assert!(w.windows(2).all(|p| p[0] >= p[1]), "{sigma}: {w:?}");
            // Its standard deviation is what was asked for.
            let variance: f32 = 2.0
                * w.iter()
                    .enumerate()
                    .map(|(i, w)| (i * i) as f32 * w)
                    .sum::<f32>();
            assert!(
                (variance.sqrt() - sigma).abs() < 0.06,
                "{sigma}: {variance}"
            );
        }
    }

    #[test]
    fn a_grid_reaches_down_past_zero() {
        assert_eq!(align_down(5, 4), 4);
        assert_eq!(align_down(-1, 4), -4);
        assert_eq!(align_down(-4, 4), -4);
    }

    #[test]
    fn levels_shrink_by_half_and_never_to_nothing() {
        let r = RectI::new(0, 0, 33, 1);
        assert_eq!(level_size(r, 0), (33, 1));
        assert_eq!(level_size(r, 1), (17, 1));
        assert_eq!(level_size(r, 3), (5, 1));
    }

    #[test]
    fn a_picture_shared_is_made_once() {
        let shared = Image::backdrop().blur(3.0);
        static TWO: anti_webui_core::shader::Shader<()> =
            anti_webui_core::shader::Shader::new("two", "");
        let top: Image = TWO.params(()).inputs([shared.clone(), shared]).into();
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        post_order(&top, &mut seen, &mut order);
        // The backdrop, its blur, and the shader.
        assert_eq!(order.len(), 3);
        assert_eq!(order[2].id(), top.id());
    }
}
