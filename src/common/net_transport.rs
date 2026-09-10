//! Standing an actual socket up behind [`NetRole`].
//!
//! [`NetRole`] says which end of the wire this process is; this is the plugin
//! that makes it true. The split matters because **Bevy plugins can only be
//! added before `run()`**, so hosting cannot be "add the server plugins now".
//! Both plugin groups are always present, in every process, including a closed
//! single-player one — what actually comes and goes is a single entity.
//!
//! That is also why `Solo` is worth having as a role. A closed game holds no
//! link entity at all: nothing is bound, nothing is polled, and there is no
//! loopback connection between the two halves of the same process.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use lightyear::prelude::client::{Client, NetcodeClient, NetcodeConfig as ClientNetcodeConfig};
use lightyear::prelude::server::{NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerUdpIo, Start, Started};
use lightyear::prelude::*;

use crate::common::net::NetRole;
use crate::constants::{DEV_PRIVATE_KEY, NETCODE_PROTOCOL_ID, TICK_HZ};

/// The one entity this plugin owns: the server we are listening on, or the
/// client we are connected through.
///
/// Marked rather than found by querying for `NetcodeServer` or `NetcodeClient`
/// so that tearing down is `despawn everything with this marker` and cannot
/// take a link entity somebody else spawned with it.
#[derive(Component)]
pub struct NetLink;

/// What the link is actually doing, as opposed to what [`NetRole`] asked for.
///
/// The two are deliberately separate: a role is intent and changes the instant
/// somebody picks a menu item, whereas a connection is attempted, takes time,
/// and may simply fail. A UI that read the role would say "Connected to
/// example.net" about a host that never answered.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum NetStatus {
    /// No link entity exists. The resting state of a closed game.
    #[default]
    Offline,
    /// Bound and listening, with this many clients attached.
    Hosting { clients: usize },
    /// Handshaking with a server that has not answered yet.
    Connecting,
    Connected,
    /// The attempt ended. Carries whatever the connection layer said.
    Failed(String),
}

impl NetStatus {
    /// A line for the menu. Localised, so it is display text and not something
    /// to parse or compare.
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
///
/// Both `ClientPlugins` and `ServerPlugins` go in unconditionally, which is
/// what makes a listen server possible at all: hosting while playing is one
/// process being both, and neither group can be added once the app is running.
pub struct NetTransportPlugin;

impl Plugin for NetTransportPlugin {
    fn build(&self, app: &mut App) {
        let tick_duration = std::time::Duration::from_secs_f64(1.0 / TICK_HZ);

        app
            // Bevy's default is already 64 Hz, but stating it here is what
            // keeps it tied to the tick Lightyear reconciles in. Two clocks
            // that agree by coincidence are two clocks that will stop
            // agreeing.
            .insert_resource(Time::<Fixed>::from_hz(TICK_HZ))
            .add_plugins(client::ClientPlugins { tick_duration })
            .add_plugins(server::ServerPlugins { tick_duration })
            // After both plugin groups and before any link entity exists,
            // which is the order Lightyear requires: the registries are
            // hashed into the handshake, so they have to be complete before
            // anything can connect.
            .add_plugins(crate::common::protocol::ProtocolPlugin)
            // After the protocol, since it registers a channel and a message.
            .add_plugins(crate::common::match_state::MatchStatePlugin)
            .add_plugins(crate::common::map_sync::MapSyncPlugin)
            .add_plugins(crate::common::net_events::NetEventsPlugin)
            .init_resource::<NetStatus>()
            .add_observer(dress_new_client)
            .add_systems(
                Update,
                (
                    open_link_for_role.run_if(resource_changed::<NetRole>),
                    report_status,
                    crate::common::net_events::replicate_projectiles,
                    crate::common::net_events::replicate_display_bodies,
                )
                    .chain(),
            );
    }
}

/// Tear down whatever link exists and stand up the one the role asks for.
///
/// Unconditionally destructive: a role change always despawns first, even from
/// `Listen { 27015 }` to `Listen { 27016 }`, because rebinding a socket in
/// place is a partially-applied state that only shows up as a bug later.
/// `apply_net_requests` refuses to write a role equal to the one already set,
/// so this does not fire for a request that changed nothing.
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
            // `UNSPECIFIED` rather than loopback: the point of hosting is that
            // somebody on the LAN can reach it.
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
            // A random client id because there is no account system to ask.
            // Distinct per process is all netcode needs it for; the identity
            // that survives a reconnect is a question for the backend in
            // `documentation/sketch.md`.
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
                    // `NetcodeClient` requires `Client` and `Link`; the
                    // receiver is what makes this end willing to be told about
                    // entities, and without it the server's replication
                    // metadata arrives with nothing to interpret it.
                    Client,
                    ReplicationReceiver,
                    PingManager::default(),
                    UdpIo::default(),
                    // Port 0: any free local port. Only the server's address
                    // has to be a number somebody chose.
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

/// Give each arriving connection the half of the link that sends.
///
/// The server endpoint is one socket; every client that turns up gets its own
/// `LinkOf` child entity, and that child — not the endpoint — is what
/// replication is addressed through. Nothing inserts this for us, and without
/// it a client completes the netcode handshake and then hears nothing, which
/// looks like a working connection right up until it times out.
fn dress_new_client(add: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert((
            ReplicationSender,
            // Round-trip time is what the timelines are synchronised against,
            // and prediction is only as good as that estimate. Without it the
            // pings arrive and are dropped with a warning.
            PingManager::default(),
            Name::from("Client of"),
        ));
}

/// Read what the link is doing back into [`NetStatus`] for the UI.
///
/// A read, never a write: nothing here decides anything, so a status that goes
/// stale is a cosmetic bug rather than a connection that quietly stops.
fn report_status(
    mut status: ResMut<NetStatus>,
    servers: Query<Entity, (With<NetLink>, With<Started>)>,
    clients: Query<
        (Option<&Connected>, Option<&Connecting>, Option<&Disconnected>),
        (With<NetLink>, With<lightyear::prelude::client::Client>),
    >,
    links_of: Query<&LinkOf>,
) {
    // A failure already recorded stays until the next role change, so that a
    // refused connection does not blink past on its way back to `Offline`.
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
            // `Unknown` is also what a link carries before it has been
            // attempted, so it is not a failure to report — the connection is
            // simply still in front of us.
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

/// Turn a typed hostname into an address to send packets at.
///
/// Blocking DNS on the main thread, which is exactly the sort of thing the
/// wasm notes in `CLAUDE.md` warn about — but it happens once, on a menu
/// click, and only on the native path. WebTransport takes a URL and does its
/// own resolution, so the browser client will not come through here.
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

    /// The port asked for is the port packets go to. Trivial, and worth
    /// pinning: `to_socket_addrs` takes the port from the tuple, and losing it
    /// would give a connection to the right machine on the wrong port.
    #[test]
    fn resolving_keeps_the_port() {
        assert_eq!(resolve("127.0.0.1", 27016).unwrap().port(), 27016);
    }
}
