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

// ---------------------------------------------------------------------------------------------
// Pictures. See docs/design/22-images.md.

@group(1) @binding(0) var picture: texture_2d<f32>;
@group(2) @binding(0) var picture_sampler: sampler;

struct PictureInstance {
    @location(0) area: vec4<f32>,  // what this instance draws
    @location(1) rect: vec4<f32>,  // where all of the picture goes
    @location(2) shape: vec4<f32>,
    @location(3) shape_radii: vec4<f32>,
    @location(4) clip: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
    @location(6) params: vec2<f32>, // x: level to read, y: opacity
}

struct PictureOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) shape: vec4<f32>,
    @location(2) @interpolate(flat) shape_radii: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) clip_radii: vec4<f32>,
    @location(5) @interpolate(flat) params: vec2<f32>,
}

@vertex
fn vs_picture(@builtin(vertex_index) vi: u32, inst: PictureInstance) -> PictureOut {
    var out: PictureOut;
    let p = inst.area.xy + corner(vi) * inst.area.zw;
    out.position = to_clip(p);
    out.uv = (p - inst.rect.xy) / inst.rect.zw;
    out.shape = inst.shape;
    out.shape_radii = inst.shape_radii;
    out.clip = inst.clip;
    out.clip_radii = inst.clip_radii;
    out.params = inst.params;
    return out;
}

@fragment
fn fs_picture(in: PictureOut) -> @location(0) vec4<f32> {
    var coverage = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (max(max(in.shape_radii.x, in.shape_radii.y), max(in.shape_radii.z, in.shape_radii.w)) > 0.0) {
        coverage *= rrect_coverage(in.position.xy, in.shape, in.shape_radii);
    }
    if (coverage <= 0.0) {
        discard;
    }
    return textureSampleLevel(picture, picture_sampler, in.uv, in.params.x) * (in.params.y * coverage);
}

// Nothing shows through it and nothing cuts it: most of a page of photos.
@fragment
fn fs_opaque_picture(in: PictureOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(picture, picture_sampler, in.uv, in.params.x);
}

// Frames of videos. See docs/design/24-video.md.

@group(1) @binding(0) var video_y: texture_2d<f32>;
@group(1) @binding(1) var video_c: texture_2d<f32>;
@group(1) @binding(2) var video_v: texture_2d<f32>;
@group(1) @binding(3) var video_sampler: sampler;

struct VideoInstance {
    @location(0) area: vec4<f32>,  // what this instance draws
    @location(1) rect: vec4<f32>,  // where all of the frame goes
    @location(2) shape: vec4<f32>,
    @location(3) shape_radii: vec4<f32>,
    @location(4) clip: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
    @location(6) red: vec4<f32>,   // how the numbers of a frame become colors
    @location(7) green: vec4<f32>,
    @location(8) blue: vec4<f32>,
    @location(9) params: vec4<f32>, // x: opacity, y: degrees clockwise, z: 3 for two chroma textures, w: transfer
    @location(10) sampling: vec4<f32>, // xy: the size of a texel, z: how many a pixel covers
}

struct VideoOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) shape: vec4<f32>,
    @location(2) @interpolate(flat) shape_radii: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) clip_radii: vec4<f32>,
    @location(5) @interpolate(flat) red: vec4<f32>,
    @location(6) @interpolate(flat) green: vec4<f32>,
    @location(7) @interpolate(flat) blue: vec4<f32>,
    @location(8) @interpolate(flat) params: vec4<f32>,
    @location(9) @interpolate(flat) sampling: vec4<f32>,
}

@vertex
fn vs_video(@builtin(vertex_index) vi: u32, inst: VideoInstance) -> VideoOut {
    var out: VideoOut;
    let p = inst.area.xy + corner(vi) * inst.area.zw;
    out.position = to_clip(p);
    let shown = (p - inst.rect.xy) / inst.rect.zw;
    // Where on the stored picture this is, with the picture turned as the file says.
    let degrees = inst.params.y;
    if (degrees == 90.0) {
        out.uv = vec2<f32>(shown.y, 1.0 - shown.x);
    } else if (degrees == 180.0) {
        out.uv = vec2<f32>(1.0 - shown.x, 1.0 - shown.y);
    } else if (degrees == 270.0) {
        out.uv = vec2<f32>(1.0 - shown.y, shown.x);
    } else {
        out.uv = shown;
    }
    out.shape = inst.shape;
    out.shape_radii = inst.shape_radii;
    out.clip = inst.clip;
    out.clip_radii = inst.clip_radii;
    out.red = inst.red;
    out.green = inst.green;
    out.blue = inst.blue;
    out.params = inst.params;
    out.sampling = inst.sampling;
    return out;
}

