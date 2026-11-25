//! Props placed in a map: obstacles, breakables and pickups. See
//! [the plan](../../documentation/prop-archetypes.md).
//!
//! A map `Prop` feature exists on every machine, but a breakable has health and
//! a pickup can be taken, so something has to be believed about each. As with
//! carousel bodies, the authority stands one replicated [`WorldProp`] up per
//! feature that names a model, and every machine draws and collides with those.

use bevy::prelude::*;

use crate::common::air_lit::LitMaterial;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::assets::Assets as PackAssets;
use crate::common::class::Stance;
use crate::common::damage::{Armor, DamageDealt, Damageable, Healed, PlayerId};
use crate::common::hitbox::{Box3, Hitboxes};
use crate::common::skeleton::clip::Clip;
use crate::common::weapon::{Ammo, Equipped, WeaponCatalog, WeaponId};
use crate::editor::prop::{PropMarker, PropModel};
use crate::editor::weapon_pickup::WeaponPickupSpec;
use crate::game::collision::CollisionWorld;
use crate::game::player::{PhysicsBody, Player};
use crate::game::weapon::{draw, Trigger};
use crate::game::collision::PropShape;
use crate::prop::archetype::{Archetype, PickupKind, PickupSpec, PickupStyle};
use crate::prop::collider::ColliderRole;
use crate::prop::baked::{BakedProp, PropCache, SurfaceMaterials};
use crate::prop::rig::{place_part, OnPropBone, PropPose};

/// A placed prop, as every machine sees it. One component so a viewer is
/// never handed a model with nowhere to put it.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[require(Transform, Visibility)]
pub struct WorldProp {
    pub model: String,
    pub at: Vec3,
    pub rotation: Quat,
}

impl WorldProp {
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.at).with_rotation(self.rotation)
    }
}

/// Replicated so a viewer stops drawing a pack somebody took. The countdown
/// back is [`Restock`], which only the authority keeps.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pickup {
    pub taken: bool,
}

/// Somebody took a pickup, for whoever keeps count; nothing that decides anything reads it.
#[derive(Message, Clone, Debug, PartialEq)]
pub struct PickupTaken {
    pub by: Option<PlayerId>,
    /// The prop's model, or a weapon pickup's weapon.
    pub what: PickupWhat,
    pub at: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PickupWhat {
    Prop(String),
    Weapon(crate::common::weapon::WeaponId),
}

/// Seconds until a taken pickup is back.
#[derive(Component)]
struct Restock(f32);

/// A pickup that gives a weapon, stood up by [`sync_weapon_pickups`] from a
/// [`crate::editor::weapon_pickup::WeaponPickup`]. Its [`WorldProp`] draws the
/// weapon's own model; its trigger is a fixed sphere, since a held prop has no
/// colliders and a pickup volume is a game rule rather than a modeler's.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GivesWeapon {
    pub weapon: WeaponId,
    /// Fraction of the weapon's reserve, for somebody who already has it.
    pub ammo: f32,
    pub respawn: f32,
    pub style: PickupStyle,
}

/// Which feature an instance stands for. On the authority only.
#[derive(Component, Clone, Copy, Debug)]
pub struct OfPropFeature(pub Entity);

/// On a feature: the instance it stood up. An instance that has gone while the
/// link stays is a breakable that broke, and is not stood back up until the
/// round changes.
#[derive(Component, Clone, Copy, Debug)]
pub struct StandsFor(pub Entity);

/// Which model and bake an instance was given its components for.
#[derive(Component)]
struct Kitted {
    model: String,
    generation: u64,
}

/// What is drawn for an instance, and for which bake.
#[derive(Component)]
#[require(PropPerformance, PartsPlacedAt)]
struct DrawnProp {
    model: String,
    generation: u64,
    parts: Vec<Entity>,
}

/// An event clip over the idle, and seconds into it. Drawn only: each machine starts it from the
/// event it reads.
#[derive(Component, Default)]
pub struct PropPerformance {
    playing: Option<(&'static str, f32)>,
}

/// What a placed prop loops, on every machine's clock.
pub const IDLE_CLIP: &str = "idle";
/// What a placed prop plays when it is hurt.
pub const HIT_CLIP: &str = "hit";

#[derive(Component)]
struct WorldPropPart;

/// Where [`place_world_prop_parts`] last put a still prop's parts, so a prop that has not moved
/// is not placed again. `None` after any frame they were animated.
#[derive(Component, Default)]
struct PartsPlacedAt(Option<Transform>);

/// How far a weapon's parts are lifted so its lowest point is where it
/// stands: a held prop's origin is its grip, not its base. Measured when it is
/// dressed.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
struct RestsOn(f32);

/// An instance's colliders in the world, by role in [`ColliderRole::ALL`]'s order. Kept on every
/// machine. A taken pickup has none.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct PlacedColliders([Vec<PropShape>; 5]);

impl PlacedColliders {
    pub fn of(baked: &BakedProp, at: &Transform) -> Self {
        let mut placed = PlacedColliders::default();
        for collider in &baked.colliders {
            let shape = collider.placed(at);
            for role in &collider.roles {
                placed.0[role.index()].push(shape.clone());
            }
        }
        placed
    }

