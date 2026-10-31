//! Watching a match from [`Seat::Spectating`]: no body, and a view the left
//! and right mouse buttons step forward and back through, wrapping — every
//! spawn point, then every living body's eye. Every machine works its list
//! out from what it can see; the seat is the server's, read off
//! [`TeamOptions`].

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::death::Dying;
use crate::game::pause_menu::MouseHeld;
use crate::game::player::{start_the_death_cam, DeathCam, InputLatch, LocalPlayer, OurLastBody, Player, Watched};
use crate::game::respawn::Seat;
use crate::game::team_choice::TeamOptions;
use crate::game::team_picker::{overlook, OVERLOOK_PITCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewpoint {
    /// Overlooked as the team picker overlooks one.
    Spawn(Entity),
    /// Out of that player's body, looking where they look.
    Eye(PlayerId),
}

/// Spawns first, by index, then bodies by id, so the order does not hang on
/// archetypes. Not `Entity`'s own order, which is not its index's.
pub fn viewpoints(spawns: impl IntoIterator<Item = Entity>, bodies: impl IntoIterator<Item = PlayerId>) -> Vec<Viewpoint> {
    let mut spawns: Vec<Entity> = spawns.into_iter().collect();
    spawns.sort_by_key(|spawn| spawn.index());
    let mut bodies: Vec<PlayerId> = bodies.into_iter().collect();
    bodies.sort();
    spawns.into_iter().map(Viewpoint::Spawn).chain(bodies.into_iter().map(Viewpoint::Eye)).collect()
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

pub struct SpectatePlugin;

impl Plugin for SpectatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpectatorView>()
            .init_resource::<MouseHeld>()
            .init_resource::<TeamOptions>()
            .add_systems(
                Update,
                // After the death cam, whose commands this overrides; before the view is decided.
                watch.after(start_the_death_cam).before(crate::game::condition::decide_the_view).run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), |mut view: ResMut<SpectatorView>| *view = SpectatorView::default());
    }
}

/// The view while spectating: a [`DeathCam`] over a spawn, or [`Watched`] on
/// one body for `place_camera` to look out of. The clicks are only read
/// here; with no body, nothing fires.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn watch(
    mut commands: Commands,
    offer: Res<TeamOptions>,
    buttons: Res<ButtonInput<MouseButton>>,
    held: Res<MouseHeld>,
    mut view: ResMut<SpectatorView>,
    ours: Query<(), With<LocalPlayer>>,
    spawns: Query<(Entity, &Transform), With<SpawnPointMarker>>,
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
    let list = viewpoints(spawns.iter().map(|(spawn, _)| spawn), bodies.iter().map(|(_, id)| *id));
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
    fn spawns_come_first_then_bodies_by_id() {
        let list = viewpoints([spawn(9), spawn(3)], [PlayerId(7), PlayerId(2)]);
        assert_eq!(
            list,
            vec![Viewpoint::Spawn(spawn(3)), Viewpoint::Spawn(spawn(9)), Viewpoint::Eye(PlayerId(2)), Viewpoint::Eye(PlayerId(7))]
        );
    }

    #[test]
    fn left_steps_forward_and_right_back_wrapping_at_the_ends() {
        let list = viewpoints([spawn(1)], [PlayerId(1), PlayerId(2)]);
        let mut view = SpectatorView::default();
        view.follow(&list, 0);
        assert_eq!(view.at, Some(Viewpoint::Spawn(spawn(1))), "started anywhere but the first");

        let mut seen = Vec::new();
        for _ in 0..3 {
            view.follow(&list, 1);
            seen.push(view.at.unwrap());
        }
        assert_eq!(seen, vec![Viewpoint::Eye(PlayerId(1)), Viewpoint::Eye(PlayerId(2)), Viewpoint::Spawn(spawn(1))]);

        view.follow(&list, -1);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(2))), "back from the first did not wrap to the last");
    }

    /// Breaks if the view follows a place in the list rather than a target.
    #[test]
    fn the_view_stays_on_its_target_as_the_list_changes() {
        let mut view = SpectatorView::default();
        view.follow(&viewpoints([spawn(1)], [PlayerId(5)]), 1);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(5))));

        view.follow(&viewpoints([spawn(1)], [PlayerId(2), PlayerId(5)]), 0);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(5))), "somebody joining moved the view");

        view.follow(&viewpoints([spawn(1)], [PlayerId(2)]), 0);
        assert_eq!(view.at, Some(Viewpoint::Eye(PlayerId(2))), "a death left the view on nobody");

        view.follow(&[], 0);
        assert_eq!(view.at, None);
    }
}
