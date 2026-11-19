//! The flash at a weapon's muzzle on each shot: four nested crowns, each a
//! truncated cone along the weapon's `-Z` from a ring at the muzzle to a star
//! of points.
//!
//! One mesh holds all four, outer first in its index buffer, drawn blended with
//! no depth write and no culling. A GPU blends one draw call's triangles in
//! index order, so each inner layer lands on the one outside it from any angle,
//! while the world's depth still hides the flash behind a wall.
//!
//! Lit by [`ShotSeen`], as recoil is, so nothing is sent: once under the body's
//! held weapon, and once under the viewmodel while that body is viewed.

use bevy::asset::embedded_asset;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::asset::RenderAssetUsages;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::common::app_mode::AppMode;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::ClipStack;
use crate::game::held::{place_held_weapons, HeldModel, HeldPart, WeaponInHand};
use crate::prop::rig::place_part;
use crate::game::player::{LocalPlayer, Watched};
use crate::game::recoil::{start_recoil, ShotSeen};
use crate::game::viewmodel::{Viewmodel, ViewmodelCamera, VIEWMODEL_LAYER};
use crate::game::warm_up::{self, WarmUp};
use crate::prop::hold::MuzzleFlashSpec;

const SHADER: &str = "embedded://grackle/game/muzzle_flash.wesl";

/// The scale of a flash's progress in its `MeshTag`. Matches the shader's.
pub const PROGRESS_STEPS: f32 = 65535.0;

/// Share of a flash its points take to reach the crown. Matches the shader's.
pub const FULL: f32 = 0.3;

/// Each flash is turned about the barrel by a random angle and scaled by up to
/// this share either way, so repeated shots differ.
const SIZE_SPREAD: f32 = 0.12;

/// One layer of the crown, in shares of the outer layer's tip length and
/// radius.
struct Layer {
    points: u32,
    tip_length: f32,
    tip_radius: f32,
    valley_length: f32,
    valley_radius: f32,
    /// Of a point's spacing, so neighboring layers' points interleave.
    turn: f32,
    seed: u32,
}

/// Outer first: the order they are drawn in.
const LAYERS: [Layer; 4] = [
    Layer { points: 9, tip_length: 1.0, tip_radius: 1.0, valley_length: 0.62, valley_radius: 0.48, turn: 0.0, seed: 1 },
    Layer { points: 9, tip_length: 0.8, tip_radius: 0.77, valley_length: 0.5, valley_radius: 0.37, turn: 0.5, seed: 2 },
    Layer { points: 7, tip_length: 0.58, tip_radius: 0.55, valley_length: 0.36, valley_radius: 0.26, turn: 0.25, seed: 3 },
    Layer { points: 6, tip_length: 0.32, tip_radius: 0.29, valley_length: 0.2, valley_radius: 0.145, turn: 0.1, seed: 4 },
];

/// The shader picks each layer's color.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug, Default)]
pub struct MuzzleFlashMaterial {}

impl Material for MuzzleFlashMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    // Blended for its draw order and lack of depth writes; every fragment it
    // keeps is opaque.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // From inside a cone, its far wall is a layer too.
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// 0 to 1, the same on every machine, so a prop's crown is too.
fn noise(seed: u32, n: u32) -> f32 {
    crate::game::flame_jet::noise(seed.wrapping_mul(1_000_003).wrapping_add(n))
}

fn between(seed: u32, n: u32, low: f32, high: f32) -> f32 {
    low + (high - low) * noise(seed, n)
}

/// The crown's rim for one layer, around `-Z`: points and valleys alternating,
/// in meters.
fn rim(layer: &Layer, spec: &MuzzleFlashSpec) -> Vec<Vec3> {
    let corners = layer.points * 2;
    (0..corners)
        .map(|i| {
            let jitter = between(layer.seed, i * 3, -0.18, 0.18);
            let angle = (i as f32 + layer.turn * 2.0 + jitter) * std::f32::consts::PI / layer.points as f32;
            let (length, radius) = match i % 2 == 0 {
                true => (
                    layer.tip_length * between(layer.seed, i * 3 + 1, 0.9, 1.08),
                    layer.tip_radius * between(layer.seed, i * 3 + 2, 0.85, 1.1),
                ),
                false => {
                    let k = between(layer.seed, i * 3 + 1, 0.85, 1.1);
                    (layer.valley_length * k, layer.valley_radius * k)
                }
            };
            Vec3::new(radius * spec.radius * angle.cos(), radius * spec.radius * angle.sin(), -length * spec.length)
        })
        .collect()
}

