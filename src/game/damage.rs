//! What a hit leads to: a red number where it landed, and — once there is no
//! health left — the body going away. The numbers are here; the reaping is in
//! [`crate::game::death`], wired up from this plugin because this is where
//! [`DamageDealt`] is registered and where the ordering against it belongs.
//!
//! Reads [`DamageDealt`] rather than being called by the thing that shot, so
//! the day a rocket, a fall or a raised floor deals damage they get numbers
//! without knowing this module exists.
//!
//! The number is anchored to a **world point** and projected every frame, not
//! pinned to a screen position when it was born. A number frozen in screen
//! space slides off the body that took the hit the moment the camera moves,
//! which is exactly when you are looking at it.
//!
//! Everything here works in **logical points**: that is what egui lays out in,
//! and it is also what `world_to_viewport` gives back, since it maps onto the
//! camera's logical viewport rect. There is no scale factor to apply, and
//! applying one anyway is invisible until somebody runs it on a retina display.
//!
//! It is drawn at a fixed size in logical points, so a hit reads the same
//! whether it landed on a body across the room or one in your face — distance
//! is what the body behind it is for. The drift upwards is screen-space for
//! the same reason: a fifth of the screen is a fifth of the screen at any
//! range, where a metre of world rise would be invisible at fifty of them.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::damage::{DamageDealt, Died};
use crate::game::death::{announce_deaths, reap_the_dead, record_damage};
use crate::game::hitscan::fire_hitscan;
use crate::game::player::PlayerCamera;

/// How long a damage number lasts, in seconds.
pub const DAMAGE_NUMBER_LIFETIME: f32 = 2.0;

/// How big the number is drawn, in logical points.
const DAMAGE_NUMBER_SIZE: f32 = 48.0;

/// How far a number climbs over its whole life, as a fraction of the window
/// height. It travels at a constant speed and expires at the top of that
/// climb, so lifetime and this together are the velocity.
const DAMAGE_NUMBER_RISE: f32 = 0.2;

/// A number floating at the place a hit landed.
#[derive(Component, Debug)]
pub struct DamageNumber {
    pub amount: u32,
    /// Where the hit was, in world space.
    pub at: Vec3,
    /// Seconds of life left.
    pub remaining: f32,
}

pub struct DamagePlugin;

