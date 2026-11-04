// The mark of a checkbox: a check that draws itself from its left end, or a bar when `gravity`
// has pulled its middle to the center. Fractions of the box, like `Checkbox.kt`.

fn stroke(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half: f32) -> f32 {
    let along = b - a;
    let length_of = max(length(along), 0.0001);
    let dir = along / length_of;
    let q = p - a;
    let x = abs(dot(q, dir) - length_of * 0.5) - length_of * 0.5 - half;
    let y = abs(dot(q, vec2<f32>(-dir.y, dir.x))) - half;
    return max(x, y);
}

fn paint(f: Frag) -> vec4<f32> {
    let size = f32(aw_box.size.x);
    let g = params.gravity;
    let left = vec2<f32>(0.2, 0.5) * size;
    let middle = vec2<f32>(mix(0.4, 0.5, g), mix(0.7, 0.5, g)) * size;
    let right = vec2<f32>(0.8, mix(0.3, 0.5, g)) * size;
    let first = distance(left, middle);
    let second = distance(middle, right);
    let drawn = (first + second) * clamp(params.fraction, 0.0, 1.0);
    if (drawn <= 0.0) {
        return vec4<f32>(0.0);
    }
    let half = params.stroke * aw_box.scale * 0.5;
    var d = stroke(f.px, left, mix(left, middle, min(drawn / first, 1.0)), half);
    if (drawn > first) {
        let rest = (drawn - first) / max(second, 0.0001);
        d = min(d, stroke(f.px, middle, mix(middle, right, rest), half));
    }
    return params.color * aw_coverage(d, f.pixel);
}
