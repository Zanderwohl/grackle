use std::fmt::Display;
use strum_macros::EnumIter;
use crate::get;

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, EnumIter)]
pub enum GameMode {
    Arena,
    CTF,
    PL,
    PLR,
    KOTH,
    CP,
    SD,
}

impl Display for GameMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GameMode::Arena => write!(f, "{}", get!("mode.arena")),
            GameMode::CTF => write!(f, "{}", get!("mode.ctf")),
            GameMode::PL => write!(f, "{}", get!("mode.pl")),
            GameMode::PLR => write!(f, "{}", get!("mode.plr")),
            GameMode::KOTH => write!(f, "{}", get!("mode.koth")),
            GameMode::CP => write!(f, "{}", get!("mode.cp")),
            GameMode::SD => write!(f, "{}", get!("mode.sd")),
        }
    }
}

impl TryFrom<&str> for GameMode {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_ref() {
            "arena" => Ok(GameMode::Arena),
            "ctf" => Ok(GameMode::CTF),
            "pl" => Ok(GameMode::PL),
            "plr" => Ok(GameMode::PLR),
            "koth" => Ok(GameMode::KOTH),
            "cp" => Ok(GameMode::CP),
            "sd" => Ok(GameMode::SD),
            _ => Err(())
        }
    }
}

impl GameMode {
    pub fn prefix(&self) -> &'static str {
        match self {
            GameMode::Arena => "arena",
            GameMode::CTF => "ctf",
            GameMode::PL => "pl",
            GameMode::PLR => "plr",
            GameMode::KOTH => "koth",
            GameMode::CP => "cp",
            GameMode::SD => "sd",
        }
    }
}
