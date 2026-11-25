//! Flames up the sides of the screen while the body we see out of is
//! [`OnFire`]: 2D, in the HUD, from the flamethrower's own sheet of shapes.
//!
//! Two kinds of flame start just below the window's bottom edge and rise:
//!
//! - **Side flames**, in the strip left or right of the HUD's safe zone. Those
//!   at the window's edge climb [`TALL`] of the window's height and those at the
//!   safe zone's edge [`SHORT`], so their outline is a V open at the top. On a
//!   window 4:3 or narrower there is no strip outside the safe zone, so each
//!   strip is at least [`LEAST_STRIP`] window heights wide.
//! - **Front flames**, bigger and fainter, along the whole bottom and past both
//!   ends of it ([`FRONT_WIDTH`]), drawn behind the side flames.
//!
//! The safe zone pushes every flame inside it toward its nearer side edge
//! ([`REPEL`]), hardest at its middle, so the crosshair stays clear.
//!
//! A fixed pool of UI nodes, stood up with the HUD and hidden while idle, each
//! placed, sized and turned by its `UiTransform` alone, so a flame never costs
//! a layout pass. Drawn under the rest of the HUD. Drawing only; nothing is sent.

use bevy::prelude::*;
use rand::Rng;

use crate::common::app_mode::AppMode;
use crate::common::flame::OnFire;
use crate::game::flame_jet::{FlameSheet, FLAME_GRID, FLAME_TEXELS};
use crate::game::player::{Player, Viewed};

/// Side flames per second, both sides together.
const RATE: f32 = 90.0;

/// Seconds a flame lives, before [`SIDE_LIFE`].
const LIFE: (f32, f32) = (0.6, 1.1);

/// Side flames climb this many times faster than [`TALL`] and [`SHORT`] over
/// [`LIFE`] imply and live this share of [`LIFE`], so climb their product higher.
const SIDE_SPEED: f32 = 4.0;
const SIDE_LIFE: f32 = 0.5;

/// Front flames per second.
const FRONT_RATE: f32 = 40.0;

/// Width front flames start across, in window widths, centered.
const FRONT_WIDTH: f32 = 1.3;

/// Height a front flame climbs to, in window heights, before the safe zone
/// pushes it aside.
const FRONT_CLIMB: f32 = 0.3;

/// A front flame's size and opacity, as multiples of a side flame's.
const FRONT_SIZE: f32 = 1.6;
const FRONT_OPACITY: f32 = 0.45;

/// Sideways acceleration at the safe zone's middle, in window heights per
/// second squared, falling to nothing at its side edges.
const REPEL: f32 = 3.0;

/// Height a side flame climbs to, in window heights, at the window's edge and at
/// the safe zone's edge, before [`SIDE_SPEED`] and [`SIDE_LIFE`].
const TALL: f32 = 0.55;
const SHORT: f32 = 0.12;

/// Width of each strip at least, in window heights.
const LEAST_STRIP: f32 = 0.12;

/// Width of a flame as it starts, in window heights, and how many times that it
/// is at the end of its life.
const START_SIZE: f32 = 0.09;
const GROWTH: f32 = 1.8;

/// Per flame, a multiplier on [`START_SIZE`].
const SIZE_SHARE: (f32, f32) = (0.7, 1.3);

/// Sideways speed toward the window's edge, in window heights per second, at
/// most. Leans the flames outward, along the V.
const OUTWARD: f32 = 0.08;

/// Window heights below the bottom edge a flame's middle starts, so no flame
/// shows its base.
const BELOW: f32 = 0.03;

/// Degrees a flame leans from upright, either way, at most.
const LEAN: f32 = 12.0;

/// Shapes per second a flame steps through the sheet.
const SHAPE_RATE: f32 = 9.0;

/// Shares of its life a flame takes to fade in, and starts fading out at.
const FADE_IN: f32 = 0.1;
const FADE_OUT_FROM: f32 = 0.45;

const OPACITY: f32 = 0.85;

/// Logical pixels: every flame's node, scaled to the flame's size.
const NODE_SIZE: f32 = 100.0;

