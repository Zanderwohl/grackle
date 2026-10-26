//! Writes the default pack's materials, `assets/default/materials/*.tgraph`. Generated here rather
//! than saved from the material editor so their contents are reviewable as code. Run from the repo
//! root:
//!
//! ```text
//! cargo run --bin default_materials
//! ```
//!
//! A material repeats across a wall, so every noise in one is value noise with a lattice period
//! equal to its frequency: that is what makes the tile's edges meet.

use std::path::Path;

use grackle::material::{self, MATERIALS_DIR};
use texture_graph_core::color::oklcha;
use texture_graph_core::*;

fn main() {
    let dir = Path::new("assets/default").join(MATERIALS_DIR);
    for (name, graph) in [("concrete", concrete())] {
        let path = dir.join(format!("{name}.{}", material::FILE_EXTENSION));
        material::save(&path, &graph).unwrap_or_else(|e| panic!("{e}"));
        println!("wrote {}", path.display());
    }
}

/// Seamless fbm in `[0, 1]`: `frequency` cycles across the tile, each octave doubling it.
fn tiling_fbm(seed_offset: u32, frequency: u32, octaves: u32) -> LayerKind {
    LayerKind::Noise(Noise {
        dims: NoiseDims::D2,
        seed_offset,
        frequency: frequency as f32,
        range: NoiseRange::Unsigned,
        output: NoiseOutput::Grayscale,
        kernel: NoiseKernel::Value,
        period: [frequency, frequency, 0],
        fractal: Fractal { octaves, lacunarity: 2.0, gain: 0.5, mode: FractalMode::Standard, normalize: true },
    })
}

fn ramp(stops: &[(f32, Color)]) -> LayerKind {
    LayerKind::ColorRamp(ColorRamp {
        stops: stops.iter().map(|&(t, c)| ColorStop { t, color: ColorInput::Const(c) }).collect(),
        space: BlendSpace::Oklch,
    })
}

/// Poured concrete: grey, a little warm, mottled at two scales.
fn concrete() -> Graph {
    let mut g = Graph::new();
    let placeholder: Vec<LayerId> = g.layers.iter().map(|layer| layer.id).collect();

    let mottle = g.add_layer("mottle", tiling_fbm(0, 4, 6)).unwrap();
    let palette = g
        .add_layer(
            "concrete",
            ramp(&[
                (0.2, oklcha(0.50, 0.006, 75.0, 1.0)),
                (0.5, oklcha(0.60, 0.008, 75.0, 1.0)),
                (0.8, oklcha(0.70, 0.010, 75.0, 1.0)),
            ]),
        )
        .unwrap();
    let colour = g
        .add_layer("colour", LayerKind::Map(Map { value: Some(mottle), palette: Some(palette) }))
        .unwrap();
    g.set_output(Output {
        color: Some(colour),
        roughness: ScalarInput::Const(0.9),
        metallic: ScalarInput::Const(0.0),
        normal: None,
    })
    .unwrap();
    for id in placeholder {
        g.remove(id).unwrap();
    }

    g.add_canvas("main").unwrap();
    for (column, id) in [mottle, palette, colour].into_iter().enumerate() {
        let row = if id == palette { 1.0 } else { 0.0 };
        g.set_position("main", id, [column as f32 * 260.0, row * 150.0]).unwrap();
    }
    g.set_output_position("main", [3.0 * 260.0, 0.0]).unwrap();
    g
}
