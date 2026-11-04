// Dual Kawase blur, the backdrop of a box, and showing a texture as it is. See
// docs/design/16-filters.md. Colors are premultiplied.

struct Blur {
    texel: vec2<f32>,   // the size of a texel of the source, in its coordinates
    limit: vec2<f32>,   // the size in texels of what the source holds
    amount: f32,        // how much of the result against `lower`
}

@group(0) @binding(0) var<uniform> blur: Blur;
@group(1) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;
@group(2) @binding(0) var lower: texture_2d<f32>;
@group(2) @binding(1) var lower_sampler: sampler;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let c = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u)) * 4.0;
    return vec4<f32>(c.x - 1.0, 1.0 - c.y, 0.0, 1.0);
}

// Never reads past what the source holds: the rest of the texture is whatever was there before.
fn tap(p: vec2<f32>) -> vec4<f32> {
    let inside = clamp(p, vec2<f32>(0.5), blur.limit - vec2<f32>(0.5));
    return textureSampleLevel(source, source_sampler, inside * blur.texel, 0.0);
}

@fragment
fn fs_down(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = position.xy * 2.0;
    return (tap(p) * 4.0
        + tap(p + vec2<f32>(-1.0, -1.0))
        + tap(p + vec2<f32>(1.0, -1.0))
        + tap(p + vec2<f32>(-1.0, 1.0))
        + tap(p + vec2<f32>(1.0, 1.0))) / 8.0;
}

@fragment
fn fs_up(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = position.xy * 0.5;
    let result = (tap(p + vec2<f32>(-1.0, 0.0))
        + tap(p + vec2<f32>(1.0, 0.0))
        + tap(p + vec2<f32>(0.0, -1.0))
        + tap(p + vec2<f32>(0.0, 1.0))
        + (tap(p + vec2<f32>(-0.5, -0.5))
            + tap(p + vec2<f32>(0.5, -0.5))
            + tap(p + vec2<f32>(-0.5, 0.5))
            + tap(p + vec2<f32>(0.5, 0.5))) * 2.0) / 12.0;
    if (blur.amount >= 1.0) {
        return result;
    }
    let size = vec2<f32>(textureDimensions(lower));
    return mix(textureSampleLevel(lower, lower_sampler, position.xy / size, 0.0), result, blur.amount);
}

struct Gauss {
    texel: vec2<f32>,
    limit: vec2<f32>,
    step: vec2<f32>,     // one texel along the direction
    // The weight at each distance, from 0 to 7.
    near: vec4<f32>,
    far: vec4<f32>,
}

@group(0) @binding(0) var<uniform> gauss: Gauss;

fn gauss_tap(p: vec2<f32>) -> vec4<f32> {
    let inside = clamp(p, vec2<f32>(0.5), gauss.limit - vec2<f32>(0.5));
    return textureSampleLevel(source, source_sampler, inside * gauss.texel, 0.0);
}

// A blur too small for going down, along one direction. Both ends read a texel exactly.
@fragment
fn fs_gauss(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = position.xy;
    var sum = gauss_tap(p) * gauss.near.x;
    for (var i = 1; i < 8; i++) {
        let w = select(gauss.far[i - 4], gauss.near[i], i < 4);
        if (w <= 0.0) {
            break;
        }
        let d = gauss.step * f32(i);
        sum += (gauss_tap(p + d) + gauss_tap(p - d)) * w;
    }
    return sum;
}

// ---------------------------------------------------------------------------------------------
// A whole texture, replacing what is there: from where a window is drawn to where it is shown.

@group(0) @binding(0) var shown: texture_2d<f32>;
@group(0) @binding(1) var shown_sampler: sampler;

@fragment
fn fs_show(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(shown));
    return textureSampleLevel(shown, shown_sampler, position.xy / size, 0.0);
}

// ---------------------------------------------------------------------------------------------
// What is behind a box, shown in the box. The box can be transformed, while what it shows stays
// where it is on the target.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var behind: texture_2d<f32>;
@group(1) @binding(1) var behind_sampler: sampler;

struct BackdropInstance {
    @location(0) matrix: vec4<f32>,
    @location(1) offset: vec2<f32>,    // where the box's top-left corner goes
    @location(2) size: vec2<f32>,      // of the box
    @location(3) radii: vec4<f32>,
    @location(4) clip: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
    @location(6) origin: vec2<f32>,    // the position on the target of the first texel
    @location(7) limit: vec2<f32>,     // the size in texels of what the texture holds
    @location(8) texel: vec2<f32>,
    @location(9) opacity: f32,
}

struct BackdropOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) size: vec2<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) clip_radii: vec4<f32>,
    @location(5) @interpolate(flat) origin: vec2<f32>,
    @location(6) @interpolate(flat) limit: vec2<f32>,
    @location(7) @interpolate(flat) texel: vec2<f32>,
    @location(8) @interpolate(flat) opacity: f32,
    // How far a pixel of the target is in the box's coordinates.
    @location(9) @interpolate(flat) pixel: f32,
}

@vertex
fn vs_backdrop(@builtin(vertex_index) vi: u32, inst: BackdropInstance) -> BackdropOut {
    var out: BackdropOut;
    let m = inst.matrix;
    let scale = sqrt(abs(m.x * m.w - m.y * m.z));
    let pixel = 1.0 / max(scale, 0.001);
    // A pixel more than the box, for the edge to fade over when it isn't on whole pixels.
    let c = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let local = c * (inst.size + vec2<f32>(2.0 * pixel)) - vec2<f32>(pixel);
    let p = vec2<f32>(m.x * local.x + m.z * local.y, m.y * local.x + m.w * local.y) + inst.offset;
    let ndc = p / globals.viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.local = local;
    out.size = inst.size;
    out.radii = inst.radii;
    out.clip = inst.clip;
    out.clip_radii = inst.clip_radii;
    out.origin = inst.origin;
    out.limit = inst.limit;
    out.texel = inst.texel;
    out.opacity = inst.opacity;
    out.pixel = pixel;
    return out;
}

fn sd_rounded_box(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    var r: f32;
    if (p.x < 0.0) {
        r = select(radii.w, radii.x, p.y < 0.0);
    } else {
        r = select(radii.z, radii.y, p.y < 0.0);
    }
    r = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_backdrop(in: BackdropOut) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let c = in.clip;
    if (p.x < c.x || p.y < c.y || p.x >= c.x + c.z || p.y >= c.y + c.w) {
        discard;
    }
    var cover = clamp(0.5 - sd_rounded_box(in.local - in.size * 0.5, in.size * 0.5, in.radii) / in.pixel, 0.0, 1.0);
    if (max(max(in.clip_radii.x, in.clip_radii.y), max(in.clip_radii.z, in.clip_radii.w)) > 0.0) {
        let half = c.zw * 0.5;
        cover *= clamp(0.5 - sd_rounded_box(p - c.xy - half, half, in.clip_radii), 0.0, 1.0);
    }
    if (cover <= 0.0) {
        discard;
    }
    let t = clamp(p - in.origin, vec2<f32>(0.5), in.limit - vec2<f32>(0.5));
    return textureSampleLevel(behind, behind_sampler, t * in.texel, 0.0) * (cover * in.opacity);
}
