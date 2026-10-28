// A wall drawn as the sky behind it: the color is a function of the direction from the eye
// alone, so it sits at infinite distance and turns with the view. See `common::sky`, whose
// `color_at` this mirrors.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};

struct SkyGradient {
    // Oklab in xyz; w is the stop's elevation, ascending from -1 to 1.
    stops: array<vec4<f32>, 5>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sky: SkyGradient;

// Björn Ottosson's, as bevy_color's `Oklaba` uses.
fn oklab_to_linear_srgb(c: vec3<f32>) -> vec3<f32> {
    let l_ = c.x + 0.3963377774 * c.y + 0.2158037573 * c.z;
    let m_ = c.x - 0.1055613458 * c.y - 0.0638541728 * c.z;
    let s_ = c.x - 0.0894841775 * c.y - 1.2914855480 * c.z;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    return vec3<f32>(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    );
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // An orthographic view looks one way everywhere; from the eye to the fragment would fan out.
    var direction = normalize(in.world_position.xyz - view.world_position);
    if view.clip_from_view[3].w == 1.0 {
        direction = normalize(-view.world_from_view[2].xyz);
    }
    let elevation = direction.y;

    var color = sky.stops[0].xyz;
    for (var i = 1u; i < 5u; i++) {
        let lo = sky.stops[i - 1u];
        let hi = sky.stops[i];
        if elevation > lo.w {
            color = mix(lo.xyz, hi.xyz, saturate((elevation - lo.w) / max(hi.w - lo.w, 1e-5)));
        }
    }
    return vec4<f32>(max(oklab_to_linear_srgb(color), vec3<f32>(0.0)), 1.0);
}
