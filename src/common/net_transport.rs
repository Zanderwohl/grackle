//! Standing an actual socket up behind [`NetRole`].
//!
//! Bevy plugins can only be added before `run()`, so both plugin groups are
//! always present, even in a solo game; what comes and goes is one
//! [`NetLink`] entity. `Solo` holds none: nothing bound, nothing polled.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use bevy::prelude::*;
use lightyear::prelude::client::{Client, NetcodeClient, NetcodeConfig as ClientNetcodeConfig};
use lightyear::prediction::prelude::PredictionManager;
use lightyear::prelude::server::{NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerUdpIo, Start, Started, Stop};
use lightyear::prelude::*;

use crate::common::match_state::{MatchChannel, ServerClosing};
use crate::common::net::{NetRequest, NetRole};
use crate::constants::{one_tick, DEV_PRIVATE_KEY, NETCODE_PROTOCOL_ID, TICK_HZ};

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
    /// Localized display text; not for parsing or comparing.
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

/// Whether a lost client link had ever been connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkLoss {
    FailedToConnect,
    Disconnected,
}

/// A client link that failed, with what to tell the player. Written once per link, when it fails;
/// a disconnect the player asked for is not one.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct LinkLost {
    pub loss: LinkLoss,
    pub message: String,
}

/// On a client link once it has been connected, so its failure reads as a disconnect.
#[derive(Component)]
struct WasConnected;

/// Netcode's states arrive as the name of a `ClientState` in a `TransportError`. Lightyear's own
/// wording is only logged; the server's or the operating system's text is passed through as data.
pub fn explain(reason: &DisconnectedReason) -> String {
    use crate::get;
    match reason {
        DisconnectedReason::TransportError(state) => match state.as_str() {
            "ConnectionRequestTimedOut" | "ChallengeResponseTimedOut" => get!("net.lost.no_answer"),
            "ConnectionTimedOut" => get!("net.lost.stopped_answering"),
            "ConnectionDenied" => get!("net.lost.denied"),
            "ConnectTokenExpired" => get!("net.lost.token_expired"),
            _ => get!("net.lost.ended"),
        },
        DisconnectedReason::ByPeer(None) | DisconnectedReason::LinkFailed(UnlinkReason::ServerStopped) => {
            get!("net.lost.closed")
        }
        DisconnectedReason::ByPeer(Some(why)) | DisconnectedReason::LinkFailed(UnlinkReason::ByPeer(why)) => {
            get!("net.lost.closed_because", "reason", why)
        }
        DisconnectedReason::LinkFailed(UnlinkReason::TransportError(why)) => get!("net.lost.network", "reason", why),
        DisconnectedReason::LinkFailed(UnlinkReason::Initial | UnlinkReason::UserRequested(_))
        | DisconnectedReason::UserRequested(_)
        | DisconnectedReason::Unknown => get!("net.lost.ended"),
    }
}

/// Adds Lightyear and keeps its link entity in step with [`NetRole`].
pub struct NetTransportPlugin;

impl Plugin for NetTransportPlugin {
    fn build(&self, app: &mut App) {
        let tick_duration = one_tick();

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
            .add_plugins(crate::common::match_phase::MatchPhaseSyncPlugin)
            .add_plugins(crate::common::player_settings::PlayerSettingsSyncPlugin)
            .add_plugins(crate::game::team_choice::TeamChoiceSyncPlugin)
            .add_plugins(crate::game::class_choice::ClassChoiceSyncPlugin)
            .add_plugins(crate::common::map_sync::MapSyncPlugin)
            .add_plugins(crate::common::pack_sync::PackSyncPlugin)
            // A client told about a weapon but not its model draws empty hands.
            .add_plugins(crate::prop::sync::PropSyncPlugin)
            .add_plugins(crate::common::net_events::NetEventsPlugin)
            .add_plugins(crate::game::spray::SpraySyncPlugin)
            .add_plugins(crate::common::postcard_sync::PostcardSyncPlugin)
            .add_plugins(crate::game::soundtrack::SoundtrackSyncPlugin)
            // Prediction is gated on this resource existing, so it stays: a
            // host now may be a client after the next menu click.
            .insert_resource(PredictionManager::default())
            .init_resource::<NetStatus>()
            .add_message::<LinkLost>()
            .init_resource::<OpenPending>()
            .add_observer(dress_new_client)
            .add_systems(Last, close_the_server_before_quitting.after(bevy::window::ExitSystems))
            .add_systems(
                Update,
                (
                    close_the_old_link.run_if(resource_changed::<NetRole>),
                    finish_closing,
                    open_link_for_role.run_if(|pending: Res<OpenPending>| pending.0),
                    leave_the_servers_world.run_if(resource_changed::<NetRole>),
                    hear_the_server_close,
                    report_status,
                    drop_a_failed_link,
                    // Keyed on lacking `Replicate` rather than on `Added`, and
                    // gated rather than returning early, so whatever was
                    // standing before somebody picked Host is sent too.
                    (
                        crate::common::net_events::replicate_projectiles,
                        crate::common::net_events::replicate_display_bodies,
                        crate::common::net_events::replicate_corpses,
                        crate::common::net_events::replicate_world_props,
                    )
                        .run_if(crate::common::net::is_hosting),
                )
                    .chain()
                    // In the frame the role changes, so the new link resets the status before
                    // `drop_a_failed_link` can read the last attempt's failure.
                    .after(crate::common::net::apply_net_requests),
            );
    }
}

