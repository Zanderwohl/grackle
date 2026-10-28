//! A map's palette: what a **role** is painted with on each side. A room slot names either a
//! material or a role, and a role is resolved by the room's [`Side`] — which is the Mirror's
//! answer, not the room's — so one half of a map is authored and the copy comes out in the other
//! team's colors. See "Team-coded paint is a role" in documentation/surfaces.md.
//!
//! Part of the map's metadata, so it saves with the blueprint and reaches clients with the rest of
//! the settings. [`publish_palette`] is the only writer of the [`Palette`] resource; nothing reads
//! `MapMetadata` for it.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::team::{Side, Team};
use crate::editor::map_metadata::MapMetadata;
use crate::tool::room::{RoomSlot, RoomSurfaces};

/// Keyed by role name. A slot's name is looked up here first, so a role shadows a material of the
/// same name.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub roles: BTreeMap<String, RolePaint>,
}

/// One role's material per side. No field is skipped when empty: the palette crosses the wire
/// with postcard, which cannot read a field left out.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RolePaint {
    #[serde(default)]
    pub sides: BTreeMap<Team, String>,
    /// For a room no Mirror claims, or a side the role does not name. `None` draws the slot's default.
    #[serde(default)]
    pub unsided: Option<String>,
}

/// Roles every map has without saying so, so a map with no art pass can still show whose side a
/// room is on. A map's own role of the same name shadows one.
static BUILT_IN: LazyLock<BTreeMap<String, RolePaint>> = LazyLock::new(|| {
    BTreeMap::from([(
        TEAM_GRID.to_owned(),
        RolePaint {
            sides: BTreeMap::from([
                (Team::Red, "grid_red".to_owned()),
                (Team::Blue, "grid_blue".to_owned()),
                (Team::Yellow, "grid_yellow".to_owned()),
                (Team::Green, "grid_green".to_owned()),
            ]),
            unsided: Some("orange".to_owned()),
        },
    )])
});

/// The default grid in each team's color.
pub const TEAM_GRID: &str = "team_grid";

impl Palette {
    /// The map's roles, then the built-in ones it does not shadow.
    pub fn every_role(&self) -> impl Iterator<Item = (&String, &RolePaint)> {
        self.roles.iter().chain(BUILT_IN.iter().filter(|(name, _)| !self.roles.contains_key(*name)))
    }

    /// The material a slot naming `name` is painted with on `side`: the name itself unless it is a
    /// role. `None` for a role with nothing for this side, which draws the slot's default.
    pub fn resolve<'a>(&'a self, name: &'a str, side: Option<Side>) -> Option<&'a str> {
        let Some(role) = self.roles.get(name).or_else(|| BUILT_IN.get(name)) else { return Some(name) };
        side.and_then(|Side(team)| role.sides.get(&team))
            .or(role.unsided.as_ref())
            .map(String::as_str)
    }

    /// What a room's slot is painted with. The one resolution: drawing it and baking its light
    /// both ask this.
    pub fn paint<'a>(&'a self, surfaces: &'a RoomSurfaces, slot: RoomSlot, side: Option<Side>) -> Option<&'a str> {
        self.resolve(surfaces.of(slot)?, side)
    }
}

/// Ungated, as [`publish_movement_rules`](crate::editor::map_metadata::publish_movement_rules) is:
/// a client is handed settings at any time.
pub fn publish_palette(metadata: Res<MapMetadata>, mut palette: ResMut<Palette>) {
    if metadata.is_changed() && *palette != metadata.palette {
        *palette = metadata.palette.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_walls() -> Palette {
        let mut palette = Palette::default();
        palette.roles.insert(
            "spawn_walls".into(),
            RolePaint {
                sides: BTreeMap::from([(Team::Red, "plaster_red".into()), (Team::Blue, "plaster_blue".into())]),
                unsided: Some("plaster".into()),
            },
        );
        palette
    }

    #[test]
    fn a_role_is_painted_by_side() {
        let palette = spawn_walls();
        assert_eq!(palette.resolve("spawn_walls", Some(Side(Team::Red))), Some("plaster_red"));
        assert_eq!(palette.resolve("spawn_walls", Some(Side(Team::Blue))), Some("plaster_blue"));
    }

    /// A side the role does not name, and a room no Mirror claims, both fall back.
    #[test]
    fn a_role_falls_back_to_its_unsided_paint() {
        let palette = spawn_walls();
        assert_eq!(palette.resolve("spawn_walls", Some(Side(Team::Green))), Some("plaster"));
        assert_eq!(palette.resolve("spawn_walls", None), Some("plaster"));
    }

    /// On a map that never mentions it, and shadowed by a map role of the same name.
    #[test]
    fn every_map_has_the_team_grid() {
        let mut palette = Palette::default();
        assert_eq!(palette.resolve(TEAM_GRID, Some(Side(Team::Yellow))), Some("grid_yellow"));
        assert_eq!(palette.resolve(TEAM_GRID, None), Some("orange"));
        assert_eq!(palette.every_role().filter(|(name, _)| *name == TEAM_GRID).count(), 1);

        palette.roles.insert(TEAM_GRID.into(), RolePaint { sides: BTreeMap::new(), unsided: Some("concrete".into()) });
        assert_eq!(palette.resolve(TEAM_GRID, Some(Side(Team::Yellow))), Some("concrete"));
        assert_eq!(palette.every_role().filter(|(name, _)| *name == TEAM_GRID).count(), 1);
    }

    #[test]
    fn a_name_that_is_no_role_is_a_material() {
        let palette = spawn_walls();
        assert_eq!(palette.resolve("concrete", Some(Side(Team::Red))), Some("concrete"));
    }

    /// The palette crosses the wire inside the map's settings, which Lightyear encodes with
    /// postcard: a map keyed by an enum has to survive that, and so does a role with nothing set,
    /// which a skipped empty field would not.
    #[test]
    fn a_palette_survives_the_wire() {
        let mut palette = spawn_walls();
        palette.roles.insert("bare".into(), RolePaint::default());
        let bytes = postcard::to_stdvec(&palette).expect("encodes");
        let back: Palette = postcard::from_bytes(&bytes).expect("decodes");
        assert_eq!(back, palette);
    }
}
