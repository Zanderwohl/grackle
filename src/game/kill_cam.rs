//! When another player kills us: slow the match to a stop while the view swings out over our body
//! and glides over to frame the killer, then spectate until we respawn. Our own death or the map's
//! keeps the plain death cam. See [documentation/kill-cam.md](../../documentation/kill-cam.md).

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{DamageSource, Died, PlayerId};
use crate::common::weapon::WeaponId;
use crate::game::collision::CollisionWorld;
use crate::game::death::Dying;
use crate::game::respawn::OurIdentity;
use crate::game::player::{start_the_death_cam, DeathCam, LocalPlayer, OurLastBody, Player, PlayerCamera, CAMERA_RADIUS};
use crate::game::ragdoll::{CorpseOf, Ragdoll};
use crate::game::replay::Replay;
use crate::game::spectate::SpectatorView;
use crate::game::team_choice::TeamOptions;

/// Seconds out from the eye to third person over our body.
pub const SWING_SECONDS: f32 = 0.5;
/// Seconds from there to the killer's framing; time has stopped when it arrives.
pub const GLIDE_SECONDS: f32 = 0.5;
/// Seconds over which the match slows to a stop.
const SLOW_SECONDS: f32 = SWING_SECONDS + GLIDE_SECONDS;
/// Seconds held on the killer before the spectator cycle.
pub const HOLD_SECONDS: f32 = 1.0;
/// Seconds a `Died` naming our killer still starts the kill cam: on a client it and the despawn
/// arrive in either order.
pub const KILL_CAM_GRACE: f32 = 1.0;
/// The share of the vertical field of view the killer's body fills when framed.
pub const FRAME_FILL: f32 = 0.6;

/// Who last killed us, from a `Died` naming another player.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct OurKiller {
    pub killer: PlayerId,
    /// What they killed us with; `None` for a kill nothing fired.
    pub weapon: Option<WeaponId>,
    /// Seconds of `Time`, when it was read.
    pub at: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Swing,
    Glide,
    Hold,
    Spectate,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: Transform,
    to: Transform,
}

/// Exists from the death until the next body. The replay runs until [`Phase::Spectate`].
#[derive(Resource, Clone, Debug)]
pub struct KillCam {
    pub killer: PlayerId,
    pub weapon: Option<WeaponId>,
    /// Seconds of `Time`.
    started: f64,
    /// Real seconds since it started.
    pub elapsed: f32,
    glide: Option<Glide>,
    /// Our eye as we died: the framing never stands further from the killer than this.
    died_at: Vec3,
    /// Our corpse's pelvis as it was drawn live, so the death cam's pivot can follow the replay.
    corpse: VecDeque<(f64, Vec3)>,
}

impl KillCam {
    pub fn new(killer: PlayerId, weapon: Option<WeaponId>, started: f64, died_at: Vec3) -> Self {
        Self { killer, weapon, started, elapsed: 0.0, glide: None, died_at, corpse: VecDeque::new() }
    }

    pub fn phase(&self) -> Phase {
        match self.elapsed {
            e if e < SWING_SECONDS => Phase::Swing,
            e if e < SWING_SECONDS + GLIDE_SECONDS => Phase::Glide,
            e if e < SWING_SECONDS + GLIDE_SECONDS + HOLD_SECONDS => Phase::Hold,
            _ => Phase::Spectate,
        }
    }

    pub fn spectating(&self) -> bool {
        self.phase() == Phase::Spectate
    }

    /// The moment drawn: the match slows linearly from full speed to a stop over the swing and
    /// the glide, so it ends half that long behind.
    pub fn view(&self) -> f64 {
        self.started + slowed(self.elapsed) as f64
    }

    /// How fast the view moves through match time: the slope of [`slowed`].
    pub fn speed(&self) -> f32 {
        (1.0 - self.elapsed / SLOW_SECONDS).max(0.0)
    }

    pub fn frozen(&self) -> bool {
        self.elapsed >= SLOW_SECONDS
    }

    /// Where the view is while gliding to the killer or held on them.
    pub fn camera(&self) -> Option<Transform> {
        let glide = self.glide?;
        let t = ((self.elapsed - SWING_SECONDS) / GLIDE_SECONDS).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        match self.phase() {
            Phase::Glide | Phase::Hold => Some(Transform {
                translation: glide.from.translation.lerp(glide.to.translation, t),
                rotation: glide.from.rotation.slerp(glide.to.rotation, t),
                scale: Vec3::ONE,
            }),
            Phase::Swing | Phase::Spectate => None,
        }
    }

