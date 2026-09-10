use clap::{Arg, ArgAction, Command};

use crate::common::net::{parse_endpoint, NetRole};
use crate::constants::DEFAULT_PORT_TEXT;

pub struct EditorParams {
    pub lang: String,
    pub game_id: String,
    /// Which end of the wire to start as. Defaults to a closed game: opening
    /// the editor to lay out a room must not involve a socket.
    pub net: NetRole,
}

impl EditorParams {
    pub fn new() -> Result<Self, String> {
        Self::from(std::env::args_os())
    }

    /// Split out from [`EditorParams::new`] so the argument grammar can be
    /// tested without a process to hand it to.
    pub fn from<I, T>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let matches = Command::new("Grackle")
            .arg(
                Arg::new("lang")
                    .long("lang")
                    .short('l')
                    .default_value("en-US")
                    .help("Language file to use for editor UI.")
            )
            .arg(
                Arg::new("game_id")
                    .long("game")
                    .short('g')
                    .default_value("none")
                    .help("ID of game to launch with special tools.")
            )
            // A listen server, not a dedicated one: the window still opens and
            // the host is a player. `--serve` on its own takes the default
            // port, because the common case is two people on a LAN who do not
            // care which number it is.
            .arg(
                Arg::new("serve")
                    .long("serve")
                    .num_args(0..=1)
                    .default_missing_value(DEFAULT_PORT_TEXT)
                    .value_name("PORT")
                    .conflicts_with("connect")
                    .help("Host a game on PORT while playing locally.")
            )
            .arg(
                Arg::new("connect")
                    .long("connect")
                    .short('c')
                    .value_name("HOST[:PORT]")
                    .action(ArgAction::Set)
                    .help("Join a game somebody else is hosting.")
            )
            .try_get_matches_from(args)
            // `--help` is not a startup failure: clap knows to put it on
            // stdout and leave with 0, whereas returning it as an error here
            // would print the whole help text as "Editor Startup Error".
            // Everything else comes back as a string so it stays testable.
            .map_err(|error| match error.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion => error.exit(),
                _ => error.to_string(),
            })?;

        let lang = matches.get_one::<String>("lang").ok_or("invalid language")?;
        let game_id = matches.get_one::<String>("game_id").ok_or("invalid game_id")?;

        // `conflicts_with` above means at most one of these is present, so the
        // order they are checked in cannot decide anything.
        let net = match (matches.get_one::<String>("serve"), matches.get_one::<String>("connect")) {
            (Some(port), _) => NetRole::Listen {
                port: port
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| format!("--serve wants a port, not {port:?}"))?,
            },
            (_, Some(endpoint)) => {
                let (host, port) = parse_endpoint(endpoint)?;
                NetRole::Client { host, port }
            }
            _ => NetRole::Solo,
        };

        Ok(Self {
            lang: lang.to_owned(),
            game_id: game_id.to_owned(),
            net,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_PORT;

    fn parse(args: &[&str]) -> Result<EditorParams, String> {
        let mut argv = vec!["grackle"];
        argv.extend_from_slice(args);
        EditorParams::from(argv)
    }

    /// The whole point of the default: double-clicking the binary gives you an
    /// editor and a closed game, with nothing listening.
    #[test]
    fn no_flags_is_a_closed_game() {
        assert_eq!(parse(&[]).unwrap().net, NetRole::Solo);
    }

    #[test]
    fn serve_alone_takes_the_default_port() {
        assert_eq!(parse(&["--serve"]).unwrap().net, NetRole::Listen { port: DEFAULT_PORT });
    }

    #[test]
    fn serve_takes_a_port_when_given_one() {
        assert_eq!(parse(&["--serve", "27016"]).unwrap().net, NetRole::Listen { port: 27016 });
    }

    #[test]
    fn connect_splits_host_from_port() {
        assert_eq!(
            parse(&["--connect", "example.net:27016"]).unwrap().net,
            NetRole::Client { host: "example.net".into(), port: 27016 }
        );
        assert_eq!(
            parse(&["--connect", "example.net"]).unwrap().net,
            NetRole::Client { host: "example.net".into(), port: DEFAULT_PORT }
        );
    }

    /// Being both ends of one wire is not a thing, and silently preferring one
    /// would leave somebody wondering why their `--connect` did nothing.
    #[test]
    fn serving_and_connecting_at_once_is_refused() {
        assert!(parse(&["--serve", "--connect", "example.net"]).is_err());
    }

    #[test]
    fn a_bad_port_is_refused_rather_than_defaulted() {
        assert!(parse(&["--serve", "0"]).is_err());
        assert!(parse(&["--serve", "http"]).is_err());
        assert!(parse(&["--connect", "example.net:0"]).is_err());
    }

    /// The flags that were there before still are.
    #[test]
    fn the_older_flags_survive() {
        let params = parse(&["--lang", "de-DE", "--game", "unlock"]).unwrap();
        assert_eq!(params.lang, "de-DE");
        assert_eq!(params.game_id, "unlock");
    }
}
