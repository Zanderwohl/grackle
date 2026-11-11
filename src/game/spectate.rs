//! Watching a match from [`Seat::Spectating`]: no body, and a view the left
//! and right mouse buttons step forward and back through, wrapping — every
//! living body's eye, then every spectator camera the seat may look through,
//! or every spawn point on a map with none. Every machine works its list out
//! from what it can see; the seat is the server's, read off [`TeamOptions`].

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::team::Side;
use crate::editor::editable::FeatureTag;
use crate::editor::mirror::MirrorCopy;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::editor::spectator_camera::SpectatorCameraSpec;
use crate::game::death::Dying;
use crate::game::pause_menu::MouseHeld;
use crate::game::player::{start_the_death_cam, DeathCam, InputLatch, LocalPlayer, OurLastBody, Player, ThroughCamera, Watched};
use crate::game::respawn::Seat;
use crate::game::team_choice::{ChooseTeam, TeamChoice, TeamOptions};
use crate::game::team_picker::{overlook, OVERLOOK_PITCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewpoint {
    /// Overlooked as the team picker overlooks one.
    Spawn(Entity),
    /// Out of that player's body, looking where they look.
    Eye(PlayerId),
    /// Out of a placed spectator camera.
    Camera(Entity),
}

/// Where a camera stands among the cameras: its order, then the feature it
/// is or was copied from, then which copy (0 for the original).
pub type CameraRank = (u32, u64, usize);

/// Bodies by id, then cameras by rank, then spawns by index only when there
/// is no camera. Indexes rather than `Entity`'s own order, which is not its
/// index's, so the order does not depend on archetypes.
pub fn viewpoints(
    bodies: impl IntoIterator<Item = PlayerId>,
    cameras: impl IntoIterator<Item = (Entity, CameraRank)>,
    spawns: impl IntoIterator<Item = Entity>,
) -> Vec<Viewpoint> {
    let mut bodies: Vec<PlayerId> = bodies.into_iter().collect();
    bodies.sort();
    let mut cameras: Vec<(Entity, CameraRank)> = cameras.into_iter().collect();
    rank(&mut cameras);
    let mut spawns: Vec<Entity> = match cameras.is_empty() {
        true => spawns.into_iter().collect(),
        false => Vec::new(),
    };
    spawns.sort_by_key(|spawn| spawn.index());
    let bodies = bodies.into_iter().map(Viewpoint::Eye);
    let cameras = cameras.into_iter().map(|(camera, _)| Viewpoint::Camera(camera));
    bodies.chain(cameras).chain(spawns.into_iter().map(Viewpoint::Spawn)).collect()
}

/// The entity index breaks ties, so the order is the same every frame.
fn rank(cameras: &mut [(Entity, CameraRank)]) {
    cameras.sort_by_key(|(camera, rank)| (*rank, camera.index()));
}

/// Every spectator camera standing, Mirror copies included.
pub type Cameras<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Transform, &'static SpectatorCameraSpec, Option<&'static FeatureTag>, Option<&'static MirrorCopy>, Option<&'static Side>),
>;

/// The cameras `seat` may look through, ranked.
pub fn cameras_for(cameras: &Cameras, seat: Seat) -> Vec<(Entity, CameraRank)> {
    let mut admitted: Vec<(Entity, CameraRank)> = cameras
        .iter()
        .filter(|(_, _, spec, _, _, side)| spec.viewers.admits(seat, side.map(|side| side.0)))
        .map(|(camera, _, spec, tag, copy, _)| {
            let (feature, image) = match (tag, copy) {
                (_, Some(copy)) => (copy.source._id(), copy.image + 1),
                (Some(tag), None) => (tag.feature_id, 0),
                (None, None) => (u64::MAX, 0),
            };
            (camera, (spec.order, feature, image))
        })
        .collect();
    rank(&mut admitted);
    admitted
}

