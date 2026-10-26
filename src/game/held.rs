//! Putting a weapon's model in a body's hands. The weapon is placed first,
//! weapon clips move it, and then both arms are solved onto it. See
//! [the plan](../../documentation/weapons-in-hand.md).
//!
//! Nothing is cached on arrival: `Equipped`, the catalogue and the model each
//! arrive on their own schedule, so what is drawn is compared with what is
//! held every frame and redrawn when they differ.

use bevy::prelude::*;

use crate::common::assets::Assets as PackAssets;
use crate::common::class::Class;
use std::collections::BTreeMap;

use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{apply_stack, ClipAssetsPlugin, ClipStack, Frames, Playing};
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::weapon::{Equipped, WeaponCatalogue, WeaponId};
use crate::game::body_mesh::FirstPersonHidden;
use crate::game::player::{LocalPlayer, Player, ViewMode};
use crate::game::skeleton::SkeletonRoot;
use crate::prop::baked::{PropCache, SurfaceMaterials};
use crate::game::collision::CollisionWorld;
use crate::prop::hold::{carry_frame, grip_with, weapon_transform, HoldSpec, ViewmodelSpec};

/// Metres above a point to start looking for the floor, and below it to keep
/// looking. Generous: a keyed foot mid-stride is above the ground, and one on a
/// step down is below.
const GROUND_PROBE: f32 = 2.0;
const GROUND_DROP: f32 = 4.0;

/// One drawn part of the weapon a body is holding.
#[derive(Component)]
pub struct HeldPart;

/// Where the weapon is, in the body's own frame. Written by
/// [`place_the_weapon`] and weapon clips; read by the arms and the drawing.
#[derive(Component, Default)]
pub struct WeaponInHand(pub Transform);

/// What is currently drawn in a body's hand, and what it was drawn for.
#[derive(Component, Default)]
#[require(ClipStack, Performance)]
pub struct HeldModel {
    weapon: Option<WeaponId>,
    /// The catalogue and prop cache have both answered, possibly with "no
    /// model". Without it, a weapon whose catalogue entry had not arrived yet
    /// would be recorded as nothing to draw and never revisited.
    resolved: bool,
    /// Cache generation the answer came from. A server's model can arrive after
    /// a client cached a miss from its own packs.
    cached_at: u64,
    parts: Vec<Entity>,
    /// Copied off the model so the pose layer never needs the prop cache.
    hold: HoldSpec,
    viewmodel: ViewmodelSpec,
    /// In the model's own space.
    muzzle: Option<Vec3>,
    /// Handles off the bake, so a save refilled under them is what plays.
    clips: BTreeMap<String, Handle<Clip>>,
}

/// What a body's weapon is in the middle of doing. A clip name rather than a
/// handle, looked up in `HeldModel` every frame, so a re-bake cannot leave a
/// stale clip playing.
#[derive(Component, Default)]
pub struct Performance {
    /// Clip name and seconds into it.
    playing: Option<(String, f32)>,
    /// A change of weapon restarts the clip; comparing needs no event.
    started_for: Option<WeaponId>,
}

impl HeldModel {
    /// The weapon the parts were built for, and the cache generation they came
    /// from.
    pub fn showing(&self) -> (Option<WeaponId>, u64) {
        (self.weapon, self.cached_at)
    }

    pub fn parts(&self) -> &[Entity] {
        &self.parts
    }

    pub fn hold(&self) -> &HoldSpec {
        &self.hold
    }

    pub fn viewmodel(&self) -> ViewmodelSpec {
        self.viewmodel
    }

    pub fn muzzle(&self) -> Option<Vec3> {
        self.muzzle
    }
}

pub struct HeldWeaponPlugin;

impl Plugin for HeldWeaponPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ClipAssetsPlugin)
            .init_resource::<PropCache>()
            .init_resource::<SurfaceMaterials>()
            .add_systems(Update, dress_held_weapons)
            // Before propagation, with the bone meshes, or a held part is drawn
            // a frame behind the hand.
            .add_systems(
                PostUpdate,
                place_held_weapons.before(TransformSystems::Propagate),
            );
    }
}

