//! Standing an actual socket up behind [`NetRole`].
//!
//! Bevy plugins can only be added before `run()`, so both plugin groups are
//! always present, even in a solo game; what comes and goes is one
//! [`NetLink`] entity. `Solo` holds none: nothing bound, nothing polled.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use lightyear::prelude::client::{Client, NetcodeClient, NetcodeConfig as ClientNetcodeConfig};
use lightyear::prediction::prelude::PredictionManager;
use lightyear::prelude::server::{NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerUdpIo, Start, Started};
use lightyear::prelude::*;

use crate::common::net::NetRole;
use crate::constants::{DEV_PRIVATE_KEY, NETCODE_PROTOCOL_ID, TICK_HZ};

/// The link entity: our listen server or our client. A marker, so teardown
/// cannot take a link entity somebody else spawned.
#[derive(Component)]
pub struct NetLink;

/// What the link is doing, as against what [`NetRole`] asked for: a role
/// changes instantly, a connection takes time and may fail.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum NetStatus {
    #[default]
    Offline,
    /// Bound and listening, with this many clients attached.
    Hosting { clients: usize },
    /// Handshaking with a server that has not answered yet.
    Connecting,
    Connected,
    /// Carries whatever the connection layer said.
    Failed(String),
}

impl NetStatus {
    /// Localised display text; not for parsing or comparing.
    pub fn describe(&self) -> String {
        match self {
            NetStatus::Offline => crate::get!("net.status.offline"),
            NetStatus::Hosting { clients } => {
                crate::get!("net.status.hosting", "clients", clients)
            }
            NetStatus::Connecting => crate::get!("net.status.connecting"),
            NetStatus::Connected => crate::get!("net.status.connected"),
            NetStatus::Failed(reason) => crate::get!("net.status.failed", "reason", reason),
        }
    }
}

/// Adds Lightyear and keeps its link entity in step with [`NetRole`].
pub struct NetTransportPlugin;

impl Plugin for NetTransportPlugin {
    fn build(&self, app: &mut App) {
        let tick_duration = std::time::Duration::from_secs_f64(1.0 / TICK_HZ);

        app
            // Tied to Lightyear's tick so the two clocks cannot drift apart.
            .insert_resource(Time::<Fixed>::from_hz(TICK_HZ))
            .add_plugins(client::ClientPlugins { tick_duration })
            .add_plugins(server::ServerPlugins { tick_duration })
            // After both plugin groups and before any link entity: the
            // registries are hashed into the handshake.
            .add_plugins(crate::common::protocol::ProtocolPlugin)
            // After the protocol, since it registers a channel and a message.
            .add_plugins(crate::common::match_state::MatchStatePlugin)
            .add_plugins(crate::common::map_sync::MapSyncPlugin)
            .add_plugins(crate::common::weapon_sync::WeaponSyncPlugin)
            // A client told about a weapon but not its model draws empty hands.
            .add_plugins(crate::prop::sync::PropSyncPlugin)
            .add_plugins(crate::common::net_events::NetEventsPlugin)
            // Prediction is gated on this resource existing, so it stays: a
            // host now may be a client after the next menu click.
            .insert_resource(PredictionManager::default())
            .init_resource::<NetStatus>()
            .add_observer(dress_new_client)
            .add_systems(
                Update,
                (
                    open_link_for_role.run_if(resource_changed::<NetRole>),
                    report_status,
                    crate::common::net_events::replicate_projectiles,
                    crate::common::net_events::replicate_display_bodies,
                    crate::common::net_events::replicate_corpses,
                )
                    .chain(),
            );
    }
}

