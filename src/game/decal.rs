//! Marks on the map's surface, each cut from the baked faces it lands on (see
//! [`crate::common::decal`] and `documentation/decals.md`).
//!
//! A [`Decal`] is a root carrying its projection, its material and one lifetime marker:
//! [`MapDecal`], [`Spray`] or [`Transient`]. Its meshes are [`DecalPiece`]s, one per room,
//! each [`DecalPieceOf`] the root and [`PaintedOn`] the room's `BakedRoomGeometry`, so the root
//! and the room each take their pieces with them. A root marked [`Uncut`] is cut on the next
//! frame.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageSamplerDescriptor, ImageType};
use bevy::light::NotShadowCaster;
use bevy::mesh::VertexAttributeValues;
use bevy::pbr::Lightmap;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::{DamageDealt, How};
use crate::common::decal::{self, Projection};
use crate::common::effects::Effect;
use crate::common::world_surface;
use crate::common::lightmap::Atlas;
use crate::common::palette::Palette;
use crate::common::team::Side;
use crate::common::player_settings::{DebrisDecals, PlayerSettings};
use crate::common::skeleton::Skeleton;
use crate::editor::decal::MapDecalSpec;
use crate::game::ragdoll::{GibImpact, GIB_SPLAT_SPEED};
use crate::tool::bakes::BakedRoomGeometry;
use crate::tool::room::{RoomSlot, RoomSurfaces};
use crate::tool::sun_bake::{keep_lightmap, SunBake};

#[derive(Component, Clone, Copy, Debug)]
#[require(Uncut, Transform, Visibility)]
pub struct Decal(pub Projection);

#[derive(Component, Clone, Debug)]
pub struct DecalMaterial(pub Handle<StandardMaterial>);

/// Cut on the next frame.
#[derive(Component, Default)]
pub struct Uncut;

/// Part of the map: lasts as long as the feature it stands for.
#[derive(Component, Default)]
pub struct MapDecal;

/// Lasts as long as its owner's connection; see [`crate::game::spray`].
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spray {
    pub owner: u64,
}

/// Gone when its kind's ring is full, when it has faded out, or when the round ends.
#[derive(Component, Clone, Copy, Debug)]
pub struct Transient {
    pub kind: TransientKind,
    /// `Time<Virtual>` seconds.
    pub born: f32,
}

/// Pieces of a lit decal take their room's lightmap.
#[derive(Component, Default)]
pub struct Lit;

#[derive(Component)]
pub struct DecalPiece {
    /// The layout its `UV_1` charts.
    faces_hash: u64,
    /// Each vertex's alpha as cut, before any fading.
    alphas: Vec<f32>,
    /// The fade step last written to the mesh, out of [`FADE_STEPS`]; writing every frame would
    /// upload the vertex buffer every frame.
    faded: u8,
}

#[derive(Component)]
#[relationship(relationship_target = DecalPieces)]
pub struct DecalPieceOf(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = DecalPieceOf, linked_spawn)]
pub struct DecalPieces(Vec<Entity>);

