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

use std::collections::HashMap;
use std::path::Path;

use grackle::material::{self, MATERIALS_DIR};
use texture_graph_core::color::oklcha;
use texture_graph_core::*;

fn main() {
    let dir = Path::new("assets/default").join(MATERIALS_DIR);
    for (name, graph) in [("concrete", concrete()), ("steel_plate", steel_plate())] {
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

fn gray(l: f32) -> Color {
    oklcha(l, 0.0, 0.0, 1.0)
}

/// A graph with the placeholder layer `Graph::new` starts with already gone.
fn empty() -> Graph {
    let mut g = Graph::new();
    let placeholder: Vec<LayerId> = g.layers.iter().map(|layer| layer.id).collect();
    g.set_output(Output { color: None, ..g.output.clone() }).unwrap();
    for id in placeholder {
        g.remove(id).unwrap();
    }
    g
}

/// `value` through a ramp: the usual way a noise becomes a colour or a gray.
fn mapped(g: &mut Graph, name: &str, value: LayerId, stops: &[(f32, Color)]) -> LayerId {
    let palette = g.add_layer(format!("{name} ramp"), ramp(stops)).unwrap();
    g.add_layer(name, LayerKind::Map(Map { value: Some(value), palette: Some(palette) })).unwrap()
}

/// Sets the output and lays every layer out left to right by depth.
fn finish(mut g: Graph, output: Output) -> Graph {
    g.set_output(output).unwrap();
    let mut depth: HashMap<LayerId, usize> = HashMap::new();
    // `layers` is id-ascending and inputs always predate their readers.
    for layer in &g.layers {
        let d = layer.kind.inputs().iter().map(|input| depth[input] + 1).max().unwrap_or(0);
        depth.insert(layer.id, d);
    }
    let mut rows: HashMap<usize, usize> = HashMap::new();
    g.add_canvas("main").unwrap();
    let ids: Vec<LayerId> = g.layers.iter().map(|layer| layer.id).collect();
    for id in ids {
        let column = depth[&id];
        let row = rows.entry(column).or_default();
        g.set_position("main", id, [column as f32 * 260.0, *row as f32 * 150.0]).unwrap();
        *row += 1;
    }
    let last = depth.values().max().copied().unwrap_or(0) + 1;
    g.set_output_position("main", [last as f32 * 260.0, 0.0]).unwrap();
    g
}

/// Poured concrete: grey, a little warm, mottled, finely pitted, rough with damper-looking patches.
fn concrete() -> Graph {
    let mut g = empty();
    let mottle = g.add_layer("mottle", tiling_fbm(0, 4, 6)).unwrap();
    let colour = mapped(&mut g, "colour", mottle, &[
        (0.2, oklcha(0.50, 0.006, 75.0, 1.0)),
        (0.5, oklcha(0.60, 0.008, 75.0, 1.0)),
        (0.8, oklcha(0.70, 0.010, 75.0, 1.0)),
    ]);
    let roughness = mapped(&mut g, "roughness", mottle, &[(0.2, gray(0.72)), (0.8, gray(0.95))]);
    let pits = g.add_layer("pits", tiling_fbm(1, 32, 3)).unwrap();
    let normal = g
        .add_layer("relief", LayerKind::HeightToNormal(HeightToNormal { source: Some(pits), strength: 0.012 }))
        .unwrap();
    finish(g, Output {
        color: Some(colour),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Rolled steel: metal, cool grey, smoother than concrete, with faint waviness from the mill.
fn steel_plate() -> Graph {
    let mut g = empty();
    let sheen = g.add_layer("sheen", tiling_fbm(10, 2, 4)).unwrap();
    let colour = mapped(&mut g, "colour", sheen, &[
        (0.2, oklcha(0.62, 0.008, 240.0, 1.0)),
        (0.8, oklcha(0.72, 0.006, 240.0, 1.0)),
    ]);
    let roughness = mapped(&mut g, "roughness", sheen, &[(0.2, gray(0.30)), (0.8, gray(0.45))]);
    let waves = g.add_layer("waves", tiling_fbm(11, 4, 2)).unwrap();
    let normal = g
        .add_layer("waviness", LayerKind::HeightToNormal(HeightToNormal { source: Some(waves), strength: 0.01 }))
        .unwrap();
    finish(g, Output {
        color: Some(colour),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(1.0),
        normal: Some(normal),
    })
}
