use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use crate::common::app_mode::AppMode;
use bevy_egui::egui;
use crate::editor::editable::FeatureTag;
use crate::get;
use crate::tool::room::{CalculateRoomGeometry, ClearRoomGeometry, Room};

#[derive(Message)]
pub struct LogECS;

/// Everything rebuilt from the map before a round starts. A set so a new kind
/// of bake joins by naming it, and the round setup orders after all of them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BakeSystems {
    All,
}

pub struct BakePlugin;

impl Plugin for BakePlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<CalculateRoomGeometry>()
            .add_message::<ClearRoomGeometry>()
            .add_message::<LogECS>()
            .add_systems(Update, (Self::post_startup, Self::clear_room_geometry, Self::log_ecs).run_if(in_state(AppMode::Editor)))
            // Ungated: the map can be edited during a round.
            .add_systems(Update, Self::bake_room_geometry)
            .configure_sets(OnEnter(AppMode::Play), BakeSystems::All)
            .add_systems(
                OnEnter(AppMode::Play),
                Self::bake_everything_for_the_round.in_set(BakeSystems::All),
            )
        ;
    }
}

impl BakePlugin {
    pub fn ui(ui: &mut egui::Ui) -> BakeCommands {
        let mut commands = BakeCommands::default();
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                if ui.button(get!("bakes.room_geometry")).clicked() {
                    commands.calculate_room_geometry = true;
                }
                if ui.small_button("x").clicked() {
                    commands.clear_room_geometry = true;
                }
            });
            if ui.button("Log ECS").clicked() {
                commands.log_ecs = true;
            }
        });
        commands
    }

    fn post_startup(
        frames: Res<FrameCount>,
        mut room_events: MessageWriter<CalculateRoomGeometry>,
    ) {
        if frames.0 == 5 {
            room_events.write(CalculateRoomGeometry);
        }
    }

    fn bake_room_geometry(
        mut events: MessageReader<CalculateRoomGeometry>,
        rooms: Query<&Room>,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();

        bake_rooms(&rooms, &existing_bakes, &mut commands, &mut meshes, &mut materials);
    }

    /// Rebuild the room meshes before a round starts, unconditionally. A
    /// client adopting the server's map enters Play the same frame, and the
    /// bake request it wrote is read a frame later. Collision does not need
    /// these meshes, so a missed bake is an invisible map, not a fall.
    fn bake_everything_for_the_round(
        rooms: Query<&Room>,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
    ) {
        bake_rooms(&rooms, &existing_bakes, &mut commands, &mut meshes, &mut materials);
    }

    fn clear_room_geometry(
        mut events: MessageReader<ClearRoomGeometry>,
        existing_bakes: Query<Entity, With<BakedRoomGeometry>>,
        mut commands: Commands,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();

        let count = existing_bakes.iter().count();
        for entity in &existing_bakes {
            commands.entity(entity).despawn();
        }
        if count > 0 {
            info!("Cleared {} baked room mesh(es)", count);
        }
    }

    fn log_ecs(
        mut events: MessageReader<LogECS>,
        query: Query<(Entity, &FeatureTag, &Transform, Option<&Room>, Option<&PointLight>)>,
    ) {
        if events.read().next().is_none() { return; }
        events.clear();

        info!("=== ECS Entity Dump ===");
        for (entity, tag, transform, room, point_light) in &query {
            let room_str = match room {
                Some(r) => format!("{:?}", r),
                None => "None".to_string(),
            };
            let light_str = match point_light {
                Some(l) => format!("{:?}", l),
                None => "None".to_string(),
            };
            info!(
                "{:?} | {:?} | {:?} | Room: {} | PointLight: {}",
                entity, tag, transform.translation, room_str, light_str,
            );
        }
        info!("=== {} entities total ===", query.iter().count());
    }
}

/// Build one mesh per room, minus the faces neighbouring rooms open up. Shared
/// by the editor's bake and the round-start bake.
fn bake_rooms(
    rooms: &Query<&Room>,
    existing_bakes: &Query<Entity, With<BakedRoomGeometry>>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for entity in existing_bakes {
        commands.entity(entity).despawn();
    }

    let material = materials.add(StandardMaterial {
        base_color: Color::srgb_u8(255, 255, 255),
        ..Default::default()
    });

    let all_rooms: Vec<Room> = rooms.iter().map(|r| Room::new(r.min, r.max)).collect();

    for (i, room) in all_rooms.iter().enumerate() {
        let others: Vec<Room> = all_rooms.iter().enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, r)| r.clone())
            .collect();
        let mesh = meshes.add(room.bake_faces(&others));
        commands.spawn((
            BakedRoomGeometry,
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
        ));
    }

    info!("Baked geometry for {} room(s)", all_rooms.len());
}

#[derive(Component)]
pub struct BakedRoomGeometry;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::GamePlugin;

    /// A headless app with a room, enough to swap into Play.
    fn headless_with_a_room() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::state::app::StatesPlugin,
            bevy::asset::AssetPlugin::default(),
            // `post_startup` bakes at frame five, past anything these tests do.
            bevy::diagnostic::FrameCountPlugin,
            bevy::input::InputPlugin,
            bevy::time::TimePlugin,
        ));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.add_plugins(GamePlugin);
        app.add_plugins(BakePlugin);
        app.world_mut().spawn(Room::new(
            Vec3::new(-4.0, 0.0, -4.0),
            Vec3::new(4.0, 3.0, 4.0),
        ));
        app.update();
        app
    }

    fn baked(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<BakedRoomGeometry>>()
            .iter(app.world())
            .count()
    }

    /// Breaks if entering Play stops baking unasked; a client's round is then
    /// played in an invisible map.
    #[test]
    fn entering_play_bakes_without_being_asked() {
        let mut app = headless_with_a_room();
        assert_eq!(baked(&mut app), 0, "something baked before the round started");

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        assert_eq!(baked(&mut app), 1, "the round started against an unbaked map");
    }

    #[test]
    fn a_room_added_since_the_last_bake_is_in_the_new_one() {
        let mut app = headless_with_a_room();
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();
        assert_eq!(baked(&mut app), 1);

        // Back to the editor, add a room, and start another round.
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Editor);
        app.update();
        app.world_mut().spawn(Room::new(
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(14.0, 3.0, 14.0),
        ));
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Play);
        app.update();

        assert_eq!(baked(&mut app), 2, "the new round was played in the old map");
    }
}

#[derive(Default)]
pub struct BakeCommands {
    pub calculate_room_geometry: bool,
    pub clear_room_geometry: bool,
    pub log_ecs: bool,
}