/// The `BakedRoomGeometry` a piece was cut from.
#[derive(Component)]
#[relationship(relationship_target = PaintedWith)]
pub struct PaintedOn(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = PaintedOn, linked_spawn)]
pub struct PaintedWith(Vec<Entity>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransientKind {
    BulletHole,
    Blood,
    Scorch,
}

impl TransientKind {
    pub const ALL: [TransientKind; 3] = [TransientKind::BulletHole, TransientKind::Blood, TransientKind::Scorch];

    /// The most kept at once, by this machine's settings.
    pub fn cap(self, limits: &DebrisDecals) -> usize {
        (match self {
            TransientKind::BulletHole => limits.bullet_holes,
            TransientKind::Blood => limits.blood,
            TransientKind::Scorch => limits.scorch,
        }) as usize
    }

    /// Seconds, the last [`FADE_SECONDS`] of them fading out.
    pub fn lifetime(self) -> f32 {
        match self {
            TransientKind::BulletHole => 60.0,
            TransientKind::Blood => 45.0,
            TransientKind::Scorch => 90.0,
        }
    }

    /// Files under a pack's `decals/`.
    fn pictures(self) -> &'static [&'static str] {
        match self {
            TransientKind::BulletHole => &["bullet_hole_1.png", "bullet_hole_2.png"],
            TransientKind::Blood => &[
                "blood_1.png",
                "blood_2.png",
                "blood_3.png",
                "blood_4.png",
                "blood_5.png",
                "blood_6.png",
                "blood_7.png",
                "blood_8.png",
                "blood_9.png",
                "blood_10.png",
                "blood_11.png",
                "blood_12.png",
                "blood_13.png",
                "blood_14.png",
            ],
            TransientKind::Scorch => &["scorch_1.png", "scorch_2.png"],
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

const FADE_SECONDS: f32 = 3.0;
const FADE_STEPS: u8 = 32;
/// Seconds a reaped body still counts as one for the hits that killed it.
const REAPED_MEMORY: f32 = 1.0;
/// Meters across the picture, most of it soot.
const BULLET_HOLE_SIZE: f32 = 0.14;
/// Meters past the body a shot's line is followed for a surface to splash.
const BLOOD_REACH: f32 = 5.0;
/// Meters below where a killing hit landed that the floor is looked for.
const POOL_DROP: f32 = 3.0;
/// Meters across the blood a body leaves where it dies.
const POOL_SIZE: f32 = 1.3;
/// Which of the blood pictures runs down a wall rather than splashing.
const DRIP: usize = 3;
/// Meters across, at no damage and at [`BLOOD_FULL_DAMAGE`].
const BLOOD_SIZE: (f32, f32) = (0.35, 1.1);
const BLOOD_FULL_DAMAGE: f32 = 100.0;
/// A gib striking this fast, in meters per second, leaves the largest splat.
const GIB_FULL_SPLAT_SPEED: f32 = 10.0;
/// How far, in meters, from where a gib struck a surface is looked for.
const GIB_SPLAT_REACH: f32 = 0.1;
/// A scorch's size is this much of its blast's radius, within the range.
const SCORCH_SCALE: f32 = 0.9;
const SCORCH_SIZE: (f32, f32) = (0.6, 3.0);

#[derive(Resource, Default)]
pub struct DecalArt {
    transient: [Vec<Handle<StandardMaterial>>; 3],
}

impl DecalArt {
    fn pick(&self, kind: TransientKind, seed: u32) -> Option<Handle<StandardMaterial>> {
        let choices = &self.transient[kind.index()];
        (!choices.is_empty()).then(|| choices[seed as usize % choices.len()].clone())
    }
}

/// The newest transient decals of each kind, oldest first.
#[derive(Resource, Default)]
pub struct TransientDecals {
    rings: [VecDeque<Entity>; 3],
}

/// Sprays and map decals.
pub fn blended_material(image: Handle<Image>) -> StandardMaterial {
    StandardMaterial {
        base_color_texture: Some(image),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.9,
        depth_bias: DEPTH_BIAS,
        ..default()
    }
}

/// Multiplied onto the wall as already lit, so it needs no light of its own and overlapping marks
/// give the same result in any order. It can only darken.
fn multiplied_material(image: Handle<Image>) -> StandardMaterial {
    StandardMaterial {
        base_color_texture: Some(image),
        alpha_mode: AlphaMode::Multiply,
        unlit: true,
        depth_bias: DEPTH_BIAS,
        ..default()
    }
}

/// On top of each piece's lift off its wall.
const DEPTH_BIAS: f32 = 8.0;

/// A decoded picture with a full mip chain, sampled trilinear: a hole 10 cm across seen from
/// across a room is a few texels of a 128-texel picture, and without mips it sparkles.
pub fn decal_image(bytes: &[u8]) -> Result<Image, String> {
    let image = Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::RENDER_WORLD,
    )
    .map_err(|e| e.to_string())?;
    let image = image.try_into_dynamic().map_err(|e| e.to_string())?.to_rgba8();
    Ok(with_mips(image.width(), image.height(), image.into_raw()))
}

/// sRGB RGBA8 with every mip down to one texel, averaged alpha-weighted so a transparent texel's
/// color does not bleed into its neighbors.
pub fn with_mips(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    let mut levels = vec![rgba];
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let above = levels.last().unwrap();
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                    let i = ((sy * w + sx) * 4) as usize;
                    let alpha = above[i + 3] as f32 / 255.0;
                    for c in 0..3 {
                        sum[c] += above[i + c] as f32 * alpha;
                    }
                    sum[3] += alpha;
                }
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..3 {
                    next[o + c] = if sum[3] > 0.0 { (sum[c] / sum[3]).round() as u8 } else { 0 };
                }
                next[o + 3] = (sum[3] / 4.0 * 255.0).round() as u8;
            }
        }
        levels.push(next);
        (w, h) = (nw, nh);
    }
    let count = levels.len() as u32;
    // `new_uninit` because `new` checks the data against the top level alone.
    let mut image = Image::new_uninit(
        bevy::render::render_resource::Extent3d { width, height, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(levels.concat());
    image.texture_descriptor.mip_level_count = count;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

fn load_picture(assets: &crate::common::assets::Assets, name: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let path = format!("decals/{name}");
    let Some(bytes) = assets.read_from_packs(&path) else {
        warn!("No pack has {path}");
        return None;
    };
    match decal_image(&bytes) {
        Ok(image) => Some(images.add(image)),
        Err(e) => {
            warn!("{path} will not decode: {e}");
            None
        }
    }
}

fn load_decal_art(
    assets: Res<crate::common::assets::Assets>,
    mut art: ResMut<DecalArt>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for kind in TransientKind::ALL {
        art.transient[kind.index()] = kind
            .pictures()
            .iter()
            .filter_map(|name| Some(materials.add(multiplied_material(load_picture(&assets, name, &mut images)?))))
            .collect();
    }
}

/// `None` for a name no pack has, so it is not looked for every frame.
#[derive(Resource, Default)]
struct MapDecalArt(HashMap<String, Option<Handle<StandardMaterial>>>);

/// Compared before writing: `apply_to_entity` writes the spec on every edit.
fn dress_map_decals(
    mut commands: Commands,
    decals: Query<(Entity, &MapDecalSpec, Option<&Decal>, Option<&DecalMaterial>)>,
    assets: Res<crate::common::assets::Assets>,
    mut art: ResMut<MapDecalArt>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, spec, drawn, material_drawn) in &decals {
        let material = match art.0.get(&spec.picture) {
            Some(material) => material.clone(),
            None => {
                let material = load_picture(&assets, &spec.picture, &mut images).map(|picture| materials.add(blended_material(picture)));
                art.0.insert(spec.picture.clone(), material.clone());
                material
            }
        };
        // A picture no pack has draws nothing, rather than whatever it showed before.
        let Some(material) = material else {
            if drawn.is_some() {
                commands.entity(entity).despawn_related::<DecalPieces>().remove::<(Decal, DecalMaterial, Uncut)>();
            }
            continue;
        };
        if drawn.is_some_and(|drawn| drawn.0 == spec.projection) && material_drawn.is_some_and(|drawn| drawn.0 == material) {
            continue;
        }
        commands.entity(entity).insert((Decal(spec.projection), DecalMaterial(material), MapDecal, Lit, Uncut));
    }
}

/// Which slots of a `BakedRoomGeometry` take decals, in [`RoomSlot::ALL`] order. Written only when
/// it differs, so `Changed<TakesDecals>` means a repaint.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakesDecals(pub [bool; 3]);

impl TakesDecals {
    /// A room not noted yet takes them everywhere.
    fn slot(this: Option<&Self>, slot: RoomSlot) -> bool {
        this.is_none_or(|this| this.0[slot.index()])
    }

    fn face(this: Option<&Self>, face: &world_surface::Face) -> bool {
        Self::slot(this, RoomSlot::of(face.plane.normal))
    }
}

pub type Surfaces<'w, 's> = Query<'w, 's, (&'static BakedRoomGeometry, Option<&'static TakesDecals>)>;

fn note_what_takes_decals(
    mut commands: Commands,
    bakes: Query<(Entity, Ref<BakedRoomGeometry>, Option<&TakesDecals>)>,
    paint: Query<(Option<&RoomSurfaces>, Option<&Side>)>,
    palette: Option<Res<Palette>>,
    repainted: Query<(), Or<(Changed<RoomSurfaces>, Changed<Side>)>>,
    (mut unpainted, mut unsided): (RemovedComponents<RoomSurfaces>, RemovedComponents<Side>),
) {
    // Summed rather than `||`, so both are read: an unread reader keeps its removals for later.
    let removed = unpainted.read().count() + unsided.read().count() > 0;
    let everything = removed || !repainted.is_empty() || palette.as_ref().is_some_and(|palette| palette.is_changed());
    let fallback = Palette::default();
    let palette = palette.as_deref().unwrap_or(&fallback);
    for (entity, baked, noted) in &bakes {
        if !everything && noted.is_some() && !baked.is_changed() {
            continue;
        }
        let (surfaces, side) = paint.get(baked.room).unwrap_or((None, None));
        let takes = TakesDecals(palette.paint_slots(surfaces, side.copied()).map(decal::takes_decals));
        if noted != Some(&takes) {
            commands.entity(entity).insert(takes);
        }
    }
}

/// Each room's lightmap layout, kept while its faces stay the same.
#[derive(Default)]
struct Charts(HashMap<Entity, (u64, Atlas, Option<(Vec3, Vec3)>)>);

impl Charts {
    fn of(&mut self, entity: Entity, baked: &BakedRoomGeometry) -> &(u64, Atlas, Option<(Vec3, Vec3)>) {
        let stale = self.0.get(&entity).is_none_or(|(hash, ..)| *hash != baked.faces_hash);
        if stale {
            self.0.insert(entity, (baked.faces_hash, Atlas::of(&baked.faces), baked.bounds));
        }
        &self.0[&entity]
    }
}

fn cut_decals(
    mut commands: Commands,
    uncut: Query<(Entity, &Decal, &DecalMaterial, Has<Lit>, Has<Transient>), With<Uncut>>,
    bakes: Query<(Entity, &BakedRoomGeometry, Option<&TakesDecals>)>,
    mut charts: Local<Charts>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if uncut.is_empty() {
        return;
    }
    charts.0.retain(|entity, _| bakes.contains(*entity));
    for (root, Decal(projection), material, lit, transient) in &uncut {
        commands.entity(root).despawn_related::<DecalPieces>().remove::<Uncut>();
        let reach = projection.bounds();
        let mut any = false;
        for (room, baked, takes) in &bakes {
            let (faces_hash, atlas, bounds) = charts.of(room, baked);
            if !bounds.is_some_and(|bounds| decal::overlaps(bounds, reach)) {
                continue;
            }
            let piece = decal::cut(projection, &baked.faces, atlas, |slot| TakesDecals::slot(takes, slot));
            if piece.is_empty() {
                continue;
            }
            any = true;
            let mut spawned = commands.spawn((
                DecalPiece { faces_hash: *faces_hash, alphas: piece.colors.iter().map(|c| c[3]).collect(), faded: FADE_STEPS },
                DecalPieceOf(root),
                PaintedOn(room),
                Mesh3d(meshes.add(piece.mesh())),
                MeshMaterial3d(material.0.clone()),
                Transform::default(),
                Visibility::default(),
                NotShadowCaster,
            ));
            if lit {
                spawned.insert(Lit);
            }
        }
        if transient && !any {
            commands.entity(root).despawn();
        }
    }
}

/// A rebaked or new room recuts the lasting decals it touches and drops the transient ones on it.
/// A repainted room recuts every decal it touches, so transient ones keep their pieces on slots
/// that still take them.
/// A despawned room takes its pieces with it, so every lasting decal is recut, and a transient one
/// left with no piece goes.
fn follow_rebakes(
    mut commands: Commands,
    changed: Query<(Entity, Ref<BakedRoomGeometry>), Or<(Changed<BakedRoomGeometry>, Changed<TakesDecals>)>>,
    mut gone: RemovedComponents<BakedRoomGeometry>,
    decals: Query<(Entity, &Decal, Has<Transient>, Option<&DecalPieces>), Without<Uncut>>,
    painted: Query<&PaintedOn>,
) {
    let any_gone = gone.read().count() > 0;
    let (mut rebaked, mut repainted) = (Vec::new(), Vec::new());
    for (room, baked) in &changed {
        let Some(bounds) = baked.bounds else { continue };
        match baked.is_changed() {
            true => rebaked.push((room, bounds)),
            false => repainted.push(bounds),
        }
    }
    if rebaked.is_empty() && repainted.is_empty() && !any_gone {
        return;
    }
    for (root, Decal(projection), transient, pieces) in &decals {
        let on = |room: Entity| pieces.is_some_and(|pieces| pieces.iter().any(|piece| painted.get(piece).is_ok_and(|p| p.0 == room)));
        let reach = projection.bounds();
        let touched = rebaked.iter().filter(|(_, bounds)| decal::overlaps(*bounds, reach)).map(|(room, _)| *room).collect::<Vec<_>>();
        let recolored = repainted.iter().any(|bounds| decal::overlaps(*bounds, reach));
        match transient {
            true if touched.iter().any(|room| on(*room)) || pieces.is_none() => {
                commands.entity(root).despawn();
            }
            true if recolored => {
                commands.entity(root).insert(Uncut);
            }
            true => {}
            false if any_gone || recolored || !touched.is_empty() => {
                commands.entity(root).insert(Uncut);
            }
            false => {}
        }
    }
}

/// Every frame, as `attach_lightmaps` does for walls, through the same [`keep_lightmap`].
fn light_decals(
    mut commands: Commands,
    sun: Option<Res<SunBake>>,
    mut pieces: Query<(Entity, &DecalPiece, &PaintedOn, &mut MeshMaterial3d<StandardMaterial>, Option<&Lightmap>), With<Lit>>,
    bakes: Query<&BakedRoomGeometry>,
) {
    for (entity, piece, painted, mut material, drawn) in &mut pieces {
        let wanted = sun.as_ref().zip(bakes.get(painted.0).ok()).and_then(|(sun, baked)| sun.light_for(baked.room, piece.faces_hash));
        keep_lightmap(&mut commands, entity, wanted, drawn, &mut material);
    }
}

/// The faces of the rooms whose bounds meet `reach`, each with whether it takes decals.
pub fn faces_near<'a>(
    bakes: impl IntoIterator<Item = (&'a BakedRoomGeometry, Option<&'a TakesDecals>)>,
    reach: (Vec3, Vec3),
) -> impl Iterator<Item = (&'a world_surface::Face, bool)> {
    bakes
        .into_iter()
        .filter(move |(baked, _)| baked.bounds.is_some_and(|bounds| decal::overlaps(bounds, reach)))
        .flat_map(|(baked, takes)| baked.faces.iter().map(move |face| (face, TakesDecals::face(takes, face))))
}

