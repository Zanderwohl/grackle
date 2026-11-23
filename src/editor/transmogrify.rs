//! Turning a feature into another kind with the same shape: a box between a
//! room, a block and a trigger, a footprint between air and solid. The tools
//! build through these functions too, so a converted feature is what that
//! kind's tool would have made. A trigger's effect is not carried over.

use bevy_egui::egui;

use crate::common::icons;
use crate::editor::block::EditorBlock;
use crate::editor::editable::FeatureTrait;
use crate::editor::editor_room::EditorRoom;
use crate::editor::footprint_block::FootprintBlock;
use crate::editor::footprint_room::FootprintRoom;
use crate::editor::trigger::EditorTrigger;
use crate::get;
use crate::tool::Tools;

/// Kinds that share a shape, each named by the tool that draws it.
const FAMILIES: &[&[Tools]] = &[&[Tools::Room, Tools::Block, Tools::Trigger], &[Tools::Footprint, Tools::FootprintBlock]];

pub fn targets(tool: &Tools) -> impl Iterator<Item = &'static Tools> + '_ {
    FAMILIES
        .iter()
        .find(|family| family.contains(tool))
        .into_iter()
        .flat_map(|family| family.iter())
        .filter(move |other| *other != tool)
}

/// A box as the feature `tool` draws; a room for any tool outside the box family.
pub fn box_as(tool: &Tools, volume: EditorRoom) -> Box<dyn FeatureTrait> {
    match tool {
        Tools::Trigger => Box::new(EditorTrigger::from_room(volume)),
        Tools::Block => Box::new(EditorBlock::from_room(volume)),
        _ => Box::new(volume),
    }
}

/// A footprint as the feature `tool` draws; air for any tool but the footprint block.
pub fn footprint_as(tool: &Tools, plan: FootprintRoom) -> Box<dyn FeatureTrait> {
    match tool {
        Tools::FootprintBlock => Box::new(FootprintBlock::from_footprint(plan)),
        _ => Box::new(plan),
    }
}

/// The inspector's last section. Returns the kind clicked.
pub fn section(ui: &mut egui::Ui, feature: &dyn FeatureTrait) -> Option<Tools> {
    let tool = feature.tool()?;
    let mut targets = targets(&tool).peekable();
    targets.peek()?;
    let mut chosen = None;
    ui.separator();
    ui.label(egui::RichText::new(get!("editor.features.transmogrify.title")).strong());
    ui.horizontal_wrapped(|ui| {
        for target in targets {
            let button = egui::Button::new((icons::atom(ui.ctx(), target.icon()), target.name())).image_tint_follows_text_color(true);
            if ui.add(button).clicked() {
                chosen = Some(target.clone());
            }
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::PointRef;

    fn a_box() -> EditorRoom {
        EditorRoom::from_point_refs(PointRef::absolute(0.0, 0.0, 0.0), PointRef::absolute(2.0, 3.0, 4.0))
    }

    #[test]
    fn each_kind_offers_the_rest_of_its_family() {
        let offered = |tool: Tools| targets(&tool).cloned().collect::<Vec<_>>();
        assert_eq!(offered(Tools::Room), [Tools::Block, Tools::Trigger]);
        assert_eq!(offered(Tools::Trigger), [Tools::Room, Tools::Block]);
        assert_eq!(offered(Tools::FootprintBlock), [Tools::Footprint]);
        assert!(offered(Tools::Stairs).is_empty());
    }

    /// A kind in a family without a `transmogrify` would show buttons that do nothing.
    #[test]
    fn every_member_of_a_family_becomes_every_other() {
        for family in FAMILIES {
            for member in *family {
                let feature = match family.contains(&Tools::Room) {
                    true => box_as(member, a_box()),
                    false => footprint_as(member, FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 2.5)),
                };
                assert_eq!(feature.tool().as_ref(), Some(member));
                for target in targets(member) {
                    let made = feature.transmogrify(target).unwrap_or_else(|| panic!("{member:?} into {target:?}"));
                    assert_eq!(made.tool().as_ref(), Some(target));
                }
            }
        }
    }

    #[test]
    fn a_box_keeps_its_corners_through_every_kind() {
        let mut feature: Box<dyn FeatureTrait> = Box::new(a_box());
        for into in [Tools::Trigger, Tools::Block, Tools::Room, Tools::Block, Tools::Trigger] {
            feature = feature.transmogrify(&into).expect("a box becomes any of its family");
            assert_eq!(feature.tool(), Some(into.clone()));
        }
        let room = feature.transmogrify(&Tools::Room).unwrap();
        assert_eq!(serde_json::to_string(&*room).unwrap(), serde_json::to_string(&a_box() as &dyn FeatureTrait).unwrap());
    }

    #[test]
    fn a_footprint_becomes_solid_and_back() {
        let plan = FootprintRoom::new(PointRef::absolute(0.0, 1.0, 0.0), 2.5);
        let original = serde_json::to_string(&plan as &dyn FeatureTrait).unwrap();
        let block = (&plan as &dyn FeatureTrait).transmogrify(&Tools::FootprintBlock).unwrap();
        assert_eq!(block.type_key(), "footprint_block");
        let back = block.transmogrify(&Tools::Footprint).unwrap();
        assert_eq!(serde_json::to_string(&*back).unwrap(), original);
    }
}
