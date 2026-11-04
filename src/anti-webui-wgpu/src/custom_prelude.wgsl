// What a custom shader can use. See docs/design/14-shaders.md. Names starting with aw_ are reserved.

struct AwGlobals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> aw_globals: AwGlobals;

struct Frag {
    px: vec2<f32>,    // from the box's top-left corner, in physical pixels; pixel centers are at .5
    ipx: vec2<i32>,   // the same position as whole pixels
    pixel: f32,       // how many physical pixels one output pixel takes
}

struct AwBox {
    size: vec2<i32>,     // with the border
    border: vec4<i32>,   // top, right, bottom, left
    padding: vec4<i32>,  // top, right, bottom, left
    radii: vec4<f32>,    // top-left, top-right, bottom-right, bottom-left
    scale: f32,          // physical pixels per logical pixel
}

var<private> aw_box: AwBox;

struct AwInput {
    origin: vec2<f32>,   // the position of the box in the texture, in texels
    texel: vec2<f32>,    // the size of a texel in the coordinates of the texture
    limit: vec2<f32>,    // the size in texels of what the texture holds
    k: f32,              // texels per pixel
}

@group(2) @binding(0) var aw_input0: texture_2d<f32>;
@group(2) @binding(1) var aw_input1: texture_2d<f32>;
@group(2) @binding(2) var aw_input2: texture_2d<f32>;
@group(2) @binding(3) var aw_input3: texture_2d<f32>;
@group(2) @binding(4) var aw_input_sampler: sampler;
@group(2) @binding(5) var<uniform> aw_inputs: array<AwInput, 4>;

// Where to read an input for a position in the box. Past what it holds is the color at its edge.
fn aw_input_at(px: vec2<f32>, input: AwInput) -> vec2<f32> {
    let t = clamp(px * input.k + input.origin, vec2<f32>(0.5), input.limit - vec2<f32>(0.5));
    return t * input.texel;
}

// The inputs, at a position in the box in physical pixels. Transparent if there is none.
fn read0(px: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(aw_input0, aw_input_sampler, aw_input_at(px, aw_inputs[0]), 0.0);
}

fn read1(px: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(aw_input1, aw_input_sampler, aw_input_at(px, aw_inputs[1]), 0.0);
}

fn read2(px: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(aw_input2, aw_input_sampler, aw_input_at(px, aw_inputs[2]), 0.0);
}

fn read3(px: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(aw_input3, aw_input_sampler, aw_input_at(px, aw_inputs[3]), 0.0);
}

// The signed distance to a rounded box centered at the origin; negative inside.
fn aw_sd_rounded_box(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
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

// The signed distance to a rounded rectangle (x, y, width, height).
fn aw_rounded_box_distance(p: vec2<f32>, rect: vec4<f32>, radii: vec4<f32>) -> f32 {
    let half = rect.zw * 0.5;
    return aw_sd_rounded_box(p - rect.xy - half, half, radii);
}

// The signed distance to the shape of the box.
fn aw_shape_distance(px: vec2<f32>) -> f32 {
    let half = vec2<f32>(aw_box.size) * 0.5;
    return aw_sd_rounded_box(px - half, half, aw_box.radii);
}

// How much of a pixel is inside, given its distance to an edge.
fn aw_coverage(distance: f32, pixel: f32) -> f32 {
    return clamp(0.5 - distance / pixel, 0.0, 1.0);
}

// The inside of the border (x, y, width, height).
fn aw_padding_rect() -> vec4<f32> {
    let b = vec4<f32>(aw_box.border);
    return vec4<f32>(b.w, b.x, f32(aw_box.size.x) - b.y - b.w, f32(aw_box.size.y) - b.x - b.z);
}

// The inside of the padding (x, y, width, height).
fn aw_content_rect() -> vec4<f32> {
    let p = vec4<f32>(aw_box.padding);
    let r = aw_padding_rect();
    return vec4<f32>(r.x + p.w, r.y + p.x, r.z - p.y - p.w, r.w - p.x - p.z);
}

