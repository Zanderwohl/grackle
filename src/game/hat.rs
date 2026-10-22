//! Hats on heads. Which hat is a class's, from `loadouts.toml`; how it sits is
//! the prop's [`HatSpec`](crate::prop::archetype::HatSpec). See
//! [the plan](../../documentation/prop-archetypes.md).
//!
//! Compared every frame rather than keyed on arrival, as a held weapon is: the
//! class, the catalogue and the model each turn up on their own schedule.

use bevy::prelude::*;

use crate::common::assets::Assets as PackAssets;
use crate::common::class::Class;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::weapon::WeaponCatalogue;
use crate::game::body_mesh::FirstPersonHidden;
use crate::game::player::{LocalPlayer, ViewMode};
use crate::game::skeleton::SkeletonRoot;
use crate::prop::archetype::{hat_transform, HatFit};
use crate::prop::baked::{PropCache, SurfaceMaterials};

#[derive(Component)]
pub struct HatPart;

/// What is on a body's head, and what it was drawn for.
#[derive(Component)]
pub struct WornHat {
    model: Option<String>,
    cached_at: u64,
    parts: Vec<Entity>,
    fit: HatFit,
}

pub struct HatPlugin;

impl Plugin for HatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropCache>()
            .init_resource::<SurfaceMaterials>()
            .add_systems(Update, dress_hats)
            // Before propagation, with the bone meshes, or the hat trails the
            // head by a frame.
            .add_systems(PostUpdate, place_hats.before(TransformSystems::Propagate));
    }
}

#[allow(clippy::too_many_arguments)]
fn dress_hats(
    mut commands: Commands,
    catalogue: Res<WeaponCatalogue>,
    packs: Res<PackAssets>,
    mut cache: ResMut<PropCache>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clips: ResMut<Assets<Clip>>,
    view: Res<ViewMode>,
    bodies: Query<(Entity, &Class, Has<LocalPlayer>, Option<&WornHat>), With<Skeleton>>,
) {
    for (body, class, ours, worn) in &bodies {
        let wanted = catalogue.loadout(*class).and_then(|loadout| loadout.hat.clone());
        if let Some(worn) = worn
            && worn.model == wanted
            && worn.cached_at == cache.generation()
        {
            continue;
        }

        if let Some(worn) = worn {
            for part in &worn.parts {
                commands.entity(*part).try_despawn();
            }
        }

        let mut parts = Vec::new();
        let mut fit = HatFit::default();
        let baked = wanted.as_deref().and_then(|model| {
            cache.ensure(model, &packs, &mut meshes, &mut materials, &mut surfaces, &mut clips)
        });
        if let Some(baked) = baked {
            fit = baked.hat.for_class(Some(*class));
            let meshes = baked.parts.clone();
            commands.entity(body).with_children(|body| {
                for (mesh, material) in meshes {
                    parts.push(
                        body.spawn((
                            HatPart,
                            FirstPersonHidden,
                            // Spawned hidden: `hide_own_body` would only catch
                            // it a frame later, a brim across the view.
                            match ours && !view.shows_own_body() {
                                true => Visibility::Hidden,
                                false => Visibility::Inherited,
                            },
                            Mesh3d(mesh),
                            MeshMaterial3d(material),
                            Transform::IDENTITY,
                        ))
                        .id(),
                    );
                }
            });
        }
        commands.entity(body).insert(WornHat {
            model: wanted,
            cached_at: cache.generation(),
            parts,
            fit,
        });
    }
}

