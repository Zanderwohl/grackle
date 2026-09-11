use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::common::net::NetRole;
use crate::get;

/// Whether the process is currently editing a map, modelling a prop, or being
/// a game.
///
/// All of them are one binary sharing one `World` on purpose: teams edit the
/// map between rounds, so the swap has to be a state transition rather than a
/// reload or a second process. Anything that would make the swap slow or lossy
/// is working against the design — see `documentation/sketch.md`.
///
/// Editor systems are gated on `in_state(AppMode::Editor)` at each plugin's
/// `add_systems` call. The feature timeline deliberately is *not*: features
/// keep syncing to entities while playing, which is what makes an edit landing
/// mid-match visible without a reload.
///
/// **`Prop` is authoring, not a match.** It is the modelling surface for the
/// things a map and a class refer to — weapons, first of all — and it is
/// deliberately a mode of the same process rather than a second tool, for the
/// same reason the map editor is: a weapon you can remodel between rounds is
/// only possible if there is no reload between holding one and changing it.
///
/// Across the wire it is indistinguishable from `Editor`: [`MatchMode`] states
/// whether the match is *being played*, and a prop editor is one of the ways
/// of not playing. A client can model a prop while the server is between
/// rounds, and is pulled back to the map editor when the server says the round
/// has ended — which is the same authority rule, not an exception to it.
///
/// [`MatchMode`]: crate::common::match_state::MatchMode
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppMode {
    #[default]
    Editor,
    Prop,
    Play,
}

impl AppMode {
    /// Whether this is a mode somebody is authoring in rather than playing in.
    ///
    /// Asked instead of matching on the variant wherever the question is
    /// really "is the match running", the same way `NetRole::is_authority` is
    /// asked instead of naming `Solo` and `Listen`. A fourth authoring mode
    /// then joins by answering `true` here rather than by being found again in
    /// every match that forgot about it.
    pub fn is_authoring(self) -> bool {
        matches!(self, AppMode::Editor | AppMode::Prop)
    }

    /// Whether this mode is a view of **the map and the match in it**.
    ///
    /// The prop editor is not: it sets the world aside and shows one prop, so
    /// anything that draws a fact about the map or the bodies standing in it
    /// has no business appearing there.
    ///
    /// This exists because `Visibility` cannot answer the question. The prop
    /// editor hides entities, and a **gizmo is not an entity** — it is a
    /// system drawing lines every frame, which no amount of hiding reaches. So
    /// the debug overlays ask this instead, and the two that were drawn
    /// unconditionally (`draw_hitboxes` and `draw_skeletons`) put the animation
    /// grid's sixty bodies' boxes around a weapon.
    ///
    /// A question rather than two named variants at each call site, for the
    /// same reason as [`AppMode::is_authoring`]: a new mode answers it once
    /// here rather than being forgotten by every overlay written before it.
    pub fn shows_the_world(self) -> bool {
        matches!(self, AppMode::Editor | AppMode::Play)
    }

    pub fn name(self) -> String {
        match self {
            AppMode::Editor => get!("app_mode.editor"),
            AppMode::Prop => get!("app_mode.prop"),
            AppMode::Play => get!("app_mode.play"),
        }
    }

