use rand::Rng;
use crate::common::item::item;
use crate::unlock::series_entry::CrateSeriesEntry;
use crate::unlock::UnlockProblem;

pub struct CrateSeries {
    pub name: String,
    pub number: u32,
    pub entries: Vec<CrateSeriesEntry>,
    total_odds: i32,
}

impl CrateSeries {
    pub fn new(name: String, number: u32, entries: Vec<CrateSeriesEntry>) -> Self {
        let total_odds = entries.iter().fold(0, |acc, e| acc + e.odds);
        Self {
            name,
            number,
            entries,
            total_odds,
        }
    }
    
    pub fn unbox_one(&self) -> Result<item::Item, UnlockProblem> {
        let random_number = rand::rng().random_range(0..self.total_odds);

        let mut current_odds = 0;
        for entry in &self.entries {
            current_odds += entry.odds;
            if random_number < current_odds {
                return entry.unbox_one()
            }
        }
        
        Err(UnlockProblem::InvalidEntryChoice(random_number))
    }
}
