//! Bodies as geometry: one child entity per bone, carrying that bone's mesh,
//! rigidly placed by that bone. Nothing tests against the mesh.
//!
//! Geometry is authored in bone space, so a skinned version would replace
//! [`pose_body_meshes`] and leave [`crate::common::skeleton::mesh`] alone.
//! Parts are placed in body-local space, so a moving body carries its mesh
//! even on a frame this does not run.
//!
//! Meshes are cached per build ([`Proportions`]), not per body: sixty bodies in
//! an animation grid are ten meshes.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::flame::Burning;
use crate::common::skeleton::mesh::body_meshes;
use crate::common::skeleton::rig::Proportions;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::player_color::PlayerColor;
use crate::common::team::Team;
use crate::game::player::{LocalPlayer, Player, ViewMode, Watched};
use crate::game::skeleton::SkeletonRoot;

/// A body that has had its parts built, holding them so they can be rebuilt.
/// Not `Children`: held weapon parts hang off the body too.
#[derive(Component, Debug)]
pub struct BodyMesh {
    parts: Vec<Entity>,
}

/// One drawn bone: an index into [`Skeleton::posed_bones`]'s list, read per
/// bone per frame.
#[derive(Component, Clone, Copy, Debug)]
pub struct BoneMesh(pub usize);

/// Drawn as part of this body, so hidden from inside its own head: bone meshes
/// and held weapon parts.
#[derive(Component)]
pub struct FirstPersonHidden;

/// A body's color when it has no team and no [`PlayerColor`], which both
/// outrank it: who a body is, and above all which side, is what must read
/// across a room.
#[derive(Component, Clone, Copy, Debug)]
pub struct BodyTint(pub Color);

pub const DEFAULT_BODY_COLOR: Color = Color::srgb(0.62, 0.64, 0.70);

/// The emissive light of a body on fire, in linear light. A glow rather than a
/// hue shift, so it reads the same on all four team colors. Over 1.0 on
/// purpose: the play camera is HDR and tone-mapped.
pub const BURNING_GLOW: LinearRgba = LinearRgba::rgb(2.4, 0.55, 0.06);

/// How a body is drawn. Also the material cache's key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyLook {
    pub color: Color,
    pub burning: bool,
}

impl BodyLook {
    /// Color to eight bits a channel, plus the fire.
    fn key(self) -> ([u8; 4], bool) {
        (self.color.to_srgba().to_u8_array(), self.burning)
    }
}

/// What a body looks like. Burning is an input here rather than written onto
/// [`BodyTint`], which would have to be restored when the fire went out.
pub fn body_look(
    team: Option<&Team>,
    player: Option<&PlayerColor>,
    tint: Option<&BodyTint>,
    burning: Option<&Burning>,
) -> BodyLook {
    BodyLook {
        color: team
            .map(|team| team.color())
            .or_else(|| player.map(|player| player.color()))
            .or_else(|| tint.map(|tint| tint.0))
            .unwrap_or(DEFAULT_BODY_COLOR),
        burning: burning.is_some(),
    }
}

/// One material per look: four teams alight or not is eight materials, not
/// one per body.
#[derive(Resource, Default)]
pub struct BodyMaterials(HashMap<([u8; 4], bool), Handle<StandardMaterial>>);

impl BodyMaterials {
    /// `pub(crate)` for the viewmodel's arms.
    pub(crate) fn get(
        &mut self,
        look: BodyLook,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.0
            .entry(look.key())
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: look.color,
                    emissive: if look.burning { BURNING_GLOW } else { LinearRgba::BLACK },
                    perceptual_roughness: 0.85,
                    ..default()
                })
            })
            .clone()
    }
}

#[derive(Resource, Default)]
pub struct BodyMeshCache(HashMap<BuildKey, Vec<Handle<Mesh>>>);

impl BodyMeshCache {
    /// One handle per bone, in [`Skeleton::bones`] order, built on first ask.
    /// Public so the viewmodel's forearms use the same meshes.
    pub fn handles(
        &mut self,
        skeleton: &Skeleton,
        meshes: &mut Assets<Mesh>,
    ) -> &Vec<Handle<Mesh>> {
        self.0.entry(BuildKey::of(&skeleton.proportions())).or_insert_with(|| {
            body_meshes(skeleton).into_iter().map(|mesh| meshes.add(mesh)).collect()
        })
    }
}

/// A [`Proportions`] as bit patterns. Exact equality is right: the numbers are
/// copied from a class's table, not computed.
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
            // `recolor_bodies` after the build, so a body lit on the frame it
            // is dressed ends up lit.
            .add_systems(Update, (build_body_meshes, recolor_bodies).chain())
            .add_systems(Update, hide_own_body)
            // Before propagation, or every body is drawn a frame behind its
            // bones.
            .add_systems(PostUpdate, pose_body_meshes.before(TransformSystems::Propagate))
        ;
    }
}

