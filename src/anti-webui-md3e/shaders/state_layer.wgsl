// A state layer and ripples in the color of the label. Each ripple is
// (x, y, how far it has grown from 0 to 1, how strong it is), with a negative x for the center.
fn paint(f: Frag) -> vec4<f32> {
    let size = vec2<f32>(aw_box.size);
    let start = max(size.x, size.y) * 0.3;
    let end = length(size) * 0.5 + 10.0 * aw_box.scale;
    var strength = params.state;
    for (var i = 0; i < 3; i++) {
        let ripple = params.ripples[i];
        if (ripple.w > 0.0) {
            let radius = mix(start, end, ripple.z);
            let center = select(ripple.xy, size * 0.5, ripple.x < 0.0);
            let inside = aw_coverage(distance(f.px, center) - radius, f.pixel);
            strength = 1.0 - (1.0 - strength) * (1.0 - ripple.w * inside);
        }
    }
    return params.color * strength;
}
