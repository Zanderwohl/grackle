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
use crate::common::skeleton::ik::{solve, Chain, Reach, LEFT_ARM, RIGHT_ARM};
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::weapon::{Equipped, WeaponCatalogue, WeaponId};
use crate::game::body_mesh::FirstPersonHidden;
use crate::game::player::Player;
use crate::game::skeleton::SkeletonRoot;
use crate::prop::baked::{PropCache, SurfaceMaterials};
use crate::prop::figure::grip_in_hand_space;
use crate::prop::hold::HoldSpec;

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
pub struct HeldModel {
    weapon: Option<WeaponId>,
    /// Whether the catalogue and the prop cache have both had their say —
    /// which they may have done by answering "there is no model", and that is
    /// an answer worth keeping.
    resolved: bool,
    parts: Vec<Entity>,
    /// Taken off the model when there is one, so the pose layer never has to
    /// reach for the prop cache.
    hold: HoldSpec,
}

pub struct HeldWeaponPlugin;

impl Plugin for HeldWeaponPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropCache>()
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
    bodies: Query<(Entity, &Equipped, Option<&HeldModel>), With<Skeleton>>,
) {
    for (body, equipped, drawn) in &bodies {
        let held = equipped.held();
        if let Some(drawn) = drawn
            && drawn.weapon == held
            && drawn.resolved
        {
            continue;
        }

        match wanted_model(equipped, &catalogue) {
            // Recorded only if the *held weapon* changed, so waiting for a
            // catalogue does not re-insert the component every frame.
            Wanted::Unknown => {
                if drawn.is_none_or(|drawn| drawn.weapon != held) {
                    replace_parts(&mut commands, body, drawn, vec![], held, false, HoldSpec::default());
                }
            }
            Wanted::Nothing => {
                replace_parts(&mut commands, body, drawn, vec![], held, true, HoldSpec::default())
            }
            Wanted::Model(model) => {
                let mut spawned = Vec::new();
                let mut hold = HoldSpec::default();
                if let Some(baked) =
                    cache.ensure(model, &packs, &mut meshes, &mut materials, &mut surfaces)
                {
                    let parts = baked.parts.clone();
                    hold = baked.hold;
                    commands.entity(body).with_children(|body| {
                        for (mesh, material) in parts {
                            spawned.push(
                                body.spawn((
                                    HeldPart,
                                    // Hidden from inside your own head, along
                                    // with the body it hangs off.
                                    FirstPersonHidden,
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
                replace_parts(&mut commands, body, drawn, spawned, held, true, hold);
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
) {
    if let Some(drawn) = drawn {
        for part in &drawn.parts {
            commands.entity(*part).despawn();
        }
    }
    commands
        .entity(body)
        .insert((HeldModel { weapon, resolved, parts, hold }, WeaponInHand::default()));
}

/// Which way an elbow points.
///
/// Backwards and a little out and down, which is where a person's elbow goes
/// when they hold something in front of them. The solver cannot work this out
/// — it is the one thing about a two-bone chain that the target does not
/// determine — and getting it wrong is an arm bent the wrong way at the elbow.
fn elbow_pole(side: f32) -> Vec3 {
    Vec3::new(0.35 * side, -0.85, 0.55).normalize()
}

/// Put the weapon where the body is aiming, and the hands on the weapon.
///
/// Runs after the animator and the look pass, composing onto the pose they
/// left — the arrangement `look_with_the_head` already uses. The lower body is
/// untouched, which is the point: a hold has to survive whatever the legs are
/// doing.
///
/// **The pivot's position comes from the animated chest and its orientation
/// from the aim.** Following the chest's rotation too would make a crouch's
/// lean tilt the weapon away from where the shot goes; ignoring its position
/// would detach the weapon from a body that is bobbing.
pub(crate) fn hold_the_weapon(
    mut bodies: Query<(
        &Player,
        &Skeleton,
        &mut Pose,
        Option<&SkeletonRoot>,
        &HeldModel,
        &mut WeaponInHand,
    )>,
) {
    for (player, skeleton, mut pose, offset, held, mut weapon) in &mut bodies {
        if !held.resolved || held.weapon.is_none() {
            continue;
        }
        let Some(chest) = skeleton.index_of(bone::CHEST) else { continue };

        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        let posed = skeleton.posed_bones(&pose, &root);
        // The chest's tail is the point between the shoulders.
        let pivot = posed[chest].tail;

        let proportions = skeleton.proportions();
        let arm = proportions.height * proportions.arm_length;

        // `-Z` is forward in the body's own frame, and a positive rotation
        // about `X` lifts it — the opposite sign to a spine bone, whose `+Y`
        // runs up its own length.
        let aim = Quat::from_rotation_x(player.pitch);
        let placed = Transform::from_translation(pivot + aim * held.hold.carry(arm))
            .with_rotation(aim);
        weapon.0 = placed;

        let grip = placed.transform_point(held.hold.grip());
        reach_for(skeleton, &mut pose, &root, &RIGHT_ARM, grip, elbow_pole(1.0));

        // The support hand only takes hold if it can. A body's arms are short
        // and a weapon carried forward is often out of the off hand's range,
        // and an arm stretched at something it cannot touch looks far worse
        // than one left doing whatever the animation had it doing.
        if let Some(support) = held.hold.support() {
            let support = placed.transform_point(support);
            reach_for(skeleton, &mut pose, &root, &LEFT_ARM, support, elbow_pole(-1.0));
        }

        // Last, because it wants the arm as the solve left it: the hand takes
        // the grip's orientation rather than the one the animation gave it, or
        // a body holds a launcher with the back of its wrist.
        let grip_rotation = grip_in_hand_space(skeleton).rotation;
        set_world_rotation(
            skeleton,
            &mut pose,
            &root,
            bone::HAND_R,
            placed.rotation * grip_rotation.inverse(),
        );
    }
}

/// Solve a chain onto a target, or leave it exactly as it was.
///
/// [`ik::solve`] writes a straightened limb and *then* reports `Short`, which
/// is right for a leg — a foot pointed at a floor it cannot reach still wants
/// to be pointing at it — and wrong for a hand: an arm stretched towards a
/// grip it cannot hold reads as broken, where one still swinging with the walk
/// cycle merely reads as not holding.
fn reach_for(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    chain: &Chain,
    target: Vec3,
    pole: Vec3,
) -> Reach {
    let before = [chain.upper, chain.lower]
        .into_iter()
        .chain(chain.tip)
        .map(|joint| (joint, pose.joint(joint)))
        .collect::<Vec<_>>();

    let reach = solve(skeleton, pose, root, chain, target, pole);
    if reach != Reach::Reached {
        for (joint, rotation) in before {
            pose.set(joint, rotation);
        }
    }
    reach
}

/// Turn a bone so it ends up facing a given way in the body's frame.
///
/// A pose stores a joint's rotation relative to its rest and its parent, so a
/// world orientation has to be divided back through both. Read off the posed
/// skeleton rather than walked up the parent chain, because the posed bone
/// already carries the product of everything above it.
fn set_world_rotation(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    name: &'static str,
    wanted: Quat,
) {
    let Some(index) = skeleton.index_of(name) else { return };
    let posed = skeleton.posed_bones(pose, root)[index].rotation;
    // `posed == parent_global * rest * joint`, so dividing the current joint
    // out leaves exactly the part this has no business changing.
    let base = posed * pose.joint(name).inverse();
    pose.set(name, base.inverse() * wanted);
}

/// Put every held part where the right hand is.
///
/// The same forward kinematics `pose_body_meshes` runs, composed with the grip
/// transform — which comes from [`grip_in_hand_space`] and therefore from the
/// reference figure, so a weapon sits in the hand the way the prop editor drew
/// it being held.
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
        world.init_resource::<PropCache>();
        world.init_resource::<SurfaceMaterials>();
        world.init_resource::<PackAssets>();

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
        world.run_system_once(hold_the_weapon).unwrap();

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

    /// **Every class has to be able to reach every prop the pack ships.** A
    /// carry is authored in fractions of arm length so that it retargets, and
    /// a build whose hand cannot get to the grip holds nothing while its arm
    /// swings past it — which is silent, and which a per-prop hold makes easy
    /// to author your way into.
    #[test]
    fn every_class_can_reach_every_shipped_props_grip() {
        let directory = std::path::Path::new("assets/default/props");
        let Ok(entries) = std::fs::read_dir(directory) else { return };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("gpp") {
                continue;
            }
            let doc = crate::prop::document::parse(&std::fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

            for class in EVERY_CLASS {
                let skeleton = humanoid(class.proportions());
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;

                let chest = skeleton.index_of(bone::CHEST).expect("bone is in the rig");
                let proportions = skeleton.proportions();
                let arm = proportions.height * proportions.arm_length;
                let pivot = skeleton.posed_bones(&pose, &root)[chest].tail;
                let placed = Transform::from_translation(pivot + doc.hold.carry(arm));

                let grip = placed.transform_point(doc.hold.grip());
                assert_eq!(
                    reach_for(&skeleton, &mut pose, &root, &RIGHT_ARM, grip, elbow_pole(1.0)),
                    Reach::Reached,
                    "{class:?} cannot reach the grip of {}",
                    path.display(),
                );

                if let Some(support) = doc.hold.support() {
                    let support = placed.transform_point(support);
                    assert_eq!(
                        reach_for(&skeleton, &mut pose, &root, &LEFT_ARM, support, elbow_pole(-1.0)),
                        Reach::Reached,
                        "{class:?} cannot reach the support grip of {} — either move it \
                         or set `two_handed = false`",
                        path.display(),
                    );
                }
            }
        }
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
            world.run_system_once(hold_the_weapon).unwrap();

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

    /// A hand that cannot reach does not stretch for it. `ik::solve` writes a
    /// straightened limb *and then* reports `Short`, which is right for a foot
    /// pointed at a floor and wrong for an arm reaching at a grip it cannot
    /// hold.
    #[test]
    fn an_arm_that_cannot_reach_is_left_where_the_animation_had_it() {
        let skeleton = humanoid(Class::Scout.proportions());
        let mut pose = Pose::rest();
        let before = pose.joint(bone::UPPER_ARM_R);

        let out_of_reach = Vec3::new(0.0, 1.4, -8.0);
        let reach = reach_for(
            &skeleton,
            &mut pose,
            &Transform::IDENTITY,
            &RIGHT_ARM,
            out_of_reach,
            elbow_pole(1.0),
        );

        assert_eq!(reach, Reach::Short);
        assert_eq!(pose.joint(bone::UPPER_ARM_R), before, "the arm stretched for it anyway");
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
            HeldModel { weapon: None, resolved: true, parts: vec![part], hold: HoldSpec::default() },
            WeaponInHand(decided),
        ));

        world.run_system_once(place_held_weapons).unwrap();
        assert_eq!(*world.get::<Transform>(part).unwrap(), decided);
    }
}
