//! How the server runs a round: how many slots it has, whether bots take the ones nobody has, how
//! well they play, and how long it lasts. Held and acted on by the authority, and not part of the map, so neither
//! saved with it nor sent to clients. `--server-config` reads them from a [`ServerConfig`] file.

use bevy::prelude::*;
use serde::Deserialize;

/// The most slots the Round tab offers.
pub const MOST_SLOTS: u32 = 32;

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct RoundSettings {
    /// Players in a round, the host and bots included. Caps the clients a server takes, from when
    /// it starts listening.
    pub slots: u32,
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
        if !self.fill_with_bots {
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

    pub fn round(&self) -> RoundSettings {
        RoundSettings {
            slots: self.slots,
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
            "slots = 9\nfill_with_bots = true\nbot_skill = \"hard\"\nbots = 2\ntime_limit = 300\nfrag_limit = 30\nbot_only_matches = true\npre_game_length = 5\npost_game_length = 20\nbetween_rounds_length = 8",
        )
        .unwrap();
        assert_eq!(
            config.round(),
            RoundSettings {
                slots: 9,
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