    pub fn playing(&self, role: ColliderRole) -> &[PropShape] {
        &self.0[role.index()]
    }
}

/// The trigger a pickup with no trigger colliders is taken through, in meters.
pub const DEFAULT_PICKUP_RADIUS: f32 = 0.4;

/// The trigger a weapon is taken through, in meters, centered on where it
/// hovers.
pub const WEAPON_PICKUP_RADIUS: f32 = 0.6;

/// How far above where it stands a weapon's lowest point hovers, in meters,
/// whatever its style: a weapon on the floor is lost against it.
pub const WEAPON_HOVER: f32 = 1.0;

/// How far a [`PickupStyle::Tf2`] pickup is drawn above where it stands, in
/// meters, and how fast it turns.
const HOVER: f32 = 0.15;
const SPIN_DEGREES_PER_SECOND: f32 = 90.0;

/// Added beside the game rather than inside `GamePlugin`, because it bakes
/// models and so needs the asset plugins the game's own tests run without.
pub struct WorldPropPlugin;

impl Plugin for WorldPropPlugin {
    fn build(&self, app: &mut App) {
        crate::common::air_lit::ensure(app);
        app.add_message::<Healed>()
            .add_message::<PickupTaken>()
            .init_resource::<PropCache>()
            .init_resource::<SurfaceMaterials>()
            .add_plugins(crate::game::pickup_beam::PickupBeamPlugin)
            .add_systems(
                Update,
                // Not mode-gated: props are map content, standing in the
                // editor as in a round. Authority-gated: see the module docs.
                (sync_world_props, sync_weapon_pickups, kit_world_props)
                    .chain()
                    .run_if(crate::common::net::has_authority),
            )
            .add_systems(Update, drop_our_own_instances.run_if(crate::common::net::is_remote_client))
            // Not authority-gated, as the room rebuild beside it is not: a
            // client predicting its movement walks into its own copy.
            .add_systems(
                FixedUpdate,
                // Outside Play too, so the nav bake reads what stands now rather than what stood
                // at the end of the last round. Not in the weapon editor, whose range has a
                // collision world of its own swapped in.
                collide_with_props
                    .after(crate::game::rebuild_collision_when_rooms_change)
                    .before(crate::game::player::step_player)
                    .run_if(not(in_state(AppMode::Weapon))),
            )
            // Ungated: a client draws what it is sent.
            .add_message::<DamageDealt>()
            // Also `DamagePlugin`'s; here too for an app with no damage layer.
            .add_message::<crate::common::damage::Kill>()
            .add_systems(
                Update,
                (place_world_props, place_prop_colliders, dress_world_props, start_hit_clips, place_world_prop_parts, box_breakables)
                    .chain(),
            )
            .add_systems(
                FixedUpdate,
                act_on_bodies_touching_props
                    .in_set(crate::game::damage::DamageSystems::Deal)
                    .run_if(in_state(AppMode::Play))
                    .run_if(crate::common::net::has_authority),
            )
            // With the bones on F6: both are what a body is measured against.
            .add_systems(
                PostUpdate,
                draw_prop_colliders
                    .run_if(resource_exists::<GizmoConfigStore>)
                    .run_if(resource_exists::<crate::game::skeleton::ShowBones>)
                    .run_if(crate::common::app_mode::showing_the_world),
            )
            .add_systems(
                Update,
                take_pickups
                    .run_if(in_state(AppMode::Play))
                    .run_if(crate::common::net::has_authority),
            )
            // A round starts and ends with everything standing again.
            .add_systems(
                OnEnter(AppMode::Play),
                stand_everything_back_up.run_if(crate::common::net::has_authority),
            )
            .add_systems(
                OnExit(AppMode::Play),
                stand_everything_back_up.run_if(crate::common::net::has_authority),
            );
    }
}

/// One instance per feature naming a model, kept where the feature is. Asks
/// about absence, as the body rule does, so a feature added mid-round or a
/// model renamed needs no case of its own.
pub fn sync_world_props(
    mut commands: Commands,
    features: Query<(Entity, &PropModel, &Transform, Option<&StandsFor>), With<PropMarker>>,
    mut instances: Query<(Entity, &mut WorldProp, &OfPropFeature), Without<GivesWeapon>>,
) {
    for (feature, model, placed, stands) in &features {
        // Built only when spawning or changing, not to compare every frame.
        let wanted = || WorldProp {
            model: model.0.clone(),
            at: placed.translation,
            rotation: placed.rotation,
        };
        let existing = stands.and_then(|stands| instances.get_mut(stands.0).ok());

        match (model.0.is_empty(), stands, existing) {
            (true, Some(stands), _) => {
                commands.entity(stands.0).try_despawn();
                commands.entity(feature).remove::<StandsFor>();
            }
            (true, None, _) => {}
            (false, None, _) => {
                let instance = commands.spawn((wanted(), OfPropFeature(feature))).id();
                commands.entity(feature).insert(StandsFor(instance));
            }
            // Broken this round.
            (false, Some(_), None) => {}
            (false, Some(_), Some((instance, mut prop, _))) => {
                if prop.model != model.0 {
                    // A different model is a different set of components, so
                    // a fresh instance rather than a patched one.
                    commands.entity(instance).despawn();
                    let instance = commands.spawn((wanted(), OfPropFeature(feature))).id();
                    commands.entity(feature).insert(StandsFor(instance));
                } else if prop.at != placed.translation || prop.rotation != placed.rotation {
                    prop.at = placed.translation;
                    prop.rotation = placed.rotation;
                }
            }
        }
    }

    for (instance, _, of) in &instances {
        if features.get(of.0).is_err() {
            commands.entity(instance).despawn();
        }
    }
}

/// A client draws the server's instances only. One stood up while this machine was the authority,
/// as after a link that failed or a disconnect with the server's map still open, would stand
/// where the server's does, never taken, so a pickup would always look ready. The features
/// forget them too, so going back to solo stands them up again.
fn drop_our_own_instances(
    mut commands: Commands,
    ours: Query<Entity, With<OfPropFeature>>,
    features: Query<Entity, With<StandsFor>>,
) {
    for instance in &ours {
        commands.entity(instance).despawn();
    }
    for feature in &features {
        commands.entity(feature).remove::<StandsFor>();
    }
}

/// One instance per weapon pickup, as [`sync_world_props`] does for props.
/// Its model is whatever the weapon names now, so a weapon re-modeled between
/// rounds is drawn anew; a weapon nobody defines yet stands nothing up.
#[allow(clippy::type_complexity)]
pub fn sync_weapon_pickups(
    mut commands: Commands,
    catalog: Res<WeaponCatalog>,
    features: Query<(Entity, &WeaponPickupSpec, &Transform, Option<&StandsFor>)>,
    mut instances: Query<(Entity, &mut WorldProp, &mut GivesWeapon, &OfPropFeature)>,
) {
    for (feature, spec, placed, stands) in &features {
        let weapon = catalog.get(spec.weapon);
        let model = || weapon.and_then(|weapon| weapon.model.as_deref()).unwrap_or_default();
        // Built only when spawning or changing, not to compare every frame.
        let wanted_prop = || WorldProp { model: model().to_owned(), at: placed.translation, rotation: placed.rotation };
        let gives = GivesWeapon { weapon: spec.weapon, ammo: spec.ammo, respawn: spec.respawn, style: spec.style };
        let existing = stands.and_then(|stands| instances.get_mut(stands.0).ok());

        match (weapon.map(|_| gives), stands, existing) {
            (None, Some(stands), _) => {
                commands.entity(stands.0).try_despawn();
                commands.entity(feature).remove::<StandsFor>();
            }
            (None, None, _) => {}
            (Some(gives), None, _) => {
                let instance = commands.spawn((wanted_prop(), gives, Pickup::default(), OfPropFeature(feature))).id();
                commands.entity(feature).insert(StandsFor(instance));
            }
            // Gone without the feature going: put back on the round's edge.
            (Some(_), Some(_), None) => {}
            (Some(gives), Some(_), Some((_, mut was_prop, mut was_gives, _))) => {
                if was_prop.model != model() || was_prop.at != placed.translation || was_prop.rotation != placed.rotation {
                    *was_prop = wanted_prop();
                }
                if *was_gives != gives {
                    *was_gives = gives;
                }
            }
        }
    }

    for (instance, _, _, of) in &instances {
        if features.get(of.0).is_err() {
            commands.entity(instance).despawn();
        }
    }
}

/// Give an instance what its archetype says it is. Re-run when the model is
/// re-baked, so a crate saved as a pickup between rounds becomes one.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn kit_world_props(
    mut commands: Commands,
    packs: Res<PackAssets>,
    mut cache: ResMut<PropCache>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<LitMaterial>>,
    mut clips: ResMut<Assets<Clip>>,
    instances: Query<(Entity, &WorldProp, Option<&Kitted>, Has<Damageable>, Has<Pickup>), Without<GivesWeapon>>,
) {
    let generation = cache.generation();
    for (entity, prop, kitted, damageable, pickup) in &instances {
        if kitted.is_some_and(|kitted| kitted.model == prop.model && kitted.generation == generation) {
            continue;
        }
        let archetype = cache
            .ensure(&prop.model, &packs, &mut meshes, &mut materials, &mut surfaces, &mut clips)
            .map(|baked| (baked.archetype, baked.breakable.health));

        let mut entity = commands.entity(entity);
        entity.insert(Kitted { model: prop.model.clone(), generation });
        match archetype {
            Some((Archetype::Breakable, health)) if !damageable => {
                entity.insert(Damageable::with_health(health.max(1)));
            }
            Some((Archetype::Breakable, _)) => {}
            _ if damageable => {
                entity.remove::<Damageable>();
            }
            _ => {}
        }
        match archetype {
            Some((Archetype::Pickup, _)) if !pickup => {
                entity.insert(Pickup::default());
            }
            Some((Archetype::Pickup, _)) => {}
            _ if pickup => {
                entity.remove::<(Pickup, Restock)>();
            }
            _ => {}
        }
    }
}

fn place_world_props(mut instances: Query<(&WorldProp, &mut Transform), Changed<WorldProp>>) {
    for (prop, mut transform) in &mut instances {
        *transform = prop.transform();
    }
}

