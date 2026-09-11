//! Handing every weapon's model to whoever connects.
//!
//! Shaped on [`crate::common::weapon_sync`], which it follows on the heels of:
//! a weapon names a model, and a client that has been told about the weapon
//! and not the model draws empty hands.
//!
//! **The document, not the meshes.** A prop is parametric and small, and the
//! client already has the whole kernel — so a model crosses the wire as what
//! the prop editor saved, and every machine builds its own triangles. That is
//! the same choice [`crate::common::map_sync`] makes for the same reason: send
//! what a thing *is*, not what it came out as.
//!
//! **Which models is a question the catalogue answers**, so nothing here has
//! to enumerate a directory — the rule in
//! [`crate::common::assets`] survives, and a browser client is told exactly
//! the models its weapons name.
//!
//! **Order does not matter, and that is on purpose.** A model arriving before
//! its weapon is cached under its own name and waits. A model arriving *after*
//! a client already went looking in its own pack replaces the failure that
//! search cached, and bodies redress because [`PropCache::generation`] moved.
//! Anything that depended on the two channels arriving in a particular order
//! would be depending on something neither of them promises.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::common::net::NetRole;
use crate::common::weapon::WeaponCatalogue;
use crate::prop::baked::{PropCache, SurfaceMaterials};
use crate::prop::document;

/// Ordered and reliable, like the catalogue: a model is state, and the last
/// one sent is the one that is true.
pub struct PropChannel;

/// Each model as the text its file holds.
///
/// **Text, not the typed value.** Lightyear encodes with postcard, which
/// refuses `deserialize_any` — exactly what an internally tagged enum needs to
/// read itself back, and `FeatureOp`, `Shape` and `Profile` are all tagged
/// that way because it is what makes a `.gpp` readable. A `PropDoc` on the
/// wire therefore encodes and then cannot be decoded, silently: the send
/// succeeds and the receive errors inside Lightyear.
///
/// Which turns out to be the better payload anyway. What crosses is exactly
/// what the prop editor saved, parsed at the far end by the same
/// [`document::parse`] that reads it off disk — one description of a prop,
/// rather than one for a file and another for a connection.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ModelSnapshot(pub Vec<(String, String)>);

/// What was last put on the wire, compared against rather than asking
/// `Res::is_changed` — insertion counts as a change, and the startup load
/// would otherwise broadcast to an empty room.
#[derive(Resource, Default)]
struct LastModelsSent(Vec<(String, String)>);

pub struct PropSyncPlugin;

impl Plugin for PropSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<PropChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        app.register_message::<ModelSnapshot>()
            .add_direction(NetworkDirection::ServerToClient);

        app.init_resource::<LastModelsSent>().add_systems(
            Update,
            (
                send_models_to_new_clients,
                broadcast_model_changes,
                adopt_the_servers_models,
            ),
        );
    }
}

/// Every model the catalogue names, read off this machine's packs.
///
/// A weapon whose model will not load is left out rather than sent empty: the
/// client then goes looking in its own packs, which is the right fallback and
/// the one it would have taken had the server said nothing.
fn models_named_by(catalogue: &WeaponCatalogue, packs: &PackAssets) -> Vec<(String, String)> {
    let mut named: Vec<&str> = catalogue
        .weapons
        .iter()
        .filter_map(|weapon| weapon.model.as_deref())
        .collect();
    named.sort_unstable();
    named.dedup();

    named
        .into_iter()
        .filter_map(|name| {
            let mut found = None;
            for pack in default_packs().iter() {
                if let Ok(doc) = document::load(packs, pack, name) {
                    found = Some(doc);
                }
            }
            let doc = found?;
            match document::to_text(&doc) {
                Ok(text) => Some((name.to_owned(), text)),
                Err(e) => {
                    error!("prop {name} will not encode: {e}");
                    None
                }
            }
        })
        .collect()
}

