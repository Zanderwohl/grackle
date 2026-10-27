//! Props placed in a map: obstacles, breakables and pickups. See
//! [the plan](../../documentation/prop-archetypes.md).
//!
//! A map `Prop` feature exists on every machine, but a breakable has health and
//! a pickup can be taken, so something has to be believed about each. As with
//! carousel bodies, the authority stands one replicated [`WorldProp`] up per
//! feature that names a model, and every machine draws and collides with those.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::assets::Assets as PackAssets;
use crate::common::class::{Class, Stance};
use crate::common::damage::{Armor, Damageable};
use crate::common::hitbox::{Box3, Hitboxes};
use crate::common::skeleton::clip::Clip;
use crate::common::weapon::{Ammo, Equipped, Slot, WeaponCatalog, WeaponId};
use crate::editor::prop::{PropMarker, PropModel};
use crate::editor::weapon_pickup::WeaponPickupSpec;
use crate::game::collision::CollisionWorld;
use crate::game::player::{PhysicsBody, Player};
use crate::game::weapon::{draw, Trigger};
use crate::prop::archetype::{Archetype, Collider, ColliderShape, PickupKind, PickupSpec, PickupStyle, RoundedBox};
use crate::prop::baked::{BakedProp, PropCache, SurfaceMaterials};

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
struct DrawnProp {
    model: String,
    generation: u64,
    parts: Vec<Entity>,
}

#[derive(Component)]
struct WorldPropPart;

/// How far a weapon's parts are lifted so its lowest point is where it
/// stands: a held prop's origin is its grip, not its base. Measured when it is
/// dressed.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
struct RestsOn(f32);

/// The trigger a pickup with no colliders is taken through, in meters.
const DEFAULT_PICKUP_RADIUS: f32 = 0.4;

