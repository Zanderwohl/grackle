// The forward camera's view through a convex lens: the middle magnified, the rim darkened and
// split slightly by color. The numbers are `game::forward_camera`'s.

import bevy_pbr::render::forward_io::VertexOutput;

struct Lens {
    // Linear.
    tint: vec4<f32>,
    bulge: f32,
    vignette: f32,
    fringe: f32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> lens: Lens;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var view_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var view_sampler: sampler;

fn sample_at(from_center: vec2<f32>, scale: f32) -> vec4<f32> {
    return textureSample(view_texture, view_sampler, 0.5 + from_center * scale);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let from_center = in.uv - 0.5;
    // 0 at the center, 1 at the rim of the circle the UVs span; clamped, or the square's corners
    // would scale past 1 and read outside the view.
    let r2 = min(dot(from_center, from_center) * 4.0, 1.0);
    let scale = mix(1.0 - lens.bulge, 1.0, r2);
    let spread = lens.fringe * r2;
    let color = vec3<f32>(
        sample_at(from_center, scale * (1.0 - spread)).r,
        sample_at(from_center, scale).g,
        sample_at(from_center, scale * (1.0 + spread)).b,
    );
    let dark = 1.0 - lens.vignette * r2 * r2;
    return vec4<f32>(color * lens.tint.rgb * dark, 1.0);
}
