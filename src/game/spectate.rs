//! Watching a match from [`Seat::Spectating`]: no body, and a view the left
//! and right mouse buttons step forward and back through, wrapping — every
//! living body's eye, then every spectator camera the seat may look through,
//! or every spawn point on a map with none. Every machine works its list out
//! from what it can see; the seat is the server's, read off [`TeamOptions`].

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
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

/// Bodies by id, then cameras by rank, then — only with no camera — spawns
/// by index, so the order does not hang on archetypes. Not `Entity`'s own
/// order, which is not its index's.
pub fn viewpoints(
    bodies: impl IntoIterator<Item = PlayerId>,
    cameras: impl IntoIterator<Item = (Entity, CameraRank)>,
    spawns: impl IntoIterator<Item = Entity>,
) -> Vec<Viewpoint> {
    let mut bodies: Vec<PlayerId> = bodies.into_iter().collect();
    bodies.sort();
    let mut cameras: Vec<(Entity, CameraRank)> = cameras.into_iter().collect();
    cameras.sort_by_key(|(camera, rank)| (*rank, camera.index()));
    let mut spawns: Vec<Entity> = match cameras.is_empty() {
        true => spawns.into_iter().collect(),
        false => Vec::new(),
    };
    spawns.sort_by_key(|spawn| spawn.index());
    let bodies = bodies.into_iter().map(Viewpoint::Eye);
    let cameras = cameras.into_iter().map(|(camera, _)| Viewpoint::Camera(camera));
    bodies.chain(cameras).chain(spawns.into_iter().map(Viewpoint::Spawn)).collect()
}

/// Every spectator camera standing, Mirror copies included.
pub type Cameras<'w, 's> =
    Query<'w, 's, (Entity, &'static Transform, &'static SpectatorCameraSpec, Option<&'static FeatureTag>, Option<&'static MirrorCopy>)>;

/// The cameras `seat` may look through, ranked.
pub fn cameras_for(cameras: &Cameras, seat: Seat) -> Vec<(Entity, CameraRank)> {
    cameras
        .iter()
        .filter(|(_, _, spec, _, _)| spec.viewers.admits(seat))
        .map(|(camera, _, spec, tag, copy)| {
            let (feature, image) = match (tag, copy) {
                (_, Some(copy)) => (copy.source._id(), copy.image + 1),
                (Some(tag), None) => (tag.feature_id, 0),
                (None, None) => (u64::MAX, 0),
            };
            (camera, (spec.order, feature, image))
        })
        .collect()
}

/// The view out of `camera`, if it still stands.
pub fn through(cameras: &Cameras, camera: Entity) -> Option<ThroughCamera> {
    let (_, placed, spec, _, _) = cameras.get(camera).ok()?;
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
                watch.after(start_the_death_cam).before(crate::game::condition::decide_the_view).run_if(in_state(AppMode::Play)),
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
    looking_through: Option<Res<ThroughCamera>>,
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

    let wanted = match view.at {
        Some(Viewpoint::Camera(camera)) => through(&cameras, camera),
        _ => None,
    };
    match wanted {
        Some(wanted) if looking_through.as_deref() != Some(&wanted) => commands.insert_resource(wanted),
        None if looking_through.is_some() => commands.remove_resource::<ThroughCamera>(),
        _ => {}
    }

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
}