/// `None` when the first face refuses decals; that face still stops the ray.
fn markable_hit<'a>(
    faces: impl IntoIterator<Item = (&'a world_surface::Face, bool)>,
    origin: Vec3,
    direction: Vec3,
    range: f32,
) -> Option<(Vec3, Vec3)> {
    decal::hit_tagged(faces, origin, direction, range).and_then(|(at, normal, takes)| takes.then_some((at, normal)))
}

/// `None` past `range` or where the first face refuses decals.
pub fn surface_hit(bakes: &Surfaces, origin: Vec3, direction: Vec3, range: f32) -> Option<(Vec3, Vec3)> {
    let end = origin + direction * range;
    markable_hit(faces_near(bakes, (origin.min(end), origin.max(end))), origin, direction, range)
}

/// A stable number from a place, so every machine picks the same picture and turn for one mark.
fn seed_of(point: Vec3) -> u32 {
    let mut hash = crate::common::stable_hash::Fnv::default();
    use std::hash::Hasher;
    for c in point.to_array() {
        hash.write_u32((c * 64.0).round() as i32 as u32);
    }
    hash.finish() as u32
}

fn turned(normal: Vec3, up: Vec3, seed: u32) -> Vec3 {
    Quat::from_axis_angle(normal, (seed % 360) as f32 * std::f32::consts::TAU / 360.0) * up
}

