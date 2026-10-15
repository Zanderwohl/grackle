//! Putting a weapon's model in a body's hand.
//!
//! Stage 1 of [the plan](../../documentation/weapons-in-hand.md), and
//! deliberately only that: the model is hung off `hand.r` and the body goes on
//! animating exactly as it did. **It will look wrong while walking** — the arms
//! swing through their cycle with a launcher along for the ride — because the
//! upper-body layer that fixes it is stage 3, and inverting the relationship
//! (place the weapon, solve the arms to it) is that stage's whole job.
//!
//! What this stage is for is proving `Weapon.model` end to end: a name in
//! `weapons.toml`, a document read through [`AssetSource`], a mesh in a hand,
//! on every machine.
//!
//! **Nothing is cached on arrival.** `Equipped`, the catalogue and the model
//! all turn up on their own schedules, so what is drawn is compared with what
//! is held and redrawn when they differ — the same "key on absence, not on
//! arrival" rule the corpse and body-dressing layers already follow. A weapon
//! held before its catalogue entry lands draws nothing and then draws.
//!
//! [`AssetSource`]: crate::common::assets::AssetSource

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

/// How far above a point to start looking for the floor, and how far below it
/// to keep looking.
///
/// Generous both ways on purpose: a foot keyed to the ground while the body is
/// mid-stride is above it, and one on a step down is below.
const GROUND_PROBE: f32 = 2.0;
const GROUND_DROP: f32 = 4.0;

/// One drawn part of the weapon a body is holding.
#[derive(Component)]
pub struct HeldPart;

/// Where the weapon is, in the body's own frame.
///
/// Written by [`hold_the_weapon`] and by nothing else, and read by the drawing
/// and by the arms alike — which is the inversion stage 3 is about. Stage 1
/// placed the model *from* the hand; now the hand is solved to the weapon, so
/// the weapon has to be decided first and written down somewhere both can see.
#[derive(Component, Default)]
pub struct WeaponInHand(pub Transform);

/// What is currently drawn in a body's hand, and what it was drawn for.
///
/// The id is what makes this self-correcting: a switch changes it, and a
/// catalogue arriving late means `resolved` is still false and the next frame
/// tries again. Without `resolved` a weapon whose entry had not arrived would
/// be recorded as "nothing to draw" and never revisited.
#[derive(Component, Default)]
#[require(ClipStack, Performance)]
pub struct HeldModel {
    weapon: Option<WeaponId>,
    /// Whether the catalogue and the prop cache have both had their say —
    /// which they may have done by answering "there is no model", and that is
    /// an answer worth keeping.
    resolved: bool,
    /// Which version of the cache that answer came from.
    ///
    /// A model sent by a server arrives after a client may already have gone
    /// looking in its own packs and cached the miss. Without this, "there is
    /// no such prop" would be the answer for the rest of the match.
    cached_at: u64,
    parts: Vec<Entity>,
    /// Taken off the model when there is one, so the pose layer never has to
    /// reach for the prop cache.
    hold: HoldSpec,
    /// And where the viewmodel hangs it, which is a different question.
    viewmodel: ViewmodelSpec,
    /// What it does over time, by name: handles straight off the bake, so a
    /// save re-inserted under them is what plays.
    clips: BTreeMap<String, Handle<Clip>>,
}

/// What a body's weapon is in the middle of doing.
///
/// A name and a clock, and deliberately not a handle: the handles come off
/// `HeldModel`, which is rebuilt whenever the weapon or the cache changes, and
/// a re-bake may have dropped the clip altogether. Keyed on absence the way
/// everything else in this file is — the name is looked up every frame and
/// one that resolves to nothing simply stops.
#[derive(Component, Default)]
pub struct Performance {
    /// What is running, and how far into it, on the clip's own clock.
    playing: Option<(String, f32)>,
    /// Which weapon it was started for. A switch is what restarts it, and
    /// comparing is what makes that need no event.
    started_for: Option<WeaponId>,
}

impl HeldModel {
    /// The weapon the parts were built for, and which version of the cache
    /// they came from — the two questions anything mirroring this has to ask
    /// as well.
    pub fn showing(&self) -> (Option<WeaponId>, u64) {
        (self.weapon, self.cached_at)
    }

    /// The drawn parts, for anything that wants to copy them.
    pub fn parts(&self) -> &[Entity] {
        &self.parts
    }

    pub fn hold(&self) -> &HoldSpec {
        &self.hold
    }

    pub fn viewmodel(&self) -> ViewmodelSpec {
        self.viewmodel
    }
}

pub struct HeldWeaponPlugin;