/// Tears down any link and stands up the one the role asks for. Always
/// despawns first, even between two `Listen` ports, since rebinding in place
/// is a half-applied state.
fn open_link_for_role(
    mut commands: Commands,
    role: Res<NetRole>,
    existing: Query<Entity, With<NetLink>>,
    mut status: ResMut<NetStatus>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    *status = NetStatus::Offline;

    match &*role {
        NetRole::Solo => {}
        NetRole::Listen { port } => {
            // `UNSPECIFIED` so the LAN can reach it.
            let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), *port);
            let server = commands
                .spawn((
                    NetLink,
                    Name::from("Listen server"),
                    NetcodeServer::new(ServerNetcodeConfig {
                        protocol_id: NETCODE_PROTOCOL_ID,
                        private_key: DEV_PRIVATE_KEY,
                        ..default()
                    }),
                    ServerUdpIo::default(),
                    LocalAddr(address),
                ))
                .id();
            commands.trigger(Start { entity: server });
            info!("Listening on {address}");
        }
        NetRole::Client { host, port } => {
            let address = match resolve(host, *port) {
                Ok(address) => address,
                Err(message) => {
                    error!("Cannot connect to {host}:{port}: {message}");
                    *status = NetStatus::Failed(message);
                    return;
                }
            };
            // Random because there are no accounts; netcode only needs it
            // distinct per process.
            let client_id = rand::random::<u64>();
            let netcode = match NetcodeClient::new(
                Authentication::Manual {
                    server_addr: address,
                    client_id,
                    private_key: DEV_PRIVATE_KEY,
                    protocol_id: NETCODE_PROTOCOL_ID,
                },
                ClientNetcodeConfig::default(),
            ) {
                Ok(netcode) => netcode,
                Err(error) => {
                    error!("Cannot build a connect token: {error}");
                    *status = NetStatus::Failed(error.to_string());
                    return;
                }
            };

            let client = commands
                .spawn((
                    NetLink,
                    Name::from("Client"),
                    netcode,
                    // Without `ReplicationReceiver` replication metadata
                    // arrives with nothing to interpret it.
                    Client,
                    ReplicationReceiver,
                    PingManager::default(),
                    UdpIo::default(),
                    // Port 0: any free local port.
                    LocalAddr(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)),
                    PeerAddr(address),
                ))
                .id();
            commands.trigger(Connect { entity: client });
            *status = NetStatus::Connecting;
            info!("Connecting to {address} as {client_id}");
        }
    }
}

/// Every client gets its own `LinkOf` child, and replication is addressed
/// through it. Without a `ReplicationSender` the handshake completes, nothing
/// flows, and the client times out.
fn dress_new_client(add: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert((
            ReplicationSender,
            // Prediction syncs timelines against round-trip time; without
            // this, pings are dropped with a warning.
            PingManager::default(),
            Name::from("Client of"),
        ));
}

/// Reads the link's state into [`NetStatus`] for the UI. Decides nothing.
fn report_status(
    mut status: ResMut<NetStatus>,
    servers: Query<Entity, (With<NetLink>, With<Started>)>,
    clients: Query<
        (Option<&Connected>, Option<&Connecting>, Option<&Disconnected>),
        (With<NetLink>, With<lightyear::prelude::client::Client>),
    >,
    links_of: Query<&LinkOf>,
) {
    // A failure stays until the next role change, so it does not blink past
    // on its way back to `Offline`.
    if matches!(*status, NetStatus::Failed(_)) {
        return;
    }

    let next = if let Some(server) = servers.iter().next() {
        NetStatus::Hosting {
            clients: links_of.iter().filter(|link| link.server == server).count(),
        }
    } else if let Some((connected, connecting, disconnected)) = clients.iter().next() {
        match (connected, connecting, disconnected) {
            (Some(_), _, _) => NetStatus::Connected,
            (_, Some(_), _) => NetStatus::Connecting,
            // `Unknown` is also the state before any attempt; not a failure.
            (_, _, Some(disconnected)) => match &disconnected.reason {
                DisconnectedReason::Unknown => NetStatus::Connecting,
                reason => NetStatus::Failed(reason.to_string()),
            },
            _ => NetStatus::Connecting,
        }
    } else {
        NetStatus::Offline
    };

    status.set_if_neq(next);
}

/// Blocking DNS on the main thread: native only and once per menu click.
/// WebTransport resolves its own URLs, so the browser will not come here.
fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    use std::net::ToSocketAddrs;

    (host, port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| format!("{host} resolved to no addresses"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostname_that_exists_resolves() {
        assert!(resolve("localhost", 27100).is_ok());
    }

    #[test]
    fn a_hostname_that_does_not_is_an_error_rather_than_a_panic() {
        assert!(resolve("this-host-does-not-exist.invalid", 27100).is_err());
    }

    /// Losing the port would connect to the right machine on the wrong port.
    #[test]
    fn resolving_keeps_the_port() {
        assert_eq!(resolve("127.0.0.1", 27016).unwrap().port(), 27016);
    }
}
