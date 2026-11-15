//! A taunt's `view`: the third-person camera swings to a set angle from the body as the taunt
//! starts, and back as it ends.
//!
//! The swing is an offset on the camera alone, which `place_camera` adds to the aim. While the
//! taunt plays, each frame's mouse motion is moved out of the aim into the offset before the fixed
//! loop sends it. The aim the body follows therefore never changes, so a body freed as the taunt
//! ends, here or on the server a round trip earlier, faces where it started. Local.

use bevy::prelude::*;

use crate::common::rotation::wrap_angle;
use crate::common::taunt::Taunts;
use crate::game::condition::Condition;
use crate::game::player::{InputLatch, LocalPlayer, Player};

/// Seconds each swing takes.
pub const SWING_SECONDS: f32 = 0.6;

/// Written only by [`swing_for_taunts`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct TauntView {
    /// The aim when a taunt with a view started, held while it plays.
    before: Option<Vec2>,
    /// Radians of yaw and pitch: the offset now, and the swing it is on.
    offset: Vec2,
    from: Vec2,
    to: Vec2,
    along: f32,
}

impl TauntView {
    pub fn offset(&self) -> Vec2 {
        self.offset
    }

    fn swing_to(&mut self, to: Vec2) {
        self.from = self.offset;
        self.to = to;
        self.along = 0.0;
    }

    /// One frame. `view` is in radians, measured from `body_yaw`.
    pub fn step(&mut self, aim: &mut Vec2, body_yaw: f32, taunting: bool, view: Option<f32>, seconds: f32) {
        if let (Some(view), None) = (view.filter(|_| taunting), self.before) {
            self.before = Some(*aim);
            self.swing_to(Vec2::new(self.offset.x + wrap_angle(body_yaw + view - aim.x - self.offset.x), 0.0));
        }
        if let Some(before) = self.before {
            let moved = *aim - before;
            *aim = before;
            self.offset += moved;
            self.from += moved;
            self.to += moved;
            if !taunting {
                self.before = None;
                self.offset.x = wrap_angle(self.offset.x);
                self.swing_to(Vec2::ZERO);
            }
        }
        self.along = (self.along + seconds / SWING_SECONDS).min(1.0);
        let eased = self.along * self.along * (3.0 - 2.0 * self.along);
        self.offset = self.from.lerp(self.to, eased);
    }
}

/// After `gather_aim` and before the fixed loop sends the aim; `place_camera` adds the offset.
pub fn swing_for_taunts(
    time: Res<Time>,
    taunts: Option<Res<Taunts>>,
    ours: Option<Single<(&Player, &Condition), With<LocalPlayer>>>,
    mut latch: ResMut<InputLatch>,
    mut view: ResMut<TauntView>,
) {
    // With no body the swing eases out, so dying mid-taunt does not cut the view.
    let (body_yaw, taunt) = ours.map_or((0.0, None), |ours| (ours.0.yaw, ours.1.taunt));
    let angle = taunt.and_then(|id| taunts.as_deref()?.get(id)?.view).map(f32::to_radians);
    let mut aim = Vec2::new(latch.0.yaw, latch.0.pitch);
    let was = aim;
    view.step(&mut aim, body_yaw, taunt.is_some(), angle, time.delta_secs());
    if aim != was {
        latch.0.yaw = aim.x;
        latch.0.pitch = aim.y;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn run(view: &mut TauntView, aim: &mut Vec2, taunting: bool, angle: Option<f32>, seconds: f32) {
        for _ in 0..(seconds / FRAME).round() as usize {
            view.step(aim, 0.3, taunting, angle, FRAME);
        }
    }

    #[test]
    fn the_camera_swings_to_the_angle_from_the_body_and_back() {
        let mut view = TauntView::default();
        let mut aim = Vec2::new(0.3, -0.2);
        run(&mut view, &mut aim, true, Some(PI), SWING_SECONDS + 0.1);
        assert!((aim + view.offset() - Vec2::new(0.3 + PI, -0.2)).length() < 1e-4, "the camera points {:?}", aim + view.offset());
        assert_eq!(aim, Vec2::new(0.3, -0.2), "the swing moved the aim the body follows");

        run(&mut view, &mut aim, false, None, SWING_SECONDS + 0.1);
        assert_eq!(view.offset(), Vec2::ZERO);
        assert_eq!(aim, Vec2::new(0.3, -0.2));
    }

    /// A moved aim would turn the body as the taunt ends.
    #[test]
    fn the_mouse_turns_the_view_and_not_the_aim() {
        let mut view = TauntView::default();
        let mut aim = Vec2::new(0.3, 0.0);
        run(&mut view, &mut aim, true, Some(FRAC_PI_2), SWING_SECONDS / 2.0);
        aim += Vec2::new(0.5, 0.2);
        view.step(&mut aim, 0.3, true, Some(FRAC_PI_2), 0.0);
        assert_eq!(aim, Vec2::new(0.3, 0.0), "the mouse moved the aim mid-taunt");
        run(&mut view, &mut aim, true, Some(FRAC_PI_2), SWING_SECONDS);
        let camera = aim + view.offset();
        assert!((camera - Vec2::new(0.3 + FRAC_PI_2 + 0.5, 0.2)).length() < 1e-4, "the camera points {camera:?}");

        view.step(&mut aim, 0.3, false, None, 0.0);
        assert_eq!(aim, Vec2::new(0.3, 0.0));
        assert!((aim + view.offset() - camera).length() < 1e-4, "the view cut rather than swinging back");
        run(&mut view, &mut aim, false, None, SWING_SECONDS);
        assert_eq!(view.offset(), Vec2::ZERO);
    }

    #[test]
    fn a_taunt_with_no_view_leaves_the_camera_alone() {
        let mut view = TauntView::default();
        let mut aim = Vec2::new(1.0, 0.1);
        run(&mut view, &mut aim, true, None, 1.0);
        aim.x += 0.4;
        run(&mut view, &mut aim, false, None, 0.1);
        assert_eq!(view.offset(), Vec2::ZERO);
        assert_eq!(aim, Vec2::new(1.4, 0.1), "a taunt with no view put the aim back");
    }

    #[test]
    fn the_swing_goes_the_short_way_round() {
        let mut view = TauntView::default();
        let mut aim = Vec2::new(0.3 + 3.0 * PI, 0.0);
        view.step(&mut aim, 0.3, true, Some(FRAC_PI_2), 0.0);
        assert!((view.to.x + FRAC_PI_2).abs() < 1e-4, "swung {} radians rather than a quarter turn back", view.to.x);
    }
}
