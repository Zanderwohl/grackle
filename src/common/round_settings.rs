//! How the server runs a round: how many slots it has, whether bots take the ones nobody has, how
//! well they play, and how long it lasts. Held and acted on by the authority, and not part of the map, so neither
//! saved with it nor sent to clients. They are kept in [`USER_SERVER_CONFIG`], which the Round tab
//! writes back to, and `--server-config` reads them from another [`ServerConfig`] file for a run.

use std::path::Path;

use bevy::prelude::*;
use serde::Deserialize;

/// The Round tab's settings between runs, read at startup unless `--server-config` names another.
pub const USER_SERVER_CONFIG: &str = "assets/user/server.toml";

/// How long the Round tab's settings stay unchanged before they are written, so a drag on a
/// number is one write.
const WRITE_AFTER_SECONDS: f32 = 0.5;

/// The most slots the Round tab offers.
pub const MOST_SLOTS: u32 = 32;

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct RoundSettings {
    /// Players in a round, the host and bots included. Caps the clients a server takes, from when
    /// it starts listening.
    pub slots: u32,
    /// Off, no bot is seated, whatever the other bot settings say, and a round starts without
    /// the nav graph or its sightlines, which only bots read.
    pub bots_enabled: bool,
    pub fill_with_bots: bool,
    pub bot_skill: BotSkill,
    /// Real seconds before the round ends, as the Rounds tab times it; `None` runs it until the
    /// mode ends it.
    pub time_limit: Option<f32>,
    /// Frags that end the round, in every mode for now; `None` is no limit.
    pub frag_limit: Option<u32>,
    /// Matches may run with no person playing. See documentation/match-cycle.md.
    pub bot_only_matches: bool,
    /// Seconds of countdown before a match goes live.
    pub pre_game_length: f32,
    /// The most seconds of humiliation after a match; it ends sooner once every loser is dead.
    pub post_game_length: f32,
    /// Seconds of the scoreboard between a post-game and the next pre-game.
    pub between_rounds_length: f32,
}

impl Default for RoundSettings {
    fn default() -> Self {
        RoundSettings {
            slots: 8,
            bots_enabled: true,
            fill_with_bots: false,
            bot_skill: BotSkill::default(),
            time_limit: None,
            frag_limit: Some(20),
            bot_only_matches: false,
            pre_game_length: 10.0,
            post_game_length: 15.0,
            between_rounds_length: 15.0,
        }
    }
}

/// Which part of each skill trait's range a bot rolls in: the aim, reaction, airblast attempt,
/// standoff comfort, drift and health drive, which went with a high score over 295 `Varied` rounds
/// (documentation/bot-brain.md). `Varied` rolls every range whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotSkill {
    Easy,
    #[default]
    Normal,
    Hard,
    Varied,
}

impl BotSkill {
    pub const ALL: [BotSkill; 4] = [BotSkill::Easy, BotSkill::Normal, BotSkill::Hard, BotSkill::Varied];

    pub fn name(self) -> String {
        match self {
            BotSkill::Easy => crate::get!("editor.round.skill.easy"),
            BotSkill::Normal => crate::get!("editor.round.skill.normal"),
            BotSkill::Hard => crate::get!("editor.round.skill.hard"),
            BotSkill::Varied => crate::get!("editor.round.skill.varied"),
        }
    }

    /// The part of `range` this skill rolls in, `better` saying which end plays well: the
    /// bottom, middle or top 40%, overlapping so neighboring tiers are not cleanly apart, or all
    /// of it.
    pub fn within(self, range: std::ops::Range<f32>, better: Better) -> std::ops::Range<f32> {
        let (low, high) = match (self, better) {
            (BotSkill::Varied, _) => (0.0, 1.0),
            (BotSkill::Normal, _) => (0.3, 0.7),
            (BotSkill::Easy, Better::Higher) | (BotSkill::Hard, Better::Lower) => (0.0, 0.4),
            (BotSkill::Easy, Better::Lower) | (BotSkill::Hard, Better::Higher) => (0.6, 1.0),
        };
        let at = |share: f32| range.start + (range.end - range.start) * share;
        at(low)..at(high)
    }
}

/// Which end of a trait's range plays well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Better {
    Lower,
    Higher,
}

impl RoundSettings {
    /// Whether a round needs what bots read (the nav graph and its sightlines). True without
    /// settings, as in a binary or test that has none.
    pub fn wants_bots(settings: Option<&RoundSettings>) -> bool {
        settings.is_none_or(|settings| settings.bots_enabled)
    }

    /// Clients the server takes, the host holding one slot. Bots are not counted: a slot a bot
    /// fills is open to anybody who connects.
    pub fn most_clients(&self) -> usize {
        self.slots.saturating_sub(1) as usize
    }