    pub fn corpse_at(&self, view: f64) -> Option<Vec3> {
        let after = self.corpse.partition_point(|(at, _)| *at <= view);
        let (at, pelvis) = match after {
            0 => return self.corpse.front().map(|(_, pelvis)| *pelvis),
            after => self.corpse[after - 1],
        };
        let Some(&(next_at, next)) = self.corpse.get(after) else { return Some(pelvis) };
        let t = ((view - at) / (next_at - at)).clamp(0.0, 1.0) as f32;
        Some(pelvis.lerp(next, t))
    }
}

/// Seconds of match time seen after `elapsed` real seconds.
pub fn slowed(elapsed: f32) -> f32 {
    let e = elapsed.clamp(0.0, SLOW_SECONDS);
    e - e * e / (2.0 * SLOW_SECONDS)
}

/// Where the camera stands to show the killer head to feet: on the line from them to `pivot`,
/// never further from them than `pivot`, pulled in short of walls. `feet` is the bottom of their
/// hull, `fov` the vertical field of view in radians.
pub fn framing(feet: Vec3, height: f32, pivot: Vec3, fov: f32, world: &CollisionWorld) -> Transform {
    let center = feet + Vec3::Y * height * 0.5;
    let wanted_distance = height * 0.5 / ((fov * 0.5).tan() * FRAME_FILL);
    let away = pivot - center;
    let distance = away.length();
    let wanted = match distance > wanted_distance {
        true => center + away / distance * wanted_distance,
        false => pivot,
    };
    let placed = world.travel(center, Vec3::splat(CAMERA_RADIUS), wanted - center);
    let mut camera = Transform::from_translation(placed);
    let toward = center - placed;
    // Standing inside them, there is nothing to turn toward.
    if toward.length_squared() > 1e-4 {
        // Straight above or below, Y is no up.
        let up = match toward.normalize().y.abs() > 0.99 {
            true => Vec3::NEG_Z,
            false => Vec3::Y,
        };
        camera.look_at(center, up);
    }
    camera
}

/// Our body's, else our identity's (a host's, with or without a body), else the body that died:
/// a client has no identity of its own and knows itself only by a body.
pub(crate) fn our_id(ours: Option<PlayerId>, identity: Option<&OurIdentity>, last: Option<&OurLastBody>, death_cam: Option<&DeathCam>) -> Option<PlayerId> {
    ours.or(identity.map(|identity| identity.0.id)).or(last.and_then(|last| last.whose())).or(death_cam.and_then(|cam| cam.whose))
}

fn learn_our_killer(
    mut commands: Commands,
    time: Res<Time>,
    mut deaths: MessageReader<Died>,
    ours: Option<Single<&PlayerId, With<LocalPlayer>>>,
    identity: Option<Res<OurIdentity>>,
    last: Option<Res<OurLastBody>>,
    death_cam: Option<Res<DeathCam>>,
) {
    let Some(us) = our_id(ours.map(|ours| **ours), identity.as_deref(), last.as_deref(), death_cam.as_deref()) else {
        deaths.clear();
        return;
    };
    for death in deaths.read() {
        if death.victim_id != Some(us) {
            continue;
        }
        if let DamageSource::Player(killer) = death.killer
            && killer != us
        {
            commands.insert_resource(OurKiller { killer, weapon: death.cause.weapon, at: time.elapsed_secs_f64() });
        }
    }
}

