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

/// The catalogue as JSON bytes.
///
/// **Bytes, not the typed value, and that is not a preference.** Lightyear
/// puts a message on the wire with postcard, and postcard refuses
/// `deserialize_any` — which is exactly what an internally tagged enum needs.
/// `WeaponAction`, `Cadence` and `Cost` are all `#[serde(tag = "kind")]`,
/// because that is what makes `weapons.toml` readable, so the whole catalogue
/// is undecodable at the far end.
///
/// It failed *silently*: the send succeeded, the receive errored inside
/// Lightyear, and every machine already had the same catalogue from its own
/// files — so the sync was a no-op in the only arrangement anybody ran. The
/// same reasoning is why [`crate::common::map_sync`] carries bytes, for a
/// different reason (a `typetag` trait object).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CatalogueSnapshot(pub Vec<u8>);

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

    let Some(message) = encode(&catalogue) else { return Ok(()) };
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
            let sent: WeaponCatalogue = match serde_json::from_slice(&message.0) {
                Ok(sent) => sent,
                // Logged rather than fatal: a catalogue that will not decode
                // leaves us with our own, which is a game that plays slightly
                // wrong rather than a client that falls over.
                Err(e) => {
                    error!("The server's weapon catalogue will not decode: {e}");
                    continue;
                }
            };
            // Declined rather than taken, because writing the resource marks
            // it changed and everything watching would react to a catalogue
            // that is the one it already had.
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
        let sent = encode(&catalogue).expect("a catalogue would not encode");
        let there_and_back: WeaponCatalogue =
            serde_json::from_slice(&sent.0).expect("a catalogue would not decode");

        assert_eq!(there_and_back, catalogue);
    }

    /// **The wire cannot carry an internally tagged enum**, which is why the
    /// payload is bytes.
    ///
    /// Lightyear encodes a message with postcard, and postcard refuses
    /// `deserialize_any` — what `#[serde(tag = "kind")]` needs to read itself
    /// back. `WeaponAction` is tagged that way because it is what makes
    /// `weapons.toml` readable, so the catalogue could never decode at the far
    /// end.
    ///
    /// It failed in the one way nobody notices: the send succeeded, the
    /// receive errored inside Lightyear, and both machines already had the
    /// same catalogue from their own files. This asserts the failure directly
    /// so that putting the typed value back on the wire fails here instead of
    /// on somebody's server.
    #[test]
    fn the_wire_cannot_carry_an_internally_tagged_enum() {
        let catalogue = default_catalogue();

        // What is actually sent: bytes, which postcard is perfectly happy with.
        let sent = encode(&catalogue).expect("a catalogue would not encode");
        let wire = postcard::to_allocvec(&sent).expect("bytes go on the wire");
        let back: CatalogueSnapshot =
            postcard::from_bytes(&wire).expect("and come back off it");
        assert_eq!(back, sent);

        // And what must not be: the typed value, which encodes and then cannot
        // be read.
        let naive = postcard::to_allocvec(&catalogue).expect("postcard will encode it");
        assert!(
            postcard::from_bytes::<WeaponCatalogue>(&naive).is_err(),
            "postcard decoded an internally tagged enum; the payload could be the typed \
             value again, and this test has stopped earning its place",
        );
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
        let arriving = encode(&default_catalogue()).expect("a catalogue would not encode");
        let decoded: WeaponCatalogue =
            serde_json::from_slice(&arriving.0).expect("a catalogue would not decode");
        assert!(decoded == held, "an identical catalogue would have been adopted again");
    }
}
