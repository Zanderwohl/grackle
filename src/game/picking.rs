//! Whether a player has the team or class picker up. Nobody is given a body while they do, since a
//! dead player at a picker is probably changing team or class and does not want to come back yet.
//! Each machine works its own player's out from its pickers; a client sends it to the server,
//! which keeps it on the link.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::net::{has_authority, NetRole};
use crate::game::class_choice::ClassOptions;
use crate::game::class_picker::ClassPicker;
use crate::game::team_choice::{TeamChannel, TeamOptions};
use crate::game::team_picker::TeamPicker;

#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtAPicker(pub bool);

/// On a client's link while that client has a picker up.
#[derive(Component, Clone, Copy, Debug)]
pub struct LinkAtAPicker;

/// Client to server, whenever it changes and on connecting.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickerUp(pub bool);

/// Whether either picker is up. A class picker that bars the way counts even under the team
/// picker, so the frame the team picker hands over to it is not a frame with neither up.
pub fn at_a_picker(team: &TeamPicker, class: &ClassPicker, seats: &TeamOptions, classes: &ClassOptions) -> bool {
    team.is_up(seats) || (!classes.allowed.is_empty() && (class.wanted || ClassPicker::bars_the_way(seats, classes)))
}

pub struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AtAPicker>().add_systems(
            Update,
            (
                // After this frame's offers, so a seat taken this frame is not read as still choosing.
                notice_the_pickers
                    .after(crate::game::team_choice::offer_teams)
                    .after(crate::game::class_choice::offer_classes),
                hear_who_is_at_a_picker.run_if(has_authority),
            )
                .before(crate::game::net_bodies::give_bodies_to_whoever_needs_one),
        );
    }
}

/// Ungated, so leaving Play reads as no picker.
fn notice_the_pickers(
    mode: Option<Res<State<AppMode>>>,
    team: Option<Res<TeamPicker>>,
    class: Option<Res<ClassPicker>>,
    seats: Option<Res<TeamOptions>>,
    classes: Option<Res<ClassOptions>>,
    mut ours: ResMut<AtAPicker>,
    mut wanted_last_frame: Local<bool>,
) {
    let playing = mode.is_some_and(|mode| *mode.get() == AppMode::Play);
    let class = class.as_deref().copied().unwrap_or_default();
    let up = playing
        && at_a_picker(
            &team.as_deref().copied().unwrap_or_default(),
            &class,
            &seats.as_deref().cloned().unwrap_or_default(),
            &classes.as_deref().cloned().unwrap_or_default(),
        );
    // `ask_for` clears `wanted` in the frame it writes `ChooseClass`, which is read and sent only
    // next frame, so the hold stays a frame longer. Every other way a picker closes waits on the
    // server, which has the choice by then.
    ours.set_if_neq(AtAPicker(up || (playing && *wanted_last_frame)));
    *wanted_last_frame = class.wanted;
}

fn hear_who_is_at_a_picker(mut commands: Commands, mut receivers: Query<(Entity, &mut MessageReceiver<PickerUp>), With<ClientOf>>) {
    for (link, mut receiver) in &mut receivers {
        let Some(latest) = receiver.receive().last() else { continue };
        match latest.0 {
            true => commands.entity(link).insert(LinkAtAPicker),
            false => commands.entity(link).remove::<LinkAtAPicker>(),
        };
    }
}

/// Added after `TeamChoiceSyncPlugin`, whose channel it uses.
pub struct PickingSyncPlugin;

impl Plugin for PickingSyncPlugin {
    fn build(&self, app: &mut App) {
        app.register_message::<PickerUp>().add_direction(NetworkDirection::ClientToServer);
        // After the choices, on the same ordered channel, so the server never hears a picker
        // close before the choice that closed it.
        app.add_systems(
            Update,
            send_whether_we_are_at_a_picker
                .after(notice_the_pickers)
                .after(crate::game::team_choice::send_our_choice)
                .after(crate::game::class_choice::send_our_choice),
        );
    }
}

fn send_whether_we_are_at_a_picker(
    role: Res<NetRole>,
    ours: Res<AtAPicker>,
    mut clients: Query<(Ref<Connected>, &mut MessageSender<PickerUp>), With<Client>>,
) {
    if role.is_authority() {
        return;
    }
    for (connected, mut sender) in &mut clients {
        if ours.is_changed() || connected.is_added() {
            sender.send::<TeamChannel>(PickerUp(ours.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::Class;
    use crate::common::team::Team;
    use crate::game::respawn::Seat;

    fn seated() -> TeamOptions {
        TeamOptions { seat: Seat::Playing(Some(Team::Red)), ..default() }
    }

    fn classes(chosen: Option<Class>) -> ClassOptions {
        ClassOptions { allowed: vec![Class::Mercenary, Class::Scout], chosen, color: None }
    }

    #[test]
    fn either_picker_up_counts() {
        let none = (TeamPicker::default(), ClassPicker::default());
        assert!(!at_a_picker(&none.0, &none.1, &seated(), &classes(Some(Class::Scout))));
        let team = TeamPicker { wanted: true, asked_from: None };
        assert!(at_a_picker(&team, &none.1, &seated(), &classes(Some(Class::Scout))));
        let class = ClassPicker { wanted: true };
        assert!(at_a_picker(&none.0, &class, &seated(), &classes(Some(Class::Scout))));
    }

    /// Breaks if the frame the team picker closes on its way to a class picker that bars the way
    /// reads as no picker, which would hand the body out before a class is chosen again.
    #[test]
    fn a_class_picker_still_to_come_counts() {
        let none = (TeamPicker::default(), ClassPicker::default());
        assert!(at_a_picker(&none.0, &none.1, &seated(), &classes(None)));
    }

    #[test]
    fn nothing_offered_has_no_class_picker() {
        let none = (TeamPicker::default(), ClassPicker { wanted: true });
        let one = ClassOptions { allowed: Vec::new(), chosen: None, color: None };
        assert!(!at_a_picker(&none.0, &none.1, &seated(), &one));
    }
}
