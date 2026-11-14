//! How things that move are lit, since they carry no lightmap: bodies, corpses, placed props and
//! projectiles. Each one's meshes get a `MeshTag` ([`air_light::pack`]) holding the light in the
//! air where it stands ([`AirLight`]) and whether a ray from it reaches sky ([`OpenSky`]), and their
//! material ([`LitMaterial`]) scales its ambient by the first and adds the sun by the second. See
//! "Things that move" in documentation/sun.md.
//!
//! The sun is a switch, not a fade: a body half out of a doorway is one or the other. It is not a
//! `DirectionalLight`, which Bevy would apply to every mesh a camera draws, walls included.

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
use crate::game::world_prop::WorldProp;
use crate::tool::sun_bake::{OpenSky, SunBake};

/// Where a body's sun ray starts, above its feet: about its chest.
const CHEST: f32 = 1.2;

/// Where any other moving thing's sun ray starts, above its origin, so a prop standing on a
/// floor does not start its ray in the floor.
const ABOVE_ORIGIN: f32 = 0.1;

/// Every lit material carries the sun, so one made since it last moved is caught too. Read before
/// it is written, so a material already right is not marked changed.
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
    // Bevy's diffuse is albedo over π times illuminance, and a lightmap texel is albedo times its
    // value, so this makes a body facing the sun as bright as a wall facing it.
    let sun = position.toward().extend(brightness.sun * PI);
    let stale: Vec<AssetId<LitMaterial>> =
        materials.iter().filter(|(_, material)| material.extension.sun != sun).map(|(id, _)| id).collect();
    for id in stale {
        if let Some(mut material) = materials.get_mut(id) {
            material.extension.sun = sun;
        }
    }
}

/// Writes `tag` onto one mesh if it differs, or takes the tag off for `None`.
fn retag(entity: Entity, tag: Option<u32>, current: Option<&MeshTag>, commands: &mut Commands) {
    match (tag, current.map(|current| current.value)) {
        (Some(wanted), current) if current != Some(wanted) => {
            commands.entity(entity).insert(MeshTag::new(wanted));
        }
        (None, Some(_)) => {
            commands.entity(entity).remove::<MeshTag>();
        }
        _ => {}
    }
}

/// The global ambient is the map's while the map is shown, which lightmapped walls ignore and
/// things that move scale. The other modes light their own scenes, so they keep the default.
fn follow_the_map_ambient(mode: Res<State<AppMode>>, brightness: Res<Brightness>, mut ambient: ResMut<GlobalAmbientLight>) {
    let wanted = if mode.get().shows_the_world() { brightness.ambient } else { BASE_AMBIENT };
    if ambient.brightness != wanted {
        ambient.brightness = wanted;
    }
}

/// How a thing standing at `probe` is lit, as its tag. Its ambient is over the global one, which is
/// the map's.
fn tag_at(probe: Vec3, toward: Vec3, open_sky: &OpenSky, air: &AirLight, base: f32) -> u32 {
    let ambient = air.at(probe).map_or(Vec3::ONE, |light| light / base);
    air_light::pack(ambient, open_sky.sees_sun(probe, toward))
}

/// Every frame, since a body walks out of a doorway without anything about it changing but where
/// it is. A mesh's tag is written only when it changes. The viewmodel's meshes take the viewed body's.
#[allow(clippy::type_complexity)]
fn light_what_moves(
    mode: Res<State<AppMode>>,
    open_sky: Res<OpenSky>,
    sun_bake: Res<SunBake>,
    position: Res<SunPosition>,
    brightness: Res<Brightness>,
    roots: Query<(Entity, &GlobalTransform, Has<Skeleton>, Has<LocalPlayer>, Has<Watched>), Or<(With<Skeleton>, With<WorldProp>, With<Projectile>)>>,
    children: Query<&Children>,
    meshes: Query<Option<&MeshTag>, With<Mesh3d>>,
    viewmodel: Query<(Entity, &RenderLayers, Option<&MeshTag>), With<Mesh3d>>,
    mut commands: Commands,
) {
    let toward = position.toward();
    // The prop editor stands a figure at the origin with the map hidden round it.
    let shows_the_world = mode.get().shows_the_world();
    let mut ours = None;
    for (root, at, body, local, watched) in &roots {
        let probe = at.translation() + Vec3::Y * if body { CHEST } else { ABOVE_ORIGIN };
        let tag = shows_the_world.then(|| tag_at(probe, toward, &open_sky, sun_bake.air(), brightness.ambient));
        // The viewmodel shows whichever body is viewed: ours, or the one being spectated.
        if local || watched {
            ours = tag;
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
        let viewmodel = app.world_mut().spawn((Mesh3d::default(), RenderLayers::layer(VIEWMODEL))).id();
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