/// Meters across, from the smallest splat at a `share` of 0 to the largest at 1 or more.
fn blood_size(share: f32) -> f32 {
    BLOOD_SIZE.0 + (BLOOD_SIZE.1 - BLOOD_SIZE.0) * share.clamp(0.0, 1.0)
}

fn thrown(at: Vec3, normal: Vec3, size: f32, direction: Vec3, seed: u32) -> Projection {
    let mut projection = Projection::on(at, normal, size, 0.3, direction);
    // The splash is drawn thrown toward +u, so +u goes the way the blood went; the drip runs down
    // `v`, so on a wall it keeps world up.
    let drip = seed as usize % TransientKind::Blood.pictures().len() == DRIP;
    if !(drip && normal.y.abs() < 0.5)
        && let Some(along) = direction.reject_from(normal).try_normalize()
    {
        projection.up = normal.cross(along);
    }
    projection
}

/// Larger the harder the gib struck. `None` when no face is there.
fn gib_splat<'a>(faces: impl IntoIterator<Item = (&'a world_surface::Face, bool)>, impact: &GibImpact) -> Option<(Projection, u32)> {
    let from = impact.at + impact.normal * GIB_SPLAT_REACH;
    let (at, normal) = markable_hit(faces, from, -impact.normal, GIB_SPLAT_REACH * 2.0)?;
    let speed = impact.velocity.length();
    let share = (speed - GIB_SPLAT_SPEED) / (GIB_FULL_SPLAT_SPEED - GIB_SPLAT_SPEED);
    let size = blood_size(share);
    let seed = seed_of(at);
    Some((thrown(at, normal, size, impact.velocity.normalize_or(-normal), seed), seed))
}

