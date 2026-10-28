//! Where a hit came from: a trapezoid just inside the window safe zone, its
//! wide edge toward the edge of the screen, at the bearing of the damage's
//! source — the muzzle a shot left, the blast, your own rocket under you.
//!
//! The source is a fixed point in the world, never a body to follow, turned
//! into a bearing every frame so the indicator swings round as we turn. Hits
//! from one spot pile onto one indicator while it is up, rather than a
//! shotgun drawing ten. Damage with no source (burning, the suicide key)
//! draws none.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::DamageDealt;
use crate::common::ui_shape::{Corner, Path, ShapeMaterial, ShapeStyle};
use crate::game::hud::WindowSafeZone;
use crate::game::player::{LocalPlayer, Player, PlayerCamera};

/// Seconds.
const FADE_IN: f32 = 0.1;
const HOLD: f32 = 0.5;
const FADE_OUT: f32 = 0.25;

/// Meters: hits from sources this close together are one indicator.
const SAME_SOURCE: f32 = 0.5;
/// Damage at which an indicator stops getting wider.
const MOST_DAMAGE: u32 = 200;
/// The wide edge, in logical pixels, at 1 damage and at [`MOST_DAMAGE`].
const NARROWEST: f32 = 60.0;
const WIDEST: f32 = 360.0;
/// The narrow edge, as a fraction of the wide one.
const TAPER: f32 = 0.6;
/// From the wide edge to the narrow one, in logical pixels.
const DEPTH: f32 = 80.0;
const CORNER: f32 = 4.0;
/// At the narrow edge; the wide edge is clear.
const INNER_OPACITY: f32 = 0.5;
const COLOR: Color = Color::srgb(1.0, 0.12, 0.08);

#[derive(Component, Debug, Clone, PartialEq)]
struct Indicator {
    /// In world space.
    from: Vec3,
    /// Everything from there since it went up.
    damage: u32,
    age: f32,
    /// Width and opacity the material was last built at.
    drawn: Option<(f32, f32)>,
}

impl Indicator {
    fn new(from: Vec3, damage: u32) -> Self {
        Self { from, damage, age: 0.0, drawn: None }
    }

    /// Back to the start of the hold, unless it is still coming in.
    fn hit_again(&mut self, from: Vec3, damage: u32) {
        self.from = from;
        self.damage += damage;
        self.age = self.age.min(FADE_IN);
    }

    /// `None` once it is over.
    fn opacity(&self) -> Option<f32> {
        let age = self.age;
        if age < FADE_IN {
            Some(age / FADE_IN)
        } else if age < FADE_IN + HOLD {
            Some(1.0)
        } else if age < FADE_IN + HOLD + FADE_OUT {
            Some(1.0 - (age - FADE_IN - HOLD) / FADE_OUT)
        } else {
            None
        }
    }

    fn width(&self) -> f32 {
        let damage = self.damage.clamp(1, MOST_DAMAGE);
        NARROWEST + (WIDEST - NARROWEST) * (damage - 1) as f32 / (MOST_DAMAGE - 1) as f32
    }
}

pub struct DamageDirectionPlugin;

impl Plugin for DamageDirectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DamageDealt>().add_systems(
            Update,
            (note_hits, place_indicators).chain().run_if(in_state(AppMode::Play)),
        );
    }
}

/// Radians clockwise from straight ahead, seen from above, of `from` from
/// `origin`; `None` when they stand in the same spot. `right` is the view's,
/// which a pitched camera keeps level where its forward is not.
fn bearing(origin: Vec3, right: Vec3, from: Vec3) -> Option<f32> {
    let right = Vec3::new(right.x, 0.0, right.z).try_normalize()?;
    let forward = Vec3::Y.cross(right);
    let toward = Vec3::new(from.x - origin.x, 0.0, from.z - origin.z);
    if toward.length_squared() < 1e-6 {
        return None;
    }
    Some(toward.dot(right).atan2(toward.dot(forward)))
}

/// The indicator's center from the zone's, in UI pixels (`+y` down), for a
/// zone `half` its size each way: its wide edge where a ray from the middle
/// at `bearing` meets the zone's edge.
fn placement(bearing: f32, half: Vec2) -> Vec2 {
    let direction = Vec2::new(bearing.sin(), -bearing.cos());
    let reach = |half: f32, along: f32| if along.abs() < 1e-6 { f32::INFINITY } else { half / along.abs() };
    let edge = direction * reach(half.x, direction.x).min(reach(half.y, direction.y));
    edge - direction * DEPTH * 0.5
}

/// Wide edge on top, so turning it by its bearing points that edge outward.
fn trapezoid(width: f32, opacity: f32) -> ShapeMaterial {
    let inset = (1.0 - TAPER) * 0.5 * width;
    let outline = Path::rounded_polygon(
        &[
            Vec2::ZERO,
            Vec2::new(width, 0.0),
            Vec2::new(width - inset, DEPTH),
            Vec2::new(inset, DEPTH),
        ],
        Corner::round(CORNER),
    );
    let style = ShapeStyle { fill: COLOR.with_alpha(opacity), fade: [0.0, INNER_OPACITY], ..default() };
    ShapeMaterial::from_path(&outline, Vec2::new(width, DEPTH), style)
}

