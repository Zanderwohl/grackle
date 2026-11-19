// `game::muzzle_flash`'s.
//
// `uv.x` is how far along its cone a fragment is, 0 at the muzzle ring and 1
// at the crown's edge, so a line of one `uv.x` is the crown scaled down. The
// front and the eraser are two such lines moving out. `uv.y` is the layer.

import bevy_pbr::render::{forward_io::VertexOutput, mesh_functions::get_tag};

// Matches `game::muzzle_flash::PROGRESS_STEPS`.
const PROGRESS_STEPS: f32 = 65535.0;

// Matches `game::muzzle_flash::FULL`: the share of the flash the points take
// to reach the crown. Nothing is erased before it.
const FULL: f32 = 0.3;

// Where the eraser starts. Every layer erases on one schedule, so each
// eraser's speed is in proportion to its layer's length.
const ERASE_START: f32 = 0.34;

// Linear, from #d8241f, #f2701d, #ffc629 and #fff3c4.
fn color(layer: u32) -> vec3<f32> {
    if layer == 0u {
        return vec3<f32>(0.6867, 0.0176, 0.0137);
    }
    if layer == 1u {
        return vec3<f32>(0.8879, 0.1620, 0.0123);
    }
    if layer == 2u {
        return vec3<f32>(1.0000, 0.5647, 0.0222);
    }
    return vec3<f32>(1.0000, 0.8963, 0.5520);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let progress = f32(get_tag(in.instance_index) & 0xffffu) / PROGRESS_STEPS;
    let layer = u32(round(in.uv.y));
    let along = in.uv.x;

    let grown = clamp(progress / FULL, 0.0, 1.0);
    let front = 1.0 - (1.0 - grown) * (1.0 - grown);

    let erasing = clamp((progress - ERASE_START) / (1.0 - ERASE_START), 0.0, 1.0);
    // Past one at the end, so the crown's own edge goes too.
    let erased = select(-1.0, sqrt(erasing) * 1.02, erasing > 0.0);

    if along > front || along <= erased {
        discard;
    }
    return vec4<f32>(color(layer), 1.0);
}