/// The model's parts as children. Compared every frame rather than keyed on
/// arrival: the instance, the model and a re-bake each turn up on their own
/// schedule.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn dress_world_props(
    mut commands: Commands,
    packs: Res<PackAssets>,
    mut cache: ResMut<PropCache>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<LitMaterial>>,
    mut clips: ResMut<Assets<Clip>>,
    instances: Query<(Entity, &WorldProp, Option<Ref<DrawnProp>>, Option<Ref<Pickup>>, Has<GivesWeapon>)>,
    mut parts: Query<&mut Visibility, With<WorldPropPart>>,
    mut unpicked: RemovedComponents<Pickup>,
) {
    let unpicked: Vec<Entity> = unpicked.read().collect();
    for (entity, prop, drawn, pickup, weapon) in &instances {
        let pickup_changed = pickup.as_ref().is_some_and(Ref::is_changed) || unpicked.contains(&entity);
        let pickup = pickup.as_deref();
        let drawn_changed = drawn.as_ref().is_some_and(Ref::is_changed);
        let drawn = drawn.as_deref();
        let visibility = match pickup.is_some_and(|pickup| pickup.taken) {
            true => Visibility::Hidden,
            false => Visibility::Inherited,
        };

        let current = drawn.filter(|drawn| {
            drawn.model == prop.model && drawn.generation == cache.generation()
        });
        if let Some(drawn) = current {
            // Parts are spawned shown as the pickup stood then.
            if !pickup_changed && !drawn_changed {
                continue;
            }
            // The parts, not the root: the prop editor hides roots, and
            // restores what it found.
            for part in &drawn.parts {
                if let Ok(mut shown) = parts.get_mut(*part)
                    && *shown != visibility
                {
                    *shown = visibility;
                }
            }
            continue;
        }

        if let Some(drawn) = drawn {
            for part in &drawn.parts {
                commands.entity(*part).try_despawn();
            }
        }
        let mut spawned = Vec::new();
        if let Some(baked) =
            cache.ensure(&prop.model, &packs, &mut meshes, &mut materials, &mut surfaces, &mut clips)
        {
            let parts = baked.parts.clone();
            if weapon {
                let lowest = parts
                    .iter()
                    .filter_map(|part| match meshes.get(&part.mesh)?.attribute(Mesh::ATTRIBUTE_POSITION)? {
                        bevy::mesh::VertexAttributeValues::Float32x3(positions) => {
                            positions.iter().map(|p| p[1]).reduce(f32::min)
                        }
                        _ => None,
                    })
                    .fold(f32::INFINITY, f32::min);
                commands.entity(entity).insert(RestsOn(if lowest.is_finite() { -lowest } else { 0.0 }));
            }
            commands.entity(entity).with_children(|instance| {
                for part in parts {
                    let mut spawning =
                        instance.spawn((WorldPropPart, Mesh3d(part.mesh), MeshMaterial3d(part.material), visibility));
                    if let Some(bone) = part.bone {
                        spawning.insert(OnPropBone(bone));
                    }
                    spawned.push(spawning.id());
                }
            });
        }
        commands.entity(entity).insert(DrawnProp {
            model: prop.model.clone(),
            generation: cache.generation(),
            parts: spawned,
        });
    }
}

fn start_hit_clips(mut dealt: MessageReader<DamageDealt>, mut props: Query<&mut PropPerformance>) {
    for hit in dealt.read() {
        if let Ok(mut performance) = props.get_mut(hit.target) {
            performance.playing = Some((HIT_CLIP, 0.0));
        }
    }
}

/// Lift and turn the parts of a hovering pickup, leaving the instance where it
/// stands so its trigger does not move with the drawing, and move each part by
/// its bone. Purely drawn: the angle is this machine's clock, and nobody needs
/// to agree on it. A pickup is still while authoring, so a mapper lines up one
/// that is not turning, but lifted, so it stands at the height it will in a round.
#[allow(clippy::type_complexity)]
fn place_world_prop_parts(
    time: Res<Time>,
    mode: Res<State<AppMode>>,
    cache: Res<PropCache>,
    clips: Res<Assets<Clip>>,
    mut instances: Query<(
        &WorldProp,
        Ref<DrawnProp>,
        &mut PropPerformance,
        &mut PartsPlacedAt,
        Option<&GivesWeapon>,
        Option<&RestsOn>,
    )>,
    mut parts: Query<(&mut Transform, Option<&OnPropBone>), With<WorldPropPart>>,
) {
    // Wrapped, or an `f32` of seconds steps visibly after a day. The wrap
    // period is a whole number of turns at this speed, so there is no jump.
    let clock = time.elapsed_secs_wrapped();
    let spin = if mode.is_authoring() {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_y((clock * SPIN_DEGREES_PER_SECOND).to_radians())
    };
    // In `f64`: an idle's length has nothing to do with the wrap period above.
    let idle_clock = time.elapsed_secs_f64();
    let mut pose = PropPose::default();
    for (prop, drawn, mut performance, mut placed_at, gives, rests) in &mut instances {
        let lift = match (gives, rests) {
            (Some(_), Some(rests)) => rests.0 + WEAPON_HOVER,
            (Some(_), None) => WEAPON_HOVER,
            (None, _) => 0.0,
        };
        let baked = cache.get(&prop.model);
        let style = gives.map(|gives| gives.style).or_else(|| {
            baked.filter(|baked| baked.archetype == Archetype::Pickup).map(|baked| baked.pickup.style)
        });
        let wanted = match style {
            Some(PickupStyle::Tf2) if gives.is_some() => Transform::from_xyz(0.0, lift, 0.0).with_rotation(spin),
            Some(PickupStyle::Tf2) => Transform::from_xyz(0.0, lift + HOVER, 0.0).with_rotation(spin),
            _ => Transform::from_xyz(0.0, lift, 0.0),
        };
        let rigged = baked.filter(|baked| !baked.rig.is_empty());
        // No bones, so a started clip has nothing to play on.
        if rigged.is_none() && performance.playing.is_some() {
            performance.playing = None;
        }
        // With no idle and nothing playing a rig stands in rest, where every delta is the identity.
        let animated = rigged.filter(|baked| performance.playing.is_some() || baked.clips.contains_key(IDLE_CLIP));
        if animated.is_none() && !drawn.is_changed() && placed_at.0 == Some(wanted) {
            continue;
        }
        placed_at.0 = animated.is_none().then_some(wanted);
        let deltas = match animated {
            Some(baked) => {
                pose_placed(baked, &clips, idle_clock, time.delta_secs(), &mut performance, &mut pose);
                baked.rig.deltas(&pose)
            }
            None => vec![],
        };
        for part in &drawn.parts {
            let Ok((mut transform, bone)) = parts.get_mut(*part) else { continue };
            let placed = place_part(wanted, &deltas, bone);
            // Compared first, so a still prop is not a changed one.
            if *transform != placed {
                *transform = placed;
            }
        }
    }
}

/// The idle at `clock`, then whatever event clip is playing over it until it ends.
fn pose_placed(
    baked: &BakedProp,
    clips: &Assets<Clip>,
    clock: f64,
    delta: f32,
    performance: &mut PropPerformance,
    pose: &mut PropPose,
) {
    *pose = PropPose::rest(&baked.rig);
    let clip = |name: &str| baked.clips.get(name).and_then(|handle| clips.get(handle));
    if let Some(idle) = clip(IDLE_CLIP) {
        let time = if idle.looping && idle.length > 0.0 { clock % idle.length as f64 } else { clock };
        crate::prop::rig::apply_clip(idle, time as f32, &baked.rig, pose);
    }
    let Some((name, seconds)) = performance.playing.as_mut() else { return };
    *seconds += delta;
    match clip(name) {
        Some(event) if event.looping || *seconds <= event.length => {
            crate::prop::rig::apply_clip(event, *seconds, &baked.rig, pose)
        }
        _ => performance.playing = None,
    }
}

#[allow(clippy::type_complexity)]
fn place_prop_colliders(
    mut commands: Commands,
    cache: Res<PropCache>,
    mut placed_for: Local<Option<u64>>,
    mut instances: Query<
        (Entity, Ref<WorldProp>, Option<Ref<Pickup>>, Option<&mut PlacedColliders>),
        Without<GivesWeapon>,
    >,
) {
    let rebaked = *placed_for != Some(cache.bakes());
    *placed_for = Some(cache.bakes());
    for (entity, prop, pickup, placed) in &mut instances {
        let pickup_changed = pickup.as_ref().is_some_and(Ref::is_changed);
        if !rebaked && !prop.is_changed() && !pickup_changed && placed.is_some() {
            continue;
        }
        let taken = pickup.is_some_and(|pickup| pickup.taken);
        let wanted = match (taken, cache.get(&prop.model)) {
            (false, Some(baked)) => PlacedColliders::of(baked, &prop.transform()),
            _ => PlacedColliders::default(),
        };
        match placed {
            Some(mut placed) => {
                if *placed != wanted {
                    *placed = wanted;
                }
            }
            None => {
                commands.entity(entity).insert(wanted);
            }
        }
    }
}

/// Its trigger colliders, else a small sphere; a weapon's is a fixed sphere. Placed from the model,
/// since [`PlacedColliders`] is empty while the pickup is taken and refilled a frame after.
fn pickup_triggers(gives: Option<&GivesWeapon>, baked: Option<&BakedProp>, at: &Transform) -> Vec<PropShape> {
    if gives.is_some() {
        return vec![weapon_trigger(at)];
    }
    let triggers: Vec<PropShape> = baked
        .into_iter()
        .flat_map(|baked| &baked.colliders)
        .filter(|collider| collider.has(ColliderRole::Trigger))
        .map(|collider| collider.placed(at))
        .collect();
    match triggers.is_empty() {
        true => vec![PropShape::sphere(at.translation, DEFAULT_PICKUP_RADIUS)],
        false => triggers,
    }
}

