//! Lighting for things that carry no lightmap: bodies, corpses, placed props and projectiles. Each
//! one's meshes get a `MeshTag` ([`air_light::pack`]) with the air's light where it stands and
//! whether one ray from it reaches sky, which [`LitMaterial`] reads. Not a `DirectionalLight`:
//! Bevy applies one to every mesh a camera draws, walls included. See documentation/sun.md.

use std::f32::consts::PI;

use bevy::camera::visibility::RenderLayers;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::common::air_light::{self, AirLight};
use crate::common::app_mode::AppMode;
use crate::common::render_layers::VIEWMODEL;
use crate::common::skeleton::rig::Skeleton;
use crate::common::lightmap::BASE_AMBIENT;
use crate::common::sky::{Brightness, SunPosition};
use crate::common::air_lit::LitMaterial;
use crate::game::player::{LocalPlayer, Watched};
use crate::game::projectile::Projectile;
use crate::game::viewmodel::{ViewmodelArm, ViewmodelPart};
use crate::game::world_prop::WorldProp;
use crate::tool::sun_bake::{OpenSky, SunBake};

/// Meters above a body's feet that its sun ray starts.
const CHEST: f32 = 1.2;

/// Meters above any other thing's origin, so a prop's ray does not start in the floor.
const ABOVE_ORIGIN: f32 = 0.1;

/// Read before written, so a material already right is not marked changed.
fn keep_the_sun_on_lit_materials(
    position: Res<SunPosition>,
    brightness: Res<Brightness>,
    mut events: MessageReader<AssetEvent<LitMaterial>>,
    mut materials: ResMut<Assets<LitMaterial>>,
) {
    let added = events.read().any(|event| matches!(event, AssetEvent::Added { .. }));
    if !(added || position.is_changed() || brightness.is_changed()) {
        return;
    }
    // Bevy's diffuse is albedo / π × illuminance and a texel is albedo × value, so × π matches a
    // body facing the sun to a wall facing it.
    let sun = position.toward().extend(brightness.sun * PI);
    let stale: Vec<AssetId<LitMaterial>> =
        materials.iter().filter(|(_, material)| material.extension.sun != sun).map(|(id, _)| id).collect();
    for id in stale {
        if let Some(mut material) = materials.get_mut(id) {
            material.extension.sun = sun;
        }
    }
}

/// `try_`: a part can be despawned later in the frame its tag was asked for, as a body is rebuilt
/// or a projectile goes off.
fn retag(entity: Entity, tag: Option<u32>, current: Option<&MeshTag>, commands: &mut Commands) {
    match (tag, current.map(|current| current.value)) {
        (Some(wanted), current) if current != Some(wanted) => {
            commands.entity(entity).try_insert(MeshTag::new(wanted));
        }
        (None, Some(_)) => {
            commands.entity(entity).try_remove::<MeshTag>();
        }
        _ => {}
    }
}

/// The other modes light their own scenes, so they keep the default.
fn follow_the_map_ambient(mode: Res<State<AppMode>>, brightness: Res<Brightness>, mut ambient: ResMut<GlobalAmbientLight>) {
    let wanted = if mode.get().shows_the_world() { brightness.ambient } else { BASE_AMBIENT };
    if ambient.brightness != wanted {
        ambient.brightness = wanted;
    }
}

/// The ambient is a multiple of `base`, the global ambient.
fn tag_at(probe: Vec3, toward: Vec3, open_sky: &OpenSky, air: &AirLight, base: f32) -> u32 {
    let ambient = air.at(probe).map_or(Vec3::ONE, |light| light / base);
    air_light::pack(ambient, open_sky.sees_sun(probe, toward))
}

/// Where a thing was last probed and what its meshes were tagged with then.
#[derive(Component, Clone, Copy, Debug)]
pub struct SunProbe {
    at: Vec3,
    tag: Option<u32>,
}

/// Meters a thing moves before it is probed again. Below what the air's 2 m lattice or a shadow's
/// edge shows.
const REPROBE_DISTANCE: f32 = 0.02;