    /// Slots open to somebody connecting, with `people` playing. Bots filling slots leave them open.
    pub fn open_slots(&self, people: usize) -> usize {
        (self.slots as usize).saturating_sub(people)
    }

    /// Bots wanted to fill the slots, with `people` playing and `seated_by_hand` bots already in.
    pub fn bots_to_fill(&self, people: usize, seated_by_hand: usize) -> usize {
        if !self.bots_enabled || !self.fill_with_bots {
            return 0;
        }
        (self.slots as usize).saturating_sub(people + seated_by_hand)
    }
}

/// A server's settings as a file, for `--server-config`: everything the Round tab holds, and bots
/// to seat by hand at startup.
#[derive(Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    pub slots: u32,
    pub bots_enabled: bool,
    pub fill_with_bots: bool,
    pub bot_skill: BotSkill,
    /// Seated by hand, so they count against the slots and stay across rounds.
    pub bots: u32,
    /// Seconds; absent or zero is no limit.
    pub time_limit: Option<f32>,
    /// Zero is no limit.
    pub frag_limit: u32,
    pub bot_only_matches: bool,
    /// Seconds.
    pub pre_game_length: f32,
    /// Seconds.
    pub post_game_length: f32,
    /// Seconds.
    pub between_rounds_length: f32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let round = RoundSettings::default();
        ServerConfig {
            slots: round.slots,
            bots_enabled: round.bots_enabled,
            fill_with_bots: round.fill_with_bots,
            bot_skill: round.bot_skill,
            bots: 0,
            time_limit: round.time_limit,
            frag_limit: round.frag_limit.unwrap_or(0),
            bot_only_matches: round.bot_only_matches,
            pre_game_length: round.pre_game_length,
            post_game_length: round.post_game_length,
            between_rounds_length: round.between_rounds_length,
        }
    }
}

impl ServerConfig {
    /// Refuses rather than clamps: a config asked for by name that silently meant something else
    /// is a server nobody can explain.
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: ServerConfig = toml::from_str(text).map_err(|error| error.to_string())?;
        if !(1..=MOST_SLOTS).contains(&config.slots) {
            return Err(format!("slots must be 1 to {MOST_SLOTS}, not {}", config.slots));
        }
        if config.time_limit.is_some_and(|limit| !(limit >= 0.0)) {
            return Err("time_limit must be a number of seconds, zero or more".to_owned());
        }
        for (key, length) in [
            ("pre_game_length", config.pre_game_length),
            ("post_game_length", config.post_game_length),
            ("between_rounds_length", config.between_rounds_length),
        ] {
            if !(length >= 0.0) {
                return Err(format!("{key} must be a number of seconds, zero or more"));
            }
        }
        Ok(config)
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// The config named on the command line, else the user's, else the defaults, each falling
    /// back past one that will not read. Returns why any was passed over; a missing user file is
    /// not a problem, since nothing has been saved yet.
    pub fn resolve(named: Option<&Path>, user: &Path) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        if let Some(named) = named {
            match Self::read(named) {
                Ok(config) => return (config, problems),
                Err(error) => problems.push(format!("--server-config {error}; reading {} instead", user.display())),
            }
        }
        if user.exists() {
            match Self::read(user) {
                Ok(config) => return (config, problems),
                Err(error) => problems.push(format!("{error}; using the default round settings")),
            }
        }
        (Self::default(), problems)
    }

    pub fn round(&self) -> RoundSettings {
        RoundSettings {
            slots: self.slots,
            bots_enabled: self.bots_enabled,
            fill_with_bots: self.fill_with_bots,
            bot_skill: self.bot_skill,
            time_limit: self.time_limit.filter(|limit| *limit > 0.0),
            frag_limit: Some(self.frag_limit).filter(|limit| *limit > 0),
            bot_only_matches: self.bot_only_matches,
            pre_game_length: self.pre_game_length,
            post_game_length: self.post_game_length,
            between_rounds_length: self.between_rounds_length,
        }
    }
}