/// Nodes for each kind, enough for every flame [`RATE`] and [`FRONT_RATE`]
/// keep alive at once; past it a flame is not started.
const SIDE_SLOTS: usize = (RATE * LIFE.1 * SIDE_LIFE * 1.2) as usize + 1;
const FRONT_SLOTS: usize = (FRONT_RATE * LIFE.1 * 1.2) as usize + 1;

/// Under every other HUD root, which sit at 0.
const LAYER: i32 = -1;

/// Under the side flames, within [`LAYER`].
const FRONT_LAYER: i32 = -1;

#[derive(Resource)]
struct HudFireLayout(Handle<TextureAtlasLayout>);

#[derive(Component)]
struct FlameSlot {
    front: bool,
    flame: Option<HudFlame>,
}

#[derive(Component)]
struct HudFireRoot;

#[derive(Component, Debug, Clone, PartialEq)]
struct HudFlame {
    /// Logical pixels from the window's top left, where its middle is now.
    at: Vec2,
    /// Logical pixels per second.
    velocity: Vec2,
    age: f32,
    life: f32,
    /// Logical pixels; the node's side.
    size: f32,
    lean: f32,
    first_shape: usize,
    /// Once faded in.
    opacity: f32,
}

impl HudFlame {
    fn share(&self) -> f32 {
        (self.age / self.life).clamp(0.0, 1.0)
    }

    fn scale(&self) -> f32 {
        1.0 + (GROWTH - 1.0) * self.share()
    }

    fn opacity(&self) -> f32 {
        let share = self.share();
        let fade_in = (share / FADE_IN).min(1.0);
        let fade_out = 1.0 - ((share - FADE_OUT_FROM) / (1.0 - FADE_OUT_FROM)).clamp(0.0, 1.0);
        self.opacity * fade_in * fade_out
    }

    fn shape(&self) -> usize {
        let shapes = (FLAME_GRID.x * FLAME_GRID.y) as usize;
        (self.first_shape + (self.age * SHAPE_RATE) as usize) % shapes
    }
}

/// Logical pixels from the window's left: the outer and inner edge of the left
/// strip. The right strip is its mirror.
fn left_strip(window: Vec2) -> (f32, f32) {
    let outside = window.x * 0.5 - safe_half(window);
    (0.0, outside.max(window.y * LEAST_STRIP).min(window.x * 0.5))
}

/// Logical pixels.
fn safe_half(window: Vec2) -> f32 {
    (window.y * 4.0 / 3.0).min(window.x) * 0.5
}

/// Logical pixels per second squared along x, away from the safe zone's middle,
/// for a flame `x` from the window's left.
fn repel(window: Vec2, x: f32) -> f32 {
    let half = safe_half(window);
    let off = x - window.x * 0.5;
    let inside = (1.0 - off.abs() / half).max(0.0);
    // Dead center goes left; either way would do.
    let away = if off > 0.0 { 1.0 } else { -1.0 };
    away * REPEL * window.y * inside
}

/// `out` is 0 at the safe zone's edge and 1 at the window's.
fn flame_at(window: Vec2, out: f32, left: bool, rng: &mut impl Rng) -> HudFlame {
    let (_, inner) = left_strip(window);
    let from_edge = inner * (1.0 - out);
    let x = if left { from_edge } else { window.x - from_edge };
    let outward = if left { -1.0 } else { 1.0 };
    let base_life = rng.random_range(LIFE.0..LIFE.1);
    let life = base_life * SIDE_LIFE;
    let climb = (SHORT + (TALL - SHORT) * out) * window.y;
    HudFlame {
        at: Vec2::new(x, window.y * (1.0 + BELOW)),
        velocity: Vec2::new(outward * OUTWARD * window.y * rng.random_range(0.0..1.0), -climb * SIDE_SPEED / base_life),
        age: 0.0,
        life,
        size: START_SIZE * window.y * rng.random_range(SIZE_SHARE.0..SIZE_SHARE.1),
        lean: rng.random_range(-LEAN..LEAN),
        first_shape: rng.random_range(0..(FLAME_GRID.x * FLAME_GRID.y) as usize),
        opacity: OPACITY,
    }
}