/// How long a server with clients stays up after it is closed, so its `ServerClosing` and netcode's
/// disconnect packets reach them before the socket goes.
pub const CLOSING_SECONDS: f32 = 0.3;

/// When netcode is stopped, after `ServerClosing` is sent: a client disconnected by netcode drops
/// a message still on its way.
const STOP_AFTER_SECONDS: f32 = 0.1;

/// A server being closed: no longer a [`NetLink`], stopped and then despawned as the time runs.
#[derive(Component)]
struct Closing {
    since: Timer,
    stopped: bool,
}

/// The role's link is still to be opened, once no server is closing: two `Listen`s on one port
/// would not both bind.
#[derive(Resource, Default)]
struct OpenPending(bool);

/// Always despawns, even between two `Listen` ports, since rebinding in place is a half-applied
/// state. A server with clients is closed over [`CLOSING_SECONDS`] instead, so they see "Server
/// Closed" rather than a timeout.
fn close_the_old_link(
    mut commands: Commands,
    role: Res<NetRole>,
    existing: Query<(Entity, Option<&Endpoint>), With<NetLink>>,
    links_of: Query<&LinkOf>,
    mut status: ResMut<NetStatus>,
    mut sender: ServerMultiMessageSender,
    mut pending: ResMut<OpenPending>,
) {
    for (link, server) in &existing {
        let has_clients = links_of.iter().any(|of| of.endpoint == link);
        match server {
            Some(server) if has_clients => {
                info!("Closing the server; telling its clients");
                if let Err(e) = sender.send::<_, MatchChannel>(&ServerClosing, server, &NetworkTarget::All) {
                    warn!("Could not tell the clients the server is closing: {e}");
                }
                commands
                    .entity(link)
                    .remove::<NetLink>()
                    .insert(Closing { since: Timer::from_seconds(CLOSING_SECONDS, TimerMode::Once), stopped: false });
            }
            _ => commands.entity(link).despawn(),
        }
    }
    // Going solo because the link failed keeps saying why.
    if !(*role == NetRole::Solo && matches!(*status, NetStatus::Failed(_))) {
        *status = NetStatus::Offline;
    }
    pending.0 = true;
}

fn finish_closing(mut commands: Commands, time: Res<Time<Real>>, mut closing: Query<(Entity, &mut Closing)>) {
    for (server, mut closing) in &mut closing {
        closing.since.tick(time.delta());
        if !closing.stopped && closing.since.elapsed_secs() >= STOP_AFTER_SECONDS {
            closing.stopped = true;
            commands.trigger(Stop { entity: server });
        }
        if closing.since.is_finished() {
            commands.entity(server).despawn();
        }
    }
}

/// Quitting while clients are connected closes the server first: the exit is held, the role goes
/// solo, and the exit is let through once the server is gone. Runs after the window's own exit
/// systems, which write a new exit every frame once the window is closed.
fn close_the_server_before_quitting(
    mut exits: ResMut<Messages<AppExit>>,
    servers: Query<Entity, (With<NetLink>, With<Started>)>,
    closing: Query<(), With<Closing>>,
    links_of: Query<&LinkOf>,
    mut requests: MessageWriter<NetRequest>,
    mut held: Local<Option<AppExit>>,
) {
    let has_clients = servers.iter().any(|server| links_of.iter().any(|of| of.endpoint == server));
    if held.is_none() && (exits.is_empty() || (!has_clients && closing.is_empty())) {
        return;
    }
    // Every exit asked for meanwhile is kept, an error over a success, so none is lost.
    for exit in exits.drain() {
        if held.as_ref().is_none_or(|held| !held.is_error()) {
            *held = Some(exit);
        }
    }
    if has_clients || !closing.is_empty() {
        if has_clients {
            info!("Closing the server before quitting");
            requests.write(NetRequest::GoSolo);
        }
        return;
    }
    if let Some(exit) = held.take() {
        exits.write(exit);
    }
}

