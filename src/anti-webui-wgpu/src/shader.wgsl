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

// Coverage of a pixel by a rounded rectangle, anti-aliased.
fn rrect_coverage(p: vec2<f32>, rect: vec4<f32>, radii: vec4<f32>) -> f32 {
    let half = rect.zw * 0.5;
    return clamp(0.5 - sd_rounded_box(p - (rect.xy + half), half, radii), 0.0, 1.0);
}

// ---------------------------------------------------------------------------------------------
// Solid rectangles on whole pixels, which need no coverage. Most of a large box is one of these.

struct SolidOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs_solid(
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
) -> SolidOut {
    var out: SolidOut;
    out.position = to_clip(rect.xy + corner(vi) * rect.zw);
    out.color = color;
    return out;
}

@fragment
fn fs_solid(in: SolidOut) -> @location(0) vec4<f32> {
    return in.color;
}

// ---------------------------------------------------------------------------------------------
// Quads and shadows. They share a pipeline so that a page of boxes with shadows is one draw call.

struct QuadInstance {
    @location(0) area: vec4<f32>,         // what this instance draws: all or part of the shape
    @location(1) shape: vec4<f32>,        // the box; shadows: the shape that is blurred
    @location(2) background: vec4<f32>,   // shadows: the color
    @location(3) border_color: vec4<f32>,
    @location(4) border: vec4<f32>,       // top, right, bottom, left; shadows: the cut
    @location(5) radii: vec4<f32>,        // tl, tr, br, bl; shadows: of the shape
    @location(6) clip: vec4<f32>,
    @location(7) clip_radii: vec4<f32>,
    @location(8) cut_radii: vec4<f32>,    // shadows only
    @location(9) params: vec2<f32>,       // x: 0 quad, 1 outer shadow, 2 inset shadow; y: sigma
}

struct QuadOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) shape: vec4<f32>,
    @location(2) @interpolate(flat) background: vec4<f32>,
    @location(3) @interpolate(flat) border_color: vec4<f32>,
    @location(4) @interpolate(flat) border: vec4<f32>,
    @location(5) @interpolate(flat) radii: vec4<f32>,
    @location(6) @interpolate(flat) clip: vec4<f32>,
    @location(7) @interpolate(flat) clip_radii: vec4<f32>,
    @location(8) @interpolate(flat) cut_radii: vec4<f32>,
    @location(9) @interpolate(flat) params: vec2<f32>,
}

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32, inst: QuadInstance) -> QuadOut {
    var out: QuadOut;
    let p = inst.area.xy + corner(vi) * inst.area.zw;
    out.position = to_clip(p);
    out.local = p;
    out.shape = inst.shape;
    out.background = inst.background;
    out.border_color = inst.border_color;
    out.border = inst.border;
    out.radii = inst.radii;
    out.clip = inst.clip;
    out.clip_radii = inst.clip_radii;
    out.cut_radii = inst.cut_radii;
    out.params = inst.params;
    return out;
}

@fragment
fn fs_quad(in: QuadOut) -> @location(0) vec4<f32> {
    let clip_cov = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (clip_cov <= 0.0) {
        discard;
    }
    if (in.params.x > 0.5) {
        let cut = rrect_coverage(in.local, in.border, in.cut_radii);
        let shape = blurred_coverage(in.local, in.shape, in.radii, in.params.y);
        var a: f32;
        if (in.params.x > 1.5) {
            a = (1.0 - shape) * cut;
        } else {
            a = shape * (1.0 - cut);
        }
        return in.background * (a * clip_cov);
    }

    let half = in.shape.zw * 0.5;
    let center = in.shape.xy + half;
    let p = in.local - center;

    let outer = sd_rounded_box(p, half, in.radii);
    let outer_cov = clamp(0.5 - outer, 0.0, 1.0);
    var color = in.background * outer_cov;

    let b = in.border;
    if (b.x + b.y + b.z + b.w > 0.0) {
        let inner_min = in.shape.xy + vec2<f32>(b.w, b.x);
        let inner_size = max(in.shape.zw - vec2<f32>(b.y + b.w, b.x + b.z), vec2<f32>(0.0));
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
// A rounded rectangle blurred by a Gaussian, after Evan Wallace's "Fast Rounded Rectangle
// Shadows": exact along x, four samples along y. A shadow's instances draw the area around what
// its cut hides (see docs/design/09-shadows-stacking-popups.md).

fn erf2(v: vec2<f32>) -> vec2<f32> {
    let s = sign(v);
    let a = abs(v);
    var x = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    x = x * x;
    return s - s / (x * x);
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066282746 * sigma);
}

// How much of the row `y` the shape covers at `x` after blurring along x.
fn blurred_row(x: f32, y: f32, sigma: f32, r: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - r - abs(y), 0.0);
    let curved = half.x - r + sqrt(max(0.0, r * r - delta * delta));
    let integral = 0.5 + 0.5 * erf2((vec2<f32>(x) + vec2<f32>(-curved, curved)) * (0.70710678 / sigma));
    return integral.y - integral.x;
}

fn blurred_coverage(p: vec2<f32>, rect: vec4<f32>, radii: vec4<f32>, sigma: f32) -> f32 {
    if (rect.z <= 0.0 || rect.w <= 0.0) {
        return 0.0;
    }
    if (sigma < 0.05) {
        return rrect_coverage(p, rect, radii);
    }
    let half = rect.zw * 0.5;
    let q = p - rect.xy - half;
    var r: f32;
    if (q.x < 0.0) {
        r = select(radii.w, radii.x, q.y < 0.0);
    } else {
        r = select(radii.z, radii.y, q.y < 0.0);
    }
    r = min(r, min(half.x, half.y));
    let low = q.y - half.y;
    let high = q.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var value = 0.0;
    for (var i = 0; i < 4; i++) {
        value += blurred_row(q.x, q.y - y, sigma, r, half) * gaussian(y, sigma) * step;
        y += step;
    }
    return clamp(value, 0.0, 1.0);
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

// ---------------------------------------------------------------------------------------------
// Layers: content kept in a texture, shown through a transform. See docs/design/12-transform.md.
// This is where an effect on the content as a whole would go.

struct LayerInstance {
    @location(0) matrix: vec4<f32>,  // a, b, c, d
    @location(1) offset: vec2<f32>,
    @location(2) size: vec2<f32>,    // in texels
    @location(3) uv: vec4<f32>,
    @location(4) clip: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
    @location(6) opacity: f32,
}

struct LayerOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) clip: vec4<f32>,
    @location(2) @interpolate(flat) clip_radii: vec4<f32>,
    @location(3) @interpolate(flat) opacity: f32,
}

@vertex
fn vs_layer(@builtin(vertex_index) vi: u32, inst: LayerInstance) -> LayerOut {
    var out: LayerOut;
    let c = corner(vi);
    let p = c * inst.size;
    let m = inst.matrix;
    out.position = to_clip(vec2<f32>(m.x * p.x + m.z * p.y, m.y * p.x + m.w * p.y) + inst.offset);
    out.uv = mix(inst.uv.xy, inst.uv.zw, c);
    out.clip = inst.clip;
    out.clip_radii = inst.clip_radii;
    out.opacity = inst.opacity;
    return out;
}

@fragment
fn fs_layer(in: LayerOut) -> @location(0) vec4<f32> {
    let clip_cov = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (clip_cov <= 0.0) {
        discard;
    }
    return textureSampleLevel(atlas, atlas_sampler, in.uv, 0.0) * (in.opacity * clip_cov);
}
