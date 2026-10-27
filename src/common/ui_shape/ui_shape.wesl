// A closed polygon drawn by signed distance: drop shadow, fill with a rim
// light and shadow, then the stroke. See `ui_shape/mod.rs`.

import bevy_ui_render::ui_vertex_output::UiVertexOutput;

struct Shape {
    fill: vec4<f32>,
    highlight: vec4<f32>,
    shadow: vec4<f32>,
    stroke: vec4<f32>,
    drop: vec4<f32>,
    // Box width, box height, stroke width, rim.
    frame: vec4<f32>,
    // Drop offset x, y, point count, unused.
    extra: vec4<f32>,
    points: array<vec4<f32>, 64>,
};

@group(1) @binding(0) var<uniform> shape: Shape;

fn point(i: u32) -> vec2<f32> {
    let pair = shape.points[i / 2u];
    return select(pair.xy, pair.zw, (i & 1u) == 1u);
}

// Inigo Quilez's exact polygon distance; negative inside, either winding.
fn distance_to_shape(p: vec2<f32>) -> f32 {
    let n = u32(shape.extra.z);
    if (n < 3u) {
        return 1e6;
    }
    var d = dot(p - point(0u), p - point(0u));
    var s = 1.0;
    var j = n - 1u;
    for (var i = 0u; i < n; i++) {
        let vi = point(i);
        let vj = point(j);
        let e = vj - vi;
        let w = p - vi;
        let b = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);
        d = min(d, dot(b, b));
        let c = vec3<bool>((p.y >= vi.y), (p.y < vj.y), (e.x * w.y > e.y * w.x));
        if (all(c) || all(!c)) {
            s = -s;
        }
        j = i;
    }
    return s * sqrt(d);
}

// One pixel of antialiasing, `px` being shape units per physical pixel.
fn coverage(d: f32, px: f32) -> f32 {
    return clamp(0.5 - d / px, 0.0, 1.0);
}

// Straight alpha, which is what the UI pipeline blends.
fn over(top: vec4<f32>, under: vec4<f32>) -> vec4<f32> {
    let a = top.a + under.a * (1.0 - top.a);
    if (a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let rgb = (top.rgb * top.a + under.rgb * under.a * (1.0 - top.a)) / a;
    return vec4<f32>(rgb, a);
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let size = shape.frame.xy;
    let p = in.uv * size;
    let px = size.x / max(in.size.x, 1.0);
    let half_stroke = shape.frame.z * 0.5;
    let rim = shape.frame.w;
    let d = distance_to_shape(p);

    var out = shape.drop;
    out.a *= coverage(distance_to_shape(p - shape.extra.xy) - half_stroke, px);

    // The fill runs under the stroke, so the two edges do not leave a seam.
    var body = shape.fill;
    if (rim > 0.0) {
        // Inside the stroke but outside the same region slid along the light:
        // a band on the edges facing it.
        let o = vec2<f32>(rim * 0.70710678);
        let lit = 1.0 - coverage(distance_to_shape(p - o) + half_stroke, px);
        let unlit = 1.0 - coverage(distance_to_shape(p + o) + half_stroke, px);
        body = mix(body, shape.highlight, lit * shape.highlight.a);
        body = mix(body, shape.shadow, unlit * shape.shadow.a);
        body.a = shape.fill.a;
    }
    out = over(vec4<f32>(body.rgb, body.a * coverage(d, px)), out);

    if (half_stroke > 0.0) {
        let stroke = shape.stroke;
        out = over(vec4<f32>(stroke.rgb, stroke.a * coverage(abs(d) - half_stroke, px)), out);
    }
    return out;
}