/// All four layers in one mesh, outer first. `UV_0` is how far along its cone
/// each vertex is (0 at the muzzle ring, 1 at the rim) and which layer it is
/// on.
pub fn crown_mesh(spec: &MuzzleFlashSpec) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (index, layer) in LAYERS.iter().enumerate() {
        let layer_uv = index as f32;
        let rim = rim(layer, spec);
        let n = rim.len() as u32;
        let first = positions.len() as u32;
        let ring = spec.base * layer.tip_radius;
        // Ring then rim, one pair per corner, so a quad is two of each.
        for corner in &rim {
            let outward = Vec2::new(corner.x, corner.y).normalize_or_zero() * ring;
            positions.push([outward.x, outward.y, 0.0]);
            uvs.push([0.0, layer_uv]);
            positions.push(corner.to_array());
            uvs.push([1.0, layer_uv]);
        }
        let center = positions.len() as u32;
        positions.push([0.0, 0.0, 0.0]);
        uvs.push([0.0, layer_uv]);
        for i in 0..n {
            let (b0, r0) = (first + i * 2, first + i * 2 + 1);
            let j = (i + 1) % n;
            let (b1, r1) = (first + j * 2, first + j * 2 + 1);
            indices.extend_from_slice(&[b0, r0, r1, b0, r1, b1]);
            // The muzzle's cap, seen only down the barrel.
            indices.extend_from_slice(&[center, b1, b0]);
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// One mesh per size, since a prop's crown is fixed and many bodies share one.
#[derive(Resource, Default)]
struct CrownMeshes(HashMap<[u32; 3], Handle<Mesh>>);

impl CrownMeshes {
    fn get(&mut self, spec: &MuzzleFlashSpec, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        let key = [spec.length, spec.radius, spec.base].map(f32::to_bits);
        self.0.entry(key).or_insert_with(|| meshes.add(crown_mesh(spec))).clone()
    }
}

#[derive(Resource)]
struct FlashLook {
    material: Handle<MuzzleFlashMaterial>,
}

#[derive(Clone, Copy, Debug)]
enum Hanging {
    /// A child of this body, placed by its [`WeaponInHand`].
    Held { body: Entity },
    /// A child of the viewmodel camera, placed by its [`Viewmodel`] and this
    /// body's prop rig.
    Viewmodel { body: Entity },
}

#[derive(Component)]
struct MuzzleFlash {
    hanging: Hanging,
    /// In the weapon's own space: the muzzle, the roll and the size jitter.
    frame: Transform,
    seconds: f32,
    age: f32,
}

/// The Fit workspace's flash at the prop's muzzle.
#[derive(Component)]
struct PreviewFlash;

/// Seconds with no flash between continuous plays.
const PREVIEW_GAP: f32 = 0.4;

/// How the Fit workspace previews the flash, set from the Hold panel. View
/// state: not saved, not undone.
#[derive(Resource, Default)]
pub struct FlashPreview {
    pub shown: bool,
    /// One moment, picked with [`clock`](Self::clock), rather than playing on a loop.
    pub still: bool,
    pub paused: bool,
    /// Seconds into the loop: the flash's own seconds, then [`PREVIEW_GAP`].
    pub clock: f32,
}

impl FlashPreview {
    /// Keeps the moment on show: a still picked from the gap is the flash's end.
    pub fn set_still(&mut self, still: bool, seconds: f32) {
        self.still = still;
        self.clock = self.clock.min(seconds);
    }

    fn advance(&mut self, seconds: f32, delta: f32) {
        if self.still {
            self.clock = self.clock.clamp(0.0, seconds);
        } else if !self.paused {
            self.clock = (self.clock + delta) % (seconds + PREVIEW_GAP);
        }
    }

    /// `None` in the gap between plays.
    fn progress(&self, seconds: f32) -> Option<f32> {
        (self.clock < seconds || self.still).then(|| (self.clock / seconds).min(1.0))
    }
}

pub struct MuzzleFlashPlugin;

impl Plugin for MuzzleFlashPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "muzzle_flash.wesl");
        warm_up::ensure(app);
        app.add_plugins(MaterialPlugin::<MuzzleFlashMaterial>::default())
            .init_resource::<CrownMeshes>()
            .add_systems(Startup, (make_the_look, warm_up).chain())
            .add_systems(
                Update,
                light_flashes
                    .after(start_recoil)
                    .run_if(crate::common::app_mode::simulating),
            )
            // Ungated, so a flash lit as a round ends still finishes and goes.
            .add_systems(
                PostUpdate,
                (age_flashes, preview_in_fit)
                    .after(place_held_weapons)
                    .before(TransformSystems::Propagate),
            );
    }
}

