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
