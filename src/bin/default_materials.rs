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
        ("red_brick", red_brick()),
        ("painted_brick_white", painted_brick(Coat::WHITE)),
        ("painted_brick_gray", painted_brick(Coat::GRAY)),
        ("painted_brick_orange", painted_brick(Coat::ORANGE)),
        ("painted_brick_red", painted_brick(Coat::RED)),
        ("painted_brick_blue", painted_brick(Coat::BLUE)),
        ("painted_brick_yellow", painted_brick(Coat::YELLOW)),
        ("painted_brick_green", painted_brick(Coat::GREEN)),
        ("corrugated_steel", corrugated_steel()),
        ("wooden_fence", wooden_fence()),
        ("pine_brown", pine(Pine::BROWN, 300)),
        ("pine_honey", pine(Pine::HONEY, 310)),
        ("pine_pale", pine(Pine::PALE, 320)),
        ("pine_silvering", pine(Pine::SILVERING, 330)),
        ("pine_gray", pine(Pine::GRAY, 340)),
        ("weathered_wood", weathered_wood()),
        ("hardwood_floor", hardwood(None)),
        ("hardwood_floor_worn", hardwood(Some(Wear { seed: 155 }))),
        ("wood_grain_laminate", laminate()),
        ("linoleum_tiles", linoleum()),
        ("linoleum_gray", checkered_linoleum(6, Coat::GRAY_CHECKER)),
        ("linoleum_orange", checkered_linoleum(6, Coat::ORANGE.checker())),
        ("linoleum_red", checkered_linoleum(6, Coat::RED.checker())),
        ("linoleum_blue", checkered_linoleum(6, Coat::BLUE.checker())),
        ("linoleum_yellow", checkered_linoleum(6, Coat::YELLOW.checker())),
        ("linoleum_green", checkered_linoleum(6, Coat::GREEN.checker())),
        ("linoleum_large_gray", checkered_linoleum(4, Coat::GRAY_CHECKER)),
        ("linoleum_large_orange", checkered_linoleum(4, Coat::ORANGE.checker())),
        ("linoleum_large_red", checkered_linoleum(4, Coat::RED.checker())),
        ("linoleum_large_blue", checkered_linoleum(4, Coat::BLUE.checker())),
        ("linoleum_large_yellow", checkered_linoleum(4, Coat::YELLOW.checker())),
        ("linoleum_large_green", checkered_linoleum(4, Coat::GREEN.checker())),
        ("osb", osb()),
        ("dirt_path", dirt_path()),
        ("desert_sand", desert_sand(false)),
        ("desert_stripes", desert_sand(true)),
        ("redwall_sandstone", redwall_sandstone(Face::Wall)),
        ("redwall_sandstone_floor", redwall_sandstone(Face::Floor)),
        ("bright_angel_sandstone", bright_angel_sandstone(Face::Wall)),
        ("bright_angel_sandstone_floor", bright_angel_sandstone(Face::Floor)),
        ("muav_limestone", muav_limestone(Face::Wall)),
        ("muav_limestone_floor", muav_limestone(Face::Floor)),
        ("tiles_white", tiles(Coat::GLAZE_WHITE)),
        ("tiles_orange", tiles(Coat::ORANGE)),
        ("tiles_red", tiles(Coat::RED)),
        ("tiles_blue", tiles(Coat::BLUE)),
        ("tiles_yellow", tiles(Coat::YELLOW)),
        ("tiles_green", tiles(Coat::GREEN)),
        ("stripes_orange", stripes(StripePaint::ORANGE)),
        ("stripes_yellow", stripes(StripePaint::YELLOW)),
        ("stripes_green", stripes(StripePaint::GREEN)),
        ("orange", grid(GridPaint::ORANGE)),
        ("grid_gray", grid(GridPaint::GRAY)),
        ("grid_light", grid(GridPaint::LIGHT)),
        ("grid_red", grid(GridPaint::RED)),
        ("grid_blue", grid(GridPaint::BLUE)),
        ("grid_yellow", grid(GridPaint::YELLOW)),
        ("grid_green", grid(GridPaint::GREEN)),
    ] {
        let path = dir.join(format!("{name}.{}", material::FILE_EXTENSION));
        material::save(&path, &graph).unwrap_or_else(|e| panic!("{e}"));
        println!("wrote {}", path.display());
    }
}

/// `add_layer` that numbers a name already taken, so a helper called twice in one graph — a
/// second layer of strands — needs no name of its own.
trait AddUnique {
    fn add_unique(&mut self, name: impl Into<String>, kind: LayerKind) -> Result<LayerId, GraphError>;
}