/// The one writer of [`KillCam`] and [`Replay`].
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn run_the_kill_cam(
    mut commands: Commands,
    time: Res<Time>,
    world: Res<CollisionWorld>,
    ours: Query<(), With<LocalPlayer>>,
    killer: Option<Res<OurKiller>>,
    death_cam: Option<Res<DeathCam>>,
    kill_cam: Option<ResMut<KillCam>>,
    replay: Option<Res<Replay>>,
    // Absent in tests that build no team choice.
    offer: Option<Res<TeamOptions>>,
    mut view: ResMut<SpectatorView>,
    bodies: Query<(&PlayerId, &Transform, &Stance), (With<Player>, Without<Dying>, Without<LocalPlayer>)>,
    corpses: Query<(&Ragdoll, &CorpseOf)>,
    camera: Option<Single<(&Transform, &Projection), With<PlayerCamera>>>,
) {
    let now = time.elapsed_secs_f64();
    // Choosing a team again or spectating ends it as a body would: the picker owns the view.
    let playing = offer.is_none_or(|offer| offer.seat.is_playing());
    if !ours.is_empty() || !playing {
        if kill_cam.is_some() {
            commands.remove_resource::<KillCam>();
        }
        if replay.is_some() {
            commands.remove_resource::<Replay>();
        }
        return;
    }
    let Some(mut kill_cam) = kill_cam else {
        let dead = death_cam.as_ref().is_some_and(|cam| cam.whose.is_some());
        if let Some(killer) = killer.filter(|killer| dead && now - killer.at <= KILL_CAM_GRACE as f64) {
            let died_at = death_cam.as_ref().map_or(Vec3::ZERO, |cam| cam.pivot);
            commands.insert_resource(KillCam::new(killer.killer, killer.weapon, now, died_at));
            commands.insert_resource(Replay { view: now, frozen: false, speed: 1.0 });
            commands.remove_resource::<OurKiller>();
        }
        return;
    };
    let was = kill_cam.phase();
    kill_cam.elapsed += time.delta_secs();
    let phase = kill_cam.phase();

    if !kill_cam.frozen()
        && let Some(whose) = death_cam.as_ref().and_then(|cam| cam.whose)
        && let Some((corpse, _)) = corpses.iter().filter(|(_, of)| of.0 == whose).min_by(|(a, _), (b, _)| a.age().total_cmp(&b.age()))
    {
        kill_cam.corpse.push_back((now, corpse.pelvis()));
    }

    // At the glide's start the live match is where the replay will stop, half a second on.
    if phase >= Phase::Glide
        && kill_cam.glide.is_none()
        && let Some(camera) = camera
    {
        let (from, projection) = camera.into_inner();
        let fov = match projection {
            Projection::Perspective(perspective) => perspective.fov,
            _ => std::f32::consts::FRAC_PI_2,
        };
        let pivot = kill_cam.died_at;
        let to = bodies
            .iter()
            .find(|(id, ..)| **id == kill_cam.killer)
            .map_or(*from, |(_, placed, stance)| {
                let feet = placed.translation - Vec3::Y * stance.height() * 0.5;
                framing(feet, stance.height(), pivot, fov, &world)
            });
        kill_cam.glide = Some(Glide { from: *from, to });
    }

    if phase == Phase::Spectate {
        // From the top of the spectator list, where the cameras are.
        if was != Phase::Spectate {
            *view = SpectatorView::default();
        }
        if replay.is_some() {
            commands.remove_resource::<Replay>();
        }
        return;
    }
    let wanted = Replay { view: kill_cam.view(), frozen: kill_cam.frozen(), speed: kill_cam.speed() };
    if replay.as_deref() != Some(&wanted) {
        commands.insert_resource(wanted);
    }
}

pub(crate) fn end_the_kill_cam(mut commands: Commands) {
    commands.remove_resource::<KillCam>();
    commands.remove_resource::<Replay>();
    commands.remove_resource::<OurKiller>();
}

pub struct KillCamPlugin;