/// What a body's hands resolve to. Split out so the three cases can be tested
/// without a `World`.
#[derive(Debug, PartialEq)]
enum Wanted<'a> {
    /// Held, but the catalogue entry has not arrived (it comes on a different
    /// channel). Ask again next frame.
    Unknown,
    /// Settled: empty hands, or a weapon with no model.
    Nothing,
    Model(&'a str),
}

fn wanted_model<'a>(equipped: &Equipped, catalogue: &'a WeaponCatalogue) -> Wanted<'a> {
    let Some(id) = equipped.held() else {
        return Wanted::Nothing;
    };
    let Some(weapon) = catalogue.get(id) else {
        return Wanted::Unknown;
    };
    match weapon.model.as_deref() {
        Some(model) => Wanted::Model(model),
        None => Wanted::Nothing,
    }
}

#[allow(clippy::too_many_arguments)]
fn dress_held_weapons(
    mut commands: Commands,
    catalogue: Res<WeaponCatalogue>,
    packs: Res<PackAssets>,
    mut cache: ResMut<PropCache>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clip_assets: ResMut<Assets<Clip>>,
    view: Res<ViewMode>,
    bodies: Query<
        (Entity, &Equipped, Option<&Class>, Option<&LocalPlayer>, Option<&HeldModel>),
        With<Skeleton>,
    >,
) {
    for (body, equipped, class, ours, drawn) in &bodies {
        let held = equipped.held();
        if let Some(drawn) = drawn
            && drawn.weapon == held
            && drawn.resolved
            && drawn.cached_at == cache.generation()
        {
            continue;
        }

        match wanted_model(equipped, &catalogue) {
            // Recorded only if the held weapon changed, so waiting for a
            // catalogue does not re-insert the component every frame.
            Wanted::Unknown => {
                if drawn.is_none_or(|drawn| drawn.weapon != held) {
                    replace_parts(
                        &mut commands, body, drawn, vec![], held, false, HoldSpec::default(),
                        ViewmodelSpec::default(), None, BTreeMap::new(), cache.generation(),
                    );
                }
            }
            Wanted::Nothing => {
                replace_parts(
                    &mut commands, body, drawn, vec![], held, true, HoldSpec::default(),
                    ViewmodelSpec::default(), None, BTreeMap::new(), cache.generation(),
                )
            }
            Wanted::Model(model) => {
                let mut spawned = Vec::new();
                let mut hold = HoldSpec::default();
                let mut viewmodel = ViewmodelSpec::default();
                let mut muzzle = None;
                let mut clips = BTreeMap::new();
                if let Some(baked) = cache.ensure(
                    model, &packs, &mut meshes, &mut materials, &mut surfaces, &mut clip_assets,
                ) {
                    let parts = baked.parts.clone();
                    hold = baked.hold.for_class(class.copied());
                    viewmodel = baked.viewmodel;
                    muzzle = baked.muzzle.map(|muzzle| muzzle.point());
                    clips = baked.clips.clone();
                    // Before the parts: a carousel body is spawned without one.
                    commands.entity(body).insert_if_new(Visibility::default()).with_children(|body| {
                        for (mesh, material) in parts {
                            spawned.push(
                                body.spawn((
                                    HeldPart,
                                    FirstPersonHidden,
                                    // Spawned already hidden: `hide_own_body`
                                    // only catches it a frame later, which
                                    // flashes the weapon across your view.
                                    match ours.is_some() && !view.shows_own_body() {
                                        true => Visibility::Hidden,
                                        false => Visibility::Inherited,
                                    },
                                    Mesh3d(mesh),
                                    MeshMaterial3d(material),
                                    // Overwritten by `place_held_weapons`
                                    // before it is drawn.
                                    Transform::IDENTITY,
                                ))
                                .id(),
                            );
                        }
                    });
                }
                let cached_at = cache.generation();
                replace_parts(
                    &mut commands, body, drawn, spawned, held, true, hold, viewmodel, muzzle,
                    clips, cached_at,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn replace_parts(
    commands: &mut Commands,
    body: Entity,
    drawn: Option<&HeldModel>,
    parts: Vec<Entity>,
    weapon: Option<WeaponId>,
    resolved: bool,
    hold: HoldSpec,
    viewmodel: ViewmodelSpec,
    muzzle: Option<Vec3>,
    clips: BTreeMap<String, Handle<Clip>>,
    cached_at: u64,
) {
    if let Some(drawn) = drawn {
        for part in &drawn.parts {
            commands.entity(*part).despawn();
        }
    }
    commands
        .entity(body)
        .insert((
            HeldModel { weapon, resolved, parts, hold, viewmodel, muzzle, clips, cached_at },
            WeaponInHand::default(),
        ));
}

/// Put the weapon where the hold says it goes, composing onto the animator and
/// look pass. Split from [`grip_the_weapon`] so weapon clips can run between
/// placing the weapon and solving the arms onto it.
pub(crate) fn place_the_weapon(
    mut bodies: Query<(
        &Player,
        &Skeleton,
        &Pose,
        Option<&SkeletonRoot>,
        &HeldModel,
        &mut WeaponInHand,
    )>,
) {
    for (player, skeleton, pose, offset, held, mut weapon) in &mut bodies {
        if !held.resolved || held.weapon.is_none() {
            continue;
        }

        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        // `-Z` is forward and positive `X` rotation lifts it: the opposite sign
        // to a spine bone.
        let aim = Quat::from_rotation_x(player.pitch);

        let Some(placed) = weapon_transform(skeleton, pose, &root, &held.hold, aim) else {
            continue;
        };
        weapon.0 = placed;
    }
}

/// Start the deploy clip when a weapon is brought out, and tick the running
/// clip. A switch is noticed by comparing, so a weapon that resolves late still
/// deploys.
///
/// A one-shot is dropped when it ends. That is seamless only because a deploy
/// ends at zero offset; a clip ending elsewhere would need blending out via
/// `Playing::weight`.
///
/// Not simulation: nothing here is predicted, replicated or reconciled.
pub(crate) fn perform_weapon_clips(
    time: Res<Time>,
    clips: Res<Assets<Clip>>,
    mut bodies: Query<(&HeldModel, &mut Performance, &mut ClipStack)>,
) {
    let delta = time.delta_secs();

    for (held, mut performance, mut stack) in &mut bodies {
        // A different weapon restarts; the same weapon re-resolving (a re-bake,
        // a late catalogue) does not.
        if performance.started_for != held.weapon {
            performance.started_for = held.weapon;
            performance.playing = held
                .clips
                .get(DEPLOY)
                .map(|_| (DEPLOY.to_string(), 0.0));
        }

        let Some((name, elapsed)) = performance.playing.as_mut() else {
            stop(&mut stack);
            continue;
        };
        *elapsed += delta;

        // Name and handle resolved every frame: a save mid-round is refilled
        // under the same handle, and a re-bake without this clip ends it.
        let found = held
            .clips
            .get(name.as_str())
            .and_then(|handle| Some((handle, clips.get(handle)?)));
        let Some((handle, clip)) = found else {
            performance.playing = None;
            stop(&mut stack);
            continue;
        };
        if !clip.looping && *elapsed > clip.length {
            performance.playing = None;
            stop(&mut stack);
            continue;
        }

        let elapsed = *elapsed;
        match stack.0.as_mut_slice() {
            // The common case: the same clip, a little further on.
            [playing] if playing.clip == *handle => playing.time = elapsed,
            _ => {
                stack.0.clear();
                stack.0.push(Playing { clip: handle.clone(), time: elapsed, weight: 1.0 });
            }
        }
    }
}

/// Take everything off a stack, without marking an empty one changed.
fn stop(stack: &mut Mut<ClipStack>) {
    if !stack.0.is_empty() {
        stack.0.clear();
    }
}

/// The clip a weapon plays when it is brought out. A prop without one simply
/// appears.
pub const DEPLOY: &str = "deploy";

/// Run a body's clips between placing the weapon and gripping it, so a weapon
/// clip moves `WeaponInHand` and [`grip_the_weapon`] solves the arms onto
/// wherever it ended up. A recoil is authored once and fits every class.
///
/// `Axis::Ground` is a ray down the collision world in a match, and the body's
/// own floor with no collision built (editor, tests, animation grid).
pub(crate) fn play_the_clips(
    mut bodies: Query<(
        &Player,
        &Skeleton,
        &mut Pose,
        Option<&SkeletonRoot>,
        &ClipStack,
        Option<&mut WeaponInHand>,
    )>,
    clips: Res<Assets<Clip>>,
    world: Option<Res<CollisionWorld>>,
) {
    for (player, skeleton, mut pose, offset, stack, weapon) in &mut bodies {
        if stack.0.is_empty() {
            continue;
        }

        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        let aim = Quat::from_rotation_x(player.pitch);

        // Read before the closure borrows anything.
        let floor = root.translation.y;
        let world = world.as_deref();
        let ground = move |at: Vec3| match world {
            Some(world) => {
                let from = at + Vec3::Y * GROUND_PROBE;
                world
                    .ray_hit(&Ray3d::new(from, Dir3::NEG_Y), GROUND_PROBE + GROUND_DROP)
                    .map(|(distance, _)| from.y - distance)
                    .unwrap_or(floor)
            }
            None => floor,
        };

        let mut weapon = weapon;
        let placed = weapon.as_deref().map(|weapon| weapon.0);
        let frames = Frames {
            body: root,
            carry: carry_frame(skeleton, &pose, &root, aim).unwrap_or(root),
            weapon: placed,
            // Filled by the viewmodel pass, which solves in eye space.
            eye: None,
            ground: &ground,
            // Weapon clips run on seconds, not footfalls.
            footfalls: None,
        };

        let mut moving = placed.unwrap_or_default();
        apply_stack(stack, &clips, skeleton, &mut pose, &frames, Some(&mut moving));
        if let Some(weapon) = weapon.as_deref_mut() {
            weapon.0 = moving;
        }
    }
}

/// Solve both arms onto `WeaponInHand`, wherever clips left it. The work is in
/// [`prop::hold`], shared with the prop editor's reference figure so the two
/// cannot disagree.
///
/// [`prop::hold`]: crate::prop::hold
pub(crate) fn grip_the_weapon(
    mut bodies: Query<(
        &Skeleton,
        &mut Pose,
        Option<&SkeletonRoot>,
        &HeldModel,
        &WeaponInHand,
    )>,
) {
    for (skeleton, mut pose, offset, held, weapon) in &mut bodies {
        if !held.resolved || held.weapon.is_none() {
            continue;
        }

        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        grip_with(skeleton, &mut pose, &root, &held.hold, &weapon.0);
    }
}

fn place_held_weapons(
    bodies: Query<(&HeldModel, &WeaponInHand)>,
    mut parts: Query<&mut Transform, With<HeldPart>>,
) {
    for (held, weapon) in &bodies {
        for part in &held.parts {
            let Ok(mut transform) = parts.get_mut(*part) else { continue };
            *transform = weapon.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use strum::IntoEnumIterator;
    use super::*;
    use crate::common::weapon::{Magazine, Mounted, Slot, Weapon};
    use crate::common::class::Class;
    use crate::common::skeleton::rig::bone;
    use crate::common::skeleton::{default_humanoid, humanoid, Pose};

    fn weapon(name: &str, model: Option<&str>) -> Weapon {
        Weapon {
            id: WeaponId::of(name),
            name_key: format!("weapon.names.{name}"),
            model: model.map(str::to_owned),
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            magazine: Magazine::BOTTOMLESS,
        }
    }

    fn holding(id: Option<WeaponId>) -> Equipped {
        let mut equipped = Equipped::default();
        equipped.slots[Slot::Primary.index()] = id;
        equipped.held = Slot::Primary;
        equipped
    }

    /// Two cases draw nothing, but only a missing catalogue entry is retried.
    #[test]
    fn an_unknown_weapon_is_asked_about_again_and_an_unmodelled_one_is_not() {
        let mut catalogue = WeaponCatalogue::default();
        catalogue.insert(weapon("modelled", Some("rocket_launcher")));
        catalogue.insert(weapon("bare", None));

        assert_eq!(
            wanted_model(&holding(Some(WeaponId::of("modelled"))), &catalogue),
            Wanted::Model("rocket_launcher"),
        );
        assert_eq!(
            wanted_model(&holding(Some(WeaponId::of("bare"))), &catalogue),
            Wanted::Nothing,
            "a weapon nobody has modelled is an answer, not a thing to retry",
        );
        assert_eq!(
            wanted_model(&holding(Some(WeaponId::of("never heard of"))), &catalogue),
            Wanted::Unknown,
            "a weapon whose catalogue entry is still in flight must be retried",
        );
        assert_eq!(wanted_model(&holding(None), &catalogue), Wanted::Nothing);
    }

    /// A weapon's model name reaches meshes on the body, using the prop the
    /// default pack ships so a rename or broken file fails here.
    #[test]
    fn a_modelled_weapon_reaches_the_hand() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(Assets::<Clip>::default());
        world.init_resource::<PropCache>();
        world.init_resource::<SurfaceMaterials>();
        world.init_resource::<PackAssets>();
        world.init_resource::<ViewMode>();

        let mut catalogue = WeaponCatalogue::default();
        catalogue.insert(weapon("launcher", Some("rocket_launcher")));
        catalogue.insert(weapon("bare", None));
        world.insert_resource(catalogue);

        let held = world
            .spawn((
                default_humanoid().clone(),
                Pose::rest(),
                holding(Some(WeaponId::of("launcher"))),
            ))
            .id();
        let empty = world
            .spawn((
                default_humanoid().clone(),
                Pose::rest(),
                holding(Some(WeaponId::of("bare"))),
            ))
            .id();

        world.run_system_once(dress_held_weapons).unwrap();

        let drawn = world.get::<HeldModel>(held).expect("the body was never dressed");
        assert!(drawn.resolved);
        assert!(!drawn.parts.is_empty(), "the launcher's model never reached the hand");
        for part in &drawn.parts {
            assert!(world.get::<HeldPart>(*part).is_some());
            // Hung off the body, or `hide_own_body` will not find it in first
            // person.
            assert_eq!(world.get::<ChildOf>(*part).map(|parent| parent.parent()), Some(held));
        }

        let bare = world.get::<HeldModel>(empty).expect("the body was never dressed");
        assert!(bare.resolved, "an unmodelled weapon should not be retried");
        assert!(bare.parts.is_empty());
    }


    fn body_holding(world: &mut World, class: Class, pitch: f32) -> Entity {
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
        world.init_resource::<Assets<Clip>>();
        world
            .spawn((
                skeleton,
                Pose::rest(),
                Player { pitch, ..default() },
                HeldModel {
                    weapon: Some(WeaponId::of("something")),
                    resolved: true,
                    parts: vec![],
                    hold: HoldSpec::default(),
                    viewmodel: ViewmodelSpec::default(),
                    muzzle: None,
                    clips: BTreeMap::new(),
                    cached_at: 0,
                },
                WeaponInHand::default(),
            ))
            .id()
    }

    /// The weapon sits in front of the chest and tilts with the pitch.
    #[test]
    fn the_weapon_is_carried_in_front_of_the_chest_and_aims_with_the_pitch() {
        let mut world = World::new();
        let level = body_holding(&mut world, Class::Soldier, 0.0);
        let up = body_holding(&mut world, Class::Soldier, 0.6);
        world.run_system_once(place_the_weapon).unwrap();

        let at = |body| world.get::<WeaponInHand>(body).unwrap().0;
        let level = at(level);
        let up = at(up);

        assert!(level.translation.z < -0.1, "the weapon is not out in front: {}", level.translation);
        assert!(level.translation.y > 1.0, "the weapon is not up at chest height");
        // `-Z` is forward, so aiming up gives a positive `y`.
        assert!(
            (level.rotation * Vec3::NEG_Z).y.abs() < 1e-3,
            "a level body is not aiming level",
        );
        assert!(
            (up.rotation * Vec3::NEG_Z).y > 0.5,
            "pitching up did not raise the weapon: {}",
            (up.rotation * Vec3::NEG_Z),
        );
        assert!(up.translation.y > level.translation.y, "aiming up did not lift the weapon");
    }

    /// A deploy that never started is a weapon that just appears; one that
    /// never ended leaves the weapon halfway up for the rest of the match.
    #[test]
    fn bringing_out_a_weapon_deploys_it_once() {
        use std::time::Duration;

        let mut world = World::new();
        let mut assets = Assets::<Clip>::default();
        let clips = BTreeMap::from([(
            DEPLOY.to_string(),
            assets.add(Clip { length: 0.4, ..Clip::default() }),
        )]);
        world.insert_resource(assets);
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(0.1));
        world.insert_resource(time);

        let body = world
            .spawn((
                HeldModel {
                    weapon: Some(WeaponId::of("something")),
                    resolved: true,
                    parts: vec![],
                    hold: HoldSpec::default(),
                    viewmodel: ViewmodelSpec::default(),
                    muzzle: None,
                    clips,
                    cached_at: 0,
                },
                Performance::default(),
                ClipStack::default(),
            ))
            .id();

        world.run_system_once(perform_weapon_clips).unwrap();
        assert_eq!(world.get::<ClipStack>(body).unwrap().0.len(), 1, "nothing deployed");

        // Still running a moment later: same weapon, no restart.
        world.run_system_once(perform_weapon_clips).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        assert!(stack.0[0].time > 0.1, "the deploy did not advance: {}", stack.0[0].time);

        for _ in 0..8 {
            world.run_system_once(perform_weapon_clips).unwrap();
        }
        assert!(
            world.get::<ClipStack>(body).unwrap().0.is_empty(),
            "the deploy never ended",
        );
    }

    /// Fails if a save mid-deploy does not reach the deploy already playing,
    /// through the handle the body already holds.
    #[test]
    fn a_mid_round_save_reaches_a_deploy_already_playing() {
        use crate::prop::document::PropDoc;
        use std::time::Duration;

        let saved = |length: f32| {
            let mut doc = PropDoc::new("prop.untitled");
            doc.animations
                .clips
                .insert(DEPLOY.into(), Clip { length, ..Clip::default() });
            doc
        };

        let mut cache = PropCache::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut surfaces = SurfaceMaterials::default();
        let mut assets = Assets::<Clip>::default();
        cache.adopt("launcher", &saved(0.5), &mut meshes, &mut materials, &mut surfaces, &mut assets, None);
        let clips = cache.get("launcher").unwrap().clips.clone();

        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(0.1));
        world.insert_resource(time);
        world.insert_resource(assets);
        let body = world
            .spawn((
                HeldModel {
                    weapon: Some(WeaponId::of("launcher")),
                    resolved: true,
                    parts: vec![],
                    hold: HoldSpec::default(),
                    viewmodel: ViewmodelSpec::default(),
                    muzzle: None,
                    clips,
                    cached_at: 0,
                },
                Performance::default(),
                ClipStack::default(),
            ))
            .id();

        world.run_system_once(perform_weapon_clips).unwrap();
        let playing = world.get::<ClipStack>(body).unwrap().0[0].clip.clone();

        // Saved again, longer — and re-adopted under the same name.
        {
            let mut assets = world.resource_mut::<Assets<Clip>>();
            cache.adopt("launcher", &saved(3.0), &mut meshes, &mut materials, &mut surfaces, &mut assets, None);
        }
        assert_eq!(cache.get("launcher").unwrap().clips[DEPLOY], playing, "the save minted a new handle");

        // Past the old length and still running: the new clip, same handle.
        for _ in 0..8 {
            world.run_system_once(perform_weapon_clips).unwrap();
        }
        let stack = world.get::<ClipStack>(body).unwrap();
        assert_eq!(stack.0.len(), 1, "the deploy ended at the length it was saved with first");
        assert_eq!(stack.0[0].clip, playing);
        assert_eq!(world.resource::<Assets<Clip>>().get(&playing).unwrap().length, 3.0);
    }

    /// A clip moves the weapon between placing and gripping, and the hands
    /// follow without any hand keys.
    #[test]
    fn a_clip_moves_the_weapon_and_the_hands_follow_it() {
        use crate::common::skeleton::clip::{Axis, Channel, Clip, Combine, Key, Space, Track, Unit};
        use crate::common::skeleton::play::{ClipStack, Playing};
        use crate::common::skeleton::Interpolation;

        let wrist = |world: &World, body: Entity| {
            let skeleton = world.get::<Skeleton>(body).unwrap().clone();
            let pose = world.get::<Pose>(body).unwrap().clone();
            let index = skeleton.index_of(crate::common::skeleton::rig::bone::FOREARM_R).unwrap();
            skeleton.posed_bones(&pose, &Transform::IDENTITY)[index].tail
        };

        // The same body twice: held plainly, and with a clip that drops the
        // weapon 0.2 m.
        let mut plain = World::new();
        let still = body_holding(&mut plain, Class::Soldier, 0.0);
        plain.run_system_once(place_the_weapon).unwrap();
        plain.run_system_once(play_the_clips).unwrap();
        plain.run_system_once(grip_the_weapon).unwrap();

        let mut world = World::new();
        let moved = body_holding(&mut world, Class::Soldier, 0.0);
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                space: Space::Carry,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Weapon {
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free, Axis::Value(-0.2), Axis::Free],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };
        let clip = world.resource_mut::<Assets<Clip>>().add(clip);
        world.entity_mut(moved).insert(ClipStack(vec![Playing {
            clip,
            time: 0.0,
            weight: 1.0,
        }]));
        world.run_system_once(place_the_weapon).unwrap();
        world.run_system_once(play_the_clips).unwrap();
        world.run_system_once(grip_the_weapon).unwrap();

        let dropped = world.get::<WeaponInHand>(moved).unwrap().0;
        let held = plain.get::<WeaponInHand>(still).unwrap().0;
        assert!(
            (held.translation.y - dropped.translation.y - 0.2).abs() < 1e-4,
            "the clip did not move the weapon: {} then {}",
            held.translation.y,
            dropped.translation.y,
        );

        assert!(
            wrist(&plain, still).y - wrist(&world, moved).y > 0.05,
            "the weapon moved and the hand stayed behind: {} then {}",
            wrist(&plain, still).y,
            wrist(&world, moved).y,
        );
    }

    /// A carry is in fractions of arm length so it retargets; a class whose
    /// hand cannot reach the grip holds nothing.
    #[test]
    fn every_class_can_get_its_trigger_hand_onto_the_grip() {
        for class in Class::iter() {
            let mut world = World::new();
            let body = body_holding(&mut world, class, 0.0);
            world.run_system_once(place_the_weapon).unwrap();
            world.run_system_once(grip_the_weapon).unwrap();

            let skeleton = world.get::<Skeleton>(body).unwrap().clone();
            let pose = world.get::<Pose>(body).unwrap().clone();
            let weapon = world.get::<WeaponInHand>(body).unwrap().0;

            let index = skeleton.index_of(bone::HAND_R).expect("bone is in the rig");
            let hand = &skeleton.posed_bones(&pose, &Transform::IDENTITY)[index];
            let grip = weapon.transform_point(HoldSpec::default().grip());

            assert!(
                (hand.head - grip).length() < 0.02,
                "{class:?} holds the grip at {} while it sits at {grip}",
                hand.head,
            );
        }
    }

    /// A drawn part goes where the hold put the weapon; left at identity it sits
    /// at the body's feet.
    #[test]
    fn a_held_part_is_placed_where_the_hold_put_the_weapon() {
        let decided = Transform::from_xyz(0.1, 1.2, -0.3)
            .with_rotation(Quat::from_rotation_x(0.4));

        let mut world = World::new();
        let part = world.spawn((HeldPart, Transform::IDENTITY)).id();
        world.spawn((
            HeldModel {
                weapon: None, resolved: true, parts: vec![part],
                hold: HoldSpec::default(), viewmodel: ViewmodelSpec::default(), muzzle: None,
                clips: BTreeMap::new(), cached_at: 0,
            },
            WeaponInHand(decided),
        ));

        world.run_system_once(place_held_weapons).unwrap();
        assert_eq!(*world.get::<Transform>(part).unwrap(), decided);
    }
}
