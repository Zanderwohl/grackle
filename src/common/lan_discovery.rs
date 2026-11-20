//! Finding listen servers on the local network without typing an address.
//!
//! While the server browser is open, a client broadcasts a [`Question`] to [`DISCOVERY_PORT`] on
//! `255.255.255.255` every [`ASK_EVERY`] seconds. Every listen server whose [`ServerListing`]
//! is discoverable answers a sender on a local address with a [`Reply`] describing itself. Each
//! server binds the discovery port with `SO_REUSEADDR` and `SO_REUSEPORT`, so several on one
//! machine all hear a broadcast. Sockets are non-blocking and read at both ends of each frame, with
//! no threads. Answers are throttled per address and in all ([`Throttle`]).
//!
//! A browser tab cannot send a UDP broadcast, so on wasm no socket opens and nothing is found.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::Connected;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::map_sync::ServerMapFile;
use crate::common::net::{is_local, NetRole};
use crate::common::player_settings::PlayerSettings;
use crate::common::round_settings::{RoundSettings, ServerListing};
use crate::constants::{DISCOVERY_PORT, NETCODE_PROTOCOL_ID};
use crate::editor::map_metadata::MapMetadata;
use crate::editor::panels::CurrentFilePath;
use crate::game::respawn::{Identity, OurIdentity, Seat};

/// Seconds between queries while the browser is open.
pub const ASK_EVERY: f64 = 2.0;
/// Seconds a server stays listed after its last reply: three queries, so one lost reply does not
/// drop it.
pub const FORGET_AFTER: f64 = 3.0 * ASK_EVERY + 0.5;

/// The most characters each string in a [`Reply`] may hold. At four bytes a character they come to
/// 1088 bytes, so a reply fits in one datagram on any link.
pub const NAME_CHARS: usize = 48;
pub const MOTD_CHARS: usize = 128;
pub const MAP_CHARS: usize = 48;
pub const MODE_CHARS: usize = 16;
pub const BUILD_CHARS: usize = 32;

const QUESTION_MAGIC: [u8; 4] = *b"GRKQ";
const REPLY_MAGIC: [u8; 4] = *b"GRKR";
/// Bump on any change to [`Question`] or [`Reply`]. Separate from [`NETCODE_PROTOCOL_ID`], which a
/// reply carries so a client can tell it could not join.
const DISCOVERY_VERSION: u8 = 1;
/// Larger than any datagram either side sends, so a longer one is truncated and fails to decode.
const DATAGRAM: usize = 1500;
/// Nonces whose send time is kept for measuring ping; replies to older queries are dropped.
const KEPT_NONCES: usize = 4;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Question {
    pub nonce: u64,
}

/// What a server says about itself. Its address is the reply's source address, since a server bound
/// to every interface does not know which of its addresses a client can reach.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The question's, to time the round trip.
    pub nonce: u64,
    pub netcode_protocol: u64,
    /// `CARGO_PKG_VERSION`.
    pub build: String,
    /// Where the game itself listens.
    pub port: u16,
    /// Blank when neither the listing nor the host's player settings name it.
    pub name: String,
    pub motd: String,
    /// Blank for a map never saved.
    pub map: String,
    /// [`crate::common::mode::GameMode::prefix`], so each client names it in its own language.
    pub mode: String,
    /// People taking a slot: the host and every connected client, but not spectators or bots.
    pub people: u16,
    pub slots: u16,
    pub bots_enabled: bool,
}

impl Reply {
    fn within_caps(&self) -> bool {
        let fits = |text: &str, most: usize| text.chars().count() <= most;
        fits(&self.build, BUILD_CHARS)
            && fits(&self.name, NAME_CHARS)
            && fits(&self.motd, MOTD_CHARS)
            && fits(&self.map, MAP_CHARS)
            && fits(&self.mode, MODE_CHARS)
    }

    fn capped(mut self) -> Self {
        for (text, most) in [
            (&mut self.build, BUILD_CHARS),
            (&mut self.name, NAME_CHARS),
            (&mut self.motd, MOTD_CHARS),
            (&mut self.map, MAP_CHARS),
            (&mut self.mode, MODE_CHARS),
        ] {
            if let Some((cut, _)) = text.char_indices().nth(most) {
                text.truncate(cut);
            }
        }
        self
    }
}

