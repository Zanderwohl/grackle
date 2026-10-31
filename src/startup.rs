use clap::{Arg, ArgAction, Command};

use std::path::PathBuf;

use crate::common::net::{parse_endpoint, NetRole};
use crate::common::round_settings::ServerConfig;
use crate::constants::DEFAULT_PORT_TEXT;

pub struct EditorParams {
    pub lang: String,
    pub game_id: String,
    /// Defaults to `Solo`: laying out a room must not involve a socket.
    pub net: NetRole,
    pub start_playing: bool,
    pub start_animating: bool,
    pub start_modeling: bool,
    pub start_texturing: bool,
    /// `--profile N`, 1–9; `None` is the usual player.
    pub profile: Option<u8>,
    /// `--map PATH`: the blueprint opened in place of the new-map template.
    pub map: Option<PathBuf>,
    /// `--server-config PATH`, read and checked here so a bad file stops the process before it
    /// opens a window.
    pub server: Option<ServerConfig>,
    pub start_spectating: bool,
    /// `--rounds N`: quit once N rounds have ended.
    pub rounds: Option<u32>,
}

impl EditorParams {
    pub fn new() -> Result<Self, String> {
        Self::from(std::env::args_os())
    }

    /// Split out from [`EditorParams::new`] so the grammar is testable.
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
            // A listen server: the window still opens and the host plays.
            .arg(
                Arg::new("serve")
                    .long("serve")
                    .num_args(0..=1)
                    .default_missing_value(DEFAULT_PORT_TEXT)
                    .value_name("PORT")
                    .conflicts_with("connect")
                    .help("Host a game on PORT while playing locally.")
            )
            // Lets a host start a round unattended, and is the only way to
            // drive the mode from a script.
            .arg(
                Arg::new("play")
                    .long("play")
                    .action(ArgAction::SetTrue)
                    .help("Start in Play rather than in the editor.")
            )
            .arg(
                Arg::new("prop")
                    .long("prop")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("play")
                    .help("Open the prop editor rather than the map editor.")
            )
            // Separate flags rather than one `--mode=`, so a mode you can start
            // in is stated with that mode rather than in a shared list.
            .arg(
                Arg::new("anim")
                    .long("anim")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("play")
                    .conflicts_with("prop")
                    .help("Open the animation editor rather than the map editor.")
            )
            .arg(
                Arg::new("material")
                    .long("material")
                    .action(ArgAction::SetTrue)
                    .conflicts_with_all(["play", "prop", "anim"])
                    .help("Open the material editor rather than the map editor.")
            )
            .arg(
                Arg::new("connect")
                    .long("connect")
                    .short('c')
                    .value_name("HOST[:PORT]")
                    .action(ArgAction::Set)
                    .help("Join a game somebody else is hosting.")
            )
            .arg(
                Arg::new("profile")
                    .long("profile")
                    .value_name("N")
                    .value_parser(clap::value_parser!(u8).range(1..=9))
                    .help("Play as profile N, 1-9, with its own name and settings.")
            )
            .arg(
                Arg::new("map")
                    .long("map")
                    .short('m')
                    .value_name("PATH")
                    .value_parser(clap::value_parser!(PathBuf))
                    .help("Open this blueprint rather than a new map.")
            )
            .arg(
                Arg::new("server_config")
                    .long("server-config")
                    .value_name("PATH")
                    .value_parser(clap::value_parser!(PathBuf))
                    .conflicts_with("connect")
                    .help("Read the Round tab's settings, and bots to seat, from a TOML file.")
            )
            .arg(
                Arg::new("spectate")
                    .long("spectate")
                    .action(ArgAction::SetTrue)
                    .help("Spectate each round rather than choosing a team.")
            )
            .arg(
                Arg::new("rounds")
                    .long("rounds")
                    .value_name("N")
                    .value_parser(clap::value_parser!(u32).range(1..))
                    .help("Quit once N rounds have ended, each logged with its standings.")
            )
            .try_get_matches_from(args)
            // `--help` exits 0 on stdout rather than printing as a startup
            // error; everything else comes back as a string so it is testable.
            .map_err(|error| match error.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion => error.exit(),
                _ => error.to_string(),
            })?;

        let lang = matches.get_one::<String>("lang").ok_or("invalid language")?;
        let game_id = matches.get_one::<String>("game_id").ok_or("invalid game_id")?;

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

        let server = match matches.get_one::<PathBuf>("server_config") {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|error| format!("--server-config {}: {error}", path.display()))?;
                Some(ServerConfig::parse(&text).map_err(|error| format!("--server-config {}: {error}", path.display()))?)
            }
            None => None,
        };

        Ok(Self {
            lang: lang.to_owned(),
            game_id: game_id.to_owned(),
            net,
            start_playing: matches.get_flag("play"),
            start_modeling: matches.get_flag("prop"),
            start_animating: matches.get_flag("anim"),
            start_texturing: matches.get_flag("material"),
            profile: matches.get_one::<u8>("profile").copied(),
            map: matches.get_one::<PathBuf>("map").cloned(),
            server,
            start_spectating: matches.get_flag("spectate"),
            rounds: matches.get_one::<u32>("rounds").copied(),
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

    #[test]
    fn a_profile_is_one_to_nine_and_absent_by_default() {
        assert_eq!(parse(&[]).unwrap().profile, None);
        assert_eq!(parse(&["--profile", "3"]).unwrap().profile, Some(3));
        assert!(parse(&["--profile", "0"]).is_err());
        assert!(parse(&["--profile", "10"]).is_err());
    }

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

    /// Silently preferring one would make the other flag appear to do nothing.
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

    #[test]
    fn play_is_off_unless_asked_for() {
        assert!(!parse(&[]).unwrap().start_playing);
        assert!(parse(&["--play"]).unwrap().start_playing);
    }

    #[test]
    fn the_prop_editor_is_off_unless_asked_for() {
        assert!(!parse(&[]).unwrap().start_modeling);
        assert!(parse(&["--prop"]).unwrap().start_modeling);
        assert!(!parse(&[]).unwrap().start_animating);
        assert!(parse(&["--anim"]).unwrap().start_animating);
        assert!(!parse(&[]).unwrap().start_texturing);
        assert!(parse(&["--material"]).unwrap().start_texturing);
    }

    /// A process cannot open in two modes; silently picking one makes a flag
    /// appear to do nothing.
    #[test]
    fn playing_and_modeling_at_once_is_refused() {
        assert!(parse(&["--play", "--prop"]).is_err());
        assert!(parse(&["--play", "--anim"]).is_err());
        assert!(parse(&["--prop", "--anim"]).is_err());
        assert!(parse(&["--material", "--prop"]).is_err());
        assert!(parse(&["--material", "--play"]).is_err());
    }

    #[test]
    fn an_unattended_round_is_spelled_out_in_flags() {
        let params = parse(&["--play", "--map", "a.gmb", "--spectate", "--rounds", "2"]).unwrap();
        assert_eq!(params.map, Some(PathBuf::from("a.gmb")));
        assert!(params.start_spectating);
        assert_eq!(params.rounds, Some(2));
        assert!(parse(&["--rounds", "0"]).is_err());
        let params = parse(&[]).unwrap();
        assert!(params.map.is_none() && params.server.is_none() && !params.start_spectating && params.rounds.is_none());
    }

    /// The shipped example has to parse, or the documented way to start a bot round fails.
    #[test]
    fn the_shipped_server_config_is_read() {
        let params = parse(&["--server-config", "assets/default/servers/bot_arena.toml"]).unwrap();
        assert!(params.server.unwrap().fill_with_bots);
        assert!(parse(&["--server-config", "assets/default/servers/no_such_file.toml"]).is_err());
    }

    #[test]
    fn the_older_flags_survive() {
        let params = parse(&["--lang", "de-DE", "--game", "unlock"]).unwrap();
        assert_eq!(params.lang, "de-DE");
        assert_eq!(params.game_id, "unlock");
    }
}
