// A progress bar: up to two bars, flat or wavy, and up to three pieces of track that are straight.
// Bars and pieces are fractions of the width, as in `Progress.kt`; other lengths are logical pixels.

fn over(top: vec4<f32>, below: vec4<f32>) -> vec4<f32> {
    return top + below * (1.0 - top.a);
}

fn piece(p: vec2<f32>, a: f32, b: f32, half: f32, w: f32, y: f32) -> f32 {
    if (b <= a) {
        return 1000000.0;
    }
    let from_x = clamp(a * w, half, w - half);
    let to_x = clamp(b * w, half, w - half);
    return distance(p, vec2<f32>(clamp(p.x, from_x, to_x), y)) - half;
}

fn bar(p: vec2<f32>, tail: f32, head: f32, half: f32, w: f32, y: f32, amplitude: f32, k: f32) -> f32 {
    if (head <= tail) {
        return 1000000.0;
    }
    let lo = clamp(tail * w, half, w - half);
    let hi = clamp(head * w, half, w - half);
    if (amplitude <= 0.0) {
        return distance(p, vec2<f32>(clamp(p.x, lo, hi), y)) - half;
    }
    // The closest point on the line, in a few steps from right above or below.
    var x = clamp(p.x, lo, hi);
    for (var i = 0; i < 2; i++) {
        let on_line = y + amplitude * sin(x * k - params.phase);
        let slope = amplitude * k * cos(x * k - params.phase);
        x = clamp(x + (p.x - x + slope * (p.y - on_line)) / (1.0 + slope * slope), lo, hi);
    }
    return distance(p, vec2<f32>(x, y + amplitude * sin(x * k - params.phase))) - half;
}

fn paint(f: Frag) -> vec4<f32> {
    let s = aw_box.scale;
    let size = vec2<f32>(aw_box.size);
    let half = params.stroke * s * 0.5;
    let y = size.y * 0.5;
    let amplitude = params.amplitude * s;
    let k = 6.2831853 / max(params.wavelength * s, 1.0);
    var track = piece(f.px, params.piece0_a, params.piece0_b, half, size.x, y);
    track = min(track, piece(f.px, params.piece1_a, params.piece1_b, half, size.x, y));
    track = min(track, piece(f.px, params.piece2_a, params.piece2_b, half, size.x, y));
    var color = params.track * aw_coverage(track, f.pixel);
    if (params.stop > 0.0) {
        let r = params.stop * s * 0.5;
        let d = distance(f.px, vec2<f32>(size.x - r, y)) - r;
        color = over(params.line * aw_coverage(d, f.pixel), color);
    }
    var line = bar(f.px, params.tail0, params.head0, half, size.x, y, amplitude, k);
    line = min(line, bar(f.px, params.tail1, params.head1, half, size.x, y, amplitude, k));
    return over(params.line * aw_coverage(line, f.pixel), color);
}