/// Give every body a part per bone, and rebuild them if it changes shape or
/// color. Change detection covers insertion, so this also dresses new bodies.
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
            Option<&PlayerColor>,
            Option<&BodyTint>,
            Option<&Burning>,
            Option<&BodyMesh>,
        ),
        Or<(Changed<Skeleton>, Changed<Team>, Changed<PlayerColor>, Changed<BodyTint>)>,
    >,
) {
    for (body, skeleton, team, player, tint, burning, existing) in &bodies {
        if let Some(existing) = existing {
            for part in &existing.parts {
                commands.entity(*part).despawn();
            }
        }

        let material = palette.get(body_look(team, player, tint, burning), &mut materials);

        let handles = cache.handles(skeleton, &mut meshes);

        let mut parts = Vec::with_capacity(handles.len());
        commands.entity(body).with_children(|body| {
            for (index, handle) in handles.iter().enumerate() {
                parts.push(
                    body.spawn((
                        BoneMesh(index),
                        FirstPersonHidden,
                        Mesh3d(handle.clone()),
                        MeshMaterial3d(material.clone()),
                        // Overwritten by `pose_body_meshes` before it is drawn.
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
            // Grid cells and spawn-point previews are spawned without one, and
            // their parts would have nothing to inherit.
            .insert_if_new(Visibility::default())
            .insert(BodyMesh { parts });
    }
}

/// Swap the material when a body catches fire or goes out, rather than letting
/// `build_body_meshes` respawn every part, which a flamethrower would trigger
/// ten times a second.
fn recolor_bodies(
    mut palette: ResMut<BodyMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut extinguished: RemovedComponents<Burning>,
    lit: Query<Entity, Added<Burning>>,
    bodies: Query<(&BodyMesh, Option<&Team>, Option<&PlayerColor>, Option<&BodyTint>, Option<&Burning>)>,
    mut parts: Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    // Both edges, or a body stays scorched after the fire is out.
    for body in lit.iter().chain(extinguished.read()) {
        // Undressed, or killed by the burn that lit it.
        let Ok((mesh, team, player, tint, burning)) = bodies.get(body) else { continue };

        let material = palette.get(body_look(team, player, tint, burning), &mut materials);
        for part in &mesh.parts {
            let Ok(mut slot) = parts.get_mut(*part) else { continue };
            slot.0 = material.clone();
        }
    }
}


/// Put every part where its bone is, for bodies whose rig, pose, root offset or
/// parts changed — a settled corpse or a preview is already in place. A
/// rebuilt body is caught by `Changed<BodyMesh>`, inserted with its parts.
fn pose_body_meshes(
    bodies: Query<
        (&Skeleton, &Pose, Option<&SkeletonRoot>, &BodyMesh),
        Or<(Changed<Pose>, Changed<BodyMesh>, Changed<SkeletonRoot>, Changed<Skeleton>)>,
    >,
    mut parts: Query<(&BoneMesh, &mut Transform)>,
) {
    for (skeleton, pose, offset, mesh) in &bodies {
        // The skeleton's root in the body's frame: its feet, below the center
        // of the collision box.
        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));

        // `parts` is built in bone order, so the bone being visited and the
        // part it places share an index. A part that says otherwise is placed
        // by its own bone instead — debug builds call it out, but a release
        // build should not quietly hang a shin where a forearm goes.
        let mut index = 0;
        skeleton.each_posed_bone(pose, &root, |visited| {
            let part = mesh.parts.get(index);
            index += 1;
            let Some(Ok((BoneMesh(bone_index), mut transform))) = part.map(|part| parts.get_mut(*part))
            else {
                return;
            };
            debug_assert_eq!(*bone_index, index - 1, "a part is out of bone order");
            let bone = match *bone_index == index - 1 {
                true => visited,
                false => skeleton.posed_bone(pose, &root, *bone_index),
            };
            // The mesh is already bone-sized; a scale would skew its normals.
            transform.set_if_neq(Transform {
                translation: bone.head,
                rotation: bone.rotation,
                scale: Vec3::ONE,
            });
        });
    }
}

/// Hide the body the camera is inside — ours, or the one a spectator is
/// [`Watched`] — and nobody else's. Every body is visited, so one no longer
/// watched is shown again.
pub(crate) fn hide_own_body(
    view: Res<ViewMode>,
    players: Query<(&Children, Has<LocalPlayer>, Has<Watched>), With<Player>>,
    mut parts: Query<&mut Visibility, With<FirstPersonHidden>>,
) {
    // The body's children rather than `BodyMesh.parts`, so a hat and anything
    // else carrying the marker are covered. Held parts are `show_held_weapons`'s.
    for (children, ours, watched) in &players {
        let wanted = match (ours || watched) && !view.shows_own_body() {
            true => Visibility::Hidden,
            false => Visibility::Inherited,
        };
        for child in children.iter() {
            let Ok(mut visibility) = parts.get_mut(child) else { continue };
            // Assigned only on change, so parts are not dirtied every frame.
            if *visibility != wanted {
                *visibility = wanted;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::player::Player;
    use crate::common::class::Class;
    use crate::common::team::Team;
    use crate::common::skeleton::rig::{bone, humanoid};

    use bevy::ecs::system::RunSystemOnce;

    /// Only the local body is hidden; `With<Player>` would hide every body.
    #[test]
    fn first_person_hides_only_the_body_the_camera_is_in() {
        let mut world = World::new();
        world.insert_resource(ViewMode::FirstPerson);

        // Children of the body, as `build_body_meshes` makes them.
        let body_with_parts = |world: &mut World, local: bool| {
            let mut body = world.spawn(Player::default());
            if local {
                body.insert(LocalPlayer);
            }
            let body = body.id();

            let mut parts: Vec<Entity> = (0..3)
                .map(|index| {
                    world
                        .spawn((BoneMesh(index), FirstPersonHidden, Visibility::Inherited, ChildOf(body)))
                        .id()
                })
                .collect();
            // A held weapon: marked the same way, not a bone.
            parts.push(
                world
                    .spawn((FirstPersonHidden, Visibility::Inherited, ChildOf(body)))
                    .id(),
            );
            parts
        };

        let mine = body_with_parts(&mut world, true);
        let theirs = body_with_parts(&mut world, false);

        world.run_system_once(hide_own_body).unwrap();

        for part in &mine {
            assert_eq!(
                *world.get::<Visibility>(*part).unwrap(),
                Visibility::Hidden,
                "our own body was left across the lens"
            );
        }
        for part in &theirs {
            assert_eq!(
                *world.get::<Visibility>(*part).unwrap(),
                Visibility::Inherited,
                "somebody else's body was hidden with ours"
            );
        }
    }

    /// A burning body keeps its team color and gets a separate material.
    #[test]
    fn burning_lights_a_body_up_without_changing_whose_it_is() {
        use crate::common::damage::DamageSource;
        use crate::common::flame::{Burning, FlameSpec};

        let burning = Burning::lit(&FlameSpec::FLAMETHROWER, DamageSource::World);

        for team in Team::ALL {
            let plain = body_look(Some(&team), None, None, None);
            let alight = body_look(Some(&team), None, None, Some(&burning));

            assert_eq!(plain.color, team.color());
            assert_eq!(alight.color, team.color(), "the fire repainted a {team:?}");
            assert!(!plain.burning);
            assert!(alight.burning);
            assert_ne!(plain.key(), alight.key(), "the two share a material");
        }
    }

    #[test]
    fn a_team_outranks_a_players_color_which_outranks_a_tint_then_the_default() {
        let tint = BodyTint(Color::srgb(0.2, 0.9, 0.3));
        let player = PlayerColor { hue: 200.0 };

        assert_eq!(body_look(None, None, None, None).color, DEFAULT_BODY_COLOR);
        assert_eq!(body_look(None, None, Some(&tint), None).color, tint.0);
        assert_eq!(body_look(None, Some(&player), Some(&tint), None).color, player.color());
        assert_eq!(
            body_look(Some(&Team::Blue), Some(&player), Some(&tint), None).color,
            Team::Blue.color(),
            "which side somebody is on was hidden"
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

    /// A body with only a rig gets one part per bone.
    #[test]
    fn a_body_is_dressed_from_its_rig_alone() {
        let mut app = app();
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Heavy));
        let bones = skeleton.bones().len();
        let body = app
            .world_mut()
            .spawn((skeleton, Pose::rest(), Transform::IDENTITY))
            .id();

        app.update();

        let mesh = app.world().get::<BodyMesh>(body).expect("nothing was built");
        assert_eq!(mesh.parts.len(), bones);
    }

    /// Bodies of one build share meshes; two builds do not.
    #[test]
    fn bodies_of_one_build_share_their_meshes() {
        let mut app = app();
        for class in [Class::Heavy, Class::Heavy, Class::Scout] {
            app.world_mut()
                .spawn((humanoid(crate::common::class::ClassBuilds::shipped().of(class)), Pose::rest(), Transform::IDENTITY));
        }

        app.update();

        assert_eq!(app.world().resource::<BodyMeshCache>().0.len(), 2);
    }

    /// Each part is placed by the bone at its own index. A wrong index gives a
    /// heap of parts that each look right.
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

    /// Bodies with the same tint share one material.
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

    /// Changing build replaces the old parts rather than keeping them.
    #[test]
    fn changing_build_replaces_the_parts() {
        let mut app = app();
        let body = app
            .world_mut()
            .spawn((humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Scout)), Pose::rest(), Transform::IDENTITY))
            .id();
        app.update();

        let before = app.world().get::<BodyMesh>(body).unwrap().parts.clone();
        app.world_mut()
            .entity_mut(body)
            .insert(humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Heavy)));
        app.update();

        let after = app.world().get::<BodyMesh>(body).unwrap().parts.clone();
        assert!(before.iter().all(|part| !after.contains(part)), "old parts were kept");
        for part in before {
            assert!(app.world().get_entity(part).is_err(), "an old part is still around");
        }
    }

    /// When each of a body's parts last had its `Transform` written.
    fn part_ticks(app: &App, body: Entity) -> Vec<bevy::ecs::change_detection::Tick> {
        app.world()
            .get::<BodyMesh>(body)
            .unwrap()
            .parts
            .iter()
            .map(|part| {
                app.world()
                    .entity(*part)
                    .get_change_ticks::<Transform>()
                    .unwrap()
                    .changed
            })
            .collect()
    }

    /// A body nothing moved leaves its parts alone, and a pose that moved one
    /// arm writes that arm and not the legs.
    ///
    /// A settled corpse and a spawn point's preview are the case: rewriting
    /// their parts every frame hands every one of them to propagation, and
    /// undoes the ragdoll's own care not to touch a sleeping corpse's pose.
    #[test]
    fn a_body_that_did_not_move_leaves_its_parts_alone() {
        let mut app = app();
        let skeleton = humanoid(Proportions::DEFAULT);
        let forearm = skeleton.index_of(bone::FOREARM_R).unwrap();
        let shin = skeleton.index_of(bone::SHIN_L).unwrap();
        let body = app
            .world_mut()
            .spawn((skeleton, Pose::rest(), Transform::IDENTITY))
            .id();
        app.update();
        let dressed = part_ticks(&app, body);

        app.update();
        assert_eq!(part_ticks(&app, body), dressed, "a still body's parts were rewritten");

        let bent = Quat::from_rotation_x(1.0);
        app.world_mut().get_mut::<Pose>(body).unwrap().set(bone::FOREARM_R, bent);
        app.update();
        let moved = part_ticks(&app, body);
        assert_ne!(moved[forearm], dressed[forearm], "the bent forearm was not placed");
        assert_eq!(moved[shin], dressed[shin], "a leg was rewritten for an arm moving");

        let skeleton = app.world().get::<Skeleton>(body).unwrap();
        let mut pose = Pose::rest();
        pose.set(bone::FOREARM_R, bent);
        let expected = skeleton.posed_bones(&pose, &Transform::IDENTITY).swap_remove(forearm);
        let part = app.world().get::<BodyMesh>(body).unwrap().parts[forearm];
        let placed = app.world().get::<Transform>(part).unwrap();
        assert!((placed.translation - expected.head).length() < 1e-4);
        assert!(placed.rotation.angle_between(expected.rotation) < 1e-3);
    }

    /// A body rebuilt as another build is placed on the frame its new parts
    /// exist, root offset and all, though its pose did not change.
    ///
    /// The rebuild is the one input that is not the pose, and the parts are
    /// spawned at their bones' rest heads — near enough to look right in a
    /// test that only checks one bone, and off by the root offset everywhere.
    #[test]
    fn a_rebuilt_body_is_placed_on_the_frame_its_parts_exist() {
        let mut app = app();
        let offset = Vec3::new(0.0, -0.9, 0.0);
        let body = app
            .world_mut()
            .spawn((
                humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Scout)),
                Pose::rest(),
                SkeletonRoot(offset),
                Transform::IDENTITY,
            ))
            .id();
        app.update();
        app.update();

        app.world_mut()
            .entity_mut(body)
            .insert(humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Heavy)));
        app.update();

        let skeleton = app.world().get::<Skeleton>(body).unwrap();
        let expected = skeleton.posed_bones(&Pose::rest(), &Transform::from_translation(offset));
        let parts = &app.world().get::<BodyMesh>(body).unwrap().parts;
        assert_eq!(parts.len(), expected.len());
        for (part, bone) in parts.iter().zip(&expected) {
            let placed = app.world().get::<Transform>(*part).unwrap();
            assert!(
                (placed.translation - bone.head).length() < 1e-4
                    && placed.rotation.angle_between(bone.rotation) < 1e-3,
                "{} is at {}, and its bone at {}",
                bone.name,
                placed.translation,
                bone.head,
            );
        }
    }
}
