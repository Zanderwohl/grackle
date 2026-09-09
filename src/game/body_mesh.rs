//! Bodies as geometry: one child entity per bone, carrying that bone's mesh.
//!
//! What replaces the wireframe prisms. The rig is unchanged and so are the
//! hitboxes — this is the third thing a body is, after the boxes it can be hit
//! on and the bones it moves as, and it is the only one that is not
//! authoritative about anything. Nothing tests against a mesh; if it drifted a
//! frame behind the simulation it would look wrong and play the same.
//!
//! **A part is a child of the body, one per bone, placed by that bone.** So
//! the vertices are rigidly parented — a shoulder does not stretch, and two
//! parts meeting at a joint interpenetrate rather than deform. That is the
//! deliberate first cut: it makes the mesh a function of the pose with no
//! extra state, and it means the geometry is already authored in bone space,
//! which is the space a skinned version wants it in. Warping vertices across a
//! joint replaces [`pose_body_meshes`] and leaves
//! [`crate::common::skeleton::mesh`] alone.
//!
//! Placing the parts in **body-local space** is what makes this cheap and
//! correct: [`Skeleton::posed_bones`] is run with the skeleton's own root
//! offset as its root, so what comes back is already relative to the entity
//! the parts hang off, and Bevy's transform propagation does the rest. No
//! inverse transforms, and a body that moves takes its mesh with it even on a
//! frame this never runs.
//!
//! Meshes are cached per **build** rather than per body: sixty bodies in an
//! animation grid are ten distinct silhouettes, and a class's mesh depends on
//! nothing but its [`Proportions`].

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::skeleton::mesh::body_meshes;
use crate::common::skeleton::rig::Proportions;
use crate::common::skeleton::{Pose, Skeleton};
use crate::game::player::{Player, ViewMode};
use crate::game::skeleton::SkeletonRoot;

/// A body that has had its parts built.
///
/// Holds them so they can be thrown away and rebuilt when the body changes
/// shape. Walking `Children` would find the camera hanging off a player as
/// well, and a system that despawned that would be a hard bug to place.
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

/// What colour a body is, if it is not the default one.
///
/// A body rather than a bone, because a colour is a statement about *whose*
/// body this is — an editor preview, and later a team — and not about which
/// part of it. Per-bone colour is a skin, and a skin is a texture rather than
/// a component.
#[derive(Component, Clone, Copy, Debug)]
pub struct BodyTint(pub Color);

/// What a body is drawn in when nothing says otherwise.
///
/// Plainly a placeholder: teams, classes and skins all want their own, and
/// none of them exist.
pub const DEFAULT_BODY_COLOUR: Color = Color::srgb(0.62, 0.64, 0.70);

/// One material per colour asked for.
///
/// Bodies of a colour share a material for the same reason bodies of a build
/// share their meshes: an animation grid is sixty bodies and a handful of
/// distinct answers.
#[derive(Resource, Default)]
pub struct BodyMaterials(HashMap<[u8; 4], Handle<StandardMaterial>>);

impl BodyMaterials {
    fn get(
        &mut self,
        colour: Color,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.0
            .entry(colour.to_srgba().to_u8_array())
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: colour,
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
/// Bit patterns rather than a rounded key: two builds that differ anywhere
/// must not share a mesh, and float equality is exactly the question being
/// asked — these numbers are copied from a table, not computed, so the usual
/// argument against comparing floats does not apply. A `NaN` in a build would
/// key to itself here and produce a mesh full of holes, which is a louder
/// failure than silently reusing somebody else's body.
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
            .add_systems(Update, (build_body_meshes, hide_own_body))
            // Before propagation rather than after it, unlike the gizmos: a
            // part is a real entity whose `GlobalTransform` has to be computed
            // from what this writes, and writing it afterwards would draw
            // every body one frame behind its own bones.
            .add_systems(PostUpdate, pose_body_meshes.before(TransformSystems::Propagate))
        ;
    }
}

/// Give every body a part per bone, and rebuild them if it changes shape or
/// colour.
///
/// Keyed on the components themselves changing, which covers insertion too: a
/// rig is inserted once on a player and could be swapped when classes become
/// something a player picks.
fn build_body_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cache: ResMut<BodyMeshCache>,
    mut palette: ResMut<BodyMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bodies: Query<
        (Entity, &Skeleton, Option<&BodyTint>, Option<&BodyMesh>),
        Or<(Changed<Skeleton>, Changed<BodyTint>)>,
    >,
) {
    for (body, skeleton, tint, existing) in &bodies {
        if let Some(existing) = existing {
            for part in &existing.parts {
                commands.entity(*part).despawn();
            }
        }

        let material = palette.get(
            tint.map_or(DEFAULT_BODY_COLOUR, |tint| tint.0),
            &mut materials,
        );

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
                        // drawn. Spawning at the identity would pile every
                        // part on the body's origin for one frame.
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
            // A body spawned without one — a grid cell, a spawn point's
            // preview — is a body whose parts would have nothing to inherit
            // from.
            .insert_if_new(Visibility::default())
            .insert(BodyMesh { parts });
    }
}

/// Put every part where its bone is.
///
/// The one hot loop here: bones per body per frame. Forward kinematics is run
/// once per body and read out by index, which is why [`BoneMesh`] holds one.
fn pose_body_meshes(
    bodies: Query<(&Skeleton, &Pose, Option<&SkeletonRoot>, &BodyMesh)>,
    mut parts: Query<(&BoneMesh, &mut Transform)>,
) {
    for (skeleton, pose, offset, mesh) in &bodies {
        // The skeleton's root in the body's own frame: its feet, which sit
        // below the middle of a body whose transform is the centre of its
        // collision box. In local space there is no rotation to undo, so this
        // is the whole of the conversion.
        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        let posed = skeleton.posed_bones(pose, &root);

        for part in &mesh.parts {
            let Ok((BoneMesh(index), mut transform)) = parts.get_mut(*part) else {
                continue;
            };
            let bone = &posed[*index];
            // Position and rotation only. The mesh is already the bone's
            // size — scaling it here would be a second opinion about that, and
            // a non-uniform one would skew its normals.
            *transform = Transform {
                translation: bone.head,
                rotation: bone.rotation,
                scale: Vec3::ONE,
            };
        }
    }
}

/// Hide the body the camera is inside.
///
/// The same rule the gizmos followed: from in your own head your own body is
/// geometry across the lens. Set on the parts rather than on the player,
/// because the player entity is also what the camera hangs off.
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
            // Assigned only on a change, so this does not mark every part of
            // every player dirty sixty times a second.
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
    use crate::common::skeleton::rig::{bone, humanoid};

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