impl Plugin for HeldWeaponPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ClipAssetsPlugin)
            .init_resource::<PropCache>()
            .init_resource::<SurfaceMaterials>()
            .add_systems(Update, dress_held_weapons)
            // With the bone meshes, before propagation: a held part is placed
            // from the same posed bones and would otherwise be drawn a frame
            // behind the hand carrying it.
            .add_systems(
                PostUpdate,
                place_held_weapons.before(TransformSystems::Propagate),
            );
    }
}

/// What a body's hands resolve to.
///
/// Split out of the system so the question can be asked without a `World`: the
/// three cases are the whole of why `HeldModel` needs a `resolved` flag, and
/// they are easy to get subtly wrong when they are three arms of a match
/// nested in a loop.
#[derive(Debug, PartialEq)]
enum Wanted<'a> {
    /// Something is held, and the catalogue has not heard of it yet. An id and
    /// its catalogue arrive on different channels, so this is ordinary — ask
    /// again next frame.
    Unknown,
    /// Nothing to draw, and that is settled: empty hands, or a weapon nobody
    /// has modelled.
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

/// Hang the held weapon's model on any body whose hands have changed.
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
            // Recorded only if the *held weapon* changed, so waiting for a
            // catalogue does not re-insert the component every frame.
            Wanted::Unknown => {
                if drawn.is_none_or(|drawn| drawn.weapon != held) {
                    replace_parts(
                        &mut commands, body, drawn, vec![], held, false, HoldSpec::default(),
                        ViewmodelSpec::default(), BTreeMap::new(), cache.generation(),
                    );
                }
            }
            Wanted::Nothing => {
                replace_parts(
                    &mut commands, body, drawn, vec![], held, true, HoldSpec::default(),
                    ViewmodelSpec::default(), BTreeMap::new(), cache.generation(),
                )
            }
            Wanted::Model(model) => {
                let mut spawned = Vec::new();
                let mut hold = HoldSpec::default();
                let mut viewmodel = ViewmodelSpec::default();
                let mut clips = BTreeMap::new();
                if let Some(baked) = cache.ensure(
                    model, &packs, &mut meshes, &mut materials, &mut surfaces, &mut clip_assets,
                ) {
                    let parts = baked.parts.clone();
                    hold = baked.hold.for_class(class.copied());
                    viewmodel = baked.viewmodel;
                    clips = baked.clips.clone();
                    commands.entity(body).with_children(|body| {
                        for (mesh, material) in parts {
                            spawned.push(
                                body.spawn((
                                    HeldPart,
                                    // Hidden from inside your own head, along
                                    // with the body it hangs off.
                                    FirstPersonHidden,
                                    // Spawned *already* hidden when it should
                                    // be. `hide_own_body` would catch it, but
                                    // not until the frame after the command
                                    // lands — which is a weapon flashing
                                    // across your own view every time you
                                    // switch to it.
                                    match ours.is_some() && !view.shows_own_body() {
                                        true => Visibility::Hidden,
                                        false => Visibility::Inherited,
                                    },
                                    Mesh3d(mesh),
                                    MeshMaterial3d(material),
                                    // Overwritten by `place_held_weapons`
                                    // before anything is drawn; the identity
                                    // would put it at the body's feet.
                                    Transform::IDENTITY,
                                ))
                                .id(),
                            );
                        }
                    });
                }
                let cached_at = cache.generation();
                replace_parts(
                    &mut commands, body, drawn, spawned, held, true, hold, viewmodel, clips,
                    cached_at,
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
            HeldModel { weapon, resolved, parts, hold, viewmodel, clips, cached_at },
            WeaponInHand::default(),
        ));
}

/// Put the weapon where the hold says it goes.
///
/// The first half of what used to be one system, and the split is what lets a
/// clip animate a weapon at all: a track in [`Space::Weapon`] needs the weapon
/// *placed* before it can be read against, and the arms have to be solved onto
/// wherever the clip left it — so the clip layer goes between the two halves
/// rather than before or after the whole thing.
///
/// Runs after the animator and the look pass, composing onto the pose they
/// left. The lower body is untouched, which is the point: a hold has to
/// survive whatever the legs are doing.
///
/// [`Space::Weapon`]: crate::common::skeleton::Space::Weapon
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
        // `-Z` is forward in the body's own frame and a positive rotation about
        // `X` lifts it — the opposite sign to a spine bone, whose `+Y` runs up
        // its own length.
        let aim = Quat::from_rotation_x(player.pitch);

        let Some(placed) = weapon_transform(skeleton, pose, &root, &held.hold, aim) else {
            continue;
        };
        weapon.0 = placed;
    }
}

