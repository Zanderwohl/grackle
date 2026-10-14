use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::common::net::NetRole;
use crate::get;

/// One binary and one `World` for every mode, so swapping is a state
/// transition rather than a reload. See CLAUDE.md, "Editor and game in one
/// process".
///
/// Across the wire only "playing or not" exists ([`MatchMode`]); the authoring
/// modes are indistinguishable there.
///
/// [`MatchMode`]: crate::common::match_state::MatchMode
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppMode {
    #[default]
    Editor,
    Prop,
    /// The timeline over a weapon's clips. Its own mode rather than a prop
    /// editor tab because it wants the whole window and a body.
    Animation,
    Play,
}

impl AppMode {
    /// Ask this rather than matching variants, so a new authoring mode joins
    /// in one place.
    pub fn is_authoring(self) -> bool {
        matches!(self, AppMode::Editor | AppMode::Prop | AppMode::Animation)
    }

    /// For gizmo overlays: the prop modes hide the world's entities, but a
    /// gizmo is not an entity, so hiding does not reach it.
    pub fn shows_the_world(self) -> bool {
        matches!(self, AppMode::Editor | AppMode::Play)
    }

    /// The two prop modes share one scene: stage, lights, framed cameras and
    /// the hidden world.
    pub fn shows_a_prop(self) -> bool {
        matches!(self, AppMode::Prop | AppMode::Animation)
    }

    pub fn name(self) -> String {
        match self {
            AppMode::Editor => get!("app_mode.editor"),
            AppMode::Prop => get!("app_mode.prop"),
            AppMode::Animation => get!("app_mode.animation"),
            AppMode::Play => get!("app_mode.play"),
        }
    }

    /// The current mode is never selectable: `NextState::set` runs the
    /// transition even for the same state, and re-entering Play restarts the
    /// round. Entering or leaving Play needs authority; swapping between
    /// authoring modes is local.
    pub fn selectable_from(self, current: AppMode, authority: bool) -> bool {
        if self == current {
            return false;
        }
        if self == AppMode::Play || current == AppMode::Play {
            return authority;
        }
        true
    }
}

/// `in_state` can only name one variant; this covers every authoring mode.
pub fn authoring(mode: Res<State<AppMode>>) -> bool {
    mode.get().is_authoring()
}

pub fn showing_a_prop(mode: Res<State<AppMode>>) -> bool {
    mode.get().shows_a_prop()
}

pub fn showing_the_world(mode: Res<State<AppMode>>) -> bool {
    mode.get().shows_the_world()
}

/// The menu bar's Mode pulldown. Beside the enum because both editors show
/// it, so a new mode cannot be offered from one menu bar and not the other.
pub fn mode_menu(
    ui: &mut egui::Ui,
    current: AppMode,
    role: Option<&NetRole>,
    request: &mut Option<AppMode>,
) {
    // No network layer is a closed game, which is its own authority.
    let authority = role.is_none_or(|role| role.is_authority());

    ui.menu_button(get!("app_mode.menu.title"), |ui| {
        for mode in [AppMode::Editor, AppMode::Prop, AppMode::Animation, AppMode::Play] {
            let enabled = mode.selectable_from(current, authority);
            let entry = ui.add_enabled(enabled, egui::Button::new(mode.name()));
            if entry.clicked() {
                ui.close_kind(UiKind::Menu);
                *request = Some(mode);
            }
            // Distinguishes "the server decides" from "you are already here".
            if !enabled && mode != current {
                entry.on_disabled_hover_text(get!("app_mode.menu.server_decides"));
            }
        }
    });
}

/// `--play`. A resource rather than a different initial state, so starting in
/// Play still goes through `OnEnter(AppMode::Play)`.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInPlay(pub bool);

/// Frames [`start_in_play`] waits for rooms before starting without any.
const GIVE_UP_AFTER: u32 = 8;

/// Waits for rooms to exist: features become entities only after
/// `sync_entities`, and entering Play earlier builds an empty collision world.
pub fn start_in_play(
    start: Res<StartInPlay>,
    mut next: ResMut<NextState<AppMode>>,
    mut waited: Local<u32>,
    mut done: Local<bool>,
    rooms: Query<(), With<crate::tool::room::Room>>,
) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if the current mode is offered, which for Play restarts the round.
    #[test]
    fn the_mode_you_are_in_is_never_offered() {
        for mode in [AppMode::Editor, AppMode::Prop, AppMode::Animation, AppMode::Play] {
            assert!(!mode.selectable_from(mode, true), "{mode:?} offered itself");
        }
    }

    /// Fails if map gizmo overlays would draw in the prop modes.
    #[test]
    fn the_prop_editor_is_not_a_view_of_the_world() {
        assert!(AppMode::Editor.shows_the_world());
        assert!(AppMode::Play.shows_the_world());
        assert!(!AppMode::Prop.shows_the_world());
        assert!(!AppMode::Animation.shows_the_world());
    }

    #[test]
    fn both_prop_modes_are_a_view_of_a_prop() {
        assert!(AppMode::Prop.shows_a_prop());
        assert!(AppMode::Animation.shows_a_prop());
        assert!(!AppMode::Editor.shows_a_prop());
        assert!(!AppMode::Play.shows_a_prop());
    }

    #[test]
    fn a_client_can_still_swap_between_the_authoring_modes() {
        assert!(AppMode::Prop.selectable_from(AppMode::Editor, false));
        assert!(AppMode::Editor.selectable_from(AppMode::Prop, false));
    }

    /// Both halves: a client that could leave a round would walk out of the
    /// one everybody else is playing.
    #[test]
    fn only_the_authority_starts_or_ends_a_round() {
        for authoring in [AppMode::Editor, AppMode::Prop, AppMode::Animation] {
            assert!(AppMode::Play.selectable_from(authoring, true));
            assert!(!AppMode::Play.selectable_from(authoring, false));
            assert!(authoring.selectable_from(AppMode::Play, true));
            assert!(!authoring.selectable_from(AppMode::Play, false));
        }
    }
}
