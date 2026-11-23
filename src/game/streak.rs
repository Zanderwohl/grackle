//! A white mesh streak down each hitscan pellet's path, one per [`ShotPath`]
//! whose weapon's trail is [`HitscanTrail::Streak`].
//! Drawing only: the shot was resolved on the tick.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::weapon::HitscanTrail;
use crate::game::hitscan::{mark_tracers, ShotPath};
use crate::game::player::PlayerCamera;
use crate::game::warm_up::{self, WarmUp};

/// Meters per second. Slow enough to take several frames across a room.
const SPEED: f32 = 150.0;

/// Meters from head to tail, shorter where the path is.
const LENGTH: f32 = 3.0;

/// Seconds. A shot at a wall a meter away is still seen.
const SHORTEST_LIFE: f32 = 0.06;

/// Meters, up close.
const RADIUS: f32 = 0.012;

/// Meters of radius per meter from the eye: 1.5 px wide at 1080p with a 90°
/// view, so a far streak does not drop between pixels.
const RADIUS_PER_METER: f32 = 0.0014;

/// Seconds. A slow frame moves a streak no further than this, so a hitch does
/// not skip most of a short one.
const LONGEST_STEP: f32 = 1.0 / 30.0;

#[derive(Component, Debug, Clone, Copy)]
struct Streak {
    from: Vec3,
    to: Vec3,
    /// Meters per second.
    speed: f32,
    age: f32,
}

impl Streak {
    fn new(from: Vec3, to: Vec3) -> Self {
        let travel = from.distance(to) + LENGTH;
        Self { from, to, speed: SPEED.min(travel / SHORTEST_LIFE), age: 0.0 }
    }

    /// Head and tail as distances along the path, or `None` once the tail has
    /// passed the end.
    fn span(&self) -> Option<(f32, f32)> {
        let path = self.from.distance(self.to);
        let head = self.age * self.speed;
        let tail = head - LENGTH;
        (tail < path).then(|| (tail.max(0.0), head.min(path)))
    }

    /// A unit cylinder along `Y`, stretched over the span and widened by the
    /// distance from `eye` to its nearest point.
    fn transform(&self, (tail, head): (f32, f32), eye: Option<Vec3>) -> Transform {
        let direction = (self.to - self.from).normalize_or(Vec3::NEG_Z);
        let width = widened(self.from, direction, (tail, head), eye, RADIUS, RADIUS_PER_METER) / RADIUS;
        Transform {
            translation: self.from + direction * (tail + head) * 0.5,
            rotation: Quat::from_rotation_arc(Vec3::Y, direction),
            scale: Vec3::new(width, (head - tail).max(1.0e-4), width),
        }
    }
}

/// `base` meters, or `per_meter` times the distance from `eye` to the nearest
/// point of the span (meters along `direction` from `from`) if that is more.
pub(crate) fn widened(from: Vec3, direction: Vec3, (start, end): (f32, f32), eye: Option<Vec3>, base: f32, per_meter: f32) -> f32 {
    eye.map_or(base, |eye| {
        let along = (eye - from).dot(direction).clamp(start, end);
        base.max(eye.distance(from + direction * along) * per_meter)
    })
}

#[derive(Resource)]
struct StreakLook {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

fn make_the_look(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(StreakLook {
        mesh: meshes.add(Cylinder::new(RADIUS, 1.0).mesh().resolution(6)),
        material: materials.add(StandardMaterial {
            // Above one so it stays white once tonemapped.
            base_color: Color::linear_rgb(3.0, 3.0, 3.0),
            unlit: true,
            alpha_mode: AlphaMode::Add,
            ..default()
        }),
    });
}

/// The streak material for [`crate::game::warm_up`].
fn warm_up(mut commands: Commands, look: Res<StreakLook>) {
    use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};

    commands.spawn((
        WarmUp,
        Mesh3d(look.mesh.clone()),
        MeshMaterial3d(look.material.clone()),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        NoFrustumCulling,
        NotShadowCaster,
        // Too small to cover a pixel of the 4×4 target.
        Transform::from_xyz(0.0, 0.0, -1.0).with_scale(Vec3::splat(1.0e-4)),
        Visibility::default(),
    ));
}

fn launch_streaks(
    mut commands: Commands,
    look: Res<StreakLook>,
    mut paths: MessageReader<ShotPath>,
    eye: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
) {
    let eye = eye.map(|eye| eye.translation());
    for path in paths.read().filter(|path| path.trail == HitscanTrail::Streak) {
        let streak = Streak::new(path.from, path.to);
        let Some(span) = streak.span() else { continue };
        commands.spawn((
            streak,
            Mesh3d(look.mesh.clone()),
            MeshMaterial3d(look.material.clone()),
            streak.transform(span, eye),
            NotShadowCaster,
            Name::new("Streak"),
        ));
    }
}