/// `text`, a server config, with every key the Round tab holds set to `settings`. Edited in place,
/// so comments and keys it does not hold (`bots`) survive; a file that is not TOML is replaced.
pub fn written_into(text: &str, settings: &RoundSettings) -> String {
    let mut doc: toml_edit::DocumentMut = text.parse().unwrap_or_default();
    // Through the shortest decimal, so 0.1 is written as 0.1 and not as 0.10000000149011612.
    let seconds = |value: f32| toml_edit::value(value.to_string().parse::<f64>().unwrap_or(f64::from(value)));
    let skill = match settings.bot_skill {
        BotSkill::Easy => "easy",
        BotSkill::Normal => "normal",
        BotSkill::Hard => "hard",
        BotSkill::Varied => "varied",
    };
    doc["slots"] = toml_edit::value(i64::from(settings.slots));
    doc["bots_enabled"] = toml_edit::value(settings.bots_enabled);
    doc["fill_with_bots"] = toml_edit::value(settings.fill_with_bots);
    doc["bot_skill"] = toml_edit::value(skill);
    match settings.time_limit {
        Some(limit) => doc["time_limit"] = seconds(limit),
        None => {
            doc.remove("time_limit");
        }
    }
    doc["frag_limit"] = toml_edit::value(i64::from(settings.frag_limit.unwrap_or(0)));
    doc["bot_only_matches"] = toml_edit::value(settings.bot_only_matches);
    doc["pre_game_length"] = seconds(settings.pre_game_length);
    doc["post_game_length"] = seconds(settings.post_game_length);
    doc["between_rounds_length"] = seconds(settings.between_rounds_length);
    doc.to_string()
}

/// Writes the Round tab's settings to a file once they have been left alone for a moment. Added
/// by the editor binary only, so tests write nothing.
pub struct RoundSettingsFilePlugin {
    pub path: std::path::PathBuf,
}

impl Plugin for RoundSettingsFilePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RoundSettingsFile(self.path.clone())).add_systems(Update, write_round_settings);
    }
}

#[derive(Resource)]
struct RoundSettingsFile(std::path::PathBuf);

#[derive(Default)]
struct Written {
    /// What the file holds, as far as this run knows; the settings it started with at first.
    written: Option<RoundSettings>,
    /// The settings last seen, and since when. Compared by value: the Round tab borrows them
    /// mutably every frame it is drawn, so change detection fires whether or not anything changed.
    seen: Option<(RoundSettings, f32)>,
}

fn write_round_settings(settings: Res<RoundSettings>, file: Res<RoundSettingsFile>, time: Res<Time<Real>>, mut state: Local<Written>) {
    let now = time.elapsed_secs();
    let current = *settings;
    let written = *state.written.get_or_insert(current);
    if state.seen.is_none_or(|(seen, _)| seen != current) {
        state.seen = Some((current, now));
    }
    let Some((_, since)) = state.seen else { return };
    if current == written || now - since < WRITE_AFTER_SECONDS {
        return;
    }
    let text = std::fs::read_to_string(&file.0).unwrap_or_default();
    match std::fs::write(&file.0, written_into(&text, &current)) {
        Ok(()) => info!("Saved the round settings to {}", file.0.display()),
        Err(error) => warn!("Could not save the round settings to {}: {error}", file.0.display()),
    }
    // Not tried again until they change, whether or not the write went through.
    state.written = Some(current);
}