fn encode<T: Serialize>(magic: [u8; 4], body: &T) -> Vec<u8> {
    let mut bytes = magic.to_vec();
    bytes.push(DISCOVERY_VERSION);
    // Postcard cannot fail on these types: no maps, no untagged enums.
    bytes.extend(postcard::to_stdvec(body).unwrap_or_default());
    bytes
}

fn decode<'a, T: Deserialize<'a>>(magic: [u8; 4], bytes: &'a [u8]) -> Option<T> {
    let body = bytes.strip_prefix(&magic)?.strip_prefix(&[DISCOVERY_VERSION])?;
    match postcard::take_from_bytes(body) {
        Ok((value, [])) => Some(value),
        _ => None,
    }
}

pub fn encode_question(query: &Question) -> Vec<u8> {
    encode(QUESTION_MAGIC, query)
}

pub fn decode_question(bytes: &[u8]) -> Option<Question> {
    decode(QUESTION_MAGIC, bytes)
}

/// Caps every string first, so what is sent always decodes.
pub fn encode_reply(reply: Reply) -> Vec<u8> {
    encode(REPLY_MAGIC, &reply.capped())
}

/// Refuses a reply with a string past its cap rather than trusting the sender to have cut it.
pub fn decode_reply(bytes: &[u8]) -> Option<Reply> {
    decode::<Reply>(REPLY_MAGIC, bytes).filter(Reply::within_caps)
}

#[derive(Debug, Clone, PartialEq)]
pub struct FoundServer {
    /// The reply's source address with the game port it named: what joining connects to.
    pub address: SocketAddr,
    pub reply: Reply,
    /// The lowest of the last [`PING_SAMPLES`] round trips, in milliseconds. Each side reads only
    /// twice a frame, so the lowest is the closest to the network's own.
    pub ping_ms: f32,
    /// Newest last.
    pub pings: VecDeque<f32>,
    /// [`now`] seconds.
    pub last_seen: f64,
}

impl FoundServer {
    /// Netcode refuses a handshake between builds whose protocol ids differ.
    pub fn joinable(&self) -> bool {
        self.reply.netcode_protocol == NETCODE_PROTOCOL_ID
    }
}

pub const PING_SAMPLES: usize = 4;
/// Seconds before a discovery port that would not bind is tried again.
const BIND_RETRY: f64 = 5.0;
/// Seconds before the same address is answered again.
const ANSWER_EVERY: f64 = 1.0;
/// Replies sent in any one second, to everybody together.
const MOST_ANSWERS_PER_SECOND: usize = 20;

/// Seconds on a monotonic clock, read when called rather than at the frame's start, so a round
/// trip is timed from the send to the read.
fn now() -> f64 {
    static START: std::sync::OnceLock<bevy::platform::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(bevy::platform::time::Instant::now).elapsed().as_secs_f64()
}

/// The servers found since the main menu opened. Inserted on entering [`AppMode::MainMenu`] and
/// removed on leaving it, so a list nobody can see holds no memory.
#[derive(Resource, Default)]
pub struct ServerList {
    pub servers: Vec<FoundServer>,
    /// Set by the server browser while it is open; queries go out only while it is.
    pub searching: bool,
    socket: Option<UdpSocket>,
    /// Nonces sent, newest last, with when.
    sent: VecDeque<(u64, f64)>,
    last_question: Option<f64>,
    /// The socket would not open; not tried again until the browser is closed and opened.
    failed: bool,
    /// The last broadcast failed, so the next failure is not warned about again.
    broadcast_failing: bool,
}

impl ServerList {
    /// Replaces what that address said before.
    pub fn file(&mut self, from: SocketAddr, reply: Reply, now: f64) {
        let Some(&(_, sent)) = self.sent.iter().find(|(nonce, _)| *nonce == reply.nonce) else { return };
        let address = SocketAddr::new(from.ip(), reply.port);
        let ping = ((now - sent) * 1000.0) as f32;
        let mut pings = self.servers.iter().find(|server| server.address == address).map(|server| server.pings.clone()).unwrap_or_default();
        pings.push_back(ping);
        while pings.len() > PING_SAMPLES {
            pings.pop_front();
        }
        let ping_ms = pings.iter().copied().fold(f32::INFINITY, f32::min);
        let found = FoundServer { address, ping_ms, pings, last_seen: now, reply };
        match self.servers.iter_mut().find(|server| server.address == address) {
            Some(server) => *server = found,
            None => self.servers.push(found),
        }
    }

