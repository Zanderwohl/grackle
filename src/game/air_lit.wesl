// A body's material: the standard one, lit by where the body stands, which `game::sunlit` packs
// into the mesh's `MeshTag` (`common::air_light::pack`): the ambient term scaled by the light in
// the air, and the sun added when the body is in it. The sun is added here rather than by a
// `DirectionalLight` because Bevy applies a directional light to every mesh a camera draws,
// walls included, whatever their render layers. Forward only, as every camera here is.

import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_functions::get_tag,
    mesh_view_bindings::view,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
};

const PI: f32 = 3.141592653589793;

struct Sun {
    // Unit, toward the sun; `w` is its illuminance.
    toward: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> sun: Sun;

// Three 8-bit mantissas and a 7-bit exponent biased by 64, less 8 for the mantissa's bits. A mesh
// with no tag reads 0, which leaves the ambient alone.
fn ambient_scale(tag: u32) -> vec3<f32> {
    if tag == 0u {
        return vec3<f32>(1.0);
    }
    let exponent = f32((tag >> 24u) & 0x7fu) - 72.0;
    let mantissa = vec3<f32>(f32(tag & 0xffu), f32((tag >> 8u) & 0xffu), f32((tag >> 16u) & 0xffu));
    return mantissa * exp2(exponent);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    );
    let tag = get_tag(in.instance_index);
    // Bevy's ambient term is scaled by the diffuse occlusion, so this replaces the global ambient
    // with this body's without a second copy of the lighting.
    pbr_input.diffuse_occlusion *= ambient_scale(tag);

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
        if (tag >> 31u) == 1u {
            // Lambertian, as Bevy's own diffuse is: albedo over π times illuminance.
            let diffuse = pbr_input.material.base_color.rgb * (1.0 - pbr_input.material.metallic);
            let n_dot_l = max(dot(pbr_input.N, sun.toward.xyz), 0.0);
            out.color += vec4<f32>(diffuse / PI * sun.toward.w * n_dot_l * view.exposure, 0.0);
        }
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
