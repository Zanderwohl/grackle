//! Names for anybody who has not given one: every pack's `bot_names.txt`, one
//! name a line, appended in pack order and dealt from a shuffled index.

use std::path::PathBuf;

use bevy::prelude::*;
use rand::seq::SliceRandom;

use crate::common::assets::{AssetError, Assets};
use crate::common::damage::PlayerId;

pub const FILE: &str = "bot_names.txt";

#[derive(Resource, Default, Debug)]
pub struct BotNames {
    names: Vec<String>,
    /// A permutation of `0..names.len()`.
    order: Vec<usize>,
}

impl BotNames {
    /// Blank lines are skipped, and a name a later pack repeats is kept once.
    pub fn from_files<'a>(files: impl IntoIterator<Item = &'a str>, rng: &mut impl rand::Rng) -> Self {
        let mut names: Vec<String> = Vec::new();
        for file in files {
            for line in file.lines() {
                let name = line.trim();
                if !name.is_empty() && !names.iter().any(|known| known == name) {
                    names.push(name.to_owned());
                }
            }
        }
        let mut order: Vec<usize> = (0..names.len()).collect();
        order.shuffle(rng);
        Self { names, order }
    }

    /// `held` if it is a pool name not in `taken`, else the first name not in `taken` from
    /// `id`'s place in the shuffle. A name repeats only when every one is taken.
    pub fn deal(&self, id: PlayerId, held: Option<&str>, taken: &[&str]) -> Option<&str> {
        let free = |name: &&str| !taken.contains(name);
        if let Some(kept) = held.and_then(|held| self.names.iter().map(String::as_str).find(|name| *name == held)).filter(free) {
            return Some(kept);
        }
        let len = self.order.len();
        let start = id.0.checked_rem(len as u64)? as usize;
        let dealt = (0..len).map(|step| self.names[self.order[(start + step) % len]].as_str());
        dealt.clone().find(free).or_else(|| dealt.into_iter().next())
    }
}

pub fn is_blank(name: &str) -> bool {
    name.trim().is_empty()
}

pub fn load_bot_names(mut commands: Commands, assets: Option<Res<Assets>>) {
    let Some(assets) = assets else { return };
    let packs: Vec<PathBuf> = crate::common::assets::default_packs();
    let files: Vec<String> = packs
        .iter()
        .filter_map(|pack| match assets.read(pack, FILE) {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(AssetError::NotFound(_)) => None,
            Err(e) => {
                warn!("{e}; skipping it");
                None
            }
        })
        .collect();
    let names = BotNames::from_files(files.iter().map(String::as_str), &mut rand::rng());
    info!("{} bot names from {} file(s)", names.names.len(), files.len());
    commands.insert_resource(names);
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;

    #[test]
    fn packs_append_skipping_blanks_and_repeats() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let names = BotNames::from_files(["Salad\n\nWhale\r\n", "Whale\n  A Dollar  \n"], &mut rng);
        let mut dealt: Vec<&str> = (0..3).map(|id| names.deal(PlayerId(id), None, &[]).unwrap()).collect();
        dealt.sort();
        assert_eq!(dealt, vec!["A Dollar", "Salad", "Whale"]);
        assert_eq!(names.deal(PlayerId(3), None, &[]), names.deal(PlayerId(0), None, &[]), "wraps with nothing taken");
    }

    #[test]
    fn a_name_somebody_has_is_not_dealt_again() {
        let names = BotNames::from_files(["Salad\nWhale\nA Dollar"], &mut rand::rng());
        let mut taken = vec![names.deal(PlayerId(0), None, &[]).unwrap()];
        taken.push(names.deal(PlayerId(3), None, &taken).unwrap());
        taken.push(names.deal(PlayerId(6), None, &taken).unwrap());
        let mut sorted = taken.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["A Dollar", "Salad", "Whale"], "ids a list's length apart still differ");
        assert!(names.deal(PlayerId(9), None, &taken).is_some(), "repeats once all are taken");
    }

    #[test]
    fn a_dealt_name_is_kept_until_somebody_else_claims_it() {
        let names = BotNames::from_files(["Salad\nWhale"], &mut rand::rng());
        assert_eq!(names.deal(PlayerId(0), Some("Whale"), &[]), Some("Whale"));
        assert_eq!(names.deal(PlayerId(0), Some("Whale"), &["Whale"]), Some("Salad"));
        assert_ne!(names.deal(PlayerId(0), Some("Typed"), &[]), Some("Typed"), "only a pool name is kept");
    }

    #[test]
    fn no_names_deals_nothing() {
        let names = BotNames::from_files([], &mut rand::rng());
        assert_eq!(names.deal(PlayerId(0), None, &[]), None);
    }

    #[test]
    fn the_default_pack_has_names() {
        let bytes = Assets::default().read(std::path::Path::new("assets/default"), FILE).unwrap();
        let names = BotNames::from_files([String::from_utf8(bytes).unwrap().as_str()], &mut rand::rng());
        assert!(names.deal(PlayerId(0), None, &[]).is_some());
    }
}
