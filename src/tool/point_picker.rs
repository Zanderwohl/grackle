//! Picking one point the way every placing tool does: a click at the cursor, or Shift to show the
//! points that already exist, a click on one, and a second click for the offset from it (right
//! click for none). Escape backs out of a relative pick, and otherwise reports [`Picked::Canceled`]
//! for the tool to decide what it drops.

use bevy::prelude::*;

use crate::common::keybind::{Action, Actions};
use crate::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::tool::tool_helpers::*;

#[derive(Clone, Default)]
enum Mode {
    #[default]
    Normal,
    Picking,
    Relative { feature: FeatureId, key: String, reference: Vec3 },
}

#[derive(Default)]
pub struct PointPicker {
    mode: Mode,
    hovered: Option<(FeatureId, String, Vec3)>,
    /// What Shift interrupted, put back if the Shift was held to fly faster.
    before_picking: Option<Mode>,
    flew: bool,
}

pub enum Picked {
    Nothing,
    /// Escape with no relative pick under way.
    Canceled,
    Absolute(Vec3),
    Relative(PointRef, Vec3),
}

impl PointPicker {
    /// A frame of input. `place` moves where a pick lands (the stairs keep their height point off
    /// the footprint), or refuses it with `None`, which keeps a relative pick's reference;
    /// `snap` is the grid a hovered point is snapped to.
    pub fn update(
        &mut self,
        mouse: &CurrentMouseInput,
        actions: &Actions,
        features: &FeatureTimeline,
        cursor: Option<Vec3>,
        snap: Option<f32>,
        place: impl Fn(Vec3) -> Option<Vec3>,
    ) -> Picked {
        let shift_held = actions.pressed(Action::Modify);
        match self.mode.clone() {
            Mode::Normal => {
                if actions.just_pressed(Action::Cancel) {
                    return Picked::Canceled;
                }
                if shift_held {
                    self.mode = Mode::Picking;
                    self.hovered = None;
                    self.before_picking = None;
                    self.flew = false;
                } else if let Some(at) = cursor.filter(|_| commit_at_cursor(mouse, actions)).and_then(&place) {
                    return Picked::Absolute(at);
                }
            }
            Mode::Picking => {
                self.flew |= camera_dragging(mouse);
                if !shift_held {
                    self.mode = self.before_picking.take().filter(|_| self.flew).unwrap_or_default();
                    self.hovered = None;
                    return Picked::Nothing;
                }
                self.hovered = mouse
                    .world_pos
                    .and_then(|ray| find_hovered_point(&ray, features, PICK_RADIUS))
                    .map(|(id, key, at)| (id, key, snap.map_or(at, |granularity| snap_vec3(at, granularity))));
                if mouse.released == Some(MouseButton::Left)
                    && let Some((feature, key, reference)) = self.hovered.take()
                {
                    self.mode = Mode::Relative { feature, key, reference };
                }
            }
            Mode::Relative { feature, key, reference } => {
                // Kept until Shift comes up, in case it was held to fly faster.
                if actions.just_pressed(Action::Modify) {
                    self.before_picking = Some(self.mode.clone());
                    self.mode = Mode::Picking;
                    self.flew = false;
                    self.hovered = None;
                    return Picked::Nothing;
                }
                if actions.just_pressed(Action::Cancel) {
                    self.mode = Mode::Normal;
                    return Picked::Nothing;
                }
                // A right click, not the end of a camera turn, is an offset of zero.
                let target = if mouse.in_camera.is_some() && mouse.clicked(MouseButton::Right) {
                    Some(reference)
                } else if commit_at_cursor(mouse, actions) {
                    cursor
                } else {
                    None
                };
                if let Some(at) = target.and_then(&place) {
                    self.mode = Mode::Normal;
                    let offset = at - reference;
                    let mut point = PointRef::reference_with_offset(feature, offset.x, offset.y, offset.z);
                    point.point_key = key;
                    return Picked::Relative(point, at);
                }
            }
        }
        Picked::Nothing
    }

    pub fn reset(&mut self) {
        *self = PointPicker::default();
    }

    /// `Some` only while a relative pick is under way.
    pub fn reference(&self) -> Option<Vec3> {
        match &self.mode {
            Mode::Relative { reference, .. } => Some(*reference),
            _ => None,
        }
    }

    pub fn is_picking(&self) -> bool {
        matches!(self.mode, Mode::Picking)
    }

    /// While Shift is held, the existing points; during a relative pick, the path to `cursor`.
    pub fn draw(&self, gizmos: &mut Gizmos, mouse: &CurrentMouseInput, features: &FeatureTimeline, cursor: Option<Vec3>) {
        if let (Some(reference), Some(cursor)) = (self.reference(), cursor) {
            draw_taxicab_path(gizmos, reference, cursor);
        }
        if self.is_picking()
            && let Some(ray) = mouse.world_pos
        {
            draw_picking_gizmos(gizmos, &ray, features, &self.hovered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::global_point::GlobalPoint;

    /// A picker partway through a relative pick from a point at (1, 2, 3).
    fn relative() -> (PointPicker, FeatureTimeline) {
        let mut features = FeatureTimeline::default();
        let feature = features.apply_feature(Box::new(GlobalPoint::new(1.0, 2.0, 3.0)));
        let picker = PointPicker { mode: Mode::Relative { feature, key: String::new(), reference: Vec3::new(1.0, 2.0, 3.0) }, ..default() };
        (picker, features)
    }

    fn escape() -> Actions {
        let mut actions = Actions::default();
        actions.press(Action::Cancel);
        actions
    }

    #[test]
    fn escape_backs_out_of_a_relative_pick_and_then_cancels() {
        let (mut picker, features) = relative();
        let mouse = CurrentMouseInput::default();
        assert!(matches!(picker.update(&mouse, &escape(), &features, Some(Vec3::ZERO), None, Some), Picked::Nothing));
        assert!(picker.reference().is_none());
        assert!(matches!(picker.update(&mouse, &escape(), &features, Some(Vec3::ZERO), None, Some), Picked::Canceled));
    }

    #[test]
    fn a_right_click_is_an_offset_of_zero() {
        let (mut picker, features) = relative();
        let mouse = CurrentMouseInput { released: Some(MouseButton::Right), in_camera: Some(Entity::PLACEHOLDER), ..default() };
        let Picked::Relative(point, at) = picker.update(&mouse, &Actions::default(), &features, Some(Vec3::ZERO), None, Some) else {
            panic!("the right click picked nothing");
        };
        assert_eq!(at, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!([point.x.value(), point.y.value(), point.z.value()], [0.0; 3]);
    }

    #[test]
    fn a_refused_pick_keeps_its_reference() {
        let (mut picker, features) = relative();
        let mouse = CurrentMouseInput { released: Some(MouseButton::Left), in_camera: Some(Entity::PLACEHOLDER), ..default() };
        assert!(matches!(picker.update(&mouse, &Actions::default(), &features, Some(Vec3::ZERO), None, |_| None), Picked::Nothing));
        assert!(picker.reference().is_some());
    }
}
