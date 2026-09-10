//! Giving every client the server's map.
//!
//! A client predicting its own movement is stepping the same physics against
//! its own copy of the walls, so the two copies have to be the same walls. If
//! they are not, prediction does not fail loudly — the client walks through a
//! doorway the server stops it at, is corrected, walks into it again, and
//! rubber-bands there forever.
//!
//! What travels is the **feature timeline**, not the baked geometry: features
//! are what the editor edits, and sending them is what will let an edit made
//! between rounds reach everybody. Today only the initial snapshot is sent, on
//! connect. Streaming the edits after it is the same channel and the next
//! piece of work — see the note at the bottom of this file.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::net::NetRole;
use crate::editor::editable::{Feature, FeatureId, FeatureTimeline};
use crate::tool::room::CalculateRoomGeometry;

/// The map, and later the edits to it.
///
/// Ordered and reliable, both of which the timeline needs: an edit is a delta
/// against the state the last one left behind, so one arriving late or not at
/// all leaves a client's map permanently different from the server's.
pub struct MapChannel;

/// The whole map, as the server currently has it.
///
/// Carries serialised bytes rather than the features themselves because
/// [`Feature`] holds a `Box<dyn FeatureTrait>` — a typetag trait object, which
/// serialises and deserialises but cannot be cloned into a message. Encoding
/// once on the server and decoding once on each client also means the format
/// is stated in one place rather than derived across a message type.
///
/// JSON, for now, and knowingly the wrong choice for anything large: it is
/// what `typetag` gives for free and a map is sent once per join. The compiled
/// map format is where this stops being acceptable.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MapSnapshot {
    pub blueprint: Vec<u8>,
}

/// The parts of a timeline that describe the map.
///
/// Deliberately not the whole `FeatureTimeline`: selection is a fact about
/// whoever is looking at it, `pending_despawns` is a fact about this world's
/// entities, and the undo history is not something a joining client has any
/// use for. What is left is what the map *is*.
#[derive(Serialize)]
struct Outgoing<'a> {
    features: Vec<&'a Feature>,
    order: &'a [FeatureId],
    id_counter: u64,
    rollback_bar: u64,
}

/// The same shape, owned, on the way in. Serde reads the borrowed form above
/// into this one because the fields line up; keep them in step.
#[derive(Deserialize)]
struct Incoming {
    features: Vec<Feature>,
    order: Vec<FeatureId>,
    id_counter: u64,
    rollback_bar: u64,
}

/// Sends the map to clients, and adopts it on them.
pub struct MapSyncPlugin;

impl Plugin for MapSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<MapChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        app.register_message::<MapSnapshot>()
            .add_direction(NetworkDirection::ServerToClient);

        app.add_systems(Update, (send_map_to_new_clients, adopt_the_servers_map));
    }
}