/// Whether a hit landed on a body. A killing hit's body is reaped before
/// anything reads the hit, and on a client its despawn and the hit may arrive
/// frames apart, so a body counts for [`REAPED_MEMORY`] after it is gone. Call
/// [`HitBodies::catch_up`] once per run first.
#[derive(bevy::ecs::system::SystemParam)]
pub struct HitBodies<'w, 's> {
    bodies: Query<'w, 's, (), With<Skeleton>>,
    reaped: RemovedComponents<'w, 's, Skeleton>,
    lately_reaped: Local<'s, Vec<(Entity, f32)>>,
    time: Res<'w, Time>,
}

impl HitBodies<'_, '_> {
    pub fn catch_up(&mut self) {
        let now = self.time.elapsed_secs();
        self.lately_reaped.retain(|(_, when)| now - *when < REAPED_MEMORY);
        let reaped: Vec<Entity> = self.reaped.read().collect();
        self.lately_reaped.extend(reaped.into_iter().map(|body| (body, now)));
    }

    pub fn is_a_body(&self, target: Entity) -> bool {
        self.bodies.contains(target) || self.lately_reaped.iter().any(|(body, _)| *body == target)
    }
}

/// From what every machine already receives, so nothing new is sent and each machine keeps its own.
#[allow(clippy::too_many_arguments)]
fn mark_the_world(
    mut commands: Commands,
    mut effects: MessageReader<Effect>,
    mut dealt: MessageReader<DamageDealt>,
    mut gibs: MessageReader<GibImpact>,
    mut hit_bodies: HitBodies,
    bakes: Surfaces,
    art: Res<DecalArt>,
    mut rings: ResMut<TransientDecals>,
    settings: Option<Res<PlayerSettings>>,
    alive: Query<(), With<Transient>>,
    time: Res<Time>,
) {
    let limits = settings.map(|settings| settings.debris).unwrap_or_default();
    let mut marks: Vec<(TransientKind, Projection, u32)> = Vec::new();

    for effect in effects.read() {
        match *effect {
            Effect::Tracer { from, to, hit: false, .. } => {
                let Some(direction) = (to - from).try_normalize() else { continue };
                if let Some((at, normal)) = surface_hit(&bakes, to - direction * 0.05, direction, 0.1) {
                    let seed = seed_of(at);
                    let mut projection = Projection::on(at, normal, BULLET_HOLE_SIZE, 0.06, direction);
                    projection.up = turned(normal, projection.up, seed);
                    marks.push((TransientKind::BulletHole, projection, seed));
                }
            }
            Effect::Blast { at, radius } => {
                // The nearest face that takes one, so a blast under a sky still scorches the floor.
                let nearest = faces_near(&bakes, (at - Vec3::splat(radius), at + Vec3::splat(radius)))
                    .filter_map(|(face, takes)| takes.then_some(face))
                    .filter_map(|face| decal::hit([face], at, -face.plane.normal, radius).map(|(point, normal)| (point.distance(at), point, normal)))
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, point, normal)) = nearest {
                    let seed = seed_of(point);
                    let size = (radius * SCORCH_SCALE).clamp(SCORCH_SIZE.0, SCORCH_SIZE.1);
                    let mut projection = Projection::on(point, normal, size, 0.5, Vec3::Z);
                    projection.up = turned(normal, projection.up, seed);
                    marks.push((TransientKind::Scorch, projection, seed));
                }
            }
            _ => {}
        }
    }

    let now = time.elapsed_secs();
    hit_bodies.catch_up();
    for hit in dealt.read() {
        if hit.amount == 0 || !hit_bodies.is_a_body(hit.target) {
            continue;
        }
        if hit.remaining == 0
            && let Some((at, normal)) = surface_hit(&bakes, hit.point, Vec3::NEG_Y, POOL_DROP)
        {
            let mut seed = seed_of(at);
            if seed as usize % TransientKind::Blood.pictures().len() == DRIP {
                seed += 1;
            }
            let mut projection = Projection::on(at, normal, POOL_SIZE, 0.3, Vec3::Z);
            projection.up = turned(normal, projection.up, seed);
            marks.push((TransientKind::Blood, projection, seed));
        }

        if !matches!(hit.cause.how, How::Hitscan | How::Splash) {
            continue;
        }
        // A blast's line runs out from its center. A projectile's direct hit has none.
        let Some(direction) = hit.from.and_then(|from| (hit.point - from).try_normalize()) else { continue };
        let Some((at, normal)) = surface_hit(&bakes, hit.point, direction, BLOOD_REACH) else { continue };
        let size = blood_size(hit.amount as f32 / BLOOD_FULL_DAMAGE);
        let seed = seed_of(at);
        marks.push((TransientKind::Blood, thrown(at, normal, size, direction, seed), seed));
    }

    for impact in gibs.read() {
        let reach = (impact.at - Vec3::splat(GIB_SPLAT_REACH), impact.at + Vec3::splat(GIB_SPLAT_REACH));
        if let Some(mark) = gib_splat(faces_near(&bakes, reach), impact) {
            marks.push((TransientKind::Blood, mark.0, mark.1));
        }
    }

    // Dead entries would count against the cap, so they go before a push or a trim. The query
    // cannot see this frame's marks, so before those are pushed.
    let over = TransientKind::ALL.iter().any(|kind| rings.rings[kind.index()].len() > kind.cap(&limits));
    if !marks.is_empty() || over {
        for ring in &mut rings.rings {
            ring.retain(|root| alive.contains(*root));
        }
    }
    for (kind, projection, seed) in marks {
        if kind.cap(&limits) == 0 {
            continue;
        }
        let Some(material) = art.pick(kind, seed) else { continue };
        let root = commands.spawn((Decal(projection), DecalMaterial(material), Transient { kind, born: now })).id();
        rings.rings[kind.index()].push_back(root);
    }
    // Every frame rather than only on a push, so lowering a limit takes effect at once.
    for kind in TransientKind::ALL {
        let ring = &mut rings.rings[kind.index()];
        while ring.len() > kind.cap(&limits) {
            if let Some(oldest) = ring.pop_front() {
                commands.entity(oldest).try_despawn();
            }
        }
    }
}

