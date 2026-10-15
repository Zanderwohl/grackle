//! Handing the server's pack tables — the weapon catalogue and the class
//! builds — to every client, so a server can ship its own and nobody else
//! needs the files. Shaped like [`crate::common::map_sync`], but with no
//! resend interval, since nothing edits these at frame rate.
//!
//! A body's ids and the tables arrive on different channels, so an id may not
//! resolve yet; nothing caches a resolution.

use std::marker::PhantomData;

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::common::class::ClassBuilds;
use crate::common::net::{is_hosting, is_remote_client};
use crate::common::weapon::WeaponCatalogue;

/// A resource the authority reads from its pack and every client adopts.
pub trait PackTable:
    Resource<Mutability = bevy::ecs::component::Mutable>
    + Clone
    + PartialEq
    + Default
    + Serialize
    + DeserializeOwned
{
    /// For the log.
    const WHAT: &'static str;
    fn len(&self) -> usize;
}

impl PackTable for WeaponCatalogue {
    const WHAT: &'static str = "weapons";
    fn len(&self) -> usize {
        self.weapons.len()
    }
}

impl PackTable for ClassBuilds {
    const WHAT: &'static str = "class builds";
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Ordered and reliable: the last table sent is the true one.
pub struct PackChannel;

/// JSON bytes, not the typed value: Lightyear encodes with postcard, which
/// refuses `deserialize_any`, and the catalogue's internally tagged enums
/// (`#[serde(tag = "kind")]`) need it. The failure is silent: the send
/// succeeds and the receive errors inside Lightyear.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(bound = "")]
pub struct Snapshot<T>(pub Vec<u8>, #[serde(skip)] PhantomData<T>);

/// Compared against as well as `Res::is_changed`, which counts the startup
/// insertion as a change.
#[derive(Resource, Default)]
struct LastSent<T>(T);

pub struct PackSyncPlugin;

impl Plugin for PackSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<PackChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        sync::<WeaponCatalogue>(app);
        sync::<ClassBuilds>(app);
    }
}

fn sync<T: PackTable>(app: &mut App) {
    app.register_message::<Snapshot<T>>()
        .add_direction(NetworkDirection::ServerToClient);

    app.init_resource::<T>().init_resource::<LastSent<T>>().add_systems(
        Update,
        (
            (send_to_new_clients::<T>, broadcast_changes::<T>).run_if(is_hosting),
            adopt_the_servers::<T>.run_if(is_remote_client),
        ),
    );
}

fn send_to_new_clients<T: PackTable>(
    table: Res<T>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let Some(message) = encode(table.as_ref()) else { return Ok(()) };
    for client in &joined {
        info!("Sending {} {} to a new client", table.len(), T::WHAT);
        sender.send_to_entities::<_, PackChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Nothing edits these at runtime yet; this covers a server reloading its
/// files mid-match.
fn broadcast_changes<T: PackTable>(
    table: Res<T>,
    server: Option<Single<&Server>>,
    mut last: ResMut<LastSent<T>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    // Pre-filter only, so the deep compare is not paid every frame.
    if !table.is_changed() || table.as_ref() == &last.0 {
        return Ok(());
    }

    last.0 = table.clone();
    let Some(message) = encode(table.as_ref()) else { return Ok(()) };
    info!("{} changed; sending {} of them to everyone", T::WHAT, table.len());
    sender.send::<_, PackChannel>(&message, &server, &NetworkTarget::All)?;
    Ok(())
}

fn encode<T: PackTable>(table: &T) -> Option<Snapshot<T>> {
    match serde_json::to_vec(table) {
        Ok(bytes) => Some(Snapshot(bytes, PhantomData)),
        Err(e) => {
            error!("The {} will not encode: {e}", T::WHAT);
            None
        }
    }
}

/// Gated on authority rather than on holding a receiver, which a listen
/// server also has.
fn adopt_the_servers<T: PackTable>(
    mut receivers: Query<&mut MessageReceiver<Snapshot<T>>>,
    mut table: ResMut<T>,
) {
    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            let sent: T = match serde_json::from_slice(&message.0) {
                Ok(sent) => sent,
                // Not fatal: we keep our own.
                Err(e) => {
                    error!("The server's {} will not decode: {e}", T::WHAT);
                    continue;
                }
            };
            // Writing would mark the resource changed for nothing.
            if sent == *table {
                continue;
            }
            info!("Adopting the server's {}: {} of them", T::WHAT, sent.len());
            *table = sent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalogue;

    fn round_trip<T: PackTable + std::fmt::Debug>(table: T) {
        let sent = encode(&table).expect("would not encode");
        let there_and_back: T = serde_json::from_slice(&sent.0).expect("would not decode");
        assert_eq!(there_and_back, table);
    }

    /// Fails if any part of a weapon or a build does not survive the payload.
    #[test]
    fn every_table_survives_the_round_trip() {
        round_trip(default_catalogue());
        round_trip(ClassBuilds::shipped());
    }

    /// Fails if the payload stops being bytes that postcard round-trips, and
    /// pins that the typed catalogue cannot decode through postcard. See
    /// [`Snapshot`].
    #[test]
    fn the_wire_cannot_carry_an_internally_tagged_enum() {
        let catalogue = default_catalogue();

        let sent = encode(&catalogue).expect("a catalogue would not encode");
        let wire = postcard::to_allocvec(&sent).expect("bytes go on the wire");
        let back: Snapshot<WeaponCatalogue> =
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
    /// which would broadcast a table repeatedly.
    #[test]
    fn only_a_change_puts_a_table_on_the_wire() {
        let mut last = LastSent(default_catalogue());
        assert_eq!(last.0, default_catalogue(), "loading the same files twice gave two catalogues");
        last.0.weapons.clear();
        assert_ne!(last.0, default_catalogue(), "emptying the catalogue was not a change");

        assert_eq!(ClassBuilds::shipped(), ClassBuilds::shipped(), "two loads gave two tables");
    }
}
