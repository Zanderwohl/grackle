use bevy::prelude::*;

use crate::constants::DEFAULT_PORT;
use crate::get;

/// Which end of the wire this process is.
///
/// One binary is all three, and the default is the one with no wire at all:
/// opening Grackle and pressing F5 must not need a socket, a port, or a second
/// process. `Solo` is a real role rather than "the server we did not start" —
/// a closed game is the common case for someone laying out a room, and making
/// it a degenerate server would put a loopback connection in the way of the
/// thing this editor exists to make fast.
///
/// `Listen` is `Solo` that also accepts company. The host is a player: same
/// window, same editor, same F5. That is what makes between-round editing
/// testable with somebody else standing in the map.
///
/// **Ask [`NetRole::is_authority`], not which variant this is.** Solo and
/// Listen both simulate and are believed; a client predicts and is corrected.
/// Almost nothing downstream cares about the difference between playing alone
/// and hosting, and a system that matches on the variant is a system that has
/// to be found again the first time a fourth role appears.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum NetRole {
    /// Closed game. No socket is opened and none is expected.
    #[default]
    Solo,
    /// Simulating for others as well as for ourselves, on `port`.
    Listen { port: u16 },
    /// Somebody else is simulating; we predict and are corrected.
    Client { host: String, port: u16 },
}

impl NetRole {
    /// Whether this process decides what actually happened.
    ///
    /// The seam prediction will need: an authority steps the world and its
    /// answer is final, a client steps the same world from the same inputs and
    /// throws the result away whenever the server disagrees. Systems that
    /// resolve anything — damage, death, who picked up what — belong behind
    /// this rather than behind `AppMode::Play` alone.
    pub fn is_authority(&self) -> bool {
        match self {
            NetRole::Solo | NetRole::Listen { .. } => true,
            NetRole::Client { .. } => false,
        }
    }

    /// Whether a socket is meant to be open. False only for `Solo`.
    pub fn is_online(&self) -> bool {
        !matches!(self, NetRole::Solo)
    }

    /// A line for the menu bar and the log. Localised, so it is display text
    /// and not something to parse or compare.
    pub fn describe(&self) -> String {
        match self {
            NetRole::Solo => get!("net.role.solo"),
            NetRole::Listen { port } => get!("net.role.listen", "port", port),
            NetRole::Client { host, port } => {
                get!("net.role.client", "host", host, "port", port)
            }
        }
    }
}

/// Asking to change [`NetRole`], from the command line or the menu.
///
/// A message rather than a direct write so that the one place which knows how
/// to tear a connection down is the only place that changes the role. Today
/// there is nothing to tear down; the moment there is, every caller here
/// already routes through it.
#[derive(Message, Debug, Clone)]
pub enum NetRequest {
    /// Go closed. Also how a client leaves a server.
    GoSolo,
    /// Start accepting connections on `port`, still playing locally.
    Host { port: u16 },
    /// Leave whatever we were doing and join somebody else's game.
    Join { host: String, port: u16 },
}

/// Owns [`NetRole`] and is the only thing that writes it.
///
/// Deliberately not gated on `AppMode`: which end of the wire we are does not
/// stop being true because somebody pressed F5 into the editor, and a listen
/// server that dropped its role on every swap would drop its players with it.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetRole>()
            .add_message::<NetRequest>()
            .add_systems(Startup, announce_role)
            .add_systems(Update, apply_net_requests);
    }
}

/// Say once, at startup, which end of the wire the command line asked for.
///
/// The CLI inserts [`NetRole`] directly rather than going through a request —
/// there is nothing to tear down before the app has run — so without this a
/// mistyped `--serve` would start a listen server that says nothing at all.
fn announce_role(role: Res<NetRole>) {
    info!("{}", role.describe());
}

/// The only writer of [`NetRole`].
///
/// Deliberately knows nothing about sockets: `NetTransportPlugin` watches
/// `Changed<NetRole>` and is the thing that binds, dials and tears down. That
/// split is what lets the role be decided by a menu click, a command-line flag
/// or a test without any of them having to know what a link entity is.
fn apply_net_requests(mut requests: MessageReader<NetRequest>, mut role: ResMut<NetRole>) {
    for request in requests.read() {
        let wanted = match request {
            NetRequest::GoSolo => NetRole::Solo,
            NetRequest::Host { port } => NetRole::Listen { port: *port },
            NetRequest::Join { host, port } => NetRole::Client {
                host: host.clone(),
                port: *port,
            },
        };

        // Bevy's change detection fires on any write, not on a differing one,
        // and the transport will be watching for changes. Re-hosting the port
        // we are already hosting must not read as a reason to rebind it.
        if *role == wanted {
            continue;
        }

        info!("Network role: {:?} -> {:?}", *role, wanted);
        *role = wanted;
    }
}

/// Run condition: this process decides what actually happened.
pub fn has_authority(role: Res<NetRole>) -> bool {
    role.is_authority()
}

/// Run condition: somebody else does, and we are predicting.
pub fn is_remote_client(role: Res<NetRole>) -> bool {
    !role.is_authority()
}

