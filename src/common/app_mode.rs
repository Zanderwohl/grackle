use bevy::prelude::*;

/// Whether the process is currently being an editor or being a game.
///
/// The two are one binary sharing one `World` on purpose: teams edit the map
/// between rounds, so the swap has to be a state transition rather than a
/// reload or a second process. Anything that would make the swap slow or lossy
/// is working against the design — see `documentation/sketch.md`.
///
/// Editor systems are gated on `in_state(AppMode::Editor)` at each plugin's
/// `add_systems` call. The feature timeline deliberately is *not*: features
/// keep syncing to entities while playing, which is what makes an edit landing
/// mid-match visible without a reload.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppMode {
    #[default]
    Editor,
    Play,
}

/// Whether `--play` was given.
///
/// A resource rather than a different initial state, so that starting in Play
/// goes through the same `OnEnter(AppMode::Play)` transition every other way
/// in does. A state set at startup would skip it, and everything a round needs
/// — the reset, the collision rebuild, the body — happens there.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInPlay(pub bool);

/// Drop into a round at startup if the command line asked for one.
///
/// Waits for the map to have reached the world first. The blueprint is loaded
/// in `Startup` but its features do not become entities until `sync_entities`
/// has run and the rooms are not baked until a frame after that, so a
/// transition made at startup enters a round whose collision world is empty —
/// the body falls through the floor of a map that is about to appear.
///
/// The frame count is a bound rather than the mechanism: a map with no rooms
/// in it should still start, just without waiting forever for a floor that is
/// never coming.
pub fn start_in_play(
    start: Res<StartInPlay>,
    mut next: ResMut<NextState<AppMode>>,
    mut waited: Local<u32>,
    mut done: Local<bool>,
    rooms: Query<(), With<crate::tool::room::Room>>,
) {
    const GIVE_UP_AFTER: u32 = 8;

    if !start.0 || *done {
        return;
    }
    *waited += 1;
    if rooms.is_empty() && *waited < GIVE_UP_AFTER {
        return;
    }
    if rooms.is_empty() {
        warn!("Starting in Play with no rooms baked; the map may be empty");
    }
    info!("Starting in Play, as asked");
    next.set(AppMode::Play);
    *done = true;
}