fn fly_streaks(
    mut commands: Commands,
    time: Res<Time>,
    eye: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
    mut streaks: Query<(Entity, &mut Streak, &mut Transform)>,
) {
    let eye = eye.map(|eye| eye.translation());
    for (entity, mut streak, mut transform) in &mut streaks {
        streak.age += time.delta_secs().min(LONGEST_STEP);
        match streak.span() {
            Some(span) => *transform = streak.transform(span, eye),
            None => commands.entity(entity).despawn(),
        }
    }
}

fn clear_streaks(mut commands: Commands, streaks: Query<Entity, With<Streak>>) {
    for streak in &streaks {
        commands.entity(streak).despawn();
    }
}

/// Ungated, as tracers are: a client draws every shot. After `mark_tracers`,
/// so a streak starts the frame its shot arrives.
fn add_flight(app: &mut App) {
    app.add_message::<ShotPath>()
        .add_systems(Update, (launch_streaks.after(mark_tracers), fly_streaks).chain());
}

/// Added in `launch.rs` beside `WorldPropPlugin`: it builds a mesh and a
/// material, which the game's tests have no assets for.
pub struct StreakPlugin;

impl Plugin for StreakPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        add_flight(app);
        app.add_systems(Startup, (make_the_look, warm_up).chain())
            .add_systems(OnExit(AppMode::Play), clear_streaks)
            .add_systems(OnExit(AppMode::Weapon), clear_streaks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Fails if the streak does not start at the shot point, overshoots the
    /// end, or never finishes.
    #[test]
    fn a_streak_runs_from_the_shot_to_where_it_stopped_and_ends() {
        let mut streak = Streak::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -30.0));
        assert_eq!(streak.span(), Some((0.0, 0.0)));

        streak.age = 0.1;
        let (tail, head) = streak.span().expect("gone too soon");
        assert!(head > tail && head - tail <= LENGTH + 1.0e-4, "{tail}..{head}");
        let drawn = streak.transform((tail, head), None);
        assert!(drawn.translation.z < 0.0 && drawn.translation.z > -30.0);
        assert!((drawn.rotation * Vec3::Y).abs_diff_eq(Vec3::NEG_Z, 1.0e-5));

        streak.age = 0.21;
        let (_, head) = streak.span().expect("gone too soon");
        assert_eq!(head, 30.0, "it ran past where the shot stopped");

        streak.age = (30.0 + LENGTH) / SPEED + 0.01;
        assert_eq!(streak.span(), None);
    }

    /// Fails if a shot at a wall a step away is over before a frame could show it.
    #[test]
    fn a_short_shot_still_lasts_a_few_frames() {
        let mut streak = Streak::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0));
        streak.age = SHORTEST_LIFE * 0.9;
        assert!(streak.span().is_some());
        streak.age = SHORTEST_LIFE * 1.01;
        assert!(streak.span().is_none());
    }

    fn an_app_that_draws_shots() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<crate::common::effects::Effect>()
            .insert_resource(StreakLook { mesh: Handle::default(), material: Handle::default() })
            .add_systems(Update, mark_tracers);
        add_flight(&mut app);
        app
    }

    /// Fails if a shot does not get its own streak in the frame it arrives, so
    /// a nine-pellet blast is nine streaks.
    #[test]
    fn every_pellet_gets_a_streak_the_frame_it_arrives() {
        let mut app = an_app_that_draws_shots();
        for x in 0..9 {
            let to = Vec3::new(x as f32, 0.0, -20.0);
            app.world_mut().write_message(crate::common::effects::Effect::Tracer { from: Vec3::ZERO, to, hit: false, by: None, trail: HitscanTrail::Streak });
        }
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&Streak>().iter(world).count(), 9);
    }

    /// Fails if leaving a round leaves streaks standing.
    #[test]
    fn clearing_takes_every_streak() {
        let mut app = an_app_that_draws_shots();
        app.world_mut().write_message(ShotPath { from: Vec3::ZERO, to: Vec3::NEG_Z * 20.0, by: None, trail: HitscanTrail::Streak });
        app.update();
        let world = app.world_mut();
        world.run_system_once(clear_streaks).unwrap();
        assert_eq!(world.query::<&Streak>().iter(world).count(), 0);
    }

    /// Fails if a far streak keeps its close-up width and drops between pixels.
    #[test]
    fn a_far_streak_is_drawn_wider() {
        let streak = Streak::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -100.0));
        let near = streak.transform((0.0, 3.0), Some(Vec3::ZERO));
        let far = streak.transform((80.0, 83.0), Some(Vec3::ZERO));
        assert_eq!(near.scale.x, 1.0);
        assert!((far.scale.x * RADIUS - 80.0 * RADIUS_PER_METER).abs() < 1.0e-5, "{}", far.scale.x);
    }
}
