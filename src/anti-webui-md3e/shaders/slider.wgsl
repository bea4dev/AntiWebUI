// The track, stops, ticks and handles of a slider, cut around the handles like `Slider.kt`. Lengths
// in the parameters are logical pixels. A kind of 0 is a slider from the start, 1 a range and 2 a
// slider from the center. With an amplitude, what is covered of a slider from the start is a wavy
// line `stroke` thick instead.

fn over(top: vec4<f32>, below: vec4<f32>) -> vec4<f32> {
    return top + below * (1.0 - top.a);
}

// Coverage of a piece of the track from `a` to `b`, with its corners on each side. It narrows to
// nothing, corners included, rather than going away at once.
fn piece(p: vec2<f32>, a: f32, b: f32, y: f32, h: f32, r0: f32, r1: f32, pixel: f32) -> f32 {
    let w = b - a;
    if (w <= 0.0) {
        return 0.0;
    }
    let first = min(r0, min(w, h) * 0.5);
    let second = min(r1, min(w, h) * 0.5);
    let d = aw_rounded_box_distance(p, vec4<f32>(a, y, w, h), vec4<f32>(first, second, second, first));
    return aw_coverage(d, pixel);
}

fn spot(p: vec2<f32>, x: f32, y: f32, r: f32, pixel: f32) -> f32 {
    return aw_coverage(distance(p, vec2<f32>(x, y)) - r, pixel);
}

// Coverage of a wavy line from the start to `b`, around `mid`. It flattens when it is shorter than
// a wavelength, so that it doesn't start with half a wave.
fn wavy(p: vec2<f32>, b: f32, mid: f32, s: f32, pixel: f32) -> f32 {
    let half = params.stroke * s * 0.5;
    let head = b - half;
    if (head < half) {
        return 0.0;
    }
    let length = params.wavelength * s;
    let amplitude = params.amplitude * s * clamp((head - half) / length, 0.0, 1.0);
    let k = 6.2831853 / length;
    // The closest point on the line, in a few steps from right above or below.
    var x = clamp(p.x, half, head);
    for (var i = 0; i < 3; i++) {
        let y = mid + amplitude * sin(x * k - params.phase);
        let slope = amplitude * k * cos(x * k - params.phase);
        x = clamp(x + (p.x - x + slope * (p.y - y)) / (1.0 + slope * slope), half, head);
    }
    let d = distance(p, vec2<f32>(x, mid + amplitude * sin(x * k - params.phase))) - half;
    return aw_coverage(d, pixel);
}

// Where a fraction is along a track of length `w`. With ticks the values between the ends keep
// clear of the corners, and the first and last steps make up for that.
fn along(t: f32, w: f32, corner: f32, n: f32) -> f32 {
    if (n <= 1.0) {
        return w * t;
    }
    let start = 1.0 - min(t * n, 1.0);
    let end = max(t * n - (n - 1.0), 0.0);
    return corner + (w - 2.0 * corner) * t + corner * (end - start);
}

fn paint(f: Frag) -> vec4<f32> {
    let s = aw_box.scale;
    let box_size = vec2<f32>(aw_box.size);
    // Along the track and across it, whichever way it goes.
    var p = f.px;
    var size = box_size;
    if (params.vertical > 0.5) {
        p = vec2<f32>(box_size.y - f.px.y, f.px.x);
        size = box_size.yx;
    }
    let inset = 2.0 * s;
    let w = size.x - inset * 2.0;
    p.x -= inset;
    let h = params.track * s;
    let y = (size.y - h) * 0.5;
    let mid = size.y * 0.5;
    let corner = params.corner * s;
    let inside = 2.0 * s;
    let gap = params.gap * s;
    let cut = 2.0 * s + gap;
    let n = params.steps + 1.0;
    let start = along(params.start, w, corner, n);
    let end = along(params.end, w, corner, n);
    let pixel = f.pixel;

    var covered = 0.0;
    var first = 0.0;
    var last = 0.0;
    var lower = 0.0;
    var upper = end;
    if (params.kind < 0.5 && params.amplitude > 0.0) {
        covered = wavy(p, end - cut, mid, s, pixel);
        last = piece(p, end + cut, w, y, h, inside, corner, pixel);
    } else if (params.kind < 0.5) {
        covered = piece(p, 0.0, end - cut, y, h, corner, inside, pixel);
        last = piece(p, end + cut, w, y, h, inside, corner, pixel);
    } else if (params.kind < 1.5) {
        first = piece(p, 0.0, start - cut, y, h, corner, inside, pixel);
        covered = piece(p, start + cut, end - cut, y, h, inside, inside, pixel);
        last = piece(p, end + cut, w, y, h, inside, corner, pixel);
        lower = start;
    } else {
        let center = w * 0.5;
        first = piece(p, 0.0, min(center - gap, end - cut), y, h, corner, inside, pixel);
        last = piece(p, max(center + gap, end + cut), w, y, h, inside, corner, pixel);
        if (end > center) {
            covered = piece(p, center, end - cut, y, h, inside, inside, pixel);
            lower = center;
        } else {
            covered = piece(p, end + cut, center, y, h, inside, inside, pixel);
            lower = end;
            upper = center;
        }
    }
    var color = params.empty * max(first, last);
    color = over(params.filled * covered, color);

    let dot = params.stop * s * 0.5;
    // Ticks, the closest of them for each pixel.
    if (params.steps > 0.0) {
        let tick = clamp(round((p.x - corner) / (w - 2.0 * corner) * n), 0.0, n);
        let x = corner + (w - 2.0 * corner) * tick / n;
        let is_stop = params.stops > 0.5
            && (tick == n || (tick == 0.0 && params.kind > 0.5));
        if (!is_stop && abs(x - end) > cut && (params.kind != 1.0 || abs(x - start) > cut)) {
            let on = x >= lower && x <= upper;
            let tick_color = select(params.tick_off, params.tick_on, on);
            color = over(tick_color * spot(p, x, mid, dot, pixel), color);
        }
    }
    // Stops go along with the pieces of track they are on.
    if (params.stops > 0.5) {
        color = over(params.filled * spot(p, w - corner, mid, dot, pixel) * last, color);
        if (params.kind > 0.5) {
            color = over(params.filled * spot(p, corner, mid, dot, pixel) * first, color);
        }
    }

    let tall = params.handle_height * s;
    let handle_y = (size.y - tall) * 0.5;
    let width = params.handle_end * s;
    let d = aw_rounded_box_distance(
        p,
        vec4<f32>(end - width * 0.5, handle_y, width, tall),
        vec4<f32>(width * 0.5),
    );
    color = over(params.thumb * aw_coverage(d, pixel), color);
    if (params.kind > 0.5 && params.kind < 1.5) {
        let other = params.handle_start * s;
        let e = aw_rounded_box_distance(
            p,
            vec4<f32>(start - other * 0.5, handle_y, other, tall),
            vec4<f32>(other * 0.5),
        );
        color = over(params.thumb * aw_coverage(e, pixel), color);
    }
    return color;
}