fn video_numbers(uv: vec2<f32>, three: bool) -> vec3<f32> {
    let y = textureSampleLevel(video_y, video_sampler, uv, 0.0).r;
    let c = textureSampleLevel(video_c, video_sampler, uv, 0.0);
    if (three) {
        return vec3<f32>(y, c.r, textureSampleLevel(video_v, video_sampler, uv, 0.0).r);
    }
    return vec3<f32>(y, c.r, c.g);
}

// Light in nits from the numbers of PQ.
fn pq_to_nits(x: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(max(x, vec3<f32>(0.0)), vec3<f32>(1.0 / m2));
    return 10000.0 * pow(max(p - c1, vec3<f32>(0.0)) / (c2 - c3 * p), vec3<f32>(1.0 / m1));
}

// Light from 0 to 1 from the numbers of HLG, for a screen of 1000 nits.
fn hlg_to_linear(x: vec3<f32>) -> vec3<f32> {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    let low = x * x / 3.0;
    let high = (exp((x - c) / a) + b) / 12.0;
    return select(high, low, x <= vec3<f32>(0.5));
}

// What is brighter than white comes down gently, and what isn't stays as it is.
fn compress_highlights(light: vec3<f32>) -> vec3<f32> {
    let peak = max(max(light.r, light.g), light.b);
    if (peak <= 0.75) {
        return light;
    }
    // Maps 0.75 to 0.75, keeps going up, and doesn't quite reach 1.
    let over = peak - 0.75;
    let down = 0.75 + 0.25 * over / (over + 0.25);
    return light * (down / peak);
}

// The numbers a screen in sRGB takes for what is in BT.2020.
fn hdr_to_sdr(numbers: vec3<f32>, transfer: f32) -> vec3<f32> {
    var light: vec3<f32>;
    if (transfer == 1.0) {
        // What is called white in a video is about 203 nits.
        light = pq_to_nits(numbers) / 203.0;
    } else {
        light = hlg_to_linear(numbers) * 1000.0 / 203.0;
    }
    let bt709 = vec3<f32>(
        1.6605 * light.r - 0.5876 * light.g - 0.0728 * light.b,
        -0.1246 * light.r + 1.1329 * light.g - 0.0083 * light.b,
        -0.0182 * light.r - 0.1006 * light.g + 1.1187 * light.b,
    );
    let out = compress_highlights(max(bt709, vec3<f32>(0.0)));
    let low = out * 12.92;
    let high = 1.055 * pow(out, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, out <= vec3<f32>(0.0031308));
}

@fragment
fn fs_video(in: VideoOut) -> @location(0) vec4<f32> {
    var coverage = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (max(max(in.shape_radii.x, in.shape_radii.y), max(in.shape_radii.z, in.shape_radii.w)) > 0.0) {
        coverage *= rrect_coverage(in.position.xy, in.shape, in.shape_radii);
    }
    if (coverage <= 0.0) {
        discard;
    }
    let three = in.params.z == 3.0;
    var numbers: vec3<f32>;
    if (in.sampling.z > 0.0) {
        // Much smaller than it is: some of what a pixel covers, not just the middle.
        let d = in.sampling.xy * in.sampling.z * 0.5;
        numbers = (video_numbers(in.uv + vec2<f32>(-d.x, -d.y), three)
            + video_numbers(in.uv + vec2<f32>(d.x, -d.y), three)
            + video_numbers(in.uv + vec2<f32>(-d.x, d.y), three)
            + video_numbers(in.uv + vec2<f32>(d.x, d.y), three)) * 0.25;
    } else {
        numbers = video_numbers(in.uv, three);
    }
    let n = vec4<f32>(numbers, 1.0);
    var rgb = clamp(vec3<f32>(dot(in.red, n), dot(in.green, n), dot(in.blue, n)), vec3<f32>(0.0), vec3<f32>(1.0));
    if (in.params.w != 0.0) {
        rgb = hdr_to_sdr(rgb, in.params.w);
    }
    let alpha = in.params.x * coverage;
    return vec4<f32>(rgb * alpha, alpha);
}

// ---------------------------------------------------------------------------------------------
// Shapes: what they cover was worked out on the CPU, and what they are painted with is in a table.
// See docs/design/25-svg.md.

@group(1) @binding(0) var path_store: texture_2d<f32>;
@group(1) @binding(1) var path_table: texture_2d<f32>;

