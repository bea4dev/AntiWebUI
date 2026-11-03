// A progress bar: a wavy line up to the progress and a straight track after it. Lengths in
// the parameters are logical pixels.

fn over(top: vec4<f32>, below: vec4<f32>) -> vec4<f32> {
    return top + below * (1.0 - top.a);
}

fn wave(x: f32, height: f32, amplitude: f32, k: f32) -> f32 {
    return height * 0.5 + amplitude * sin(x * k - params.phase);
}

fn paint(f: Frag) -> vec4<f32> {
    let s = aw_box.scale;
    let size = vec2<f32>(aw_box.size);
    let half = params.stroke * s * 0.5;
    let amplitude = params.amplitude * s * params.wave;
    let k = 6.2831853 / (params.wavelength * s);
    let head = mix(half, size.x - half, params.progress);

    var color = vec4<f32>(0.0);

    // The track starts one gap away from the end of the line, ends included.
    let track = head + 2.0 * half + params.gap * s;
    if (track < size.x - half) {
        let a = vec2<f32>(track, size.y * 0.5);
        let b = vec2<f32>(size.x - half, size.y * 0.5);
        let d = distance(f.px, vec2<f32>(clamp(f.px.x, a.x, b.x), a.y)) - half;
        color = params.track * aw_coverage(d, f.pixel);
        let dot = distance(f.px, vec2<f32>(size.x - params.stop * s * 0.5, a.y)) - params.stop * s * 0.5;
        color = over(params.line * aw_coverage(dot, f.pixel), color);
    }

    if (params.progress > 0.0) {
        // The closest point on the line, in a few steps from right above or below.
        var x = clamp(f.px.x, half, head);
        for (var i = 0; i < 2; i++) {
            let y = wave(x, size.y, amplitude, k);
            let slope = amplitude * k * cos(x * k - params.phase);
            x = clamp(x + slope * (f.px.y - y) / (1.0 + slope * slope), half, head);
        }
        let d = distance(f.px, vec2<f32>(x, wave(x, size.y, amplitude, k))) - half;
        color = over(params.line * aw_coverage(d, f.pixel), color);
    }
    return color;
}