fn place_hats(
    bodies: Query<(&Skeleton, &Pose, Option<&SkeletonRoot>, &WornHat)>,
    mut parts: Query<&mut Transform, With<HatPart>>,
) {
    for (skeleton, pose, offset, worn) in &bodies {
        if worn.parts.is_empty() {
            continue;
        }
        let root = Transform::from_translation(offset.map_or(Vec3::ZERO, |offset| offset.0));
        let Some(placed) = hat_transform(skeleton, pose, &root, &worn.fit) else { continue };
        for part in &worn.parts {
            if let Ok(mut transform) = parts.get_mut(*part) {
                *transform = placed;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::default_humanoid;
    use crate::common::weapon::Loadout;

    /// The shipped loadout's hat, from the file to a head. Fails if the
    /// loadout's key, the bake or the crown frame stop meeting up.
    #[test]
    fn a_class_with_a_hat_wears_it_on_its_head() {
        if std::fs::metadata("assets/default/props/top_hat.gpp").is_err() {
            // Not run from the repo root.
            return;
        }
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::render::mesh::MeshPlugin,
        ));
        app.init_asset::<StandardMaterial>();
        app.add_plugins(crate::common::skeleton::ClipAssetsPlugin);
        let mut catalogue = WeaponCatalogue::default();
        catalogue.set_loadout(Class::Mercenary, Loadout { hat: Some("top_hat".into()), ..default() });
        app.insert_resource(catalogue)
            .init_resource::<PackAssets>()
            .init_resource::<ViewMode>()
            .add_plugins(HatPlugin);

        let skeleton = default_humanoid();
        let head = skeleton.index_of(crate::common::skeleton::rig::bone::HEAD).unwrap();
        let crown = skeleton.posed_bone(&Pose::rest(), &Transform::IDENTITY, head).tail;
        let body = app.world_mut().spawn((skeleton.clone(), Pose::rest(), Class::Mercenary)).id();
        let bare = app.world_mut().spawn((default_humanoid().clone(), Pose::rest(), Class::Heavy)).id();
        app.update();
        app.update();

        let worn = app.world().get::<WornHat>(body).expect("no hat was put on");
        assert!(!worn.parts.is_empty(), "the hat has no parts");
        let placed = app.world().get::<Transform>(worn.parts[0]).unwrap();
        assert!((placed.translation - crown).length() < 0.1, "the hat is not on the head");

        let bare = app.world().get::<WornHat>(bare).unwrap();
        assert!(bare.parts.is_empty(), "a class with no hat wore one");
    }

    fn worn_parts(app: &App, body: Entity) -> Vec<Visibility> {
        let worn = app.world().get::<WornHat>(body).expect("no hat was put on");
        assert!(!worn.parts.is_empty(), "the hat has no parts");
        worn.parts.iter().map(|part| *app.world().get::<Visibility>(*part).unwrap()).collect()
    }

    /// Your own hat is hidden from inside your head, where its brim would cut
    /// across the view, and drawn in third person. Fails if the hat loses the
    /// marker `hide_own_body` goes by, or is hung somewhere it does not look.
    #[test]
    fn your_own_hat_shows_in_third_person_and_not_in_first() {
        if std::fs::metadata("assets/default/props/top_hat.gpp").is_err() {
            return;
        }
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::render::mesh::MeshPlugin,
        ));
        app.init_asset::<StandardMaterial>();
        app.add_plugins(crate::common::skeleton::ClipAssetsPlugin);
        let mut catalogue = WeaponCatalogue::default();
        catalogue.set_loadout(Class::Mercenary, Loadout { hat: Some("top_hat".into()), ..default() });
        app.insert_resource(catalogue)
            .init_resource::<PackAssets>()
            .insert_resource(ViewMode::FirstPerson)
            .add_plugins(HatPlugin)
            .add_systems(Update, crate::game::body_mesh::hide_own_body.after(dress_hats));

        let ours = app
            .world_mut()
            .spawn((default_humanoid().clone(), Pose::rest(), Class::Mercenary, LocalPlayer))
            .id();
        let theirs =
            app.world_mut().spawn((default_humanoid().clone(), Pose::rest(), Class::Mercenary)).id();
        app.update();
        app.update();

        assert!(worn_parts(&app, ours).iter().all(|v| *v == Visibility::Hidden), "our hat is in our eyes");
        assert!(worn_parts(&app, theirs).iter().all(|v| *v != Visibility::Hidden), "their hat vanished");

        *app.world_mut().resource_mut::<ViewMode>() = ViewMode::ThirdPerson;
        app.update();
        assert!(worn_parts(&app, ours).iter().all(|v| *v != Visibility::Hidden), "our hat is missing in third person");

        *app.world_mut().resource_mut::<ViewMode>() = ViewMode::FirstPerson;
        app.update();
        assert!(worn_parts(&app, ours).iter().all(|v| *v == Visibility::Hidden), "our hat came back into view");
    }
}
