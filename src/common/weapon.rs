//! What a weapon is, as data. The systems live in [`crate::game::weapon`].
//!
//! These values cross the wire, so a server can ship its own weapons. Nothing
//! here reads a file: a client is told its catalogue by the server.
//!
//! An action is what one button does; a weapon is a pair of them. A new weapon
//! is a row in a data file; a new kind of weapon is a [`WeaponAction`] variant
//! and a system that answers for it.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::class::Class;
use crate::common::flame::FlameSpec;
use crate::common::projectile::ProjectileSpec;

/// A stable FNV-1a hash of the id string, not an index into the catalogue,
/// so machines with differently ordered files agree on it.
///
/// Written out rather than taken from a std hasher, whose output may change
/// between releases; this number is on the wire.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Reflect, Serialize, Deserialize,
)]
pub struct WeaponId(pub u32);

impl WeaponId {
    pub const fn of(name: &str) -> Self {
        let bytes = name.as_bytes();
        let mut hash: u32 = 0x811c_9dc5;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u32;
            hash = hash.wrapping_mul(0x0100_0193);
            i += 1;
        }
        Self(hash)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct HitscanSpec {
    #[serde(rename = "range")]
    pub range_m: f32,
    pub damage: u32,
    pub headshot_multiplier: u32,
    /// Metres per second of corpse shove, per point of damage.
    pub shove_per_damage: f32,
    #[serde(rename = "shove_radius")]
    pub shove_radius_m: f32,
}

/// Stub.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ExtinguishSpec {
    pub range: f32,
    pub spread: f32,
}

/// Stub.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct HealBeamSpec {
    pub range: f32,
    pub heal_per_second: u32,
}

/// Stub.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct InvulnSpec {
    pub duration: f32,
    pub cooldown: f32,
}

/// Stub.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ScopeSpec {
    pub zoom: f32,
}

/// What one mouse button does.
///
/// The stubs round-trip through the data files but have no behaviour yet.
/// They log rather than panic, so a listen server does not drop everybody
/// because somebody right-clicked.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WeaponAction {
    Hitscan(HitscanSpec),
    Projectile(ProjectileSpec),
    Flame(FlameSpec),
    Extinguish(ExtinguishSpec),
    HealBeam(HealBeamSpec),
    Invulnerable(InvulnSpec),
    Scope(ScopeSpec),
    /// A variant rather than `Option<WeaponAction>`: a weapon with no
    /// secondary is an ordinary weapon, not one with something missing.
    None,
}

impl WeaponAction {
    /// Only used to decide whether to log.
    pub fn is_implemented(&self) -> bool {
        matches!(
            self,
            WeaponAction::Hitscan(_)
                | WeaponAction::Projectile(_)
                | WeaponAction::Flame(_)
                | WeaponAction::None
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence {
    Semi,
    /// `interval` in seconds.
    Automatic { interval: f32 },
}

/// On the button rather than the weapon: a flamethrower's puff and airblast
/// cost different amounts, and a scope toggle costs nothing.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cost {
    Free,
    /// Out of the clip, or out of the reserve if the weapon has no clip.
    Ammo { rounds: u32 },
    /// `0.0`–`1.0` of the weapon's meter.
    Charge { fraction: f32 },
}

/// An action mounted on a button. The cadence lives here, not in the spec,
/// because a cooldown is a fact about a button.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Mounted {
    pub action: WeaponAction,
    pub cadence: Cadence,
    pub cost: Cost,
}

impl Mounted {
    pub const EMPTY: Self = Self {
        action: WeaponAction::None,
        cadence: Cadence::Semi,
        cost: Cost::Free,
    };
}

/// `clip: None` is a weapon with no magazine (the minigun) that draws each
/// shot from the reserve and never reloads, rather than a clip the size of the
/// reserve.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Magazine {
    #[serde(default)]
    pub clip: Option<u32>,
    /// Rounds carried beyond what is loaded.
    pub reserve: u32,
    /// Seconds.
    pub reload: f32,
    /// One round at a time and interruptible (a shotgun), or the whole clip at
    /// once.
    #[serde(default)]
    pub reload_one_at_a_time: bool,
}

impl Magazine {
    pub const BOTTOMLESS: Self = Self {
        clip: None,
        reserve: 0,
        reload: 0.0,
        reload_one_at_a_time: false,
    };
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weapon {
    pub id: WeaponId,
    /// A lang key, not display text.
    pub name_key: String,
    /// A prop name under a pack's `props/`, without the extension. A name
    /// rather than geometry so the catalogue stays small on the wire; each
    /// machine builds the model from its own pack. `None` is unmodelled, kept
    /// distinct from a name that fails to load.
    #[serde(default)]
    pub model: Option<String>,
    /// The digit that selects it, `1..=BUCKETS`. Weapons may share one.
    pub bucket: u8,
    /// Seconds after being drawn before it may fire.
    pub deploy: f32,
    pub primary: Mounted,
    pub secondary: Mounted,
    pub magazine: Magazine,
}

/// A bucket is the weapon's; a [`Slot`] is a place in a class's loadout.
pub const BUCKETS: u8 = 9;

/// More than any class needs today, because growing it later means growing a
/// replicated array.
pub const SLOTS: usize = 6;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, EnumIter, Reflect,
    Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    #[default]
    Primary,
    Secondary,
    Melee,
    Utility,
    Gadget,
    Special,
}