pub fn through(cameras: &Cameras, camera: Entity) -> Option<ThroughCamera> {
    let (_, placed, spec, ..) = cameras.get(camera).ok()?;
    Some(ThroughCamera { translation: placed.translation, rotation: placed.rotation, fov: spec.fov })
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct SpectatorView {
    pub at: Option<Viewpoint>,
    /// Where `at` last stood in the list, so a target that goes is replaced
    /// by whatever took its place.
    index: usize,
}

impl SpectatorView {
    /// Stays on the same target while it is listed, then steps `by`.
    pub fn follow(&mut self, list: &[Viewpoint], by: isize) {
        if list.is_empty() {
            self.at = None;
            return;
        }
        let here = self.at.and_then(|at| list.iter().position(|seen| *seen == at)).unwrap_or(self.index.min(list.len() - 1));
        self.index = (here as isize + by).rem_euclid(list.len() as isize) as usize;
        self.at = Some(list[self.index]);
    }
}

/// `--spectate`: each round, our seat goes straight from choosing to spectating.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct StartSpectating(pub bool);

/// Asked once each time the offer says we are choosing and may spectate. Keyed on the offer rather
/// than on entering Play: before our identity exists the offer cannot spectate, and on a client it
/// is a round trip behind.
fn spectate_from_the_start(start: Res<StartSpectating>, offer: Res<TeamOptions>, mut asked: Local<bool>, mut requests: MessageWriter<ChooseTeam>) {
    let choosing = offer.seat == Seat::Choosing && offer.spectate;
    if start.0 && choosing && !*asked {
        requests.write(ChooseTeam { choice: TeamChoice::Spectate, from: Seat::Choosing });
    }
    *asked = choosing;
}

pub struct SpectatePlugin;

impl Plugin for SpectatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpectatorView>()
            .init_resource::<MouseHeld>()
            .init_resource::<TeamOptions>()
            .init_resource::<StartSpectating>()
            .add_message::<ChooseTeam>()
            .add_systems(Update, spectate_from_the_start.run_if(in_state(AppMode::Play)))
            .add_systems(
                Update,
                // After the death cam, whose commands this overrides; before the view is decided.
                (watch, look_through)
                    .chain()
                    .after(start_the_death_cam)
                    .before(crate::game::condition::decide_the_view)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), |mut view: ResMut<SpectatorView>| *view = SpectatorView::default());
    }
}

/// The view while spectating: a [`DeathCam`] over a spawn, a
/// [`ThroughCamera`], or [`Watched`] on one body for `place_camera` to look
/// out of. The clicks are only read here; with no body, nothing fires.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn watch(
    mut commands: Commands,
    offer: Res<TeamOptions>,
    buttons: Res<ButtonInput<MouseButton>>,
    held: Res<MouseHeld>,
    mut view: ResMut<SpectatorView>,
    ours: Query<(), With<LocalPlayer>>,
    spawns: Query<(Entity, &Transform), With<SpawnPointMarker>>,
    cameras: Cameras,
    bodies: Query<(Entity, &PlayerId), (With<Player>, Without<Dying>)>,
    watched: Query<Entity, With<Watched>>,
    death_cam: Option<Res<DeathCam>>,
    last: Option<Res<OurLastBody>>,
    mut latch: ResMut<InputLatch>,
) {
    let watching = |target: Option<Entity>, commands: &mut Commands| {
        for body in watched.iter().filter(|body| Some(*body) != target) {
            commands.entity(body).remove::<Watched>();
        }
        if let Some(target) = target.filter(|target| !watched.contains(*target)) {
            commands.entity(target).insert(Watched);
        }
    };
    // The body given up for this seat is still ours until its despawn lands.
    if offer.seat != Seat::Spectating || !ours.is_empty() {
        watching(None, &mut commands);
        view.set_if_neq(SpectatorView::default());
        return;
    }
    // Chosen, not died: nobody wants to watch their own corpse.
    if last.is_some() {
        commands.remove_resource::<OurLastBody>();
    }

    let by = match held.any() {
        true => 0,
        false => buttons.just_pressed(MouseButton::Left) as isize - buttons.just_pressed(MouseButton::Right) as isize,
    };
    let list = viewpoints(bodies.iter().map(|(_, id)| *id), cameras_for(&cameras, offer.seat), spawns.iter().map(|(spawn, _)| spawn));
    let was = view.at;
    view.follow(&list, by);

    match view.at {
        Some(Viewpoint::Spawn(spawn)) => {
            watching(None, &mut commands);
            let Ok((_, placed)) = spawns.get(spawn) else { return };
            let wanted = overlook(placed.translation);
            if death_cam.as_deref() != Some(&wanted) {
                commands.insert_resource(wanted);
            }
            if was != view.at {
                latch.0.yaw = placed.rotation.to_euler(EulerRot::YXZ).0;
                latch.0.pitch = OVERLOOK_PITCH;
            }
        }
        Some(Viewpoint::Eye(id)) => {
            watching(bodies.iter().find(|(_, of)| **of == id).map(|(body, _)| body), &mut commands);
            if death_cam.is_some() {
                commands.remove_resource::<DeathCam>();
            }
        }
        Some(Viewpoint::Camera(_)) => {
            watching(None, &mut commands);
            if death_cam.is_some() {
                commands.remove_resource::<DeathCam>();
            }
        }
        None => watching(None, &mut commands),
    }
}