#[allow(clippy::type_complexity)]
fn note_hits(
    mut commands: Commands,
    mut dealt: MessageReader<DamageDealt>,
    us: Option<Single<Entity, (With<LocalPlayer>, With<Player>)>>,
    mut indicators: Query<&mut Indicator>,
    zone: Option<Single<Entity, With<WindowSafeZone>>>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let hits: Vec<DamageDealt> = dealt.read().copied().collect();
    let (Some(us), Some(zone), Some(mut materials)) = (us, zone, materials) else { return };
    for hit in hits {
        let Some(from) = hit.from else { continue };
        if hit.target != *us || hit.amount == 0 {
            continue;
        }
        match indicators.iter_mut().find(|indicator| indicator.from.distance(from) < SAME_SOURCE) {
            Some(mut indicator) => indicator.hit_again(from, hit.amount),
            None => {
                let indicator = Indicator::new(from, hit.amount);
                let material = materials.add(trapezoid(indicator.width(), 0.0));
                let spawned = commands
                    .spawn((
                        indicator,
                        Node { position_type: PositionType::Absolute, ..default() },
                        MaterialNode(material),
                        UiTransform::IDENTITY,
                        Pickable::IGNORE,
                    ))
                    .id();
                commands.entity(*zone).add_child(spawned);
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn place_indicators(
    mut commands: Commands,
    time: Res<Time>,
    zone: Option<Single<&ComputedNode, With<WindowSafeZone>>>,
    camera: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
    body: Option<Single<&GlobalTransform, (With<LocalPlayer>, With<Player>)>>,
    mut indicators: Query<(
        Entity,
        &mut Indicator,
        &mut Node,
        &mut UiTransform,
        &MaterialNode<ShapeMaterial>,
    )>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let (Some(zone), Some(camera), Some(mut materials)) = (zone, camera, materials) else { return };
    let half = zone.size() * zone.inverse_scale_factor() * 0.5;
    let origin = body.map_or(camera.translation(), |body| body.translation());
    let right = camera.right().as_vec3();

    for (entity, mut indicator, mut node, mut transform, material) in &mut indicators {
        indicator.age += time.delta_secs();
        let Some(opacity) = indicator.opacity() else {
            commands.entity(entity).despawn();
            continue;
        };
        let width = indicator.width();
        if indicator.drawn != Some((width, opacity))
            && let Some(mut drawn) = materials.get_mut(&material.0)
        {
            *drawn = trapezoid(width, opacity);
            indicator.drawn = Some((width, opacity));
        }

        // Straight ahead from right underfoot.
        let angle = bearing(origin, right, indicator.from).unwrap_or(0.0);
        let center = half + placement(angle, half);
        node.left = Val::Px(center.x - width * 0.5);
        node.top = Val::Px(center.y - DEPTH * 0.5);
        node.width = Val::Px(width);
        node.height = Val::Px(DEPTH);
        transform.set_if_neq(UiTransform::from_rotation(Rot2::radians(angle)));
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;

    #[test]
    fn it_comes_in_holds_and_goes() {
        let mut indicator = Indicator::new(Vec3::ZERO, 10);
        let at = |indicator: &mut Indicator, age: f32| {
            indicator.age = age;
            indicator.opacity()
        };
        assert_eq!(at(&mut indicator, 0.05), Some(0.5));
        assert_eq!(at(&mut indicator, 0.3), Some(1.0));
        assert!((at(&mut indicator, FADE_IN + HOLD + FADE_OUT * 0.5).unwrap() - 0.5).abs() < 1e-5);
        assert_eq!(at(&mut indicator, 1.0), None);
    }

    #[test]
    fn another_hit_holds_it_again_and_widens_it() {
        let mut indicator = Indicator::new(Vec3::ZERO, 1);
        assert_eq!(indicator.width(), NARROWEST);
        indicator.age = FADE_IN + HOLD + 0.1;
        indicator.hit_again(Vec3::X, 500);
        assert_eq!(indicator.age, FADE_IN);
        assert_eq!(indicator.width(), WIDEST);
        assert_eq!(indicator.from, Vec3::X);

        // Still coming in: it carries on coming in.
        let mut fresh = Indicator::new(Vec3::ZERO, 1);
        fresh.age = 0.02;
        fresh.hit_again(Vec3::ZERO, 1);
        assert_eq!(fresh.age, 0.02);
    }

    /// An unturned camera looks down `-Z` with `+X` on its right.
    #[test]
    fn the_bearing_is_clockwise_from_ahead() {
        let close = |a: Option<f32>, b: f32| (a.unwrap() - b).abs() < 1e-5;
        assert!(close(bearing(Vec3::ZERO, Vec3::X, Vec3::NEG_Z), 0.0));
        assert!(close(bearing(Vec3::ZERO, Vec3::X, Vec3::X * 5.0), FRAC_PI_2));
        assert!(close(bearing(Vec3::ZERO, Vec3::X, Vec3::NEG_X), -FRAC_PI_2));
        // Turned to face +X, somebody at +X is ahead.
        assert!(close(bearing(Vec3::ZERO, Vec3::Z, Vec3::X), 0.0));
        // Height does not matter.
        assert!(close(bearing(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 9.0, -1.0)), 0.0));
        assert_eq!(bearing(Vec3::ZERO, Vec3::X, Vec3::Y), None);
    }

    #[test]
    fn it_sits_inside_the_edge_it_points_at() {
        let half = Vec2::new(400.0, 300.0);
        let ahead = placement(0.0, half);
        assert!((ahead - Vec2::new(0.0, -300.0 + DEPTH * 0.5)).length() < 1e-3);
        let right = placement(FRAC_PI_2, half);
        assert!((right - Vec2::new(400.0 - DEPTH * 0.5, 0.0)).length() < 1e-3);
        let behind = placement(std::f32::consts::PI, half);
        assert!((behind - Vec2::new(0.0, 300.0 - DEPTH * 0.5)).length() < 1e-3);
    }
}