/// The trigger a weapon is taken through, in meters, centered on where it
/// hovers.
const WEAPON_PICKUP_RADIUS: f32 = 0.6;

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
        app.init_resource::<PropCache>()
            .init_resource::<SurfaceMaterials>()
            .add_systems(
                Update,
                // Not mode-gated: props are map content, standing in the
                // editor as in a round. Authority-gated: see the module docs.
                (sync_world_props, sync_weapon_pickups, kit_world_props)
                    .chain()
                    .run_if(crate::common::net::has_authority),
            )
            // Not authority-gated, as the room rebuild beside it is not: a
            // client predicting its movement walks into its own copy.
            .add_systems(
                FixedUpdate,
                collide_with_props
                    .after(crate::game::rebuild_collision_when_rooms_change)
                    .before(crate::game::player::step_player)
                    .run_if(in_state(AppMode::Play)),
            )
            // Ungated: a client draws what it is sent.
            .add_systems(Update, (place_world_props, dress_world_props, float_pickups, box_breakables).chain())
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
        let wanted = catalog.get(spec.weapon).map(|weapon| {
            (
                WorldProp {
                    model: weapon.model.clone().unwrap_or_default(),
                    at: placed.translation,
                    rotation: placed.rotation,
                },
                GivesWeapon { weapon: spec.weapon, ammo: spec.ammo, respawn: spec.respawn, style: spec.style },
            )
        });
        let existing = stands.and_then(|stands| instances.get_mut(stands.0).ok());

        match (wanted, stands, existing) {
            (None, Some(stands), _) => {
                commands.entity(stands.0).try_despawn();
                commands.entity(feature).remove::<StandsFor>();
            }
            (None, None, _) => {}
            (Some(wanted), None, _) => {
                let instance = commands.spawn((wanted, Pickup::default(), OfPropFeature(feature))).id();
                commands.entity(feature).insert(StandsFor(instance));
            }
            // Gone without the feature going: put back on the round's edge.
            (Some(_), Some(_), None) => {}
            (Some((prop, gives)), Some(_), Some((_, mut was_prop, mut was_gives, _))) => {
                if *was_prop != prop {
                    *was_prop = prop;
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
    mut materials: ResMut<Assets<StandardMaterial>>,
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
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clips: ResMut<Assets<Clip>>,
    instances: Query<(Entity, &WorldProp, Option<&DrawnProp>, Option<&Pickup>, Has<GivesWeapon>)>,
    mut parts: Query<&mut Visibility, With<WorldPropPart>>,
) {
    for (entity, prop, drawn, pickup, weapon) in &instances {
        let visibility = match pickup.is_some_and(|pickup| pickup.taken) {
            true => Visibility::Hidden,
            false => Visibility::Inherited,
        };

        let current = drawn.filter(|drawn| {
            drawn.model == prop.model && drawn.generation == cache.generation()
        });
        if let Some(drawn) = current {
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
                    .filter_map(|(mesh, _)| match meshes.get(mesh)?.attribute(Mesh::ATTRIBUTE_POSITION)? {
                        bevy::mesh::VertexAttributeValues::Float32x3(positions) => {
                            positions.iter().map(|p| p[1]).reduce(f32::min)
                        }
                        _ => None,
                    })
                    .fold(f32::INFINITY, f32::min);
                commands.entity(entity).insert(RestsOn(if lowest.is_finite() { -lowest } else { 0.0 }));
            }
            commands.entity(entity).with_children(|instance| {
                for (mesh, material) in parts {
                    spawned.push(
                        instance
                            .spawn((WorldPropPart, Mesh3d(mesh), MeshMaterial3d(material), visibility))
                            .id(),
                    );
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

/// Lift and turn the parts of a hovering pickup, leaving the instance where it
/// stands so its trigger does not move with the drawing. Purely drawn: the
/// angle is this machine's clock, and nobody needs to agree on it.
/// Still while authoring, so a mapper lines up a pickup that is not turning,
/// but lifted, so it stands at the height it will in a round.
fn float_pickups(
    time: Res<Time>,
    mode: Res<State<AppMode>>,
    cache: Res<PropCache>,
    instances: Query<(&WorldProp, &DrawnProp, Option<&GivesWeapon>, Option<&RestsOn>)>,
    mut parts: Query<&mut Transform, With<WorldPropPart>>,
) {
    // Wrapped, or an `f32` of seconds steps visibly after a day. The wrap
    // period is a whole number of turns at this speed, so there is no jump.
    let spin = if mode.is_authoring() {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_y((time.elapsed_secs_wrapped() * SPIN_DEGREES_PER_SECOND).to_radians())
    };
    for (prop, drawn, gives, rests) in &instances {
        let lift = match (gives, rests) {
            (Some(_), Some(rests)) => rests.0 + WEAPON_HOVER,
            (Some(_), None) => WEAPON_HOVER,
            (None, _) => 0.0,
        };
        let style = gives.map(|gives| gives.style).or_else(|| {
            cache
                .get(&prop.model)
                .filter(|baked| baked.archetype == Archetype::Pickup)
                .map(|baked| baked.pickup.style)
        });
        let wanted = match style {
            Some(PickupStyle::Tf2) if gives.is_some() => Transform::from_xyz(0.0, lift, 0.0).with_rotation(spin),
            Some(PickupStyle::Tf2) => Transform::from_xyz(0.0, lift + HOVER, 0.0).with_rotation(spin),
            _ => Transform::from_xyz(0.0, lift, 0.0),
        };
        for part in &drawn.parts {
            // Compared first, so a still pickup is not a changed one.
            if let Ok(mut transform) = parts.get_mut(*part)
                && *transform != wanted
            {
                *transform = wanted;
            }
        }
    }
}

/// Everything a placed prop is solid as, in the world.
pub fn placed_colliders<'a>(
    baked: &'a BakedProp,
    at: &Transform,
) -> impl Iterator<Item = RoundedBox> + 'a {
    let at = *at;
    baked.colliders.iter().map(move |collider| collider.rounded().placed(&at))
}

/// What a pickup is taken through: its colliders, or a small sphere when it
/// has none, so a pack modeled without one can still be walked into.
fn triggers(baked: &BakedProp, at: &Transform) -> Vec<RoundedBox> {
    if baked.colliders.is_empty() {
        let fallback = Collider {
            origin: [0.0; 3],
            shape: ColliderShape::Sphere { radius: DEFAULT_PICKUP_RADIUS },
        };
        return vec![fallback.rounded().placed(at)];
    }
    placed_colliders(baked, at).collect()
}

/// A breakable is shot as the box around its colliders, and has no head.
/// Every machine works this out, since a client draws hitboxes too. A prop
/// that stops being breakable loses them, or shots would go on stopping at it.
fn box_breakables(
    mut commands: Commands,
    cache: Res<PropCache>,
    instances: Query<(Entity, &WorldProp, Option<&Hitboxes>, Has<Damageable>)>,
) {
    for (entity, prop, boxes, breakable) in &instances {
        if !breakable {
            if boxes.is_some() {
                commands.entity(entity).remove::<Hitboxes>();
            }
            continue;
        }
        let Some(baked) = cache.get(&prop.model) else { continue };
        let Some(body) = bounding(placed_colliders(baked, &prop.transform())) else { continue };
        let wanted = Hitboxes { body, head: None };
        if boxes != Some(&wanted) {
            commands.entity(entity).insert(wanted);
        }
    }
}

fn bounding(shapes: impl Iterator<Item = RoundedBox>) -> Option<Box3> {
    let (min, max) = shapes.fold(None, |bounds: Option<(Vec3, Vec3)>, shape| {
        let (lo, hi) = (shape.center - shape.half_extents(), shape.center + shape.half_extents());
        Some(match bounds {
            Some((min, max)) => (min.min(lo), max.max(hi)),
            None => (lo, hi),
        })
    })?;
    Some(Box3 { center: (min + max) * 0.5, half_extents: (max - min) * 0.5 })
}

/// Build the solid props into the collision world. On every machine, so a
/// client predicting its movement walks into the same crate the server does.
/// Run each tick before the step; there are few enough props that rebuilding
/// is cheaper than knowing when not to.
pub fn collide_with_props(
    mut collision: ResMut<CollisionWorld>,
    cache: Res<PropCache>,
    instances: Query<&WorldProp>,
) {
    let collision = collision.bypass_change_detection();
    collision.props.clear();
    for prop in &instances {
        let Some(baked) = cache.get(&prop.model) else { continue };
        if !baked.archetype.blocks() {
            continue;
        }
        collision.props.extend(placed_colliders(baked, &prop.transform()));
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
            &PhysicsBody,
            &Stance,
            Option<&mut Damageable>,
            Option<&mut Armor>,
            Option<&mut Ammo>,
            Option<&mut Equipped>,
            Option<&mut Trigger>,
            Option<&Class>,
        ),
        With<Player>,
    >,
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

        let (triggers, respawn) = match gives {
            Some(gives) => (vec![weapon_trigger(&prop.transform())], gives.respawn),
            None => {
                let Some(baked) = cache.get(&prop.model) else { continue };
                (triggers(baked, &prop.transform()), baked.pickup.respawn)
            }
        };
        for (body, stance, health, armor, ammo, equipped, trigger, class) in &mut bodies {
            let half = stance.half_extents();
            if !triggers.iter().any(|trigger| trigger.overlaps(body.current, half)) {
                continue;
            }
            let gained = match gives {
                Some(gives) => {
                    let (Some(mut ammo), Some(mut equipped), Some(mut trigger)) = (ammo, equipped, trigger) else {
                        continue;
                    };
                    give_weapon(gives, &mut equipped, &mut ammo, &mut trigger, class.copied(), &catalog)
                }
                None => {
                    let Some(baked) = cache.get(&prop.model) else { continue };
                    let ammo = ammo.zip(equipped).map(|(ammo, equipped)| (ammo, equipped.into_inner() as &Equipped));
                    give(&baked.pickup, health, armor, ammo, &catalog)
                }
            };
            if !gained {
                continue;
            }
            pickup.taken = true;
            commands.entity(entity).insert(Restock(respawn.max(0.0)));
            break;
        }
    }
}

/// Returns whether the taker got anything.
fn give(
    spec: &PickupSpec,
    health: Option<Mut<Damageable>>,
    armor: Option<Mut<Armor>>,
    ammo: Option<(Mut<Ammo>, &Equipped)>,
    catalog: &WeaponCatalog,
) -> bool {
    let fraction = spec.amount.max(0.0);
    match spec.kind {
        PickupKind::Health => {
            let Some(mut health) = health else { return false };
            if health.is_full() {
                return false;
            }
            let amount = share(health.max(), fraction);
            health.heal(amount) > 0
        }
        PickupKind::Ammo => {
            let Some((mut ammo, equipped)) = ammo else { return false };
            let mut gained = false;
            for slot in Slot::ALL {
                let Some(weapon) = equipped.get(slot).and_then(|id| catalog.get(id)) else {
                    continue;
                };
                let max = weapon.magazine.reserve;
                // Read before writing, so a full body is not marked changed.
                if ammo.at(slot).reserve >= max {
                    continue;
                }
                let amount = share(max, fraction);
                let state = ammo.at_mut(slot);
                state.reserve = (state.reserve + amount).min(max);
                gained = true;
            }
            gained
        }
        PickupKind::Armor => {
            let Some(mut armor) = armor else { return false };
            if armor.is_full() {
                return false;
            }
            let amount = share(armor.max(), fraction);
            armor.add(amount) > 0
        }
        // Specced, with no behavior yet: there is no flag mode.
        PickupKind::Flag => false,
    }
}

fn weapon_trigger(at: &Transform) -> RoundedBox {
    Collider {
        origin: [0.0, WEAPON_HOVER, 0.0],
        shape: ColliderShape::Sphere { radius: WEAPON_PICKUP_RADIUS },
    }
    .rounded()
    .placed(at)
}

/// A weapon not carried goes into a free slot — the one its class's loadout
/// would put it in if that is free, else the first — with full ammo, and is
/// drawn; which digit draws it later is its bucket's business. One already
/// carried tops up its reserve and leaves the hands alone. Nothing gained, from a full reserve or no free slot, leaves the
/// pickup where it is. Returns whether the taker got anything.
fn give_weapon(
    gives: &GivesWeapon,
    equipped: &mut Mut<Equipped>,
    ammo: &mut Mut<Ammo>,
    trigger: &mut Trigger,
    class: Option<Class>,
    catalog: &WeaponCatalog,
) -> bool {
    let Some(weapon) = catalog.get(gives.weapon) else { return false };

    if let Some(slot) = Slot::ALL.into_iter().find(|slot| equipped.get(*slot) == Some(gives.weapon)) {
        let max = weapon.magazine.reserve;
        // Read before writing, so a full body is not marked changed.
        if ammo.at(slot).reserve >= max || gives.ammo <= 0.0 {
            return false;
        }
        let state = ammo.at_mut(slot);
        state.reserve = (state.reserve + share(max, gives.ammo)).min(max);
        return true;
    }

    let free = |slot: &Slot| equipped.get(*slot).is_none();
    let preferred = class.and_then(|class| catalog.loadout(class)).and_then(|loadout| {
        Slot::ALL.into_iter().filter(free).find(|slot| {
            loadout.at(*slot).is_some_and(|choices| choices.permitted.contains(&gives.weapon))
        })
    });
    let Some(slot) = preferred.or_else(|| Slot::ALL.into_iter().find(free)) else { return false };

    equipped.slots[slot.index()] = Some(gives.weapon);
    *ammo.at_mut(slot) = Ammo::full(&weapon.magazine);
    draw(slot, equipped, ammo, trigger, catalog);
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

    fn adopt_pickup(app: &mut App, style: PickupStyle) {
        let mut doc = crate::prop::document::PropDoc::new("hovering");
        doc.push(crate::prop::feature::FeatureOp::Primitive {
            shape: crate::prop::solid::Shape::Box { size: [0.2; 3] },
            placement: Default::default(),
            surface: Default::default(),
        });
        doc.archetype = Archetype::Pickup;
        doc.pickup.style = style;
        let world = app.world_mut();
        world.resource_scope(|world, mut cache: Mut<PropCache>| {
            world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
                world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                    world.resource_scope(|world, mut surfaces: Mut<SurfaceMaterials>| {
                        let mut clips = world.resource_mut::<Assets<Clip>>();
                        cache.adopt("hovering", &doc, &mut meshes, &mut materials, &mut surfaces, &mut clips, None);
                    });
                });
            });
        });
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

    fn rifle() -> (WeaponCatalog, Equipped) {
        let mut catalog = WeaponCatalog::default();
        catalog.insert(Weapon {
            key: "rifle".into(),
            id: WeaponId::of("rifle"),
            name_key: "weapon.names.rifle".into(),
            model: None,
            bucket: 1,
            deploy: 0.0,
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            magazine: Magazine { clip: Some(10), reserve: 40, reload: 1.0, reload_one_at_a_time: false },
        });
        let mut equipped = Equipped::default();
        equipped.slots[Slot::Primary.index()] = Some(WeaponId::of("rifle"));
        (catalog, equipped)
    }

    fn with_launcher(catalog: &mut WeaponCatalog) -> WeaponId {
        let id = WeaponId::of("launcher");
        catalog.insert(Weapon {
            key: "launcher".into(),
            id,
            name_key: "weapon.names.launcher".into(),
            model: Some("rocket_launcher".into()),
            bucket: 4,
            deploy: 0.0,
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            magazine: Magazine { clip: Some(4), reserve: 20, reload: 1.0, reload_one_at_a_time: true },
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
        let mut world = World::new();
        let body = world.spawn((equipped, ammo)).id();
        let mut trigger = Trigger::default();
        let mut query = world.query::<(&mut Equipped, &mut Ammo)>();
        let (mut e, mut a) = query.get_mut(&mut world, body).unwrap();
        let gained = give_weapon(&gives, &mut e, &mut a, &mut trigger, None, catalog);
        (gained, *world.get::<Equipped>(body).unwrap(), *world.get::<Ammo>(body).unwrap(), trigger)
    }

    #[test]
    fn a_weapon_not_carried_goes_into_a_free_slot_with_full_ammo() {
        let (mut catalog, equipped) = rifle();
        let launcher = with_launcher(&mut catalog);

        let (gained, equipped, ammo, _) = take_weapon(gives(launcher), equipped, Ammo::default(), &catalog);

        assert!(gained);
        let slot = Slot::ALL.into_iter().find(|slot| equipped.get(*slot) == Some(launcher)).expect("not carried");
        assert_ne!(slot, Slot::Primary, "it took the rifle's place");
        assert_eq!((ammo.at(slot).clip, ammo.at(slot).reserve), (4, 20));
        assert_eq!(equipped.get(Slot::Primary), Some(WeaponId::of("rifle")));
    }

    #[test]
    fn a_weapon_already_carried_tops_up_its_reserve_and_then_is_walked_past() {
        let (catalog, equipped) = rifle();
        let mut ammo = Ammo::default();
        ammo.at_mut(Slot::Primary).reserve = 10;

        let (gained, equipped, ammo, _) = take_weapon(gives(WeaponId::of("rifle")), equipped, ammo, &catalog);
        assert!(gained);
        assert_eq!(ammo.at(Slot::Primary).reserve, 30);

        let (gained, _, ammo, _) = take_weapon(gives(WeaponId::of("rifle")), equipped, ammo, &catalog);
        assert!(gained);
        assert_eq!(ammo.at(Slot::Primary).reserve, 40, "topped up past the reserve");
        let (gained, _, _, _) = take_weapon(gives(WeaponId::of("rifle")), equipped, ammo, &catalog);
        assert!(!gained, "a full reserve took it");
    }

    #[test]
    fn nobody_with_every_slot_full_can_take_a_new_weapon() {
        let (mut catalog, _) = rifle();
        let launcher = with_launcher(&mut catalog);
        let mut equipped = Equipped::default();
        for slot in Slot::ALL {
            equipped.slots[slot.index()] = Some(WeaponId::of("rifle"));
        }

        let (gained, after, _, _) = take_weapon(gives(launcher), equipped, Ammo::default(), &catalog);
        assert!(!gained);
        assert_eq!(after, equipped);
    }

    #[test]
    fn a_new_weapon_is_drawn_and_one_already_carried_is_not() {
        let (mut catalog, equipped) = rifle();
        let launcher = with_launcher(&mut catalog);
        catalog.weapons.iter_mut().find(|weapon| weapon.id == launcher).unwrap().deploy = 0.5;
        let mut ammo = Ammo::default();
        ammo.at_mut(Slot::Primary).reloading = 1.0;

        let (_, drawn, ammo, trigger) = take_weapon(gives(launcher), equipped, ammo, &catalog);
        assert_eq!(drawn.held(), Some(launcher));
        assert!(!trigger.deployed(), "it was drawn without a deploy");
        assert_eq!(ammo.at(Slot::Primary).reloading, 0.0, "the rifle went on reloading in a holster");

        let mut low = ammo;
        low.at_mut(Slot::Primary).reserve = 0;
        let (gained, after, _, trigger) = take_weapon(gives(WeaponId::of("rifle")), drawn, low, &catalog);
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
            let part = world.spawn((WorldPropPart, Transform::IDENTITY)).id();
            world.spawn((
                WorldProp { model: "launcher".into(), at: Vec3::ZERO, rotation: Quat::IDENTITY },
                DrawnProp { model: "launcher".into(), generation: 0, parts: vec![part] },
                GivesWeapon { style, ..gives(WeaponId::of("launcher")) },
                // The grip is 0.2 m above the weapon's lowest point.
                RestsOn(0.2),
            ));

            world.run_system_once(float_pickups).unwrap();
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
        let body = world.spawn((*health, *ammo)).id();
        let mut query = world.query::<(&mut Damageable, &mut Ammo)>();
        let (h, a) = query.get_mut(&mut world, body).unwrap();
        let gained = give(&spec, Some(h), None, Some((a, equipped)), catalog);
        *health = *world.get::<Damageable>(body).unwrap();
        *ammo = *world.get::<Ammo>(body).unwrap();
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
        ammo.at_mut(Slot::Primary).reserve = 30;

        assert!(give_to(spec, &mut health, &mut ammo, &equipped, &catalog));
        assert_eq!(ammo.at(Slot::Primary).reserve, 40, "topped up past the reserve");
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
        let body = world.spawn(Armor::default()).id();
        let spec = PickupSpec { kind: PickupKind::Armor, amount: 0.6, respawn: 1.0, ..default() };
        let take = |world: &mut World| {
            let armor = world.get_mut::<Armor>(body).unwrap();
            give(&spec, None, Some(armor), None, &WeaponCatalog::default())
        };

        assert!(take(&mut world));
        assert_eq!(world.get::<Armor>(body).unwrap().points(), 60);
        assert!(take(&mut world), "a part-full body was refused");
        assert!(world.get::<Armor>(body).unwrap().is_full());
        assert!(!take(&mut world), "full armor took it");
    }
}
