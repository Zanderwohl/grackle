//! What a weapon *is*, as data: an identity, two buttons, and how it is fed.
//!
//! The systems live in [`crate::game::weapon`]; everything here is a value.
//! That split is the same one [`crate::common::projectile`] and
//! [`crate::common::flame`] already make, and it is load-bearing now rather
//! than tidy: these values cross the wire, so a server can ship its own
//! weapons and every client be told what they are.
//!
//! **An action is what one button does.** A weapon is a pair of them — left
//! click and right click — which is the whole taxonomy. Adding a *weapon* is
//! adding a table to a data file; adding a *kind* of weapon is adding a
//! [`WeaponAction`] variant and a system that answers for it.
//!
//! Two things are deliberately not here. A body's weapons are named by
//! [`WeaponId`] and nothing else, so a body never carries a second opinion
//! about what a rocket does; and nothing in this module reads a file, because
//! a client is *told* its catalogue by the server and must never disagree.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::class::Class;
use crate::common::flame::FlameSpec;
use crate::common::projectile::ProjectileSpec;

/// What a weapon is called on the wire and in a data file.
///
/// A stable hash of the id string, and **not an index into the catalogue**.
/// An index is a fact about the order a catalogue happened to load in, so two
/// machines with different files would read the same number as different
/// weapons — the same two-descriptions-of-one-thing shape every bug in the map
/// layer has had.
///
/// FNV-1a written out here rather than taken from a hasher whose output is
/// allowed to change between releases: this number is on the wire, and a
/// weapon that renames itself when the standard library is upgraded is not a
/// weapon anybody can debug.
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

/// The numbers a traced shot is made of.
///
/// These were module constants in [`crate::game::hitscan`], marked as
/// placeholders to be deleted once a real weapon stated its own. This is that:
/// a hitscan weapon now says how far it reaches and what it is worth, and no
/// two hitscan weapons have to agree.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct HitscanSpec {
    /// How far the shot carries, in metres.
    pub range: f32,
    /// What a body shot takes off.
    pub damage: u32,
    /// What a head shot multiplies that by.
    pub headshot_multiplier: u32,
    /// How hard a corpse is shoved, per point of damage, in metres per second.
    pub shove_per_damage: f32,
    /// How much of a corpse one bullet drags with it, in metres. Tight — a
    /// bullet moves a limb, an explosion moves a body.
    pub shove_radius: f32,
}

/// Putting a fire out. Not implemented — see the module docs on stubs.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ExtinguishSpec {
    pub range: f32,
    pub spread: f32,
}

/// Healing somebody at the other end of a beam. Not implemented.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct HealBeamSpec {
    pub range: f32,
    pub heal_per_second: u32,
}

/// Being untouchable for a while. Not implemented.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct InvulnSpec {
    pub duration: f32,
    pub cooldown: f32,
}

/// Narrowing the field of view. Not implemented.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ScopeSpec {
    pub zoom: f32,
}

/// What one mouse button does.
///
/// Each variant carries its own numbers inline, which is what makes a weapon a
/// row in a file rather than a type. Four of these are **stubs**: the shape is
/// real and round-trips through the data files, and the behaviour is not
/// written yet. They log rather than panicking — a listen server that dropped
/// everybody because somebody right-clicked would be a worse placeholder than
/// a weapon that does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WeaponAction {
    /// Lands the instant it is fired. See [`crate::game::hitscan`].
    Hitscan(HitscanSpec),
    /// Leaves the muzzle and has to get there. See
    /// [`crate::game::projectile`].
    Projectile(ProjectileSpec),
    /// A cone of fire. See [`crate::game::flame`].
    Flame(FlameSpec),
    /// Stub.
    Extinguish(ExtinguishSpec),
    /// Stub.
    HealBeam(HealBeamSpec),
    /// Stub.
    Invulnerable(InvulnSpec),
    /// Stub.
    Scope(ScopeSpec),
    /// A button that does nothing, **stated rather than implied**.
    ///
    /// An `Option<WeaponAction>` would make every reader ask the same question
    /// twice — is there an action, and is it one I answer for — and a weapon
    /// with no secondary is a perfectly ordinary weapon rather than a weapon
    /// with something missing.
    None,
}

impl WeaponAction {
    /// Whether anything answers for this yet. Used only to decide whether to
    /// log; nothing branches on it to decide what happened.
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

/// How a button answers being held down.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence {
    /// One shot per press, however long it is held.
    Semi,
    /// Keeps firing while held, this many seconds apart.
    Automatic { interval: f32 },
}

