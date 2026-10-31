//! What a weapon is, as data. The systems live in [`crate::game::weapon`].
//!
//! These values cross the wire, so a server can ship its own weapons. Nothing
//! here reads a file: a client is told its catalog by the server.
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

/// A stable FNV-1a hash of the id string, not an index into the catalog,
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
    /// Meters per second of corpse shove, per point of damage.
    pub shove_per_damage: f32,
    #[serde(rename = "shove_radius")]
    pub shove_radius_m: f32,
    /// `None` is one bullet down the aim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pellets: Option<PelletGrid>,
    /// Thrown at the body only by the shot that kills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_shove: Option<KillShoveSpec>,
    /// More damage close up: `scale` times it at `near`, rising to full at `far`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp_up: Option<DistanceScale>,
    /// Less damage far off: full at `near`, falling to `scale` times it at `far`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub falloff: Option<DistanceScale>,
}

/// A straight line in damage over distance from the shooter, flat beyond either end. Ramp-up and
/// falloff are one line each, so either can be had without the other.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct DistanceScale {
    /// Meters.
    pub near: f32,
    pub far: f32,
    pub scale: f32,
}

impl DistanceScale {
    /// 0 at or before `near`, 1 at or past `far`.
    fn along(&self, distance: f32) -> f32 {
        ((distance - self.near) / (self.far - self.near).max(1e-3)).clamp(0.0, 1.0)
    }
}

/// What a hit `distance` meters from the shooter is worth, times its damage, under both lines.
pub fn distance_scale(ramp_up: Option<DistanceScale>, falloff: Option<DistanceScale>, distance: f32) -> f32 {
    let ramp = ramp_up.map_or(1.0, |line| line.scale + (1.0 - line.scale) * line.along(distance));
    let fall = falloff.map_or(1.0, |line| 1.0 + (line.scale - 1.0) * line.along(distance));
    ramp * fall
}

impl HitscanSpec {
    /// What a hit `distance` meters from the shooter's eye is worth, times its damage.
    pub fn scale_at(&self, distance: f32) -> f32 {
        distance_scale(self.ramp_up, self.falloff, distance)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct KillShoveSpec {
    /// Meters per second along the aim, at the hit.
    pub speed: f32,
    /// Up on top of `speed`, in meters per second, so the body leaves the floor
    /// rather than sliding along it.
    #[serde(default)]
    pub lift: f32,
    /// Wide enough to take in most of the body, so it goes as one.
    #[serde(rename = "radius")]
    pub radius_m: f32,
}

/// A square of pellets, the same every shot: an RNG spread cannot be
/// reconciled, and a fixed pattern is one a player can learn.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct PelletGrid {
    /// Pellets along each side, so 3 is nine.
    pub across: u32,
    /// Meters traveled per meter of pattern width, corner pellet to corner
    /// pellet: 30 is a square one meter wide at thirty meters.
    pub spread: f32,
}

impl PelletGrid {
    /// Each pellet's offset from the aim, in meters across per meter ahead.
    pub fn offsets(&self) -> Vec<Vec2> {
        let across = self.across.max(1);
        let width = if across > 1 && self.spread > 0.0 { 1.0 / self.spread } else { 0.0 };
        let step = if across > 1 { width / (across - 1) as f32 } else { 0.0 };
        let start = -width / 2.0;
        (0..across)
            .flat_map(|row| {
                (0..across).map(move |column| {
                    Vec2::new(start + column as f32 * step, start + row as f32 * step)
                })
            })
            .collect()
    }
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
/// The stubs round-trip through the data files but have no behavior yet.
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
    /// A shot per press. `interval`, in seconds, is the least time between
    /// two, so clicking fast cannot outrun the weapon; a press inside it is
    /// dropped, as one while drawing is.
    Semi {
        #[serde(default)]
        interval: f32,
    },
    /// Shots while held, `interval` seconds apart.
    Automatic { interval: f32 },
}

impl Cadence {
    pub fn interval(self) -> f32 {
        match self {
            Cadence::Semi { interval } | Cadence::Automatic { interval } => interval,
        }
    }
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
        cadence: Cadence::Semi { interval: 0.0 },
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
    /// Seconds for the first round of a reload, where it differs from the rest: a hand going to
    /// the pouch before it starts feeding them in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_reload: Option<f32>,
}

impl Magazine {
    pub const BOTTOMLESS: Self = Self {
        clip: None,
        reserve: 0,
        reload: 0.0,
        reload_one_at_a_time: false,
        first_reload: None,
    };
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weapon {
    pub id: WeaponId,
    /// What `weapons.toml` calls it, which is what a map names it by; `id` is
    /// its hash.
    #[serde(default)]
    pub key: String,
    /// A lang key, not display text.
    pub name_key: String,
    /// A prop name under a pack's `props/`, without the extension. A name
    /// rather than geometry so the catalog stays small on the wire; each
    /// machine builds the model from its own pack. `None` is unmodeled, kept
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

/// The places a class's loadout names; no bound on what a body carries.
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

/// What a body carries, by id. The list and what is held are one component
/// because replication does not promise to deliver two in one packet.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Equipped {
    /// The class's starting weapons in loadout-slot order, then pickups in
    /// the order they were taken. Never reordered and never holding one id
    /// twice.
    carried: Vec<WeaponId>,
    /// Always one of `carried`, or nothing.
    held: Option<WeaponId>,
}

impl Equipped {
    /// Holding the first, with any repeat dropped.
    pub fn carrying(weapons: impl IntoIterator<Item = WeaponId>) -> Self {
        let mut equipped = Self::default();
        for weapon in weapons {
            equipped.add(weapon);
        }
        equipped.held = equipped.carried.first().copied();
        equipped
    }