fn age_transients(
    mut commands: Commands,
    time: Res<Time>,
    decals: Query<(Entity, &Transient, Option<&DecalPieces>)>,
    mut pieces: Query<(&mut DecalPiece, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let now = time.elapsed_secs();
    for (root, transient, owned) in &decals {
        let left = transient.kind.lifetime() - (now - transient.born);
        if left <= 0.0 {
            commands.entity(root).despawn();
            continue;
        }
        if left >= FADE_SECONDS {
            continue;
        }
        let step = ((left / FADE_SECONDS) * FADE_STEPS as f32).ceil().clamp(1.0, FADE_STEPS as f32) as u8;
        let strength = step as f32 / FADE_STEPS as f32;
        for piece in owned.into_iter().flat_map(|owned| owned.iter()) {
            let Ok((mut piece, mesh)) = pieces.get_mut(piece) else { continue };
            if piece.faded == step {
                continue;
            }
            piece.faded = step;
            let Some(mut mesh) = meshes.get_mut(&mesh.0) else { continue };
            if let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR) {
                for (color, alpha) in colors.iter_mut().zip(&piece.alphas) {
                    color[3] = alpha * strength;
                }
            }
        }
    }
}

/// Every decal material, with and without a lightmap, for [`crate::game::warm_up`].
fn warm_up_decals(
    mut commands: Commands,
    art: Res<DecalArt>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};

    let layer = RenderLayers::layer(crate::common::render_layers::WARM_UP);
    // Laid out as a cut piece is, with all three corners in one place so nothing is rasterized.
    let point = decal::Piece {
        positions: vec![[0.0, 0.0, -1.0]; 3],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.5, 0.5]; 3],
        lightmap_uvs: vec![[0.5, 0.5]; 3],
        colors: vec![[1.0; 4]; 3],
        indices: vec![0, 1, 2],
    };
    let mesh = meshes.add(point.mesh());
    let blended = materials.add(blended_material(images.add(Image::default())));
    let lightmap = images.add(Image::default());
    let multiplied = art.transient.iter().flatten().cloned();
    for (material, lit) in multiplied.map(|m| (m, false)).chain([(blended.clone(), false), (blended, true)]) {
        let mut piece = commands.spawn((
            crate::game::warm_up::WarmUp,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            layer.clone(),
            NoFrustumCulling,
            NotShadowCaster,
            Transform::default(),
            Visibility::default(),
        ));
        if lit {
            piece.insert(Lightmap { image: lightmap.clone(), uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0), bicubic_sampling: false });
        }
    }
}

