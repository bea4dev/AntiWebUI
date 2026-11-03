// A shape given by how far its outline is from the center in each direction.

fn radius_at(u: f32) -> f32 {
    let i = i32(floor(u)) % 128;
    let t = params.table[i / 4];
    return t[i % 4];
}

fn paint(f: Frag) -> vec4<f32> {
    let size = vec2<f32>(aw_box.size);
    let side = min(size.x, size.y) * params.scale;
    let p = f.px - size * 0.5;
    let distance_to_center = length(p);
    var angle = atan2(p.y, p.x) - params.rotation;
    angle = fract(angle / 6.2831853) * 128.0;
    let a = radius_at(angle);
    let b = radius_at(angle + 1.0);
    let radius = mix(a, b, fract(angle)) * side;
    // How steep the outline is here, to turn the difference along the ray into a distance to it.
    let slope = (b - a) * side * 128.0 / 6.2831853 / max(distance_to_center, 0.001);
    let d = (distance_to_center - radius) / sqrt(1.0 + slope * slope);
    return params.color * aw_coverage(d, f.pixel);
}
