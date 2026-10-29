//! Puts every room and block in the map's order (`documentation/blocks.md`).
//! A Mirror's copy stands at its source's place, not the Mirror's: at the
//! Mirror it would come after a room that tunnels through the original, and the
//! two halves would differ.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::editor::editable::{FeatureId, FeatureTag, FeatureTimeline};
use crate::editor::mirror::MirrorCopy;
use crate::tool::room::{Block, Room, VolumeOrder};

pub fn place(position: usize, images: &[usize]) -> VolumeOrder {
    VolumeOrder(std::iter::once(position).chain(images.iter().copied()).map(|n| n as u32).collect())
}

/// Each feature's place in the timeline, by id.
pub fn positions(timeline: &FeatureTimeline) -> HashMap<FeatureId, usize> {
    timeline.feature_order().iter().enumerate().map(|(position, id)| (*id, position)).collect()
}

/// Every frame, writing only what moved: a written order is a changed one, and
/// a changed one is rebaked.
pub fn place_volumes(
    timeline: Res<FeatureTimeline>,
    volumes: Query<(Entity, Option<&VolumeOrder>), Or<(With<Room>, With<Block>)>>,
    parents: Query<&ChildOf>,
    tags: Query<&FeatureTag>,
    copies: Query<&MirrorCopy>,
    mut commands: Commands,
) {
    let positions = positions(&timeline);
    for (entity, current) in &volumes {
        let mut images = Vec::new();
        let mut source = None;
        let mut feature = None;
        let mut at = entity;
        loop {
            if let Ok(copy) = copies.get(at) {
                images.push(copy.image);
                source.get_or_insert(copy.source);
            }
            if let Ok(tag) = tags.get(at) {
                feature = Some(FeatureId::from_raw(tag.feature_id));
                break;
            }
            let Ok(parent) = parents.get(at) else { break };
            at = parent.parent();
        }
        let Some(position) = source.or(feature).and_then(|id| positions.get(&id)) else { continue };
        let order = place(*position, &images);
        if current != Some(&order) {
            commands.entity(entity).insert(order);
        }
    }
}