    fn forget_the_silent(&mut self, now: f64) {
        self.servers.retain(|server| now - server.last_seen <= FORGET_AFTER);
    }

    /// A nonce to send at `now`, kept for timing its replies; `None` before [`ASK_EVERY`] has
    /// passed since the last.
    fn next_question(&mut self, now: f64) -> Option<Question> {
        if self.last_question.is_some_and(|last| now - last < ASK_EVERY) {
            return None;
        }
        self.last_question = Some(now);
        let nonce = rand::random::<u64>();
        self.sent.push_back((nonce, now));
        while self.sent.len() > KEPT_NONCES {
            self.sent.pop_front();
        }
        Some(Question { nonce })
    }
}

/// How often the responder may answer: once per [`ANSWER_EVERY`] to an address, and
/// [`MOST_ANSWERS_PER_SECOND`] in all. A reply is up to 85 times the size of a question and UDP's
/// source address can be forged, so this caps what a forger can make the server send.
#[derive(Default)]
struct Throttle {
    last_answered: std::collections::HashMap<SocketAddr, f64>,
    /// When each reply in the last second went out, oldest first.
    recent: VecDeque<f64>,
}

impl Throttle {
    /// Whether `to` may be answered at `now`, counting the answer if so. An address is its IP and
    /// port, so two browsers on one machine are answered apart.
    fn allow(&mut self, to: SocketAddr, now: f64) -> bool {
        while self.recent.front().is_some_and(|sent| now - sent >= 1.0) {
            self.recent.pop_front();
        }
        self.last_answered.retain(|_, answered| now - *answered < ANSWER_EVERY);
        if self.last_answered.contains_key(&to) || self.recent.len() >= MOST_ANSWERS_PER_SECOND {
            return false;
        }
        self.last_answered.insert(to, now);
        self.recent.push_back(now);
        true
    }
}

/// The listen server's discovery socket, open while it is hosting and its listing is discoverable.
#[derive(Resource, Default)]
struct Responder {
    socket: Option<UdpSocket>,
    /// After a bind fails, when to try again.
    retry_at: Option<f64>,
    /// A bind has failed since the socket last opened, so later failures are not warned about.
    warned: bool,
    throttle: Throttle,
}

pub struct LanDiscoveryPlugin;

impl Plugin for LanDiscoveryPlugin {
    fn build(&self, app: &mut App) {
        let searching = resource_exists::<ServerList>;
        // Read at both ends of the frame, so a datagram waits at most about half a frame.
        app.init_resource::<Responder>()
            .add_systems(OnEnter(AppMode::MainMenu), |mut commands: Commands| commands.init_resource::<ServerList>())
            .add_systems(OnExit(AppMode::MainMenu), |mut commands: Commands| commands.remove_resource::<ServerList>())
            .add_systems(First, (answer_questions, read_replies.run_if(searching)))
            .add_systems(Update, (open_or_close_the_responder, ask.run_if(searching)))
            .add_systems(Last, (answer_questions, read_replies.run_if(searching)));
    }
}