/// Hand the map over the moment somebody connects.
///
/// On connect rather than on entering Play: a client is in the editor with
/// everybody else between rounds, and an editor showing an empty map until the
/// round starts would be an editor nobody can edit with.
fn send_map_to_new_clients(
    timeline: Res<FeatureTimeline>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let snapshot = Outgoing {
        features: timeline
            .feature_order()
            .iter()
            .filter_map(|id| timeline.get_feature(id))
            .collect(),
        order: timeline.feature_order(),
        id_counter: timeline.id_counter(),
        rollback_bar: timeline.rollback_bar(),
    };
    let blueprint = match serde_json::to_vec(&snapshot) {
        Ok(bytes) => bytes,
        Err(error) => {
            // Not fatal to the server: the client simply has no map, which is
            // better than dropping everybody because one feature would not
            // encode.
            error!("Could not encode the map to send: {error}");
            return Ok(());
        }
    };
    let message = MapSnapshot { blueprint };

    for client in &joined {
        info!("Sending the map ({} bytes) to a new client", message.blueprint.len());
        sender.send_to_entities::<_, MapChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Replace our map with the server's.
///
/// Only on a client, and the check is on authority rather than on having a
/// receiver: a listen server holds a `MessageReceiver` too, and adopting its
/// own map back would throw away the very entities it is replicating.
fn adopt_the_servers_map(
    role: Res<NetRole>,
    mut receivers: Query<&mut MessageReceiver<MapSnapshot>>,
    mut timeline: ResMut<FeatureTimeline>,
    mut bake: MessageWriter<CalculateRoomGeometry>,
) {
    if role.is_authority() {
        return;
    }

    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            let incoming: Incoming = match serde_json::from_slice(&message.blueprint) {
                Ok(incoming) => incoming,
                Err(error) => {
                    error!("The server's map would not decode: {error}");
                    continue;
                }
            };
            info!("Adopting the server's map: {} feature(s)", incoming.features.len());

            let features = incoming
                .features
                .into_iter()
                .map(|feature| (feature.id(), feature))
                .collect();
            timeline.adopt(FeatureTimeline::from_parts(
                features,
                incoming.order,
                incoming.id_counter,
                incoming.rollback_bar,
                // No history: undo is a fact about whoever made the edits, and
                // a client that could undo the server's map would be editing
                // its own copy of a map it does not own.
                Vec::new(),
            ));
            // Queued, not immediate: the features have no entities until
            // `sync_entities` has run, and a bake this frame would bake an
            // empty world. `CalculateRoomGeometry` is read on a later frame,
            // which is what makes this work.
            //
            // Joining a round bakes twice over — once here and once in
            // `BakeSystems::All` as Play is entered — and that is the right
            // trade. This message is what covers a map arriving while a round
            // is already under way, which is where between-round editing is
            // going; the other covers a map that arrives in the same breath as
            // the round starts, which this message is one frame too late for.
            bake.write(CalculateRoomGeometry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::PointRef;
    use crate::editor::editor_room::EditorRoom;

    /// The round trip the wire actually does: encode a server's features,
    /// decode them into a client's timeline, and get the same map back.
    ///
    /// Worth pinning because `Feature` serialises through `typetag`, so a
    /// feature type whose registration is missing fails here rather than at
    /// compile time — which is the same class of omission the seven-place
    /// registration note in `CLAUDE.md` warns about.
    #[test]
    fn a_map_survives_the_round_trip() {
        let mut server = FeatureTimeline::default();
        server.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(-4.0, 0.0, -4.0),
            PointRef::absolute(4.0, 3.0, 4.0),
        )));
        server.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(6.0, 0.0, -2.0),
            PointRef::absolute(12.0, 3.0, 2.0),
        )));

        let snapshot = Outgoing {
            features: server
                .feature_order()
                .iter()
                .filter_map(|id| server.get_feature(id))
                .collect(),
            order: server.feature_order(),
            id_counter: server.id_counter(),
            rollback_bar: server.rollback_bar(),
        };
        let bytes = serde_json::to_vec(&snapshot).expect("the map would not encode");
        let incoming: Incoming =
            serde_json::from_slice(&bytes).expect("the map would not decode");

        assert_eq!(incoming.features.len(), 2);
        assert_eq!(incoming.order, *server.feature_order());
        assert_eq!(incoming.id_counter, server.id_counter());
        assert_eq!(
            incoming.rollback_bar, server.rollback_bar(),
            "the bar decides which features exist; a map that arrives with it wrong is a map \
             that is partly there"
        );
    }

    /// Adopting a map has to take the previous one's entities down with it.
    ///
    /// The failure this pins is quiet and would look like a working sync: the
    /// new rooms appear, the old ones are never despawned, and the client is
    /// standing in two maps at once with a `CollisionWorld` built from both.
    #[test]
    fn adopting_a_map_takes_the_old_one_down() {
        let mut app = App::new();
        app.init_resource::<FeatureTimeline>();
        app.add_systems(Update, FeatureTimeline::sync_entities);

        app.world_mut()
            .resource_mut::<FeatureTimeline>()
            .apply_feature(Box::new(EditorRoom::from_point_refs(
                PointRef::absolute(-1.0, 0.0, -1.0),
                PointRef::absolute(1.0, 1.0, 1.0),
            )));
        app.update();

        let before: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<crate::tool::room::Room>>()
            .iter(app.world())
            .collect();
        assert_eq!(before.len(), 1, "the first map never reached the world");

        let mut theirs = FeatureTimeline::default();
        theirs.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(10.0, 0.0, 10.0),
            PointRef::absolute(14.0, 3.0, 14.0),
        )));
        theirs.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(20.0, 0.0, 20.0),
            PointRef::absolute(24.0, 3.0, 24.0),
        )));
        app.world_mut().resource_mut::<FeatureTimeline>().adopt(theirs);
        app.update();

        let after: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<crate::tool::room::Room>>()
            .iter(app.world())
            .collect();
        assert_eq!(after.len(), 2, "the adopted map did not reach the world");
        assert!(
            !after.iter().any(|entity| before.contains(entity)),
            "the old map's rooms are still standing in the world"
        );
    }

    /// A decoded feature must not arrive already believing it has an entity:
    /// the ids in it would be the server's, and `sync_entities` skips anything
    /// that already has one. The map would be adopted and never appear.
    #[test]
    fn a_decoded_feature_has_no_entity_yet() {
        let mut server = FeatureTimeline::default();
        let id = server.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(1.0, 1.0, 1.0),
        )));
        // As `sync_entities` would have left it on the server.
        server
            .features_mut()
            .get_mut(&id)
            .unwrap()
            .object_mut()
            .set_entity(Some(Entity::from_raw_u32(41).unwrap()));

        let snapshot = Outgoing {
            features: server.feature_order().iter().filter_map(|id| server.get_feature(id)).collect(),
            order: server.feature_order(),
            id_counter: server.id_counter(),
            rollback_bar: server.rollback_bar(),
        };
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let incoming: Incoming = serde_json::from_slice(&bytes).unwrap();

        assert!(
            incoming.features[0].object().entity().is_none(),
            "the server's entity id came across; the client would never spawn the feature"
        );
    }
}
