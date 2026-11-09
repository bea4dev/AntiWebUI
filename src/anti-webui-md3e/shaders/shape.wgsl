// A shape given by how far its outline is from a point in each direction, stretched over the box
// and turned around its middle.

fn radius_at(u: f32) -> f32 {
    let i = i32(floor(u)) % 256;
    let t = params.table[i / 4];
    return t[i % 4];
}

fn paint(f: Frag) -> vec4<f32> {
    let size = vec2<f32>(aw_box.size);
    let side = size * params.scale;
    let turn = vec2<f32>(cos(params.rotation), sin(params.rotation));
    let p = f.px - size * 0.5;
    // Turned back, then in the units of the table.
    let q = vec2<f32>(turn.x * p.x + turn.y * p.y, turn.x * p.y - turn.y * p.x);
    let u = q / side - params.origin;
    let distance_to_origin = max(length(u), 0.000001);
    let at = fract(atan2(u.y, u.x) / 6.2831853) * 256.0;
    let a = radius_at(at);
    let b = radius_at(at + 1.0);
    let radius = mix(a, b, fract(at));
    // How the difference along the ray changes across the box, to turn it into a distance in
    // pixels, however much the box stretches the shape.
    let slope = (b - a) * 256.0 / 6.2831853 / distance_to_origin;
    let along = u / distance_to_origin;
    let across = vec2<f32>(-along.y, along.x);
    let gradient = (along - slope * across) / side;
    var d = (distance_to_origin - radius) / max(length(gradient), 0.000001);
    // Near the point the table is made around, that estimate falls apart where the outline is
    // steep, but all within the nearest part of the outline is at least that far inside.
    d = min(d, (distance_to_origin - params.least) * min(side.x, side.y));
    var coverage = aw_coverage(d, f.pixel);
    if (params.stroke > 0.0) {
        coverage = coverage - aw_coverage(d + params.stroke * aw_box.scale, f.pixel);
    }
    return params.color * coverage;
}