/// Split `host`, `host:port` or `[::1]:port` into its two halves.
///
/// Hand-rolled rather than going through `ToSocketAddrs` because that resolves
/// DNS — a blocking call, on the main thread, for a string somebody is still
/// halfway through typing. Resolution is the transport's problem; this is a
/// syntax check on what the user typed.
pub fn parse_endpoint(text: &str) -> Result<(String, u16), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(get!("net.error.empty"));
    }

    // Bracketed IPv6 literal: the colons inside are part of the address.
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| get!("net.error.unclosed_bracket"))?;
        return match tail {
            "" => Ok((host.to_owned(), DEFAULT_PORT)),
            _ => {
                let port = tail
                    .strip_prefix(':')
                    .ok_or_else(|| get!("net.error.malformed"))?;
                Ok((host.to_owned(), parse_port(port)?))
            }
        };
    }

    match text.rsplit_once(':') {
        // More than one colon and no brackets is a bare IPv6 literal, which
        // has no room left for a port. Take the whole thing as the host.
        Some(_) if text.matches(':').count() > 1 => Ok((text.to_owned(), DEFAULT_PORT)),
        Some((host, port)) if !host.is_empty() => Ok((host.to_owned(), parse_port(port)?)),
        Some(_) => Err(get!("net.error.no_host")),
        None => Ok((text.to_owned(), DEFAULT_PORT)),
    }
}

fn parse_port(text: &str) -> Result<u16, String> {
    match text.parse::<u16>() {
        // Port 0 means "any free port" to the OS, which is never what somebody
        // typing a port into a box meant.
        Ok(0) | Err(_) => Err(get!("net.error.port", "port", text)),
        Ok(port) => Ok(port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_process_is_closed() {
        assert_eq!(NetRole::default(), NetRole::Solo);
        assert!(!NetRole::default().is_online());
    }

    /// Hosting is playing plus company, so the host is still believed. Getting
    /// this backwards would make a listen server ask itself for permission.
    #[test]
    fn only_a_client_gives_up_authority() {
        assert!(NetRole::Solo.is_authority());
        assert!(NetRole::Listen { port: 27015 }.is_authority());
        assert!(!NetRole::Client { host: "h".into(), port: 27015 }.is_authority());
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(NetPlugin);
        app
    }

    fn request(app: &mut App, request: NetRequest) {
        app.world_mut().write_message(request);
        app.update();
    }

    #[test]
    fn a_request_is_what_moves_the_role() {
        let mut app = app();
        assert_eq!(*app.world().resource::<NetRole>(), NetRole::Solo);

        request(&mut app, NetRequest::Host { port: 27015 });
        assert_eq!(*app.world().resource::<NetRole>(), NetRole::Listen { port: 27015 });

        request(&mut app, NetRequest::Join { host: "example".into(), port: 40 });
        assert_eq!(
            *app.world().resource::<NetRole>(),
            NetRole::Client { host: "example".into(), port: 40 }
        );

        request(&mut app, NetRequest::GoSolo);
        assert_eq!(*app.world().resource::<NetRole>(), NetRole::Solo);
    }

    /// The transport will rebind on `Changed<NetRole>`, so asking for the role
    /// we already have must not look like a change. Otherwise re-picking
    /// "Host" from the menu drops everybody who is already connected.
    #[test]
    fn asking_for_the_role_we_already_have_changes_nothing() {
        let mut app = app();
        request(&mut app, NetRequest::Host { port: 27015 });

        // Clear the change tick this update left behind.
        app.update();
        assert!(!app.world().resource_ref::<NetRole>().is_changed());

        request(&mut app, NetRequest::Host { port: 27015 });
        assert!(
            !app.world().resource_ref::<NetRole>().is_changed(),
            "re-hosting the port we are already hosting read as a change"
        );
    }

    #[test]
    fn a_bare_host_gets_the_default_port() {
        assert_eq!(parse_endpoint("localhost"), Ok(("localhost".into(), DEFAULT_PORT)));
        assert_eq!(parse_endpoint("  10.0.0.4 "), Ok(("10.0.0.4".into(), DEFAULT_PORT)));
    }

    #[test]
    fn a_port_is_taken_from_the_end() {
        assert_eq!(parse_endpoint("localhost:27016"), Ok(("localhost".into(), 27016)));
    }

    /// An IPv6 literal is mostly colons, so the last one is not a port
    /// separator unless brackets said it was.
    #[test]
    fn ipv6_needs_brackets_to_carry_a_port() {
        assert_eq!(parse_endpoint("::1"), Ok(("::1".into(), DEFAULT_PORT)));
        assert_eq!(parse_endpoint("[::1]"), Ok(("::1".into(), DEFAULT_PORT)));
        assert_eq!(parse_endpoint("[::1]:27016"), Ok(("::1".into(), 27016)));
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        assert!(parse_endpoint("").is_err());
        assert!(parse_endpoint("   ").is_err());
        assert!(parse_endpoint("localhost:0").is_err());
        assert!(parse_endpoint("localhost:99999").is_err());
        assert!(parse_endpoint("localhost:http").is_err());
        assert!(parse_endpoint(":27015").is_err());
        assert!(parse_endpoint("[::1").is_err());
    }
}