/// What one pull of a button costs.
///
/// **On the button rather than on the weapon**, because the two halves of one
/// weapon do not cost the same thing: a flamethrower's puff is a round and its
/// airblast is five, a scope toggle is nothing at all, and an invuln is a
/// meter rather than a magazine.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cost {
    /// Nothing. A melee swing, a scope toggle.
    Free,
    /// Rounds, out of the weapon's clip — or straight out of its reserve if it
    /// has no clip.
    Ammo { rounds: u32 },
    /// A fraction of the weapon's own meter, `0.0`–`1.0`.
    Charge { fraction: f32 },
}

/// An action as it is mounted on a button: what it does, how fast, what it
/// costs.
///
/// The cadence is **here and not inside the spec**, because a cooldown is a
/// fact about a button. `FlameSpec::interval` moved into this and stopped
/// existing; leaving both would have been two writers for one rate of fire,
/// and the symptom is a data file that is quietly ignored.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Mounted {
    pub action: WeaponAction,
    pub cadence: Cadence,
    pub cost: Cost,
}

impl Mounted {
    /// A button that does nothing, costs nothing and is always ready.
    pub const EMPTY: Self = Self {
        action: WeaponAction::None,
        cadence: Cadence::Semi,
        cost: Cost::Free,
    };
}

/// How a weapon is fed.
///
/// **`clip: None` is the minigun case** — a weapon with no magazine, whose
/// whole supply is loaded at once and which draws each shot straight from the
/// reserve. Stated as an absent clip rather than as a clip the size of the
/// reserve, because the second is a lie a reload system then has to keep
/// believing.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Magazine {
    /// How many rounds fit loaded. A rocket launcher's four.
    #[serde(default)]
    pub clip: Option<u32>,
    /// How many the carrier holds beyond what is loaded.
    ///
    /// The weapon's number. If a class is ever to carry more than another with
    /// the same weapon, that override is one field on [`SlotChoices`] and
    /// nothing else moves.
    pub reserve: u32,
    /// Seconds to reload.
    pub reload: f32,
    /// Whether a reload puts in one round at a time and may be interrupted (a
    /// shotgun) or fills the clip in one go (a rocket launcher).
    #[serde(default)]
    pub reload_one_at_a_time: bool,
}

impl Magazine {
    /// A weapon that costs nothing to fire and never runs out: a melee, and
    /// the sensible default for a row that does not mention a magazine.
    pub const BOTTOMLESS: Self = Self {
        clip: None,
        reserve: 0,
        reload: 0.0,
        reload_one_at_a_time: false,
    };
}

/// One thing you can hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weapon {
    pub id: WeaponId,
    /// A lang key, **not a name**. A display string in a data file would be
    /// text outside the lang layer, which is the one thing `CLAUDE.md` says
    /// never to do.
    pub name_key: String,
    pub primary: Mounted,
    pub secondary: Mounted,
    pub magazine: Magazine,
}

/// How many slots a body has.
///
/// Six, which is more than any class needs today, and that is the point: a
/// slot count is a fact about the widest class that will ever exist, and
/// growing it later means growing a replicated array. A class that wants three
/// leaves three empty.
pub const SLOTS: usize = 6;

/// Which hand a weapon is in.
///
/// Named rather than numbered: "slot 2" is a fact about a keyboard, and this
/// has to mean the same thing on both ends of a connection. The last three are
/// named for what they are *for* rather than after any class, so a class
/// needing one does not have to be the class the slot was named after.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, EnumIter, Reflect, Serialize, Deserialize,
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

    /// The slot a key press means, or `None` for a digit past the last slot.
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

/// What a body is carrying, and which of it is in its hands.
///
/// **Ids and nothing else**: a body carries what to look the numbers up by,
/// not the numbers. That is what makes this cheap enough to replicate whenever
/// it changes, and what stops a client holding a second opinion about what a
/// rocket does.
///
/// One component rather than two — the ids and the held slot together — because
/// replication does not promise to deliver two components in one packet, and
/// half an answer is a body nothing can describe. That is the lesson
/// [`crate::game::ragdoll`] already paid for.
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

    /// Switch to a slot.
    ///
    /// A slot with nothing in it is ignored, so pressing `5` on a class that
    /// carries three leaves you holding what you had. Idempotent, which is
    /// what lets the input be *read* rather than taken: selecting the slot
    /// already held is not a second switch, so a replayed tick reaches the
    /// same loadout.
    pub fn select(&mut self, slot: Slot) {
        if self.slots[slot.index()].is_some() {
            self.held = slot;
        }
    }
}

