// AntiWebUI renderer shaders. All positions are physical pixels with the origin at the top-left.
// Colors are premultiplied.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

fn to_clip(p: vec2<f32>) -> vec4<f32> {
    let ndc = p / globals.viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(ndc, 0.0, 1.0);
}

fn corner(vi: u32) -> vec2<f32> {
    return vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
}

fn inside_clip(p: vec2<f32>, clip: vec4<f32>) -> bool {
    return p.x >= clip.x && p.y >= clip.y && p.x < clip.x + clip.z && p.y < clip.y + clip.w;
}

// Coverage of a pixel by the clip: 0 or 1 for rectangles, anti-aliased at rounded corners.
fn clip_coverage(p: vec2<f32>, clip: vec4<f32>, radii: vec4<f32>) -> f32 {
    if (!inside_clip(p, clip)) {
        return 0.0;
    }
    if (max(max(radii.x, radii.y), max(radii.z, radii.w)) <= 0.0) {
        return 1.0;
    }
    let half = clip.zw * 0.5;
    return clamp(0.5 - sd_rounded_box(p - (clip.xy + half), half, radii), 0.0, 1.0);
}

// Signed distance to a rounded box centered at the origin. Radii: tl, tr, br, bl (y down).
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

// Abramowitz–Stegun approximation of erf.
fn erf(x: f32) -> f32 {
    let s = sign(x);
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-a * a);
    return s * y;
}

// ---------------------------------------------------------------------------------------------
// Quads and shadows

struct QuadInstance {
    @location(0) rect: vec4<f32>,
    @location(1) background: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    @location(3) border: vec4<f32>,   // top, right, bottom, left
    @location(4) radii: vec4<f32>,    // tl, tr, br, bl
    @location(5) clip: vec4<f32>,
    @location(6) params: vec4<f32>,   // x: kind (0 quad, 1 shadow), y: blur
    @location(7) clip_radii: vec4<f32>,
}

struct QuadOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) background: vec4<f32>,
    @location(3) @interpolate(flat) border_color: vec4<f32>,
    @location(4) @interpolate(flat) border: vec4<f32>,
    @location(5) @interpolate(flat) radii: vec4<f32>,
    @location(6) @interpolate(flat) clip: vec4<f32>,
    @location(7) @interpolate(flat) params: vec4<f32>,
    @location(8) @interpolate(flat) clip_radii: vec4<f32>,
}

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32, inst: QuadInstance) -> QuadOut {
    var out: QuadOut;
    var rect = inst.rect;
    if (inst.params.x > 0.5) {
        // Shadows extend beyond the box by three standard deviations.
        let grow = inst.params.y * 1.5 + 1.0;
        rect = vec4<f32>(rect.xy - vec2<f32>(grow), rect.zw + vec2<f32>(grow * 2.0));
    }
    let p = rect.xy + corner(vi) * rect.zw;
    out.position = to_clip(p);
    out.local = p;
    out.rect = inst.rect;
    out.background = inst.background;
    out.border_color = inst.border_color;
    out.border = inst.border;
    out.radii = inst.radii;
    out.clip = inst.clip;
    out.params = inst.params;
    out.clip_radii = inst.clip_radii;
    return out;
}

@fragment
fn fs_quad(in: QuadOut) -> @location(0) vec4<f32> {
    let clip_cov = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (clip_cov <= 0.0) {
        discard;
    }
    let half = in.rect.zw * 0.5;
    let center = in.rect.xy + half;
    let p = in.local - center;

    if (in.params.x > 0.5) {
        let sigma = max(in.params.y * 0.5, 0.0001);
        let d = sd_rounded_box(p, half, in.radii);
        let a = 0.5 * (1.0 - erf(d / (sigma * 1.41421356)));
        return in.background * (a * clip_cov);
    }

    let outer = sd_rounded_box(p, half, in.radii);
    let outer_cov = clamp(0.5 - outer, 0.0, 1.0);
    var color = in.background * outer_cov;

    let b = in.border;
    if (b.x + b.y + b.z + b.w > 0.0) {
        let inner_min = in.rect.xy + vec2<f32>(b.w, b.x);
        let inner_size = max(in.rect.zw - vec2<f32>(b.y + b.w, b.x + b.z), vec2<f32>(0.0));
        let inner_half = inner_size * 0.5;
        let inner_center = inner_min + inner_half;
        let inner_radii = max(in.radii - vec4<f32>(max(b.w, b.x), max(b.x, b.y), max(b.y, b.z), max(b.z, b.w)), vec4<f32>(0.0));
        var inner_cov = 0.0;
        if (inner_size.x > 0.0 && inner_size.y > 0.0) {
            inner_cov = clamp(0.5 - sd_rounded_box(in.local - inner_center, inner_half, inner_radii), 0.0, 1.0);
        }
        let ring = clamp(outer_cov - inner_cov, 0.0, 1.0);
        color = in.border_color * ring + color * (1.0 - in.border_color.a * ring);
    }
    return color * clip_cov;
}

// ---------------------------------------------------------------------------------------------
// Glyphs

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct GlyphInstance {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) clip: vec4<f32>,
    @location(4) params: vec4<f32>, // x: 1 for color glyphs
    @location(5) clip_radii: vec4<f32>,
}

struct GlyphOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) @interpolate(flat) clip_radii: vec4<f32>,
}

@vertex
fn vs_glyph(@builtin(vertex_index) vi: u32, inst: GlyphInstance) -> GlyphOut {
    var out: GlyphOut;
    let c = corner(vi);
    out.position = to_clip(inst.rect.xy + c * inst.rect.zw);
    out.uv = mix(inst.uv.xy, inst.uv.zw, c);
    out.color = inst.color;
    out.clip = inst.clip;
    out.params = inst.params;
    out.clip_radii = inst.clip_radii;
    return out;
}

@fragment
fn fs_glyph(in: GlyphOut) -> @location(0) vec4<f32> {
    let clip_cov = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (clip_cov <= 0.0) {
        discard;
    }
    let texel = textureSampleLevel(atlas, atlas_sampler, in.uv, 0.0);
    if (in.params.x > 0.5) {
        return texel * (in.color.a * clip_cov);
    }
    return in.color * (texel.a * clip_cov);
}