    /// Whether this mode can be picked from `current`.
    ///
    /// Split out of the menu so the rule is testable without an egui context,
    /// the way `ConnectDialog::submit` is. There are two clauses and they are
    /// different in kind:
    ///
    /// - **You cannot pick the mode you are in.** `NextState::set` runs the
    ///   transition even for the same state, and `OnEnter(Play)` tears the
    ///   match down and rebuilds it — so re-picking "Play" would restart the
    ///   round.
    /// - **Starting or ending a round needs authority.** A client is shown the
    ///   match the server is running: everyone drops into the editor between
    ///   rounds together and drops back into the round together, which only
    ///   works if one process owns the transition. Swapping between the two
    ///   *authoring* modes is local and needs nothing, which is why the clause
    ///   names `Play` on either side rather than asking whether this is a
    ///   change at all.
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

/// A run condition for "somebody is authoring rather than playing".
///
/// The counterpart to `NetRole::has_authority`, and here for the same reason:
/// `in_state` cannot ask a question, only name a variant, so a system wanted
/// in both authoring modes would otherwise have to name both — and a fourth
/// mode would have to find every one of those places again.
pub fn authoring(mode: Res<State<AppMode>>) -> bool {
    mode.get().is_authoring()
}

/// A run condition for [`AppMode::shows_the_world`] — what the debug overlays
/// that draw the map and the bodies in it are gated on.
pub fn showing_the_world(mode: Res<State<AppMode>>) -> bool {
    mode.get().shows_the_world()
}

/// The menu bar's **Mode** pulldown.
///
/// Here rather than in either editor's panel module because both of them show
/// it and neither owns it: the enum, what its variants are called, and the rule
/// about which can be reached are one thing, and splitting them is how a fourth
/// mode ends up offered from one menu bar and not the other.
///
/// Deliberately **not backed by a keyboard shortcut for the prop editor.** `F5`
/// is the round and `Escape` is the pause menu; a third mode key would be a
/// thing to remember in exchange for saving a menu click on something nobody
/// does mid-fight.
pub fn mode_menu(
    ui: &mut egui::Ui,
    current: AppMode,
    role: Option<&NetRole>,
    request: &mut Option<AppMode>,
) {
    // Absent means no network layer at all, which is a closed game, and a
    // closed game is its own authority — the same reading `toggle_mode` takes.
    let authority = role.is_none_or(|role| role.is_authority());

    ui.menu_button(get!("app_mode.menu.title"), |ui| {
        // What we are, before what we could become — the same shape the
        // network menu has, and for the same reason: "which mode am I in"
        // should not be answered by noticing which entry is greyed out.
        ui.label(get!("app_mode.menu.current", "mode", current.name()));
        ui.separator();

        for mode in [AppMode::Editor, AppMode::Prop, AppMode::Play] {
            let enabled = mode.selectable_from(current, authority);
            let entry = ui.add_enabled(enabled, egui::Button::new(mode.name()));
            if entry.clicked() {
                ui.close_kind(UiKind::Menu);
                *request = Some(mode);
            }
            // Greyed out for two quite different reasons, and a player who
            // cannot start a round deserves to be told which. Without this,
            // a client's Play entry looks identical to the one you are
            // already standing in.
            if !enabled && mode != current {
                entry.on_disabled_hover_text(get!("app_mode.menu.server_decides"));
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `NextState::set` runs the transition even for the state already in
    /// force, and `OnEnter(Play)` tears the match down and rebuilds it — so an
    /// entry for the mode you are standing in is a button that restarts your
    /// round. That is why the menu greys it out rather than relying on every
    /// caller to compare first.
    #[test]
    fn the_mode_you_are_in_is_never_offered() {
        for mode in [AppMode::Editor, AppMode::Prop, AppMode::Play] {
            assert!(!mode.selectable_from(mode, true), "{mode:?} offered itself");
        }
    }

    /// The prop editor sets the world aside and shows one prop, so a debug
    /// overlay about the map or the bodies in it does not belong there.
    ///
    /// Worth a test of its own because the failure is invisible in reverse: a
    /// gizmo is not an entity, so nothing the prop editor hides reaches one,
    /// and the symptom was the animation grid's sixty bodies' hitboxes drawn
    /// around a weapon. An overlay that forgets to ask this is an overlay that
    /// leaks, silently, into every mode written after it.
    #[test]
    fn the_prop_editor_is_not_a_view_of_the_world() {
        assert!(AppMode::Editor.shows_the_world());
        assert!(AppMode::Play.shows_the_world());
        assert!(!AppMode::Prop.shows_the_world());
    }

    /// Swapping which thing you are authoring is local and concerns nobody
    /// else, so it works on a client exactly as it does on a host.
    #[test]
    fn a_client_can_still_swap_between_the_authoring_modes() {
        assert!(AppMode::Prop.selectable_from(AppMode::Editor, false));
        assert!(AppMode::Editor.selectable_from(AppMode::Prop, false));
    }

    /// Starting *or* ending a round is the server's call, and both halves
    /// matter: a client that could not start one but could leave one would
    /// walk out of the round everybody else is still playing.
    #[test]
    fn only_the_authority_starts_or_ends_a_round() {
        for authoring in [AppMode::Editor, AppMode::Prop] {
            assert!(AppMode::Play.selectable_from(authoring, true));
            assert!(!AppMode::Play.selectable_from(authoring, false));
            assert!(authoring.selectable_from(AppMode::Play, true));
            assert!(!authoring.selectable_from(AppMode::Play, false));
        }
    }
}
