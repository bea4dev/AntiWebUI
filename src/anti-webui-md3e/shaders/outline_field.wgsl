// The outline of a text field, with a gap at the top where the label sits. Lengths in the parameters
// are logical pixels.

fn paint(f: Frag) -> vec4<f32> {
    let s = aw_box.scale;
    let size = vec2<f32>(aw_box.size);
    let width = params.width * s;
    let half = width * 0.5;
    let rect = vec4<f32>(half, half, size.x - width, size.y - width);
    let r = max(params.radius * s - half, 0.0);
    let d = abs(aw_rounded_box_distance(f.px, rect, vec4<f32>(r))) - half;
    var coverage = aw_coverage(d, f.pixel);
    if (params.notch_end > params.notch_start && f.px.y < width + f.pixel) {
        let inside = min(f.px.x - params.notch_start * s, params.notch_end * s - f.px.x);
        coverage *= 1.0 - clamp(inside / f.pixel + 0.5, 0.0, 1.0);
    }
    return params.color * coverage;
}
