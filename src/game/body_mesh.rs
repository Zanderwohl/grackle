//! Bodies as geometry: one child entity per bone, carrying that bone's mesh.
//!
//! The one thing a body is that is authoritative about nothing: hit
//! registration reads the hitboxes and the simulation reads the bones, so a
//! mesh a frame behind either would look wrong and play the same.
//!
//! **A part is a child of the body, one per bone, placed by that bone**, so
//! vertices are rigidly parented: a shoulder does not stretch, and two parts
//! meeting at a joint interpenetrate. That makes the mesh a function of the
//! pose with no state of its own, and leaves the geometry authored in bone
//! space — the space a skinned version wants it in, so warping vertices across
//! a joint replaces [`pose_body_meshes`] and leaves
//! [`crate::common::skeleton::mesh`] alone.
//!
//! Parts are placed in **body-local space**: [`Skeleton::posed_bones`] is run
//! with the skeleton's root offset as its root, so what comes back is already
//! relative to the entity the parts hang off and propagation does the rest. A
//! body that moves takes its mesh with it even on a frame this never runs.
//!
//! Meshes are cached per **build** rather than per body: sixty bodies in an
//! animation grid are ten distinct silhouettes, and a class's mesh depends on
//! nothing but its [`Proportions`].

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::flame::Burning;
use crate::common::skeleton::mesh::body_meshes;
use crate::common::skeleton::rig::Proportions;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::team::Team;
use crate::game::player::{Player, ViewMode};
use crate::game::skeleton::SkeletonRoot;

/// A body that has had its parts built.
///
/// Holds them so they can be thrown away and rebuilt when the body changes
/// shape. `Children` would not do: a player's camera hangs off it too, and a
/// system that despawned that would be a hard bug to place.
#[derive(Component, Debug)]
pub struct BodyMesh {
    parts: Vec<Entity>,
}

/// One drawn bone: an index into the same list [`Skeleton::posed_bones`]
/// returns.
///
/// An index rather than a name, because this is read once per bone per body
/// per frame and the two lists are the same list by construction.
#[derive(Component, Clone, Copy, Debug)]
pub struct BoneMesh(pub usize);

/// What colour a body is, if it is not the default one and has no team.
///
/// A body rather than a bone, because a colour says *whose* body this is — an
/// editor preview, and now a team. Colour that varies across one body is a
/// skin, which is a texture.
///
/// [`Team`] outranks it. The two are not really the same statement: a tint is
/// "draw this body specially" and a team is "this body is on that side", and
/// whose side you are on is the thing you have to be able to read across a
/// room in a fight. A spawn point's preview is green because nothing on it has
/// a side.
#[derive(Component, Clone, Copy, Debug)]
pub struct BodyTint(pub Color);

/// What a body is drawn in when it has neither a team nor a tint.
pub const DEFAULT_BODY_COLOUR: Color = Color::srgb(0.62, 0.64, 0.70);

/// How a body on fire is lit from inside, in linear light.
///
/// **Fire is a glow, not a hue.** It used to pull a body's base colour towards
/// orange, which worked when every body was the same grey and stops working
/// the moment bodies are coloured: a burning red is a red, and a burning
/// yellow was already most of the way there. An emissive term is the one
/// signal that reads identically on all four teams, because it adds light
/// rather than replacing colour — and it does not have to be remembered and
/// undone, because it is not written onto the body at all.
///
/// Over 1.0 on purpose: the play camera is HDR and tone-mapped, so this is a
/// body that is *brighter than white* in the red channel rather than one
/// painted a lighter orange.
pub const BURNING_GLOW: LinearRgba = LinearRgba::rgb(2.4, 0.55, 0.06);

/// How a body is drawn: what colour it is, and whether it is alight.
///
/// One value rather than two arguments threaded around, because it is also the
/// material cache's key — two bodies that look the same share a material, and
/// what "the same" means is exactly this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyLook {
    pub colour: Color,
    pub burning: bool,
}

impl BodyLook {
    /// The cache key: the colour to eight bits a channel, plus the fire.
    fn key(self) -> ([u8; 4], bool) {
        (self.colour.to_srgba().to_u8_array(), self.burning)
    }
}

/// What a body looks like, in one place.
///
/// Burning is an *input* to this rather than something written onto the body
/// as a [`BodyTint`]. Overwriting the tint would mean remembering what was
/// underneath it and putting it back when the fire went out, and the failure
/// mode of forgetting is a body that stays scorched for the rest of the match.
/// A body's appearance is a function of what the body is, and being on fire is
/// part of what it is — as is whose side it is on.
pub fn body_look(
    team: Option<&Team>,
    tint: Option<&BodyTint>,
    burning: Option<&Burning>,
) -> BodyLook {
    BodyLook {
        colour: team
            .map(|team| team.colour())
            .or_else(|| tint.map(|tint| tint.0))
            .unwrap_or(DEFAULT_BODY_COLOUR),
        burning: burning.is_some(),
    }
}

