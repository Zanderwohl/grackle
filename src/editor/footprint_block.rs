//! A footprint stood up as solid: each named region a [`Block`] prism that cuts
//! the rooms before it, as [`EditorBlock`] does a box. The drawing, its handles
//! and its inspector are the [`FootprintRoom`]'s.
//!
//! [`Block`]: crate::tool::room::Block
//! [`EditorBlock`]: crate::editor::block::EditorBlock

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::PointResolutionError;
use crate::common::world_surface::{Fill, Volume};
use crate::editor::block::BLOCK_COLOR;
use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::footprint_room::FootprintRoom;
use crate::editor::mirror::Mirroring;
use crate::get;

#[derive(Serialize, Deserialize)]
pub struct FootprintBlock {
    plan: FootprintRoom,
}

impl FootprintBlock {
    pub fn from_footprint(plan: FootprintRoom) -> Self {
        Self { plan }
    }
}

#[typetag::serde(name = "footprint_block")]
impl FeatureTrait for FootprintBlock {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        self.plan.get_point(key)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        self.plan.editor_ui(ui, features, prior_feature_order, retarget_request)
    }

    fn type_name(&self) -> String {
        get!("editor.features.footprint_block.title")
    }

    fn tool(&self) -> Option<crate::tool::Tools> { Some(crate::tool::Tools::FootprintBlock) }
    fn type_key(&self) -> &'static str { "footprint_block" }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        self.plan.draw(gizmos, BLOCK_COLOR);
    }

    fn selected_gizmos(&self, gizmos: &mut Gizmos) {
        self.plan.selected_gizmos(gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.plan.entity()
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.plan.set_entity(entity);
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        self.plan.stand_up(commands, entity, Fill::Solid);
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        self.plan.resolve_references(features);
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        self.plan.parent_ids()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        self.plan.available_point_keys()
    }

    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)> {
        self.plan.reference_points_for_ray(ray)
    }

    fn point_refs(&self) -> Vec<&PointRef> {
        self.plan.point_refs()
    }

    fn get_point_ref(&self, key: &str) -> Option<&PointRef> {
        self.plan.get_point_ref(key)
    }

    fn get_point_ref_mut(&mut self, key: &str) -> Option<&mut PointRef> {
        self.plan.get_point_ref_mut(key)
    }

    fn own_bases(&self, point_ref_key: &str) -> Vec<(String, Vec3)> {
        self.plan.own_bases(point_ref_key)
    }

    fn rebase_on_own(&mut self, point_ref_key: &str, base: &str) -> bool {
        self.plan.rebase_on_own(point_ref_key, base)
    }

    fn volumes(&self) -> Vec<Volume> {
        self.plan.rooms().iter().cloned().map(Volume::solid).collect()
    }

    fn footprint(&self) -> Option<&FootprintRoom> {
        Some(&self.plan)
    }

    fn footprint_mut(&mut self) -> Option<&mut FootprintRoom> {
        Some(&mut self.plan)
    }

    fn drag_handle(&mut self, is_max: bool, axis: u8, new_world_value: f32) -> bool {
        self.plan.drag_handle(is_max, axis, new_world_value)
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.plan.mirror(mirroring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::FeatureTimeline;
    use crate::editor::editor_room::EditorRoom;
    use crate::editor::footprint_room::ChainCorner;
    use crate::editor::volume_order::place_volumes;
    use crate::game::collision::CollisionWorld;
    use crate::tool::room::{Block, MapVolumes, Room};
    use crate::tool::show::GizmoVisibility;
    use crate::tool::tool_helpers::find_nearest_feature_hit;
    use bevy::ecs::system::RunSystemOnce;

    fn column() -> FootprintBlock {
        let mut plan = FootprintRoom::new(PointRef::absolute(4.0, 0.0, 4.0), 3.0);
        let chain: Vec<_> = [(4.0, 4.0), (6.0, 4.0), (4.0, 6.0)]
            .into_iter()
            .map(|(x, z)| ChainCorner::New(PointRef::absolute(x, 0.0, z)))
            .collect();
        plan.add_chain(&chain, true, &HashMap::new());
        FootprintBlock::from_footprint(plan)
    }

    #[test]
    fn a_footprint_block_is_no_room() {
        let block = column();
        assert!(block.rooms().is_empty());
        let volumes = block.volumes();
        assert_eq!(volumes.len(), 1);
        assert!(volumes.iter().all(|volume| volume.fill == crate::common::world_surface::Fill::Solid));
        assert!((volumes[0].shape.volume() - 2.0 * 3.0).abs() < 1e-4);
    }

    /// Breaks if a column stands up as air, or collision or picking misses it.
    #[test]
    fn a_column_in_a_hall_is_solid_and_picked() {
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(10.0, 3.0, 10.0),
        )));
        let column = timeline.apply_feature(Box::new(column()));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, place_volumes).chain());
        app.update();
        app.update();

        let world = app.world_mut();
        assert_eq!(world.query::<&Block>().iter(world).count(), 1, "the column stood up no block");
        assert_eq!(world.query::<&Room>().iter(world).count(), 1, "the column stood up a room");

        let volumes = app.world_mut().run_system_once(|volumes: MapVolumes| volumes.volumes()).unwrap();
        let mut collision = CollisionWorld::default();
        collision.rebuild_volumes(&volumes);
        let at_the_column = Ray3d::new(Vec3::new(4.5, 1.0, 1.0), Dir3::Z);
        assert!((collision.ray_distance(&at_the_column, 20.0).unwrap() - 3.0).abs() < 1e-3, "the column is not there");
        assert!(!collision.inside_map(Vec3::new(4.5, 1.0, 4.5)), "inside the column is map");

        let timeline = app.world().resource::<FeatureTimeline>();
        let (picked, _) = find_nearest_feature_hit(&at_the_column, timeline, &GizmoVisibility::default()).unwrap();
        assert_eq!(picked, column);
    }

    /// Breaks if a Mirror's copy of a column stands up a room.
    #[test]
    fn a_mirrored_column_stands_up_blocks() {
        use crate::editor::mirror::{Mirror, Symmetry};
        let mut timeline = FeatureTimeline::default();
        let column = timeline.apply_feature(Box::new(column()));
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_symmetry(Symmetry::MirrorX).with_sources(vec![column]),
        ));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, (FeatureTimeline::sync_entities, place_volumes).chain());
        app.update();
        app.update();

        let world = app.world_mut();
        let mut blocks: Vec<f32> = world.query::<&Block>().iter(world).map(|block| block.0.bounds().0.x).collect();
        blocks.sort_by(f32::total_cmp);
        assert_eq!(world.query::<&Room>().iter(world).count(), 0, "a copy stood up a room");
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert!(blocks[0] < 0.0 && blocks[1] > 0.0, "the copy is not across the mirror: {blocks:?}");
    }
}