/// Bot seats [`ServerConfig::bots`] asks for, stood up once at startup.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct BotsAtStartup(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_is_the_round_tabs_defaults() {
        let config = ServerConfig::parse("").unwrap();
        assert_eq!(config.round(), RoundSettings::default());
        assert_eq!(config.bots, 0);
    }

    #[test]
    fn a_config_sets_every_round_setting() {
        let config = ServerConfig::parse(
            "slots = 9\nbots_enabled = false\nfill_with_bots = true\nbot_skill = \"hard\"\nbots = 2\ntime_limit = 300\nfrag_limit = 30\nbot_only_matches = true\npre_game_length = 5\npost_game_length = 20\nbetween_rounds_length = 8",
        )
        .unwrap();
        assert_eq!(
            config.round(),
            RoundSettings {
                slots: 9,
                bots_enabled: false,
                fill_with_bots: true,
                bot_skill: BotSkill::Hard,
                time_limit: Some(300.0),
                frag_limit: Some(30),
                bot_only_matches: true,
                pre_game_length: 5.0,
                post_game_length: 20.0,
                between_rounds_length: 8.0,
            }
        );
        assert_eq!(config.bots, 2);
        assert_eq!(ServerConfig::parse("time_limit = 0").unwrap().round().time_limit, None);
        assert_eq!(ServerConfig::parse("frag_limit = 0").unwrap().round().frag_limit, None);
    }

    /// A typo that parsed as the default would be a setting that silently did nothing.
    #[test]
    fn a_bad_config_is_refused() {
        assert!(ServerConfig::parse("slot = 9").is_err());
        assert!(ServerConfig::parse("slots = 0").is_err());
        assert!(ServerConfig::parse("slots = 99").is_err());
        assert!(ServerConfig::parse("time_limit = -5").is_err());
        assert!(ServerConfig::parse("pre_game_length = -1").is_err());
        assert!(ServerConfig::parse("bot_skill = \"expert\"").is_err());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("grackle-round-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Breaks if the order is not named, then the user's, then the defaults, or a bad file stops
    /// the fall rather than being passed over.
    #[test]
    fn a_config_that_will_not_read_falls_back_to_the_next() {
        let named = scratch("named.toml");
        let user = scratch("user.toml");
        let missing = scratch("missing.toml");
        std::fs::write(&named, "slots = 3").unwrap();
        std::fs::write(&user, "slots = 5").unwrap();

        let (config, problems) = ServerConfig::resolve(Some(&named), &user);
        assert_eq!((config.slots, problems.len()), (3, 0), "the named file was not the one used");
        let (config, problems) = ServerConfig::resolve(None, &user);
        assert_eq!((config.slots, problems.len()), (5, 0), "the user's file was not read by default");
        let (config, problems) = ServerConfig::resolve(None, &missing);
        assert_eq!((config, problems.len()), (ServerConfig::default(), 0), "a missing user file was a problem");

        std::fs::write(&named, "slots = 0").unwrap();
        let (config, problems) = ServerConfig::resolve(Some(&named), &user);
        assert_eq!((config.slots, problems.len()), (5, 1), "a bad named file did not fall back to the user's");
        let (config, problems) = ServerConfig::resolve(Some(&missing), &user);
        assert_eq!((config.slots, problems.len()), (5, 1), "a missing named file did not fall back to the user's");

        std::fs::write(&user, "slots = \"many\"").unwrap();
        let (config, problems) = ServerConfig::resolve(Some(&named), &user);
        assert_eq!((config, problems.len()), (ServerConfig::default(), 2), "two bad files did not fall back to the defaults");
        let (config, problems) = ServerConfig::resolve(None, &user);
        assert_eq!((config, problems.len()), (ServerConfig::default(), 1));
    }

    /// Breaks if what the Round tab writes does not read back as the same settings, or the
    /// write loses a comment or a key the tab does not hold.
    #[test]
    fn the_round_settings_written_read_back_and_keep_the_rest_of_the_file() {
        let settings = RoundSettings {
            slots: 6,
            bots_enabled: false,
            fill_with_bots: true,
            bot_skill: BotSkill::Varied,
            time_limit: Some(90.5),
            frag_limit: None,
            bot_only_matches: true,
            pre_game_length: 0.1,
            post_game_length: 4.0,
            between_rounds_length: 0.0,
        };
        let text = written_into("# mine\nbots = 2\nslots = 9\n", &settings);
        assert!(text.contains("# mine") && text.contains("pre_game_length = 0.1\n"), "{text}");
        let read = ServerConfig::parse(&text).unwrap();
        assert_eq!((read.round(), read.bots), (settings, 2));
        let unlimited = written_into(&text, &RoundSettings { time_limit: None, ..settings });
        assert_eq!(ServerConfig::parse(&unlimited).unwrap().round().time_limit, None);
        assert_eq!(ServerConfig::parse(&written_into("not = [toml", &settings)).unwrap().round(), settings);
    }

    /// Breaks if a change is written before it settles, an unchanged run writes at all, or a
    /// settled change is never written.
    #[test]
    fn the_round_tab_writes_once_its_settings_settle() {
        use bevy::time::TimeUpdateStrategy;
        let path = scratch("written.toml");
        let _ = std::fs::remove_file(&path);
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, RoundSettingsFilePlugin { path: path.clone() }));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f32(0.2)));
        app.init_resource::<RoundSettings>();
        for _ in 0..10 {
            app.update();
        }
        assert!(!path.exists(), "settings nobody changed were written");

        for slots in 2..6 {
            app.world_mut().resource_mut::<RoundSettings>().slots = slots;
            app.update();
            assert!(!path.exists(), "written mid-drag");
        }
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(ServerConfig::read(&path).unwrap().slots, 5);
    }

    /// Breaks if a harder tier rolls a worse trait, Normal is not between the other two, or a
    /// tier strays outside the range.
    #[test]
    fn harder_bots_roll_the_better_end() {
        let range = 0.3..0.9;
        let mid = |skill: BotSkill, better| {
            let part = skill.within(range.clone(), better);
            assert!(part.start >= range.start && part.end <= range.end && part.start < part.end, "{skill:?} strays from the range");
            (part.start + part.end) / 2.0
        };
        let tiers = [BotSkill::Easy, BotSkill::Normal, BotSkill::Hard];
        let [easy, normal, hard] = tiers.map(|skill| mid(skill, Better::Lower));
        assert!(hard < normal && normal < easy, "lower is better: {hard} {normal} {easy}");
        let [easy, normal, hard] = tiers.map(|skill| mid(skill, Better::Higher));
        assert!(easy < normal && normal < hard, "higher is better: {easy} {normal} {hard}");
        assert_eq!(BotSkill::Varied.within(range.clone(), Better::Lower), range);
    }
}
