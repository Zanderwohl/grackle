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
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::weapon::{Equipped, WeaponCatalogue, WeaponId};
use crate::game::body_mesh::FirstPersonHidden;
use crate::game::skeleton::SkeletonRoot;
use crate::prop::baked::{PropCache, SurfaceMaterials};
use crate::prop::figure::grip_in_hand_space;

/// One drawn part of the weapon a body is holding.
#[derive(Component)]
pub struct HeldPart;

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
                    replace_parts(&mut commands, body, drawn, vec![], held, false);
                }
            }
            Wanted::Nothing => replace_parts(&mut commands, body, drawn, vec![], held, true),
            Wanted::Model(model) => {
                let mut spawned = Vec::new();
                if let Some(baked) =
                    cache.ensure(model, &packs, &mut meshes, &mut materials, &mut surfaces)
                {
                    let parts = baked.parts.clone();
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
                replace_parts(&mut commands, body, drawn, spawned, held, true);
            }
        }
    }
}

fn replace_parts(
    commands: &mut Commands,
    body: Entity,
    drawn: Option<&HeldModel>,
    parts: Vec<Entity>,
    weapon: Option<WeaponId>,
    resolved: bool,
) {
    if let Some(drawn) = drawn {
        for part in &drawn.parts {
            commands.entity(*part).despawn();
        }
    }
    commands.entity(body).insert(HeldModel { weapon, resolved, parts });
}

/// Put every held part where the right hand is.
///
/// The same forward kinematics `pose_body_meshes` runs, composed with the grip
/// transform — which comes from [`grip_in_hand_space`] and therefore from the
/// reference figure, so a weapon sits in the hand the way the prop editor drew
/// it being held.
fn place_held_weapons(
    bodies: Query<(&Skeleton, &Pose, Option<&SkeletonRoot>, &HeldModel)>,
    mut parts: Query<&mut Transform, With<HeldPart>>,
) {
    for (skeleton, pose, offset, held) in &bodies {
        if held.parts.is_empty() {
            continue;
        }
        let Some(index) = skeleton.index_of(bone::HAND_R) else { continue };

        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        let hand = &skeleton.posed_bones(pose, &root)[index];
        let placed = Transform::from_translation(hand.head)
            .with_rotation(hand.rotation)
            .mul_transform(grip_in_hand_space(skeleton));

        for part in &held.parts {
            let Ok(mut transform) = parts.get_mut(*part) else { continue };
            *transform = placed;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::weapon::{Magazine, Mounted, Slot, Weapon};
    use crate::common::skeleton::{default_humanoid, Pose};

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

    /// The model goes where the hand is, not where the body is. A part left at
    /// the identity sits at the body's feet, which is the failure this exists
    /// to catch.
    #[test]
    fn a_held_part_is_placed_at_the_hand() {
        let skeleton = default_humanoid().clone();
        let index = skeleton.index_of(bone::HAND_R).expect("bone is in the rig");

        let mut world = World::new();
        let part = world.spawn((HeldPart, Transform::IDENTITY)).id();
        world.spawn((
            skeleton.clone(),
            Pose::rest(),
            HeldModel { weapon: None, resolved: true, parts: vec![part] },
        ));

        world.run_system_once(place_held_weapons).unwrap();

        let placed = world.get::<Transform>(part).unwrap().translation;
        let hand = &skeleton.posed_bones(&Pose::rest(), &Transform::IDENTITY)[index];
        let centre = (hand.head + hand.tail) / 2.0;
        assert!(
            (placed - centre).length() < 1e-4,
            "the model sits at {placed} rather than in the hand at {centre}",
        );
    }
}