fn make_the_look(mut commands: Commands, mut materials: ResMut<Assets<MuzzleFlashMaterial>>) {
    commands.insert_resource(FlashLook { material: materials.add(MuzzleFlashMaterial {}) });
}

fn warm_up(
    mut commands: Commands,
    look: Res<FlashLook>,
    mut crowns: ResMut<CrownMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    commands.spawn((
        WarmUp,
        Mesh3d(crowns.get(&MuzzleFlashSpec::default(), &mut meshes)),
        MeshMaterial3d(look.material.clone()),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        NoFrustumCulling,
        NotShadowCaster,
        MeshTag::new(progress_tag(FULL)),
        // Too small to cover a pixel of the 4×4 target.
        Transform::from_xyz(0.0, 0.0, -1.0).with_scale(Vec3::splat(1.0e-4)),
        Visibility::default(),
    ));
}

fn progress_tag(progress: f32) -> u32 {
    (progress.clamp(0.0, 1.0) * PROGRESS_STEPS).round() as u32
}

fn flash_frame(muzzle: Vec3, seed: u32) -> Transform {
    let roll = noise(seed, 0) * std::f32::consts::TAU;
    let size = 1.0 + (noise(seed, 1) * 2.0 - 1.0) * SIZE_SPREAD;
    Transform::from_translation(muzzle)
        .with_rotation(Quat::from_rotation_z(roll))
        .with_scale(Vec3::splat(size))
}

#[allow(clippy::too_many_arguments)]
fn light_flashes(
    mut commands: Commands,
    mut seen: MessageReader<ShotSeen>,
    look: Res<FlashLook>,
    mut crowns: ResMut<CrownMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    bodies: Query<(&HeldModel, Has<LocalPlayer>, Has<Watched>)>,
    parts: Query<(&Visibility, Option<&RenderLayers>), With<HeldPart>>,
    cameras: Query<(Entity, &Camera), With<ViewmodelCamera>>,
    mut lit: Local<u32>,
) {
    for shot in seen.read() {
        let Ok((held, ours, watched)) = bodies.get(shot.body) else { continue };
        let viewed = ours || watched;
        let (Some(muzzle), Some(spec)) = (held.muzzle(), held.muzzle_flash()) else { continue };
        // A weapon that is not drawn (humiliated, taunting) does not flash.
        let Some((visibility, layers)) = held.parts().first().and_then(|part| parts.get(*part).ok()) else {
            continue;
        };
        if *visibility == Visibility::Hidden {
            continue;
        }
        *lit = lit.wrapping_add(1);
        let frame = flash_frame(muzzle, *lit);
        let mesh = crowns.get(&spec, &mut meshes);
        let flash = |hanging| {
            (
                MuzzleFlash { hanging, frame, seconds: spec.seconds.max(0.01), age: 0.0 },
                Mesh3d(mesh.clone()),
                MeshMaterial3d(look.material.clone()),
                NotShadowCaster,
                MeshTag::new(0),
                Transform::from_scale(Vec3::ZERO),
            )
        };
        commands.entity(shot.body).with_child((
            flash(Hanging::Held { body: shot.body }),
            *visibility,
            layers.cloned().unwrap_or_default(),
        ));
        if viewed {
            for (camera, settings) in &cameras {
                if settings.is_active {
                    commands.entity(camera).with_child((
                        flash(Hanging::Viewmodel { body: shot.body }),
                        RenderLayers::layer(VIEWMODEL_LAYER),
                        Visibility::Inherited,
                    ));
                }
            }
        }
    }
}