fn open_or_close_the_responder(role: Option<Res<NetRole>>, listing: Option<Res<ServerListing>>, mut responder: ResMut<Responder>) {
    let hosting = matches!(role.as_deref(), Some(NetRole::Listen { .. }));
    let wanted = hosting && listing.is_some_and(|listing| listing.discoverable);
    if !wanted {
        if responder.socket.take().is_some() {
            info!("Stopped answering LAN discovery");
        }
        responder.retry_at = None;
        responder.warned = false;
        return;
    }
    let now = now();
    if responder.socket.is_some() || responder.retry_at.is_some_and(|at| now < at) {
        return;
    }
    match sockets::responder(DISCOVERY_PORT) {
        Ok(socket) => {
            info!("Answering LAN discovery on port {DISCOVERY_PORT}");
            *responder = Responder { socket: Some(socket), ..default() };
        }
        Err(error) => {
            match responder.warned {
                false => warn!("Cannot answer LAN discovery on port {DISCOVERY_PORT}: {error}; trying again every {BIND_RETRY} s"),
                true => debug!("Cannot answer LAN discovery on port {DISCOVERY_PORT}: {error}"),
            }
            responder.warned = true;
            responder.retry_at = Some(now + BIND_RETRY);
        }
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct Describe<'w, 's> {
    role: Option<Res<'w, NetRole>>,
    listing: Option<Res<'w, ServerListing>>,
    player: Option<Res<'w, PlayerSettings>>,
    round: Option<Res<'w, RoundSettings>>,
    metadata: Option<Res<'w, MapMetadata>>,
    server_file: Option<Res<'w, ServerMapFile>>,
    file: Option<Res<'w, CurrentFilePath>>,
    us: Option<Res<'w, OurIdentity>>,
    links: Query<'w, 's, Option<&'static Identity>, (With<ClientOf>, With<Connected>)>,
}

impl Describe<'_, '_> {
    fn reply(&self, nonce: u64) -> Option<Reply> {
        let Some(NetRole::Listen { port }) = self.role.as_deref() else { return None };
        let takes_a_slot = |identity: Option<&Identity>| identity.is_none_or(|who| who.seat != Seat::Spectating);
        let people = std::iter::once(self.us.as_deref().map(|us| &us.0)).chain(self.links.iter()).filter(|who| takes_a_slot(*who)).count();
        let round = self.round.as_deref().copied().unwrap_or_default();
        let listing = self.listing.as_deref().cloned().unwrap_or_default();
        let name = match listing.name.trim() {
            "" => self.player.as_deref().map(|player| player.name.trim().to_owned()).unwrap_or_default(),
            name => name.to_owned(),
        };
        Some(Reply {
            nonce,
            netcode_protocol: NETCODE_PROTOCOL_ID,
            build: env!("CARGO_PKG_VERSION").to_owned(),
            port: *port,
            name,
            motd: listing.motd.trim().to_owned(),
            map: crate::game::match_banner::map_name(self.metadata.as_deref(), self.server_file.as_deref(), self.file.as_deref()),
            mode: self.metadata.as_deref().map_or(crate::common::mode::GameMode::Arena, |metadata| metadata.game_mode).prefix().to_owned(),
            people: people.min(u16::MAX as usize) as u16,
            slots: round.slots.min(u32::from(u16::MAX)) as u16,
            bots_enabled: round.bots_enabled,
        })
    }
}

fn answer_questions(mut responder: ResMut<Responder>, describe: Describe) {
    let Responder { socket: Some(socket), throttle, .. } = &mut *responder else { return };
    let mut buffer = [0u8; DATAGRAM];
    loop {
        let (length, from) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
            Err(error) => {
                // Tried again next time: an error that repeats would otherwise hold the frame here.
                debug!("LAN discovery read failed: {error}");
                return;
            }
        };
        if !is_local(from.ip()) {
            continue;
        }
        let Some(question) = decode_question(&buffer[..length]) else { continue };
        if !throttle.allow(from, now()) {
            continue;
        }
        let Some(reply) = describe.reply(question.nonce) else { return };
        if let Err(error) = socket.send_to(&encode_reply(reply), from) {
            debug!("Could not answer LAN discovery from {from}: {error}");
        }
    }
}

fn ask(mut list: ResMut<ServerList>) {
    if !list.searching {
        list.socket = None;
        list.last_question = None;
        list.failed = false;
        return;
    }
    if list.failed {
        return;
    }
    if list.socket.is_none() {
        match sockets::seeker() {
            Ok(socket) => list.socket = Some(socket),
            Err(error) => {
                warn!("Cannot search the LAN for servers: {error}");
                list.failed = true;
                return;
            }
        }
    }
    let now = now();
    let Some(question) = list.next_question(now) else { return };
    let to = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DISCOVERY_PORT);
    let Some(socket) = &list.socket else { return };
    match socket.send_to(&encode_question(&question), to) {
        Ok(_) => list.broadcast_failing = false,
        Err(error) if !list.broadcast_failing => {
            warn!("Could not broadcast a LAN discovery query: {error}; trying again every {ASK_EVERY} s");
            list.broadcast_failing = true;
        }
        Err(error) => debug!("Could not broadcast a LAN discovery query: {error}"),
    }
}