impl AddUnique for Graph {
    fn add_unique(&mut self, name: impl Into<String>, kind: LayerKind) -> Result<LayerId, GraphError> {
        let name = name.into();
        let taken = |n: &str| self.layers.iter().any(|layer| layer.name == n);
        let mut unique = name.clone();
        let mut count = 1;
        while taken(&unique) {
            count += 1;
            unique = format!("{name} {count}");
        }
        self.add_layer(unique, kind)
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
    let palette = g.add_unique(format!("{name} ramp"), ramp(stops)).unwrap();
    g.add_unique(name, LayerKind::Map(Map { value: Some(value), palette: Some(palette) })).unwrap()
}

fn coordinate(g: &mut Graph, axis: Axis) -> LayerId {
    let name = match axis {
        Axis::U => "u",
        Axis::V => "v",
        Axis::W => "w",
    };
    g.add_unique(name, LayerKind::Coordinate(Coordinate { axis })).unwrap()
}

/// Unsigned: `[0, 1]`. A triangle peaks at 1 where `input * frequency + phase` is a quarter cycle
/// in, so `phase: 0.25` puts its peaks where `input * frequency` is a whole number. A whole
/// `frequency` over `u` or `v` keeps the tile seamless.
fn wave(g: &mut Graph, name: &str, input: LayerId, shape: WaveShape, frequency: f32, phase: f32) -> LayerId {
    g.add_unique(
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
    g.add_unique(
        name,
        LayerKind::Mix(Mix { a: Some(a), b: Some(b), mode, factor: ScalarInput::Const(0.5), space: BlendSpace::Oklch }),
    )
    .unwrap()
}

/// `a` toward `b` by `by`'s L.
fn blend(g: &mut Graph, name: &str, a: LayerId, b: LayerId, by: LayerId) -> LayerId {
    g.add_unique(
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

/// [`blend`] in linear light, for two colors far apart in hue, which an Oklch blend would carry
/// round the hue circle through colors neither of them is.
fn blend_linear(g: &mut Graph, name: &str, a: LayerId, b: LayerId, by: LayerId) -> LayerId {
    g.add_unique(
        name,
        LayerKind::Mix(Mix {
            a: Some(a),
            b: Some(b),
            mode: BlendMode::Blend,
            factor: ScalarInput::Layer(by),
            space: BlendSpace::LinearSrgb,
        }),
    )
    .unwrap()
}

/// Of two grays. Luma is monotonic in L for a gray.
fn pick(g: &mut Graph, name: &str, a: LayerId, b: LayerId, mode: MinMaxMode) -> LayerId {
    g.add_unique(name, LayerKind::MinMax(MinMax { a: Some(a), b: Some(b), mode, criterion: Criterion::Luma }))
        .unwrap()
}

fn solid(g: &mut Graph, name: &str, c: Color) -> LayerId {
    g.add_unique(name, LayerKind::Color(c)).unwrap()
}

fn normal_of(g: &mut Graph, height: LayerId, strength: f32) -> LayerId {
    g.add_unique("normal", LayerKind::HeightToNormal(HeightToNormal { source: Some(height), strength }))
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

/// Poured concrete: pale, a little warm, with a fine grain of sand and aggregate and the odd air
/// hole, under broad patches a meter or two across. Nothing between the two scales, since a
/// middle-sized noise repeating every tile reads as a pattern rather than as concrete.
fn concrete() -> Graph {
    let mut g = empty();
    let patches = g.add_unique("patches", tiling_fbm(0, 1, 3)).unwrap();
    let base = mapped(&mut g, "base", patches, &[
        (0.25, oklcha(0.70, 0.008, 75.0, 1.0)),
        (0.75, oklcha(0.77, 0.010, 78.0, 1.0)),
    ]);
    let grain = g.add_unique("grain", tiling_fbm(1, 128, 2)).unwrap();
    let sand = mapped(&mut g, "sand", grain, &[
        (0.3, oklcha(0.64, 0.010, 72.0, 1.0)),
        (0.7, oklcha(0.80, 0.008, 80.0, 1.0)),
    ]);
    let sand_share = solid(&mut g, "sand share", gray(0.18));
    let sanded = blend(&mut g, "sanded", base, sand, sand_share);
    let pocking = g.add_unique("pocking", tiling_fbm(2, 64, 2)).unwrap();
    let holes = remap(&mut g, "holes", pocking, 0.81, 0.85);
    let hole_share = scaled(&mut g, "hole share", holes, 0.0, 0.35);
    let shade = solid(&mut g, "shade", oklcha(0.48, 0.008, 70.0, 1.0));
    let color = blend(&mut g, "color", sanded, shade, hole_share);
    let roughness = scaled(&mut g, "roughness", patches, 0.86, 0.95);
    let grit = scaled(&mut g, "grit", grain, 0.0, 0.5);
    let dents = scaled(&mut g, "dents", holes, 0.0, 0.6);
    let height = mix(&mut g, "height", grit, dents, BlendMode::Subtract);
    let normal = normal_of(&mut g, height, 0.003);
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
    let sheen = g.add_unique("sheen", tiling_fbm(10, 2, 4)).unwrap();
    let color = mapped(&mut g, "color", sheen, &[
        (0.2, oklcha(0.62, 0.008, 240.0, 1.0)),
        (0.8, oklcha(0.72, 0.006, 240.0, 1.0)),
    ]);
    let roughness = mapped(&mut g, "roughness", sheen, &[(0.2, gray(0.30)), (0.8, gray(0.45))]);
    let waves = g.add_unique("waves", tiling_fbm(11, 4, 2)).unwrap();
    let normal = g
        .add_unique("waviness", LayerKind::HeightToNormal(HeightToNormal { source: Some(waves), strength: 0.01 }))
        .unwrap();
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(1.0),
        normal: Some(normal),
    })
}

/// How metallic the floor and wall sheet metals are. Less than steel is, because a map has
/// nothing for metal to reflect but its point lights, so a fully metallic surface reads near black
/// wherever no highlight lands; the rest of their light is diffuse.
const SHEET_METALLIC: f32 = 0.4;

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

    let grime = g.add_unique("grime", tiling_fbm(20, 4, 5)).unwrap();
    let (grimy, clean) = noisy_pair(oklcha(0.58, 0.006, 70.0, 1.0), oklcha(0.72, 0.004, 230.0, 1.0));
    let sheet = mapped(&mut g, "sheet", grime, &[(0.25, grimy), (0.75, clean)]);
    let polished = solid(&mut g, "polished", oklcha(0.86, 0.004, 230.0, 1.0));
    let color = blend(&mut g, "color", sheet, polished, height);
    let (rough, smooth) = noisy(0.45, 0.70);
    let dull = scaled(&mut g, "dull", grime, smooth, rough);
    let shine = solid(&mut g, "shine", gray(0.28));
    let roughness = blend(&mut g, "roughness", dull, shine, height);
    let normal = normal_of(&mut g, height, 0.004);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(SHEET_METALLIC),
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
    let grain_noise = g.add_unique("grain noise", tiling_fbm(3, 8, 3)).unwrap();
    let grain = g
        .add_unique(
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

    let mottle = g.add_unique("mottle", tiling_fbm(0, 4, 6)).unwrap();
    let middle = oklcha(0.61, 0.008, 75.0, 1.0);
    let base = mapped(&mut g, "base", mottle, &[
        (0.2, toward(middle, oklcha(0.52, 0.007, 75.0, 1.0))),
        (0.5, middle),
        (0.8, toward(middle, oklcha(0.69, 0.010, 75.0, 1.0))),
    ]);
    let streak_noise = g.add_unique("streak noise", tiling_fbm(2, 4, 4)).unwrap();
    let streaks = g
        .add_unique(
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
    let pits = g.add_unique("pits", tiling_fbm(1, 32, 3)).unwrap();
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

/// `a * b` for two grays: `a` blended up from black by `b`. Not a `Multiply`, which multiplies in
/// linear sRGB and so is not linear in L.
fn product(g: &mut Graph, name: &str, zero: LayerId, a: LayerId, b: LayerId) -> LayerId {
    blend(g, name, zero, a, b)
}

/// One octave of value noise on a `cells`-square lattice that wraps at the tile's edge.
fn lattice(seed_offset: u32, cells: u32) -> LayerKind {
    tiling_fbm(seed_offset, cells, 1)
}

/// Value noise stretched by a whole `scale` per axis, so it still tiles. A scale of 8 in `v`
/// draws features eight times shorter in `v`, so they run along `u`.
fn stretched(g: &mut Graph, name: &str, source: LayerId, scale: [f32; 2]) -> LayerId {
    g.add_unique(
        name,
        LayerKind::Transform(Transform {
            source: Some(source),
            offset: [0.0; 3],
            rotate_uv: 0.0,
            scale: [scale[0], scale[1], 1.0],
            coord_mode: CoordMode::Passthrough,
            edge_mode: EdgeMode::Extend,
        }),
    )
    .unwrap()
}

/// Bricks along `u` and courses down `v` in one two-meter tile: 250 × 83 mm, a 225 × 75 mm module
/// scaled up so a tile holds whole bricks. An even number of courses, so the bond ends on the
/// course it began with.
const STRETCHERS: u32 = 8;
const COURSES: u32 = 24;
const TILE_MM: f32 = 2000.0;

/// How a wall of bricks is laid and finished, in millimeters.
struct Bond {
    joint: f32,
    /// How far in from a joint's edge the arris has rounded over to the face.
    arris: f32,
    /// How far a joint's edge wanders either way.
    chips: f32,
    seed: u32,
}

struct Bricks {
    /// 1 in a joint, 0 on a brick.
    joint: LayerId,
    /// 0 at a joint's edge, rising over the arris to 1 on the face.
    face: LayerId,
    /// One value per brick, in `[0, 1]`, unrelated to its neighbors'.
    lot: LayerId,
}

/// Stretcher bond: each course half a brick along from the one above.
///
/// Texture-graph has no cell noise, so a brick's own value is looked up: a brick's column and
/// course, each `x - fract(x)` from a sawtooth, make an id `column / STRETCHERS + course /
/// (STRETCHERS * COURSES)`, and a `Map` reads a lattice of that many cells at it, so each id lands
/// on its own lattice point. The id is wrapped into `[0, 1)` so the half brick past the tile's
/// right edge reads the same cell as its other half at the left, since a `Map` clamps on the GPU.
fn bricks(g: &mut Graph, bond: &Bond) -> Bricks {
    let (columns, courses) = (STRETCHERS as f32, COURSES as f32);
    let u = coordinate(g, Axis::U);
    let v = coordinate(g, Axis::V);

    // 1 on even courses, 0 on odd, pushing the even ones half a brick along.
    let parity = wave(g, "parity", v, WaveShape::Square, courses / 2.0, 0.0);
    let half_brick = scaled(g, "half brick", parity, 0.0, 0.5 / columns);
    let along = mix(g, "along", u, half_brick, BlendMode::Add);

    let into_course = wave(g, "into course", v, WaveShape::Sawtooth, courses, 0.0);
    let course_step = scaled(g, "course step", into_course, 0.0, 1.0 / courses);
    let course_start = mix(g, "course start", v, course_step, BlendMode::Subtract);
    let course = wave(g, "course", course_start, WaveShape::Sawtooth, 1.0, 0.0);
    let into_brick = wave(g, "into brick", along, WaveShape::Sawtooth, columns, 0.0);
    let brick_step = scaled(g, "brick step", into_brick, 0.0, 1.0 / columns);
    let column = mix(g, "column", along, brick_step, BlendMode::Subtract);
    let course_share = scaled(g, "course share", course, 0.0, 1.0 / columns);
    let id_sum = mix(g, "id sum", column, course_share, BlendMode::Add);
    let id = wave(g, "id", id_sum, WaveShape::Sawtooth, 1.0, 0.0);
    let draw = g.add_unique("draw", lattice(bond.seed, STRETCHERS * COURSES)).unwrap();
    let lot = g.add_unique("lot", LayerKind::Map(Map { value: Some(id), palette: Some(draw) })).unwrap();

    // A triangle wave peaks at 1 on a joint's center line and falls by 2 per cycle, so `mm` from
    // the line it reads `1 - 2 * mm / cycle`.
    let course_cycle = TILE_MM / courses;
    let brick_cycle = TILE_MM / columns;
    let across_course = wave(g, "across course", v, WaveShape::Triangle, courses, 0.25);
    let across_brick = wave(g, "across brick", along, WaveShape::Triangle, columns, 0.25);
    let at = |mm: f32, cycle: f32| 1.0 - 2.0 * mm / cycle;
    let edge = bond.joint / 2.0;
    // One texel either side of the edge.
    let soft = TILE_MM / 512.0 / 2.0;

    let course_joint = remap(g, "course joint", across_course, at(edge + soft, course_cycle), at(edge - soft, course_cycle));
    let brick_joint = remap(g, "brick joint", across_brick, at(edge + soft, brick_cycle), at(edge - soft, brick_cycle));
    let joint = pick(g, "joint", course_joint, brick_joint, MinMaxMode::Max);

    // Convex: most of the rise is near the edge, so the arris reads as rounded, not chamfered.
    let arris = |g: &mut Graph, name: &str, across: LayerId, cycle: f32| {
        let (lo, hi) = (at(edge, cycle), at(edge + bond.arris, cycle));
        mapped(g, name, across, &[(hi, gray(1.0)), (hi + (lo - hi) * 0.4, gray(0.75)), (lo, gray(0.0))])
    };
    let course_face = arris(g, "course face", across_course, course_cycle);
    let brick_face = arris(g, "brick face", across_brick, brick_cycle);
    let face = pick(g, "face", course_face, brick_face, MinMaxMode::Min);

    // A signed driver, so an edge wanders both ways rather than moving off its line.
    let chipping = g
        .add_unique(
            "chipping",
            LayerKind::Noise(Noise {
                dims: NoiseDims::D2,
                seed_offset: bond.seed + 1,
                frequency: 48.0,
                range: NoiseRange::Signed,
                output: NoiseOutput::Grayscale,
                kernel: NoiseKernel::Value,
                period: [48, 48, 0],
                fractal: Fractal { octaves: 3, lacunarity: 2.0, gain: 0.5, mode: FractalMode::Standard, normalize: true },
            }),
        )
        .unwrap();
    let wander = bond.chips / TILE_MM;
    let warp = |g: &mut Graph, name: &str, source: LayerId| {
        g.add_unique(
            name,
            LayerKind::Warp(Warp { source: Some(source), by: Some(chipping), mode: WarpMode::Scalar, amount: [wander, wander, 0.0] }),
        )
        .unwrap()
    };
    let joint = warp(g, "chipped joint", joint);
    let face = warp(g, "chipped face", face);
    Bricks { joint, face, lot }
}

/// Fired clay brick in a stretcher bond: the color differs from brick to brick, the face is
/// gritty and pitted, the arrises chipped, the joints raked back in gray sand-and-cement mortar.
fn red_brick() -> Graph {
    let mut g = empty();
    let b = bricks(&mut g, &Bond { joint: 10.0, arris: 3.0, chips: 2.5, seed: 50 });

    let fired = mapped(&mut g, "fired", b.lot, &[
        (0.0, oklcha(0.38, 0.080, 30.0, 1.0)),
        (0.5, oklcha(0.46, 0.100, 35.0, 1.0)),
        (1.0, oklcha(0.53, 0.105, 42.0, 1.0)),
    ]);
    let mottle = g.add_unique("mottle", tiling_fbm(51, 8, 5)).unwrap();
    let scorched = remap(&mut g, "scorched", mottle, 0.45, 0.85);
    let scorch_share = scaled(&mut g, "scorch share", scorched, 0.0, 0.5);
    let scorch = solid(&mut g, "scorch", oklcha(0.30, 0.050, 30.0, 1.0));
    let clay = blend(&mut g, "clay", fired, scorch, scorch_share);
    let speckle = g.add_unique("speckle", tiling_fbm(52, 128, 2)).unwrap();
    let dark_fleck = remap(&mut g, "dark fleck", speckle, 0.24, 0.18);
    let dark_specks = scaled(&mut g, "dark specks", dark_fleck, 0.0, 0.6);
    let iron = solid(&mut g, "iron", oklcha(0.26, 0.030, 40.0, 1.0));
    let flecked = blend(&mut g, "flecked", clay, iron, dark_specks);
    let light_fleck = remap(&mut g, "light fleck", speckle, 0.78, 0.84);
    let light_specks = scaled(&mut g, "light specks", light_fleck, 0.0, 0.5);
    let sand = solid(&mut g, "sand", oklcha(0.66, 0.050, 70.0, 1.0));
    let brick = blend(&mut g, "brick", flecked, sand, light_specks);

    let mortar_grain = g.add_unique("mortar grain", tiling_fbm(53, 32, 4)).unwrap();
    let (dull, pale) = noisy_pair(oklcha(0.50, 0.010, 85.0, 1.0), oklcha(0.72, 0.010, 85.0, 1.0));
    let mortar = mapped(&mut g, "mortar", mortar_grain, &[(0.25, dull), (0.75, pale)]);
    let color = blend(&mut g, "color", brick, mortar, b.joint);

    let grit = g.add_unique("grit", tiling_fbm(54, 64, 3)).unwrap();
    let brick_rough = scaled(&mut g, "brick roughness", grit, 0.86, 0.96);
    let mortar_rough = solid(&mut g, "mortar roughness", gray(0.97));
    let roughness = blend(&mut g, "roughness", brick_rough, mortar_rough, b.joint);

    let pitting = g.add_unique("pitting", tiling_fbm(55, 32, 3)).unwrap();
    let pits = remap(&mut g, "pits", pitting, 0.64, 0.74);
    let lumps = g.add_unique("lumps", tiling_fbm(56, 8, 3)).unwrap();
    let swell = scaled(&mut g, "swell", lumps, 0.55, 0.70);
    let proud = scaled(&mut g, "proud", b.lot, 0.0, 0.12);
    let sharp = scaled(&mut g, "sharp", grit, 0.0, 1.0);
    let holes = scaled(&mut g, "holes", pits, 0.0, 0.6);
    let laid = mix(&mut g, "laid", swell, proud, BlendMode::Add);
    let gritty = mix(&mut g, "gritty", laid, sharp, BlendMode::Add);
    let brick_top = mix(&mut g, "brick top", gritty, holes, BlendMode::Subtract);
    let mortar_sand = scaled(&mut g, "mortar sand", mortar_grain, 0.0, 0.12);
    let mortar_grit = scaled(&mut g, "mortar grit", grit, 0.0, 0.15);
    let mortar_top = mix(&mut g, "mortar top", mortar_sand, mortar_grit, BlendMode::Add);
    let height = blend(&mut g, "height", mortar_top, brick_top, b.face);
    let normal = normal_of(&mut g, height, 0.004);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// A paint or a glaze, in Oklch.
#[derive(Clone, Copy)]
struct Coat {
    lightness: f32,
    chroma: f32,
    hue: f32,
}

impl Coat {
    const WHITE: Coat = Coat { lightness: 0.91, chroma: 0.008, hue: 90.0 };
    /// A glaze whiter than any paint.
    const GLAZE_WHITE: Coat = Coat { lightness: 0.94, chroma: 0.005, hue: 95.0 };
    const GRAY: Coat = Coat { lightness: 0.62, chroma: 0.006, hue: 250.0 };
    const ORANGE: Coat = Coat { lightness: 0.72, chroma: 0.130, hue: 58.0 };
    /// The four teams' colors, lightness close together so no team's walls read as lit differently.
    const RED: Coat = Coat { lightness: 0.56, chroma: 0.150, hue: 26.0 };
    const BLUE: Coat = Coat { lightness: 0.56, chroma: 0.120, hue: 252.0 };
    const YELLOW: Coat = Coat { lightness: 0.80, chroma: 0.135, hue: 90.0 };
    const GREEN: Coat = Coat { lightness: 0.60, chroma: 0.120, hue: 148.0 };
    /// The two tiles of the gray checkered vinyl floor.
    const GRAY_CHECKER: (Coat, Coat) = (
        Coat { lightness: 0.88, chroma: 0.006, hue: 250.0 },
        Coat { lightness: 0.85, chroma: 0.006, hue: 250.0 },
    );

    const fn lighter(self, by: f32) -> Coat {
        Coat { lightness: self.lightness + by, ..self }
    }

    /// `by` of the way to white: lightness toward 1 and chroma toward 0.
    const fn toward_white(self, by: f32) -> Coat {
        Coat { lightness: self.lightness + (1.0 - self.lightness) * by, chroma: self.chroma * (1.0 - by), ..self }
    }

    /// The two tiles of a checkered vinyl floor in this color: a pale pastel, and that a little
    /// lighter and paler again.
    const fn checker(self) -> (Coat, Coat) {
        let base = self.toward_white(0.55).paler(0.75);
        (base, base.paler(0.8).lighter(0.03))
    }

    /// Chroma scaled by `by`.
    const fn paler(self, by: f32) -> Coat {
        Coat { chroma: self.chroma * by, ..self }
    }
}

/// The red brick's bond under coats of satin latex: the paint has filled the grit and pits and
/// softened the arrises and joints, and has a little sheen.
fn painted_brick(paint: Coat) -> Graph {
    let mut g = empty();
    let b = bricks(&mut g, &Bond { joint: 10.0, arris: 10.0, chips: 1.5, seed: 50 });
    let coat = |l: f32| oklcha(l, paint.chroma, paint.hue, 1.0);

    let trowel = g.add_unique("roller", tiling_fbm(70, 4, 4)).unwrap();
    let (thin, thick) = noisy_pair(coat(paint.lightness - 0.03), coat(paint.lightness + 0.02));
    let wall = mapped(&mut g, "wall", trowel, &[(0.25, thin), (0.75, thick)]);
    // The brick under it still shows a little through the paint.
    let show_through = scaled(&mut g, "show through", b.lot, 0.0, 0.25);
    let darker = solid(&mut g, "darker", coat(paint.lightness - 0.05));
    let bricks_under = blend(&mut g, "bricks under", wall, darker, show_through);
    let pooled = solid(&mut g, "pooled", coat(paint.lightness - 0.06));
    let pooled_share = scaled(&mut g, "pooled share", b.joint, 0.0, 0.6);
    let color = blend(&mut g, "color", bricks_under, pooled, pooled_share);

    let sheen = g.add_unique("sheen", tiling_fbm(72, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.40, 0.55);
    let face_rough = scaled(&mut g, "face roughness", sheen, smooth, rough);
    let joint_rough = solid(&mut g, "joint roughness", gray(0.65));
    let roughness = blend(&mut g, "roughness", face_rough, joint_rough, b.joint);

    let peel = g.add_unique("peel", tiling_fbm(71, 64, 2)).unwrap();
    let pitting = g.add_unique("pitting", tiling_fbm(55, 32, 3)).unwrap();
    let pits = remap(&mut g, "pits", pitting, 0.68, 0.76);
    let lumps = g.add_unique("lumps", tiling_fbm(56, 8, 3)).unwrap();
    let swell = scaled(&mut g, "swell", lumps, 0.60, 0.68);
    let proud = scaled(&mut g, "proud", b.lot, 0.0, 0.05);
    let orange_peel = scaled(&mut g, "orange peel", peel, 0.0, 0.06);
    let dimples = scaled(&mut g, "dimples", pits, 0.0, 0.06);
    let laid = mix(&mut g, "laid", swell, proud, BlendMode::Add);
    let rolled = mix(&mut g, "rolled", laid, orange_peel, BlendMode::Add);
    let brick_top = mix(&mut g, "brick top", rolled, dimples, BlendMode::Subtract);
    let joint_top = scaled(&mut g, "joint top", peel, 0.25, 0.31);
    let height = blend(&mut g, "height", joint_top, brick_top, b.face);
    let normal = normal_of(&mut g, height, 0.002);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Glazed ceramic tiles, about 4 inches, twenty to a tile. 100 mm in a 2 m tile.
const TILES: u32 = 20;

/// Glazed square tiles in gray grout: each tile a shade off its neighbors and set a hair proud or
/// shy of them, the glaze glossy and its edges cushioned down to the grout.
///
/// A tile's own value is looked up as a brick's is ([`bricks`]), with no offset between rows.
fn tiles(glaze: Coat) -> Graph {
    let count = TILES as f32;
    let cycle = TILE_MM / count;
    let mut g = empty();
    let u = coordinate(&mut g, Axis::U);
    let v = coordinate(&mut g, Axis::V);

    let into_column = wave(&mut g, "into column", u, WaveShape::Sawtooth, count, 0.0);
    let column_step = scaled(&mut g, "column step", into_column, 0.0, 1.0 / count);
    let column = mix(&mut g, "column", u, column_step, BlendMode::Subtract);
    let into_row = wave(&mut g, "into row", v, WaveShape::Sawtooth, count, 0.0);
    let row_step = scaled(&mut g, "row step", into_row, 0.0, 1.0 / count);
    let row_start = mix(&mut g, "row start", v, row_step, BlendMode::Subtract);
    let row = wave(&mut g, "row", row_start, WaveShape::Sawtooth, 1.0, 0.0);
    let row_share = scaled(&mut g, "row share", row, 0.0, 1.0 / count);
    let id_sum = mix(&mut g, "id sum", column, row_share, BlendMode::Add);
    let id = wave(&mut g, "id", id_sum, WaveShape::Sawtooth, 1.0, 0.0);
    let draw = g.add_unique("draw", lattice(95, TILES * TILES)).unwrap();
    let lot = g.add_unique("lot", LayerKind::Map(Map { value: Some(id), palette: Some(draw) })).unwrap();

    let at = |mm: f32| 1.0 - 2.0 * mm / cycle;
    let across_u = wave(&mut g, "across u", u, WaveShape::Triangle, count, 0.25);
    let across_v = wave(&mut g, "across v", v, WaveShape::Triangle, count, 0.25);
    // 4 mm of grout, softened over about half a texel either side.
    let grout_u = remap(&mut g, "grout u", across_u, at(3.0), at(1.0));
    let grout_v = remap(&mut g, "grout v", across_v, at(3.0), at(1.0));
    let grout = pick(&mut g, "grout", grout_u, grout_v, MinMaxMode::Max);
    let cushion = |g: &mut Graph, name: &str, across: LayerId| {
        mapped(g, name, across, &[(at(6.0), gray(1.0)), (at(3.5), gray(0.7)), (at(2.0), gray(0.0))])
    };
    let face_u = cushion(&mut g, "face u", across_u);
    let face_v = cushion(&mut g, "face v", across_v);
    let face = pick(&mut g, "face", face_u, face_v, MinMaxMode::Min);

    let shade = |l: f32| oklcha(l, glaze.chroma, glaze.hue, 1.0);
    let glazed = mapped(&mut g, "glazed", lot, &[(0.0, shade(glaze.lightness - 0.025)), (1.0, shade(glaze.lightness + 0.01))]);
    let grime = g.add_unique("grime", tiling_fbm(96, 16, 3)).unwrap();
    let (dirty, clean) = noisy_pair(oklcha(0.50, 0.008, 80.0, 1.0), oklcha(0.66, 0.006, 80.0, 1.0));
    let grout_color = mapped(&mut g, "grout color", grime, &[(0.25, dirty), (0.75, clean)]);
    let color = blend_linear(&mut g, "color", glazed, grout_color, grout);

    let (gloss, satin) = noisy(0.12, 0.28);
    let glaze_rough = scaled(&mut g, "glaze roughness", lot, gloss, satin);
    let grout_rough = solid(&mut g, "grout roughness", gray(0.92));
    let roughness = blend(&mut g, "roughness", glaze_rough, grout_rough, grout);

    let ripple = g.add_unique("ripple", tiling_fbm(97, 32, 2)).unwrap();
    let waves = scaled(&mut g, "waves", ripple, 0.0, 0.05);
    let lippage = scaled(&mut g, "lippage", lot, 0.75, 0.85);
    let top = mix(&mut g, "top", lippage, waves, BlendMode::Add);
    let grout_sand = scaled(&mut g, "grout sand", grime, 0.0, 0.08);
    let height = blend(&mut g, "height", grout_sand, top, face);
    let normal = normal_of(&mut g, height, 0.0015);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Galvanized corrugated sheet, the ribs running down `v`: scratched across the crests more in
/// some places than others, and streaked with rust and grime that ran down the troughs.
///
/// The profile is a sine of 77 mm pitch and 18 mm depth. A height of 1 is 18 mm, and the tile is
/// 2 m across, so a normal strength of `18 / 2000` gives the sheet's real slope.
fn corrugated_steel() -> Graph {
    const PITCH: f32 = 26.0;
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let u = coordinate(&mut g, Axis::U);
    let profile = wave(&mut g, "profile", u, WaveShape::Sine, PITCH, 0.0);
    let crest = remap(&mut g, "crest", profile, 0.55, 0.95);
    let trough = remap(&mut g, "trough", profile, 0.45, 0.05);

    let spangle = g.add_unique("spangle", tiling_fbm(60, 6, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.70, 0.006, 230.0, 1.0), oklcha(0.82, 0.004, 230.0, 1.0));
    let zinc = mapped(&mut g, "zinc", spangle, &[(0.25, dim), (0.75, bright)]);

    let wear_noise = g.add_unique("wear noise", tiling_fbm(61, 2, 3)).unwrap();
    let wear = remap(&mut g, "wear", wear_noise, 0.30, 0.60);
    let scratch_noise = g
        .add_unique(
            "scratch noise",
            LayerKind::Noise(Noise {
                dims: NoiseDims::D2,
                seed_offset: 62,
                frequency: 8.0,
                range: NoiseRange::Unsigned,
                output: NoiseOutput::Grayscale,
                kernel: NoiseKernel::Value,
                period: [8, 8, 0],
                fractal: Fractal { octaves: 4, lacunarity: 2.0, gain: 0.5, mode: FractalMode::Ridged, normalize: true },
            }),
        )
        .unwrap();
    let scratch_lines = stretched(&mut g, "scratch lines", scratch_noise, [1.0, 12.0]);
    let scratches = remap(&mut g, "scratches", scratch_lines, 0.62, 0.85);
    let exposure = scaled(&mut g, "exposure", crest, 0.25, 1.0);
    let reach = product(&mut g, "reach", zero, exposure, wear);
    let scuff = product(&mut g, "scuff", zero, scratches, reach);
    let bare = solid(&mut g, "bare", oklcha(0.90, 0.003, 230.0, 1.0));
    let scuffed = blend(&mut g, "scuffed", zinc, bare, scuff);
    // Zinc that has weathered broadly goes paler and duller.
    let chalky = solid(&mut g, "chalky", oklcha(0.82, 0.004, 210.0, 1.0));
    let chalk_share = scaled(&mut g, "chalk share", wear, 0.0, 0.35);
    let weathered = blend(&mut g, "weathered", scuffed, chalky, chalk_share);

    let streak_noise = g.add_unique("streak noise", tiling_fbm(63, 4, 5)).unwrap();
    let streak_lines = stretched(&mut g, "streak lines", streak_noise, [8.0, 1.0]);
    let streaks = remap(&mut g, "streaks", streak_lines, 0.42, 0.70);
    let staining_noise = g.add_unique("staining noise", tiling_fbm(64, 2, 3)).unwrap();
    let staining = remap(&mut g, "staining", staining_noise, 0.30, 0.60);
    let gutter = scaled(&mut g, "gutter", trough, 0.3, 1.0);
    let run = product(&mut g, "run", zero, streaks, gutter);
    let stain = product(&mut g, "stain", zero, run, staining);
    let kind = g.add_unique("rust or grime", tiling_fbm(65, 4, 3)).unwrap();
    let stain_color = mapped(&mut g, "stain color", kind, &[
        (0.35, oklcha(0.46, 0.085, 52.0, 1.0)),
        (0.65, oklcha(0.38, 0.020, 70.0, 1.0)),
    ]);
    let stain_share = scaled(&mut g, "stain share", stain, 0.0, 0.9);
    let color = blend_linear(&mut g, "color", weathered, stain_color, stain_share);

    let (smooth, rough) = noisy(0.32, 0.48);
    let zinc_rough = scaled(&mut g, "zinc roughness", spangle, smooth, rough);
    let scratch_rough = solid(&mut g, "scratch roughness", gray(0.55));
    let scuffed_rough = blend(&mut g, "scuffed roughness", zinc_rough, scratch_rough, scuff);
    let chalk_rough = solid(&mut g, "chalk roughness", gray(0.65));
    let weathered_rough = blend(&mut g, "weathered roughness", scuffed_rough, chalk_rough, chalk_share);
    let stain_rough = solid(&mut g, "stain roughness", gray(0.88));
    let roughness = blend(&mut g, "roughness", weathered_rough, stain_rough, stain_share);
    let metal = solid(&mut g, "metal", gray(SHEET_METALLIC));
    let oxide = solid(&mut g, "oxide", gray(0.1));
    let metallic = blend(&mut g, "metallic", metal, oxide, stain_share);

    let dent_noise = g.add_unique("dent noise", tiling_fbm(66, 4, 3)).unwrap();
    let dents = scaled(&mut g, "dents", dent_noise, 0.0, 0.08);
    let grooves = scaled(&mut g, "grooves", scuff, 0.0, 0.01);
    let dented = mix(&mut g, "dented", profile, dents, BlendMode::Add);
    let height = mix(&mut g, "height", dented, grooves, BlendMode::Subtract);
    let normal = normal_of(&mut g, height, 18.0 / TILE_MM);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Layer(metallic),
        normal: Some(normal),
    })
}

/// Pine's figure, unbroken across the texture tile: each layer is 0 to 1.
struct PineFigure {
    /// The dark late wood of each growth ring.
    late_wood: LayerId,
    knot_core: LayerId,
    /// The rings bent dark round a knot.
    knot_swirl: LayerId,
    fibers: LayerId,
    splits: LayerId,
}

/// Growth-ring lines running along `v`, `lines` of them across the texture tile, wandering with
/// noise stretched along them and bent round knots. `u` plus the wander, through a whole number of
/// cycles, so the lines tile. Takes seeds `seed` to `seed + 3`.
fn pine_figure(g: &mut Graph, u: LayerId, lines: u32, seed: u32) -> PineFigure {
    let count = lines as f32;
    let figure_noise = g.add_unique("figure noise", tiling_fbm(seed, 4, 3)).unwrap();
    let figure = stretched(g, "figure", figure_noise, [4.0, 1.0]);
    let wander = scaled(g, "wander", figure, 0.0, 3.0 / count);
    let wandering = mix(g, "wandering", u, wander, BlendMode::Add);
    let knot_spots = g.add_unique("knot spots", tiling_fbm(seed + 1, 8, 2)).unwrap();
    let knot_noise = stretched(g, "knot noise", knot_spots, [2.0, 1.0]);
    let knot_swirl = remap(g, "knot swirl", knot_noise, 0.70, 0.84);
    let swirl = scaled(g, "swirl", knot_swirl, 0.0, 2.0 / count);
    let bent = mix(g, "bent", wandering, swirl, BlendMode::Add);
    let rings = wave(g, "rings", bent, WaveShape::Triangle, count, 0.0);
    let late_wood = remap(g, "late wood", rings, 0.70, 0.95);
    let knot_core = remap(g, "knot core", knot_noise, 0.82, 0.84);
    let fiber_noise = g.add_unique("fiber noise", tiling_fbm(seed + 2, 8, 4)).unwrap();
    let fibers = stretched(g, "fibers", fiber_noise, [16.0, 1.0]);
    let split_noise = g
        .add_unique(
            "split noise",
            LayerKind::Noise(Noise {
                dims: NoiseDims::D2,
                seed_offset: seed + 3,
                frequency: 4.0,
                range: NoiseRange::Unsigned,
                output: NoiseOutput::Grayscale,
                kernel: NoiseKernel::Value,
                period: [4, 4, 0],
                fractal: Fractal { octaves: 3, lacunarity: 2.0, gain: 0.5, mode: FractalMode::Ridged, normalize: true },
            }),
        )
        .unwrap();
    let split_lines = stretched(g, "split lines", split_noise, [24.0, 1.0]);
    let splits = remap(g, "splits", split_lines, 0.93, 0.98);
    PineFigure { late_wood, knot_core, knot_swirl, fibers, splits }
}

/// A pine board's tone, in Oklch: the five the fence's boards are drawn between.
#[derive(Clone, Copy)]
struct Pine {
    lightness: f32,
    chroma: f32,
    hue: f32,
}

impl Pine {
    const BROWN: Pine = Pine { lightness: 0.50, chroma: 0.050, hue: 62.0 };
    const HONEY: Pine = Pine { lightness: 0.62, chroma: 0.065, hue: 72.0 };
    const PALE: Pine = Pine { lightness: 0.70, chroma: 0.070, hue: 80.0 };
    /// Weathering toward gray.
    const SILVERING: Pine = Pine { lightness: 0.60, chroma: 0.030, hue: 78.0 };
    const GRAY: Pine = Pine { lightness: 0.56, chroma: 0.015, hue: 80.0 };

    fn color(self) -> Color {
        oklcha(self.lightness, self.chroma, self.hue, 1.0)
    }

    fn shade(self, dl: f32) -> Color {
        oklcha(self.lightness + dl, self.chroma, self.hue, 1.0)
    }
}

/// One of the fence's pine tones as an unbroken surface: no gaps, nails or board edges, so the
/// shape it is painted on is the shape of the board. Growth rings about 24 mm apart run along `v`,
/// knotty and split here and there, with the grain raised by weathering.
fn pine(tone: Pine, seed: u32) -> Graph {
    let mut g = empty();
    let u = coordinate(&mut g, Axis::U);
    let PineFigure { late_wood, knot_core, knot_swirl, fibers, splits } = pine_figure(&mut g, u, 84, seed);

    // A broad drift in tone along the grain, so a long piece is not one flat color.
    let drift_noise = g.add_unique("drift noise", tiling_fbm(seed + 4, 2, 3)).unwrap();
    let drift = stretched(&mut g, "drift", drift_noise, [4.0, 1.0]);
    let (dim, bright) = noisy_pair(tone.shade(-0.04), tone.shade(0.04));
    let lumber = mapped(&mut g, "lumber", drift, &[(0.25, dim), (0.75, bright)]);
    // The late wood is the same wood darker, so a gray board's rings are gray too.
    let late = solid(&mut g, "late", oklcha(tone.lightness * 0.8, tone.chroma * 0.9, tone.hue - 6.0, 1.0));
    let fiber_share = scaled(&mut g, "fiber share", fibers, 0.0, 0.25);
    let fibered = blend(&mut g, "fibered", lumber, late, fiber_share);
    let ring_share = scaled(&mut g, "ring share", late_wood, 0.0, 0.45);
    let grained = blend(&mut g, "grained", fibered, late, ring_share);
    let knot_rim_share = scaled(&mut g, "knot rim share", knot_swirl, 0.0, 0.35);
    let knotty = blend(&mut g, "knotty", grained, late, knot_rim_share);
    let knot = solid(&mut g, "knot", oklcha(tone.lightness * 0.6, tone.chroma, tone.hue - 12.0, 1.0));
    let knotted = blend(&mut g, "knotted", knotty, knot, knot_core);
    let crack = solid(&mut g, "crack", oklcha(0.20, 0.020, 60.0, 1.0));
    let color = blend(&mut g, "color", knotted, crack, splits);

    let roughness = scaled(&mut g, "roughness", fibers, 0.82, 0.95);

    let raised = scaled(&mut g, "raised grain", late_wood, 0.0, 0.25);
    let fuzz = scaled(&mut g, "fuzz", fibers, 0.0, 0.15);
    let base = solid(&mut g, "base", gray(0.5));
    let grained_top = mix(&mut g, "grained top", base, raised, BlendMode::Add);
    let fuzzy = mix(&mut g, "fuzzy", grained_top, fuzz, BlendMode::Add);
    let crack_depth = scaled(&mut g, "crack depth", splits, 0.0, 0.4);
    let height = mix(&mut g, "height", fuzzy, crack_depth, BlendMode::Subtract);
    let normal = normal_of(&mut g, height, 0.004);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// A privacy fence of cheap pine boards standing side by side down `v`: each board its own tone,
/// some weathered gray, knotty, split here and there, cupped, with a hairline gap between boards
/// and two rusting nails per board at each of two rails.
///
/// A board's own value is looked up as a brick's is ([`bricks`]), from its column alone. Each
/// board's figure is that one wood pattern shifted by its value, so no two neighbors match.
fn wooden_fence() -> Graph {
    /// 143 mm, about a nominal 1×6.
    const BOARDS: u32 = 14;
    let boards = BOARDS as f32;
    let board_cycle = TILE_MM / boards;
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let u = coordinate(&mut g, Axis::U);
    let v = coordinate(&mut g, Axis::V);

    let into_board = wave(&mut g, "into board", u, WaveShape::Sawtooth, boards, 0.0);
    let board_step = scaled(&mut g, "board step", into_board, 0.0, 1.0 / boards);
    let column_start = mix(&mut g, "column start", u, board_step, BlendMode::Subtract);
    let column = wave(&mut g, "column", column_start, WaveShape::Sawtooth, 1.0, 0.0);
    let draw = g.add_unique("draw", lattice(80, BOARDS)).unwrap();
    let lot = g.add_unique("lot", LayerKind::Map(Map { value: Some(column), palette: Some(draw) })).unwrap();

    // About six lines to a board.
    let PineFigure { late_wood, knot_core, knot_swirl, fibers, splits } = pine_figure(&mut g, u, 6 * BOARDS, 81);
    // Shifted by the board's own value; the same at a tile's two edges, since the id wraps.
    let shift = |g: &mut Graph, name: &str, source: LayerId| {
        g.add_unique(
            name,
            LayerKind::Warp(Warp { source: Some(source), by: Some(lot), mode: WarpMode::Scalar, amount: [0.5, 0.5, 0.0] }),
        )
        .unwrap()
    };
    let late_wood = shift(&mut g, "board late wood", late_wood);
    let knot_core = shift(&mut g, "board knots", knot_core);
    let knot_swirl = shift(&mut g, "board knot swirl", knot_swirl);
    let fibers = shift(&mut g, "board fibers", fibers);
    let splits = shift(&mut g, "board splits", splits);

    // Across a board: a triangle peaking on the gap between two boards.
    let at = |mm: f32| 1.0 - 2.0 * mm / board_cycle;
    let across_gap = wave(&mut g, "across gap", u, WaveShape::Triangle, boards, 0.25);
    let gap = remap(&mut g, "gap", across_gap, at(3.5), at(1.0));
    let face = mapped(&mut g, "face", across_gap, &[(at(10.0), gray(1.0)), (at(5.0), gray(0.75)), (at(2.0), gray(0.0))]);
    // A board cups: its edges stand proud of its middle.
    let cup = scaled(&mut g, "cup", across_gap, 0.0, 0.10);

    let lumber = mapped(&mut g, "lumber", lot, &[
        (0.0, Pine::BROWN.color()),
        (0.35, Pine::HONEY.color()),
        (0.65, Pine::PALE.color()),
        (0.85, Pine::SILVERING.color()),
        (1.0, Pine::GRAY.color()),
    ]);
    let fiber_share = scaled(&mut g, "fiber share", fibers, 0.0, 0.25);
    let late = solid(&mut g, "late", oklcha(0.42, 0.050, 58.0, 1.0));
    let fibered = blend(&mut g, "fibered", lumber, late, fiber_share);
    let ring_share = scaled(&mut g, "ring share", late_wood, 0.0, 0.45);
    let grained = blend(&mut g, "grained", fibered, late, ring_share);
    let knot_rim_share = scaled(&mut g, "knot rim share", knot_swirl, 0.0, 0.35);
    let knotty = blend(&mut g, "knotty", grained, late, knot_rim_share);
    let knot = solid(&mut g, "knot", oklcha(0.30, 0.050, 50.0, 1.0));
    let knotted = blend(&mut g, "knotted", knotty, knot, knot_core);
    let crack = solid(&mut g, "crack", oklcha(0.20, 0.020, 60.0, 1.0));
    let split = blend(&mut g, "split", knotted, crack, splits);

    // Nails at a quarter and three quarters across each board, on rails at a quarter and three
    // quarters down the tile. Both triangles have the board's cycle, so a nail is round enough.
    let nail_u = wave(&mut g, "nail u", u, WaveShape::Triangle, 2.0 * boards, 0.75);
    let nail_v = wave(&mut g, "nail v", v, WaveShape::Triangle, 2.0 * boards, 0.25);
    let half_u = scaled(&mut g, "half nail u", nail_u, 0.0, 0.5);
    let half_v = scaled(&mut g, "half nail v", nail_v, 0.0, 0.5);
    let nail_sum = mix(&mut g, "nail sum", half_u, half_v, BlendMode::Add);
    let nail_cycle = board_cycle / 2.0;
    let nail_head = remap(&mut g, "nail head", nail_sum, 1.0 - 8.0 / nail_cycle, 1.0 - 6.0 / nail_cycle);
    let rail_line = wave(&mut g, "rail line", v, WaveShape::Triangle, 2.0, 0.75);
    let rail = remap(&mut g, "rail", rail_line, 0.95, 0.97);
    let nail = product(&mut g, "nail", zero, nail_head, rail);
    // Rust runs about 60 mm down the board from each nail. A sawtooth restarting at each rail.
    let below_rail = wave(&mut g, "below rail", v, WaveShape::Sawtooth, 2.0, -0.5);
    let run_down = remap(&mut g, "run down", below_rail, 0.12, 0.0);
    let run_width = remap(&mut g, "run width", nail_u, 0.86, 0.96);
    let run = product(&mut g, "run", zero, run_width, run_down);
    let rust_noise = g.add_unique("rust noise", tiling_fbm(85, 32, 3)).unwrap();
    let blotchy = scaled(&mut g, "blotchy", rust_noise, 0.3, 0.9);
    let rust_share = product(&mut g, "rust share", zero, run, blotchy);
    let rust = solid(&mut g, "rust", oklcha(0.40, 0.080, 48.0, 1.0));
    let rusted = blend_linear(&mut g, "rusted", split, rust, rust_share);
    let iron = solid(&mut g, "iron", oklcha(0.32, 0.020, 50.0, 1.0));
    let nailed = blend(&mut g, "nailed", rusted, iron, nail);
    let shadow = solid(&mut g, "shadow", oklcha(0.12, 0.010, 60.0, 1.0));
    let color = blend(&mut g, "color", nailed, shadow, gap);

    let roughness_noise = scaled(&mut g, "wood roughness", fibers, 0.82, 0.95);
    let rough = solid(&mut g, "rough", gray(0.97));
    let roughness = blend(&mut g, "roughness", roughness_noise, rough, gap);

    // Weathering wears the soft early wood away, leaving the late wood standing.
    let raised = scaled(&mut g, "raised grain", late_wood, 0.0, 0.25);
    let fuzz = scaled(&mut g, "fuzz", fibers, 0.0, 0.15);
    let warp = scaled(&mut g, "warp", lot, 0.0, 0.15);
    let base = solid(&mut g, "base", gray(0.5));
    let cupped = mix(&mut g, "cupped", base, cup, BlendMode::Add);
    let offset = mix(&mut g, "offset", cupped, warp, BlendMode::Add);
    let grained_top = mix(&mut g, "grained top", offset, raised, BlendMode::Add);
    let fuzzy = mix(&mut g, "fuzzy", grained_top, fuzz, BlendMode::Add);
    let crack_depth = scaled(&mut g, "crack depth", splits, 0.0, 0.4);
    let cracked = mix(&mut g, "cracked", fuzzy, crack_depth, BlendMode::Subtract);
    let nail_rise = scaled(&mut g, "nail rise", nail, 0.0, 0.15);
    let board_top = mix(&mut g, "board top", cracked, nail_rise, BlendMode::Add);
    let height = blend(&mut g, "height", zero, board_top, face);
    let normal = normal_of(&mut g, height, 0.004);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Two coats of one hue in Oklch, a darker and a lighter, alternating as stripes.
struct StripePaint {
    dark: f32,
    light: f32,
    chroma: f32,
    hue: f32,
}

impl StripePaint {
    const ORANGE: StripePaint = StripePaint { dark: 0.66, light: 0.78, chroma: 0.125, hue: 58.0 };
    /// Lighter than the rest: a yellow as dark as the orange reads as olive.
    const YELLOW: StripePaint = StripePaint { dark: 0.78, light: 0.88, chroma: 0.125, hue: 92.0 };
    const GREEN: StripePaint = StripePaint { dark: 0.58, light: 0.70, chroma: 0.115, hue: 148.0 };
}

/// Matte paint in vertical stripes, eight pairs to a tile, so 125 mm each, with the trowel's
/// unevenness and the roller's orange peel the grid has.
fn stripes(paint: StripePaint) -> Graph {
    const PAIRS: f32 = 8.0;
    let mut g = empty();
    let u = coordinate(&mut g, Axis::U);
    // A triangle peaking at the middle of each light stripe; the light half is above 0.5. The
    // ramp's width softens the edge over about a texel: 4 mm in a 250 mm cycle is 0.03.
    let across = wave(&mut g, "across", u, WaveShape::Triangle, PAIRS, 0.25);
    let light_stripe = remap(&mut g, "light stripe", across, 0.485, 0.515);

    let trowel = g.add_unique("trowel", tiling_fbm(90, 2, 5)).unwrap();
    let coat = |l: f32| oklcha(l, paint.chroma, paint.hue, 1.0);
    let (dark_lo, dark_hi) = noisy_pair(coat(paint.dark - 0.03), coat(paint.dark + 0.03));
    let dark = mapped(&mut g, "dark coat", trowel, &[(0.25, dark_lo), (0.75, dark_hi)]);
    let (light_lo, light_hi) = noisy_pair(coat(paint.light - 0.03), coat(paint.light + 0.03));
    let light = mapped(&mut g, "light coat", trowel, &[(0.25, light_lo), (0.75, light_hi)]);
    let color = blend(&mut g, "color", dark, light, light_stripe);

    let sheen = g.add_unique("sheen", tiling_fbm(92, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.70, 0.85);
    let roughness = scaled(&mut g, "roughness", sheen, smooth, rough);
    let peel = g.add_unique("peel", tiling_fbm(91, 64, 2)).unwrap();
    let fine = scaled(&mut g, "fine", peel, 0.0, 0.5);
    // The second coat stands a hair proud of the first.
    let coat_edge = scaled(&mut g, "coat edge", light_stripe, 0.0, 0.15);
    let height = mix(&mut g, "height", fine, coat_edge, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.003 * NOISE);
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
    let trowel = g.add_unique("trowel", tiling_fbm(30, 2, 5)).unwrap();
    let (shaded, bright) = noisy_pair(
        oklcha(paint.lightness.0, paint.chroma.0, paint.hue, 1.0),
        oklcha(paint.lightness.1, paint.chroma.1, paint.hue, 1.0),
    );
    let color = mapped(&mut g, "color", trowel, &[(0.25, shaded), (0.75, bright)]);
    let sheen = g.add_unique("sheen", tiling_fbm(32, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.80, 0.95);
    let roughness = scaled(&mut g, "roughness", sheen, smooth, rough);
    let peel = g.add_unique("peel", tiling_fbm(31, 64, 2)).unwrap();
    let fine = scaled(&mut g, "fine", peel, 0.0, 0.5);
    // The trowel's undulation, much broader and so much gentler in slope than the peel.
    let skim = g.add_unique("skim", tiling_fbm(33, 3, 3)).unwrap();
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

/// A grid's paint in Oklch: the shaded and bright ends its trowel swings between, and the stripe.
struct GridPaint {
    lightness: (f32, f32),
    chroma: f32,
    hue: f32,
    stripe_lightness: f32,
    stripe_chroma: f32,
}

impl GridPaint {
    const fn new(lightness: (f32, f32), chroma: f32, hue: f32, stripe_lightness: f32, stripe_chroma: f32) -> Self {
        Self { lightness, chroma, hue, stripe_lightness, stripe_chroma }
    }

    /// Creamy orange: what walls with no material draw.
    const ORANGE: GridPaint = GridPaint::new((0.76, 0.82), 0.108, 64.0, 0.92, 0.12);
    /// The orange's lightness with no color, a touch warm: floors.
    const GRAY: GridPaint = GridPaint::new((0.76, 0.82), 0.004, 70.0, 0.92, 0.006);
    /// Ceilings.
    const LIGHT: GridPaint = GridPaint::new((0.88, 0.92), 0.004, 70.0, 0.98, 0.006);
    const RED: GridPaint = GridPaint::new((0.74, 0.80), 0.105, 24.0, 0.90, 0.10);
    const BLUE: GridPaint = GridPaint::new((0.74, 0.80), 0.090, 245.0, 0.91, 0.08);
    /// Lighter than the rest: a yellow as dark as the orange reads as olive.
    const YELLOW: GridPaint = GridPaint::new((0.84, 0.88), 0.110, 95.0, 0.95, 0.12);
    const GREEN: GridPaint = GridPaint::new((0.76, 0.82), 0.100, 145.0, 0.92, 0.10);
}

/// Matte paint ruled with a brighter stripe on every meter, so distances read at a glance: the
/// defaults a room draws before anybody paints it, and `team_grid`'s colors.
///
/// A tile is two meters, so lines at `u` and `v` frequency 2 fall on whole meters of the world.
fn grid(paint: GridPaint) -> Graph {
    let mut g = empty();
    let u = coordinate(&mut g, Axis::U);
    let v = coordinate(&mut g, Axis::V);
    // A triangle wave falls by 2 per cycle away from its peak, so 0.97 is 1.5 cm either side of the
    // line; the ramp's width softens the edge over a couple of texels.
    let across_u = wave(&mut g, "across u", u, WaveShape::Triangle, 2.0, 0.25);
    let across_v = wave(&mut g, "across v", v, WaveShape::Triangle, 2.0, 0.25);
    let line_u = remap(&mut g, "line u", across_u, 0.966, 0.974);
    let line_v = remap(&mut g, "line v", across_v, 0.966, 0.974);
    let lines = pick(&mut g, "lines", line_u, line_v, MinMaxMode::Max);

    let trowel = g.add_unique("trowel", tiling_fbm(40, 2, 5)).unwrap();
    let (shaded, bright) = noisy_pair(
        oklcha(paint.lightness.0, paint.chroma, paint.hue - 2.0, 1.0),
        oklcha(paint.lightness.1, paint.chroma, paint.hue + 2.0, 1.0),
    );
    let base = mapped(&mut g, "paint", trowel, &[(0.25, shaded), (0.75, bright)]);
    let stripe = solid(&mut g, "stripe", oklcha(paint.stripe_lightness, paint.stripe_chroma, paint.hue + 10.0, 1.0));
    let color = blend(&mut g, "color", base, stripe, lines);

    let sheen = g.add_unique("sheen", tiling_fbm(42, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.70, 0.85);
    let roughness = scaled(&mut g, "roughness", sheen, smooth, rough);
    let peel = g.add_unique("peel", tiling_fbm(41, 64, 2)).unwrap();
    let height = scaled(&mut g, "height", peel, 0.0, 0.5);
    let normal = normal_of(&mut g, height, 0.003 * NOISE);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// A staggered grid down `v`: `rows` rows, each of `columns` pieces along `u`, each row slid along
/// by its own random share of a piece, so joints in neighboring rows fall anywhere.
///
/// A piece's id is built as a brick's is ([`bricks`]), with the row's slide in place of the half
/// brick.
struct Courses {
    /// `column / columns + row / (columns * rows)`, wrapped into `[0, 1)`.
    id: LayerId,
    /// 1 on the line between two rows, falling to 0 midway across one.
    across_row: LayerId,
    /// 1 on a joint between two pieces in a row, falling to 0 midway along one.
    across_end: LayerId,
    /// 0 at a row's top edge rising to 1 at its bottom.
    into_row: LayerId,
    /// The row's top edge, wrapped into `[0, 1)`.
    row: LayerId,
    /// The piece's left end along its row, unwrapped: up to one piece past 1.
    column: LayerId,
}

/// `u` and `v` are where the grid is laid out from: the coordinates, or the coordinates bent.
/// Each must be its coordinate plus something that tiles, so a tile's far edge reads one whole
/// row or column on from its near one.
fn courses(g: &mut Graph, u: LayerId, v: LayerId, columns: u32, rows: u32, seed: u32) -> Courses {
    let (c, r) = (columns as f32, rows as f32);
    let into_row = wave(g, "into row", v, WaveShape::Sawtooth, r, 0.0);
    let row_step = scaled(g, "row step", into_row, 0.0, 1.0 / r);
    let row_start = mix(g, "row start", v, row_step, BlendMode::Subtract);
    let row = wave(g, "row", row_start, WaveShape::Sawtooth, 1.0, 0.0);
    let slide_draw = g.add_unique("slide draw", lattice(seed, rows)).unwrap();
    let row_lot = g.add_unique("row lot", LayerKind::Map(Map { value: Some(row), palette: Some(slide_draw) })).unwrap();
    let slide = scaled(g, "slide", row_lot, 0.0, 1.0 / c);
    let along = mix(g, "along", u, slide, BlendMode::Add);
    let into_piece = wave(g, "into piece", along, WaveShape::Sawtooth, c, 0.0);
    let piece_step = scaled(g, "piece step", into_piece, 0.0, 1.0 / c);
    let column = mix(g, "column", along, piece_step, BlendMode::Subtract);
    let row_share = scaled(g, "row share", row, 0.0, 1.0 / c);
    let id_sum = mix(g, "id sum", column, row_share, BlendMode::Add);
    let id = wave(g, "id", id_sum, WaveShape::Sawtooth, 1.0, 0.0);
    let across_row = wave(g, "across row", v, WaveShape::Triangle, r, 0.25);
    let across_end = wave(g, "across end", along, WaveShape::Triangle, c, 0.25);
    Courses { id, across_row, across_end, into_row, row, column }
}

/// Lines drawn out along `u`: `noise` stretched `stretch` times shorter in `v`, which alone
/// repeats `stretch` times down the tile, so each repeat is pushed aside by a broad noise of its
/// own to break the ladder.
fn streaks(g: &mut Graph, name: &str, noise: LayerKind, stretch: f32, seed: u32) -> LayerId {
    let source = g.add_unique(format!("{name} noise"), noise).unwrap();
    let long = stretched(g, &format!("{name} long"), source, [1.0, stretch]);
    let scramble = g.add_unique(format!("{name} scramble"), tiling_fbm(seed, 4, 2)).unwrap();
    shifted(g, name, long, scramble, 0.15)
}

/// A piece's own value in `[0, 1]` from a lattice of `cells` points, one per id. Up to about 256
/// cells: the lattice is baked at [`grackle::material::TILE_TEXELS`] across, and ids closer than a
/// texel or two would read the same value.
fn lot_of(g: &mut Graph, name: &str, id: LayerId, cells: u32, seed: u32) -> LayerId {
    let draw = g.add_unique(format!("{name} draw"), lattice(seed, cells)).unwrap();
    g.add_unique(name, LayerKind::Map(Map { value: Some(id), palette: Some(draw) })).unwrap()
}

/// A value in `[0, 1)` for each of any number of ids: `fract(k2 * sin(k1 * id))`, the shader
/// hash. It magnifies the id's rounding, so it suits only ids that do not wrap across a tile's
/// edge, where the two halves of one piece would compute it from ids a rounding apart.
fn hashed(g: &mut Graph, name: &str, id: LayerId, k1: f32, k2: f32) -> LayerId {
    let inner = wave(g, &format!("{name} inner"), id, WaveShape::Sine, k1, 0.0);
    wave(g, name, inner, WaveShape::Sawtooth, k2, 0.0)
}

/// `source` shifted along both axes by `by` times `amount`. The shift grows `source`'s bake domain
/// by `amount` either way and the texel count stays, so a fine source wants a small amount.
fn shifted(g: &mut Graph, name: &str, source: LayerId, by: LayerId, amount: f32) -> LayerId {
    g.add_unique(name, LayerKind::Warp(Warp { source: Some(source), by: Some(by), mode: WarpMode::Scalar, amount: [amount, amount, 0.0] }))
        .unwrap()
}

/// `fbm` with ridged octaves: thin bright lines where the noise crosses its middle.
fn ridged(seed_offset: u32, frequency: u32, octaves: u32) -> LayerKind {
    LayerKind::Noise(Noise {
        dims: NoiseDims::D2,
        seed_offset,
        frequency: frequency as f32,
        range: NoiseRange::Unsigned,
        output: NoiseOutput::Grayscale,
        kernel: NoiseKernel::Value,
        period: [frequency, frequency, 0],
        fractal: Fractal { octaves, lacunarity: 2.0, gain: 0.5, mode: FractalMode::Ridged, normalize: true },
    })
}

/// The figure of a flat-sawn board whose length runs along `u`: growth rings cut by a plane, so
/// they run as lines along the board and close into arches where the cut drifts toward the pith.
///
/// A ring is a contour of `0.6 * d² + 0.3 * drift + wobble`. `d` is the distance across the board
/// from the pith, which each board puts somewhere of its own by its `lot`, within the board or
/// beside it, so some boards run straight and some arch. `drift` is a broad noise along the board,
/// read shifted by the `lot`, so no two neighbors match.
struct Figure {
    /// 1 on each ring's dark line of late wood.
    late: LayerId,
    /// Fine noise drawn out along the board.
    fibers: LayerId,
    /// 1 in a pore: short dashes along the grain.
    pores: LayerId,
}

/// `across` runs 0 to 1 across the board.
fn flat_sawn(g: &mut Graph, across: LayerId, lot: LayerId, rings: f32, seed: u32) -> Figure {
    // `q` spans 0.35 of a board's width and is 0.5 at the pith: across the board's whole width
    // when the lot is about 0.4, off one side or the other toward 0 or 1.
    let narrow = scaled(g, "narrow across", across, 0.0, 0.35);
    let pith = scaled(g, "pith", lot, 0.0, 0.65);
    let q = mix(g, "q", narrow, pith, BlendMode::Add);
    let stops: Vec<(f32, Color)> = (0..=8)
        .map(|i| {
            let q = i as f32 / 8.0;
            (q, gray(0.6 * (2.0 * q - 1.0).powi(2)))
        })
        .collect();
    let d_squared = mapped(g, "d²", q, &stops);
    let drift_noise = g.add_unique("drift noise", tiling_fbm(seed, 2, 2)).unwrap();
    let drift_board = shifted(g, "board drift", drift_noise, lot, 1.0);
    let drift = scaled(g, "drift", drift_board, 0.0, 0.3);
    let wobble_noise = g.add_unique("wobble noise", tiling_fbm(seed + 1, 4, 3)).unwrap();
    let wobble_long = stretched(g, "wobble long", wobble_noise, [1.0, 4.0]);
    let wobble_board = shifted(g, "board wobble", wobble_long, lot, 0.25);
    let wobble = scaled(g, "wobble", wobble_board, 0.0, 0.08);
    let cut = mix(g, "cut", d_squared, drift, BlendMode::Add);
    let ring_at = mix(g, "ring at", cut, wobble, BlendMode::Add);
    let ring_wave = wave(g, "ring wave", ring_at, WaveShape::Triangle, rings, 0.0);
    let late = mapped(g, "late wood", ring_wave, &[(0.55, gray(0.0)), (0.9, gray(0.8)), (1.0, gray(1.0))]);
    let fibers = streaks(g, "fibers", tiling_fbm(seed + 2, 16, 3), 8.0, seed + 3);
    let pore_lines = streaks(g, "pore lines", tiling_fbm(seed + 4, 48, 2), 3.0, seed + 5);
    let pores = remap(g, "pores", pore_lines, 0.72, 0.80);
    Figure { late, fibers, pores }
}

/// Barn siding left out for decades: boards two meters long across `u`, 250 mm wide, butted end
/// to end at random, weathered silver with the brown still in the grooves. The soft early wood
/// has worn away and left the late wood standing, the boards have checked along the grain, the
/// knots are dark, and each nail at a stud has wept a dark stain down the board.
fn weathered_wood() -> Graph {
    const ROWS: u32 = 8;
    const STUDS: f32 = 4.0;
    let row_mm = TILE_MM / ROWS as f32;
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let (u, v) = (coordinate(&mut g, Axis::U), coordinate(&mut g, Axis::V));
    let c = courses(&mut g, u, v, 1, ROWS, 140);
    let lot = lot_of(&mut g, "lot", c.id, ROWS, 141);
    let figure = flat_sawn(&mut g, c.into_row, lot, 36.0, 142);

    let knot_spots = g.add_unique("knot spots", tiling_fbm(145, 6, 2)).unwrap();
    let knot_long = stretched(&mut g, "knot long", knot_spots, [1.0, 2.0]);
    let knots = shifted(&mut g, "board knots", knot_long, lot, 0.3);
    let knot_core = remap(&mut g, "knot core", knots, 0.80, 0.83);
    let knot_rim = remap(&mut g, "knot rim", knots, 0.68, 0.80);
    let checks_long = streaks(&mut g, "checks long", ridged(146, 8, 3), 8.0, 149);
    let checks = remap(&mut g, "checks", checks_long, 0.90, 0.97);

    let at = |mm: f32, cycle: f32| 1.0 - 2.0 * mm / cycle;
    let gap = remap(&mut g, "gap", c.across_row, at(4.0, row_mm), at(1.5, row_mm));
    let end_gap = remap(&mut g, "end gap", c.across_end, at(3.0, TILE_MM), at(1.0, TILE_MM));
    let gaps = pick(&mut g, "gaps", gap, end_gap, MinMaxMode::Max);
    let row_edge = mapped(&mut g, "row edge", c.across_row, &[(at(14.0, row_mm), gray(1.0)), (at(6.0, row_mm), gray(0.8)), (at(2.0, row_mm), gray(0.0))]);
    let end_edge = mapped(&mut g, "end edge", c.across_end, &[(at(10.0, TILE_MM), gray(1.0)), (at(4.0, TILE_MM), gray(0.8)), (at(1.0, TILE_MM), gray(0.0))]);
    let face = pick(&mut g, "face", row_edge, end_edge, MinMaxMode::Min);

    let silver = mapped(&mut g, "silver", lot, &[
        (0.0, oklcha(0.55, 0.018, 70.0, 1.0)),
        (0.4, oklcha(0.63, 0.012, 78.0, 1.0)),
        (0.8, oklcha(0.68, 0.008, 85.0, 1.0)),
        (1.0, oklcha(0.60, 0.022, 68.0, 1.0)),
    ]);
    let patches = g.add_unique("patches", tiling_fbm(147, 3, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.50, 0.020, 70.0, 1.0), oklcha(0.74, 0.006, 90.0, 1.0));
    let weather = mapped(&mut g, "weather", patches, &[(0.25, dim), (0.75, bright)]);
    let weather_share = solid(&mut g, "weather share", gray(0.35));
    let weathered = blend(&mut g, "weathered", silver, weather, weather_share);
    let fiber_share = scaled(&mut g, "fiber share", figure.fibers, 0.0, 0.35);
    let groove = solid(&mut g, "groove", oklcha(0.40, 0.025, 62.0, 1.0));
    let fibered = blend(&mut g, "fibered", weathered, groove, fiber_share);
    // The late wood stands proud and bleaches; the early wood between is grooved and keeps its brown.
    let early = remap(&mut g, "early", figure.late, 0.6, 0.0);
    let early_share = scaled(&mut g, "early share", early, 0.0, 0.30);
    let grooved = blend(&mut g, "grooved", fibered, groove, early_share);
    let bleach = solid(&mut g, "bleach", oklcha(0.76, 0.006, 90.0, 1.0));
    let ridge_share = scaled(&mut g, "ridge share", figure.late, 0.0, 0.30);
    let ridged_color = blend(&mut g, "ridged", grooved, bleach, ridge_share);
    let rim_share = scaled(&mut g, "knot rim share", knot_rim, 0.0, 0.4);
    let rimmed = blend(&mut g, "rimmed", ridged_color, groove, rim_share);
    let knot = solid(&mut g, "knot", oklcha(0.28, 0.025, 55.0, 1.0));
    let knotted = blend(&mut g, "knotted", rimmed, knot, knot_core);
    let crack = solid(&mut g, "crack", oklcha(0.18, 0.015, 60.0, 1.0));
    let cracked = blend(&mut g, "cracked", knotted, crack, checks);

    // A nail at each stud, a third of the way down each board, weeping a stain 150 mm down it.
    let stud = wave(&mut g, "stud", u, WaveShape::Triangle, STUDS, 0.25);
    let nail_row = wave(&mut g, "nail row", v, WaveShape::Triangle, ROWS as f32, 0.25 - 1.0 / 3.0);
    let half_stud = scaled(&mut g, "half stud", stud, 0.0, 0.5);
    let half_row = scaled(&mut g, "half nail row", nail_row, 0.0, 0.5);
    let nail_sum = mix(&mut g, "nail sum", half_stud, half_row, BlendMode::Add);
    // In the sum, a stud cycle of 500 mm counts for half: `mm` off the stud line reads `mm / 500`.
    let nail = remap(&mut g, "nail", nail_sum, 1.0 - 6.0 / 500.0, 1.0 - 4.0 / 500.0);
    let below = wave(&mut g, "below nail", v, WaveShape::Sawtooth, ROWS as f32, -1.0 / 3.0);
    let run_down = remap(&mut g, "run down", below, 0.6, 0.0);
    let run_width = remap(&mut g, "run width", stud, 1.0 - 2.0 * 12.0 / 500.0, 1.0 - 2.0 * 3.0 / 500.0);
    let run = product(&mut g, "run", zero, run_width, run_down);
    let weep_noise = g.add_unique("weep noise", tiling_fbm(148, 32, 3)).unwrap();
    let blotchy = scaled(&mut g, "blotchy", weep_noise, 0.2, 0.7);
    let weep_share = product(&mut g, "weep share", zero, run, blotchy);
    let weep = solid(&mut g, "weep", oklcha(0.30, 0.020, 55.0, 1.0));
    let wept = blend(&mut g, "wept", cracked, weep, weep_share);
    let iron = solid(&mut g, "iron", oklcha(0.24, 0.010, 50.0, 1.0));
    let nailed = blend(&mut g, "nailed", wept, iron, nail);
    let shadow = solid(&mut g, "shadow", oklcha(0.12, 0.010, 60.0, 1.0));
    let color = blend(&mut g, "color", nailed, shadow, gaps);

    let rough_wood = scaled(&mut g, "wood roughness", figure.fibers, 0.90, 0.97);
    let rough = solid(&mut g, "rough", gray(0.98));
    let roughness = blend(&mut g, "roughness", rough_wood, rough, gaps);

    let base = solid(&mut g, "base", gray(0.45));
    let warp = scaled(&mut g, "warp", lot, 0.0, 0.10);
    let offset = mix(&mut g, "offset", base, warp, BlendMode::Add);
    let raised = scaled(&mut g, "raised grain", figure.late, 0.0, 0.25);
    let grained = mix(&mut g, "grained", offset, raised, BlendMode::Add);
    let fuzz = scaled(&mut g, "fuzz", figure.fibers, 0.0, 0.12);
    let fuzzy = mix(&mut g, "fuzzy", grained, fuzz, BlendMode::Add);
    let check_depth = scaled(&mut g, "check depth", checks, 0.0, 0.35);
    let checked = mix(&mut g, "checked", fuzzy, check_depth, BlendMode::Subtract);
    let knot_rise = scaled(&mut g, "knot rise", knot_core, 0.0, 0.08);
    let knot_top = mix(&mut g, "knot top", checked, knot_rise, BlendMode::Add);
    let nail_dip = scaled(&mut g, "nail dip", nail, 0.0, 0.15);
    let board_top = mix(&mut g, "board top", knot_top, nail_dip, BlendMode::Subtract);
    let height = blend(&mut g, "height", zero, board_top, face);
    let normal = normal_of(&mut g, height, 0.005);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Decades of foot traffic on a hardwood floor, drawn from its own noises.
#[derive(Clone, Copy)]
struct Wear {
    seed: u32,
}

/// Red oak strip flooring under satin polyurethane: 83 mm boards, a meter long, laid across `u` with
/// the joints staggered at random, each board its own shade, the open-pored figure flat-sawn,
/// eased edges between boards. `None` is fresh from the sander and its last coat.
///
/// Worn, it has been walked on for decades without a refinish: the finish is gone to bare, grayed
/// wood along the traffic, scratched every which way, dented by heels, and grime has packed into
/// the joints and the pores.
fn hardwood(wear: Option<Wear>) -> Graph {
    const ROWS: u32 = 24;
    const COLUMNS: u32 = 2;
    let row_mm = TILE_MM / ROWS as f32;
    let end_mm = TILE_MM / COLUMNS as f32;
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let (u, v) = (coordinate(&mut g, Axis::U), coordinate(&mut g, Axis::V));
    let c = courses(&mut g, u, v, COLUMNS, ROWS, 150);
    let lot = lot_of(&mut g, "lot", c.id, COLUMNS * ROWS, 151);
    let figure = flat_sawn(&mut g, c.into_row, lot, 22.0, 152);

    let at = |mm: f32, cycle: f32| 1.0 - 2.0 * mm / cycle;
    let row_joint = remap(&mut g, "row joint", c.across_row, at(1.5, row_mm), at(0.5, row_mm));
    let end_joint = remap(&mut g, "end joint", c.across_end, at(1.5, end_mm), at(0.5, end_mm));
    let joint = pick(&mut g, "joint", row_joint, end_joint, MinMaxMode::Max);
    let row_ease = mapped(&mut g, "row ease", c.across_row, &[(at(5.0, row_mm), gray(1.0)), (at(2.5, row_mm), gray(0.7)), (at(1.0, row_mm), gray(0.0))]);
    let end_ease = mapped(&mut g, "end ease", c.across_end, &[(at(5.0, end_mm), gray(1.0)), (at(2.5, end_mm), gray(0.7)), (at(1.0, end_mm), gray(0.0))]);
    let face = pick(&mut g, "face", row_ease, end_ease, MinMaxMode::Min);

    let oak = mapped(&mut g, "oak", lot, &[
        (0.0, oklcha(0.70, 0.070, 74.0, 1.0)),
        (0.3, oklcha(0.64, 0.085, 64.0, 1.0)),
        (0.6, oklcha(0.60, 0.090, 55.0, 1.0)),
        (0.85, oklcha(0.66, 0.080, 68.0, 1.0)),
        (1.0, oklcha(0.53, 0.080, 52.0, 1.0)),
    ]);
    let late = solid(&mut g, "late", oklcha(0.44, 0.085, 50.0, 1.0));
    let fiber_share = scaled(&mut g, "fiber share", figure.fibers, 0.0, 0.12);
    let fibered = blend(&mut g, "fibered", oak, late, fiber_share);
    let ring_share = scaled(&mut g, "ring share", figure.late, 0.0, 0.50);
    let grained = blend(&mut g, "grained", fibered, late, ring_share);
    // Oak's pores open in the late wood: dark flecks along the rings.
    let late_pores = product(&mut g, "late pores", zero, figure.pores, figure.late);
    let pore = solid(&mut g, "pore", oklcha(0.34, 0.060, 48.0, 1.0));
    let pore_share = scaled(&mut g, "pore share", late_pores, 0.0, 0.6);
    let pored = blend(&mut g, "pored", grained, pore, pore_share);
    let joint_color = solid(&mut g, "joint color", oklcha(0.30, 0.050, 50.0, 1.0));
    let eased = remap(&mut g, "eased", face, 1.0, 0.0);
    let ease_share = scaled(&mut g, "ease share", eased, 0.0, 0.35);
    let edged = blend(&mut g, "edged", pored, joint_color, ease_share);
    let wood = blend(&mut g, "wood", edged, joint_color, joint);

    let (gloss, satin) = noisy(0.26, 0.36);
    let sheen = g.add_unique("sheen", tiling_fbm(153, 4, 3)).unwrap();
    let finish_rough = scaled(&mut g, "finish roughness", sheen, gloss, satin);
    let joint_rough = solid(&mut g, "joint roughness", gray(0.7));
    let fresh_rough = blend(&mut g, "fresh roughness", finish_rough, joint_rough, joint);

    let base = solid(&mut g, "base", gray(0.5));
    let lippage = scaled(&mut g, "lippage", lot, 0.0, 0.06);
    let laid = mix(&mut g, "laid", base, lippage, BlendMode::Add);
    let ring_relief = scaled(&mut g, "ring relief", figure.late, 0.0, 0.03);
    let pore_relief = scaled(&mut g, "pore relief", late_pores, 0.0, 0.08);
    let ringed = mix(&mut g, "ringed", laid, ring_relief, BlendMode::Add);
    let fresh_top = mix(&mut g, "fresh top", ringed, pore_relief, BlendMode::Subtract);

    let (color, roughness, top) = match wear {
        None => (wood, fresh_rough, fresh_top),
        Some(wear) => {
            let s = wear.seed;
            // Where feet go: broad, uneven paths over most of the floor, broken up by grit, with
            // the corners of the tile left closer to new.
            let traffic_noise = g.add_unique("traffic noise", tiling_fbm(s, 2, 4)).unwrap();
            let traffic = remap(&mut g, "traffic", traffic_noise, 0.25, 0.75);
            let scuff_noise = g.add_unique("scuff noise", tiling_fbm(s + 1, 64, 3)).unwrap();
            let scuff_spread = scaled(&mut g, "scuff spread", scuff_noise, 0.5, 1.0);
            let worn = product(&mut g, "worn", zero, traffic, scuff_spread);

            // Scratches along, across and diagonally. A single ridged octave is 1 exactly on the
            // noise's middle contour, so it draws a thin line; a mask keeps a scatter of them.
            let scratch_set = |g: &mut Graph, name: &str, lines: LayerId, seed: u32| {
                let crisp = remap(g, &format!("{name} crisp"), lines, 0.93, 0.99);
                let mask_noise = g.add_unique(format!("{name} mask noise"), tiling_fbm(seed, 8, 3)).unwrap();
                let mask = remap(g, &format!("{name} mask"), mask_noise, 0.55, 0.68);
                product(g, name, zero, crisp, mask)
            };
            let along_lines = streaks(&mut g, "along lines", ridged(s + 2, 16, 1), 6.0, s + 7);
            let along = scratch_set(&mut g, "along scratches", along_lines, s + 9);
            let across_noise = g.add_unique("across noise", ridged(s + 3, 16, 1)).unwrap();
            let across_long = stretched(&mut g, "across long", across_noise, [6.0, 1.0]);
            let across_scramble = g.add_unique("across scramble", tiling_fbm(s + 8, 4, 2)).unwrap();
            let across_lines = shifted(&mut g, "across lines", across_long, across_scramble, 0.15);
            let across = scratch_set(&mut g, "across scratches", across_lines, s + 10);
            let slant_noise = g.add_unique("slant noise", ridged(s + 4, 12, 1)).unwrap();
            // A quarter turn's half, scaled by √2: the turned lattice lands on whole cells, so it
            // still tiles.
            let slant_lines = g
                .add_unique(
                    "slant lines",
                    LayerKind::Transform(Transform {
                        source: Some(slant_noise),
                        offset: [0.0; 3],
                        rotate_uv: std::f32::consts::FRAC_PI_4,
                        scale: [std::f32::consts::SQRT_2, 4.0 * std::f32::consts::SQRT_2, 1.0],
                        coord_mode: CoordMode::Passthrough,
                        edge_mode: EdgeMode::Extend,
                    }),
                )
                .unwrap();
            let slant = scratch_set(&mut g, "slant scratches", slant_lines, s + 11);
            let cross = pick(&mut g, "cross", along, across, MinMaxMode::Max);
            let lines = pick(&mut g, "lines", cross, slant, MinMaxMode::Max);
            let reach = scaled(&mut g, "reach", traffic, 0.5, 1.0);
            let scratches = product(&mut g, "scratches", zero, lines, reach);
            let dent_noise = g.add_unique("dent noise", tiling_fbm(s + 5, 128, 1)).unwrap();
            let dent_spots = remap(&mut g, "dent spots", dent_noise, 0.86, 0.92);
            let dents = product(&mut g, "dents", zero, dent_spots, traffic);

            // With the amber finish gone the wood is duller and grayer, and dirt is ground in.
            let bare = solid(&mut g, "bare", oklcha(0.56, 0.045, 66.0, 1.0));
            let bare_share = scaled(&mut g, "bare share", worn, 0.0, 0.65);
            let stripped = blend(&mut g, "stripped", wood, bare, bare_share);
            let dirt_noise = g.add_unique("dirt noise", tiling_fbm(s + 6, 8, 4)).unwrap();
            let (dark_dirt, light_dirt) = noisy_pair(oklcha(0.32, 0.030, 62.0, 1.0), oklcha(0.46, 0.035, 66.0, 1.0));
            let dirt = mapped(&mut g, "dirt", dirt_noise, &[(0.25, dark_dirt), (0.75, light_dirt)]);
            let grime_share = scaled(&mut g, "grime share", worn, 0.0, 0.25);
            let grimed = blend(&mut g, "grimed", stripped, dirt, grime_share);
            let packed_grain = product(&mut g, "packed grain", zero, late_pores, traffic);
            let packed_share = scaled(&mut g, "packed share", packed_grain, 0.0, 0.5);
            let packed = blend(&mut g, "packed", grimed, dirt, packed_share);
            let gunk = solid(&mut g, "gunk", oklcha(0.22, 0.020, 60.0, 1.0));
            let edge_grime = scaled(&mut g, "edge grime", eased, 0.0, 0.6);
            let edge_dirty = blend(&mut g, "edge dirty", packed, gunk, edge_grime);
            let jointed = blend(&mut g, "jointed", edge_dirty, gunk, joint);
            // A scratch through the finish shows as a pale line.
            let scratch_white = solid(&mut g, "scratch white", oklcha(0.80, 0.030, 76.0, 1.0));
            let scratch_share = scaled(&mut g, "scratch share", scratches, 0.0, 0.75);
            let scratched = blend(&mut g, "scratched", jointed, scratch_white, scratch_share);
            let dent_share = scaled(&mut g, "dent share", dents, 0.0, 0.5);
            let color = blend(&mut g, "color", scratched, gunk, dent_share);

            let bare_rough = solid(&mut g, "bare roughness", gray(0.72));
            let worn_rough = blend(&mut g, "worn roughness", fresh_rough, bare_rough, worn);
            let scratch_rough = solid(&mut g, "scratch roughness", gray(0.6));
            let roughness = blend(&mut g, "roughness", worn_rough, scratch_rough, scratches);

            let scratch_depth = scaled(&mut g, "scratch depth", scratches, 0.0, 0.10);
            let dent_depth = scaled(&mut g, "dent depth", dents, 0.0, 0.25);
            let raised = scaled(&mut g, "worn grain", figure.late, 0.0, 0.05);
            let raised_worn = product(&mut g, "raised worn", zero, raised, worn);
            let scratched_top = mix(&mut g, "scratched top", fresh_top, scratch_depth, BlendMode::Subtract);
            let dented = mix(&mut g, "dented", scratched_top, dent_depth, BlendMode::Subtract);
            let top = mix(&mut g, "worn top", dented, raised_worn, BlendMode::Add);
            (color, roughness, top)
        }
    };
    let height = blend(&mut g, "height", zero, top, face);
    let normal = normal_of(&mut g, height, 0.0025);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// 1970s wood-grain laminate, as on a desk or a stereo cabinet: a printed walnut in flitches
/// 250 mm wide across `v`, orange-brown and darker than the real thing, cathedral figure, on a flat
/// semi-gloss plastic with only a faint embossed pore. No joints: one sheet covers a desk.
fn laminate() -> Graph {
    const FLITCHES: u32 = 8;
    let mut g = empty();
    let v = coordinate(&mut g, Axis::V);
    let into_flitch = wave(&mut g, "into flitch", v, WaveShape::Sawtooth, FLITCHES as f32, 0.0);
    let flitch_step = scaled(&mut g, "flitch step", into_flitch, 0.0, 1.0 / FLITCHES as f32);
    let flitch_start = mix(&mut g, "flitch start", v, flitch_step, BlendMode::Subtract);
    let flitch = wave(&mut g, "flitch", flitch_start, WaveShape::Sawtooth, 1.0, 0.0);
    // Hashed, not drawn from a lattice: eight lattice values can bunch, and flitches with near
    // lots share a figure, lined up across the sheet.
    let lot = hashed(&mut g, "lot", flitch, 71.93, 389.17);
    let figure = flat_sawn(&mut g, into_flitch, lot, 40.0, 161);

    let walnut = mapped(&mut g, "walnut", lot, &[
        (0.0, oklcha(0.50, 0.085, 52.0, 1.0)),
        (0.5, oklcha(0.55, 0.095, 58.0, 1.0)),
        (1.0, oklcha(0.47, 0.080, 50.0, 1.0)),
    ]);
    let streak_long = streaks(&mut g, "streak long", tiling_fbm(162, 4, 4), 4.0, 165);
    let (light, dark) = noisy_pair(oklcha(0.62, 0.100, 64.0, 1.0), oklcha(0.40, 0.070, 45.0, 1.0));
    let streaks = mapped(&mut g, "streaks", streak_long, &[(0.25, light), (0.75, dark)]);
    let streak_share = solid(&mut g, "streak share", gray(0.5));
    let streaked = blend(&mut g, "streaked", walnut, streaks, streak_share);
    let late = solid(&mut g, "late", oklcha(0.32, 0.060, 42.0, 1.0));
    let ring_share = scaled(&mut g, "ring share", figure.late, 0.0, 0.70);
    let grained = blend(&mut g, "grained", streaked, late, ring_share);
    let fiber_share = scaled(&mut g, "fiber share", figure.fibers, 0.0, 0.25);
    let fibered = blend(&mut g, "fibered", grained, late, fiber_share);
    // The printed pores: a dot screen of dark dashes along the grain.
    let pores = figure.pores;
    let pore = solid(&mut g, "pore", oklcha(0.30, 0.055, 42.0, 1.0));
    let pore_share = scaled(&mut g, "pore share", pores, 0.0, 0.3);
    let color = blend(&mut g, "color", fibered, pore, pore_share);

    let sheen = g.add_unique("sheen", tiling_fbm(163, 4, 3)).unwrap();
    let (gloss, satin) = noisy(0.30, 0.38);
    let roughness = scaled(&mut g, "roughness", sheen, gloss, satin);
    let peel = g.add_unique("peel", tiling_fbm(164, 64, 2)).unwrap();
    let fine = scaled(&mut g, "fine", peel, 0.0, 0.15);
    let emboss = scaled(&mut g, "emboss", pores, 0.0, 0.25);
    let height = mix(&mut g, "height", fine, emboss, BlendMode::Subtract);
    let normal = normal_of(&mut g, height, 0.0015);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// White speckled vinyl composition tile, seven to a two-meter tile, so 286 mm, about 11¼": an off
/// white base flecked with gray, charcoal, beige and black chips in drifts, each tile a shade off
/// its neighbors, tight butt joints with dirt in them, and a waxed finish scuffed dull in places.
fn linoleum() -> Graph {
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let VinylGrid { lot, joint, edge, .. } = vinyl_grid(&mut g, 7, 170);

    let base = mapped(&mut g, "base", lot, &[
        (0.0, oklcha(0.88, 0.014, 88.0, 1.0)),
        (1.0, oklcha(0.92, 0.010, 92.0, 1.0)),
    ]);
    let cloud = g.add_unique("cloud", tiling_fbm(171, 8, 4)).unwrap();
    let cloud_tile = shifted(&mut g, "tile cloud", cloud, lot, 0.25);
    let (cloud_dim, cloud_bright) = noisy_pair(oklcha(0.84, 0.014, 85.0, 1.0), oklcha(0.94, 0.008, 95.0, 1.0));
    let clouded = mapped(&mut g, "clouded", cloud_tile, &[(0.25, cloud_dim), (0.75, cloud_bright)]);
    let cloud_share = solid(&mut g, "cloud share", gray(0.5));
    let mottled = blend(&mut g, "mottled", base, clouded, cloud_share);
    // Chips gather in drifts: the drift raises or lowers every chip's threshold.
    let drift_noise = g.add_unique("drift", tiling_fbm(172, 16, 3)).unwrap();
    let drift_tile = shifted(&mut g, "tile drift", drift_noise, lot, 0.2);
    let drift = scaled(&mut g, "drift amount", drift_tile, 0.0, 0.10);
    let chip = |g: &mut Graph, name: &str, seed: u32, frequency: u32, lo: f32, hi: f32| {
        let noise = g.add_unique(format!("{name} noise"), tiling_fbm(seed, frequency, 2)).unwrap();
        let tile_noise = shifted(g, &format!("tile {name}"), noise, lot, 0.05);
        let drifted = mix(g, &format!("drifted {name}"), tile_noise, drift, BlendMode::Add);
        remap(g, name, drifted, lo, hi)
    };
    let gray_chips = chip(&mut g, "gray chips", 173, 96, 0.84, 0.88);
    let beige_chips = chip(&mut g, "beige chips", 174, 96, 0.85, 0.89);
    let charcoal_chips = chip(&mut g, "charcoal chips", 175, 160, 0.89, 0.92);
    let black_chips = chip(&mut g, "black chips", 176, 192, 0.91, 0.94);
    let gray_chip = solid(&mut g, "gray chip", oklcha(0.68, 0.006, 250.0, 1.0));
    let beige_chip = solid(&mut g, "beige chip", oklcha(0.78, 0.035, 75.0, 1.0));
    let charcoal_chip = solid(&mut g, "charcoal chip", oklcha(0.45, 0.006, 250.0, 1.0));
    let black_chip = solid(&mut g, "black chip", oklcha(0.24, 0.004, 250.0, 1.0));
    let grayed = blend(&mut g, "grayed", mottled, gray_chip, gray_chips);
    let beiged = blend(&mut g, "beiged", grayed, beige_chip, beige_chips);
    let charcoaled = blend(&mut g, "charcoaled", beiged, charcoal_chip, charcoal_chips);
    let flecked = blend(&mut g, "flecked", charcoaled, black_chip, black_chips);
    let grime = solid(&mut g, "grime", oklcha(0.62, 0.030, 80.0, 1.0));
    let edge_grime = scaled(&mut g, "edge grime", edge, 0.0, 0.25);
    let edged = blend(&mut g, "edged", flecked, grime, edge_grime);
    let gap = solid(&mut g, "gap", oklcha(0.40, 0.020, 75.0, 1.0));
    let joint_share = scaled(&mut g, "joint share", joint, 0.0, 0.55);
    let color = blend(&mut g, "color", edged, gap, joint_share);

    let scuff_noise = g.add_unique("scuff noise", tiling_fbm(177, 4, 5)).unwrap();
    let (waxed, dull) = noisy(0.25, 0.55);
    let wax = scaled(&mut g, "wax", scuff_noise, waxed, dull);
    let joint_rough = solid(&mut g, "joint roughness", gray(0.8));
    let roughness = blend(&mut g, "roughness", wax, joint_rough, joint);

    let ripple = g.add_unique("ripple", tiling_fbm(178, 32, 2)).unwrap();
    let waves = scaled(&mut g, "waves", ripple, 0.0, 0.04);
    let curl = scaled(&mut g, "curl", lot, 0.75, 0.80);
    let top = mix(&mut g, "top", curl, waves, BlendMode::Add);
    let face = remap(&mut g, "face", edge, 0.6, 0.0);
    let height = blend(&mut g, "height", zero, top, face);
    let normal = normal_of(&mut g, height, 0.001);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Square vinyl tiles laid square across the texture tile.
struct VinylGrid {
    u: LayerId,
    v: LayerId,
    /// Each tile's own value in `[0, 1)`.
    lot: LayerId,
    /// 1 in a butt joint, softened over a texel either side.
    joint: LayerId,
    /// 1 within a few millimeters of a joint, where dirt gathers and the tile's edge turns down.
    edge: LayerId,
}

fn vinyl_grid(g: &mut Graph, tiles: u32, seed: u32) -> VinylGrid {
    let count = tiles as f32;
    let cycle = TILE_MM / count;
    let u = coordinate(g, Axis::U);
    let v = coordinate(g, Axis::V);
    let into_column = wave(g, "into column", u, WaveShape::Sawtooth, count, 0.0);
    let column_step = scaled(g, "column step", into_column, 0.0, 1.0 / count);
    let column = mix(g, "column", u, column_step, BlendMode::Subtract);
    let into_row = wave(g, "into row", v, WaveShape::Sawtooth, count, 0.0);
    let row_step = scaled(g, "row step", into_row, 0.0, 1.0 / count);
    let row_start = mix(g, "row start", v, row_step, BlendMode::Subtract);
    let row = wave(g, "row", row_start, WaveShape::Sawtooth, 1.0, 0.0);
    let row_share = scaled(g, "row share", row, 0.0, 1.0 / count);
    let id_sum = mix(g, "id sum", column, row_share, BlendMode::Add);
    let id = wave(g, "id", id_sum, WaveShape::Sawtooth, 1.0, 0.0);
    let lot = lot_of(g, "lot", id, tiles * tiles, seed);

    let at = |mm: f32| 1.0 - 2.0 * mm / cycle;
    let across_u = wave(g, "across u", u, WaveShape::Triangle, count, 0.25);
    let across_v = wave(g, "across v", v, WaveShape::Triangle, count, 0.25);
    // A butt joint is under a millimeter; drawn that thin it would catch a texel on one seam and
    // miss the next, so it is spread soft over a texel either side.
    let joint_u = remap(g, "joint u", across_u, at(4.0), at(0.0));
    let joint_v = remap(g, "joint v", across_v, at(4.0), at(0.0));
    let joint = pick(g, "joint", joint_u, joint_v, MinMaxMode::Max);
    let edge_u = remap(g, "edge u", across_u, at(8.0), at(1.0));
    let edge_v = remap(g, "edge v", across_v, at(8.0), at(1.0));
    let edge = pick(g, "edge", edge_u, edge_v, MinMaxMode::Max);
    VinylGrid { u, v, lot, joint, edge }
}

/// Vinyl composition tile in two close shades of one color laid as a checkerboard, `tiles` to a
/// two-meter tile (6 is 333 mm, about 13"; 4 is 500 mm, about 20"): each tile clouded and finely
/// flecked with lighter and darker chips of its own color, a shade off its neighbors, with dirt in
/// the joints and a waxed finish scuffed dull in places. `light` is the lighter or more saturated
/// of the two.
fn checkered_linoleum(tiles: u32, (light, dark): (Coat, Coat)) -> Graph {
    assert!(tiles % 2 == 0, "an odd count puts two tiles of one shade side by side at the texture's edge");
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let VinylGrid { u, v, lot, joint, edge } = vinyl_grid(&mut g, tiles, 180);

    // Square waves of half the tile frequency are 1 on alternate columns and rows; their
    // exclusive or, a + b - 2ab, is 1 on alternate tiles.
    let half = tiles as f32 / 2.0;
    let odd_column = wave(&mut g, "odd column", u, WaveShape::Square, half, 0.0);
    let odd_row = wave(&mut g, "odd row", v, WaveShape::Square, half, 0.0);
    let either = mix(&mut g, "either", odd_column, odd_row, BlendMode::Add);
    let both = mix(&mut g, "both", odd_column, odd_row, BlendMode::Multiply);
    let once = mix(&mut g, "once", either, both, BlendMode::Subtract);
    let checker = mix(&mut g, "checker", once, both, BlendMode::Subtract);

    let shade = |coat: Coat, dl: f32| oklcha(coat.lightness + dl, coat.chroma, coat.hue, 1.0);
    let cloud = g.add_unique("cloud", tiling_fbm(181, 8, 4)).unwrap();
    let cloud_tile = shifted(&mut g, "tile cloud", cloud, lot, 0.25);
    let cloud_share = solid(&mut g, "cloud share", gray(0.5));
    let mottled_shade = |g: &mut Graph, name: &str, coat: Coat| {
        let tile = mapped(g, &format!("{name} tile"), lot, &[(0.0, shade(coat, -0.015)), (1.0, shade(coat, 0.015))]);
        let (dim, bright) = noisy_pair(shade(coat, -0.05), shade(coat, 0.05));
        let clouded = mapped(g, &format!("{name} clouded"), cloud_tile, &[(0.25, dim), (0.75, bright)]);
        blend(g, &format!("{name} mottled"), tile, clouded, cloud_share)
    };
    let dark_mottled = mottled_shade(&mut g, "dark", dark);
    let light_mottled = mottled_shade(&mut g, "light", light);
    let mottled = blend(&mut g, "mottled", dark_mottled, light_mottled, checker);
    let drift_noise = g.add_unique("drift", tiling_fbm(182, 16, 3)).unwrap();
    let drift_tile = shifted(&mut g, "tile drift", drift_noise, lot, 0.2);
    let drift = scaled(&mut g, "drift amount", drift_tile, 0.0, 0.10);
    let chip = |g: &mut Graph, name: &str, seed: u32, frequency: u32, lo: f32, hi: f32| {
        let noise = g.add_unique(format!("{name} noise"), tiling_fbm(seed, frequency, 2)).unwrap();
        let tile_noise = shifted(g, &format!("tile {name}"), noise, lot, 0.05);
        let drifted = mix(g, &format!("drifted {name}"), tile_noise, drift, BlendMode::Add);
        remap(g, name, drifted, lo, hi)
    };
    let pale_chips = chip(&mut g, "pale chips", 183, 128, 0.86, 0.90);
    let deep_chips = chip(&mut g, "deep chips", 184, 160, 0.88, 0.92);
    // Halfway to white rather than a fixed step lighter: on a tile already near white, a step
    // past L = 1 converts to sRGB as pink.
    let pale = oklcha(light.lightness + (1.0 - light.lightness) * 0.5, light.chroma * 0.5, light.hue, 1.0);
    let pale_chip = solid(&mut g, "pale chip", pale);
    let deep_chip = solid(&mut g, "deep chip", shade(dark, -0.14));
    let paled = blend(&mut g, "paled", mottled, pale_chip, pale_chips);
    let flecked = blend(&mut g, "flecked", paled, deep_chip, deep_chips);
    let grime = solid(&mut g, "grime", oklcha(dark.lightness * 0.75, 0.030, 80.0, 1.0));
    let edge_grime = scaled(&mut g, "edge grime", edge, 0.0, 0.25);
    let edged = blend(&mut g, "edged", flecked, grime, edge_grime);
    let gap = solid(&mut g, "gap", oklcha(0.35, 0.020, 75.0, 1.0));
    let joint_share = scaled(&mut g, "joint share", joint, 0.0, 0.55);
    let color = blend(&mut g, "color", edged, gap, joint_share);

    let scuff_noise = g.add_unique("scuff noise", tiling_fbm(187, 4, 5)).unwrap();
    let (waxed, dull) = noisy(0.25, 0.55);
    let wax = scaled(&mut g, "wax", scuff_noise, waxed, dull);
    let joint_rough = solid(&mut g, "joint roughness", gray(0.8));
    let roughness = blend(&mut g, "roughness", wax, joint_rough, joint);

    let ripple = g.add_unique("ripple", tiling_fbm(188, 32, 2)).unwrap();
    let waves = scaled(&mut g, "waves", ripple, 0.0, 0.04);
    let curl = scaled(&mut g, "curl", lot, 0.75, 0.80);
    let top = mix(&mut g, "top", curl, waves, BlendMode::Add);
    let face = remap(&mut g, "face", edge, 0.6, 0.0);
    let height = blend(&mut g, "height", zero, top, face);
    let normal = normal_of(&mut g, height, 0.001);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// One layer of an OSB's strands: a staggered grid of flakes along `u`, each a share of the
/// layer's cells, with ragged edges.
struct Strands {
    /// 1 where this layer's flake lies, 0 where the layer below shows.
    cover: LayerId,
    /// The flake's own value in `[0, 1)`.
    lot: LayerId,
    /// A rim a few millimeters wide round each flake, where it shades the layer below.
    halo: LayerId,
}

/// `a * x + b * y` for whole `a` and `b`, over a base that keeps it positive. Add and Subtract are
/// unclamped, and a whole multiple of a coordinate plus a constant is still something a grid of
/// whole frequencies tiles over.
fn combination(g: &mut Graph, name: &str, x: LayerId, y: LayerId, a: i32, b: i32) -> LayerId {
    let base = 2.0 * (a.min(0).abs() + b.min(0).abs()) as f32;
    let mut sum = solid(g, &format!("{name} base"), gray(base));
    for (term, k) in [(x, a), (y, b)] {
        let mode = if k > 0 { BlendMode::Add } else { BlendMode::Subtract };
        for _ in 0..k.abs() {
            sum = mix(g, name, sum, term, mode);
        }
    }
    sum
}

/// Which way a layer's strands lie: along `(a, b)` in `(u, v)`. Whole numbers, so the turned grid
/// still meets itself at the tile's edges; `(1, 0)` is along `u`.
#[derive(Clone, Copy)]
struct Lie {
    a: i32,
    b: i32,
    /// How far a flake's ends and sides are torn, in mm, along `u` and `v` before turning.
    rag_mm: (f32, f32),
}

impl Lie {
    const ALONG: Lie = Lie { a: 1, b: 0, rag_mm: (30.0, 6.0) };

    /// How many times larger the turned grid's coordinates run than the tile's.
    fn stretch(self) -> f32 {
        ((self.a * self.a + self.b * self.b) as f32).sqrt()
    }
}

fn strands(g: &mut Graph, columns: u32, rows: u32, seed: u32, v_offset: f32, share: f32, lie: Lie) -> Strands {
    // A turned grid runs `stretch` times faster, so a cycle is that much shorter on the board.
    let (c, r) = (columns as f32 * lie.stretch(), rows as f32 * lie.stretch());
    // Strands lie only roughly along `u` and their edges are torn: the grid is laid out from
    // coordinates pushed about by a broad noise, which bends the rows, and a fine one, which
    // rags the edges. Added rather than warped, so nothing downstream is a warp of a warp.
    let push = |g: &mut Graph, name: &str, axis: Axis, offset: f32, bend_mm: f32, rag_mm: f32| {
        let at = coordinate(g, axis);
        let bend_noise = g.add_unique(format!("{name} bend noise"), tiling_fbm(seed + 2 + axis as u32, 4, 2)).unwrap();
        let bend = scaled(g, &format!("{name} bend"), bend_noise, offset, offset + bend_mm / TILE_MM);
        let rag_noise = g.add_unique(format!("{name} rag noise"), tiling_fbm(seed + 4 + axis as u32, 64, 2)).unwrap();
        let rag = scaled(g, &format!("{name} rag"), rag_noise, 0.0, rag_mm / TILE_MM);
        let bent = mix(g, &format!("{name} bent"), at, bend, BlendMode::Add);
        mix(g, name, bent, rag, BlendMode::Add)
    };
    let u = push(g, "strand u", Axis::U, 0.0, 120.0, lie.rag_mm.0);
    let v = push(g, "strand v", Axis::V, v_offset, 100.0, lie.rag_mm.1);
    // Turned so the grid's rows run along `(a, b)`: `u'` rises along it and `v'` across it.
    let (u, v) = if (lie.a, lie.b) == (1, 0) {
        (u, v)
    } else {
        (combination(g, "turned u", u, v, lie.a, lie.b), combination(g, "turned v", u, v, -lie.b, lie.a))
    };
    let k = courses(g, u, v, columns, rows, seed);
    // A flake's own values, one per draw: a row's value plus a column's, wrapped. Too many flakes
    // for one lattice ([`lot_of`]), and a hash of the id would tell a flake's two halves at the
    // tile's edge apart by the id's rounding. A lattice read a rounding off reads the same.
    let column = wave(g, "flake column", k.column, WaveShape::Sawtooth, 1.0, 0.0);
    let draw = |g: &mut Graph, name: &str, draw_seed: u32| {
        let by_row = lot_of(g, &format!("{name} by row"), k.row, rows, draw_seed);
        let by_column = lot_of(g, &format!("{name} by column"), column, columns, draw_seed + 1);
        let sum = mix(g, &format!("{name} sum"), by_row, by_column, BlendMode::Add);
        wave(g, name, sum, WaveShape::Sawtooth, 1.0, 0.0)
    };
    let lot = draw(g, "flake lot", seed + 10);
    let presence = draw(g, "flake presence", seed + 12);
    let present = remap(g, "present", presence, share + 0.01, share - 0.01);
    // Each flake falls short of its cell by its own share, up to 40% of its length and half its
    // width, so no two are the same size. Pushing the distance to the edge up moves the edge in.
    let length_draw = draw(g, "flake length", seed + 14);
    let width_draw = draw(g, "flake width", seed + 16);
    let shorter = scaled(g, "shorter", length_draw, 0.0, 0.4);
    let narrower = scaled(g, "narrower", width_draw, 0.0, 0.5);
    let across_end = mix(g, "flake across end", k.across_end, shorter, BlendMode::Add);
    let across_row = mix(g, "flake across row", k.across_row, narrower, BlendMode::Add);
    // A flake fills what is left short of a gap at its edges and ends. Lengths in mm.
    let at = |mm: f32, cycle: f32| 1.0 - 2.0 * mm / cycle;
    let edge = |g: &mut Graph, name: &str, side_mm: (f32, f32), end_mm: (f32, f32)| {
        let side = remap(g, &format!("{name} side"), across_row, at(side_mm.0, TILE_MM / r), at(side_mm.1, TILE_MM / r));
        let end = remap(g, &format!("{name} end"), across_end, at(end_mm.0, TILE_MM / c), at(end_mm.1, TILE_MM / c));
        let outside = pick(g, &format!("{name} outside"), side, end, MinMaxMode::Max);
        remap(g, name, outside, 1.0, 0.0)
    };
    let inside = edge(g, "inside", (4.0, 1.5), (14.0, 4.0));
    let wide = edge(g, "wide", (1.5, 0.0), (6.0, 0.0));
    let zero = solid(g, "zero", gray(0.0));
    let cover = product(g, "cover", zero, inside, present);
    let wide_cover = product(g, "wide cover", zero, wide, present);
    let halo = mix(g, "halo", wide_cover, cover, BlendMode::Subtract);
    Strands { cover, lot, halo }
}

/// Oriented strand board: five layers of pine and aspen strands, up to 220 mm long and 50 mm wide,
/// lying mostly along `u` but for a sparse third layer turned every which way, the upper ones
/// covering part of the lower and shading it at their edges, every flake its own size and its own
/// tone of straw or honey, dark gaps where none lies flat, a fiber grain inside each, and the waxy
/// resin sheen of a pressed panel.
fn osb() -> Graph {
    let mut g = empty();
    let bottom = strands(&mut g, 10, 44, 180, 0.0, 1.0, Lie::ALONG);
    let lower = strands(&mut g, 12, 52, 195, 0.19, 0.7, Lie::ALONG);
    // The third layer went into the press badly oriented: sparse flakes at five angles across the
    // grain, each angle a layer of its own, its fibers turned with it.
    const ASKEW: [(i32, i32); 5] = [(2, 1), (1, 2), (0, 1), (-1, 2), (-2, 1)];
    let askew: Vec<(Strands, LayerId)> = ASKEW
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| {
            let lie = Lie { a, b, rag_mm: (14.0, 14.0) };
            let seed = 200 + 10 * i as u32;
            // As many cells as the bottom layer's, fewer by the turned grid's stretch, so the
            // flakes are as big.
            let cells = |n: f32| (n / lie.stretch()).round() as u32;
            let layer = strands(&mut g, cells(10.0), cells(44.0), seed, 0.31 + 0.13 * i as f32, 0.12, lie);
            let noise = g.add_unique("askew fiber noise", tiling_fbm(seed + 7, 16, 3)).unwrap();
            // Turned by -atan2(b, a) then scaled by |(a, b)|: whole entries, so it tiles, and the
            // stretched noise's lines run along `(a, b)`.
            let fibers = g
                .add_unique(
                    "askew fibers",
                    LayerKind::Transform(Transform {
                        source: Some(noise),
                        offset: [0.0; 3],
                        rotate_uv: -(b as f32).atan2(a as f32),
                        scale: [lie.stretch(), 6.0 * lie.stretch(), 1.0],
                        coord_mode: CoordMode::Passthrough,
                        edge_mode: EdgeMode::Extend,
                    }),
                )
                .unwrap();
            (layer, fibers)
        })
        .collect();
    let middle = strands(&mut g, 11, 48, 183, 0.43, 0.6, Lie::ALONG);
    let top = strands(&mut g, 9, 40, 186, 0.71, 0.5, Lie::ALONG);

    let tone = |g: &mut Graph, name: &str, lot: LayerId| {
        mapped(g, name, lot, &[
            (0.0, oklcha(0.84, 0.055, 88.0, 1.0)),
            (0.25, oklcha(0.76, 0.080, 80.0, 1.0)),
            (0.5, oklcha(0.68, 0.095, 70.0, 1.0)),
            (0.75, oklcha(0.60, 0.090, 62.0, 1.0)),
            (0.9, oklcha(0.50, 0.080, 56.0, 1.0)),
            (1.0, oklcha(0.72, 0.040, 85.0, 1.0)),
        ])
    };
    let void = solid(&mut g, "void", oklcha(0.46, 0.065, 60.0, 1.0));
    let bottom_tone = tone(&mut g, "bottom tone", bottom.lot);
    let middle_tone = tone(&mut g, "middle tone", middle.lot);
    let top_tone = tone(&mut g, "top tone", top.lot);
    // Each layer a little darker for the shade of the one above.
    let shade = solid(&mut g, "shade", oklcha(0.40, 0.060, 60.0, 1.0));
    let shade_share = solid(&mut g, "shade share", gray(0.15));
    let bottom_shaded = blend(&mut g, "bottom shaded", bottom_tone, shade, shade_share);
    let layer_bottom = blend(&mut g, "layer bottom", void, bottom_shaded, bottom.cover);
    let lower_tone = tone(&mut g, "lower tone", lower.lot);
    let lower_halo = scaled(&mut g, "lower halo share", lower.halo, 0.0, 0.35);
    let bottom_halo = blend(&mut g, "bottom halo", layer_bottom, shade, lower_halo);
    let lower_shade_share = solid(&mut g, "lower shade share", gray(0.11));
    let lower_shaded = blend(&mut g, "lower shaded", lower_tone, shade, lower_shade_share);
    let mut layer_one = blend(&mut g, "layer one", bottom_halo, lower_shaded, lower.cover);
    let askew_shade_share = solid(&mut g, "askew shade share", gray(0.09));
    for (layer, _) in &askew {
        let askew_tone = tone(&mut g, "askew tone", layer.lot);
        let halo_share = scaled(&mut g, "askew halo share", layer.halo, 0.0, 0.35);
        let under = blend(&mut g, "under askew", layer_one, shade, halo_share);
        let shaded = blend(&mut g, "askew shaded", askew_tone, shade, askew_shade_share);
        layer_one = blend(&mut g, "askew layer", under, shaded, layer.cover);
    }
    let middle_shade_share = solid(&mut g, "middle shade share", gray(0.07));
    let middle_shaded = blend(&mut g, "middle shaded", middle_tone, shade, middle_shade_share);
    let middle_halo = scaled(&mut g, "middle halo share", middle.halo, 0.0, 0.35);
    let one_shaded = blend(&mut g, "one shaded", layer_one, shade, middle_halo);
    let layer_two = blend(&mut g, "layer two", one_shaded, middle_shaded, middle.cover);
    let top_halo = scaled(&mut g, "top halo share", top.halo, 0.0, 0.35);
    let two_shaded = blend(&mut g, "two shaded", layer_two, shade, top_halo);
    let flakes = blend(&mut g, "flakes", two_shaded, top_tone, top.cover);

    let fiber_along = streaks(&mut g, "fiber along", tiling_fbm(189, 32, 3), 6.0, 192);
    // Each flake shows the fibers of its own lie: stacked as the flakes are.
    let mut fiber_long = fiber_along;
    for (layer, fibers) in &askew {
        fiber_long = blend(&mut g, "fiber field", fiber_long, *fibers, layer.cover);
    }
    let fiber_long = blend(&mut g, "fiber field", fiber_long, fiber_along, middle.cover);
    let fiber_long = blend(&mut g, "fiber field", fiber_long, fiber_along, top.cover);
    let (fiber_dark, fiber_light) = noisy_pair(oklcha(0.50, 0.080, 62.0, 1.0), oklcha(0.86, 0.050, 88.0, 1.0));
    let fiber_color = mapped(&mut g, "fiber color", fiber_long, &[(0.2, fiber_dark), (0.8, fiber_light)]);
    let fiber_share = solid(&mut g, "fiber share", gray(0.45));
    let fibered = blend(&mut g, "fibered", flakes, fiber_color, fiber_share);
    let resin_noise = g.add_unique("resin noise", tiling_fbm(190, 8, 4)).unwrap();
    let resin_spots = remap(&mut g, "resin spots", resin_noise, 0.62, 0.80);
    let resin_share = scaled(&mut g, "resin share", resin_spots, 0.0, 0.25);
    let resin = solid(&mut g, "resin", oklcha(0.50, 0.090, 58.0, 1.0));
    let color = blend(&mut g, "color", fibered, resin, resin_share);

    let wax = g.add_unique("wax", tiling_fbm(191, 4, 3)).unwrap();
    let (waxy, dry) = noisy(0.62, 0.82);
    let flake_rough = scaled(&mut g, "flake roughness", wax, waxy, dry);
    let void_rough = solid(&mut g, "void roughness", gray(0.95));
    let roughness = blend(&mut g, "roughness", void_rough, flake_rough, bottom.cover);

    let zero = solid(&mut g, "zero", gray(0.0));
    let bottom_height = scaled(&mut g, "bottom height", bottom.lot, 0.15, 0.25);
    let base = product(&mut g, "base", zero, bottom_height, bottom.cover);
    let one_height = scaled(&mut g, "one height", lower.lot, 0.30, 0.40);
    let mut one = blend(&mut g, "one", base, one_height, lower.cover);
    for (layer, _) in &askew {
        let askew_height = scaled(&mut g, "askew height", layer.lot, 0.40, 0.48);
        one = blend(&mut g, "askew one", one, askew_height, layer.cover);
    }
    let two_height = scaled(&mut g, "two height", middle.lot, 0.50, 0.58);
    let two = blend(&mut g, "two", one, two_height, middle.cover);
    let three_height = scaled(&mut g, "three height", top.lot, 0.65, 0.75);
    let three = blend(&mut g, "three", two, three_height, top.cover);
    let fiber_relief = scaled(&mut g, "fiber relief", fiber_long, 0.0, 0.08);
    let height = mix(&mut g, "height", three, fiber_relief, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.003);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// `v` bent up and down by a broad noise, up to `amount` of the tile, so what is laid out along it
/// undulates rather than ruling straight across. Still `v` plus something that tiles, so a whole
/// frequency over it keeps the tile seamless.
fn bent_v(g: &mut Graph, name: &str, v: LayerId, seed: u32, frequency: u32, amount: f32) -> LayerId {
    let noise = g.add_unique(format!("{name} noise"), tiling_fbm(seed, frequency, 3)).unwrap();
    let bend = scaled(g, &format!("{name} bend"), noise, 0.0, amount);
    mix(g, name, v, bend, BlendMode::Add)
}

/// A random profile down the tile, read at `along`: fbm along the palette's `u`, which a `Map`
/// reads at `v = 0`. Wrapped into `[0, 1)` first, because a `Map` clamps on the GPU; the profile
/// tiles, so the wrap is seamless. Over `v`, it draws beds that run along `u`.
fn profile(g: &mut Graph, name: &str, along: LayerId, seed: u32, frequency: u32, octaves: u32) -> LayerId {
    let at = wave(g, &format!("{name} at"), along, WaveShape::Sawtooth, 1.0, 0.0);
    let draw = g.add_unique(format!("{name} draw"), tiling_fbm(seed, frequency, octaves)).unwrap();
    g.add_unique(name, LayerKind::Map(Map { value: Some(at), palette: Some(draw) })).unwrap()
}

/// The most stops texture-graph's GPU baker takes in one ramp; a ramp with more panics there.
const MAX_RAMP_STOPS: usize = 16;

/// `value` cut into hard-edged bands: `colors[i]` below `edges[i]` and above the edge before it.
/// Too many bands for one ramp are cut at the middle edge into two, chosen between by a step.
fn banded(g: &mut Graph, name: &str, value: LayerId, edges: &[f32], colors: &[Color]) -> LayerId {
    assert_eq!(colors.len(), edges.len() + 1);
    const SOFT: f32 = 0.003;
    if 2 * edges.len() + 2 > MAX_RAMP_STOPS {
        let middle = edges.len() / 2;
        let below = banded(g, &format!("{name} below"), value, &edges[..middle], &colors[..=middle]);
        let above = banded(g, &format!("{name} above"), value, &edges[middle + 1..], &colors[middle + 1..]);
        let step = remap(g, &format!("{name} step"), value, edges[middle] - SOFT, edges[middle] + SOFT);
        return blend(g, name, below, above, step);
    }
    let mut stops = vec![(0.0, colors[0])];
    for (i, &edge) in edges.iter().enumerate() {
        stops.push((edge - SOFT, colors[i]));
        stops.push((edge + SOFT, colors[i + 1]));
    }
    stops.push((1.0, colors[edges.len()]));
    mapped(g, name, value, &stops)
}

/// Thin lines where `profile` crosses each of `count` evenly spaced levels: where it changes
/// quickly they crowd together, and where it is flat they spread out, so the spacing is irregular.
fn contours(g: &mut Graph, name: &str, profile: LayerId, count: f32, width: f32) -> LayerId {
    let crossing = wave(g, &format!("{name} wave"), profile, WaveShape::Triangle, count, 0.25);
    remap(g, name, crossing, 1.0 - 2.0 * width, 1.0 - width / 2.0)
}

/// `lines` fading in and out along their length, so no parting runs the whole tile unbroken.
fn broken(g: &mut Graph, name: &str, zero: LayerId, lines: LayerId, seed: u32) -> LayerId {
    let noise = g.add_unique(format!("{name} noise"), tiling_fbm(seed, 4, 3)).unwrap();
    let mask = remap(g, &format!("{name} mask"), noise, 0.40, 0.60);
    product(g, name, zero, lines, mask)
}

/// Fbm in a lattice stretched along `v`: `scale` times shorter in `u`, so streaks run down a wall.
fn down_streaks(g: &mut Graph, name: &str, seed: u32, frequency: u32, scale: f32) -> LayerId {
    let source = g.add_unique(format!("{name} noise"), tiling_fbm(seed, frequency, 3)).unwrap();
    stretched(g, name, source, [scale, 1.0])
}

/// Which face of a bedded rock a material shows.
#[derive(Clone, Copy, PartialEq)]
enum Face {
    /// Across the beds: they run level along `u`, and water has run down it.
    Wall,
    /// Along the beds, as the top of an outcrop: the floor cuts across a few beds at a shallow
    /// angle, so they show as irregular terraces, with the wall's colors so the two meet.
    Floor,
}

impl Face {
    /// `x`, or on a floor a share of it: a floor's top has been worn smooth, so its streaks,
    /// patches and lines are fainter and its beds stand out of it less.
    fn calm(self, x: f32) -> f32 {
        match self {
            Face::Wall => x,
            Face::Floor => x * 0.4,
        }
    }

    /// Bed colors, or on a floor each brought halfway to their mean lightness and chroma, so
    /// neighboring beds differ gently whichever stretch of them a floor crosses.
    fn beds(self, colors: &[Color]) -> Vec<Color> {
        match self {
            Face::Wall => colors.to_vec(),
            Face::Floor => {
                let n = colors.len() as f32;
                let l = colors.iter().map(|c| c.l).sum::<f32>() / n;
                let chroma = colors.iter().map(|c| c.chroma).sum::<f32>() / n;
                colors
                    .iter()
                    .map(|c| oklcha(l + (c.l - l) * 0.5, chroma + (c.chroma - chroma) * 0.5, c.hue.into_positive_degrees(), 1.0))
                    .collect()
            }
        }
    }

    /// A height `x` that sits about `middle`, brought toward it on a floor by [`Face::calm`].
    fn level(self, middle: f32, x: f32) -> Color {
        match self {
            Face::Wall => gray(x),
            Face::Floor => gray(middle + self.calm(x - middle)),
        }
    }
}

/// What a floor's beds are read along: a broad noise over `span` of the profile from `from`, so a
/// floor shows that stretch of the beds a wall does. Picked per rock for a stretch with several
/// beds in it, since a stretch inside one bed is a flat floor.
fn cut_across(g: &mut Graph, seed: u32, from: f32, span: f32) -> LayerId {
    let surface = g.add_unique("surface", tiling_fbm(seed, 2, 4)).unwrap();
    scaled(g, "cut", surface, from, from + span)
}

/// Where water has stained the face: streaks down a wall, patches on a floor.
fn runoff(g: &mut Graph, face: Face, seed: u32, frequency: u32, scale: f32) -> LayerId {
    match face {
        Face::Wall => down_streaks(g, "wash", seed, frequency, scale),
        Face::Floor => g.add_unique("wash", tiling_fbm(seed, frequency, 3)).unwrap(),
    }
}

/// Red cliff sandstone, as in the Grand Canyon's Redwall: beds from a few centimeters to a
/// quarter meter, each its own shade of brick red, terracotta, rust or pale pink-buff, ruled
/// through by fine laminae, the softer beds weathered back behind the harder, and stained by red
/// wash that has run down the face. On a wall `v` runs down, so the beds run level.
fn redwall_sandstone(face: Face) -> Graph {
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let bent = match face {
        Face::Wall => {
            let v = coordinate(&mut g, Axis::V);
            let broad = bent_v(&mut g, "broad bend", v, 200, 1, 0.04);
            bent_v(&mut g, "bent", broad, 207, 3, 0.012)
        }
        Face::Floor => cut_across(&mut g, 250, 0.0, 0.2),
    };
    let beds = profile(&mut g, "beds", bent, 201, 6, 4);
    let fine = profile(&mut g, "fine", bent, 202, 24, 3);

    let edges = [0.33, 0.37, 0.40, 0.44, 0.47, 0.50, 0.53, 0.57, 0.60, 0.64, 0.68];
    let deep = oklcha(0.42, 0.085, 33.0, 1.0);
    let brick = oklcha(0.50, 0.100, 37.0, 1.0);
    let terracotta = oklcha(0.56, 0.110, 44.0, 1.0);
    let rust = oklcha(0.52, 0.115, 48.0, 1.0);
    let buff = oklcha(0.66, 0.065, 55.0, 1.0);
    let pink = oklcha(0.61, 0.080, 40.0, 1.0);
    let bedded = banded(&mut g, "bedded", beds, &edges, &face.beds(&[
        deep, brick, terracotta, brick, pink, rust, brick, buff, terracotta, deep, rust, brick,
    ]));
    let lamina_whole = contours(&mut g, "laminae whole", fine, 6.0, 0.06);
    let lamina_lines = broken(&mut g, "laminae", zero, lamina_whole, 208);
    let lamina_share = scaled(&mut g, "lamina share", lamina_lines, 0.0, face.calm(0.35));
    let lamina = solid(&mut g, "lamina", oklcha(0.40, 0.075, 34.0, 1.0));
    let laminated = blend(&mut g, "laminated", bedded, lamina, lamina_share);
    let pale_lines = contours(&mut g, "pale laminae", beds, 9.0, 0.04);
    let pale_share = scaled(&mut g, "pale share", pale_lines, 0.0, face.calm(0.25));
    let pale = solid(&mut g, "pale", oklcha(0.70, 0.055, 58.0, 1.0));
    let lined = blend(&mut g, "lined", laminated, pale, pale_share);

    let wash = runoff(&mut g, face, 203, 2, 6.0);
    let washed = remap(&mut g, "washed", wash, 0.50, 0.75);
    let wash_share = scaled(&mut g, "wash share", washed, 0.0, face.calm(0.40));
    let stain = solid(&mut g, "stain", oklcha(0.40, 0.090, 32.0, 1.0));
    let stained = blend(&mut g, "stained", lined, stain, wash_share);
    let mottle = g.add_unique("mottle", tiling_fbm(204, 4, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.40, 0.080, 35.0, 1.0), oklcha(0.70, 0.070, 50.0, 1.0));
    let patches = mapped(&mut g, "patches", mottle, &[(0.25, dim), (0.75, bright)]);
    let patch_share = solid(&mut g, "patch share", gray(face.calm(0.40)));
    let patched = blend(&mut g, "patched", stained, patches, patch_share);
    let grain = g.add_unique("grain", tiling_fbm(205, 128, 2)).unwrap();
    let (dark_grain, light_grain) = noisy_pair(oklcha(0.38, 0.070, 38.0, 1.0), oklcha(0.74, 0.060, 55.0, 1.0));
    let sand = mapped(&mut g, "sand", grain, &[(0.3, dark_grain), (0.7, light_grain)]);
    let sand_share = solid(&mut g, "sand share", gray(0.25));
    let color = blend(&mut g, "color", patched, sand, sand_share);

    let roughness = scaled(&mut g, "roughness", grain, 0.86, 0.95);

    // The harder beds stand proud; the softer ones are weathered back between them.
    let hard = face.level(0.60, 0.75);
    let soft = face.level(0.60, 0.45);
    let ledges = banded(&mut g, "ledges", beds, &edges, &[
        hard, soft, hard, hard, soft, hard, soft, hard, soft, hard, soft, hard,
    ]);
    let undercut = scaled(&mut g, "undercut", lamina_lines, 0.0, face.calm(0.12));
    let cut = mix(&mut g, "cut", ledges, undercut, BlendMode::Subtract);
    let grit = scaled(&mut g, "grit", grain, 0.0, 0.10);
    let gritty = mix(&mut g, "gritty", cut, grit, BlendMode::Add);
    let pitting = g.add_unique("pitting", tiling_fbm(206, 32, 3)).unwrap();
    let pits = remap(&mut g, "pits", pitting, 0.66, 0.74);
    let pit_depth = scaled(&mut g, "pit depth", pits, 0.0, face.calm(0.15));
    let pitted = mix(&mut g, "pitted", gritty, pit_depth, BlendMode::Subtract);
    let chipping = g.add_unique("chipping", tiling_fbm(209, 16, 4)).unwrap();
    let chips = scaled(&mut g, "chips", chipping, 0.0, face.calm(0.20));
    let height = mix(&mut g, "height", pitted, chips, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.005);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Gray-green sandstone and shale, as in the Grand Canyon's Bright Angel: thin, even beds of
/// greenish and plain gray with the odd rust-stained one, split along close partings, and a fine
/// silty grain. Gray first and green second: the chroma stays under 0.03.
fn bright_angel_sandstone(face: Face) -> Graph {
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let bent = match face {
        Face::Wall => {
            let v = coordinate(&mut g, Axis::V);
            let broad = bent_v(&mut g, "broad bend", v, 210, 1, 0.03);
            bent_v(&mut g, "bent", broad, 216, 3, 0.008)
        }
        Face::Floor => cut_across(&mut g, 251, 0.45, 0.2),
    };
    let beds = profile(&mut g, "beds", bent, 211, 8, 3);
    let fine = profile(&mut g, "fine", bent, 212, 16, 3);

    let edges = [0.34, 0.38, 0.41, 0.44, 0.47, 0.50, 0.53, 0.56, 0.59, 0.62, 0.66];
    let sage = oklcha(0.60, 0.030, 140.0, 1.0);
    let slate = oklcha(0.55, 0.014, 165.0, 1.0);
    let olive = oklcha(0.56, 0.030, 125.0, 1.0);
    let silver = oklcha(0.66, 0.014, 135.0, 1.0);
    let moss = oklcha(0.51, 0.028, 148.0, 1.0);
    let rust = oklcha(0.57, 0.040, 65.0, 1.0);
    let bedded = banded(&mut g, "bedded", beds, &edges, &face.beds(&[
        slate, sage, silver, moss, sage, rust, slate, olive, silver, sage, moss, slate,
    ]));
    let parting_lines = contours(&mut g, "parting lines", fine, 4.0, 0.06);
    let partings = broken(&mut g, "partings", zero, parting_lines, 217);
    let parting_share = scaled(&mut g, "parting share", partings, 0.0, face.calm(0.45));
    let parting = solid(&mut g, "parting", oklcha(0.38, 0.014, 150.0, 1.0));
    let parted = blend(&mut g, "parted", bedded, parting, parting_share);
    let silt_lines = contours(&mut g, "silt", beds, 5.0, 0.04);
    let silt_share = scaled(&mut g, "silt share", silt_lines, 0.0, face.calm(0.25));
    let silt = solid(&mut g, "silt", oklcha(0.72, 0.012, 120.0, 1.0));
    let silted = blend(&mut g, "silted", parted, silt, silt_share);

    let mottle = g.add_unique("mottle", tiling_fbm(213, 4, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.46, 0.020, 140.0, 1.0), oklcha(0.72, 0.012, 130.0, 1.0));
    let patches = mapped(&mut g, "patches", mottle, &[(0.25, dim), (0.75, bright)]);
    let patch_share = solid(&mut g, "patch share", gray(face.calm(0.30)));
    let patched = blend(&mut g, "patched", silted, patches, patch_share);
    let wash = runoff(&mut g, face, 214, 4, 8.0);
    let washed = remap(&mut g, "washed", wash, 0.55, 0.78);
    let wash_share = scaled(&mut g, "wash share", washed, 0.0, face.calm(0.25));
    let stain = solid(&mut g, "stain", oklcha(0.50, 0.030, 70.0, 1.0));
    let stained = blend(&mut g, "stained", patched, stain, wash_share);
    let grain = g.add_unique("grain", tiling_fbm(215, 128, 2)).unwrap();
    let (dark_grain, light_grain) = noisy_pair(oklcha(0.42, 0.020, 145.0, 1.0), oklcha(0.76, 0.010, 120.0, 1.0));
    let sand = mapped(&mut g, "sand", grain, &[(0.3, dark_grain), (0.7, light_grain)]);
    let sand_share = solid(&mut g, "sand share", gray(0.22));
    let color = blend(&mut g, "color", stained, sand, sand_share);

    let roughness = scaled(&mut g, "roughness", grain, 0.84, 0.94);

    let hard = face.level(0.60, 0.70);
    let soft = face.level(0.60, 0.50);
    let ledges = banded(&mut g, "ledges", beds, &edges, &[
        soft, hard, hard, soft, hard, soft, hard, soft, hard, soft, hard, soft,
    ]);
    let split = scaled(&mut g, "split", partings, 0.0, face.calm(0.20));
    let cut = mix(&mut g, "cut", ledges, split, BlendMode::Subtract);
    let grit = scaled(&mut g, "grit", grain, 0.0, 0.08);
    let gritty = mix(&mut g, "gritty", cut, grit, BlendMode::Add);
    let chipping = g.add_unique("chipping", tiling_fbm(218, 16, 4)).unwrap();
    let chips = scaled(&mut g, "chips", chipping, 0.0, face.calm(0.15));
    let height = mix(&mut g, "height", gritty, chips, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.005);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Mottled limestone, as in the Grand Canyon's Muav: thin and medium beds of dark and light gray,
/// a few buff, with dark partings between them; each bed blotched with the other gray where
/// burrows churned the mud, and pitted where rain has dissolved it.
fn muav_limestone(face: Face) -> Graph {
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let bent = match face {
        Face::Wall => {
            let v = coordinate(&mut g, Axis::V);
            let broad = bent_v(&mut g, "broad bend", v, 220, 1, 0.03);
            bent_v(&mut g, "bent", broad, 226, 4, 0.015)
        }
        Face::Floor => cut_across(&mut g, 252, 0.55, 0.2),
    };
    let beds = profile(&mut g, "beds", bent, 221, 5, 4);

    let edges = [0.36, 0.42, 0.47, 0.53, 0.58, 0.64];
    let dark = oklcha(0.46, 0.008, 245.0, 1.0);
    let darker = oklcha(0.40, 0.007, 250.0, 1.0);
    let light = oklcha(0.66, 0.006, 235.0, 1.0);
    let lighter = oklcha(0.72, 0.008, 90.0, 1.0);
    let buff = oklcha(0.64, 0.024, 75.0, 1.0);
    let bedded = banded(&mut g, "bedded", beds, &edges, &face.beds(&[dark, light, darker, buff, light, dark, lighter]));
    // Which way a bed's blotches go: light in a dark bed, dark in a light one.
    let other = banded(&mut g, "other", beds, &edges, &face.beds(&[light, darker, light, dark, dark, lighter, dark]));
    let burrows = g.add_unique("burrows", tiling_fbm(222, 6, 5)).unwrap();
    let flattened = stretched(&mut g, "flattened", burrows, [1.0, 2.0]);
    let blotches = remap(&mut g, "blotches", flattened, 0.52, 0.70);
    let blotch_share = scaled(&mut g, "blotch share", blotches, 0.0, face.calm(0.30));
    let mottled = blend_linear(&mut g, "mottled", bedded, other, blotch_share);
    let parting_lines = contours(&mut g, "parting lines", beds, 4.0, 0.05);
    let partings = broken(&mut g, "partings", zero, parting_lines, 227);
    let parting_share = scaled(&mut g, "parting share", partings, 0.0, face.calm(0.5));
    let parting = solid(&mut g, "parting", oklcha(0.30, 0.008, 250.0, 1.0));
    let parted = blend(&mut g, "parted", mottled, parting, parting_share);

    let wash = runoff(&mut g, face, 223, 2, 6.0);
    let washed = remap(&mut g, "washed", wash, 0.55, 0.80);
    let wash_share = scaled(&mut g, "wash share", washed, 0.0, face.calm(0.15));
    let streak = solid(&mut g, "streak", oklcha(0.76, 0.006, 90.0, 1.0));
    let streaked = blend(&mut g, "streaked", parted, streak, wash_share);
    let pitting = g.add_unique("pitting", tiling_fbm(224, 64, 2)).unwrap();
    let pits = remap(&mut g, "pits", pitting, 0.73, 0.78);
    let pit_share = scaled(&mut g, "pit share", pits, 0.0, face.calm(0.25));
    let shade = solid(&mut g, "shade", oklcha(0.28, 0.006, 250.0, 1.0));
    let pitted = blend(&mut g, "pitted", streaked, shade, pit_share);
    let grain = g.add_unique("grain", tiling_fbm(225, 128, 2)).unwrap();
    let (dark_grain, light_grain) = noisy_pair(oklcha(0.40, 0.006, 250.0, 1.0), oklcha(0.76, 0.006, 90.0, 1.0));
    let speck = mapped(&mut g, "speck", grain, &[(0.3, dark_grain), (0.7, light_grain)]);
    let speck_share = solid(&mut g, "speck share", gray(0.18));
    let color = blend(&mut g, "color", pitted, speck, speck_share);

    let roughness = scaled(&mut g, "roughness", grain, 0.82, 0.93);

    let hard = face.level(0.64, 0.72);
    let soft = face.level(0.64, 0.55);
    let ledges = banded(&mut g, "ledges", beds, &edges, &[soft, hard, soft, hard, hard, soft, hard]);
    let split = scaled(&mut g, "split", partings, 0.0, face.calm(0.25));
    let cut = mix(&mut g, "cut", ledges, split, BlendMode::Subtract);
    let knobs = scaled(&mut g, "knobs", blotches, 0.0, face.calm(0.06));
    let knobbly = mix(&mut g, "knobbly", cut, knobs, BlendMode::Add);
    let pit_depth = scaled(&mut g, "pit depth", pits, 0.0, face.calm(0.25));
    let dissolved = mix(&mut g, "dissolved", knobbly, pit_depth, BlendMode::Subtract);
    let grit = scaled(&mut g, "grit", grain, 0.0, 0.05);
    let gritty = mix(&mut g, "gritty", dissolved, grit, BlendMode::Add);
    let chipping = g.add_unique("chipping", tiling_fbm(228, 16, 4)).unwrap();
    let chips = scaled(&mut g, "chips", chipping, 0.0, face.calm(0.15));
    let height = mix(&mut g, "height", gritty, chips, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.005);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// A beaten dirt path: packed brown earth, darker and smoother where feet have tamped it down,
/// with fine grit, scattered pebbles standing a little proud, and faint cracks where it has dried.
fn dirt_path() -> Graph {
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let patches = g.add_unique("patches", tiling_fbm(230, 2, 4)).unwrap();
    let (loose, packed) = noisy_pair(oklcha(0.56, 0.045, 70.0, 1.0), oklcha(0.42, 0.040, 60.0, 1.0));
    let base = mapped(&mut g, "base", patches, &[(0.25, loose), (0.75, packed)]);
    let mottle = g.add_unique("mottle", tiling_fbm(231, 8, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.40, 0.040, 58.0, 1.0), oklcha(0.62, 0.045, 75.0, 1.0));
    let blotches = mapped(&mut g, "blotches", mottle, &[(0.25, dim), (0.75, bright)]);
    let blotch_share = solid(&mut g, "blotch share", gray(0.4));
    let earth = blend(&mut g, "earth", base, blotches, blotch_share);
    let trodden_noise = g.add_unique("trodden noise", tiling_fbm(232, 2, 3)).unwrap();
    let trodden = remap(&mut g, "trodden", trodden_noise, 0.50, 0.65);
    let tamp_share = scaled(&mut g, "tamp share", trodden, 0.0, 0.35);
    let tamped_color = solid(&mut g, "tamped color", oklcha(0.40, 0.035, 58.0, 1.0));
    let tamped = blend(&mut g, "tamped", earth, tamped_color, tamp_share);

    let grain = g.add_unique("grain", tiling_fbm(233, 128, 2)).unwrap();
    let dark_fleck = remap(&mut g, "dark fleck", grain, 0.28, 0.20);
    let dark_share = scaled(&mut g, "dark share", dark_fleck, 0.0, 0.35);
    let humus = solid(&mut g, "humus", oklcha(0.30, 0.030, 55.0, 1.0));
    let darkened = blend(&mut g, "darkened", tamped, humus, dark_share);
    let light_fleck = remap(&mut g, "light fleck", grain, 0.72, 0.80);
    let light_share = scaled(&mut g, "light share", light_fleck, 0.0, 0.5);
    let grit_color = solid(&mut g, "grit color", oklcha(0.70, 0.030, 80.0, 1.0));
    let gritty = blend(&mut g, "gritty", darkened, grit_color, light_share);

    let stones = g.add_unique("stones", tiling_fbm(234, 48, 2)).unwrap();
    let pebble = remap(&mut g, "pebble", stones, 0.71, 0.74);
    let tint = g.add_unique("tint", tiling_fbm(235, 16, 2)).unwrap();
    let pebble_color = mapped(&mut g, "pebble color", tint, &[
        (0.35, oklcha(0.46, 0.025, 65.0, 1.0)),
        (0.50, oklcha(0.58, 0.020, 80.0, 1.0)),
        (0.65, oklcha(0.50, 0.035, 58.0, 1.0)),
    ]);
    let pebbled = blend(&mut g, "pebbled", gritty, pebble_color, pebble);
    let crack_lines = g.add_unique("crack lines", ridged(236, 4, 3)).unwrap();
    let cracks = remap(&mut g, "cracks", crack_lines, 0.90, 0.96);
    let crack_mask = product(&mut g, "dry cracks", zero, cracks, trodden);
    let crack_share = scaled(&mut g, "crack share", crack_mask, 0.0, 0.5);
    let crack = solid(&mut g, "crack", oklcha(0.26, 0.025, 55.0, 1.0));
    let color = blend(&mut g, "color", pebbled, crack, crack_share);

    let dirt_rough = scaled(&mut g, "dirt roughness", trodden, 0.97, 0.86);
    let pebble_rough = solid(&mut g, "pebble roughness", gray(0.75));
    let roughness = blend(&mut g, "roughness", dirt_rough, pebble_rough, pebble);

    let swell = scaled(&mut g, "swell", patches, 0.35, 0.55);
    let lumps = scaled(&mut g, "lumps", mottle, 0.0, 0.10);
    let lumpy = mix(&mut g, "lumpy", swell, lumps, BlendMode::Add);
    let flatten = scaled(&mut g, "flatten", trodden, 0.0, 0.05);
    let flat = mix(&mut g, "flat", lumpy, flatten, BlendMode::Subtract);
    let grit = scaled(&mut g, "grit", grain, 0.0, 0.10);
    let ground = mix(&mut g, "ground", flat, grit, BlendMode::Add);
    let crack_depth = scaled(&mut g, "crack depth", crack_mask, 0.0, 0.10);
    let cracked = mix(&mut g, "cracked", ground, crack_depth, BlendMode::Subtract);
    let rise = scaled(&mut g, "rise", stones, 0.0, 0.30);
    let pebble_top = mix(&mut g, "pebble top", cracked, rise, BlendMode::Add);
    let height = blend(&mut g, "height", cracked, pebble_top, pebble);
    let normal = normal_of(&mut g, height, 0.006);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}

/// Desert sand: pale warm tan, the dark heavy grains collected in the low spots and the high ones a
/// shade lighter.
///
/// Rippled, the wind has combed it into ripples about 12 cm apart along `u`, each with a long
/// gentle windward face and a short steep lee, meandering and fading in and out. Otherwise it lies
/// in soft hummocks with no direction to them.
fn desert_sand(rippled: bool) -> Graph {
    const RIPPLES: f32 = 16.0;
    let mut g = empty();
    let zero = solid(&mut g, "zero", gray(0.0));
    let ripple = if rippled {
        let v = coordinate(&mut g, Axis::V);
        let bent = bent_v(&mut g, "bent", v, 240, 1, 0.06);
        let bent_more = bent_v(&mut g, "bent more", bent, 241, 4, 0.035);
        let into_ripple = wave(&mut g, "into ripple", bent_more, WaveShape::Sawtooth, RIPPLES, 0.0);
        // Up the windward face over most of the cycle, down the lee in the rest.
        mapped(&mut g, "ripple", into_ripple, &[
            (0.0, gray(0.0)),
            (0.45, gray(0.55)),
            (0.75, gray(1.0)),
            (0.82, gray(0.85)),
            (1.0, gray(0.0)),
        ])
    } else {
        let hummocks = g.add_unique("hummocks", tiling_fbm(245, 8, 4)).unwrap();
        remap(&mut g, "ripple", hummocks, 0.30, 0.70)
    };
    let strength_noise = g.add_unique("strength noise", tiling_fbm(242, 2, 3)).unwrap();
    let strength = remap(&mut g, "strength", strength_noise, 0.30, 0.65);
    let relief = product(&mut g, "relief", zero, ripple, strength);

    let drift = g.add_unique("drift", tiling_fbm(243, 2, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.70, 0.065, 72.0, 1.0), oklcha(0.84, 0.055, 82.0, 1.0));
    let base = mapped(&mut g, "base", drift, &[(0.25, dim), (0.75, bright)]);
    let crest_share = scaled(&mut g, "crest share", relief, 0.0, 0.35);
    let crest = solid(&mut g, "crest", oklcha(0.84, 0.050, 82.0, 1.0));
    let crested = blend(&mut g, "crested", base, crest, crest_share);
    let trough = remap(&mut g, "trough", relief, 0.12, 0.0);
    let trough_strength = product(&mut g, "trough strength", zero, trough, strength);
    let grain = g.add_unique("grain", tiling_fbm(244, 128, 2)).unwrap();
    let heavy = remap(&mut g, "heavy", grain, 0.30, 0.22);
    let heavy_together = product(&mut g, "heavy together", zero, heavy, trough_strength);
    let heavy_in_troughs = scaled(&mut g, "heavy in troughs", heavy_together, 0.0, 0.5);
    let scattered = scaled(&mut g, "scattered", heavy, 0.0, 0.10);
    let heavy_share = pick(&mut g, "heavy share", heavy_in_troughs, scattered, MinMaxMode::Max);
    let magnetite = solid(&mut g, "magnetite", oklcha(0.55, 0.045, 60.0, 1.0));
    let speckled = blend(&mut g, "speckled", crested, magnetite, heavy_share);
    let (dark_grain, light_grain) = noisy_pair(oklcha(0.62, 0.070, 65.0, 1.0), oklcha(0.90, 0.040, 88.0, 1.0));
    let sand = mapped(&mut g, "sand", grain, &[(0.3, dark_grain), (0.7, light_grain)]);
    let sand_share = solid(&mut g, "sand share", gray(0.35));
    let color = blend(&mut g, "color", speckled, sand, sand_share);

    let roughness = scaled(&mut g, "roughness", grain, 0.92, 0.98);

    let base_height = solid(&mut g, "base height", gray(0.3));
    let swell = scaled(&mut g, "swell", drift, 0.0, 0.15);
    let dune = mix(&mut g, "dune", base_height, swell, BlendMode::Add);
    let rippled = scaled(&mut g, "rippled", relief, 0.0, 0.45);
    let combed = mix(&mut g, "combed", dune, rippled, BlendMode::Add);
    let grit = scaled(&mut g, "grit", grain, 0.0, 0.03);
    let height = mix(&mut g, "height", combed, grit, BlendMode::Add);
    let normal = normal_of(&mut g, height, 0.008);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}
