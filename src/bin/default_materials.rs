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
    for (name, graph) in [
        ("concrete", concrete()),
        ("steel_plate", steel_plate()),
        ("tread_plate", tread_plate()),
        ("board_formed_concrete", board_formed_concrete()),
        ("plaster", plaster(Paint::OFF_WHITE)),
        ("plaster_red", plaster(Paint::PASTEL_RED)),
        ("plaster_blue", plaster(Paint::PASTEL_BLUE)),
        ("plaster_yellow", plaster(Paint::PASTEL_YELLOW)),
        ("plaster_green", plaster(Paint::PASTEL_GREEN)),
    ] {
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

/// How much of its full swing a room material's noise keeps: mottling, grime, streaks, grain and
/// noisy relief. Patterns — lozenges, seams, boards, holes — keep all of theirs.
const NOISE: f32 = 0.25;

/// `lo..hi` narrowed about its middle to [`NOISE`] of its width.
fn noisy(lo: f32, hi: f32) -> (f32, f32) {
    let middle = (lo + hi) / 2.0;
    let half = (hi - lo) / 2.0 * NOISE;
    (middle - half, middle + half)
}

/// `c`, [`NOISE`] of the way out from `middle` in lightness and chroma; its own hue.
fn toward(middle: Color, c: Color) -> Color {
    oklcha(
        middle.l + (c.l - middle.l) * NOISE,
        middle.chroma + (c.chroma - middle.chroma) * NOISE,
        c.hue.into_positive_degrees(),
        c.alpha,
    )
}

/// Two colors a noise swings between, narrowed about their middle by [`NOISE`].
fn noisy_pair(a: Color, b: Color) -> (Color, Color) {
    let middle = oklcha((a.l + b.l) / 2.0, (a.chroma + b.chroma) / 2.0, 0.0, 1.0);
    (toward(middle, a), toward(middle, b))
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

/// `value` through a ramp: the usual way a noise becomes a color or a gray.
fn mapped(g: &mut Graph, name: &str, value: LayerId, stops: &[(f32, Color)]) -> LayerId {
    let palette = g.add_layer(format!("{name} ramp"), ramp(stops)).unwrap();
    g.add_layer(name, LayerKind::Map(Map { value: Some(value), palette: Some(palette) })).unwrap()
}

fn coordinate(g: &mut Graph, axis: Axis) -> LayerId {
    let name = match axis {
        Axis::U => "u",
        Axis::V => "v",
        Axis::W => "w",
    };
    g.add_layer(name, LayerKind::Coordinate(Coordinate { axis })).unwrap()
}

/// Unsigned: `[0, 1]`. A triangle peaks at 1 where `input * frequency + phase` is a quarter cycle
/// in, so `phase: 0.25` puts its peaks where `input * frequency` is a whole number. A whole
/// `frequency` over `u` or `v` keeps the tile seamless.
fn wave(g: &mut Graph, name: &str, input: LayerId, shape: WaveShape, frequency: f32, phase: f32) -> LayerId {
    g.add_layer(
        name,
        LayerKind::Wave(Wave {
            input: ScalarInput::Layer(input),
            shape,
            frequency,
            phase,
            range: NoiseRange::Unsigned,
        }),
    )
    .unwrap()
}

/// `lo..hi` onto `0..1`, held at either end. With `lo > hi`, inverted.
fn remap(g: &mut Graph, name: &str, x: LayerId, lo: f32, hi: f32) -> LayerId {
    mapped(g, name, x, &[(lo, gray(0.0)), (hi, gray(1.0))])
}

/// `0..1` onto `lo..hi`.
fn scaled(g: &mut Graph, name: &str, x: LayerId, lo: f32, hi: f32) -> LayerId {
    mapped(g, name, x, &[(0.0, gray(lo)), (1.0, gray(hi))])
}

fn mix(g: &mut Graph, name: &str, a: LayerId, b: LayerId, mode: BlendMode) -> LayerId {
    g.add_layer(
        name,
        LayerKind::Mix(Mix { a: Some(a), b: Some(b), mode, factor: ScalarInput::Const(0.5), space: BlendSpace::Oklch }),
    )
    .unwrap()
}

/// `a` toward `b` by `by`'s L.
fn blend(g: &mut Graph, name: &str, a: LayerId, b: LayerId, by: LayerId) -> LayerId {
    g.add_layer(
        name,
        LayerKind::Mix(Mix {
            a: Some(a),
            b: Some(b),
            mode: BlendMode::Blend,
            factor: ScalarInput::Layer(by),
            space: BlendSpace::Oklch,
        }),
    )
    .unwrap()
}

/// Of two grays. Luma is monotonic in L for a gray.
fn pick(g: &mut Graph, name: &str, a: LayerId, b: LayerId, mode: MinMaxMode) -> LayerId {
    g.add_layer(name, LayerKind::MinMax(MinMax { a: Some(a), b: Some(b), mode, criterion: Criterion::Luma }))
        .unwrap()
}

fn solid(g: &mut Graph, name: &str, c: Color) -> LayerId {
    g.add_layer(name, LayerKind::Color(c)).unwrap()
}

fn normal_of(g: &mut Graph, height: LayerId, strength: f32) -> LayerId {
    g.add_layer("normal", LayerKind::HeightToNormal(HeightToNormal { source: Some(height), strength }))
        .unwrap()
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

/// Poured concrete: gray, a little warm, mottled, finely pitted, rough with damper-looking patches.
fn concrete() -> Graph {
    let mut g = empty();
    let mottle = g.add_layer("mottle", tiling_fbm(0, 4, 6)).unwrap();
    let color = mapped(&mut g, "color", mottle, &[
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
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Rolled steel: metal, cool gray, smoother than concrete, with faint waviness from the mill.
fn steel_plate() -> Graph {
    let mut g = empty();
    let sheen = g.add_layer("sheen", tiling_fbm(10, 2, 4)).unwrap();
    let color = mapped(&mut g, "color", sheen, &[
        (0.2, oklcha(0.62, 0.008, 240.0, 1.0)),
        (0.8, oklcha(0.72, 0.006, 240.0, 1.0)),
    ]);
    let roughness = mapped(&mut g, "roughness", sheen, &[(0.2, gray(0.30)), (0.8, gray(0.45))]);
    let waves = g.add_layer("waves", tiling_fbm(11, 4, 2)).unwrap();
    let normal = g
        .add_layer("waviness", LayerKind::HeightToNormal(HeightToNormal { source: Some(waves), strength: 0.01 }))
        .unwrap();
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(1.0),
        normal: Some(normal),
    })
}

/// Diamond tread plate: raised lozenges on a steel sheet, alternating between the two diagonals
/// so each one's neighbors across the grid lie the other way. Tops polished by boots, grime
/// between.
///
/// Built on the diagonals `p = u + v` and `q = u - v`, so every wave stays whole across the tile.
/// A lozenge sits wherever `p * LATTICE` and `q * LATTICE` are both whole. Its shape is a weighted
/// sum of the two triangle waves, `0.75 * across + 0.25 * along`, which is a rhombus three times
/// longer than wide with pointed tips; cutting it below its peak gives a flat top and sloped
/// sides. Weights rather than `3 *`, because a gray above 1 does not survive a ramp. Which
/// diagonal it lies along alternates with the parity of `p * LATTICE`, a square wave at half the
/// lattice's frequency.
fn tread_plate() -> Graph {
    const LATTICE: f32 = 16.0;
    let mut g = empty();
    let u = coordinate(&mut g, Axis::U);
    let v = coordinate(&mut g, Axis::V);
    let p = mix(&mut g, "p", u, v, BlendMode::Add);
    let q = mix(&mut g, "q", u, v, BlendMode::Subtract);
    let across_p = wave(&mut g, "across p", p, WaveShape::Triangle, LATTICE, 0.25);
    let across_q = wave(&mut g, "across q", q, WaveShape::Triangle, LATTICE, 0.25);
    let narrow_p = scaled(&mut g, "narrow p", across_p, 0.0, 0.75);
    let narrow_q = scaled(&mut g, "narrow q", across_q, 0.0, 0.75);
    let long_p = scaled(&mut g, "long p", across_p, 0.0, 0.25);
    let long_q = scaled(&mut g, "long q", across_q, 0.0, 0.25);

    // Peak 1. The edge at 0.79 makes the tips stop short of the next lozenge along (a sum of
    // 0.75 + 0.25 * along reaches it with `along` at 0.16), and the flat top starts at 0.9.
    let sum_along_p = mix(&mut g, "sum along p", narrow_q, long_p, BlendMode::Add);
    let along_p = remap(&mut g, "along p", sum_along_p, 0.79, 0.90);
    let sum_along_q = mix(&mut g, "sum along q", narrow_p, long_q, BlendMode::Add);
    let along_q = remap(&mut g, "along q", sum_along_q, 0.79, 0.90);

    let parity = wave(&mut g, "parity", p, WaveShape::Square, LATTICE / 2.0, 0.25);
    let other = remap(&mut g, "other parity", parity, 1.0, 0.0);
    let these = mix(&mut g, "these", along_p, parity, BlendMode::Multiply);
    let those = mix(&mut g, "those", along_q, other, BlendMode::Multiply);
    let height = pick(&mut g, "height", these, those, MinMaxMode::Max);

    let grime = g.add_layer("grime", tiling_fbm(20, 4, 5)).unwrap();
    let (grimy, clean) = noisy_pair(oklcha(0.46, 0.006, 70.0, 1.0), oklcha(0.60, 0.004, 230.0, 1.0));
    let sheet = mapped(&mut g, "sheet", grime, &[(0.25, grimy), (0.75, clean)]);
    let polished = solid(&mut g, "polished", oklcha(0.74, 0.004, 230.0, 1.0));
    let color = blend(&mut g, "color", sheet, polished, height);
    let (rough, smooth) = noisy(0.45, 0.70);
    let dull = scaled(&mut g, "dull", grime, smooth, rough);
    let shine = solid(&mut g, "shine", gray(0.28));
    let roughness = blend(&mut g, "roughness", dull, shine, height);
    let normal = normal_of(&mut g, height, 0.004);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(1.0),
        normal: Some(normal),
    })
}

/// Board-formed concrete: panels a meter high with shallow seams between them, the imprint of the
/// formwork's boards across each, a form-tie hole every meter, mottled, with faint streaks where
/// water has run down it.
///
/// On a wall `v` runs down, a tile is two meters square, so seams every half `v` are a meter apart.
fn board_formed_concrete() -> Graph {
    let mut g = empty();
    let u = coordinate(&mut g, Axis::U);
    let v = coordinate(&mut g, Axis::V);

    let rows = wave(&mut g, "rows", v, WaveShape::Triangle, 2.0, 0.25);
    let columns = wave(&mut g, "columns", u, WaveShape::Triangle, 1.0, 0.25);
    let row_seam = remap(&mut g, "row seam", rows, 0.96, 0.99);
    let column_seam = remap(&mut g, "column seam", columns, 0.98, 0.995);
    let seams = pick(&mut g, "seams", row_seam, column_seam, MinMaxMode::Max);
    // Half a cycle off the seams: a tie hole in the middle of each meter.
    // Halved before adding, because a gray above 1 does not survive a ramp.
    let tie_u = wave(&mut g, "tie u", u, WaveShape::Triangle, 2.0, 0.75);
    let tie_v = wave(&mut g, "tie v", v, WaveShape::Triangle, 2.0, 0.75);
    let half_u = scaled(&mut g, "half tie u", tie_u, 0.0, 0.5);
    let half_v = scaled(&mut g, "half tie v", tie_v, 0.0, 0.5);
    let tie_sum = mix(&mut g, "tie sum", half_u, half_v, BlendMode::Add);
    let ties = remap(&mut g, "ties", tie_sum, 0.975, 0.985);
    let recess = pick(&mut g, "recess", seams, ties, MinMaxMode::Max);

    // Twelve boards to a tile, about 17 cm each: a faint groove where two met, and a slightly
    // different tone from one board to the next.
    let boards = wave(&mut g, "boards", v, WaveShape::Triangle, 12.0, 0.25);
    let board_seam = remap(&mut g, "board seam", boards, 0.93, 0.99);
    let board_tone = wave(&mut g, "board tone", v, WaveShape::Square, 6.0, 0.0);
    let grain_noise = g.add_layer("grain noise", tiling_fbm(3, 8, 3)).unwrap();
    let grain = g
        .add_layer(
            "grain",
            LayerKind::Transform(Transform {
                source: Some(grain_noise),
                offset: [0.0; 3],
                rotate_uv: 0.0,
                // Squashed along `v`, so the wood's grain runs along the boards. Whole numbers,
                // so it still tiles.
                scale: [1.0, 6.0, 1.0],
                coord_mode: CoordMode::Passthrough,
                edge_mode: EdgeMode::Extend,
            }),
        )
        .unwrap();

    let mottle = g.add_layer("mottle", tiling_fbm(0, 4, 6)).unwrap();
    let middle = oklcha(0.61, 0.008, 75.0, 1.0);
    let base = mapped(&mut g, "base", mottle, &[
        (0.2, toward(middle, oklcha(0.52, 0.007, 75.0, 1.0))),
        (0.5, middle),
        (0.8, toward(middle, oklcha(0.69, 0.010, 75.0, 1.0))),
    ]);
    let streak_noise = g.add_layer("streak noise", tiling_fbm(2, 4, 4)).unwrap();
    let streaks = g
        .add_layer(
            "streaks",
            LayerKind::Transform(Transform {
                source: Some(streak_noise),
                offset: [0.0; 3],
                rotate_uv: 0.0,
                // Whole numbers, so the stretched noise still tiles.
                scale: [6.0, 1.0, 1.0],
                coord_mode: CoordMode::Passthrough,
                edge_mode: EdgeMode::Extend,
            }),
        )
        .unwrap();
    let staining = remap(&mut g, "staining", streaks, 0.6, 0.9);
    let faint = scaled(&mut g, "faint", staining, 0.0, 0.18 * NOISE);
    let stain = solid(&mut g, "stain", oklcha(0.42, 0.012, 70.0, 1.0));
    let alternate = scaled(&mut g, "alternate", board_tone, 0.0, 0.08);
    let toned = blend(&mut g, "toned", base, stain, alternate);
    let wood = scaled(&mut g, "wood", grain, 0.0, 0.12 * NOISE);
    let grained = blend(&mut g, "grained", toned, stain, wood);
    let lines = scaled(&mut g, "lines", board_seam, 0.0, 0.25);
    let lined = blend(&mut g, "lined", grained, stain, lines);
    let stained = blend(&mut g, "stained", lined, stain, faint);
    let shadow = solid(&mut g, "shadow", oklcha(0.34, 0.006, 75.0, 1.0));
    let deep = scaled(&mut g, "deep", recess, 0.0, 0.8);
    let color = blend(&mut g, "color", stained, shadow, deep);

    let (smooth, rough) = noisy(0.82, 0.95);
    let roughness = scaled(&mut g, "roughness", mottle, smooth, rough);
    let pits = g.add_layer("pits", tiling_fbm(1, 32, 3)).unwrap();
    let fine = scaled(&mut g, "fine", pits, 0.0, 0.04 * NOISE);
    let face = remap(&mut g, "face", recess, 0.9, 0.0);
    let imprint = scaled(&mut g, "imprint", board_seam, 0.0, 0.08);
    let boarded = mix(&mut g, "boarded", face, imprint, BlendMode::Subtract);
    let grain_relief = scaled(&mut g, "grain relief", grain, 0.0, 0.02 * NOISE);
    let textured = mix(&mut g, "textured", boarded, grain_relief, BlendMode::Add);
    let height = mix(&mut g, "height", textured, fine, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.002);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// What a plaster is painted, in Oklch: the shaded and bright ends its trowel swings between.
struct Paint {
    lightness: (f32, f32),
    chroma: (f32, f32),
    hue: f32,
}

impl Paint {
    const OFF_WHITE: Paint = Paint { lightness: (0.85, 0.90), chroma: (0.008, 0.010), hue: 85.0 };
    /// The spawn rooms' team colors: pale enough to read as painted plaster, not as a light.
    const PASTEL_RED: Paint = Paint { lightness: (0.78, 0.83), chroma: (0.088, 0.092), hue: 22.0 };
    const PASTEL_BLUE: Paint = Paint { lightness: (0.77, 0.82), chroma: (0.078, 0.082), hue: 245.0 };
    /// Toward amber and away from blue, as the teams' body colors are, so the four stay apart.
    const PASTEL_YELLOW: Paint = Paint { lightness: (0.84, 0.89), chroma: (0.088, 0.092), hue: 88.0 };
    const PASTEL_GREEN: Paint = Paint { lightness: (0.78, 0.83), chroma: (0.078, 0.082), hue: 145.0 };
}

/// Painted plaster: broad soft variation and a gentle undulation from the trowel, and a fine
/// orange-peel relief from the paint roller.
fn plaster(paint: Paint) -> Graph {
    let mut g = empty();
    let trowel = g.add_layer("trowel", tiling_fbm(30, 2, 5)).unwrap();
    let (shaded, bright) = noisy_pair(
        oklcha(paint.lightness.0, paint.chroma.0, paint.hue, 1.0),
        oklcha(paint.lightness.1, paint.chroma.1, paint.hue, 1.0),
    );
    let color = mapped(&mut g, "color", trowel, &[(0.25, shaded), (0.75, bright)]);
    let sheen = g.add_layer("sheen", tiling_fbm(32, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.80, 0.95);
    let roughness = scaled(&mut g, "roughness", sheen, smooth, rough);
    let peel = g.add_layer("peel", tiling_fbm(31, 64, 2)).unwrap();
    let fine = scaled(&mut g, "fine", peel, 0.0, 0.5);
    // The trowel's undulation, much broader and so much gentler in slope than the peel.
    let skim = g.add_layer("skim", tiling_fbm(33, 3, 3)).unwrap();
    let broad = scaled(&mut g, "broad", skim, 0.0, 0.5);
    let height = mix(&mut g, "height", fine, broad, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.003 * NOISE);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}
