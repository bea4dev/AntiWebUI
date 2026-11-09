// A circular progress indicator: an arc, flat or wavy, and an arc of track. Angles are radians,
// clockwise from 3 o'clock; lengths are logical pixels.

const TAU: f32 = 6.2831853;

fn over(top: vec4<f32>, below: vec4<f32>) -> vec4<f32> {
    return top + below * (1.0 - top.a);
}

fn radius_at(angle: f32, r: f32, a: f32) -> f32 {
    return r + a * cos(params.waves * angle - params.phase);
}

fn on_curve(angle: f32, r: f32, a: f32) -> vec2<f32> {
    return radius_at(angle, r, a) * vec2<f32>(cos(angle), sin(angle));
}

// How far `p` is from an arc with round ends, whose radius may wave.
fn arc(p: vec2<f32>, start: f32, sweep: f32, r: f32, a: f32, half: f32) -> f32 {
    if (sweep <= 0.0) {
        return 1000000.0;
    }
    if (sweep >= TAU - 0.0001 && a == 0.0) {
        return abs(length(p) - r) - half;
    }
    let angle = atan2(p.y, p.x);
    let along = fract((angle - start) / TAU) * TAU;
    if (along <= sweep) {
        // The closest point on the arc, in a few steps from the one at the same angle.
        var t = angle;
        for (var i = 0; i < 3; i++) {
            let dir = vec2<f32>(cos(t), sin(t));
            let at = radius_at(t, r, a);
            let tangent = -a * params.waves * sin(params.waves * t - params.phase) * dir
                + at * vec2<f32>(-dir.y, dir.x);
            t += dot(p - at * dir, tangent) / dot(tangent, tangent);
        }
        return distance(p, on_curve(t, r, a)) - half;
    }
    let first = distance(p, on_curve(start, r, a));
    let last = distance(p, on_curve(start + sweep, r, a));
    return min(first, last) - half;
}

fn paint(f: Frag) -> vec4<f32> {
    let s = aw_box.scale;
    let size = vec2<f32>(aw_box.size);
    let half = params.stroke * s * 0.5;
    let p = f.px - size * 0.5;
    let a = params.amplitude * s;
    let r = min(size.x, size.y) * 0.5 - half - a;
    let track = arc(p, params.track_start, params.track_sweep, r, 0.0, half);
    let color = params.track * aw_coverage(track, f.pixel);
    let line = arc(p, params.start, params.sweep, r, a, half);
    return over(params.line * aw_coverage(line, f.pixel), color);
}