/// The server said it is closing: report it now rather than wait for the link to time out.
fn hear_the_server_close(
    mut receivers: Query<&mut MessageReceiver<ServerClosing>>,
    mut status: ResMut<NetStatus>,
    mut lost: MessageWriter<LinkLost>,
) {
    for mut receiver in &mut receivers {
        if receiver.receive().count() == 0 || matches!(*status, NetStatus::Failed(_)) {
            continue;
        }
        info!("The server is closing");
        let message = crate::get!("net.lost.closed");
        *status = NetStatus::Failed(message.clone());
        lost.write(LinkLost { loss: LinkLoss::Disconnected, message });
    }
}

fn open_link_for_role(
    mut commands: Commands,
    role: Res<NetRole>,
    closing: Query<(), With<Closing>>,
    mut pending: ResMut<OpenPending>,
    mut status: ResMut<NetStatus>,
    round: Option<Res<crate::common::round_settings::RoundSettings>>,
    mut lost: MessageWriter<LinkLost>,
) {
    if !closing.is_empty() {
        return;
    }
    pending.0 = false;
    let mut fail = |status: &mut NetStatus, message: String| {
        *status = NetStatus::Failed(message.clone());
        lost.write(LinkLost { loss: LinkLoss::FailedToConnect, message });
    };

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
                        max_clients: round.as_deref().copied().unwrap_or_default().most_clients(),
                        // `LocalAddr` is `0.0.0.0`, which the check matches only against a loopback
                        // token, so it would ignore a client that joined by the LAN address. With
                        // `DEV_PRIVATE_KEY` in the binary the check proves nothing.
                        server_addr_check: false,
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
                    warn!("Cannot connect to {host}:{port}: {message}");
                    fail(&mut status, crate::get!("net.lost.unresolved", "host", host, "reason", message));
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
                    fail(&mut status, crate::get!("net.lost.no_token"));
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
                    // Port 0: any free local port, in the peer's family, or
                    // nothing an IPv6 server sends can reach it.
                    LocalAddr(SocketAddr::new(unspecified_like(address), 0)),
                    PeerAddr(address),
                ))
                .id();
            commands.trigger(Connect { entity: client });
            *status = NetStatus::Connecting;
            info!("Connecting to {address} as {client_id}");
        }
    }
}

fn unspecified_like(peer: SocketAddr) -> IpAddr {
    match peer {
        SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }
}

/// Once we stop being a client nothing will update or despawn what the server
/// sent, so it goes, and a round we were only watching ends, back in the mode we
/// joined from: as the authority now, we would otherwise be playing on a world
/// nobody set up.
fn leave_the_servers_world(
    mut commands: Commands,
    role: Res<NetRole>,
    mut was_client: Local<bool>,
    replicated: Query<Entity, With<Replicated>>,
    mode: Option<Res<State<crate::common::app_mode::AppMode>>>,
    next: Option<ResMut<NextState<crate::common::app_mode::AppMode>>>,
    joined: Option<Res<crate::common::app_mode::JoinedFrom>>,
) {
    let client = matches!(*role, NetRole::Client { .. });
    if !std::mem::replace(&mut *was_client, client) || client {
        return;
    }
    for entity in &replicated {
        commands.entity(entity).despawn();
    }
    if mode.is_some_and(|mode| *mode.get() == crate::common::app_mode::AppMode::Play)
        && let Some(mut next) = next
    {
        next.set(joined.as_deref().copied().unwrap_or_default().after_leaving());
    }
}

