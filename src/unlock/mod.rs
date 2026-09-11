use std::fmt::Display;
use lazy_static::lazy_static;
use particle_effecta::ParticleEffectEntry;
use series::CrateSeries;
use series_entry::CrateSeriesEntry;
use stat_tracker::StatTrackerEntry;
use crate::common::item::item::Item;
use crate::get;

mod series;
mod series_entry;
mod particle_effecta;
mod stat_tracker;

pub enum UnlockProblem {
    NoSuchSeries(u32),
    ItemCreationLoop,
    InvalidEntryChoice(i32),
    NoQualities(String),
    NoSuchPrototype(String)
}

impl Display for UnlockProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnlockProblem::NoSuchSeries(series) => {
                write!(f, "{}", get!("crate_drop.error.no_such_series", "series", series))?;
            }
            UnlockProblem::ItemCreationLoop => {
                write!(f, "{}", get!("crate_drop.error.quality_loop"))?;
            }
            UnlockProblem::InvalidEntryChoice(attempted_entry) => {
                write!(f, "{}", get!("crate_drop.error.invalid_entry_choice", "entry", attempted_entry))?;
            }
            UnlockProblem::NoQualities(item_key) => {
                write!(f, "{}", get!("crate_drop.error.no_odds", "item", item_key))?;
            }
            UnlockProblem::NoSuchPrototype(prototype_key) => {
                write!(f, "{}", get!("crate_drop.error.no_such_prototype", "prototype", prototype_key))?;
            }
        }
        Ok(())
    }
}

lazy_static! {
    pub static ref SERIES: Vec<CrateSeries> = init_crate_series();
}

pub fn unlock(series: u32) -> Result<Item, UnlockProblem> {
    if SERIES.len() <= series as usize {
        return Err(UnlockProblem::NoSuchSeries(series));
    }
    let series = &SERIES[series as usize];
    series.unbox_one()
}

fn init_crate_series() -> Vec<CrateSeries> {
    let mut series = Vec::new();

    series.push(CrateSeries::new(
        "Stock Items".to_string(),
        0,
        vec![
            CrateSeriesEntry::new("shotgun", 200),
            CrateSeriesEntry::new("medigun", 100),
            CrateSeriesEntry::new("top_hat", 100),
        ],
    ));

    series.push(CrateSeries::new(
        "".to_string(),
        1,
        vec![
            CrateSeriesEntry::new_with("shotgun", 100,
                                       false,
                                       vec![],
                                       vec![
                                           StatTrackerEntry::none(1000),
                                           StatTrackerEntry::kills(100),
                                           StatTrackerEntry::max_weapon(100),
                                       ],
            ),
            CrateSeriesEntry::new_with("medigun", 100,
                                       false,
                                       vec![],
                                       vec![
                                           StatTrackerEntry::none(1000),
                                           StatTrackerEntry::healing(100),
                                           StatTrackerEntry::max_medigun(100),
                                       ],
            ),
            CrateSeriesEntry::new_with("top_hat", 100,
                                       true,
                                    vec![
                                        // electric, fire, fire-blue
                                        ParticleEffectEntry::none(1000),
                                        ParticleEffectEntry::some(100, "electric".to_owned()),
                                        ParticleEffectEntry::some(100, "fire".to_owned()),
                                        ParticleEffectEntry::some(100, "fire-blue".to_owned()),
                                    ],
                                    vec![
                                        StatTrackerEntry::none(1000),
                                        StatTrackerEntry::points(100),
                                    ],
            ),
        ],
    ));

    series
}
