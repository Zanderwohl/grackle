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

/// [`blend`] in linear light, for two colors far apart in hue, which an Oklch blend would carry
/// round the hue circle through colors neither of them is.
fn blend_linear(g: &mut Graph, name: &str, a: LayerId, b: LayerId, by: LayerId) -> LayerId {
    g.add_layer(
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

/// Poured concrete: pale, a little warm, with a fine grain of sand and aggregate and the odd air
/// hole, under broad patches a meter or two across. Nothing between the two scales, since a
/// middle-sized noise repeating every tile reads as a pattern rather than as concrete.
fn concrete() -> Graph {
    let mut g = empty();
    let patches = g.add_layer("patches", tiling_fbm(0, 1, 3)).unwrap();
    let base = mapped(&mut g, "base", patches, &[
        (0.25, oklcha(0.70, 0.008, 75.0, 1.0)),
        (0.75, oklcha(0.77, 0.010, 78.0, 1.0)),
    ]);
    let grain = g.add_layer("grain", tiling_fbm(1, 128, 2)).unwrap();
    let sand = mapped(&mut g, "sand", grain, &[
        (0.3, oklcha(0.64, 0.010, 72.0, 1.0)),
        (0.7, oklcha(0.80, 0.008, 80.0, 1.0)),
    ]);
    let sand_share = solid(&mut g, "sand share", gray(0.18));
    let sanded = blend(&mut g, "sanded", base, sand, sand_share);
    let pocking = g.add_layer("pocking", tiling_fbm(2, 64, 2)).unwrap();
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

    let grime = g.add_layer("grime", tiling_fbm(20, 4, 5)).unwrap();
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
    g.add_layer(
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
    let draw = g.add_layer("draw", lattice(bond.seed, STRETCHERS * COURSES)).unwrap();
    let lot = g.add_layer("lot", LayerKind::Map(Map { value: Some(id), palette: Some(draw) })).unwrap();

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
        .add_layer(
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
        g.add_layer(
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
    let mottle = g.add_layer("mottle", tiling_fbm(51, 8, 5)).unwrap();
    let scorched = remap(&mut g, "scorched", mottle, 0.45, 0.85);
    let scorch_share = scaled(&mut g, "scorch share", scorched, 0.0, 0.5);
    let scorch = solid(&mut g, "scorch", oklcha(0.30, 0.050, 30.0, 1.0));
    let clay = blend(&mut g, "clay", fired, scorch, scorch_share);
    let speckle = g.add_layer("speckle", tiling_fbm(52, 128, 2)).unwrap();
    let dark_fleck = remap(&mut g, "dark fleck", speckle, 0.24, 0.18);
    let dark_specks = scaled(&mut g, "dark specks", dark_fleck, 0.0, 0.6);
    let iron = solid(&mut g, "iron", oklcha(0.26, 0.030, 40.0, 1.0));
    let flecked = blend(&mut g, "flecked", clay, iron, dark_specks);
    let light_fleck = remap(&mut g, "light fleck", speckle, 0.78, 0.84);
    let light_specks = scaled(&mut g, "light specks", light_fleck, 0.0, 0.5);
    let sand = solid(&mut g, "sand", oklcha(0.66, 0.050, 70.0, 1.0));
    let brick = blend(&mut g, "brick", flecked, sand, light_specks);

    let mortar_grain = g.add_layer("mortar grain", tiling_fbm(53, 32, 4)).unwrap();
    let (dull, pale) = noisy_pair(oklcha(0.50, 0.010, 85.0, 1.0), oklcha(0.72, 0.010, 85.0, 1.0));
    let mortar = mapped(&mut g, "mortar", mortar_grain, &[(0.25, dull), (0.75, pale)]);
    let color = blend(&mut g, "color", brick, mortar, b.joint);

    let grit = g.add_layer("grit", tiling_fbm(54, 64, 3)).unwrap();
    let brick_rough = scaled(&mut g, "brick roughness", grit, 0.86, 0.96);
    let mortar_rough = solid(&mut g, "mortar roughness", gray(0.97));
    let roughness = blend(&mut g, "roughness", brick_rough, mortar_rough, b.joint);

    let pitting = g.add_layer("pitting", tiling_fbm(55, 32, 3)).unwrap();
    let pits = remap(&mut g, "pits", pitting, 0.64, 0.74);
    let lumps = g.add_layer("lumps", tiling_fbm(56, 8, 3)).unwrap();
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
}

/// The red brick's bond under coats of satin latex: the paint has filled the grit and pits and
/// softened the arrises and joints, and has a little sheen.
fn painted_brick(paint: Coat) -> Graph {
    let mut g = empty();
    let b = bricks(&mut g, &Bond { joint: 10.0, arris: 10.0, chips: 1.5, seed: 50 });
    let coat = |l: f32| oklcha(l, paint.chroma, paint.hue, 1.0);

    let trowel = g.add_layer("roller", tiling_fbm(70, 4, 4)).unwrap();
    let (thin, thick) = noisy_pair(coat(paint.lightness - 0.03), coat(paint.lightness + 0.02));
    let wall = mapped(&mut g, "wall", trowel, &[(0.25, thin), (0.75, thick)]);
    // The brick under it still shows a little through the paint.
    let show_through = scaled(&mut g, "show through", b.lot, 0.0, 0.25);
    let darker = solid(&mut g, "darker", coat(paint.lightness - 0.05));
    let bricks_under = blend(&mut g, "bricks under", wall, darker, show_through);
    let pooled = solid(&mut g, "pooled", coat(paint.lightness - 0.06));
    let pooled_share = scaled(&mut g, "pooled share", b.joint, 0.0, 0.6);
    let color = blend(&mut g, "color", bricks_under, pooled, pooled_share);

    let sheen = g.add_layer("sheen", tiling_fbm(72, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.40, 0.55);
    let face_rough = scaled(&mut g, "face roughness", sheen, smooth, rough);
    let joint_rough = solid(&mut g, "joint roughness", gray(0.65));
    let roughness = blend(&mut g, "roughness", face_rough, joint_rough, b.joint);

    let peel = g.add_layer("peel", tiling_fbm(71, 64, 2)).unwrap();
    let pitting = g.add_layer("pitting", tiling_fbm(55, 32, 3)).unwrap();
    let pits = remap(&mut g, "pits", pitting, 0.68, 0.76);
    let lumps = g.add_layer("lumps", tiling_fbm(56, 8, 3)).unwrap();
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
    let draw = g.add_layer("draw", lattice(95, TILES * TILES)).unwrap();
    let lot = g.add_layer("lot", LayerKind::Map(Map { value: Some(id), palette: Some(draw) })).unwrap();

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
    let grime = g.add_layer("grime", tiling_fbm(96, 16, 3)).unwrap();
    let (dirty, clean) = noisy_pair(oklcha(0.50, 0.008, 80.0, 1.0), oklcha(0.66, 0.006, 80.0, 1.0));
    let grout_color = mapped(&mut g, "grout color", grime, &[(0.25, dirty), (0.75, clean)]);
    let color = blend_linear(&mut g, "color", glazed, grout_color, grout);

    let (gloss, satin) = noisy(0.12, 0.28);
    let glaze_rough = scaled(&mut g, "glaze roughness", lot, gloss, satin);
    let grout_rough = solid(&mut g, "grout roughness", gray(0.92));
    let roughness = blend(&mut g, "roughness", glaze_rough, grout_rough, grout);

    let ripple = g.add_layer("ripple", tiling_fbm(97, 32, 2)).unwrap();
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

    let spangle = g.add_layer("spangle", tiling_fbm(60, 6, 4)).unwrap();
    let (dim, bright) = noisy_pair(oklcha(0.70, 0.006, 230.0, 1.0), oklcha(0.82, 0.004, 230.0, 1.0));
    let zinc = mapped(&mut g, "zinc", spangle, &[(0.25, dim), (0.75, bright)]);

    let wear_noise = g.add_layer("wear noise", tiling_fbm(61, 2, 3)).unwrap();
    let wear = remap(&mut g, "wear", wear_noise, 0.30, 0.60);
    let scratch_noise = g
        .add_layer(
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

    let streak_noise = g.add_layer("streak noise", tiling_fbm(63, 4, 5)).unwrap();
    let streak_lines = stretched(&mut g, "streak lines", streak_noise, [8.0, 1.0]);
    let streaks = remap(&mut g, "streaks", streak_lines, 0.42, 0.70);
    let staining_noise = g.add_layer("staining noise", tiling_fbm(64, 2, 3)).unwrap();
    let staining = remap(&mut g, "staining", staining_noise, 0.30, 0.60);
    let gutter = scaled(&mut g, "gutter", trough, 0.3, 1.0);
    let run = product(&mut g, "run", zero, streaks, gutter);
    let stain = product(&mut g, "stain", zero, run, staining);
    let kind = g.add_layer("rust or grime", tiling_fbm(65, 4, 3)).unwrap();
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

    let dent_noise = g.add_layer("dent noise", tiling_fbm(66, 4, 3)).unwrap();
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
    let draw = g.add_layer("draw", lattice(80, BOARDS)).unwrap();
    let lot = g.add_layer("lot", LayerKind::Map(Map { value: Some(column), palette: Some(draw) })).unwrap();

    // The figure: lines about six to a board, running down it, wandering with noise stretched
    // down the board and bent round the knots. `u` plus the wander, through a whole number of
    // cycles, so the lines tile.
    const LINES: f32 = 6.0;
    let figure_noise = g.add_layer("figure noise", tiling_fbm(81, 4, 3)).unwrap();
    let figure = stretched(&mut g, "figure", figure_noise, [4.0, 1.0]);
    let wander = scaled(&mut g, "wander", figure, 0.0, 3.0 / (LINES * boards));
    let wandering = mix(&mut g, "wandering", u, wander, BlendMode::Add);
    let knot_spots = g.add_layer("knot spots", tiling_fbm(82, 8, 2)).unwrap();
    let knot_noise = stretched(&mut g, "knot noise", knot_spots, [2.0, 1.0]);
    let knot_swirl = remap(&mut g, "knot swirl", knot_noise, 0.70, 0.84);
    let swirl = scaled(&mut g, "swirl", knot_swirl, 0.0, 2.0 / (LINES * boards));
    let bent = mix(&mut g, "bent", wandering, swirl, BlendMode::Add);
    let rings = wave(&mut g, "rings", bent, WaveShape::Triangle, LINES * boards, 0.0);
    let late_wood = remap(&mut g, "late wood", rings, 0.70, 0.95);
    let knot_core = remap(&mut g, "knot core", knot_noise, 0.82, 0.84);
    let fiber_noise = g.add_layer("fiber noise", tiling_fbm(83, 8, 4)).unwrap();
    let fibers = stretched(&mut g, "fibers", fiber_noise, [16.0, 1.0]);
    let split_noise = g
        .add_layer(
            "split noise",
            LayerKind::Noise(Noise {
                dims: NoiseDims::D2,
                seed_offset: 84,
                frequency: 4.0,
                range: NoiseRange::Unsigned,
                output: NoiseOutput::Grayscale,
                kernel: NoiseKernel::Value,
                period: [4, 4, 0],
                fractal: Fractal { octaves: 3, lacunarity: 2.0, gain: 0.5, mode: FractalMode::Ridged, normalize: true },
            }),
        )
        .unwrap();
    let split_lines = stretched(&mut g, "split lines", split_noise, [24.0, 1.0]);
    let splits = remap(&mut g, "splits", split_lines, 0.93, 0.98);
    // Shifted by the board's own value; the same at a tile's two edges, since the id wraps.
    let shift = |g: &mut Graph, name: &str, source: LayerId| {
        g.add_layer(
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
        (0.0, oklcha(0.50, 0.050, 62.0, 1.0)),
        (0.35, oklcha(0.62, 0.065, 72.0, 1.0)),
        (0.65, oklcha(0.70, 0.070, 80.0, 1.0)),
        (0.85, oklcha(0.60, 0.030, 78.0, 1.0)),
        (1.0, oklcha(0.56, 0.015, 80.0, 1.0)),
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
    let rust_noise = g.add_layer("rust noise", tiling_fbm(85, 32, 3)).unwrap();
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

    let trowel = g.add_layer("trowel", tiling_fbm(90, 2, 5)).unwrap();
    let coat = |l: f32| oklcha(l, paint.chroma, paint.hue, 1.0);
    let (dark_lo, dark_hi) = noisy_pair(coat(paint.dark - 0.03), coat(paint.dark + 0.03));
    let dark = mapped(&mut g, "dark coat", trowel, &[(0.25, dark_lo), (0.75, dark_hi)]);
    let (light_lo, light_hi) = noisy_pair(coat(paint.light - 0.03), coat(paint.light + 0.03));
    let light = mapped(&mut g, "light coat", trowel, &[(0.25, light_lo), (0.75, light_hi)]);
    let color = blend(&mut g, "color", dark, light, light_stripe);

    let sheen = g.add_layer("sheen", tiling_fbm(92, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.70, 0.85);
    let roughness = scaled(&mut g, "roughness", sheen, smooth, rough);
    let peel = g.add_layer("peel", tiling_fbm(91, 64, 2)).unwrap();
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

    let trowel = g.add_layer("trowel", tiling_fbm(40, 2, 5)).unwrap();
    let (shaded, bright) = noisy_pair(
        oklcha(paint.lightness.0, paint.chroma, paint.hue - 2.0, 1.0),
        oklcha(paint.lightness.1, paint.chroma, paint.hue + 2.0, 1.0),
    );
    let base = mapped(&mut g, "paint", trowel, &[(0.25, shaded), (0.75, bright)]);
    let stripe = solid(&mut g, "stripe", oklcha(paint.stripe_lightness, paint.stripe_chroma, paint.hue + 10.0, 1.0));
    let color = blend(&mut g, "color", base, stripe, lines);

    let sheen = g.add_layer("sheen", tiling_fbm(42, 4, 3)).unwrap();
    let (smooth, rough) = noisy(0.70, 0.85);
    let roughness = scaled(&mut g, "roughness", sheen, smooth, rough);
    let peel = g.add_layer("peel", tiling_fbm(41, 64, 2)).unwrap();
    let height = scaled(&mut g, "height", peel, 0.0, 0.5);
    let normal = normal_of(&mut g, height, 0.003 * NOISE);
    finish(g, Output {
        color: Some(color),
        roughness: ScalarInput::Layer(roughness),
        metallic: ScalarInput::Const(0.0),
        normal: Some(normal),
    })
}