/// One material per look asked for.
///
/// Bodies that look the same share a material for the same reason bodies of a
/// build share their meshes: an animation grid is sixty bodies and a handful
/// of distinct answers. Four teams alight or not is eight, not sixty.
#[derive(Resource, Default)]
pub struct BodyMaterials(HashMap<([u8; 4], bool), Handle<StandardMaterial>>);

impl BodyMaterials {
    fn get(
        &mut self,
        look: BodyLook,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.0
            .entry(look.key())
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: look.colour,
                    // The whole of what fire does to a body: it keeps its own
                    // colour and is lit from inside.
                    emissive: if look.burning { BURNING_GLOW } else { LinearRgba::BLACK },
                    perceptual_roughness: 0.85,
                    ..default()
                })
            })
            .clone()
    }
}

/// Built meshes, keyed by the numbers they were built from.
#[derive(Resource, Default)]
pub struct BodyMeshCache(HashMap<BuildKey, Vec<Handle<Mesh>>>);

/// A [`Proportions`] as something that can be hashed.
///
/// Bit patterns, because exact equality is the question being asked: these
/// numbers are copied from a class's table rather than computed, so the usual
/// argument against comparing floats does not apply.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct BuildKey([u32; 9]);

impl BuildKey {
    fn of(p: &Proportions) -> BuildKey {
        BuildKey([
            p.height.to_bits(),
            p.hip_height.to_bits(),
            p.torso_length.to_bits(),
            p.arm_length.to_bits(),
            p.head_length.to_bits(),
            p.shoulder_half_width.to_bits(),
            p.hip_half_width.to_bits(),
            p.girth.to_bits(),
            p.belly.to_bits(),
        ])
    }
}

pub struct BodyMeshPlugin;

impl Plugin for BodyMeshPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<BodyMeshCache>()
            .init_resource::<BodyMaterials>()
            // `recolour_bodies` after the build, so a body that catches fire
            // on the same frame it is dressed is dressed and then scorched
            // rather than scorched and then dressed plain.
            .add_systems(Update, (build_body_meshes, recolour_bodies).chain())
            .add_systems(Update, hide_own_body)
            // Before propagation rather than after it, unlike the gizmos: a
            // part is a real entity whose `GlobalTransform` has to be computed
            // from what this writes, and writing it afterwards would draw
            // every body one frame behind its own bones.
            .add_systems(PostUpdate, pose_body_meshes.before(TransformSystems::Propagate))
        ;
    }
}

/// Give every body a part per bone, and rebuild them if it changes shape or
/// colour. Change detection covers insertion, so this is also what dresses a
/// body in the first place.
fn build_body_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cache: ResMut<BodyMeshCache>,
    mut palette: ResMut<BodyMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bodies: Query<
        (
            Entity,
            &Skeleton,
            Option<&Team>,
            Option<&BodyTint>,
            Option<&Burning>,
            Option<&BodyMesh>,
        ),
        Or<(Changed<Skeleton>, Changed<Team>, Changed<BodyTint>)>,
    >,
) {
    for (body, skeleton, team, tint, burning, existing) in &bodies {
        if let Some(existing) = existing {
            for part in &existing.parts {
                commands.entity(*part).despawn();
            }
        }

        let material = palette.get(body_look(team, tint, burning), &mut materials);

        let handles = cache
            .0
            .entry(BuildKey::of(&skeleton.proportions()))
            .or_insert_with(|| {
                body_meshes(skeleton)
                    .into_iter()
                    .map(|mesh| meshes.add(mesh))
                    .collect()
            });

        let mut parts = Vec::with_capacity(handles.len());
        commands.entity(body).with_children(|body| {
            for (index, handle) in handles.iter().enumerate() {
                parts.push(
                    body.spawn((
                        BoneMesh(index),
                        Mesh3d(handle.clone()),
                        MeshMaterial3d(material.clone()),
                        // Overwritten by `pose_body_meshes` before anything is
                        // drawn; the identity would pile every part on the
                        // body's origin for a frame.
                        Transform::from_translation(
                            skeleton.bones()[index].head,
                        ),
                    ))
                    .id(),
                );
            }
        });

        commands
            .entity(body)
            // Grid cells and spawn-point previews are spawned without one,
            // and their parts would have nothing to inherit from.
            .insert_if_new(Visibility::default())
            .insert(BodyMesh { parts });
    }
}