/// `across` runs 0 to 1 over [`FRONT_WIDTH`], left to right.
fn front_flame_at(window: Vec2, across: f32, rng: &mut impl Rng) -> HudFlame {
    let x = window.x * (0.5 + (across - 0.5) * FRONT_WIDTH);
    let life = rng.random_range(LIFE.0..LIFE.1);
    HudFlame {
        at: Vec2::new(x, window.y * (1.0 + BELOW)),
        velocity: Vec2::new(0.0, -FRONT_CLIMB * window.y / life),
        age: 0.0,
        life,
        size: FRONT_SIZE * START_SIZE * window.y * rng.random_range(SIZE_SHARE.0..SIZE_SHARE.1),
        lean: rng.random_range(-LEAN..LEAN),
        first_shape: rng.random_range(0..(FLAME_GRID.x * FLAME_GRID.y) as usize),
        opacity: OPACITY * FRONT_OPACITY,
    }
}

fn make_the_look(mut commands: Commands, mut layouts: ResMut<Assets<TextureAtlasLayout>>) {
    let layout = TextureAtlasLayout::from_grid(UVec2::splat(FLAME_TEXELS), FLAME_GRID.x, FLAME_GRID.y, None, None);
    commands.insert_resource(HudFireLayout(layouts.add(layout)));
}

/// With its whole pool, once the flamethrower's sheet exists.
fn spawn_the_root(
    mut commands: Commands,
    existing: Query<(), With<HudFireRoot>>,
    sheet: Option<Res<FlameSheet>>,
    layout: Option<Res<HudFireLayout>>,
) {
    let (Some(sheet), Some(layout)) = (sheet, layout) else { return };
    if !existing.is_empty() {
        return;
    }
    let slot = |front: bool| {
        (
            FlameSlot { front, flame: None },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(NODE_SIZE),
                height: Val::Px(NODE_SIZE),
                ..default()
            },
            ImageNode::from_atlas_image(sheet.0.clone(), TextureAtlas { layout: layout.0.clone(), index: 0 }),
            UiTransform::IDENTITY,
            Visibility::Hidden,
            ZIndex(if front { FRONT_LAYER } else { 0 }),
            Pickable::IGNORE,
        )
    };
    commands
        .spawn((
            HudFireRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                overflow: Overflow::clip(),
                ..default()
            },
            GlobalZIndex(LAYER),
            Pickable::IGNORE,
            Name::new("HUD fire"),
        ))
        .with_children(|root| {
            for _ in 0..SIDE_SLOTS {
                root.spawn(slot(false));
            }
            for _ in 0..FRONT_SLOTS {
                root.spawn(slot(true));
            }
        });
}

fn light_the_flames(
    time: Res<Time>,
    root: Option<Single<&ComputedNode, With<HudFireRoot>>>,
    burning: Query<(), (Viewed, With<Player>, With<OnFire>)>,
    mut slots: Query<&mut FlameSlot>,
    settings: Option<Res<crate::common::player_settings::PlayerSettings>>,
    mut owed: Local<(f32, f32)>,
) {
    let Some(root) = root else { return };
    let window = root.size() * root.inverse_scale_factor();
    if burning.is_empty() || window.y <= 0.0 {
        *owed = (0.0, 0.0);
        return;
    }
    owed.0 += RATE * time.delta_secs();
    // Reduced, only the side flames: the front ones cross the middle of the view.
    if !settings.is_some_and(|settings| settings.accessibility.reduce_particles) {
        owed.1 += FRONT_RATE * time.delta_secs();
    }
    let (mut sides, mut fronts) = (owed.0.floor(), owed.1.floor());
    owed.0 -= sides;
    owed.1 -= fronts;
    let mut rng = rand::rng();
    for mut slot in &mut slots {
        if slot.flame.is_some() {
            continue;
        }
        if slot.front && fronts >= 1.0 {
            fronts -= 1.0;
            slot.flame = Some(front_flame_at(window, rng.random_range(0.0..1.0), &mut rng));
        } else if !slot.front && sides >= 1.0 {
            sides -= 1.0;
            slot.flame = Some(flame_at(window, rng.random_range(0.0..1.0), rng.random_bool(0.5), &mut rng));
        }
    }
}

