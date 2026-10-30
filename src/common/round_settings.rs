//! How the server runs a round: how many slots it has, and whether bots take the ones nobody
//! has. Held and acted on by the authority, and not part of the map, so neither saved with it nor
//! sent to clients.

use bevy::prelude::*;

/// The most slots the Round tab offers.
pub const MOST_SLOTS: u32 = 32;

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct RoundSettings {
    /// Players in a round, the host and bots included. Caps the clients a server takes, from when
    /// it starts listening.
    pub slots: u32,
    pub fill_with_bots: bool,
}

impl Default for RoundSettings {
    fn default() -> Self {
        RoundSettings { slots: 8, fill_with_bots: false }
    }
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
