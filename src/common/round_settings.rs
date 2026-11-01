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
}

impl Default for RoundSettings {
    fn default() -> Self {
        RoundSettings { slots: 8, fill_with_bots: false, bot_skill: BotSkill::Normal, time_limit: None }
    }
}

/// Which part of each temperament range a bot rolls in, for the traits that went with a high
/// score over 72 bot-rounds: how fast the aim settles, how long an ambush is held, and how little
/// it roams and how much it keeps to cover. Every other trait rolls its whole range at any skill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotSkill {
    Easy,
    #[default]
    Normal,
    Hard,
}

impl BotSkill {
    pub const ALL: [BotSkill; 3] = [BotSkill::Easy, BotSkill::Normal, BotSkill::Hard];

    /// The part of `range` this skill rolls in, `better` saying which end plays well: the
    /// bottom, middle or top 40%, overlapping so neighboring tiers are not cleanly apart.
    pub fn within(self, range: std::ops::Range<f32>, better: Better) -> std::ops::Range<f32> {
        let (low, high) = match (self, better) {
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
    /// Seated as `B` seats them, so they count against the slots and stay across rounds.
    pub bots: u32,
    /// Seconds; absent or zero is no limit.
    pub time_limit: Option<f32>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let round = RoundSettings::default();
        ServerConfig { slots: round.slots, fill_with_bots: round.fill_with_bots, bot_skill: round.bot_skill, bots: 0, time_limit: round.time_limit }
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
        Ok(config)
    }

    pub fn round(&self) -> RoundSettings {
        RoundSettings {
            slots: self.slots,
            fill_with_bots: self.fill_with_bots,
            bot_skill: self.bot_skill,
            time_limit: self.time_limit.filter(|limit| *limit > 0.0),
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
        let config = ServerConfig::parse("slots = 9\nfill_with_bots = true\nbot_skill = \"hard\"\nbots = 2\ntime_limit = 300").unwrap();
        assert_eq!(config.round(), RoundSettings { slots: 9, fill_with_bots: true, bot_skill: BotSkill::Hard, time_limit: Some(300.0) });
        assert_eq!(config.bots, 2);
        assert_eq!(ServerConfig::parse("time_limit = 0").unwrap().round().time_limit, None);
    }

    /// A typo that parsed as the default would be a setting that silently did nothing.
    #[test]
    fn a_bad_config_is_refused() {
        assert!(ServerConfig::parse("slot = 9").is_err());
        assert!(ServerConfig::parse("slots = 0").is_err());
        assert!(ServerConfig::parse("slots = 99").is_err());
        assert!(ServerConfig::parse("time_limit = -5").is_err());
        assert!(ServerConfig::parse("bot_skill = \"expert\"").is_err());
    }

    /// Breaks if a harder tier rolls a worse trait, or a tier strays outside the range.
    #[test]
    fn harder_bots_roll_the_better_end() {
        let settle = 0.3..0.9;
        let easy = BotSkill::Easy.within(settle.clone(), Better::Lower);
        let hard = BotSkill::Hard.within(settle.clone(), Better::Lower);
        assert!(hard.end < easy.start);
        assert!(hard.start >= settle.start && easy.end <= settle.end);
        let patience = 4.0..20.0;
        assert!(BotSkill::Hard.within(patience.clone(), Better::Higher).start > BotSkill::Easy.within(patience, Better::Higher).end);
    }
}
