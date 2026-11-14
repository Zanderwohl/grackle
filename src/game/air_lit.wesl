// A body's material: the standard one, with the ambient term scaled by the light in the air where
// the body stands, which `game::sunlit` packs into the mesh's `MeshTag`. See
// `common::air_light::pack` for the encoding. Forward only, as every camera here is.

import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_functions::get_tag,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
};

// RGBE: three 8-bit mantissas and an exponent biased by 128, less 8 for the mantissa's bits. A
// mesh with no tag reads 0, which leaves the ambient alone.
fn ambient_scale(tag: u32) -> vec3<f32> {
    if tag == 0u {
        return vec3<f32>(1.0);
    }
    let exponent = f32((tag >> 24u) & 0xffu) - 136.0;
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
    // Bevy's ambient term is scaled by the diffuse occlusion, so this replaces the global ambient
    // with this body's without a second copy of the lighting.
    pbr_input.diffuse_occlusion *= ambient_scale(get_tag(in.instance_index));

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