    pub fn carried(&self) -> &[WeaponId] {
        &self.carried
    }

    pub fn held(&self) -> Option<WeaponId> {
        self.held
    }

    pub fn carries(&self, weapon: WeaponId) -> bool {
        self.carried.contains(&weapon)
    }

    /// One not carried is ignored. Idempotent, so a replayed tick reaches the
    /// same loadout.
    pub fn select(&mut self, weapon: WeaponId) {
        if self.carries(weapon) {
            self.held = Some(weapon);
        }
    }

    /// Returns whether it was new.
    pub fn add(&mut self, weapon: WeaponId) -> bool {
        if self.carries(weapon) {
            return false;
        }
        self.carried.push(weapon);
        true
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
/// rare, and replication is change-detected per component. Keyed by weapon,
/// not by position in `Equipped::carried`, because the two arrive in
/// different packets.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Ammo {
    pub weapons: Vec<(WeaponId, AmmoState)>,
}

impl Ammo {
    /// `None` until a client's `Ammo` catches up with a pickup: draw blank.
    pub fn at(&self, weapon: WeaponId) -> Option<&AmmoState> {
        self.weapons.iter().find(|(id, _)| *id == weapon).map(|(_, state)| state)
    }

    /// Makes an empty entry if there is none.
    pub fn at_mut(&mut self, weapon: WeaponId) -> &mut AmmoState {
        let index = match self.weapons.iter().position(|(id, _)| *id == weapon) {
            Some(index) => index,
            None => {
                self.weapons.push((weapon, AmmoState::default()));
                self.weapons.len() - 1
            }
        };
        &mut self.weapons[index].1
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
    /// Validated at load to be one of `permitted`. `None` starts the slot
    /// empty, while a mode's loadout may still fill it.
    pub default: Option<WeaponId>,
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
    /// A fresh body's health; `None` is [`DEFAULT_HEALTH`](crate::common::damage::DEFAULT_HEALTH).
    #[serde(default)]
    pub health: Option<u32>,
    /// The most armor a body holds; `None` is [`DEFAULT_MAX_ARMOR`](crate::common::damage::DEFAULT_MAX_ARMOR).
    #[serde(default)]
    pub armor: Option<u32>,
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
pub struct WeaponCatalog {
    pub weapons: Vec<Weapon>,
    pub loadouts: Vec<(Class, Loadout)>,
}

impl WeaponCatalog {
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

    /// Resolve on every call; never cache. The ids and the catalog arrive on
    /// different channels.
    pub fn held_by(&self, equipped: &Equipped) -> Option<&Weapon> {
        self.get(equipped.held()?)
    }

    /// Returned together so a body cannot hold a weapon with no ammo entry.
    /// Anything missing gives empty hands rather than a panic.
    ///
    /// `only` is a mode's loadout: each slot gets the first of those weapons
    /// the class permits there, else nothing. `None` is the class's defaults.
    pub fn starting_equipment(&self, class: Class, only: Option<&[WeaponId]>) -> (Equipped, Ammo) {
        let mut equipped = Equipped::default();
        let mut ammo = Ammo::default();

        let Some(loadout) = self.loadout(class) else {
            return (equipped, ammo);
        };

        for slot in Slot::ALL {
            let Some(choices) = loadout.at(slot) else { continue };
            let chosen = match only {
                None => choices.default,
                Some(only) => only.iter().copied().find(|id| choices.permitted.contains(id)),
            };
            let Some(weapon) = chosen.and_then(|id| self.get(id)) else { continue };
            if equipped.add(weapon.id) {
                *ammo.at_mut(weapon.id) = Ammo::full(&weapon.magazine);
            }
        }

        // The first carried rather than the primary's: a class with an empty
        // primary would spawn holding nothing.
        equipped.held = equipped.carried.first().copied();

        (equipped, ammo)
    }
}