/// The one writer of [`ThroughCamera`]. None with a body of ours or while
/// dead; while spectating, the camera [`SpectatorView`] is on; otherwise the
/// first camera the seat may use.
#[allow(clippy::too_many_arguments)]
pub fn look_through(
    mut commands: Commands,
    offer: Res<TeamOptions>,
    view: Res<SpectatorView>,
    ours: Query<(), With<LocalPlayer>>,
    death_cam: Option<Res<DeathCam>>,
    last: Option<Res<OurLastBody>>,
    cameras: Cameras,
    current: Option<Res<ThroughCamera>>,
) {
    let dead = last.is_some() || death_cam.is_some_and(|cam| cam.whose.is_some());
    let wanted = match (ours.is_empty(), offer.seat) {
        (false, _) => None,
        (true, Seat::Spectating) => match view.at {
            Some(Viewpoint::Camera(camera)) => through(&cameras, camera),
            _ => None,
        },
        (true, _) if dead => None,
        (true, seat) => cameras_for(&cameras, seat).first().and_then(|(camera, _)| through(&cameras, *camera)),
    };
    match wanted {
        Some(wanted) if current.as_deref() != Some(&wanted) => commands.insert_resource(wanted),
        None if current.is_some() => commands.remove_resource::<ThroughCamera>(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(index: u32) -> Entity {
        Entity::from_raw_u32(index).unwrap()
    }

    #[test]
    fn bodies_come_first_by_id_then_spawns_with_no_camera() {
        let list = viewpoints([PlayerId(7), PlayerId(2)], [], [spawn(9), spawn(3)]);
        assert_eq!(
            list,
            vec![Viewpoint::Eye(PlayerId(2)), Viewpoint::Eye(PlayerId(7)), Viewpoint::Spawn(spawn(3)), Viewpoint::Spawn(spawn(9))]
        );
    }

    #[test]
    fn cameras_follow_the_bodies_by_order_then_feature_and_take_the_spawns_place() {
        let cameras = [(spawn(20), (1000, 4, 0)), (spawn(21), (5, 9, 0)), (spawn(22), (1000, 2, 1)), (spawn(23), (1000, 2, 0))];
        let list = viewpoints([PlayerId(1)], cameras, [spawn(3)]);
        assert_eq!(
            list,
            vec![
                Viewpoint::Eye(PlayerId(1)),
                Viewpoint::Camera(spawn(21)),
                Viewpoint::Camera(spawn(23)),
                Viewpoint::Camera(spawn(22)),
                Viewpoint::Camera(spawn(20)),
            ]
        );
    }

    #[test]
    fn left_steps_forward_and_right_back_wrapping_at_the_ends() {
        let list = viewpoints([PlayerId(1), PlayerId(2)], [], [spawn(1)]);
        let mut view = SpectatorView::default();
        view.follow(&list, 0);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(1))), "started anywhere but the first");

        let mut seen = Vec::new();
        for _ in 0..3 {
            view.follow(&list, 1);
            seen.push(view.at.unwrap());
        }
        assert_eq!(seen, vec![Viewpoint::Eye(PlayerId(2)), Viewpoint::Spawn(spawn(1)), Viewpoint::Eye(PlayerId(1))]);

        view.follow(&list, -1);
        assert_eq!(view.at, Some(Viewpoint::Spawn(spawn(1))), "back from the first did not wrap to the last");
    }

    /// Breaks if the view follows a place in the list rather than a target.
    #[test]
    fn the_view_stays_on_its_target_as_the_list_changes() {
        let mut view = SpectatorView::default();
        view.follow(&viewpoints([PlayerId(5)], [], []), 0);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(5))));

        view.follow(&viewpoints([PlayerId(2), PlayerId(5)], [], []), 0);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(5))), "somebody joining moved the view");

        view.follow(&viewpoints([PlayerId(2)], [], []), 0);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(2))), "a death left the view on nobody");

        view.follow(&[], 0);
        assert_eq!(view.at, None);
    }

    /// Breaks if the view stays in a camera after stepping off it, a body or a
    /// death, or looks through one shut to its seat.
    #[test]
    fn the_view_looks_through_a_camera_only_while_it_should() {
        use bevy::ecs::system::RunSystemOnce;
        use crate::common::team::Team;
        use crate::editor::spectator_camera::Viewers;

        let mut world = World::new();
        world.insert_resource(TeamOptions { seat: Seat::Spectating, ..default() });
        world.init_resource::<SpectatorView>();
        let spec = |order, viewers| SpectatorCameraSpec { fov: 1.0, order, viewers };
        let late = world.spawn((Transform::from_xyz(1.0, 0.0, 0.0), spec(7, Viewers::default()), FeatureTag { feature_id: 1 })).id();
        world
            .spawn((Transform::from_xyz(2.0, 0.0, 0.0), spec(3, Viewers { red: false, ..default() }), FeatureTag { feature_id: 2 }));
        let through = |world: &mut World| {
            world.run_system_once(look_through).unwrap();
            world.get_resource::<ThroughCamera>().map(|through| through.translation.x)
        };

        world.resource_mut::<SpectatorView>().at = Some(Viewpoint::Camera(late));
        assert_eq!(through(&mut world), Some(1.0), "the camera the spectator is on was not looked through");
        world.resource_mut::<SpectatorView>().at = Some(Viewpoint::Spawn(late));
        assert_eq!(through(&mut world), None, "stepping off the camera left the view in it");

        world.resource_mut::<TeamOptions>().seat = Seat::Playing(Some(Team::Blue));
        assert_eq!(through(&mut world), Some(2.0), "waiting for a body, not the first camera by order");
        world.resource_mut::<TeamOptions>().seat = Seat::Playing(Some(Team::Red));
        assert_eq!(through(&mut world), Some(1.0), "a camera shut to Red was looked through by Red");
        world.entity_mut(late).insert(Side(Team::Blue));
        assert_eq!(through(&mut world), None, "Red looked through a camera local to Blue");
        world.entity_mut(late).remove::<Side>();

        world.insert_resource(DeathCam { pivot: Vec3::ZERO, whose: Some(PlayerId(1)) });
        assert_eq!(through(&mut world), None, "a camera took the death cam's place");
        world.remove_resource::<DeathCam>();

        world.spawn(LocalPlayer);
        assert_eq!(through(&mut world), None, "a body of ours was looked past");
    }
}
