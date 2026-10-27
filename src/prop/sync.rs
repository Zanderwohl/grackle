//! Handing every model in use — weapons, hats, placed props — to whoever
//! connects, after [`crate::common::pack_sync`].
//!
//! The document crosses, not meshes; each machine bakes its own. The
//! catalog and the placed props say which models to send, so nothing
//! enumerates a directory.
//!
//! Arrival order relative to the catalog does not matter: an early model
//! waits in the cache, and a late one replaces a cached failure and bumps
//! [`PropCache::generation`].

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::common::net::NetRole;
use crate::common::weapon::WeaponCatalog;
use crate::prop::baked::{PropCache, SurfaceMaterials};
use crate::game::world_prop::WorldProp;
use crate::prop::document;

/// Ordered and reliable: the last model sent is the true one.
pub struct PropChannel;

/// `(name, file text)` per model.
///
/// Text, not `PropDoc`: postcard refuses `deserialize_any`, which the
/// internally tagged `FeatureOp`, `Shape` and `Profile` need, so a typed
/// `PropDoc` encodes but fails to decode inside Lightyear while the send
/// reports success.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ModelSnapshot(pub Vec<(String, String)>);

/// Compared against rather than `Res::is_changed`, which counts insertion.
/// The names are kept too, so a placed prop being dragged is a comparison
/// rather than a re-read of every model off disk.
#[derive(Resource, Default)]
struct LastModelsSent {
    names: Vec<String>,
    models: Vec<(String, String)>,
}

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

        app.add_plugins(crate::common::skeleton::ClipAssetsPlugin);
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

/// Sorted, so two calls over the same things compare equal.
fn names_in_use<'a>(
    catalog: &'a WeaponCatalog,
    placed: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let weapons = catalog.weapons.iter().filter_map(|weapon| weapon.model.as_deref());
    let hats = catalog.loadouts.iter().filter_map(|(_, loadout)| loadout.hat.as_deref());
    let mut named: Vec<String> = weapons.chain(hats).chain(placed).map(str::to_owned).collect();
    named.sort_unstable();
    named.dedup();
    named
}

/// A model that will not load is left out rather than sent empty, so the
/// client falls back to its own packs.
fn models_named_by(names: &[String], packs: &PackAssets) -> Vec<(String, String)> {
    names
        .iter()
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
    catalog: Res<WeaponCatalog>,
    packs: Res<PackAssets>,
    placed: Query<&WorldProp>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    if joined.is_empty() {
        return Ok(());
    }

    let names = names_in_use(&catalog, placed.iter().map(|prop| prop.model.as_str()));
    let message = ModelSnapshot(models_named_by(&names, &packs));
    for client in &joined {
        info!("Sending {} models to a new client", message.0.len());
        sender.send_to_entities::<_, PropChannel>(&message, core::iter::once(client))?;
    }
    Ok(())
}

/// Resends when the catalog changes, the host saves a prop, or a placed prop
/// comes to name a model nobody has been sent. Gated because
/// `models_named_by` re-reads every model off disk.
#[allow(clippy::too_many_arguments)]
fn broadcast_model_changes(
    catalog: Res<WeaponCatalog>,
    packs: Res<PackAssets>,
    placed: Query<&WorldProp>,
    moved: Query<(), Changed<WorldProp>>,
    // Absent in a build with no prop editor.
    editor: Option<Res<crate::prop::document::PropEditor>>,
    server: Option<Single<&Endpoint, With<Server>>>,
    mut last: ResMut<LastModelsSent>,
    mut saves: Local<u64>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let saved = editor.map(|editor| editor.saves()).unwrap_or(0);
    let just_saved = saved != *saves;
    *saves = saved;

    let Some(server) = server else { return Ok(()) };
    if !catalog.is_changed() && !just_saved && moved.is_empty() {
        return Ok(());
    }

    let names = names_in_use(&catalog, placed.iter().map(|prop| prop.model.as_str()));
    if names == last.names && !just_saved && !catalog.is_changed() {
        return Ok(());
    }
    let models = models_named_by(&names, &packs);
    last.names = names;
    if models == last.models {
        return Ok(());
    }

    last.models = models.clone();
    info!("Models changed; sending {} of them to everyone", models.len());
    sender.send::<_, PropChannel>(&ModelSnapshot(models), &server, &NetworkTarget::All)?;
    Ok(())
}

/// Guarded on authority rather than on holding a receiver, which a listen
/// server also has.
fn adopt_the_servers_models(
    role: Res<NetRole>,
    mut receivers: Query<&mut MessageReceiver<ModelSnapshot>>,
    mut cache: ResMut<PropCache>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clips: ResMut<Assets<crate::common::skeleton::clip::Clip>>,
) {
    if role.is_authority() {
        return;
    }

    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            info!("Adopting the server's models: {} of them", message.0.len());
            for (name, text) in &message.0 {
                match document::parse(text) {
                    Ok(doc) => {
                        cache.adopt(name, &doc, &mut meshes, &mut materials, &mut surfaces, &mut clips, None)
                    }
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
            bucket: 1,
            deploy: 0.0,
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            magazine: Magazine::BOTTOMLESS,
        }
    }

    /// Fails if unnamed or unloadable models are sent, or a shared one twice.
    #[test]
    fn only_the_models_the_catalog_names_are_sent() {
        let mut catalog = WeaponCatalog::default();
        catalog.insert(weapon("launcher", Some("rocket_launcher")));
        catalog.insert(weapon("spare", Some("rocket_launcher")));
        catalog.insert(weapon("bare", None));
        catalog.insert(weapon("missing", Some("no_such_prop")));

        let named = models_named_by(&names_in_use(&catalog, std::iter::empty()), &PackAssets::default());
        let names: Vec<&str> = named.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["rocket_launcher"],
            "the sent set should be the loadable models the catalog names, once each",
        );
    }

    /// Fails if a hat or a placed prop's model stops being sent: a client
    /// missing one draws nothing and, for a crate, walks through it while the
    /// server stops it.
    #[test]
    fn hats_and_placed_props_are_sent_too() {
        let mut catalog = WeaponCatalog::default();
        catalog.insert(weapon("launcher", Some("rocket_launcher")));
        catalog.set_loadout(
            crate::common::class::Class::Mercenary,
            crate::common::weapon::Loadout { hat: Some("top_hat".into()), ..default() },
        );
        let names = names_in_use(&catalog, ["crate", "crate", "health_pack"].into_iter());
        assert_eq!(names, ["crate", "health_pack", "rocket_launcher", "top_hat"]);
    }

    /// Encodes with postcard as Lightyear does. Fails if the payload goes back
    /// to a typed `PropDoc`, which would decode-fail silently on the client.
    #[test]
    fn a_model_survives_the_wire_as_text_and_would_not_as_a_document() {
        let mut catalog = WeaponCatalog::default();
        catalog.insert(weapon("launcher", Some("rocket_launcher")));
        let sent = ModelSnapshot(models_named_by(
            &names_in_use(&catalog, std::iter::empty()),
            &PackAssets::default(),
        ));
        assert!(!sent.0.is_empty(), "the default pack ships a model to test with");

        let wire = postcard::to_allocvec(&sent).expect("text goes on the wire");
        let back: ModelSnapshot = postcard::from_bytes(&wire).expect("and comes back off it");
        assert_eq!(sent, back);

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