fn read_replies(mut list: ResMut<ServerList>) {
    let mut buffer = [0u8; DATAGRAM];
    loop {
        let Some(socket) = &list.socket else { break };
        match socket.recv_from(&mut buffer) {
            Ok((length, from)) => {
                if let Some(reply) = decode_reply(&buffer[..length]) {
                    list.file(from, reply, now());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => {
                debug!("LAN discovery read failed: {error}");
                break;
            }
        }
    }
    if list.searching {
        list.forget_the_silent(now());
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod sockets {
    use std::io;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

    use socket2::{Domain, Protocol, Socket, Type};

    /// Bound to `port` on every interface, shared with any other server on this machine.
    pub fn responder(port: u16) -> io::Result<UdpSocket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))))]
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
        Ok(socket.into())
    }

    /// Any free port, allowed to broadcast.
    pub fn seeker() -> io::Result<UdpSocket> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        socket.set_nonblocking(true)?;
        Ok(socket)
    }
}

#[cfg(target_arch = "wasm32")]
mod sockets {
    use std::io;
    use std::net::UdpSocket;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "a browser cannot send a UDP broadcast")
    }

    pub fn responder(_port: u16) -> io::Result<UdpSocket> {
        Err(unsupported())
    }

    pub fn seeker() -> io::Result<UdpSocket> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(nonce: u64) -> Reply {
        Reply {
            nonce,
            netcode_protocol: NETCODE_PROTOCOL_ID,
            build: "0.1.0".into(),
            port: 27100,
            name: "Upstairs".into(),
            motd: "Rockets only".into(),
            map: "Foundry".into(),
            mode: "arena".into(),
            people: 2,
            slots: 8,
            bots_enabled: true,
        }
    }

    #[test]
    fn a_query_and_a_reply_survive_the_wire() {
        let query = Question { nonce: 0xfeed_beef };
        assert_eq!(decode_question(&encode_question(&query)), Some(query));
        assert_eq!(decode_reply(&encode_reply(reply(7))), Some(reply(7)));
    }

    /// Breaks if a datagram meant for something else, another version, or one cut short or padded
    /// is read as a query or reply.
    #[test]
    fn anything_else_on_the_port_is_ignored() {
        let bytes = encode_reply(reply(7));
        assert_eq!(decode_question(&bytes), None, "a reply read as a query");
        assert_eq!(decode_reply(&bytes[..bytes.len() - 1]), None, "a truncated reply read");
        assert_eq!(decode_reply(&[bytes.as_slice(), &[0]].concat()), None, "trailing bytes read");
        let mut other_version = bytes.clone();
        other_version[4] = DISCOVERY_VERSION + 1;
        assert_eq!(decode_reply(&other_version), None, "another version read");
        assert_eq!(decode_question(b"GET / HTTP/1.1"), None);
    }

    /// Breaks if a long string is sent uncut, cut mid-character, or a reply over the caps is read.
    #[test]
    fn strings_are_capped_both_ways() {
        let long = Reply { name: "é".repeat(NAME_CHARS + 10), motd: "x".repeat(MOTD_CHARS * 2), ..reply(1) };
        let read = decode_reply(&encode_reply(long.clone())).expect("a capped reply did not decode");
        assert_eq!((read.name.chars().count(), read.motd.chars().count()), (NAME_CHARS, MOTD_CHARS));

        let uncut = encode(REPLY_MAGIC, &long);
        assert_eq!(decode_reply(&uncut), None, "a reply past the caps was trusted");

        let widest = Reply {
            build: "😀".repeat(BUILD_CHARS),
            name: "😀".repeat(NAME_CHARS),
            motd: "😀".repeat(MOTD_CHARS),
            map: "😀".repeat(MAP_CHARS),
            mode: "😀".repeat(MODE_CHARS),
            ..reply(u64::MAX)
        };
        assert!(encode_reply(widest).len() <= 1200, "the widest reply does not fit a datagram");
    }

    /// Breaks if a reply is timed against the wrong query, a stale nonce is accepted, the same
    /// server is listed twice, or a server is never forgotten.
    #[test]
    fn the_list_times_files_and_forgets() {
        let mut list = ServerList::default();
        let first = list.next_question(10.0).unwrap();
        assert_eq!(list.next_question(11.0), None, "queried again inside the interval");
        let from: SocketAddr = "192.168.1.20:53111".parse().unwrap();

        list.file(from, reply(first.nonce), 10.05);
        assert_eq!(list.servers.len(), 1);
        assert_eq!(list.servers[0].address, "192.168.1.20:27100".parse().unwrap(), "joined by the reply's source port");
        assert!((list.servers[0].ping_ms - 50.0).abs() < 0.5);

        let second = list.next_question(12.0).unwrap();
        list.file(from, Reply { people: 3, ..reply(second.nonce) }, 12.02);
        assert_eq!(list.servers.len(), 1, "one server listed twice");
        assert_eq!(list.servers[0].reply.people, 3);
        assert!((list.servers[0].ping_ms - 20.0).abs() < 0.5, "the ping is not the lowest of the recent round trips");

        list.file(from, reply(12345), 12.03);
        assert_eq!(list.servers[0].reply.people, 3, "a reply to a query never sent was filed");
        let third = list.next_question(14.0).unwrap();
        list.file(from, reply(third.nonce), 14.09);
        assert!((list.servers[0].ping_ms - 20.0).abs() < 0.5, "a slow round trip raised the ping above a recent faster one");

        list.forget_the_silent(14.09 + FORGET_AFTER - 0.1);
        assert_eq!(list.servers.len(), 1);
        list.forget_the_silent(14.09 + FORGET_AFTER + 0.1);
        assert!(list.servers.is_empty(), "a silent server stayed listed");
    }

    /// Breaks if an address is answered twice inside a second, two ports on one machine are not
    /// answered apart, or more than the cap is answered in a second.
    #[test]
    fn answers_are_throttled_per_address_and_in_all() {
        let mut throttle = Throttle::default();
        let at = |port: u16| SocketAddr::from(([192, 168, 1, 7], port));
        assert!(throttle.allow(at(5000), 10.0));
        assert!(!throttle.allow(at(5000), 10.5), "the same address was answered twice in a second");
        assert!(throttle.allow(at(5001), 10.5), "another port on the same machine was refused");
        assert!(throttle.allow(at(5000), 11.0), "the address was refused after a second");

        let mut throttle = Throttle::default();
        let answered = (0..30).filter(|port| throttle.allow(at(6000 + port), 20.0 + f64::from(*port) * 0.01)).count();
        assert_eq!(answered, MOST_ANSWERS_PER_SECOND);
        assert!(throttle.allow(at(7000), 21.0), "the cap did not reopen after a second");
    }

    /// Breaks if a second server on the same machine cannot bind the discovery port. On a port of
    /// its own, so a running server is not in the way.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn two_servers_share_the_discovery_port() {
        let port = 47101;
        let first = sockets::responder(port).expect("the first server could not bind");
        let second = sockets::responder(port).expect("a second server on the same machine could not bind");
        drop((first, second));
    }

    /// Breaks if the responder answers a query with nothing hosting, or a query sent to it over
    /// loopback goes unanswered while hosting.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_listen_server_answers_a_query() {
        let mut app = App::new();
        app.insert_resource(NetRole::Listen { port: 27999 });
        app.insert_resource(ServerListing { name: "Test".into(), motd: "hi".into(), discoverable: true });
        app.insert_resource(RoundSettings { slots: 5, ..default() });
        app.init_resource::<Responder>();
        app.add_systems(Update, answer_questions);
        app.world_mut().resource_mut::<Responder>().socket = Some(sockets::responder(0).unwrap());
        let server = app.world().resource::<Responder>().socket.as_ref().unwrap().local_addr().unwrap().port();

        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        client.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        client.send_to(&encode_question(&Question { nonce: 99 }), (Ipv4Addr::LOCALHOST, server)).unwrap();
        // The query may take a moment to arrive on a busy machine.
        for _ in 0..50 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(10));
            let mut buffer = [0u8; DATAGRAM];
            client.set_nonblocking(true).unwrap();
            if let Ok((length, _)) = client.recv_from(&mut buffer) {
                let reply = decode_reply(&buffer[..length]).expect("the answer did not decode");
                assert_eq!((reply.nonce, reply.port, reply.name.as_str(), reply.motd.as_str()), (99, 27999, "Test", "hi"));
                assert_eq!((reply.people, reply.slots, reply.bots_enabled), (1, 5, true));
                return;
            }
        }
        panic!("the listen server never answered");
    }
}