/// Swap the material on a body whose colour has changed but whose *shape* has
/// not.
///
/// Catching fire must not rebuild a body. `build_body_meshes` throws every
/// part away and respawns it, which is the right thing when the skeleton
/// changes and absurd when somebody is set alight ten times a second — and
/// twice per burn is still ten entities of churn for a colour. Shape and
/// colour are two different questions and this is the second one.
fn recolour_bodies(
    mut palette: ResMut<BodyMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut extinguished: RemovedComponents<Burning>,
    lit: Query<Entity, Added<Burning>>,
    bodies: Query<(&BodyMesh, Option<&Team>, Option<&BodyTint>, Option<&Burning>)>,
    mut parts: Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    // Both edges. Only having the first is a body that stays scorched after
    // the fire is out, which looks like the fire never went out.
    for body in lit.iter().chain(extinguished.read()) {
        // Undressed, or despawned between catching fire and this running —
        // a body killed by the burn that lit it.
        let Ok((mesh, team, tint, burning)) = bodies.get(body) else { continue };

        let material = palette.get(body_look(team, tint, burning), &mut materials);
        for part in &mesh.parts {
            let Ok(mut slot) = parts.get_mut(*part) else { continue };
            slot.0 = material.clone();
        }
    }
}


/// Put every part where its bone is.
///
/// The hot loop: bones per body per frame. Forward kinematics runs once per
/// body and is read out by index, which is why [`BoneMesh`] holds one.
fn pose_body_meshes(
    bodies: Query<(&Skeleton, &Pose, Option<&SkeletonRoot>, &BodyMesh)>,
    mut parts: Query<(&BoneMesh, &mut Transform)>,
) {
    for (skeleton, pose, offset, mesh) in &bodies {
        // The skeleton's root in the body's own frame: its feet, below the
        // middle of a body whose transform is the centre of its collision box.
        // Local space has no rotation to undo, so this is the whole of it.
        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        let posed = skeleton.posed_bones(pose, &root);

        for part in &mesh.parts {
            let Ok((BoneMesh(index), mut transform)) = parts.get_mut(*part) else {
                continue;
            };
            let bone = &posed[*index];
            // The mesh is already the bone's size; a non-uniform scale here
            // would be a second opinion about that and would skew its normals.
            *transform = Transform {
                translation: bone.head,
                rotation: bone.rotation,
                scale: Vec3::ONE,
            };
        }
    }
}