/// A breakable is shot as the box around its shot colliders, and has no head, so a shot through a
/// corner of the box that misses a turned or round collider still counts. Every machine works
/// this out, since a client draws hitboxes too. A prop that stops being breakable loses them, or shots would go on
/// stopping at it.
fn box_breakables(
    mut commands: Commands,
    instances: Query<(Entity, Ref<PlacedColliders>, Option<&Hitboxes>, Option<Ref<Damageable>>)>,
) {
    for (entity, placed, boxes, breakable) in &instances {
        let wanted = breakable
            .filter(|_| !placed.playing(ColliderRole::Shots).is_empty())
            .and_then(|breakable| {
                if !placed.is_changed() && !breakable.is_added() && boxes.is_some() {
                    return boxes.copied();
                }
                Some(Hitboxes { body: bounding(placed.playing(ColliderRole::Shots).iter())?, head: None })
            });
        match wanted {
            Some(wanted) if boxes != Some(&wanted) => {
                commands.entity(entity).insert(wanted);
            }
            Some(_) => {}
            None if boxes.is_some() => {
                commands.entity(entity).remove::<Hitboxes>();
            }
            None => {}
        }
    }
}

fn bounding<'a>(shapes: impl Iterator<Item = &'a PropShape>) -> Option<Box3> {
    let (min, max) = shapes.fold(None, |bounds: Option<(Vec3, Vec3)>, shape| {
        let (lo, hi) = shape.bounds();
        Some(match bounds {
            Some((min, max)) => (min.min(lo), max.max(hi)),
            None => (lo, hi),
        })
    })?;
    Some(Box3 { center: (min + max) * 0.5, half_extents: (max - min) * 0.5 })
}

/// Build the solid and shot-stopping props into the collision world. On every machine, so a
/// client predicting its movement walks into the same crate the server does. Run each tick
/// before the step, rebuilding only when an instance came, went or changed its colliders, or
/// something else wrote the collision world.
pub fn collide_with_props(
    mut collision: ResMut<CollisionWorld>,
    instances: Query<&PlacedColliders>,
    changed: Query<(), Changed<PlacedColliders>>,
    mut built: Local<Option<usize>>,
) {
    let seen = instances.iter().len();
    if *built == Some(seen) && changed.is_empty() && !collision.is_changed() {
        return;
    }
    *built = Some(seen);
    let collision = collision.bypass_change_detection();
    collision.props.clear();
    collision.shots.clear();
    for placed in &instances {
        collision.props.extend_from_slice(placed.playing(ColliderRole::Solid));
        collision.shots.extend_from_slice(placed.playing(ColliderRole::Shots));
    }
}

/// Credited to the map. A body already burning starts its afterburn over.
#[allow(clippy::type_complexity)]
fn act_on_bodies_touching_props(
    mut commands: Commands,
    instances: Query<&PlacedColliders>,
    mut bodies: Query<
        (Entity, &PhysicsBody, &Stance, Option<&mut crate::common::flame::Burning>),
        (With<Player>, Without<crate::game::death::Dying>),
    >,
    mut kills: MessageWriter<crate::common::damage::Kill>,
) {
    use crate::common::damage::{DamageSource, How, Kill};
    use crate::common::flame::{Burning, FlameSpec};
    let touching = |role: ColliderRole, center: Vec3, half: Vec3| {
        instances.iter().any(|placed| placed.playing(role).iter().any(|shape| shape.overlaps(center, half)))
    };
    for (body, placed, stance, burning) in &mut bodies {
        let (center, half) = (placed.current, stance.half_extents());
        if touching(ColliderRole::Kills, center, half) {
            kills.write(Kill { target: body, source: DamageSource::World, point: center, how: How::Kill });
            continue;
        }
        if !touching(ColliderRole::Ignites, center, half) {
            continue;
        }
        match burning {
            Some(mut burning) => burning.relight(&FlameSpec::FLAMETHROWER, DamageSource::World),
            None => {
                commands.entity(body).try_insert(Burning::lit(&FlameSpec::FLAMETHROWER, DamageSource::World));
            }
        }
    }
}

/// Count taken pickups back in, and let anybody standing in a ready one take
/// it — unless it would give them nothing.
#[allow(clippy::type_complexity)]
fn take_pickups(
    mut commands: Commands,
    time: Res<Time>,
    cache: Res<PropCache>,
    catalog: Res<WeaponCatalog>,
    mut pickups: Query<(Entity, &WorldProp, &mut Pickup, Option<&mut Restock>, Option<&GivesWeapon>)>,
    mut bodies: Query<
        (
            Entity,
            &PhysicsBody,
            &Stance,
            Option<&mut Damageable>,
            Option<&mut Armor>,
            Option<&mut Ammo>,
            Option<&mut Equipped>,
            Option<&mut Trigger>,
            Option<&PlayerId>,
            Option<&crate::game::condition::Condition>,
        ),
        With<Player>,
    >,
    mut healed: MessageWriter<Healed>,
    mut taken: MessageWriter<PickupTaken>,
) {
    for (entity, prop, mut pickup, restock, gives) in &mut pickups {
        if pickup.taken {
            match restock {
                Some(mut restock) => {
                    restock.0 -= time.delta_secs();
                    if restock.0 <= 0.0 {
                        pickup.taken = false;
                        commands.entity(entity).remove::<Restock>();
                    }
                }
                // Taken with no clock, which nothing writes: put it back.
                None => pickup.taken = false,
            }
            continue;
        }

        let at = prop.transform();
        let baked = cache.get(&prop.model);
        let respawn = match (gives, baked) {
            (Some(gives), _) => gives.respawn,
            (None, Some(baked)) => baked.pickup.respawn,
            (None, None) => continue,
        };
        let triggers = pickup_triggers(gives, baked, &at);
        let Some(reach) = bounding(triggers.iter()) else { continue };
        for (body_entity, body, stance, health, armor, ammo, equipped, trigger, id, condition) in &mut bodies {
            let half = stance.half_extents();
            // Overlapping a trigger needs overlapping the box around all of them.
            if ((body.current - reach.center).abs() - half - reach.half_extents).max_element() > 1.0e-4 {
                continue;
            }
            if !triggers.iter().any(|trigger| trigger.overlaps(body.current, half)) {
                continue;
            }
            let gained = match gives {
                Some(gives) => {
                    let (Some(mut ammo), Some(mut equipped), Some(mut trigger)) = (ammo, equipped, trigger) else {
                        continue;
                    };
                    let draws = condition.is_none_or(|condition| condition.may_switch_weapons());
                    give_weapon(gives, &mut equipped, &mut ammo, &mut trigger, &catalog, draws)
                }
                None => {
                    let Some(baked) = cache.get(&prop.model) else { continue };
                    let ammo = ammo.zip(equipped).map(|(ammo, equipped)| (ammo, equipped.into_inner() as &Equipped));
                    match give(&baked.pickup, health, armor, ammo, &catalog) {
                        None => false,
                        Some(amount) => {
                            if let Some(&healer) = id
                                && amount > 0
                            {
                                healed.write(Healed { target: body_entity, healer, amount });
                            }
                            true
                        }
                    }
                }
            };
            if !gained {
                continue;
            }
            pickup.taken = true;
            taken.write(PickupTaken {
                by: id.copied(),
                what: gives.map_or_else(|| PickupWhat::Prop(prop.model.clone()), |gives| PickupWhat::Weapon(gives.weapon)),
                at: prop.at,
            });
            commands.entity(entity).insert(Restock(respawn.max(0.0)));
            break;
        }
    }
}

/// `None` if the taker got nothing, else how much health it got back.
fn give(
    spec: &PickupSpec,
    health: Option<Mut<Damageable>>,
    armor: Option<Mut<Armor>>,
    ammo: Option<(Mut<Ammo>, &Equipped)>,
    catalog: &WeaponCatalog,
) -> Option<u32> {
    let fraction = spec.amount.max(0.0);
    match spec.kind {
        PickupKind::Health => {
            let mut health = health?;
            if health.is_full() {
                return None;
            }
            let amount = share(health.max(), fraction);
            let healed = health.heal(amount);
            (healed > 0).then_some(healed)
        }
        PickupKind::Ammo => {
            let (mut ammo, equipped) = ammo?;
            let mut gained = false;
            for &id in equipped.carried() {
                let Some(weapon) = catalog.get(id) else { continue };
                let max = weapon.magazine.reserve;
                // Read before writing, so a full body is not marked changed.
                if ammo.at(id).map_or(0, |state| state.reserve) >= max {
                    continue;
                }
                let amount = share(max, fraction);
                let state = ammo.at_mut(id);
                state.reserve = (state.reserve + amount).min(max);
                gained = true;
            }
            gained.then_some(0)
        }
        PickupKind::Armor => {
            let mut armor = armor?;
            if armor.is_full() {
                return None;
            }
            let amount = share(armor.max(), fraction);
            (armor.add(amount) > 0).then_some(0)
        }
        // Specced, with no behavior yet: there is no flag mode.
        PickupKind::Flag => None,
    }
}