/// Every client gets its own `LinkOf` child, and replication is addressed
/// through it. Without a `ReplicationSender` the handshake completes, nothing
/// flows, and the client times out.
fn dress_new_client(add: On<Add<LinkOf>>, mut commands: Commands) {
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
    mut commands: Commands,
    mut status: ResMut<NetStatus>,
    servers: Query<Entity, (With<NetLink>, With<Started>)>,
    clients: Query<
        (Entity, Option<&Connected>, Option<&Connecting>, Option<&Disconnected>, Has<WasConnected>),
        (With<NetLink>, With<lightyear::prelude::client::Client>),
    >,
    links_of: Query<&LinkOf>,
    mut lost: MessageWriter<LinkLost>,
) {
    // A failure stays until the next role change, so it does not blink past
    // on its way back to `Offline`.
    if matches!(*status, NetStatus::Failed(_)) {
        return;
    }

    let next = if let Some(server) = servers.iter().next() {
        NetStatus::Hosting {
            clients: links_of.iter().filter(|link| link.endpoint == server).count(),
        }
    } else if let Some((link, connected, connecting, disconnected, was_connected)) = clients.iter().next() {
        match (connected, connecting, disconnected) {
            (Some(_), _, _) => {
                if !was_connected {
                    commands.entity(link).insert(WasConnected);
                }
                NetStatus::Connected
            }
            (_, Some(_), _) => NetStatus::Connecting,
            // `Unknown` is also the state before any attempt; not a failure.
            (_, _, Some(disconnected)) => match &disconnected.reason {
                DisconnectedReason::Unknown => NetStatus::Connecting,
                reason => {
                    info!("Lost the link to the server: {reason}");
                    let message = explain(reason);
                    let loss = match was_connected {
                        true => LinkLoss::Disconnected,
                        false => LinkLoss::FailedToConnect,
                    };
                    lost.write(LinkLost { loss, message: message.clone() });
                    NetStatus::Failed(message)
                }
            },
            _ => NetStatus::Connecting,
        }
    } else {
        NetStatus::Offline
    };

    status.set_if_neq(next);
}

/// Going solo clears the spectating and Disconnect; [`NetStatus::Failed`]
/// still says why.
fn drop_a_failed_link(status: Res<NetStatus>, role: Res<NetRole>, mut requests: MessageWriter<NetRequest>) {
    if matches!(*status, NetStatus::Failed(_)) && role.is_online() {
        requests.write(NetRequest::GoSolo);
    }
}