fn age_flashes(
    mut commands: Commands,
    time: Res<Time>,
    clips: Res<Assets<Clip>>,
    weapons: Query<(&WeaponInHand, &HeldModel, &ClipStack)>,
    parts: Query<&RenderLayers, (With<HeldPart>, Without<MuzzleFlash>)>,
    cameras: Query<&ChildOf, With<ViewmodelCamera>>,
    views: Query<(&Viewmodel, &ClipStack)>,
    mut flashes: Query<(Entity, &mut MuzzleFlash, &ChildOf, &mut Transform, &mut MeshTag, &mut RenderLayers)>,
) {
    for (entity, mut flash, parent, mut transform, mut tag, mut layers) in &mut flashes {
        flash.age += time.delta_secs();
        if flash.age >= flash.seconds {
            commands.entity(entity).despawn();
            continue;
        }
        let weapon = match flash.hanging {
            // The held parts' layers change as the view goes in or out of the head.
            Hanging::Held { body } => weapons.get(body).ok().map(|(weapon, held, stack)| {
                if let Some(wanted) = held.parts().first().and_then(|part| parts.get(*part).ok()) {
                    layers.set_if_neq(wanted.clone());
                }
                on_its_bone(weapon.0, held, stack, &clips)
            }),
            Hanging::Viewmodel { body } => {
                let view = cameras.get(parent.parent()).ok().and_then(|camera| views.get(camera.parent()).ok());
                view.zip(weapons.get(body).ok()).and_then(|((view, stack), (_, held, _))| {
                    Some(on_its_bone(view.weapon()?, held, stack, &clips))
                })
            }
        };
        let Some(weapon) = weapon else {
            commands.entity(entity).despawn();
            continue;
        };
        transform.set_if_neq(weapon * flash.frame);
        tag.set_if_neq(MeshTag::new(progress_tag(flash.age / flash.seconds)));
    }
}

/// `weapon` moved by the muzzle's bone, as `stack` poses it.
fn on_its_bone(weapon: Transform, held: &HeldModel, stack: &ClipStack, clips: &Assets<Clip>) -> Transform {
    let Some(bone) = held.muzzle_bone() else { return weapon };
    place_part(weapon, &held.rig().deltas_for(stack, clips), Some(&bone))
}

