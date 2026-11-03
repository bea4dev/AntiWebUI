
struct AwInstance {
    @location(0) area: vec4<f32>,         // what to draw, in the target
    @location(1) rect: vec4<f32>,         // the box, in the target
    @location(2) rect_radii: vec4<f32>,   // its corners, in the target
    @location(3) clip: vec4<f32>,
    @location(4) clip_radii: vec4<f32>,
    @location(5) radii: vec4<f32>,        // its corners, as the shader sees them
    @location(6) border: vec4<i32>,
    @location(7) padding: vec4<i32>,
    @location(8) size: vec2<i32>,
    @location(9) misc: vec3<f32>,         // texels per pixel, scale, opacity
    @location(10) index: u32,             // of the parameters
}

struct AwOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) rect: vec4<f32>,
    @location(1) @interpolate(flat) rect_radii: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
    @location(3) @interpolate(flat) clip_radii: vec4<f32>,
    @location(4) @interpolate(flat) radii: vec4<f32>,
    @location(5) @interpolate(flat) border: vec4<i32>,
    @location(6) @interpolate(flat) padding: vec4<i32>,
    @location(7) @interpolate(flat) size: vec2<i32>,
    @location(8) @interpolate(flat) misc: vec3<f32>,
    @location(9) @interpolate(flat) index: u32,
}

@vertex
fn aw_vs(@builtin(vertex_index) vi: u32, inst: AwInstance) -> AwOut {
    var out: AwOut;
    let c = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = inst.area.xy + c * inst.area.zw;
    let ndc = p / aw_globals.viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.rect = inst.rect;
    out.rect_radii = inst.rect_radii;
    out.clip = inst.clip;
    out.clip_radii = inst.clip_radii;
    out.radii = inst.radii;
    out.border = inst.border;
    out.padding = inst.padding;
    out.size = inst.size;
    out.misc = inst.misc;
    out.index = inst.index;
    return out;
}

@fragment
fn aw_fs(in: AwOut) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    var cover = 1.0;
    let c = in.clip;
    if (p.x < c.x || p.y < c.y || p.x >= c.x + c.z || p.y >= c.y + c.w) {
        discard;
    }
    if (max(max(in.clip_radii.x, in.clip_radii.y), max(in.clip_radii.z, in.clip_radii.w)) > 0.0) {
        cover = aw_coverage(aw_rounded_box_distance(p, c, in.clip_radii), 1.0);
    }
    if (max(max(in.rect_radii.x, in.rect_radii.y), max(in.rect_radii.z, in.rect_radii.w)) > 0.0) {
        cover *= aw_coverage(aw_rounded_box_distance(p, in.rect, in.rect_radii), 1.0);
    }
    if (cover <= 0.0) {
        discard;
    }
    aw_box = AwBox(in.size, in.border, in.padding, in.radii, in.misc.y);
    params = aw_params[in.index];
    var f: Frag;
    f.px = (p - in.rect.xy) / in.misc.x;
    f.ipx = vec2<i32>(floor(f.px));
    f.pixel = 1.0 / in.misc.x;
    return paint(f) * (cover * in.misc.z);
}
