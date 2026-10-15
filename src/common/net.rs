use bevy::prelude::*;

use crate::constants::DEFAULT_PORT;
use crate::get;

/// Which end of the wire this process is.
///
/// `Solo` is a real role with no socket, not a degenerate server: laying out a
/// room must not need a loopback connection. `Listen` is `Solo` that also
/// accepts connections; the host is still a player.
///
/// Ask [`NetRole::is_authority`] rather than matching the variant, so a fourth
/// role does not have to be found again in every match.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum NetRole {
    #[default]
    Solo,
    Listen { port: u16 },
    Client { host: String, port: u16 },
}

impl NetRole {
    /// Whether this process decides what happened. Anything that resolves damage,
    /// death or pickups belongs behind this, not behind `AppMode::Play` alone.
    pub fn is_authority(&self) -> bool {
        match self {
            NetRole::Solo | NetRole::Listen { .. } => true,
            NetRole::Client { .. } => false,
        }
    }

    pub fn is_online(&self) -> bool {
        !matches!(self, NetRole::Solo)
    }

    /// Localised display text; not for parsing or comparing.
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

/// A request to change [`NetRole`]. A message so that [`apply_net_requests`] is
/// the only writer of the role.
#[derive(Message, Debug, Clone)]
pub enum NetRequest {
    /// Also how a client leaves a server.
    GoSolo,
    Host { port: u16 },
    Join { host: String, port: u16 },
}

/// Not gated on `AppMode`: a listen server that dropped its role on F5 would
/// drop its players with it.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetRole>()
            .add_message::<NetRequest>()
            .add_systems(Startup, announce_role)
            .add_systems(Update, apply_net_requests);
    }
}

/// The CLI inserts [`NetRole`] directly, so without this a mistyped `--serve`
/// would host silently.
fn announce_role(role: Res<NetRole>) {
    info!("{}", role.describe());
}

/// The only writer of [`NetRole`]. Knows nothing about sockets;
/// `NetTransportPlugin` reacts to `Changed<NetRole>`.
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

        // Change detection fires on any write; re-hosting the same port must
        // not read as a reason to rebind.
        if *role == wanted {
            continue;
        }

        info!("Network role: {:?} -> {:?}", *role, wanted);
        *role = wanted;
    }
}

/// Run condition. `Option` so a build or test without `NetPlugin` counts as a
/// closed game, which is its own authority.
pub fn has_authority(role: Option<Res<NetRole>>) -> bool {
    role.is_none_or(|role| role.is_authority())
}

pub fn is_remote_client(role: Option<Res<NetRole>>) -> bool {
    !has_authority(role)
}

/// Run condition: there are clients to send things to. A run condition, not an
/// early return, so `Added` filters still see what was added while solo.
pub fn is_hosting(role: Option<Res<NetRole>>) -> bool {
    role.is_some_and(|role| matches!(*role, NetRole::Listen { .. }))
}

/// Split `host`, `host:port` or `[::1]:port`. Hand-rolled because
/// `ToSocketAddrs` resolves DNS, blocking, on a string still being typed.
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
        // Several colons without brackets is a bare IPv6 literal with no port.
        Some(_) if text.matches(':').count() > 1 => Ok((text.to_owned(), DEFAULT_PORT)),
        Some((host, port)) if !host.is_empty() => Ok((host.to_owned(), parse_port(port)?)),
        Some(_) => Err(get!("net.error.no_host")),
        None => Ok((text.to_owned(), DEFAULT_PORT)),
    }
}

fn parse_port(text: &str) -> Result<u16, String> {
    match text.parse::<u16>() {
        // Port 0 means "any free port" to the OS, never what a user typed it for.
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

    /// A listen server that gave up authority would ask itself for permission.
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

    /// The transport rebinds on `Changed<NetRole>`, so re-picking "Host" would
    /// drop everybody connected.
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
