use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy_egui::egui;
use crate::get;
use crate::tool::room::{CalculateRoomGeometry, ClearRoomGeometry, Room};

pub struct BakePlugin;

impl Plugin for BakePlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<CalculateRoomGeometry>()
            .add_message::<ClearRoomGeometry>()
            .add_systems(Update, (Self::post_startup, Self::bake_room_geometry, Self::clear_room_geometry))
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

        for entity in &existing_bakes {
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
}

#[derive(Component)]
pub struct BakedRoomGeometry;

#[derive(Default)]
pub struct BakeCommands {
    pub calculate_room_geometry: bool,
    pub clear_room_geometry: bool,
}