/// On entering and leaving Play: transient marks belong to a round.
fn clear_transients(mut commands: Commands, decals: Query<Entity, With<Transient>>, mut rings: ResMut<TransientDecals>) {
    for root in &decals {
        commands.entity(root).despawn();
    }
    *rings = TransientDecals::default();
}

/// Added in `launch.rs` beside `WorldPropPlugin`: it builds meshes and materials, which the game's
/// tests have no assets for.
pub struct DecalPlugin;

impl Plugin for DecalPlugin {
    fn build(&self, app: &mut App) {
        crate::game::warm_up::ensure(app);
        app.init_resource::<DecalArt>()
            .init_resource::<TransientDecals>()
            .init_resource::<MapDecalArt>()
            // Also registered by `RagdollPlugin`; here so decals work without it.
            .add_message::<GibImpact>()
            .add_systems(Startup, (load_decal_art, warm_up_decals).chain())
            // Ungated: the map's decals and sprays stand in the editor too, and the map can
            // change during a round.
            .add_systems(
                Update,
                (
                    dress_map_decals,
                    mark_the_world,
                    age_transients,
                    note_what_takes_decals,
                    follow_rebakes,
                    cut_decals,
                    light_decals,
                )
                    .chain()
                    .after(crate::tool::bakes::BakePlugin::auto_bake_rooms),
            )
            .add_systems(OnEnter(AppMode::Play), clear_transients)
            .add_systems(OnExit(AppMode::Play), clear_transients);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_room() -> Vec<world_surface::Face> {
        world_surface::of_rooms(&[crate::tool::room::Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))])
    }

    fn taking(faces: &[world_surface::Face]) -> impl Iterator<Item = (&world_surface::Face, bool)> {
        faces.iter().map(|face| (face, true))
    }

    /// Fails if a gib striking a floor leaves no blood there, or a harder strike a smaller splat.
    #[test]
    fn a_gib_leaves_blood_where_it_strikes_and_more_the_harder() {
        let faces = a_room();
        let strike = |speed: f32| GibImpact { at: Vec3::new(2.0, 0.0, 2.0), normal: Vec3::Y, velocity: Vec3::new(1.0, -speed, 0.0) };

        let (soft, _) = gib_splat(taking(&faces), &strike(3.0)).expect("no blood on the floor");
        let (hard, _) = gib_splat(taking(&faces), &strike(12.0)).expect("no blood on the floor");
        assert!(soft.center.distance(Vec3::new(2.0, 0.0, 2.0)) < 0.01, "blood at {}", soft.center);
        assert!(soft.normal.abs_diff_eq(Vec3::Y, 1.0e-5));
        assert!(hard.size.x > soft.size.x, "{} is not larger than {}", hard.size.x, soft.size.x);
    }

    /// Fails if a gib struck somewhere with no baked face leaves blood floating in the air.
    #[test]
    fn a_gib_leaves_no_blood_where_there_is_no_surface() {
        let faces = a_room();
        let impact = GibImpact { at: Vec3::new(2.0, 1.5, 2.0), normal: Vec3::Y, velocity: Vec3::NEG_Y * 5.0 };
        assert!(gib_splat(taking(&faces), &impact).is_none());
    }

    /// Fails if a gib leaves blood on a floor that refuses decals, or the ray goes on past it.
    #[test]
    fn a_gib_leaves_no_blood_on_a_floor_that_refuses_it() {
        let faces = a_room();
        let impact = GibImpact { at: Vec3::new(2.0, 0.0, 2.0), normal: Vec3::Y, velocity: Vec3::NEG_Y * 12.0 };
        let refusing = faces.iter().map(|face| (face, RoomSlot::of(face.plane.normal) != RoomSlot::Floor));
        assert!(gib_splat(refusing, &impact).is_none());
    }

    /// Fails if a slot painted sky is not noted as refusing decals, or a repaint is not noted.
    #[test]
    fn a_sky_slot_refuses_decals_until_it_is_repainted() {
        let mut app = App::new();
        app.add_systems(Update, note_what_takes_decals);
        let surfaces = RoomSurfaces { ceiling: Some(crate::common::sky::SKYBOX.into()), ..default() };
        let room = app.world_mut().spawn(surfaces).id();
        let faces = a_room();
        let baked = BakedRoomGeometry { room, faces_hash: 0, bounds: decal::bounds_of(&faces), faces, air: None };
        let bake = app.world_mut().spawn(baked).id();

        app.update();
        assert_eq!(app.world().get::<TakesDecals>(bake), Some(&TakesDecals([true, true, false])));

        app.world_mut().get_mut::<RoomSurfaces>(room).unwrap().ceiling = None;
        app.update();
        assert_eq!(app.world().get::<TakesDecals>(bake), Some(&TakesDecals([true, true, true])));
    }

    /// Fails if painting a ceiling sky leaves the decals on it, takes away the ones on its walls,
    /// or painting it back does not put them back.
    #[test]
    fn repainting_a_slot_sky_recuts_only_what_lies_on_it() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .add_systems(Update, (note_what_takes_decals, follow_rebakes, cut_decals).chain());
        let room = app.world_mut().spawn(RoomSurfaces::default()).id();
        let faces = a_room();
        let baked = BakedRoomGeometry { room, faces_hash: 0, bounds: decal::bounds_of(&faces), faces, air: None };
        app.world_mut().spawn(baked);
        let mark = |at: Vec3, normal: Vec3| (Decal(Projection::on(at, normal, 0.5, 0.2, -normal)), DecalMaterial(default()));
        let ceiling = app.world_mut().spawn(mark(Vec3::new(2.0, 3.0, 2.0), Vec3::NEG_Y)).id();
        let wall = app.world_mut().spawn((mark(Vec3::new(2.0, 1.5, 0.0), Vec3::Z), Transient { kind: TransientKind::BulletHole, born: 0.0 })).id();
        let pieces = |app: &App, root: Entity| app.world().get::<DecalPieces>(root).map_or(0, |pieces| pieces.len());

        app.update();
        assert_eq!((pieces(&app, ceiling), pieces(&app, wall)), (1, 1));

        app.world_mut().get_mut::<RoomSurfaces>(room).unwrap().ceiling = Some(crate::common::sky::SKYBOX.into());
        app.update();
        app.update();
        assert_eq!(pieces(&app, ceiling), 0);
        assert!(app.world().get_entity(wall).is_ok() && pieces(&app, wall) == 1, "the wall's bullet hole went");

        app.world_mut().get_mut::<RoomSurfaces>(room).unwrap().ceiling = None;
        app.update();
        app.update();
        assert_eq!(pieces(&app, ceiling), 1);
    }

    #[test]
    fn every_mip_halves_down_to_one_texel() {
        let image = with_mips(8, 4, vec![255; 8 * 4 * 4]);
        assert_eq!(image.texture_descriptor.mip_level_count, 4);
        assert_eq!(image.data.as_ref().unwrap().len(), (32 + 8 + 2 + 1) * 4);
    }

    #[test]
    fn a_transparent_texel_does_not_darken_its_mip() {
        // One white opaque texel beside three black transparent ones.
        let mut rgba = vec![0u8; 2 * 2 * 4];
        rgba[..4].copy_from_slice(&[255, 255, 255, 255]);
        let image = with_mips(2, 2, rgba);
        let data = image.data.as_ref().unwrap();
        assert_eq!(&data[16..20], &[255, 255, 255, 64]);
    }

    #[test]
    fn every_shipped_decal_picture_decodes() {
        let assets = crate::common::assets::Assets::default();
        for kind in TransientKind::ALL {
            for name in kind.pictures() {
                let bytes = assets.read_from_packs(&format!("decals/{name}")).unwrap_or_else(|| panic!("{name} is missing"));
                decal_image(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
    }
}