fn move_the_flames(
    time: Res<Time>,
    root: Option<Single<&ComputedNode, With<HudFireRoot>>>,
    mut slots: Query<(&mut FlameSlot, &mut UiTransform, &mut ImageNode, &mut Visibility)>,
    mut last_window: Local<Vec2>,
) {
    let Some(root) = root else { return };
    let window = root.size() * root.inverse_scale_factor();
    let resized = (*last_window != window && last_window.x > 0.0 && last_window.y > 0.0)
        .then(|| window / *last_window);
    *last_window = window;
    let dt = time.delta_secs();
    for (mut slot, mut transform, mut image, mut visibility) in &mut slots {
        let Some(flame) = slot.flame.as_mut() else { continue };
        flame.age += dt;
        if flame.age >= flame.life {
            slot.flame = None;
            *visibility = Visibility::Hidden;
            continue;
        }
        if let Some(ratio) = resized {
            flame.at *= ratio;
            flame.velocity *= ratio;
            flame.size *= ratio.y;
        }
        flame.velocity.x += repel(window, flame.at.x) * dt;
        let step = flame.velocity * dt;
        flame.at += step;
        let corner = flame.at - Vec2::splat(NODE_SIZE * 0.5);
        *transform = UiTransform {
            translation: Val2::px(corner.x, corner.y),
            scale: Vec2::splat(flame.size / NODE_SIZE * flame.scale()),
            // The sheet's flames point right; turned a quarter to point up.
            rotation: Rot2::degrees(flame.lean - 90.0),
        };
        image.color = Color::WHITE.with_alpha(flame.opacity());
        if let Some(atlas) = image.texture_atlas.as_mut() {
            atlas.index = flame.shape();
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

fn put_out_the_flames(mut commands: Commands, roots: Query<Entity, With<HudFireRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

/// Added in `launch.rs` beside [`crate::game::flame_jet::FlameJetPlugin`], whose
/// sheet it draws, since the game's tests have no asset plugins.
pub struct HudFirePlugin;

impl Plugin for HudFirePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, make_the_look)
            .add_systems(
                Update,
                (spawn_the_root, light_the_flames, move_the_flames).chain().run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), put_out_the_flames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if a 16:9 window's strip runs into the safe zone, or a 4:3 one has
    /// no strip at all.
    #[test]
    fn the_strips_are_outside_the_safe_zone_or_at_least_their_least() {
        let wide = Vec2::new(1920.0, 1080.0);
        assert_eq!(left_strip(wide).1, 1920.0 * 0.5 - 1080.0 * 2.0 / 3.0);
        let square = Vec2::new(1440.0, 1080.0);
        assert_eq!(left_strip(square).1, 1080.0 * LEAST_STRIP);
    }

    /// Fails if a flame at the window's edge climbs no higher than one at the
    /// safe zone's edge, which is what makes the V.
    #[test]
    fn flames_climb_highest_at_the_windows_edge() {
        let window = Vec2::new(1920.0, 1080.0);
        let mut rng = rand::rng();
        let climb = |out: f32, rng: &mut rand::rngs::ThreadRng| {
            let flame = flame_at(window, out, true, rng);
            -flame.velocity.y * flame.life
        };
        let (edge, inner) = (climb(1.0, &mut rng), climb(0.0, &mut rng));
        let scale = SIDE_SPEED * SIDE_LIFE * window.y;
        assert!((edge - TALL * scale).abs() < 0.01 && (inner - SHORT * scale).abs() < 0.01, "{edge} and {inner}");
        let right = flame_at(window, 1.0, false, &mut rng);
        assert_eq!(right.at.x, window.x);
        assert!(right.velocity.x >= 0.0, "leans inward");
    }

    /// Fails if the safe zone pulls a flame toward its middle, or pushes one
    /// outside it.
    #[test]
    fn the_safe_zone_pushes_flames_out_of_itself() {
        let window = Vec2::new(1920.0, 1080.0);
        assert!(repel(window, 900.0) < 0.0 && repel(window, 1020.0) > 0.0);
        assert!(repel(window, 600.0).abs() < repel(window, 900.0).abs(), "harder toward the middle");
        assert_eq!(repel(window, 100.0), 0.0);
        assert_eq!(repel(window, 1820.0), 0.0);
    }
}
