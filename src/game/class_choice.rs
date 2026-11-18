//! Choosing a class. Each game mode lists the classes it allows (`modes.toml`); the authority
//! offers them, keeps each person's choice on their [`Identity`], and stands their next body up
//! as it. The picker that asks is [`crate::game::class_picker`].
//!
//! - A person is owed a body only once their class is known: chosen, or the mode's only one
//!   ([`ModeRules::class_for`]).
//! - A choice made with a living body waits for that body to die; one made while dead or
//!   waiting is the next body's class. Nothing is killed for it, unlike a change of team.
//! - Choices are forgotten as each match starts, so everybody chooses again.
//! - Bots have no picker: each seat rolls one at random among the classes it could fight with
//!   ([`ModeRules::random_class_for_a_bot`]) and keeps it until the next match.
//!
//! [`ModeRules::class_for`]: crate::common::game_rules::ModeRules::class_for
//! [`ModeRules::random_class_for_a_bot`]: crate::common::game_rules::ModeRules::random_class_for_a_bot

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::class::Class;
use crate::common::damage::PlayerId;
use crate::common::game_rules::GameRules;
use crate::common::match_phase::MatchPhase;
use crate::common::net::{has_authority, NetRole};
use crate::game::death::Dying;
use crate::game::player::Player;
use crate::common::player_color::PlayerColor;
use crate::game::respawn::{HueStart, Identity, OurIdentity};
use crate::game::team_choice::{Chair, TeamChannel};

/// Written by the picker on every machine; the authority answers its own and a client's goes
/// over the wire.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChooseClass(pub Class);

/// What this machine's player may choose and has chosen. On the authority it is worked out
/// here; on a client it is the server's latest offer.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ClassOptions {
    /// In the picker's order.
    pub allowed: Vec<Class>,
    /// What the next body will be, when the mode still allows it.
    pub chosen: Option<Class>,
    /// The player's own color, which the portraits wear where no team outranks it. A client
    /// with no body has no other way to learn it.
    pub color: Option<PlayerColor>,
}

impl ClassOptions {
    /// Whether a player must answer the picker before they are given a body.
    pub fn must_choose(&self) -> bool {
        self.chosen.is_none() && self.allowed.len() > 1
    }
}

pub fn options(who: &Identity, rules: &GameRules, start: Option<&HueStart>) -> ClassOptions {
    let allowed = rules.allowed_classes();
    let chosen = who.class.filter(|class| allowed.contains(class));
    ClassOptions { allowed, chosen, color: start.map(|start| who.color(start)) }
}

pub struct ClassChoicePlugin;

impl Plugin for ClassChoicePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ChooseClass>()
            .init_resource::<ClassOptions>()
            .add_systems(
                Update,
                // Before the bodies, so a first choice is stood up that frame. Ungated by Play so
                // requests outside it are drained.
                (seat_whoever_chose_a_class, offer_classes.run_if(in_state(AppMode::Play)).run_if(offers_may_have_changed))
                    .chain()
                    .before(crate::game::net_bodies::give_bodies_to_whoever_needs_one)
                    .run_if(has_authority),
            )
            .add_systems(OnEnter(MatchPhase::PreGame), forget_the_classes.run_if(has_authority));
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn seat_whoever_chose_a_class(
    mode: Res<State<AppMode>>,
    mut ours: MessageReader<ChooseClass>,
    mut us: Option<ResMut<OurIdentity>>,
    mut links: Query<&mut Identity, With<ClientOf>>,
    mut receivers: Query<(Entity, &mut MessageReceiver<ChooseClass>), With<ClientOf>>,
    rules: Res<GameRules>,
    alive: Query<&PlayerId, (With<Player>, Without<Dying>)>,
) {
    let mut requests: Vec<(Chair, Class)> = ours.read().map(|request| (Chair::Ours, request.0)).collect();
    for (link, mut receiver) in &mut receivers {
        requests.extend(receiver.receive().map(|request| (Chair::Link(link), request.0)));
    }
    if requests.is_empty() || *mode.get() != AppMode::Play {
        return;
    }
    let allowed = rules.allowed_classes();
    for (chair, class) in requests {
        // Read first and written only on a real change: a changed identity wakes every offer.
        let who = match chair {
            Chair::Ours => us.as_deref().map(|us| us.0),
            Chair::Link(link) => links.get(link).ok().copied(),
        };
        let Some(who) = who else { continue };
        if !allowed.contains(&class) {
            info!("{} cannot play the {:?}: the mode does not allow it", who.id, class);
            continue;
        }
        if who.class == Some(class) {
            continue;
        }
        match chair {
            Chair::Ours => us.as_deref_mut().expect("read above").0.class = Some(class),
            Chair::Link(link) => links.get_mut(link).expect("read above").class = Some(class),
        }
        // A toast once there are toasts: documentation/notifications_and_toasts.md.
        match alive.iter().any(|id| *id == who.id) {
            true => info!("{} will play the {:?} from their next spawn", who.id, class),
            false => info!("{} will spawn as the {:?}", who.id, class),
        }
    }
}

/// Bot seats too, so each rolls again.
fn forget_the_classes(mut us: Option<ResMut<OurIdentity>>, mut links: Query<&mut Identity, Or<(With<ClientOf>, With<crate::game::bot::BotSeat>)>>) {
    if let Some(us) = us.as_deref_mut() {
        us.0.class = None;
    }
    for mut who in &mut links {
        who.class = None;
    }
}