/// Start a clip when a weapon is brought out, and keep the running one ticking.
///
/// The whole of the driving layer for now, and deliberately small. A switch is
/// noticed by **comparing** rather than by an event — the same "key on
/// absence, not on arrival" rule the rest of this file follows — so a weapon
/// whose catalogue entry or model turned up late still deploys when it finally
/// resolves, rather than having missed its one chance.
///
/// A one-shot is dropped when it runs out. That is seamless here because a
/// deploy composes onto the carry and ends at no offset at all, so the last
/// frame of the clip and the first frame without it are the same pose — a clip
/// that ended somewhere else would need blending out, which is what
/// `Playing::weight` is for and what a real performance layer will use.
///
/// **Not simulation.** Nothing here is predicted, replicated or reconciled: a
/// deploy is something you watch, and `WeaponActionFired` is where a fire will
/// come from when there is one. How a remote viewer is told is the open
/// question this stage deliberately leaves alone.
pub(crate) fn perform_weapon_clips(
    time: Res<Time>,
    clips: Res<Assets<Clip>>,
    mut bodies: Query<(&HeldModel, &mut Performance, &mut ClipStack)>,
) {
    let delta = time.delta_secs();

    for (held, mut performance, mut stack) in &mut bodies {
        // Bringing out a different weapon restarts the performance; the same
        // weapon re-resolving — a re-bake, a catalogue arriving — does not,
        // because redrawing a model is not drawing it again.
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

        // The name is looked up every frame and its handle resolved in the
        // assets, rather than either being cached: a model saved in the prop
        // editor mid-round is inserted under the same handle, so it is the
        // one being performed a frame later, and a model re-baked without
        // this clip simply ends it.
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

/// The clip a weapon plays when it is brought out.
///
/// A convention rather than a field: a prop that has one gets a deploy and one
/// that does not simply appears, which is the same reading `Weapon.model`
/// already takes of an absent model.
pub const DEPLOY: &str = "deploy";

/// Run whatever clips a body has on it, between the weapon being placed and
/// the hands being put on it.
///
/// The whole arrangement stage 6 rests on. A weapon clip dips, kicks or swings
/// `WeaponInHand`, and `grip_the_weapon` — which runs next — solves both arms
/// onto wherever it ended up. So a recoil is authored once, in the weapon's
/// own file, and is correct for a Scout and a Heavy alike, with no hand ever
/// keyed and no pose authored per class.
///
/// **The ground closure is the honest half of `Axis::Ground`.** In a match it
/// is a ray down the collision world; with no collision built — the editor,
/// a test, a body standing in an animation grid — it is the body's own floor.
/// Degrading to a plane is a decision rather than a fallback: a clip authored
/// against "nothing" would play differently from how it was drawn, which is
/// the failure the weapons plan already names about holds.
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

        // Read once, before the closure borrows anything: the floor a body is
        // standing on is what a plane-shaped ground answers with.
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
            // A viewmodel clip is framed against the eye, and a third-person
            // body has no viewmodel — so this is `None` here and filled in by
            // the viewmodel pass, which solves in eye space already.
            eye: None,
            ground: &ground,
            // A weapon clip runs on seconds, and has no feet to follow.
            footfalls: None,
        };

        let mut moving = placed.unwrap_or_default();
        apply_stack(stack, &clips, skeleton, &mut pose, &frames, Some(&mut moving));
        if let Some(weapon) = weapon.as_deref_mut() {
            weapon.0 = moving;
        }
    }
}

/// Solve both arms onto wherever the weapon ended up.
///
/// The second half, and it deliberately reads `WeaponInHand` rather than
/// recomputing the placement: whatever moved the weapon between the two — a
/// recoil, a reload, nothing at all — is invisible from here, which is exactly
/// what makes a weapon clip need no per-hand authoring and work on every build.
///
/// All of the work is [`prop::hold`]'s, because the reference figure in the
/// prop editor does exactly this and the two must not be able to disagree.
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