/// Every frame: a body can walk into the sun with nothing changing but its position. A thing
/// that has not moved keeps its tag, and its meshes are visited only when that changes or a mesh
/// joins it, as when a body is dressed or a weapon is swapped.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn light_what_moves(
    mode: Res<State<AppMode>>,
    open_sky: Res<OpenSky>,
    sun_bake: Res<SunBake>,
    position: Res<SunPosition>,
    brightness: Res<Brightness>,
    roots: Query<
        (Entity, &GlobalTransform, Has<Skeleton>, Has<LocalPlayer>, Has<Watched>, Option<&SunProbe>),
        Or<(With<Skeleton>, With<WorldProp>, With<Projectile>)>,
    >,
    arrived: Query<Entity, Or<(Added<Mesh3d>, Changed<ChildOf>)>>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    meshes: Query<Option<&MeshTag>, With<Mesh3d>>,
    viewmodel: Query<
        (Entity, &RenderLayers, Option<&MeshTag>),
        (With<Mesh3d>, Or<(With<ViewmodelPart>, With<ViewmodelArm>)>),
    >,
    mut joined: Local<bevy::platform::collections::HashSet<Entity>>,
    mut commands: Commands,
) {
    let toward = position.toward();
    // The prop editor stands a figure at the origin with the map hidden round it.
    let shows_the_world = mode.get().shows_the_world();
    let stale = mode.is_changed()
        || open_sky.is_changed()
        || sun_bake.is_changed()
        || position.is_changed()
        || brightness.is_changed();

    joined.clear();
    for mesh in &arrived {
        let root = std::iter::once(mesh).chain(parents.iter_ancestors(mesh)).find(|entity| roots.contains(*entity));
        if let Some(root) = root {
            joined.insert(root);
        }
    }

    let mut ours = None;
    for (root, at, body, local, watched, probed) in &roots {
        let probe = at.translation() + Vec3::Y * if body { CHEST } else { ABOVE_ORIGIN };
        let kept = probed.filter(|probed| !stale && probed.at.distance_squared(probe) <= REPROBE_DISTANCE * REPROBE_DISTANCE);
        let tag = match kept {
            Some(probed) => probed.tag,
            None => shows_the_world.then(|| tag_at(probe, toward, &open_sky, sun_bake.air(), brightness.ambient)),
        };
        // The viewmodel shows ours, or the one being spectated.
        if local || watched {
            ours = tag;
        }
        if kept.is_none() {
            commands.entity(root).try_insert(SunProbe { at: probe, tag });
        }
        if probed.is_some_and(|probed| probed.tag == tag) && !joined.contains(&root) {
            continue;
        }
        for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok(current) = meshes.get(entity) {
                retag(entity, tag, current, &mut commands);
            }
        }
    }
    for (entity, layers, current) in &viewmodel {
        if layers.intersects(&RenderLayers::layer(VIEWMODEL)) {
            retag(entity, ours, current, &mut commands);
        }
    }
}

pub struct SunlitPlugin;

impl Plugin for SunlitPlugin {
    fn build(&self, app: &mut App) {
        // Ungated: bodies stand in the editor too.
        app.add_systems(Update, (follow_the_map_ambient, keep_the_sun_on_lit_materials, light_what_moves));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::Class;
    use crate::common::skeleton::rig::humanoid;
    use crate::common::sunlight::LitSurface;
    use crate::tool::room::{Room, RoomSlot};

    /// A room open to the sky and a closed one far off, a body standing in each, ours in the open
    /// one, and a viewmodel mesh.
    fn app() -> (App, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin).init_state::<AppMode>();
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(8.0, 4.0, 8.0)), Room::new(Vec3::new(20.0, 0.0, 0.0), Vec3::new(28.0, 4.0, 8.0))];
        let surface = LitSurface::build(&rooms, |room, slot| room == 0 && slot == RoomSlot::Ceiling);
        app.insert_resource(OpenSky::of(surface))
            .init_resource::<SunBake>()
            .init_resource::<SunPosition>()
            .init_resource::<Brightness>()
            .add_systems(Update, light_what_moves);
        let mut body = |at: Vec3, ours: bool| {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Heavy));
            let mut root = app.world_mut().spawn((skeleton, GlobalTransform::from_translation(at)));
            if ours {
                root.insert(LocalPlayer);
            }
            let root = root.id();
            app.world_mut().spawn((Mesh3d::default(), ChildOf(root))).id()
        };
        let outside = body(Vec3::new(4.0, 0.0, 4.0), true);
        let inside = body(Vec3::new(24.0, 0.0, 4.0), false);
        let viewmodel = app.world_mut().spawn((Mesh3d::default(), RenderLayers::layer(VIEWMODEL), ViewmodelPart)).id();
        app.update();
        (app, outside, inside, viewmodel)
    }

    fn tag(app: &App, part: Entity) -> Option<u32> {
        app.world().get::<MeshTag>(part).map(|tag| tag.value)
    }

    #[test]
    fn a_body_under_the_sky_is_sunlit_and_one_indoors_is_not() {
        let (app, outside, inside, viewmodel) = app();
        let sunlit = |part| tag(&app, part).expect("every part is tagged") >> 31 == 1;
        assert!(sunlit(outside), "the body under the open sky is not in the sun");
        assert!(!sunlit(inside), "the body in the closed room is in the sun");
        assert_eq!(tag(&app, viewmodel), tag(&app, outside), "the viewmodel is not lit as our body is");
    }

    /// The prop editor stands things up with the map hidden, so nothing there is lit by it.
    #[test]
    fn nothing_is_tagged_where_the_map_is_not_shown() {
        let (mut app, outside, inside, viewmodel) = app();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Prop);
        app.update();
        app.update();
        for part in [outside, inside, viewmodel] {
            assert_eq!(tag(&app, part), None);
        }
    }
}