fn offers_may_have_changed(
    us: Option<Res<OurIdentity>>,
    identities: Query<(), Changed<Identity>>,
    rules: Res<GameRules>,
    start: Option<Res<HueStart>>,
) -> bool {
    us.is_some_and(|us| us.is_changed()) || !identities.is_empty() || rules.is_changed() || start.is_some_and(|start| start.is_changed())
}

/// On a client's link, written only on change, which is what sends it.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct ClassOffer(pub ClassOptions);

pub(crate) fn offer_classes(
    mut commands: Commands,
    mut ours: ResMut<ClassOptions>,
    us: Option<Res<OurIdentity>>,
    links: Query<(Entity, &Identity, Option<&ClassOffer>), With<ClientOf>>,
    rules: Res<GameRules>,
    start: Option<Res<HueStart>>,
) {
    let start = start.as_deref();
    if let Some(us) = us {
        ours.set_if_neq(options(&us.0, &rules, start));
    }
    for (link, who, last) in &links {
        let offer = options(who, &rules, start);
        if last.is_some_and(|last| last.0 == offer) {
            continue;
        }
        commands.entity(link).insert(ClassOffer(offer));
    }
}

/// The wire half, beside the transport's other plugins. Rides the team picker's channel, which
/// is ordered, so an offer is never overtaken by an older one.
pub struct ClassChoiceSyncPlugin;

impl Plugin for ClassChoiceSyncPlugin {
    fn build(&self, app: &mut App) {
        app.register_message::<ClassOptions>().add_direction(NetworkDirection::ServerToClient);
        app.register_message::<ChooseClass>().add_direction(NetworkDirection::ClientToServer);
        app.add_systems(Update, (send_our_choice, send_offers, take_the_offer));
    }
}

#[allow(clippy::type_complexity)]
fn send_offers(offers: Query<(Entity, &ClassOffer), (With<ClientOf>, Changed<ClassOffer>)>, mut sender: ServerMultiMessageSender) -> Result {
    for (link, offer) in &offers {
        sender.send_to_entities::<_, TeamChannel>(&offer.0, core::iter::once(link))?;
    }
    Ok(())
}

fn send_our_choice(
    role: Res<NetRole>,
    mut ours: MessageReader<ChooseClass>,
    mut clients: Query<&mut MessageSender<ChooseClass>, (With<Client>, With<Connected>)>,
) {
    let requests: Vec<ChooseClass> = ours.read().copied().collect();
    if role.is_authority() {
        return;
    }
    for mut sender in &mut clients {
        for request in &requests {
            sender.send::<TeamChannel>(*request);
        }
    }
}

fn take_the_offer(mut receivers: Query<&mut MessageReceiver<ClassOptions>, With<Client>>, mut ours: ResMut<ClassOptions>) {
    for mut receiver in &mut receivers {
        if let Some(latest) = receiver.receive().last() {
            ours.set_if_neq(latest);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::game_rules::ModeRules;
    use crate::game::respawn::Seat;

    fn world_with_us(classes: Vec<Class>) -> World {
        let mut world = World::new();
        world.insert_resource(State::new(AppMode::Play));
        world.insert_resource(GameRules(ModeRules { classes, ..default() }));
        world.init_resource::<ClassOptions>();
        world.init_resource::<Messages<ChooseClass>>();
        world.insert_resource(OurIdentity(Identity { id: PlayerId(1), seat: Seat::Playing(None), color: 0, class: None }));
        world
    }

    fn ask(world: &mut World, class: Class) {
        world.write_message(ChooseClass(class));
        world.run_system_once(seat_whoever_chose_a_class).unwrap();
    }

    fn our_class(world: &World) -> Option<Class> {
        world.resource::<OurIdentity>().0.class
    }

    #[test]
    fn a_class_the_mode_allows_is_kept_for_the_next_body() {
        let mut world = world_with_us(vec![Class::Mercenary, Class::Scout]);
        ask(&mut world, Class::Scout);
        assert_eq!(our_class(&world), Some(Class::Scout));
    }

    #[test]
    fn a_class_the_mode_leaves_out_is_refused() {
        let mut world = world_with_us(vec![Class::Mercenary, Class::Scout]);
        ask(&mut world, Class::Spy);
        assert_eq!(our_class(&world), None);
    }

    /// Breaks if choosing a class kills the body, as a change of team does.
    #[test]
    fn choosing_with_a_body_leaves_it_standing() {
        let mut world = world_with_us(vec![Class::Mercenary, Class::Scout]);
        let body = world.spawn((Player::default(), PlayerId(1), Class::Mercenary)).id();
        ask(&mut world, Class::Scout);
        assert_eq!(our_class(&world), Some(Class::Scout));
        assert_eq!(world.get::<Class>(body), Some(&Class::Mercenary), "the living body changed class");
    }

    /// Breaks if the picker is forced on a mode of one class, or skipped while a choice is owed.
    #[test]
    fn only_a_mode_of_several_classes_asks() {
        let mut world = world_with_us(vec![Class::Mercenary, Class::Scout]);
        world.run_system_once(offer_classes).unwrap();
        assert!(world.resource::<ClassOptions>().must_choose());
        ask(&mut world, Class::Mercenary);
        world.run_system_once(offer_classes).unwrap();
        assert!(!world.resource::<ClassOptions>().must_choose());

        let mut world = world_with_us(vec![Class::Pyro]);
        world.run_system_once(offer_classes).unwrap();
        assert!(!world.resource::<ClassOptions>().must_choose());
    }

    #[test]
    fn a_new_match_forgets_every_choice() {
        let mut world = world_with_us(vec![Class::Mercenary, Class::Scout]);
        ask(&mut world, Class::Scout);
        world.run_system_once(forget_the_classes).unwrap();
        assert_eq!(our_class(&world), None);
    }
}