/// Put every drawn part where the hold decided the weapon is.
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

    /// The three cases, and the reason `HeldModel` carries a `resolved` flag at
    /// all: two of them mean "nothing is drawn" and only one of them means
    /// "stop asking". An id and its catalogue entry arrive on different
    /// channels, so a weapon the catalogue has not heard of yet is ordinary.
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

    /// The whole of stage 1, end to end: a name in a weapon, a document read
    /// off disk, meshes on an entity hung off the body. Uses the prop the
    /// default pack actually ships, so a rename or a broken file fails here.
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
            // Hung off the body, or `hide_own_body` will not find it and a
            // launcher is left across your own lens in first person.
            assert_eq!(world.get::<ChildOf>(*part).map(|parent| parent.parent()), Some(held));
        }

        let bare = world.get::<HeldModel>(empty).expect("the body was never dressed");
        assert!(bare.resolved, "an unmodelled weapon should not be retried");
        assert!(bare.parts.is_empty());
    }

    const EVERY_CLASS: [Class; 11] = [
        Class::Scout, Class::Soldier, Class::Pyro, Class::Demoman,
        Class::Heavy, Class::Engineer, Class::Medic, Class::Sniper,
        Class::Spy, Class::Civilian, Class::Mercenary,
    ];

    fn body_holding(world: &mut World, class: Class, pitch: f32) -> Entity {
        let skeleton = humanoid(class.proportions());
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
                    clips: BTreeMap::new(),
                    cached_at: 0,
                },
                WeaponInHand::default(),
            ))
            .id()
    }

    /// The hold's whole reason for existing: the weapon goes where the body is
    /// aiming, in front of the chest, and it tilts with the pitch. Stage 1 put
    /// it wherever the walk cycle had left the hand.
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
        // `-Z` is forward, so a weapon aimed up has a forward vector with a
        // positive `y`.
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

    /// Bringing out a weapon starts its deploy, and the deploy ends.
    ///
    /// Both halves matter. A clip that never started is a weapon that appears;
    /// one that never ended is a body stuck holding its launcher halfway up
    /// for the rest of the match, and neither of those errors.
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
                    clips,
                    cached_at: 0,
                },
                Performance::default(),
                ClipStack::default(),
            ))
            .id();

        // First tick: the weapon is new, so the deploy is on the stack.
        world.run_system_once(perform_weapon_clips).unwrap();
        assert_eq!(world.get::<ClipStack>(body).unwrap().0.len(), 1, "nothing deployed");

        // And it is still running a moment later, rather than restarting —
        // the weapon has not changed, and a re-resolve is not a re-draw.
        world.run_system_once(perform_weapon_clips).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        assert!(stack.0[0].time > 0.1, "the deploy did not advance: {}", stack.0[0].time);

        // Past its length it comes off, and the body simply holds the weapon.
        for _ in 0..8 {
            world.run_system_once(perform_weapon_clips).unwrap();
        }
        assert!(
            world.get::<ClipStack>(body).unwrap().0.is_empty(),
            "the deploy never ended",
        );
    }

    /// **A save mid-deploy is what the deploy plays next**, through the
    /// handle the body already holds. The prop editor publishes a save by
    /// adopting it, the cache inserts the new clip under the old handle, and
    /// nothing copies a clip onto the body to get it there — the stack keeps
    /// the handle it started with and simply samples something new.
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
        cache.adopt("launcher", &saved(0.5), &mut meshes, &mut materials, &mut surfaces, &mut assets);
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
            cache.adopt("launcher", &saved(3.0), &mut meshes, &mut materials, &mut surfaces, &mut assets);
        }
        assert_eq!(cache.get("launcher").unwrap().clips[DEPLOY], playing, "the save minted a new handle");

        // Past the old length, the deploy is still running: it is the new clip
        // that is being played, through the handle the body already had.
        for _ in 0..8 {
            world.run_system_once(perform_weapon_clips).unwrap();
        }
        let stack = world.get::<ClipStack>(body).unwrap();
        assert_eq!(stack.0.len(), 1, "the deploy ended at the length it was saved with first");
        assert_eq!(stack.0[0].clip, playing);
        assert_eq!(world.resource::<Assets<Clip>>().get(&playing).unwrap().length, 3.0);
    }

    /// The reason `hold_the_weapon` was split in three.
    ///
    /// A clip moves the weapon between the placing and the gripping, and the
    /// hands follow it there — without the clip knowing a hand exists and
    /// without a single key being authored for one. That is what makes a
    /// recoil written once correct on every build, and it is the whole of what
    /// the split buys: placing and gripping in one system leaves nowhere for a
    /// clip to go.
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

        // The same body twice: once held plainly, once with a clip that drops
        // the weapon a fifth of a metre.
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

    /// **Every class has to be able to reach its own weapon.** A carry is
    /// authored in fractions of arm length precisely so it retargets, and a
    /// build whose hand cannot get to the grip holds nothing while its arm
    /// swings past it.
    #[test]
    fn every_class_can_get_its_trigger_hand_onto_the_grip() {
        for class in EVERY_CLASS {
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

    /// The drawn parts go wherever `hold_the_weapon` decided, and nowhere
    /// else. A part left at the identity sits at the body's feet.
    #[test]
    fn a_held_part_is_placed_where_the_hold_put_the_weapon() {
        let decided = Transform::from_xyz(0.1, 1.2, -0.3)
            .with_rotation(Quat::from_rotation_x(0.4));

        let mut world = World::new();
        let part = world.spawn((HeldPart, Transform::IDENTITY)).id();
        world.spawn((
            HeldModel {
                weapon: None, resolved: true, parts: vec![part],
                hold: HoldSpec::default(), viewmodel: ViewmodelSpec::default(),
                clips: BTreeMap::new(), cached_at: 0,
            },
            WeaponInHand(decided),
        ));

        world.run_system_once(place_held_weapons).unwrap();
        assert_eq!(*world.get::<Transform>(part).unwrap(), decided);
    }
}