/// What one of a body's weapons has loaded, in reserve and charged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct AmmoState {
    pub clip: u32,
    pub reserve: u32,
    /// Seconds left of a reload in progress.
    ///
    /// Counting **down**, like [`crate::game::weapon::Trigger`] and for the
    /// same reason: the default is a weapon that is ready, not one stuck
    /// half-way through a reload it never started.
    pub reloading: f32,
    /// `0.0`–`1.0`, for actions that cost [`Cost::Charge`].
    pub charge: f32,
}

/// What each of a body's weapons has.
///
/// A component beside [`Equipped`] and keyed the same way, because ammo is a
/// fact about *this carrier's copy* of a weapon: a weapon is a value, and two
/// people holding the same rocket launcher are not sharing its four rockets.
///
/// Its own component rather than fields inside `Equipped` because the two
/// change at completely different rates — a switch is rare, a round is every
/// shot — and replication is change-detected per component.
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

    /// A slot loaded to the top: full clip, full reserve, no reload running,
    /// no charge. What a life starts with.
    pub fn full(magazine: &Magazine) -> AmmoState {
        AmmoState {
            clip: magazine.clip.unwrap_or(0),
            reserve: magazine.reserve,
            reloading: 0.0,
            charge: 0.0,
        }
    }
}

/// What a class may carry in one slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotChoices {
    /// Every weapon this class may put here.
    pub permitted: Vec<WeaponId>,
    /// What it starts with. Validated at load to be one of `permitted` — a
    /// default outside the list is a permitted list that means nothing.
    pub default: WeaponId,
}

/// What a class may carry, slot by slot.
///
/// A map rather than an array, because most classes leave most slots empty and
/// an array of `Option` in a *data* file reads as six rows of nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Loadout {
    pub slots: HashMap<Slot, SlotChoices>,
}

/// Every weapon this match has, and what each class may hold.
///
/// A resource, because it is a fact about the match rather than about any
/// body. The authority reads it from disk; a client's is replaced wholesale by
/// what the server sends — the same arrangement the map already has, and for
/// the same reason: two descriptions of one thing is where the bugs live.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WeaponCatalogue {
    pub weapons: HashMap<WeaponId, Weapon>,
    pub loadouts: HashMap<Class, Loadout>,
}

impl WeaponCatalogue {
    pub fn get(&self, id: WeaponId) -> Option<&Weapon> {
        self.weapons.get(&id)
    }

    /// The weapon in a body's hands, if the catalogue has heard of it.
    ///
    /// Resolved **every time it is asked for**, never cached when an
    /// `Equipped` arrives: a body's ids and the catalogue travel on different
    /// channels, and a resolution cached on arrival is a body that is never
    /// named at all. Key on absence, not on arrival.
    pub fn held_by(&self, equipped: &Equipped) -> Option<&Weapon> {
        self.get(equipped.held()?)
    }

    /// What a body starts a life with: full clips, full reserves.
    ///
    /// The two come back together because they are derived from the same
    /// lookup, and deriving them separately is how a body ends up holding a
    /// weapon it has no ammo entry for. A missing class, a missing weapon, an
    /// empty catalogue: all give empty hands — a body that cannot shoot rather
    /// than a panic.
    pub fn starting_equipment(&self, class: Class) -> (Equipped, Ammo) {
        let mut equipped = Equipped::default();
        let mut ammo = Ammo::default();

        let Some(loadout) = self.loadouts.get(&class) else {
            return (equipped, ammo);
        };

        for slot in Slot::ALL {
            let Some(choices) = loadout.slots.get(&slot) else { continue };
            let Some(weapon) = self.get(choices.default) else { continue };
            equipped.slots[slot.index()] = Some(weapon.id);
            ammo.slots[slot.index()] = Ammo::full(&weapon.magazine);
        }

        // Hold the first slot that actually has something in it, rather than
        // `Primary` regardless: a class whose primary is empty would otherwise
        // spawn holding nothing while carrying a weapon.
        equipped.held = Slot::ALL
            .into_iter()
            .find(|slot| equipped.get(*slot).is_some())
            .unwrap_or_default();

        (equipped, ammo)
    }
}