impl Slot {
    pub const ALL: [Slot; SLOTS] = [
        Slot::Primary,
        Slot::Secondary,
        Slot::Melee,
        Slot::Utility,
        Slot::Gadget,
        Slot::Special,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn name(self) -> String {
        match self {
            Slot::Primary => crate::get!("weapon.slots.primary"),
            Slot::Secondary => crate::get!("weapon.slots.secondary"),
            Slot::Melee => crate::get!("weapon.slots.melee"),
            Slot::Utility => crate::get!("weapon.slots.utility"),
            Slot::Gadget => crate::get!("weapon.slots.gadget"),
            Slot::Special => crate::get!("weapon.slots.special"),
        }
    }
}

/// Ids only, never weapon numbers. The ids and the held slot are one component
/// because replication does not promise to deliver two components in one
/// packet.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize,
)]
pub struct Equipped {
    pub slots: [Option<WeaponId>; SLOTS],
    pub held: Slot,
}

impl Equipped {
    pub fn held(&self) -> Option<WeaponId> {
        self.slots[self.held.index()]
    }

    pub fn get(&self, slot: Slot) -> Option<WeaponId> {
        self.slots[slot.index()]
    }

    /// An empty slot is ignored. Idempotent, so a replayed tick reaches the
    /// same loadout.
    pub fn select(&mut self, slot: Slot) {
        if self.slots[slot.index()].is_some() {
            self.held = slot;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct AmmoState {
    pub clip: u32,
    pub reserve: u32,
    /// Seconds left, counting down, so the default is a weapon that is ready.
    pub reloading: f32,
    /// `0.0`–`1.0`, for [`Cost::Charge`].
    pub charge: f32,
}

/// Separate from [`Equipped`] because ammo changes every shot and a switch is
/// rare, and replication is change-detected per component.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize,
)]
pub struct Ammo {
    pub slots: [AmmoState; SLOTS],
}

impl Ammo {
    pub fn at(&self, slot: Slot) -> &AmmoState {
        &self.slots[slot.index()]
    }

    pub fn at_mut(&mut self, slot: Slot) -> &mut AmmoState {
        &mut self.slots[slot.index()]
    }

    pub fn full(magazine: &Magazine) -> AmmoState {
        AmmoState {
            clip: magazine.clip.unwrap_or(0),
            reserve: magazine.reserve,
            reloading: 0.0,
            charge: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotChoices {
    pub permitted: Vec<WeaponId>,
    /// Validated at load to be one of `permitted`.
    pub default: WeaponId,
}

/// A list rather than a `HashMap`, whose iteration order would make equal
/// values encode differently on the wire. Only slots the class uses appear.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Loadout {
    pub slots: Vec<(Slot, SlotChoices)>,
    /// A prop under a pack's `props/`, worn on the head. A class's rather than
    /// a player's until there are items to own one by.
    #[serde(default)]
    pub hat: Option<String>,
}

impl Loadout {
    pub fn at(&self, slot: Slot) -> Option<&SlotChoices> {
        self.slots.iter().find(|(at, _)| *at == slot).map(|(_, choices)| choices)
    }

    /// Replaces, so a later pack can retool a class.
    pub fn set(&mut self, slot: Slot, choices: SlotChoices) {
        match self.slots.iter_mut().find(|(at, _)| *at == slot) {
            Some(entry) => entry.1 = choices,
            None => self.slots.push((slot, choices)),
        }
    }
}

/// The authority reads it from disk; a client's is replaced wholesale by the
/// server's.
///
/// Lists rather than maps: it is compared for equality to decide whether to
/// send it, and a `HashMap` iterates in an arbitrary order. A few dozen
/// weapons make a scan free.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WeaponCatalogue {
    pub weapons: Vec<Weapon>,
    pub loadouts: Vec<(Class, Loadout)>,
}

impl WeaponCatalogue {
    pub fn get(&self, id: WeaponId) -> Option<&Weapon> {
        self.weapons.iter().find(|weapon| weapon.id == id)
    }

    /// Replaces a weapon of the same id, so a later pack can retune one.
    pub fn insert(&mut self, weapon: Weapon) {
        match self.weapons.iter_mut().find(|already| already.id == weapon.id) {
            Some(already) => *already = weapon,
            None => self.weapons.push(weapon),
        }
    }

    pub fn loadout(&self, class: Class) -> Option<&Loadout> {
        self.loadouts.iter().find(|(for_class, _)| *for_class == class).map(|(_, l)| l)
    }

    pub fn set_loadout(&mut self, class: Class, loadout: Loadout) {
        match self.loadouts.iter_mut().find(|(for_class, _)| *for_class == class) {
            Some(entry) => entry.1 = loadout,
            None => self.loadouts.push((class, loadout)),
        }
    }

    /// Resolve on every call; never cache. The ids and the catalogue arrive on
    /// different channels.
    pub fn held_by(&self, equipped: &Equipped) -> Option<&Weapon> {
        self.get(equipped.held()?)
    }

    /// Returned together so a body cannot hold a weapon with no ammo entry.
    /// Anything missing gives empty hands rather than a panic.
    pub fn starting_equipment(&self, class: Class) -> (Equipped, Ammo) {
        let mut equipped = Equipped::default();
        let mut ammo = Ammo::default();

        let Some(loadout) = self.loadout(class) else {
            return (equipped, ammo);
        };

        for slot in Slot::ALL {
            let Some(choices) = loadout.at(slot) else { continue };
            let Some(weapon) = self.get(choices.default) else { continue };
            equipped.slots[slot.index()] = Some(weapon.id);
            ammo.slots[slot.index()] = Ammo::full(&weapon.magazine);
        }

        // Not `Primary` regardless: a class with an empty primary would spawn
        // holding nothing.
        equipped.held = Slot::ALL
            .into_iter()
            .find(|slot| equipped.get(*slot).is_some())
            .unwrap_or_default();

        (equipped, ammo)
    }
}

