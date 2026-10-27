use clap::{Arg, ArgAction, Command};

use crate::common::net::{parse_endpoint, NetRole};
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

        Ok(Self {
            lang: lang.to_owned(),
            game_id: game_id.to_owned(),
            net,
            start_playing: matches.get_flag("play"),
            start_modeling: matches.get_flag("prop"),
            start_animating: matches.get_flag("anim"),
            start_texturing: matches.get_flag("material"),
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
    fn the_older_flags_survive() {
        let params = parse(&["--lang", "de-DE", "--game", "unlock"]).unwrap();
        assert_eq!(params.lang, "de-DE");
        assert_eq!(params.game_id, "unlock");
    }
}
