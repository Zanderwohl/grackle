// `game::scope_vignette`'s: clear at 0, black everywhere at 1.

import bevy_pbr::render::{forward_io::VertexOutput, mesh_functions::get_tag};

// Matches `game::scope_vignette::AMOUNT_STEPS`.
const AMOUNT_STEPS: f32 = 65535.0;

struct Vignette {
    aspect: f32,
    // Half screen heights.
    soft: f32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> vignette: Vignette;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let from_center = (in.uv - 0.5) * vec2<f32>(vignette.aspect, 1.0) * 2.0;
    let corner = length(vec2<f32>(vignette.aspect, 1.0));
    let amount = f32(get_tag(in.instance_index)) / AMOUNT_STEPS;
    let edge = mix(corner, -vignette.soft, amount);
    let dark = smoothstep(edge, edge + vignette.soft, length(from_center));
    return vec4<f32>(0.0, 0.0, 0.0, dark);
}
