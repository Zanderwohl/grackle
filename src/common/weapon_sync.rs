//! Handing the server's weapon catalogue to every client, so a server can
//! ship its own weapons. Shaped like [`crate::common::map_sync`], but with no
//! resend interval, since nothing edits weapons at frame rate.
//!
//! A body's `WeaponId`s and the catalogue arrive on different channels, so an
//! id may not resolve yet; nothing caches a resolution.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::net::NetRole;
use crate::common::weapon::WeaponCatalogue;

/// Ordered and reliable: the last catalogue sent is the true one.
pub struct WeaponChannel;

/// JSON bytes, not the typed value: Lightyear encodes with postcard, which
/// refuses `deserialize_any`, and the internally tagged enums
/// (`#[serde(tag = "kind")]`) need it. The failure is silent: the send
/// succeeds and the receive errors inside Lightyear.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CatalogueSnapshot(pub Vec<u8>);

/// Compared against instead of `Res::is_changed`, which counts the startup
/// insertion as a change.
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

fn send_catalogue_to_new_clients(
    catalogue: Res<WeaponCatalogue>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let Some(message) = encode(&catalogue) else { return Ok(()) };
    for client in &joined {
        info!("Sending {} weapons to a new client", catalogue.weapons.len());
        sender.send_to_entities::<_, WeaponChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Nothing edits weapons at runtime yet; this covers a server reloading its
/// files mid-match.
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
    let Some(message) = encode(&catalogue) else { return Ok(()) };
    info!("Weapons changed; sending {} of them to everyone", catalogue.weapons.len());
    sender.send::<_, WeaponChannel>(&message, &server, &NetworkTarget::All)?;
    Ok(())
}

fn encode(catalogue: &WeaponCatalogue) -> Option<CatalogueSnapshot> {
    match serde_json::to_vec(catalogue) {
        Ok(bytes) => Some(CatalogueSnapshot(bytes)),
        Err(e) => {
            error!("The weapon catalogue will not encode: {e}");
            None
        }
    }
}

/// Gated on authority rather than on holding a receiver, which a listen
/// server also has.
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
            let sent: WeaponCatalogue = match serde_json::from_slice(&message.0) {
                Ok(sent) => sent,
                // Not fatal: we keep our own catalogue.
                Err(e) => {
                    error!("The server's weapon catalogue will not decode: {e}");
                    continue;
                }
            };
            // Writing would mark the resource changed for nothing.
            if sent == *catalogue {
                continue;
            }
            info!("Adopting the server's weapons: {} of them", sent.weapons.len());
            *catalogue = sent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalogue;

    /// Fails if any part of a weapon does not survive the JSON payload.
    #[test]
    fn a_catalogue_survives_the_round_trip() {
        let catalogue = default_catalogue();
        let sent = encode(&catalogue).expect("a catalogue would not encode");
        let there_and_back: WeaponCatalogue =
            serde_json::from_slice(&sent.0).expect("a catalogue would not decode");

        assert_eq!(there_and_back, catalogue);
    }

    /// Fails if the payload stops being bytes that postcard round-trips, and
    /// pins that the typed catalogue cannot decode through postcard. See
    /// [`CatalogueSnapshot`].
    #[test]
    fn the_wire_cannot_carry_an_internally_tagged_enum() {
        let catalogue = default_catalogue();

        let sent = encode(&catalogue).expect("a catalogue would not encode");
        let wire = postcard::to_allocvec(&sent).expect("bytes go on the wire");
        let back: CatalogueSnapshot =
            postcard::from_bytes(&wire).expect("and come back off it");
        assert_eq!(back, sent);

        let naive = postcard::to_allocvec(&catalogue).expect("postcard will encode it");
        assert!(
            postcard::from_bytes::<WeaponCatalogue>(&naive).is_err(),
            "postcard decoded an internally tagged enum; the payload could be the typed \
             value again, and this test has stopped earning its place",
        );
    }

    /// Fails if two loads of the same files compare unequal (hash ordering),
    /// which would broadcast the catalogue repeatedly.
    #[test]
    fn only_a_change_puts_the_catalogue_on_the_wire() {
        let mut last = LastCatalogueSent(default_catalogue());
        let again = default_catalogue();
        assert_eq!(last.0, again, "loading the same files twice gave two catalogues");

        last.0.weapons.clear();
        assert_ne!(last.0, again, "emptying the catalogue was not a change");
    }

    #[test]
    fn a_client_declines_a_catalogue_it_already_has() {
        let held = default_catalogue();
        let arriving = encode(&default_catalogue()).expect("a catalogue would not encode");
        let decoded: WeaponCatalogue =
            serde_json::from_slice(&arriving.0).expect("a catalogue would not decode");
        assert!(decoded == held, "an identical catalogue would have been adopted again");
    }
}
