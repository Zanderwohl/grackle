//! Handing the weapon catalogue to everybody who connects.
//!
//! The server reads its weapons off disk and every client is *told* what they
//! are. That is the whole point of the catalogue being data: a server can ship
//! its own weapons and nobody has to have the same files.
//!
//! Shaped on [`crate::common::map_sync`], because it is the same problem — one
//! description of one thing, held by the authority, replaced wholesale
//! elsewhere. Two differences are worth knowing:
//!
//! - **The payload is the typed value, not a bag of bytes.** `MapSnapshot`
//!   carries `Vec<u8>` only because a `Feature` is a `typetag` trait object
//!   that cannot be cloned into a message. Nothing here is a trait object.
//! - **There is no resend interval.** The map needs one because dragging a
//!   room edits the timeline at frame rate. Nothing edits a weapon at frame
//!   rate yet, and a floor added for symmetry would be a number nobody can
//!   explain.
//!
//! A client's `WeaponId`s and its catalogue arrive on different channels, so a
//! body can be holding a weapon the catalogue has not described yet. That is
//! not a case to prevent — it is why nothing caches a resolution: an id is
//! looked up every time it is drawn, and an id nobody can resolve draws as
//! nothing rather than as an error. Key on absence, not on arrival.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::net::NetRole;
use crate::common::weapon::WeaponCatalogue;

/// Ordered and reliable, like the map: a catalogue is state, and the last one
/// sent is the one that is true.
pub struct WeaponChannel;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CatalogueSnapshot(pub WeaponCatalogue);

/// What was last put on the wire.
///
/// Compared against rather than asking `Res::is_changed`, because change
/// detection counts a resource's *insertion* as a change — the same trap that
/// grabs the cursor inside the editor on the first frame of the process. The
/// startup load would otherwise broadcast to an empty room, and again to the
/// first person who connected.
#[derive(Resource, Default)]
struct LastCatalogueSent(WeaponCatalogue);

pub struct WeaponSyncPlugin;

impl Plugin for WeaponSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<WeaponChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        app.register_message::<CatalogueSnapshot>()
            .add_direction(NetworkDirection::ServerToClient);

        app.init_resource::<LastCatalogueSent>().add_systems(
            Update,
            (
                send_catalogue_to_new_clients,
                broadcast_catalogue_changes,
                adopt_the_servers_catalogue,
            ),
        );
    }
}

/// Hand the catalogue over the moment somebody connects.
///
/// Before anything else can matter: a client is given a body within a frame or
/// two of connecting, and a body whose weapons nobody can name is a body the
/// HUD draws blank.
fn send_catalogue_to_new_clients(
    catalogue: Res<WeaponCatalogue>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let message = CatalogueSnapshot(catalogue.clone());
    for client in &joined {
        info!("Sending {} weapons to a new client", catalogue.weapons.len());
        sender.send_to_entities::<_, WeaponChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Tell everybody when the catalogue changes under them.
///
/// Nothing edits weapons at runtime today; this is here because a server that
/// reloaded its files mid-match would otherwise be the only machine playing
/// the new game.
fn broadcast_catalogue_changes(
    catalogue: Res<WeaponCatalogue>,
    server: Option<Single<&Server>>,
    mut last: ResMut<LastCatalogueSent>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    if catalogue.as_ref() == &last.0 {
        return Ok(());
    }

    last.0 = catalogue.clone();
    info!("Weapons changed; sending {} of them to everyone", catalogue.weapons.len());
    sender.send::<_, WeaponChannel>(
        &CatalogueSnapshot(catalogue.clone()),
        &server,
        &NetworkTarget::All,
    )?;
    Ok(())
}

/// Replace our catalogue with the server's.
///
/// Only on a client, and the check is on authority rather than on holding a
/// receiver: a listen server holds one too, and adopting its own catalogue
/// back would be a server taking instructions from itself.
fn adopt_the_servers_catalogue(
    role: Res<NetRole>,
    mut receivers: Query<&mut MessageReceiver<CatalogueSnapshot>>,
    mut catalogue: ResMut<WeaponCatalogue>,
) {
    if role.is_authority() {
        return;
    }

    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            // Declined rather than taken, because writing the resource marks
            // it changed and everything watching would react to a catalogue
            // that is the one it already had.
            if message.0 == *catalogue {
                continue;
            }
            info!("Adopting the server's weapons: {} of them", message.0.weapons.len());
            *catalogue = message.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalogue;

    /// Everything a weapon is made of survives the trip, including every kind
    /// of action. A variant with a broken `Serialize` fails here rather than
    /// as a weapon that does nothing on one machine.
    ///
    /// Through JSON rather than through the wire's own encoding, because JSON
    /// is the stricter of the two — it refuses a map key that is not a string,
    /// which is exactly the mistake the catalogue's lists exist to make
    /// impossible. Something that survives this survives anything Lightyear
    /// does to it.
    #[test]
    fn a_catalogue_survives_the_round_trip() {
        let catalogue = default_catalogue();
        let text = serde_json::to_string(&CatalogueSnapshot(catalogue.clone()))
            .expect("a catalogue would not encode");
        let there_and_back: CatalogueSnapshot =
            serde_json::from_str(&text).expect("a catalogue would not decode");

        assert_eq!(there_and_back.0, catalogue);
    }

    /// The sibling of `only_an_edit_changes_the_bytes`: what decides whether
    /// to send is the value, so inserting the resource at startup is not a
    /// broadcast, and neither is reading the same files twice.
    ///
    /// This is also what pins the catalogue's lists to a stable order. With
    /// maps inside it, loading the same files twice would compare unequal
    /// often enough to be maddening and rarely enough to ship.
    #[test]
    fn only_a_change_puts_the_catalogue_on_the_wire() {
        let mut last = LastCatalogueSent(default_catalogue());
        let again = default_catalogue();
        assert_eq!(last.0, again, "loading the same files twice gave two catalogues");

        last.0.weapons.clear();
        assert_ne!(last.0, again, "emptying the catalogue was not a change");
    }

    /// A catalogue identical to the one already held is declined, because
    /// taking it marks the resource changed and everything watching reacts to
    /// nothing having happened.
    ///
    /// The decision is the comparison, which is what this states; whether the
    /// message arrived at all is Lightyear's business and is covered by the
    /// protocol registering.
    #[test]
    fn a_client_declines_a_catalogue_it_already_has() {
        let held = default_catalogue();
        let arriving = CatalogueSnapshot(default_catalogue());
        assert!(arriving.0 == held, "an identical catalogue would have been adopted again");
    }
}