/// Blocking DNS on the main thread: native only and once per menu click.
/// WebTransport resolves its own URLs, so the browser will not come here.
///
/// IPv4 first: a listen server binds IPv4 only, and macOS resolves
/// `localhost` to `::1` first, which times out against it. A host with only
/// an IPv6 address still connects to an IPv6 server.
fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    use std::net::ToSocketAddrs;

    let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs().map_err(|error| error.to_string())?.collect();
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or(addresses.first())
        .copied()
        .ok_or_else(|| format!("{host} resolved to no addresses"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get;

    #[test]
    fn a_hostname_that_exists_resolves() {
        assert!(resolve("localhost", 27100).is_ok());
    }

    #[test]
    fn a_hostname_that_does_not_is_an_error_rather_than_a_panic() {
        assert!(resolve("this-host-does-not-exist.invalid", 27100).is_err());
    }

    #[test]
    fn a_failed_link_goes_solo_once() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<Messages<NetRequest>>();
        world.insert_resource(NetStatus::Failed("timed out".into()));
        world.insert_resource(NetRole::Client { host: "example".into(), port: 1 });
        world.run_system_once(drop_a_failed_link).unwrap();
        let sent = |world: &World| world.resource::<Messages<NetRequest>>().len();
        assert_eq!(sent(&world), 1);
        assert!(matches!(world.resource::<Messages<NetRequest>>().iter_current_update_messages().next(), Some(NetRequest::GoSolo)));

        *world.resource_mut::<NetRole>() = NetRole::Solo;
        world.run_system_once(drop_a_failed_link).unwrap();
        assert_eq!(sent(&world), 1, "already solo, and asked again");
    }

    #[test]
    fn localhost_resolves_to_ipv4_where_the_listen_server_is() {
        assert!(resolve("localhost", 27100).unwrap().is_ipv4());
    }

    #[test]
    fn a_client_binds_in_its_peers_family() {
        assert!(unspecified_like("127.0.0.1:1".parse().unwrap()).is_ipv4());
        assert!(unspecified_like("[::1]:1".parse().unwrap()).is_ipv6());
    }

    #[test]
    fn leaving_a_server_mid_round_drops_what_it_sent_and_goes_back_to_the_editor() {
        use crate::common::app_mode::AppMode;
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppMode::Play)
            .insert_resource(NetRole::Client { host: "example".into(), port: 1 })
            .add_systems(Update, leave_the_servers_world.run_if(resource_changed::<NetRole>));
        app.update();
        let theirs = app.world_mut().spawn(Replicated).id();
        let ours = app.world_mut().spawn_empty().id();

        *app.world_mut().resource_mut::<NetRole>() = NetRole::Solo;
        app.update();
        app.update();
        assert!(app.world().get_entity(theirs).is_err(), "the server's entity outlived the link");
        assert!(app.world().get_entity(ours).is_ok());
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Editor);
    }

    /// An app with the real transport and nothing else; every system missing a resource from the
    /// rest of the game is skipped.
    fn networked_app(mode: crate::common::app_mode::AppMode) -> App {
        let mut app = App::new();
        app.set_error_handler(bevy::ecs::error::ignore);
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin, bevy::asset::AssetPlugin::default()))
            .insert_state(mode)
            .add_plugins(crate::common::net::NetPlugin)
            .add_plugins(NetTransportPlugin)
            .init_resource::<Lost>()
            .add_systems(Last, |mut lost: MessageReader<LinkLost>, mut kept: ResMut<Lost>| kept.0.extend(lost.read().cloned()));
        app.finish();
        app.cleanup();
        app
    }

    #[derive(Resource, Default)]
    struct Lost(Vec<LinkLost>);

    fn lost(app: &App) -> &[LinkLost] {
        &app.world().resource::<Lost>().0
    }

    fn run_until(apps: &mut [&mut App], seconds: f32, done: impl Fn(&[&mut App]) -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed().as_secs_f32() < seconds {
            for app in apps.iter_mut() {
                app.update();
            }
            if done(apps) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        false
    }

    /// A UDP port nothing is bound to, by binding port 0 and letting it go. Another process could
    /// take it in between, but tests running at once never draw the same one.
    fn free_port() -> u16 {
        std::net::UdpSocket::bind("0.0.0.0:0").and_then(|socket| socket.local_addr()).expect("a free port").port()
    }

    /// This machine's address on its LAN, found by routing a socket without sending anything.
    fn lan_address() -> Option<IpAddr> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("10.254.254.254:1").ok()?;
        Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
    }

    /// Fails if a retry after a timeout reads the last attempt's failure and goes solo at once.
    #[test]
    fn a_retry_after_a_failed_connect_tries_again() {
        let mut app = networked_app(crate::common::app_mode::AppMode::Editor);
        let join = |app: &mut App| app.world_mut().write_message(NetRequest::Join { host: "127.0.0.1".into(), port: 1 });
        join(&mut app);
        let failed = |apps: &[&mut App]| matches!(*apps[0].world().resource::<NetRole>(), NetRole::Solo)
            && matches!(*apps[0].world().resource::<NetStatus>(), NetStatus::Failed(_));
        assert!(run_until(&mut [&mut app], 10.0, failed), "the first attempt never failed");
        assert_eq!(lost(&app), [LinkLost { loss: LinkLoss::FailedToConnect, message: get!("net.lost.no_answer") }]);

        join(&mut app);
        let connecting = |apps: &[&mut App]| *apps[0].world().resource::<NetStatus>() == NetStatus::Connecting;
        assert!(run_until(&mut [&mut app], 1.0, connecting), "the retry did not try");
        assert!(!run_until(&mut [&mut app], 1.0, failed), "the retry went solo before timing out");
    }

    /// Fails if a client joining by the host's LAN address is ignored, if it joins a round in the
    /// editor, or if it does not follow the host in and out of Play.
    #[test]
    fn a_client_joins_by_lan_address_and_follows_the_host() {
        use crate::common::app_mode::AppMode;
        let host_ip = lan_address().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let port = free_port();

        let mut host = networked_app(AppMode::Play);
        host.world_mut().write_message(NetRequest::Host { port });
        let mut client = networked_app(AppMode::Editor);
        let mode = |app: &App| *app.world().resource::<State<AppMode>>().get();
        assert!(run_until(&mut [&mut host], 2.0, |apps| matches!(*apps[0].world().resource::<NetStatus>(), NetStatus::Hosting { .. })));

        client.world_mut().write_message(NetRequest::Join { host: host_ip.to_string(), port });
        let playing = |want: AppMode| move |apps: &[&mut App]| *apps[1].world().resource::<State<AppMode>>().get() == want;
        assert!(
            run_until(&mut [&mut host, &mut client], 10.0, playing(AppMode::Play)),
            "joined {host_ip} mid-round and is in {:?}, status {:?}",
            mode(&client),
            client.world().resource::<NetStatus>(),
        );

        host.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        assert!(run_until(&mut [&mut host, &mut client], 5.0, playing(AppMode::Editor)), "stayed in Play after the host left it");
        host.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        assert!(run_until(&mut [&mut host, &mut client], 5.0, playing(AppMode::Play)), "stayed in the editor when the host started a round");
        assert!(lost(&client).is_empty(), "{:?}", lost(&client));
    }

    fn connected_pair() -> (App, App) {
        use crate::common::app_mode::AppMode;
        let port = free_port();
        let mut host = networked_app(AppMode::Editor);
        host.world_mut().write_message(NetRequest::Host { port });
        let mut client = networked_app(AppMode::Editor);
        assert!(run_until(&mut [&mut host], 2.0, |apps| matches!(*apps[0].world().resource::<NetStatus>(), NetStatus::Hosting { .. })));
        client.world_mut().write_message(NetRequest::Join { host: "127.0.0.1".into(), port });
        let both = |apps: &[&mut App]| {
            *apps[1].world().resource::<NetStatus>() == NetStatus::Connected
                && *apps[0].world().resource::<NetStatus>() == NetStatus::Hosting { clients: 1 }
        };
        assert!(run_until(&mut [&mut host, &mut client], 10.0, both), "never connected");
        (host, client)
    }

    fn closed() -> LinkLost {
        LinkLost { loss: LinkLoss::Disconnected, message: get!("net.lost.closed") }
    }

    /// Fails if a host going solo leaves its clients to time out rather than say the server closed.
    #[test]
    fn a_host_that_closes_the_server_tells_its_clients() {
        let (mut host, mut client) = connected_pair();
        host.world_mut().write_message(NetRequest::GoSolo);
        let told = |apps: &[&mut App]| !lost(apps[1]).is_empty();
        assert!(run_until(&mut [&mut host, &mut client], 1.0, told), "the client was not told within a second");
        assert_eq!(lost(&client), [closed()]);
        assert!(run_until(&mut [&mut host, &mut client], 1.0, |apps| *apps[1].world().resource::<NetRole>() == NetRole::Solo));
    }

    /// Fails if quitting with clients connected leaves before telling them, or never leaves.
    #[test]
    fn quitting_while_hosting_closes_the_server_first() {
        let (mut host, mut client) = connected_pair();
        host.world_mut().write_message(AppExit::Success);
        host.update();
        assert_eq!(host.should_exit(), None, "quit before closing the server");
        let quit = |apps: &[&mut App]| apps[0].should_exit().is_some();
        assert!(run_until(&mut [&mut host, &mut client], 2.0, quit), "never quit");
        assert!(run_until(&mut [&mut client], 1.0, |apps| !lost(apps[0]).is_empty()));
        assert_eq!(lost(&client), [closed()]);
    }

    /// Fails if a quit asked for just after the server was closed leaves before the close is done,
    /// or an error asked for meanwhile is lost.
    #[test]
    fn a_quit_during_a_close_waits_and_keeps_an_error() {
        let (mut host, mut client) = connected_pair();
        host.world_mut().write_message(NetRequest::GoSolo);
        host.update();
        host.world_mut().write_message(AppExit::Success);
        host.update();
        assert_eq!(host.should_exit(), None, "quit before the close was done");
        host.world_mut().write_message(AppExit::error());
        let quit = |apps: &[&mut App]| apps[0].should_exit().is_some();
        assert!(run_until(&mut [&mut host, &mut client], 2.0, quit), "never quit");
        assert!(host.should_exit().is_some_and(|exit| exit.is_error()), "the error was lost");
    }

    /// Fails if a connection that worked and then went silent reads as a failure to connect.
    #[test]
    fn a_server_that_vanishes_reads_as_a_disconnect() {
        let (host, mut client) = connected_pair();
        drop(host);
        assert!(run_until(&mut [&mut client], 10.0, |apps| !lost(apps[0]).is_empty()), "never noticed");
        assert_eq!(
            lost(&client),
            [LinkLost { loss: LinkLoss::Disconnected, message: get!("net.lost.stopped_answering") }],
        );
    }

    /// Losing the port would connect to the right machine on the wrong port.
    #[test]
    fn resolving_keeps_the_port() {
        assert_eq!(resolve("127.0.0.1", 27016).unwrap().port(), 27016);
    }
}
