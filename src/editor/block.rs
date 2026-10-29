//! A box of solid that cuts the rooms before it in the map's order
//! (`documentation/blocks.md`). It stands up a [`Block`], never a [`Room`],
//! so it is never asked for `rooms`.
//!
//! [`Room`]: crate::tool::room::Room

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::PointResolutionError;
use crate::common::ray::PickBox;
use crate::common::world_surface::Volume;
use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::editor_room::EditorRoom;
use crate::editor::mirror::Mirroring;
use crate::get;
use crate::tool::room::{Block, Room};

pub const BLOCK_COLOR: Color = Color::srgb(0.45, 0.6, 0.9);

#[derive(Serialize, Deserialize)]
pub struct EditorBlock {
    volume: EditorRoom,
}

impl EditorBlock {
    pub fn from_room(volume: EditorRoom) -> Self {
        Self { volume }
    }

    fn shape(&self) -> Room {
        let (min, max) = self.volume.bounds();
        Room::new(min, max)
    }
}

#[typetag::serde(name = "block")]
impl FeatureTrait for EditorBlock {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        self.volume.get_point(key)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        self.volume.editor_ui(ui, features, prior_feature_order, retarget_request)
    }

    fn type_name(&self) -> String {
        get!("editor.features.block.title")
    }

    fn type_key(&self) -> &'static str { "block" }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        self.volume.draw_box(gizmos, BLOCK_COLOR);
    }

    fn entity(&self) -> Option<Entity> {
        self.volume.entity()
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.volume.set_entity(entity);
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        let (min, max) = self.volume.bounds();
        commands.entity(entity).insert((
            Transform::from_translation((min + max) / 2.0),
            Block(self.shape()),
            self.volume.surfaces().clone(),
        ));
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        self.volume.resolve_references(features);
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        self.volume.parent_ids()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        self.volume.available_point_keys()
    }

    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)> {
        self.volume.reference_points_for_ray(ray)
    }

    fn drag_handle(&mut self, is_max: bool, axis: u8, new_world_value: f32) -> bool {
        self.volume.drag_handle(is_max, axis, new_world_value)
    }

    fn pick_boxes(&self) -> Vec<PickBox> {
        let (min, max) = self.volume.bounds();
        vec![PickBox { center: (min + max) / 2.0, rotation: Quat::IDENTITY, half_extents: (max - min) / 2.0 }]
    }

    fn drag_handle_bounds(&self) -> Option<(Vec3, Vec3)> {
        Some(self.volume.bounds())
    }

    fn volumes(&self) -> Vec<Volume> {
        vec![Volume::solid(self.shape())]
    }

    fn get_point_ref(&self, key: &str) -> Option<&PointRef> {
        self.volume.get_point_ref(key)
    }

    fn get_point_ref_mut(&mut self, key: &str) -> Option<&mut PointRef> {
        self.volume.get_point_ref_mut(key)
    }

    fn point_refs(&self) -> Vec<&PointRef> {
        self.volume.point_refs()
    }

    fn own_bases(&self, point_ref_key: &str) -> Vec<(String, Vec3)> {
        self.volume.own_bases(point_ref_key)
    }

    fn rebase_on_own(&mut self, point_ref_key: &str, base: &str) -> bool {
        self.volume.rebase_on_own(point_ref_key, base)
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.volume.mirror(mirroring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use crate::editor::editable::FeatureTimeline;
    use crate::editor::mirror::{Mirror, Symmetry};
    use crate::editor::volume_order::place_volumes;
    use crate::game::collision::CollisionWorld;
    use crate::tool::room::{MapVolumes, VolumeOrder};
    use crate::tool::show::GizmoVisibility;
    use crate::tool::tool_helpers::find_nearest_feature_hit;

    fn boxed(min: Vec3, max: Vec3) -> EditorRoom {
        EditorRoom::from_point_refs(PointRef::absolute(min.x, min.y, min.z), PointRef::absolute(max.x, max.y, max.z))
    }

    fn standing(timeline: FeatureTimeline) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, place_volumes).chain());
        app.update();
        app.update();
        app
    }

    fn collision(app: &mut App) -> CollisionWorld {
        let volumes = app.world_mut().run_system_once(|volumes: MapVolumes| volumes.volumes()).unwrap();
        let mut collision = CollisionWorld::default();
        collision.rebuild_volumes(&volumes);
        collision
    }

    /// A hall split by a wall-block, and a doorway room after it. Breaks if
    /// the volumes reach the surface in any order but the timeline's.
    #[test]
    fn a_doorway_after_a_wall_tunnels_through_it_and_before_it_does_not() {
        let mut timeline = FeatureTimeline::default();
        let hall = timeline.apply_feature(Box::new(boxed(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0))));
        let wall = timeline.apply_feature(Box::new(EditorBlock::from_room(boxed(Vec3::new(0.0, 0.0, 4.0), Vec3::new(10.0, 3.0, 6.0)))));
        let door = timeline.apply_feature(Box::new(boxed(Vec3::new(2.0, 0.0, 3.0), Vec3::new(3.0, 2.0, 7.0))));
        let mut app = standing(timeline);

        let through_the_door = Ray3d::new(Vec3::new(2.5, 1.0, 1.0), Dir3::Z);
        let beside_it = Ray3d::new(Vec3::new(5.0, 1.0, 1.0), Dir3::Z);
        let world = collision(&mut app);
        assert!((world.ray_distance(&through_the_door, 20.0).unwrap() - 9.0).abs() < 1e-3, "the doorway is shut");
        assert!((world.ray_distance(&beside_it, 20.0).unwrap() - 3.0).abs() < 1e-3, "the wall is not there");
        assert!(!world.inside_map(Vec3::new(5.0, 1.0, 5.0)), "inside the wall is map");

        let timeline = app.world().resource::<FeatureTimeline>();
        let (picked, _) = find_nearest_feature_hit(&beside_it, timeline, &GizmoVisibility::default()).unwrap();
        assert_eq!(picked, wall);
        let (picked, _) = find_nearest_feature_hit(&through_the_door, timeline, &GizmoVisibility::default()).unwrap();
        assert_eq!(picked, hall);

        app.world_mut().resource_mut::<FeatureTimeline>().move_to(door, 1);
        app.update();
        let world = collision(&mut app);
        assert!((world.ray_distance(&through_the_door, 20.0).unwrap() - 3.0).abs() < 1e-3, "a doorway before the wall still opens it");
    }

    /// Breaks if a copy takes the Mirror's place: a room between the two
    /// would tunnel through one half and not the other.
    #[test]
    fn a_mirrored_block_stands_at_its_sources_place() {
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(boxed(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 3.0, 10.0))));
        let block = timeline.apply_feature(Box::new(EditorBlock::from_room(boxed(Vec3::new(2.0, 0.0, 2.0), Vec3::new(4.0, 1.0, 4.0)))));
        timeline.apply_feature(Box::new(boxed(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 3.0, 1.0))));
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_symmetry(Symmetry::MirrorX).with_sources(vec![block]),
        ));
        let mut app = standing(timeline);

        let world = app.world_mut();
        let mut places: Vec<VolumeOrder> = world.query_filtered::<&VolumeOrder, With<Block>>().iter(world).cloned().collect();
        places.sort();
        assert_eq!(places, vec![VolumeOrder(vec![1]), VolumeOrder(vec![1, 0])]);
    }

    /// Breaks if the entities' order (bakes, collision) and the copies' own
    /// (picking) disagree for a copy of a copy.
    #[test]
    fn a_mirror_of_a_mirror_places_its_copies_as_picking_does() {
        use crate::editor::volume_order::{place, positions, ranked};
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(boxed(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 3.0, 10.0))));
        let block = timeline.apply_feature(Box::new(EditorBlock::from_room(boxed(Vec3::new(2.0, 0.0, 3.0), Vec3::new(4.0, 1.0, 5.0)))));
        let across_x = timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_symmetry(Symmetry::MirrorX).with_sources(vec![block]),
        ));
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_symmetry(Symmetry::MirrorZ).with_sources(vec![block, across_x]),
        ));
        let mut app = standing(timeline);

        let key = |volume: &Room| { let min = volume.bounds().0; (min.x as i32, min.z as i32) };
        let world = app.world_mut();
        let mut entities: Vec<((i32, i32), VolumeOrder)> =
            world.query::<(&Block, &VolumeOrder)>().iter(world).map(|(block, order)| (key(&block.0), order.clone())).collect();
        entities.sort();

        let timeline = app.world().resource::<FeatureTimeline>();
        let positions = positions(timeline);
        let mut picked: Vec<((i32, i32), VolumeOrder)> = Vec::new();
        for (id, feature) in timeline.active_features() {
            for volume in feature.object().volumes() {
                picked.push((key(&volume.shape), place(positions[&id], &[])));
            }
            for copy in feature.object().standing_copies() {
                for volume in copy.object.volumes() {
                    picked.push((key(&volume.shape), place(positions[&copy.source], &copy.images)));
                }
            }
        }
        let mut orders: Vec<VolumeOrder> = picked.iter().map(|(_, order)| order.clone()).collect();
        ranked(&mut orders);
        let mut picked: Vec<_> = picked.into_iter().map(|(at, _)| at).zip(orders).filter(|(at, _)| *at != (-10, -10)).collect();
        picked.sort();
        assert_eq!(entities.len(), 4, "{entities:?}");
        assert_eq!(entities, picked);
    }

    /// Breaks if a block answers `rooms`: it would be air.
    #[test]
    fn a_block_is_no_room() {
        let room = EditorRoom::from_point_refs(PointRef::absolute(0.0, 0.0, 0.0), PointRef::absolute(1.0, 1.0, 1.0));
        let block = EditorBlock::from_room(room);
        assert!(block.rooms().is_empty());
        assert!(block.volumes().iter().all(|volume| volume.fill == crate::common::world_surface::Fill::Solid));
    }
}
