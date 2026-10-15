//! Giving every client the server's map, so a predicting client collides with
//! the same walls; when they differ the client rubber-bands silently.
//!
//! The feature timeline travels, not baked geometry, and always the whole map:
//! a delta protocol would be a second description of a map. Cost is kept down
//! by comparing bytes with the last send and by [`RESEND_INTERVAL`]. See
//! CLAUDE.md, "The map every client is standing in".

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::net::{is_hosting, is_remote_client};
use crate::editor::editable::{Feature, FeatureId, FeatureTimeline};
use crate::tool::room::CalculateRoomGeometry;

/// Ordered and reliable, so an older snapshot cannot replace a newer one and
/// a dropped one is not lost.
pub struct MapChannel;

/// Seconds between sends, at least. Dragging edits every frame and adopting
/// rebuilds every feature entity and re-bakes, so this caps a client at four
/// rebuilds a second.
const RESEND_INTERVAL: f32 = 0.25;

/// Bytes, because [`Feature`] is a typetag trait object that cannot be cloned
/// into a message.
///
/// JSON is knowingly temporary: it is what `typetag` gives for free, and the
/// compiled map format is where it stops being acceptable.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MapSnapshot {
    pub blueprint: Vec<u8>,
}

/// Not the whole `FeatureTimeline`: selection, pending despawns and undo
/// history are local.
#[derive(Serialize)]
struct Outgoing<'a> {
    features: Vec<&'a Feature>,
    order: &'a [FeatureId],
    id_counter: u64,
}

/// [`Outgoing`]'s fields, owned. Unknown fields are refused, so
/// `a_map_survives_the_round_trip` fails when one side gains a field the
/// other lacks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Incoming {
    features: Vec<Feature>,
    order: Vec<FeatureId>,
    id_counter: u64,
}

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

        app.init_resource::<LastMapSent>()
            .add_systems(
                Update,
                (
                    (send_map_to_new_clients, broadcast_map_changes).run_if(is_hosting),
                    adopt_the_servers_map.run_if(is_remote_client),
                ),
            );
    }
}

/// Bytes rather than a dirty flag: selecting a feature marks the timeline
/// changed.
#[derive(Resource, Default)]
struct LastMapSent {
    blueprint: Vec<u8>,
    since: f32,
    /// The timeline changed since the last look at the bytes. Latched rather
    /// than read off `is_changed` at send time, because this system runs every
    /// frame and a change made inside the resend window has aged out of
    /// `is_changed` by the time the window closes — the last frame of a drag
    /// would never be sent.
    pending: bool,
}

/// The one encoder for both the join snapshot and the broadcast.
fn encode(timeline: &FeatureTimeline) -> Option<Vec<u8>> {
    let snapshot = Outgoing {
        features: timeline
            .feature_order()
            .iter()
            .filter_map(|id| timeline.get_feature(id))
            .collect(),
        order: timeline.feature_order(),
        id_counter: timeline.id_counter(),
    };
    match serde_json::to_vec(&snapshot) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            error!("Could not encode the map to send: {error}");
            None
        }
    }
}

fn broadcast_map_changes(
    time: Res<Time>,
    timeline: Res<FeatureTimeline>,
    server: Option<Single<&Server>>,
    mut last: ResMut<LastMapSent>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };

    last.pending |= timeline.is_changed();
    last.since += time.delta_secs();
    if last.since < RESEND_INTERVAL {
        return Ok(());
    }
    // Pre-filter only; the byte comparison decides.
    if !last.pending {
        return Ok(());
    }
    last.pending = false;

    let Some(blueprint) = encode(&timeline) else { return Ok(()) };
    if blueprint == last.blueprint {
        return Ok(());
    }

    last.since = 0.0;
    last.blueprint = blueprint.clone();
    info!("Map changed; sending {} bytes to everyone", blueprint.len());
    sender.send::<_, MapChannel>(&MapSnapshot { blueprint }, &server, &NetworkTarget::All)?;
    Ok(())
}

/// On connect rather than on entering Play, because clients are in the editor
/// between rounds too.
fn send_map_to_new_clients(
    timeline: Res<FeatureTimeline>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let Some(blueprint) = encode(&timeline) else { return Ok(()) };
    let message = MapSnapshot { blueprint };

    for client in &joined {
        info!("Sending the map ({} bytes) to a new client", message.blueprint.len());
        sender.send_to_entities::<_, MapChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Gated on authority rather than on having a receiver: a listen server has
/// one too, and adopting its own map would despawn what it is replicating.
fn adopt_the_servers_map(
    mut receivers: Query<&mut MessageReceiver<MapSnapshot>>,
    mut timeline: ResMut<FeatureTimeline>,
    mut bake: MessageWriter<CalculateRoomGeometry>,
    // Adopting is a visible rebuild, so an identical map is skipped.
    mut last: Local<Vec<u8>>,
) {
    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            if message.blueprint == *last {
                continue;
            }
            *last = message.blueprint.clone();
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
                // No history: a client must not undo the server's map.
                incoming.id_counter,
            ));
            // Read a frame later, after `sync_entities` has given the features
            // entities; a bake this frame would bake nothing. This covers a map
            // arriving mid-round; `BakeSystems::All` covers one arriving as Play
            // starts, so joining bakes twice.
            bake.write(CalculateRoomGeometry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::PointRef;
    use crate::editor::editor_room::EditorRoom;

    /// Fails if selecting a feature changes the encoded bytes, which would
    /// broadcast the map on every click.
    #[test]
    fn only_an_edit_changes_the_bytes() {
        let mut timeline = FeatureTimeline::default();
        let id = timeline.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(4.0, 3.0, 4.0),
        )));
        let before = encode(&timeline).expect("the map would not encode");

        timeline.select(Some(id));
        assert_eq!(
            encode(&timeline).as_deref(),
            Some(before.as_slice()),
            "selecting a feature would have re-sent the whole map"
        );

        timeline.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(10.0, 0.0, 10.0),
            PointRef::absolute(14.0, 3.0, 14.0),
        )));
        assert_ne!(
            encode(&timeline).as_deref(),
            Some(before.as_slice()),
            "an edit did not change what would be sent"
        );
    }

    /// Fails if a feature does not survive the wire encoding; a missing
    /// `typetag` registration shows up here rather than at compile time.
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
        };
        let bytes = serde_json::to_vec(&snapshot).expect("the map would not encode");
        let incoming: Incoming =
            serde_json::from_slice(&bytes).expect("the map would not decode");

        assert_eq!(incoming.features.len(), 2);
        assert_eq!(incoming.order, *server.feature_order());
        assert_eq!(incoming.id_counter, server.id_counter());
    }

    /// Fails if adopting leaves the old map's entities standing, which looks
    /// like a working sync with collision built from both maps.
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

    /// Fails if a feature's server-side entity crosses the wire;
    /// `sync_entities` would skip it and the map would never appear.
    #[test]
    fn a_decoded_feature_has_no_entity_yet() {
        let mut server = FeatureTimeline::default();
        let id = server.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(0.0, 0.0, 0.0),
            PointRef::absolute(1.0, 1.0, 1.0),
        )));
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
        };
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let incoming: Incoming = serde_json::from_slice(&bytes).unwrap();

        assert!(
            incoming.features[0].object().entity().is_none(),
            "the server's entity id came across; the client would never spawn the feature"
        );
    }
}