impl Plugin for DamagePlugin {
    fn build(&self, app: &mut App) {
        app
            // Registered here, by the reader, because this is the module that
            // outlives any one thing that writes it.
            .add_message::<DamageDealt>()
            .add_message::<Died>()
            // On the tick, in this order, in the same tick as the shot. A
            // body reaped before its log is written is a kill credited to
            // nobody — see [`crate::game::death`].
            .add_systems(
                FixedUpdate,
                (record_damage, reap_the_dead)
                    .chain()
                    .after(fire_hitscan)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(Update, (spawn_damage_numbers, fade_damage_numbers, announce_deaths).chain())
            .add_systems(
                EguiPrimaryContextPass,
                draw_damage_numbers.run_if(in_state(AppMode::Play)),
            )
            // Numbers belong to the match, not to the editor: F5 out and the
            // last shot's feedback should not still be hanging in a viewport.
            .add_systems(OnExit(AppMode::Play), clear_damage_numbers)
        ;
    }
}

fn spawn_damage_numbers(mut commands: Commands, mut hits: MessageReader<DamageDealt>) {
    for hit in hits.read() {
        commands.spawn((
            DamageNumber {
                amount: hit.amount,
                at: hit.point,
                remaining: DAMAGE_NUMBER_LIFETIME,
            },
            Name::new("Damage number"),
        ));
    }
}

fn fade_damage_numbers(
    mut commands: Commands,
    time: Res<Time>,
    mut numbers: Query<(Entity, &mut DamageNumber)>,
) {
    for (entity, mut number) in &mut numbers {
        number.remaining -= time.delta_secs();
        if number.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn clear_damage_numbers(mut commands: Commands, numbers: Query<Entity, With<DamageNumber>>) {
    for number in &numbers {
        commands.entity(number).despawn();
    }
}

/// How opaque a number is, given how much of its life is left.
///
/// Linear, and its own function so that a curve — a hold and then a fall, a
/// drift upwards — is a change here rather than a change threaded through the
/// drawing.
fn opacity(remaining: f32) -> f32 {
    (remaining / DAMAGE_NUMBER_LIFETIME).clamp(0.0, 1.0)
}

/// How far above its hit point a number has climbed, in logical points, given
/// how much of its life is left and how tall the window is.
///
/// Linear in age: zero the frame it appears, the full rise the frame it goes.
fn rise(remaining: f32, window_height: f32) -> f32 {
    let age = (1.0 - remaining / DAMAGE_NUMBER_LIFETIME).clamp(0.0, 1.0);
    age * DAMAGE_NUMBER_RISE * window_height
}

fn draw_damage_numbers(
    mut contexts: EguiContexts,
    numbers: Query<&DamageNumber>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    if numbers.is_empty() {
        return;
    }
    let Ok((camera, view)) = camera.single() else { return };
    let Ok(window) = window.single() else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // `world_to_viewport` maps onto the camera's *logical* viewport rect, which
    // is the same space egui lays out in — so the projection is already in
    // points and must not be divided by the scale factor again. Doing that put
    // every number in the top-left quadrant of a retina display.
    let height = window.height();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("damage_numbers"),
    ));

    for number in &numbers {
        // Behind the camera, or outside the viewport, and there is nowhere to
        // draw it — a hit you have already turned away from.
        let Ok(screen) = camera.world_to_viewport(view, number.at) else { continue };

        let alpha = (opacity(number.remaining) * 255.0) as u8;
        painter.text(
            egui::pos2(screen.x, screen.y - rise(number.remaining, height)),
            egui::Align2::CENTER_CENTER,
            number.amount.to_string(),
            egui::FontId::proportional(DAMAGE_NUMBER_SIZE),
            egui::Color32::from_rgba_unmultiplied(255, 40, 40, alpha),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::{DamageSource, PlayerId};

    fn a_hit(amount: u32, point: Vec3) -> DamageDealt {
        DamageDealt {
            target: Entity::from_raw_u32(1).unwrap(),
            source: DamageSource::Player(PlayerId(0)),
            amount,
            remaining: 70,
            point,
        }
    }

    /// A hit puts a number where it landed, in the world rather than on the
    /// screen — the projection happens at drawing time, every frame.
    #[test]
    fn a_hit_leaves_a_number_at_the_hit_point() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageDealt>>();
        let point = Vec3::new(1.0, 2.0, 3.0);
        world.write_message(a_hit(30, point));

        world.run_system_once(spawn_damage_numbers).unwrap();

        let number = world.query::<&DamageNumber>().single(&world).unwrap();
        assert_eq!(number.amount, 30);
        assert_eq!(number.at, point);
        assert_eq!(number.remaining, DAMAGE_NUMBER_LIFETIME);
    }

    /// And it is gone by the time its two seconds are up, rather than sitting
    /// invisible in the world forever.
    #[test]
    fn a_number_is_despawned_once_it_has_faded() {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.spawn(DamageNumber { amount: 30, at: Vec3::ZERO, remaining: 0.05 });

        // Two ticks of a 60 Hz frame is more than the sliver left.
        for _ in 0..2 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_millis(33));
            world.run_system_once(fade_damage_numbers).unwrap();
        }

        assert_eq!(world.query::<&DamageNumber>().iter(&world).count(), 0);
    }

    /// Full when it appears, gone when it expires.
    #[test]
    fn a_number_fades_over_its_lifetime() {
        assert_eq!(opacity(DAMAGE_NUMBER_LIFETIME), 1.0);
        assert_eq!(opacity(0.0), 0.0);
        assert!(opacity(DAMAGE_NUMBER_LIFETIME * 0.5) < opacity(DAMAGE_NUMBER_LIFETIME));
    }

    /// It starts at the hit point and ends a fifth of the screen above it,
    /// having covered the ground at a constant speed rather than easing.
    #[test]
    fn a_number_climbs_a_fifth_of_the_screen_at_a_constant_speed() {
        let height = 1000.0;
        assert_eq!(rise(DAMAGE_NUMBER_LIFETIME, height), 0.0);
        assert!((rise(0.0, height) - 200.0).abs() < 1e-3);

        // Constant speed: equal slices of life are equal distances.
        let quarter = DAMAGE_NUMBER_LIFETIME * 0.25;
        let first = rise(DAMAGE_NUMBER_LIFETIME - quarter, height) - rise(DAMAGE_NUMBER_LIFETIME, height);
        let last = rise(0.0, height) - rise(quarter, height);
        assert!((first - last).abs() < 1e-3, "{first} != {last}");
    }
}