const PATH_STORE_WIDTH: u32 = 2048u;
const PATH_TABLE_WIDTH: u32 = 1024u;
const PATH_SOLID: u32 = 2147483648u;

fn path_row(i: u32) -> vec4<f32> {
    return textureLoad(
        path_table,
        vec2<i32>(i32(i % PATH_TABLE_WIDTH), i32(i / PATH_TABLE_WIDTH)),
        0
    );
}

struct PathInstance {
    @location(0) pos: vec2<i32>,
    @location(1) size: vec2<u32>,
    @location(2) alpha: u32,
    @location(3) draw: u32,
}

struct PathOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) clip: vec4<f32>,
    @location(2) @interpolate(flat) clip_radii: vec4<f32>,
    @location(3) @interpolate(flat) origin: vec2<i32>,
    @location(4) @interpolate(flat) width: u32,
    @location(5) @interpolate(flat) alpha: u32,
    @location(6) @interpolate(flat) draw: u32,
}

@vertex
fn vs_path(@builtin(vertex_index) vi: u32, inst: PathInstance) -> PathOut {
    var out: PathOut;
    let c = corner(vi);
    out.position = to_clip(vec2<f32>(inst.pos) + c * vec2<f32>(inst.size));
    out.color = path_row(inst.draw);
    out.clip = path_row(inst.draw + 1u);
    out.clip_radii = path_row(inst.draw + 2u);
    out.origin = inst.pos;
    out.width = inst.size.x;
    out.alpha = inst.alpha;
    out.draw = inst.draw;
    return out;
}

/// The color of a gradient at a pixel, premultiplied.
fn path_gradient(p: vec2<f32>, kind: f32, first: u32, count: u32, spread: f32, draw: u32) -> vec4<f32> {
    let m0 = path_row(draw + 4u);
    let m1 = path_row(draw + 5u);
    let q = vec2<f32>(
        m0.x * p.x + m0.z * p.y + m1.x,
        m0.y * p.x + m0.w * p.y + m1.y
    );
    var t = q.x;
    if (kind > 1.5) {
        // Where the ray from the first stop through this pixel leaves the circle.
        let f = m1.zw;
        let d = q - f;
        let a = dot(d, d);
        let b = dot(f, d);
        let c = dot(f, f) - 1.0;
        t = 0.0;
        if (a > 0.0) {
            t = a / (-b + sqrt(max(b * b - a * c, 0.0)));
        }
    }
    if (spread > 1.5) {
        t = fract(t);
    } else if (spread > 0.5) {
        t = 1.0 - abs(fract(t * 0.5) * 2.0 - 1.0);
    } else {
        t = clamp(t, 0.0, 1.0);
    }
    var previous = path_row(first);
    var previous_at = path_row(first + 1u).x;
    var color = previous;
    if (t > previous_at) {
        color = path_row(first + (count - 1u) * 2u);
        for (var i = 1u; i < count; i++) {
            let next = path_row(first + i * 2u);
            let next_at = path_row(first + i * 2u + 1u).x;
            if (t <= next_at) {
                let span = next_at - previous_at;
                var k = 1.0;
                if (span > 0.0) {
                    k = (t - previous_at) / span;
                }
                color = mix(previous, next, k);
                break;
            }
            previous = next;
            previous_at = next_at;
        }
    }
    return vec4<f32>(color.rgb * color.a, color.a);
}

@fragment
fn fs_path(in: PathOut) -> @location(0) vec4<f32> {
    let clip_cov = clip_coverage(in.position.xy, in.clip, in.clip_radii);
    if (clip_cov <= 0.0) {
        discard;
    }
    var cover: f32;
    if ((in.alpha & PATH_SOLID) != 0u) {
        cover = f32(in.alpha & 255u) / 255.0;
    } else {
        let at = vec2<i32>(floor(in.position.xy)) - in.origin;
        let i = in.alpha + u32(at.y) * in.width + u32(at.x);
        cover = textureLoad(
            path_store,
            vec2<i32>(i32(i % PATH_STORE_WIDTH), i32(i / PATH_STORE_WIDTH)),
            0
        ).r;
    }
    var color = in.color;
    let gradient = path_row(in.draw + 3u);
    if (gradient.x > 0.5) {
        let shaded = path_gradient(
            in.position.xy,
            gradient.x,
            u32(gradient.y),
            u32(gradient.z),
            gradient.w,
            in.draw
        );
        color = shaded * in.color.a;
    }
    return color * (cover * clip_cov);
}
