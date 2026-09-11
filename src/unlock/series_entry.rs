use std::sync::Arc;
use bevy::log::{info, warn};
use rand::Rng;
use crate::common::item::{PARTICLES, PROTOTYPES};
use crate::common::item::item::{Item, ParticleEffect, StatTracker};
use crate::get;
use crate::unlock::stat_tracker::StatTrackerEntry;
use crate::unlock::particle_effecta::ParticleEffectEntry;
use crate::unlock::UnlockProblem;
use crate::unlock::UnlockProblem::{ItemCreationLoop, NoQualities, NoSuchPrototype};

pub struct CrateSeriesEntry {
    pub prototype_key: String,
    pub particle_effects: Vec<ParticleEffectEntry>,
    pub total_particle_effect_odds: i32,
    pub stat_trackers: Vec<StatTrackerEntry>,
    pub total_stat_tracker_odds: i32,
    pub odds: i32,
    pub allow_plain: bool,
}

impl CrateSeriesEntry {
    pub fn new(prototype_key: &str, odds: i32) -> CrateSeriesEntry {
        Self {
            prototype_key: prototype_key.to_owned(),
            particle_effects: Vec::new(),
            stat_trackers: Vec::new(),
            odds,
            allow_plain: true,
            total_particle_effect_odds: 0,
            total_stat_tracker_odds: 0,
        }
    }

    pub fn new_with(prototype_key: &str, odds: i32, allow_plain: bool, particle_effects: Vec<ParticleEffectEntry>, stat_trackers: Vec<StatTrackerEntry>) -> Self {
        let mut entry = CrateSeriesEntry::new(prototype_key, odds);
        entry.allow_plain = allow_plain;
        for particle_effect in particle_effects {
            entry.add_particle_effect(particle_effect);
        }
        for stat_tracker in stat_trackers {
            entry.add_stat_tracker(stat_tracker);
        }
        entry
    }

    pub fn add_particle_effect(&mut self, particle_effect: ParticleEffectEntry) {
        self.total_particle_effect_odds += particle_effect.odds;
        self.particle_effects.push(particle_effect);
    }

    pub fn add_stat_tracker(&mut self, stat_tracker: StatTrackerEntry) {
        self.total_stat_tracker_odds += stat_tracker.odds;
        self.stat_trackers.push(stat_tracker);
    }

    pub fn unbox_one(&self) -> Result<Item, UnlockProblem> {
        let key = &self.prototype_key;
        let prototype = PROTOTYPES.get(key).ok_or(NoSuchPrototype(key.to_owned()))?.clone();

        let mut item = Item::new(prototype);

        let mut effect: Option<Arc<ParticleEffect>> = None;
        let mut stat_tracker: Option<StatTracker> = None;

        let mut loops = 0;
        let loop_limit = (self.particle_effects.len() + self.stat_trackers.len()) as i32 * 10; // TODO: This is nuts. Are we potentially looping thousands of times?
        info!("Particle effect odds: {}; Stat tracker odds: {}", self.total_particle_effect_odds, self.total_stat_tracker_odds);

        'create: loop  {
            loops += 1;
            if self.total_particle_effect_odds > 0 {
                let mut current_odds = 0;
                let random_number = rand::rng().random_range(0..self.total_particle_effect_odds);
                info!("Particle effect: {}", random_number);
                'particle: for entry in &self.particle_effects {
                    current_odds += entry.odds;
                    info!("\tCurrent odds: {} (+{}) - {}", current_odds, entry.odds, random_number < current_odds);
                    if random_number < current_odds {
                        if effect.is_none() {
                            if let Some(key) = &entry.particle_effect_key && let Some(selected_effect) = PARTICLES.get(key) {
                                info!("\t\tSetting!");
                                effect = Some(selected_effect.clone());
                            }
                        }
                        break 'particle;
                    }
                }
            }

            if self.total_stat_tracker_odds > 0 {
                let random_number = rand::rng().random_range(0..self.total_stat_tracker_odds);
                let mut current_odds = 0;
                info!("Stat tracker: {}", random_number);
                'stat_tracker: for entry in &self.stat_trackers {
                    current_odds += entry.odds;
                    info!("\tCurrent odds: {} (+{}) - {}", current_odds, entry.odds, random_number < current_odds);
                    if random_number < current_odds {
                        if stat_tracker.is_none() {
                            if let Some(new_stat_tracker) = &entry.stat_tracker {
                                info!("\t\tSetting!");
                                stat_tracker = Some(new_stat_tracker.clone());
                            }
                        }
                        break 'stat_tracker;
                    }
                }

            }

            if !self.allow_plain && self.total_stat_tracker_odds == 0 && self.total_particle_effect_odds > 0 {
                warn!("{}", get!("create_drop.error.no_odds", "item", self.prototype_key));
                return Err(NoQualities(self.prototype_key.clone()));
            }
            if self.allow_plain || effect.is_some() || stat_tracker.is_some() { break 'create; }
            if loops >= loop_limit {
                warn!("{}", get!("crate_drop.error.quality_loop", "item", self.prototype_key));
                return Err(ItemCreationLoop);
            }
        }

        item.stat_tracker = stat_tracker;
        item.particle_effect = effect;

        Ok(item)
    }
}