/// Every placed prop's colliders in the colors of their roles, and each pickup's trigger: bright
/// while it can be taken, dim while it restocks.
fn draw_prop_colliders(
    show: Res<crate::game::skeleton::ShowBones>,
    cache: Res<PropCache>,
    instances: Query<(&WorldProp, Option<&Pickup>, Option<&GivesWeapon>)>,
    mut gizmos: Gizmos,
) {
    if !show.0 {
        return;
    }
    for (prop, pickup, gives) in &instances {
        let at = prop.transform();
        let taken = pickup.is_some_and(|pickup| pickup.taken);
        let dim = |color: Color| if taken { color.with_alpha(0.25) } else { color };
        let baked = cache.get(&prop.model);
        let fallback = pickup.is_some()
            && baked.is_none_or(|baked| !baked.colliders.iter().any(|collider| collider.has(ColliderRole::Trigger)));
        if gives.is_some() || fallback {
            let reach = if gives.is_some() { WEAPON_PICKUP_RADIUS } else { DEFAULT_PICKUP_RADIUS };
            let center = if gives.is_some() { at.transform_point(Vec3::Y * WEAPON_HOVER) } else { at.translation };
            gizmos.sphere(Isometry3d::from_translation(center), reach, dim(Color::srgb(0.4, 0.9, 0.5)));
        }
        if gives.is_some() {
            continue;
        }
        let Some(baked) = baked else { continue };
        for collider in &baked.colliders {
            collider.draw(&mut gizmos, &at, dim(collider.color()));
        }
    }
}

fn weapon_trigger(at: &Transform) -> PropShape {
    PropShape::sphere(at.transform_point(Vec3::Y * WEAPON_HOVER), WEAPON_PICKUP_RADIUS)
}

/// A weapon not carried is added with full ammo, and drawn if `draws`; one
/// carried tops up its reserve. Returns whether the taker got anything.
fn give_weapon(
    gives: &GivesWeapon,
    equipped: &mut Mut<Equipped>,
    ammo: &mut Mut<Ammo>,
    trigger: &mut Trigger,
    catalog: &WeaponCatalog,
    draws: bool,
) -> bool {
    let Some(weapon) = catalog.get(gives.weapon) else { return false };

    if equipped.carries(weapon.id) {
        let max = weapon.magazine.reserve;
        // Read before writing, so a full body is not marked changed.
        if ammo.at(weapon.id).map_or(0, |state| state.reserve) >= max || gives.ammo <= 0.0 {
            return false;
        }
        let state = ammo.at_mut(weapon.id);
        state.reserve = (state.reserve + share(max, gives.ammo)).min(max);
        return true;
    }

    equipped.add(weapon.id);
    *ammo.at_mut(weapon.id) = Ammo::full(&weapon.magazine);
    if draws {
        draw(weapon.id, equipped, ammo, trigger, catalog);
    }
    true
}

/// `fraction` of `max`, rounded up and at least one. Rounded up only past
/// float noise: `0.6 * 100.0` is `60.000002` in `f32`.
fn share(max: u32, fraction: f32) -> u32 {
    ((max as f32 * fraction - 1.0e-3).ceil() as u32).max(1)
}