fn send_models_to_new_clients(
    catalogue: Res<WeaponCatalogue>,
    packs: Res<PackAssets>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let models = models_named_by(&catalogue, &packs);
    let message = ModelSnapshot(models);
    for client in &joined {
        info!("Sending {} weapon models to a new client", message.0.len());
        sender.send_to_entities::<_, PropChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Tell everybody when the models change under them.
///
/// Which today means when the *catalogue* does, since nothing re-reads a prop
/// at runtime. A server that reloaded its files mid-match would otherwise be
/// the only machine holding the new ones.
fn broadcast_model_changes(
    catalogue: Res<WeaponCatalogue>,
    packs: Res<PackAssets>,
    server: Option<Single<&Server>>,
    mut last: ResMut<LastModelsSent>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    if !catalogue.is_changed() {
        return Ok(());
    }

    let models = models_named_by(&catalogue, &packs);
    if models == last.0 {
        return Ok(());
    }

    last.0 = models.clone();
    info!("Models changed; sending {} of them to everyone", models.len());
    sender.send::<_, PropChannel>(&ModelSnapshot(models), &server, &NetworkTarget::All)?;
    Ok(())
}

/// Take the server's models over anything of ours.
///
/// Only on a client, and the guard is on authority rather than on holding a
/// receiver: a listen server holds one too, and adopting its own models back
/// would be a server taking instructions from itself.
fn adopt_the_servers_models(
    role: Res<NetRole>,
    mut receivers: Query<&mut MessageReceiver<ModelSnapshot>>,
    mut cache: ResMut<PropCache>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if role.is_authority() {
        return;
    }

    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            info!("Adopting the server's weapon models: {} of them", message.0.len());
            for (name, text) in &message.0 {
                match document::parse(text) {
                    Ok(doc) => {
                        cache.adopt(name, &doc, &mut meshes, &mut materials, &mut surfaces)
                    }
                    // Logged rather than fatal: one model nobody can read is a
                    // weapon drawn with empty hands, not a client that falls
                    // over.
                    Err(e) => error!("the server's prop {name} will not parse: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::document::PropDoc;
    use crate::common::weapon::{Magazine, Mounted, Weapon, WeaponId};

    fn weapon(name: &str, model: Option<&str>) -> Weapon {
        Weapon {
            id: WeaponId::of(name),
            name_key: format!("weapon.names.{name}"),
            model: model.map(str::to_owned),
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            magazine: Magazine::BOTTOMLESS,
        }
    }

    /// **The catalogue decides which models go**, so nothing has to list a
    /// directory — which a browser client could not do anyway. A weapon
    /// nobody has modelled contributes nothing, and two weapons sharing a
    /// model send it once.
    #[test]
    fn only_the_models_the_catalogue_names_are_sent() {
        let mut catalogue = WeaponCatalogue::default();
        catalogue.insert(weapon("launcher", Some("rocket_launcher")));
        catalogue.insert(weapon("spare", Some("rocket_launcher")));
        catalogue.insert(weapon("bare", None));
        catalogue.insert(weapon("missing", Some("no_such_prop")));

        let named = models_named_by(&catalogue, &PackAssets::default());
        let names: Vec<&str> = named.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["rocket_launcher"],
            "the sent set should be the loadable models the catalogue names, once each",
        );
    }

    /// **Over the wire's own encoding**, which is the whole reason the payload
    /// is text.
    ///
    /// Lightyear uses postcard, and postcard refuses `deserialize_any` — what
    /// an internally tagged enum needs to read itself back. A `PropDoc` sent
    /// directly encodes and then cannot be decoded, and it fails in the way
    /// nobody notices: inside Lightyear, on the receiving machine, while the
    /// send reports success. This asserts both halves so that putting the
    /// typed value back fails here instead of on somebody's server.
    #[test]
    fn a_model_survives_the_wire_as_text_and_would_not_as_a_document() {
        let mut catalogue = WeaponCatalogue::default();
        catalogue.insert(weapon("launcher", Some("rocket_launcher")));
        let sent = ModelSnapshot(models_named_by(&catalogue, &PackAssets::default()));
        assert!(!sent.0.is_empty(), "the default pack ships a model to test with");

        let wire = postcard::to_allocvec(&sent).expect("text goes on the wire");
        let back: ModelSnapshot = postcard::from_bytes(&wire).expect("and comes back off it");
        assert_eq!(sent, back);

        // And it is still the same prop at the other end.
        let there = document::parse(&back.0[0].1).expect("the text is a prop");
        let here = document::parse(&sent.0[0].1).expect("the text is a prop");
        assert_eq!(there, here);
        assert!(there.evaluate().problems.is_empty());
        assert_eq!(there.evaluate().triangle_count(), here.evaluate().triangle_count());

        // What the typed value would have done.
        let naive = postcard::to_allocvec(&here).expect("postcard will encode it");
        assert!(
            postcard::from_bytes::<PropDoc>(&naive).is_err(),
            "postcard decoded an internally tagged enum; the payload could be the document \
             again, and this test has stopped earning its place",
        );
    }
}