impl Plugin for KillCamPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::game::replay::ReplayPlugin)
            .add_systems(
                Update,
                (learn_our_killer, run_the_kill_cam)
                    .chain()
                    .after(start_the_death_cam)
                    .before(crate::game::spectate::watch)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), end_the_kill_cam);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaks if the match does not slow to a stop half a second behind, or runs backward.
    #[test]
    fn the_match_slows_to_a_stop_half_the_slowdown_behind() {
        assert_eq!(slowed(0.0), 0.0);
        let slow = SLOW_SECONDS;
        assert!((slowed(slow) - slow * 0.5).abs() < 1e-6);
        assert_eq!(slowed(slow + 5.0), slowed(slow), "time moved after it stopped");
        let mut last = 0.0;
        for step in 1..=100 {
            let seen = slowed(step as f32 * slow / 100.0);
            assert!(seen >= last, "ran backward");
            last = seen;
        }
    }

    /// Breaks if the particles' clock does not follow the view's slope.
    #[test]
    fn the_speed_is_the_slope_of_the_slowdown() {
        let slow = SLOW_SECONDS;
        let mut cam = KillCam::new(PlayerId(1), None, 0.0, Vec3::ZERO);
        let step = 1e-3;
        for at in [0.0, 0.25, 0.5, 0.9] {
            cam.elapsed = at * slow;
            let slope = (slowed(cam.elapsed + step) - slowed(cam.elapsed)) / step;
            assert!((cam.speed() - slope).abs() < 1e-2, "at {at}: speed {} slope {slope}", cam.speed());
        }
        cam.elapsed = slow + 1.0;
        assert_eq!(cam.speed(), 0.0, "particles move after time stopped");
    }

    /// Breaks if the killer is not framed head to feet, if the camera moves further away than we
    /// died, or if it does not look at them.
    #[test]
    fn the_killer_fills_the_frame_from_no_further_than_we_died() {
        let world = CollisionWorld::default();
        let fov = 1.2;
        let far = framing(Vec3::ZERO, 2.0, Vec3::new(0.0, 1.0, 50.0), fov, &world);
        let distance = far.translation.distance(Vec3::Y);
        let filled = 2.0 / (2.0 * distance * (fov * 0.5).tan());
        assert!((filled - FRAME_FILL).abs() < 1e-3, "filled {filled} of the view");
        assert!(far.forward().dot((Vec3::Y - far.translation).normalize()) > 0.999, "not looking at the killer");

        let near = Vec3::new(0.0, 1.0, 1.0);
        assert_eq!(framing(Vec3::ZERO, 2.0, near, fov, &world).translation, near, "backed away from where we died");
        let above = framing(Vec3::ZERO, 2.0, Vec3::new(0.0, 4.0, 0.0), fov, &world);
        assert!(above.rotation.is_finite() && above.forward().dot(Vec3::NEG_Y) > 0.999, "not looking straight down from above");
    }

    fn death(victim: u64, killer: DamageSource) -> Died {
        Died { victim: Entity::PLACEHOLDER, victim_name: None, victim_id: Some(PlayerId(victim)), killer, assist: None, at: Vec3::ZERO, cause: Default::default() }
    }

    /// Breaks if a kill by somebody else does not run the replay through to spectating their
    /// eye, if our own death does, or if a respawn does not end it.
    #[test]
    fn a_kill_runs_the_replay_until_spectating_and_a_respawn_ends_it() {
        let mut world = World::new();
        world.init_resource::<Messages<Died>>();
        world.init_resource::<Time>();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<SpectatorView>();
        let learn = world.register_system(learn_our_killer);
        let run = world.register_system(run_the_kill_cam);
        let tick = |world: &mut World, seconds: f32| {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(seconds));
            world.run_system(learn).unwrap();
            world.run_system(run).unwrap();
        };
        let body = world.spawn((LocalPlayer, PlayerId(5))).id();

        world.write_message(death(5, DamageSource::Player(PlayerId(5))));
        tick(&mut world, 0.0);
        assert!(!world.contains_resource::<OurKiller>(), "our own death has a killer");

        world.write_message(death(5, DamageSource::Player(PlayerId(7))));
        tick(&mut world, 0.0);
        assert!(!world.contains_resource::<KillCam>(), "started while our body stood");
        world.despawn(body);
        world.insert_resource(DeathCam { pivot: Vec3::ZERO, whose: Some(PlayerId(5)) });
        tick(&mut world, 0.1);
        assert_eq!(world.resource::<KillCam>().killer, PlayerId(7));
        assert!(world.contains_resource::<Replay>());

        for _ in 0..30 {
            tick(&mut world, 0.1);
        }
        assert!(world.resource::<KillCam>().spectating());
        assert!(!world.contains_resource::<Replay>(), "still replaying while spectating");
        assert_eq!(world.resource::<SpectatorView>().at, None, "spectating did not start from the top of the list");

        world.spawn(LocalPlayer);
        tick(&mut world, 0.1);
        assert!(!world.contains_resource::<KillCam>(), "outlived the respawn");
    }

    /// Breaks if spectating after the kill cam starts on a body while a spectator camera stands.
    #[test]
    fn spectating_after_the_kill_cam_starts_in_a_camera() {
        use crate::editor::spectator_camera::{SpectatorCameraSpec, Viewers};
        use crate::game::pause_menu::MouseHeld;
        use crate::game::player::InputLatch;
        use crate::game::respawn::Seat;
        use crate::game::spectate::{watch, Viewpoint};
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(TeamOptions { seat: Seat::Playing(None), ..default() });
        world.init_resource::<crate::common::keybind::Actions>();
        world.init_resource::<MouseHeld>();
        world.init_resource::<InputLatch>();
        world.init_resource::<SpectatorView>();
        let mut cam = KillCam::new(PlayerId(7), None, 0.0, Vec3::ZERO);
        cam.elapsed = SWING_SECONDS + GLIDE_SECONDS + HOLD_SECONDS;
        assert!(cam.spectating());
        world.insert_resource(cam);
        world.spawn((Player::default(), PlayerId(7)));
        let camera = world.spawn((Transform::default(), SpectatorCameraSpec { fov: 1.0, order: 0, viewers: Viewers::default() })).id();

        world.run_system_once(watch).unwrap();
        assert_eq!(world.resource::<SpectatorView>().at, Some(Viewpoint::Camera(camera)));
    }

    /// Breaks if the phases do not come in order, or the glide does not arrive.
    #[test]
    fn the_glide_arrives_and_holds_before_spectating() {
        let mut cam = KillCam::new(PlayerId(1), None, 0.0, Vec3::ZERO);
        assert_eq!(cam.phase(), Phase::Swing);
        let to = Transform::from_xyz(5.0, 1.0, 0.0);
        cam.glide = Some(Glide { from: Transform::IDENTITY, to });
        cam.elapsed = SWING_SECONDS + GLIDE_SECONDS;
        assert_eq!(cam.phase(), Phase::Hold);
        assert!(cam.frozen());
        assert_eq!(cam.camera().unwrap().translation, to.translation);
        cam.elapsed += HOLD_SECONDS;
        assert!(cam.spectating());
        assert_eq!(cam.camera(), None);
    }
}
