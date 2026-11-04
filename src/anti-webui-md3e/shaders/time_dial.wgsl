// The face of a time picker: a circle, and the hand with its dot and handle. `angle` is in radians
// clockwise from 3 o'clock; lengths in the parameters are logical pixels.

fn over(top: vec4<f32>, below: vec4<f32>) -> vec4<f32> {
    return top + below * (1.0 - top.a);
}

fn paint(f: Frag) -> vec4<f32> {
    let s = aw_box.scale;
    let size = vec2<f32>(aw_box.size);
    let center = size * 0.5;
    let face = aw_coverage(distance(f.px, center) - min(size.x, size.y) * 0.5, f.pixel);
    var color = params.face * face;
    let direction = vec2<f32>(cos(params.angle), sin(params.angle));
    let handle_r = params.handle * s * 0.5;
    let hand_end = center + direction * params.length * s;
    let handle = aw_coverage(distance(f.px, hand_end) - handle_r, f.pixel);
    let along = clamp(dot(f.px - center, direction), 0.0, params.length * s - handle_r);
    let line = aw_coverage(distance(f.px, center + direction * along) - s, f.pixel);
    let dot_at = aw_coverage(distance(f.px, center) - 4.0 * s, f.pixel);
    color = over(params.hand * max(line, dot_at), color);
    return over(params.hand * handle, color);
}