/// Mend what is standing in place and forget which broke, so the next frame
/// stands those back up. Mended rather than respawned, so the props a body is
/// spawned against are never all missing at once.
fn stand_everything_back_up(
    mut commands: Commands,
    features: Query<(Entity, &StandsFor)>,
    mut instances: Query<(Entity, Option<&mut Damageable>, Option<&mut Pickup>), With<WorldProp>>,
) {
    for (feature, stands) in &features {
        if instances.get(stands.0).is_err() {
            commands.entity(feature).remove::<StandsFor>();
        }
    }
    for (instance, health, pickup) in &mut instances {
        if let Some(mut health) = health {
            health.restore();
        }
        if let Some(mut pickup) = pickup {
            pickup.taken = false;
            commands.entity(instance).remove::<Restock>();
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::weapon::{Magazine, Mounted, Weapon, WeaponId};

    fn feature(world: &mut World, model: &str, at: Vec3) -> Entity {
        world
            .spawn((PropMarker, PropModel(model.into()), Transform::from_translation(at)))
            .id()
    }

    fn instances(world: &mut World) -> Vec<WorldProp> {
        world.query::<&WorldProp>().iter(world).cloned().collect()
    }

    #[test]
    fn a_feature_naming_a_model_gets_one_instance_and_keeps_it() {
        let mut world = World::new();
        feature(&mut world, "crate", Vec3::X);
        feature(&mut world, "", Vec3::Y);

        for _ in 0..3 {
            world.run_system_once(sync_world_props).unwrap();
        }
        let standing = instances(&mut world);
        assert_eq!(standing.len(), 1, "the cube got an instance, or the crate got several");
        assert_eq!(standing[0].at, Vec3::X);
    }

    #[test]
    fn an_instance_follows_its_feature_and_goes_with_it() {
        let mut world = World::new();
        let crate_feature = feature(&mut world, "crate", Vec3::X);
        world.run_system_once(sync_world_props).unwrap();

        world.get_mut::<Transform>(crate_feature).unwrap().translation = Vec3::Z;
        world.run_system_once(sync_world_props).unwrap();
        assert_eq!(instances(&mut world)[0].at, Vec3::Z, "the instance stayed behind");

        world.despawn(crate_feature);
        world.run_system_once(sync_world_props).unwrap();
        assert!(instances(&mut world).is_empty(), "an orphan was left standing");
    }

    /// Fails if a broken crate is stood straight back up by the absence rule.
    #[test]
    fn a_broken_prop_stays_broken_until_the_round_changes() {
        let mut world = World::new();
        feature(&mut world, "crate", Vec3::X);
        world.run_system_once(sync_world_props).unwrap();

        let instance = world.query_filtered::<Entity, With<WorldProp>>().single(&world).unwrap();
        world.despawn(instance);
        world.run_system_once(sync_world_props).unwrap();
        assert!(instances(&mut world).is_empty(), "the crate came straight back");

        world.run_system_once(stand_everything_back_up).unwrap();
        world.run_system_once(sync_world_props).unwrap();
        assert_eq!(instances(&mut world).len(), 1, "a new round did not bring it back");
    }

    /// Fails if a new round respawns what is still standing, which leaves a
    /// frame with no props for a body to be spawned against.
    #[test]
    fn a_new_round_mends_what_is_standing_in_place() {
        let mut world = World::new();
        feature(&mut world, "crate", Vec3::X);
        world.run_system_once(sync_world_props).unwrap();
        let instance = world.query_filtered::<Entity, With<WorldProp>>().single(&world).unwrap();
        let mut health = Damageable::with_health(50);
        health.apply(30);
        world.entity_mut(instance).insert(health);

        world.run_system_once(stand_everything_back_up).unwrap();
        world.run_system_once(sync_world_props).unwrap();
        let standing = world.query_filtered::<Entity, With<WorldProp>>().single(&world).unwrap();
        assert_eq!(standing, instance, "a standing crate was respawned");
        assert!(world.get::<Damageable>(instance).unwrap().is_full(), "the crate was not mended");
    }

    fn app_with_assets() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::state::app::StatesPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::render::mesh::MeshPlugin,
        ));
        app.init_asset::<StandardMaterial>();
        app.add_plugins(crate::common::skeleton::ClipAssetsPlugin);
        app.init_state::<AppMode>()
            .init_resource::<PackAssets>()
            .init_resource::<CollisionWorld>()
            .init_resource::<WeaponCatalog>()
            .add_plugins(WorldPropPlugin);
        app
    }

    /// The shipped crate, from its file to something that can be shot and
    /// walked into. Fails if any link between the archetype and the
    /// components it stands for breaks.
    #[test]
    fn a_placed_crate_is_shootable_solid_and_drawn() {
        if std::fs::metadata("assets/default/props/crate.gpp").is_err() {
            // Not run from the repo root.
            return;
        }
        let mut app = app_with_assets();
        app.world_mut()
            .spawn((PropMarker, PropModel("crate".into()), Transform::from_xyz(2.0, 0.0, 0.0)));
        app.update();
        app.update();

        let world = app.world_mut();
        let (health, boxes, drawn) = world
            .query_filtered::<(&Damageable, &Hitboxes, &DrawnProp), With<WorldProp>>()
            .single(world)
            .expect("the crate was not stood up with health, hitboxes and a model");
        assert_eq!(health.max(), 75);
        assert!(boxes.head.is_none(), "a crate has a head");
        assert!(boxes.body.contains(Vec3::new(2.0, 0.3, 0.0)));
        assert!(!drawn.parts.is_empty(), "nothing was drawn");

        world.run_system_once(collide_with_props).unwrap();
        let collision = world.resource::<CollisionWorld>();
        assert_eq!(collision.props.len(), 1, "the crate is not solid");
    }

    /// The shipped crate jolts when it is hurt and is still again once its `hit` clip ends.
    #[test]
    fn a_hurt_crate_plays_its_hit_clip_and_settles() {
        if std::fs::metadata("assets/default/props/crate.gpp").is_err() {
            return;
        }
        let mut app = app_with_assets();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(40)));
        app.world_mut().spawn((PropMarker, PropModel("crate".into()), Transform::default()));
        app.update();
        app.update();
        let (instance, part) = {
            let world = app.world_mut();
            let (instance, drawn) = world.query::<(Entity, &DrawnProp)>().single(world).unwrap();
            (instance, drawn.parts[0])
        };
        let at = |app: &App| *app.world().get::<Transform>(part).unwrap();
        assert_eq!(at(&app), Transform::IDENTITY, "a crate nobody hurt is moving");

        app.world_mut().write_message(DamageDealt {
            target: instance,
            source: crate::common::damage::DamageSource::World,
            amount: 10,
            remaining: 65,
            point: Vec3::ZERO,
            from: None,
            cause: default(),
        });
        app.update();
        assert!(at(&app).translation.y > 0.01, "the crate did not jolt: {:?}", at(&app));
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(at(&app), Transform::IDENTITY, "the crate never settled");
    }

    fn adopt_pickup(app: &mut App, style: PickupStyle) {
        let mut doc = crate::prop::document::PropDoc::new("hovering");
        doc.push(crate::prop::feature::FeatureOp::Primitive {
            shape: crate::prop::solid::Shape::Box { size: [0.2; 3] },
            placement: Default::default(),
            surface: Default::default(),
        });
        doc.archetype = Archetype::Pickup;
        doc.pickup.style = style;
        adopt(app, "hovering", &doc);
    }

    fn adopt(app: &mut App, name: &str, doc: &crate::prop::document::PropDoc) {
        let world = app.world_mut();
        world.resource_scope(|world, mut cache: Mut<PropCache>| {
            world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
                world.resource_scope(|world, mut materials: Mut<Assets<LitMaterial>>| {
                    world.resource_scope(|world, mut surfaces: Mut<SurfaceMaterials>| {
                        let mut clips = world.resource_mut::<Assets<Clip>>();
                        cache.adopt(name, doc, &mut meshes, &mut materials, &mut surfaces, &mut clips, None);
                    });
                });
            });
        });
    }

    #[test]
    fn a_placed_fan_turns_on_its_idle() {
        use crate::common::skeleton::clip::{Channel, Key, Track};
        use crate::common::skeleton::interpolation::Interpolation;

        let mut app = app_with_assets();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)));
        let mut doc = crate::prop::document::PropDoc::new("fan");
        let blade = doc.push(crate::prop::feature::FeatureOp::Primitive {
            shape: crate::prop::solid::Shape::Box { size: [0.6, 0.02, 0.1] },
            placement: Default::default(),
            surface: Default::default(),
        });
        doc.archetype = Archetype::Obstacle;
        doc.bones.push(crate::prop::rig::PropBone { parts: vec![blade], ..crate::prop::rig::PropBone::new("rotor", Vec3::ZERO) });
        let mut idle = Clip { length: 1.0, looping: true, ..default() };
        idle.tracks.push(Track::of(Channel::PropBone {
            bone: "rotor".into(),
            keys: vec![
                Key::all(0.0, [0.0, 0.0, 0.0], Interpolation::Linear),
                Key::all(1.0, [0.0, std::f32::consts::PI, 0.0], Interpolation::Linear),
            ],
        }));
        doc.animations.clips.insert(IDLE_CLIP.into(), idle);
        adopt(&mut app, "fan", &doc);
        app.world_mut().spawn((PropMarker, PropModel("fan".into()), Transform::default()));
        for _ in 0..4 {
            app.update();
        }
        let world = app.world_mut();
        let drawn = world.query::<&DrawnProp>().single(world).unwrap();
        let turned = world.get::<Transform>(drawn.parts[0]).unwrap().rotation;
        assert!(turned.angle_between(Quat::IDENTITY) > 0.1, "the fan stood still");
    }

    /// Fails if a client, which is sent the instance and draws it, does not hide a pickup taken
    /// on the server and show it again when it is back.
    #[test]
    fn a_client_hides_a_pickup_the_server_says_is_taken() {
        let mut app = app_with_assets();
        app.insert_resource(crate::common::net::NetRole::Client { host: "localhost".into(), port: 1 });
        adopt_pickup(&mut app, PickupStyle::Quake);
        let instance = app
            .world_mut()
            .spawn((WorldProp { model: "hovering".into(), at: Vec3::ZERO, rotation: Quat::IDENTITY }, Pickup::default()))
            .id();
        let shown = |app: &mut App| {
            app.update();
            app.update();
            let world = app.world_mut();
            let drawn = world.get::<DrawnProp>(instance).expect("the client drew nothing").parts.clone();
            assert!(!drawn.is_empty());
            drawn.iter().all(|part| world.get::<Visibility>(*part) != Some(&Visibility::Hidden))
        };
        assert!(shown(&mut app), "a pickup standing was hidden");
        app.world_mut().get_mut::<Pickup>(instance).unwrap().taken = true;
        assert!(!shown(&mut app), "a pickup taken was still drawn");
        app.world_mut().get_mut::<Pickup>(instance).unwrap().taken = false;
        assert!(shown(&mut app), "a pickup back was still hidden");
    }

    /// Fails if a machine that becomes a client keeps the pickups it stood up alone, which stand
    /// where the server's do and are never taken, or if going back to solo does not stand them up.
    #[test]
    fn a_client_draws_only_the_servers_pickups() {
        let mut app = app_with_assets();
        adopt_pickup(&mut app, PickupStyle::Quake);
        app.world_mut().spawn((PropMarker, PropModel("hovering".into()), Transform::default()));
        let ours = |app: &mut App| {
            app.update();
            app.update();
            let world = app.world_mut();
            world.query_filtered::<(), With<WorldProp>>().iter(world).count()
        };
        assert_eq!(ours(&mut app), 1, "solo stood up no pickup");
        app.insert_resource(crate::common::net::NetRole::Client { host: "localhost".into(), port: 1 });
        assert_eq!(ours(&mut app), 0, "a client kept the pickup it stood up alone");
        app.insert_resource(crate::common::net::NetRole::Solo);
        assert_eq!(ours(&mut app), 1, "back to solo, the pickup was not stood up again");
    }

    /// Fails if a TF2 pickup turns while it is being placed, or stops turning
    /// in a round.
    #[test]
    fn a_tf2_pickup_turns_only_in_a_round() {
        let mut app = app_with_assets();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)));
        adopt_pickup(&mut app, PickupStyle::Tf2);
        app.world_mut().spawn((PropMarker, PropModel("hovering".into()), Transform::default()));
        let turn = |app: &mut App| {
            for _ in 0..3 {
                app.update();
            }
            let world = app.world_mut();
            let drawn = world.query::<&DrawnProp>().single(world).unwrap();
            world.get::<Transform>(drawn.parts[0]).unwrap().rotation
        };

        assert_eq!(turn(&mut app), Quat::IDENTITY, "it turned in the editor");
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        assert_ne!(turn(&mut app), Quat::IDENTITY, "it stood still in a round");
    }

    /// Fails if flipping the style moves what the pickup is taken through, or
    /// if flipping it back leaves the drawing in the air.
    #[test]
    fn a_tf2_pickup_hovers_where_a_quake_one_sits() {
        let mut app = app_with_assets();
        adopt_pickup(&mut app, PickupStyle::Tf2);
        let at = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut().spawn((PropMarker, PropModel("hovering".into()), Transform::from_translation(at)));
        app.update();
        app.update();

        let world = app.world_mut();
        let (prop, drawn) = world.query::<(&WorldProp, &DrawnProp)>().single(world).unwrap();
        assert_eq!(prop.at, at, "the instance moved with its drawing");
        let part = drawn.parts[0];
        assert_eq!(world.get::<Transform>(part).unwrap().translation, Vec3::Y * HOVER);

        adopt_pickup(&mut app, PickupStyle::Quake);
        app.update();
        app.update();
        let world = app.world_mut();
        let drawn = world.query::<&DrawnProp>().single(world).unwrap();
        assert_eq!(*world.get::<Transform>(drawn.parts[0]).unwrap(), Transform::IDENTITY);
    }

    #[test]
    fn a_placed_pickup_is_not_solid() {
        if std::fs::metadata("assets/default/props/health_pack.gpp").is_err() {
            return;
        }
        let mut app = app_with_assets();
        app.world_mut().spawn((PropMarker, PropModel("health_pack".into()), Transform::IDENTITY));
        app.update();
        app.update();

        let world = app.world_mut();
        assert!(world.query_filtered::<(), (With<WorldProp>, With<Pickup>)>().single(world).is_ok());
        world.run_system_once(collide_with_props).unwrap();
        assert!(world.resource::<CollisionWorld>().props.is_empty(), "a health pack is a wall");
    }

    #[test]
    fn colliders_reach_the_collision_world_by_their_roles() {
        use crate::prop::collider::ColliderShape;
        use crate::prop::feature::FeatureOp;
        let mut app = app_with_assets();
        let mut doc = crate::prop::document::PropDoc::new("rack");
        doc.archetype = Archetype::Obstacle;
        for roles in [vec![ColliderRole::Solid, ColliderRole::Shots], vec![ColliderRole::Solid], vec![ColliderRole::Trigger]] {
            doc.push(FeatureOp::Collider {
                roles: roles.into_iter().collect(),
                shape: ColliderShape::Box { size: [0.5; 3] },
                placement: crate::prop::profile::Placement { origin: [0.0, 0.25, 0.0], rotation: [0.0, 0.5, 0.0] },
            });
        }
        adopt(&mut app, "rack", &doc);
        app.world_mut().spawn((PropMarker, PropModel("rack".into()), Transform::from_xyz(3.0, 0.0, 0.0)));
        app.update();
        app.update();

        let world = app.world_mut();
        world.run_system_once(collide_with_props).unwrap();
        let collision = world.resource::<CollisionWorld>();
        assert_eq!((collision.props.len(), collision.shots.len()), (2, 1));
        assert!(collision.props[0].overlaps(Vec3::new(3.0, 0.25, 0.0), Vec3::splat(0.1)), "not where the prop stands");
    }

    fn touching(world: &mut World, role: ColliderRole) -> Entity {
        let mut placed = PlacedColliders::default();
        placed.0[role.index()].push(PropShape::sphere(Vec3::new(0.0, 1.0, 0.0), 0.5));
        world.spawn(placed);
        world.init_resource::<Messages<crate::common::damage::Kill>>();
        let inside = world.spawn((Player::default(), PhysicsBody::at(Vec3::new(0.0, 1.0, 0.7)), Stance::Standing)).id();
        world.spawn((Player::default(), PhysicsBody::at(Vec3::new(5.0, 1.0, 0.0)), Stance::Standing));
        world.run_system_once(act_on_bodies_touching_props).unwrap();
        inside
    }

    #[test]
    fn a_killing_collider_kills_only_whoever_touches_it() {
        let mut world = World::new();
        let inside = touching(&mut world, ColliderRole::Kills);
        let kills: Vec<_> = world.resource_mut::<Messages<crate::common::damage::Kill>>().drain().collect();
        assert_eq!(kills.len(), 1, "{kills:?}");
        assert_eq!(kills[0].target, inside);
    }

    #[test]
    fn an_igniting_collider_sets_alight_only_whoever_touches_it() {
        let mut world = World::new();
        let inside = touching(&mut world, ColliderRole::Ignites);
        let mut alight = world.query_filtered::<Entity, With<crate::common::flame::Burning>>();
        assert_eq!(alight.iter(&world).collect::<Vec<_>>(), vec![inside]);
        assert!(world.resource::<Messages<crate::common::damage::Kill>>().is_empty());
    }

    fn rifle() -> (WeaponCatalog, Equipped) {
        let mut catalog = WeaponCatalog::default();
        catalog.insert(Weapon {
            key: "rifle".into(),
            id: WeaponId::of("rifle"),
            name_key: "weapon.names.rifle".into(),
            model: None,
            icon: None,
            bucket: 1,
            deploy: 0.0,
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            scoped_primary: None,
            magazine: Magazine { clip: Some(10), reserve: 40, reload: 1.0, reload_one_at_a_time: false, first_reload: None },
        });
        let rifle = WeaponId::of("rifle");
        (catalog, Equipped::carrying([rifle]))
    }

    fn with_launcher(catalog: &mut WeaponCatalog) -> WeaponId {
        let id = WeaponId::of("launcher");
        catalog.insert(Weapon {
            key: "launcher".into(),
            id,
            name_key: "weapon.names.launcher".into(),
            model: Some("rocket_launcher".into()),
            icon: None,
            bucket: 4,
            deploy: 0.0,
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            scoped_primary: None,
            magazine: Magazine { clip: Some(4), reserve: 20, reload: 1.0, reload_one_at_a_time: true, first_reload: None },
        });
        id
    }

    fn gives(weapon: WeaponId) -> GivesWeapon {
        GivesWeapon { weapon, ammo: 0.5, respawn: 15.0, style: PickupStyle::Tf2 }
    }

    /// Runs [`give_weapon`] on a body carrying `equipped`, returning what it
    /// gained and how it came out.
    fn take_weapon(
        gives: GivesWeapon,
        equipped: Equipped,
        ammo: Ammo,
        catalog: &WeaponCatalog,
    ) -> (bool, Equipped, Ammo, Trigger) {
        take_weapon_drawing(gives, equipped, ammo, catalog, true)
    }

    fn take_weapon_drawing(
        gives: GivesWeapon,
        equipped: Equipped,
        ammo: Ammo,
        catalog: &WeaponCatalog,
        draws: bool,
    ) -> (bool, Equipped, Ammo, Trigger) {
        let mut world = World::new();
        let body = world.spawn((equipped, ammo)).id();
        let mut trigger = Trigger::default();
        let mut query = world.query::<(&mut Equipped, &mut Ammo)>();
        let (mut e, mut a) = query.get_mut(&mut world, body).unwrap();
        let gained = give_weapon(&gives, &mut e, &mut a, &mut trigger, catalog, draws);
        (gained, world.get::<Equipped>(body).unwrap().clone(), world.get::<Ammo>(body).unwrap().clone(), trigger)
    }

    #[test]
    fn a_weapon_not_carried_is_added_with_full_ammo() {
        let (mut catalog, equipped) = rifle();
        let launcher = with_launcher(&mut catalog);

        let (gained, equipped, ammo, _) = take_weapon(gives(launcher), equipped, Ammo::default(), &catalog);

        assert!(gained);
        assert_eq!(equipped.carried(), [WeaponId::of("rifle"), launcher]);
        assert_eq!((ammo.at(launcher).unwrap().clip, ammo.at(launcher).unwrap().reserve), (4, 20));
    }

    /// A body taunting keeps the weapon its taunt may be using.
    #[test]
    fn a_weapon_taken_mid_taunt_is_carried_but_not_drawn() {
        let (mut catalog, equipped) = rifle();
        let launcher = with_launcher(&mut catalog);
        let held = equipped.held();

        let (gained, equipped, _, _) = take_weapon_drawing(gives(launcher), equipped, Ammo::default(), &catalog, false);

        assert!(gained);
        assert!(equipped.carries(launcher));
        assert_eq!(equipped.held(), held, "the pickup was drawn mid-taunt");
    }

    #[test]
    fn a_weapon_already_carried_tops_up_its_reserve_and_then_is_walked_past() {
        let (catalog, equipped) = rifle();
        let mut ammo = Ammo::default();
        ammo.at_mut(WeaponId::of("rifle")).reserve = 10;

        let (gained, equipped, ammo, _) = take_weapon(gives(WeaponId::of("rifle")), equipped, ammo, &catalog);
        assert!(gained);
        assert_eq!(ammo.at(WeaponId::of("rifle")).unwrap().reserve, 30);

        let (gained, _, ammo, _) = take_weapon(gives(WeaponId::of("rifle")), equipped.clone(), ammo, &catalog);
        assert!(gained);
        assert_eq!(ammo.at(WeaponId::of("rifle")).unwrap().reserve, 40, "topped up past the reserve");
        let (gained, _, _, _) = take_weapon(gives(WeaponId::of("rifle")), equipped, ammo, &catalog);
        assert!(!gained, "a full reserve took it");
    }

    /// The class's six loadout slots are no bound on what a body carries.
    #[test]
    fn a_body_carrying_many_weapons_still_takes_a_new_one() {
        let (mut catalog, _) = rifle();
        let launcher = with_launcher(&mut catalog);
        let many: Vec<WeaponId> = (0..12).map(|n| WeaponId::of(&format!("filler {n}"))).collect();
        let equipped = Equipped::carrying(many);

        let (gained, after, _, _) = take_weapon(gives(launcher), equipped, Ammo::default(), &catalog);
        assert!(gained);
        assert_eq!(after.carried().len(), 13);
        assert_eq!(after.held(), Some(launcher));
    }

    #[test]
    fn a_new_weapon_is_drawn_and_one_already_carried_is_not() {
        let (mut catalog, equipped) = rifle();
        let launcher = with_launcher(&mut catalog);
        catalog.weapons.iter_mut().find(|weapon| weapon.id == launcher).unwrap().deploy = 0.5;
        let mut ammo = Ammo::default();
        ammo.at_mut(WeaponId::of("rifle")).reloading = 1.0;

        let (_, drawn, ammo, trigger) = take_weapon(gives(launcher), equipped, ammo, &catalog);
        assert_eq!(drawn.held(), Some(launcher));
        assert!(!trigger.deployed(), "it was drawn without a deploy");
        assert_eq!(ammo.at(WeaponId::of("rifle")).unwrap().reloading, 0.0, "the rifle went on reloading in a holster");

        let mut low = ammo.clone();
        low.at_mut(WeaponId::of("rifle")).reserve = 0;
        let (gained, after, _, trigger) = take_weapon(gives(WeaponId::of("rifle")), drawn.clone(), low, &catalog);
        assert!(gained);
        assert_eq!(after.held(), Some(launcher), "topping up the rifle drew it");
        assert!(trigger.deployed());
    }

    #[test]
    fn a_weapon_hovers_with_its_lowest_point_at_the_hover_height_in_either_style() {
        for style in PickupStyle::ALL {
            let mut world = World::new();
            world.insert_resource(Time::<()>::default());
            world.insert_resource(State::new(AppMode::Editor));
            world.init_resource::<PropCache>();
            world.insert_resource(Assets::<Clip>::default());
            let part = world.spawn((WorldPropPart, Transform::IDENTITY)).id();
            world.spawn((
                WorldProp { model: "launcher".into(), at: Vec3::ZERO, rotation: Quat::IDENTITY },
                DrawnProp { model: "launcher".into(), generation: 0, parts: vec![part] },
                GivesWeapon { style, ..gives(WeaponId::of("launcher")) },
                // The grip is 0.2 m above the weapon's lowest point.
                RestsOn(0.2),
            ));

            world.run_system_once(place_world_prop_parts).unwrap();
            assert_eq!(world.get::<Transform>(part).unwrap().translation.y, 0.2 + WEAPON_HOVER, "{style:?}");
        }
    }

    #[test]
    fn a_weapon_pickup_stands_up_drawn_as_its_weapon_and_goes_with_its_feature() {
        let mut world = World::new();
        let (mut catalog, _) = rifle();
        let launcher = with_launcher(&mut catalog);
        world.insert_resource(catalog);
        let spec = crate::editor::weapon_pickup::WeaponPickupSpec {
            weapon: launcher,
            ammo: 0.5,
            respawn: 15.0,
            style: PickupStyle::Tf2,
        };
        let feature = world.spawn((spec, Transform::from_xyz(1.0, 0.0, 2.0))).id();

        world.run_system_once(sync_weapon_pickups).unwrap();
        world.flush();
        let (prop, gives) = world.query::<(&WorldProp, &GivesWeapon)>().single(&world).unwrap();
        assert_eq!(prop.model, "rocket_launcher");
        assert_eq!(prop.at, Vec3::new(1.0, 0.0, 2.0));
        assert_eq!(gives.weapon, launcher);

        world.despawn(feature);
        world.run_system_once(sync_weapon_pickups).unwrap();
        world.flush();
        assert_eq!(world.query::<&WorldProp>().iter(&world).count(), 0);
    }

    #[test]
    fn a_pickup_of_a_weapon_nobody_defines_stands_nothing_up() {
        let mut world = World::new();
        world.insert_resource(WeaponCatalog::default());
        let spec = crate::editor::weapon_pickup::WeaponPickupSpec {
            weapon: WeaponId::of("nothing"),
            ammo: 0.5,
            respawn: 15.0,
            style: PickupStyle::Tf2,
        };
        world.spawn((spec, Transform::IDENTITY));

        world.run_system_once(sync_weapon_pickups).unwrap();
        world.flush();
        assert_eq!(world.query::<&WorldProp>().iter(&world).count(), 0);
    }

    fn give_to(spec: PickupSpec, health: &mut Damageable, ammo: &mut Ammo, equipped: &Equipped, catalog: &WeaponCatalog) -> bool {
        let mut world = World::new();
        let body = world.spawn((*health, ammo.clone())).id();
        let mut query = world.query::<(&mut Damageable, &mut Ammo)>();
        let (h, a) = query.get_mut(&mut world, body).unwrap();
        let gained = give(&spec, Some(h), None, Some((a, equipped)), catalog).is_some();
        *health = *world.get::<Damageable>(body).unwrap();
        *ammo = world.get::<Ammo>(body).unwrap().clone();
        gained
    }

    #[test]
    fn a_health_pack_heals_by_a_fraction_and_is_walked_past_at_full() {
        let (catalog, equipped) = rifle();
        let spec = PickupSpec { kind: PickupKind::Health, amount: 0.25, respawn: 1.0, ..default() };
        let mut ammo = Ammo::default();

        let mut health = Damageable::with_health(200);
        assert!(!give_to(spec, &mut health, &mut ammo, &equipped, &catalog), "full health took it");

        health.apply(120);
        assert!(give_to(spec, &mut health, &mut ammo, &equipped, &catalog));
        assert_eq!(health.health(), 130);
    }

    #[test]
    fn an_ammo_pack_tops_up_reserves_and_no_further() {
        let (catalog, equipped) = rifle();
        let spec = PickupSpec { kind: PickupKind::Ammo, amount: 0.5, respawn: 1.0, ..default() };
        let mut health = Damageable::with_health(100);
        let mut ammo = Ammo::default();
        ammo.at_mut(WeaponId::of("rifle")).reserve = 30;

        assert!(give_to(spec, &mut health, &mut ammo, &equipped, &catalog));
        assert_eq!(ammo.at(WeaponId::of("rifle")).unwrap().reserve, 40, "topped up past the reserve");
        assert!(!give_to(spec, &mut health, &mut ammo, &equipped, &catalog), "full reserves took it");
    }

    #[test]
    fn a_flag_gives_nothing_yet() {
        let (catalog, equipped) = rifle();
        let mut health = Damageable::with_health(100);
        health.apply(50);
        let mut ammo = Ammo::default();
        let spec = PickupSpec { kind: PickupKind::Flag, amount: 1.0, respawn: 1.0, ..default() };
        assert!(!give_to(spec, &mut health, &mut ammo, &equipped, &catalog));
    }

    #[test]
    fn armor_fills_by_a_fraction_of_its_maximum_and_is_walked_past_full() {
        let mut world = World::new();
        let body = world.spawn(Armor::with_max(100)).id();
        let spec = PickupSpec { kind: PickupKind::Armor, amount: 0.6, respawn: 1.0, ..default() };
        let take = |world: &mut World| {
            let armor = world.get_mut::<Armor>(body).unwrap();
            give(&spec, None, Some(armor), None, &WeaponCatalog::default()).is_some()
        };

        assert!(take(&mut world));
        assert_eq!(world.get::<Armor>(body).unwrap().points(), 60);
        assert!(take(&mut world), "a part-full body was refused");
        assert!(world.get::<Armor>(body).unwrap().is_full());
        assert!(!take(&mut world), "full armor took it");
    }
}