#[allow(clippy::too_many_arguments)]
fn preview_in_fit(
    mut commands: Commands,
    time: Res<Time>,
    mode: Res<State<AppMode>>,
    editor: Option<Res<crate::prop::document::PropEditor>>,
    fitting: Option<Res<crate::prop::gizmo::HoldHandles>>,
    preview: Option<ResMut<FlashPreview>>,
    look: Res<FlashLook>,
    mut meshes: ResMut<Assets<Mesh>>,
    // Its own, not `CrownMeshes`: a drag passes through hundreds of sizes, and
    // replacing the handle frees the last one.
    mut crown: Local<Option<([u32; 3], Handle<Mesh>)>>,
    stages: Query<Entity, With<crate::prop::view::PropStage>>,
    mut previews: Query<(Entity, &ChildOf, &mut Mesh3d, &mut Transform, &mut MeshTag, &mut Visibility), With<PreviewFlash>>,
) {
    let mut preview = preview.filter(|preview| preview.shown);
    let wanted = (*mode.get() == AppMode::Prop && fitting.is_some_and(|fitting| fitting.0))
        .then(|| editor.and_then(|editor| editor.doc().muzzle.clone()))
        .flatten()
        .and_then(|muzzle| Some((muzzle.point(), muzzle.flash()?)))
        .zip(stages.single().ok());
    let (Some(((muzzle, spec), stage)), Some(preview)) = (wanted, preview.as_mut()) else {
        for (entity, ..) in &previews {
            commands.entity(entity).despawn();
        }
        *crown = None;
        return;
    };
    let key = [spec.length, spec.radius, spec.base].map(f32::to_bits);
    if crown.as_ref().is_none_or(|(built, _)| *built != key) {
        *crown = Some((key, meshes.add(crown_mesh(&spec))));
    }
    let Some((_, mesh)) = crown.as_ref() else { return };
    let mesh = mesh.clone();
    let seconds = spec.seconds.max(0.01);
    preview.advance(seconds, time.delta_secs());
    let progress = preview.progress(seconds);
    let frame = Transform::from_translation(muzzle);
    let mut found = false;
    for (entity, parent, mut drawn, mut transform, mut tag, mut visibility) in &mut previews {
        if parent.parent() != stage || found {
            commands.entity(entity).despawn();
            continue;
        }
        found = true;
        if drawn.0 != mesh {
            drawn.0 = mesh.clone();
        }
        transform.set_if_neq(frame);
        tag.set_if_neq(MeshTag::new(progress_tag(progress.unwrap_or(0.0))));
        visibility.set_if_neq(match progress {
            Some(_) => Visibility::Inherited,
            None => Visibility::Hidden,
        });
    }
    if !found {
        commands.entity(stage).with_child((
            PreviewFlash,
            Mesh3d(mesh),
            MeshMaterial3d(look.material.clone()),
            NotShadowCaster,
            MeshTag::new(progress_tag(FULL)),
            frame,
            Visibility::Inherited,
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::render_layers::{INSIDE_HEAD, WORLD};
    use crate::prop::hold::MuzzleSpec;

    const MUZZLE: Vec3 = Vec3::new(0.0, 0.1, -0.4);

    struct Range {
        world: World,
        body: Entity,
        part: Entity,
        camera: Entity,
    }

    /// A body holding a flashing weapon, seen from inside its head, and a
    /// viewmodel camera.
    fn range(viewed: bool, shown: Visibility) -> Range {
        range_on(viewed, shown, None, crate::prop::rig::PropRig::default())
    }

    fn range_on(viewed: bool, shown: Visibility, bone: Option<&str>, rig: crate::prop::rig::PropRig) -> Range {
        let mut world = World::new();
        world.init_resource::<Messages<ShotSeen>>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<Clip>>();
        world.init_resource::<CrownMeshes>();
        world.insert_resource(FlashLook { material: Handle::default() });
        world.insert_resource(Time::<()>::default());

        let body = world.spawn(WeaponInHand(Transform::from_xyz(1.0, 2.0, 3.0))).id();
        if viewed {
            world.entity_mut(body).insert(LocalPlayer);
        }
        let part = world.spawn((HeldPart, shown, RenderLayers::layer(INSIDE_HEAD), ChildOf(body))).id();
        let bone = bone.map(str::to_string);
        let muzzle = MuzzleSpec { at: MUZZLE.to_array(), bone, flash: Some(MuzzleFlashSpec::default()) };
        world.entity_mut(body).insert(HeldModel::with_parts_and_muzzle(vec![part], muzzle, rig));

        let view = world.spawn((Viewmodel::default(), ClipStack::default())).id();
        let camera = world.spawn((ViewmodelCamera, Camera::default(), ChildOf(view))).id();
        Range { world, body, part, camera }
    }

    fn flashes_under(world: &mut World, parent: Entity) -> Vec<Entity> {
        let mut flashes = world.query_filtered::<(Entity, &ChildOf), With<MuzzleFlash>>();
        flashes.iter(world).filter(|(_, of)| of.parent() == parent).map(|(entity, _)| entity).collect()
    }

    fn shoot(range: &mut Range) {
        range.world.write_message(ShotSeen { body: range.body, button: crate::game::weapon::Button::Primary });
        range.world.run_system_once(light_flashes).unwrap();
    }

    fn step(world: &mut World, seconds: f32) {
        world.resource_mut::<Time>().advance_by(Duration::from_secs_f32(seconds));
        world.run_system_once(age_flashes).unwrap();
    }

    #[test]
    fn a_shot_flashes_on_the_body_and_on_the_viewmodel_only_while_viewed() {
        let mut viewed = range(true, Visibility::Inherited);
        shoot(&mut viewed);
        assert_eq!(flashes_under(&mut viewed.world, viewed.body).len(), 1);
        assert_eq!(flashes_under(&mut viewed.world, viewed.camera).len(), 1);

        let mut seen = range(false, Visibility::Inherited);
        shoot(&mut seen);
        assert_eq!(flashes_under(&mut seen.world, seen.body).len(), 1);
        assert!(flashes_under(&mut seen.world, seen.camera).is_empty(), "somebody else's shot on our viewmodel");
    }

    #[test]
    fn a_flash_on_a_bone_moves_with_it() {
        use crate::common::skeleton::clip::{Channel, Key, Track};
        use crate::common::skeleton::interpolation::Interpolation;
        use crate::common::skeleton::play::{Layer, Playing};

        let back = Vec3::new(0.0, 0.0, 0.05);
        let rig = crate::prop::rig::PropRig::of(&[crate::prop::rig::PropBone::new("barrel", Vec3::ZERO)]);
        let mut range = range_on(false, Visibility::Inherited, Some("barrel"), rig);
        let mut clip = Clip { length: 1.0, ..default() };
        clip.tracks.push(Track::of(Channel::PropBoneOffset {
            bone: "barrel".into(),
            keys: vec![Key::all(0.0, back.to_array(), Interpolation::Linear)],
        }));
        let handle = range.world.resource_mut::<Assets<Clip>>().add(clip);
        range.world.entity_mut(range.body).insert(ClipStack(vec![Playing::new(Layer::Weapon, handle, 0.0)]));

        shoot(&mut range);
        let flash = flashes_under(&mut range.world, range.body)[0];
        step(&mut range.world, 0.01);
        let weapon = range.world.get::<WeaponInHand>(range.body).unwrap().0;
        let placed = range.world.get::<Transform>(flash).unwrap().translation;
        assert!(placed.distance(weapon.transform_point(MUZZLE + back)) < 1e-5, "left behind by its barrel: {placed}");
    }

    #[test]
    fn a_hidden_weapon_does_not_flash() {
        let mut range = range(true, Visibility::Hidden);
        shoot(&mut range);
        assert!(flashes_under(&mut range.world, range.body).is_empty());
        assert!(flashes_under(&mut range.world, range.camera).is_empty());
    }

    #[test]
    fn a_flash_follows_the_weapon_and_its_layers_then_goes() {
        let mut range = range(false, Visibility::Inherited);
        shoot(&mut range);
        let flash = flashes_under(&mut range.world, range.body)[0];

        step(&mut range.world, 0.01);
        let weapon = range.world.get::<WeaponInHand>(range.body).unwrap().0;
        let placed = range.world.get::<Transform>(flash).unwrap().translation;
        assert!(placed.distance(weapon.transform_point(MUZZLE)) < 1e-5, "not at the muzzle: {placed}");
        assert_eq!(range.world.get::<RenderLayers>(flash), Some(&RenderLayers::layer(INSIDE_HEAD)));

        range.world.entity_mut(range.part).insert(RenderLayers::layer(WORLD));
        step(&mut range.world, 0.01);
        assert_eq!(range.world.get::<RenderLayers>(flash), Some(&RenderLayers::layer(WORLD)), "left inside the head");

        step(&mut range.world, MuzzleFlashSpec::default().seconds);
        assert!(range.world.get_entity(flash).is_err(), "outlived its seconds");
    }

    #[test]
    fn the_crown_points_down_the_barrel_from_the_muzzle_ring() {
        let spec = MuzzleFlashSpec::default();
        let mesh = crown_mesh(&spec);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("no positions");
        };
        for p in positions {
            assert!(p[2] <= 0.0, "a corner ahead of the muzzle runs back into the weapon: {p:?}");
            assert!(-p[2] <= spec.length * 1.08 + 1e-4, "a point past its length: {p:?}");
            assert!(Vec2::new(p[0], p[1]).length() <= spec.radius * 1.1 + 1e-4, "a point past its radius: {p:?}");
        }
    }

    #[test]
    fn the_layers_are_drawn_outer_first() {
        let mesh = crown_mesh(&MuzzleFlashSpec::default());
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0) else {
            panic!("no uvs");
        };
        let Some(Indices::U32(indices)) = mesh.indices() else { panic!("no indices") };
        let layers: Vec<f32> = indices.iter().map(|&i| uvs[i as usize][1]).collect();
        assert!(layers.windows(2).all(|pair| pair[0] <= pair[1]), "an inner layer is drawn under an outer one");
        assert_eq!(layers.first(), Some(&0.0));
        assert_eq!(layers.last(), Some(&(LAYERS.len() as f32 - 1.0)));
    }

    #[test]
    fn every_layer_is_inside_the_one_around_it() {
        for pair in LAYERS.windows(2) {
            let (outer, inner) = (&pair[0], &pair[1]);
            assert!(inner.tip_length < outer.tip_length && inner.tip_radius < outer.tip_radius);
        }
    }

    #[test]
    fn a_continuous_preview_plays_waits_and_plays_again() {
        let seconds = 0.1;
        let mut preview = FlashPreview { shown: true, ..default() };
        preview.advance(seconds, 0.05);
        assert_eq!(preview.progress(seconds), Some(0.5));
        preview.advance(seconds, 0.1);
        assert_eq!(preview.progress(seconds), None, "between plays");
        preview.advance(seconds, PREVIEW_GAP);
        assert!(preview.progress(seconds).is_some_and(|progress| progress < 0.6), "played again");
    }

    #[test]
    fn a_paused_or_still_preview_stays_where_it_was_put() {
        let seconds = 0.1;
        let mut preview = FlashPreview { shown: true, paused: true, clock: 0.03, ..default() };
        preview.advance(seconds, 1.0);
        assert_eq!(preview.clock, 0.03);
        preview.clock = 0.3;
        preview.set_still(true, seconds);
        preview.advance(seconds, 1.0);
        assert_eq!(preview.progress(seconds), Some(1.0), "a still from the gap is the flash's end");
    }
}