/// Hide the body the camera is inside: from in your own head your own body is
/// geometry across the lens.
///
/// Set on the parts rather than on the player, which is also what the camera
/// hangs off.
fn hide_own_body(
    view: Res<ViewMode>,
    players: Query<&BodyMesh, With<Player>>,
    mut parts: Query<&mut Visibility, With<BoneMesh>>,
) {
    let wanted = if view.shows_own_body() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    for mesh in &players {
        for part in &mesh.parts {
            let Ok(mut visibility) = parts.get_mut(*part) else { continue };
            // Assigned only on a change, so this does not dirty every part of
            // every player every frame.
            if *visibility != wanted {
                *visibility = wanted;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::Class;
    use crate::common::team::Team;
    use crate::common::skeleton::rig::{bone, humanoid};

    /// Fire is a glow, not a repaint: a burning body keeps the colour that
    /// says whose it is and is lit from inside instead. That is the property
    /// that makes it work on all four teams, red included.
    #[test]
    fn burning_lights_a_body_up_without_changing_whose_it_is() {
        use crate::common::damage::DamageSource;
        use crate::common::flame::{Burning, FlameSpec};

        let burning = Burning::lit(&FlameSpec::FLAMETHROWER, DamageSource::World);

        for team in Team::ALL {
            let plain = body_look(Some(&team), None, None);
            let alight = body_look(Some(&team), None, Some(&burning));

            assert_eq!(plain.colour, team.colour());
            assert_eq!(alight.colour, team.colour(), "the fire repainted a {team:?}");
            assert!(!plain.burning);
            assert!(alight.burning);
            assert_ne!(plain.key(), alight.key(), "the two share a material");
        }
    }

    /// The order of precedence, all three at once: a team wins over a tint,
    /// a tint wins over the default, and being alight is orthogonal to all of
    /// it.
    #[test]
    fn a_team_outranks_a_tint_which_outranks_the_default() {
        let tint = BodyTint(Color::srgb(0.2, 0.9, 0.3));

        assert_eq!(body_look(None, None, None).colour, DEFAULT_BODY_COLOUR);
        assert_eq!(body_look(None, Some(&tint), None).colour, tint.0);
        assert_eq!(
            body_look(Some(&Team::Blue), Some(&tint), None).colour,
            Team::Blue.colour(),
            "a spawn point's green hid a team"
        );
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::asset::AssetPlugin::default(),
            bevy::render::mesh::MeshPlugin,
        ));
        app.init_asset::<StandardMaterial>();
        app.init_resource::<ViewMode>();
        app.add_plugins(BodyMeshPlugin);
        app
    }

    /// A body gets one part per bone, without anything having to say so at the
    /// place it was spawned.
    #[test]
    fn a_body_is_dressed_from_its_rig_alone() {
        let mut app = app();
        let skeleton = humanoid(Class::Heavy.proportions());
        let bones = skeleton.bones().len();
        let body = app
            .world_mut()
            .spawn((skeleton, Pose::rest(), Transform::IDENTITY))
            .id();

        app.update();

        let mesh = app.world().get::<BodyMesh>(body).expect("nothing was built");
        assert_eq!(mesh.parts.len(), bones);
    }

    /// Two bodies of the same build share their meshes, and two builds do not.
    /// Sixty bodies in an animation grid is the case that makes this worth
    /// having rather than a nicety.
    #[test]
    fn bodies_of_one_build_share_their_meshes() {
        let mut app = app();
        for class in [Class::Heavy, Class::Heavy, Class::Scout] {
            app.world_mut()
                .spawn((humanoid(class.proportions()), Pose::rest(), Transform::IDENTITY));
        }

        app.update();

        assert_eq!(app.world().resource::<BodyMeshCache>().0.len(), 2);
    }

    /// The seam this rests on: a part is placed by the bone at its own index,
    /// in the body's frame. Get the index wrong and a body is a heap of parts
    /// that each individually look right.
    #[test]
    fn each_part_stands_where_its_bone_does() {
        let mut app = app();
        let skeleton = humanoid(Proportions::DEFAULT);
        let head = skeleton.index_of(bone::HEAD).expect("the rig has a head");
        let expected = skeleton
            .posed_bones(&Pose::rest(), &Transform::IDENTITY)
            .swap_remove(head)
            .head;
        let body = app
            .world_mut()
            .spawn((skeleton, Pose::rest(), Transform::IDENTITY))
            .id();

        app.update();

        let part = app.world().get::<BodyMesh>(body).unwrap().parts[head];
        let placed = app.world().get::<Transform>(part).unwrap().translation;
        assert!(
            (placed - expected).length() < 1e-4,
            "the head part is at {placed}, and the head bone at {expected}",
        );
    }

    /// A tinted body is painted its own colour, and bodies asking for the same
    /// colour share one material — the same reasoning as the mesh cache, and
    /// the thing that keeps a map full of green spawn points to one material.
    #[test]
    fn a_tint_paints_a_body_without_multiplying_materials() {
        let mut app = app();
        let green = Color::srgb(0.13, 0.95, 0.35);
        let rig = || (humanoid(Proportions::DEFAULT), Pose::rest(), Transform::IDENTITY);

        let plain = app.world_mut().spawn(rig()).id();
        let first = app.world_mut().spawn((rig(), BodyTint(green))).id();
        let second = app.world_mut().spawn((rig(), BodyTint(green))).id();

        app.update();

        let material = |body: Entity| {
            let part = app.world().get::<BodyMesh>(body).expect("dressed").parts[0];
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(part)
                .expect("a part is painted")
                .0
                .clone()
        };

        assert_eq!(material(first), material(second), "two green bodies, two materials");
        assert_ne!(material(plain), material(first), "the tint was ignored");
        // The default and the green, and nothing else.
        assert_eq!(app.world().resource::<BodyMaterials>().0.len(), 2);
    }

    /// A rig swapped for another build takes its geometry with it, rather than
    /// leaving the old class's parts behind on top of the new ones.
    #[test]
    fn changing_build_replaces_the_parts() {
        let mut app = app();
        let body = app
            .world_mut()
            .spawn((humanoid(Class::Scout.proportions()), Pose::rest(), Transform::IDENTITY))
            .id();
        app.update();

        let before = app.world().get::<BodyMesh>(body).unwrap().parts.clone();
        app.world_mut()
            .entity_mut(body)
            .insert(humanoid(Class::Heavy.proportions()));
        app.update();

        let after = app.world().get::<BodyMesh>(body).unwrap().parts.clone();
        assert!(before.iter().all(|part| !after.contains(part)), "old parts were kept");
        for part in before {
            assert!(app.world().get_entity(part).is_err(), "an old part is still around");
        }
    }
}
