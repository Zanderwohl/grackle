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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleNameRefused {
    /// Another of the map's roles has it.
    Taken,
    Material,
}

/// Roles every map has without saying so, so a map with no art pass can still show whose side a
/// room is on. A map's own role of the same name shadows one.
static BUILT_IN: LazyLock<BTreeMap<String, RolePaint>> = LazyLock::new(|| {
    let per_team = |prefix: &str, unsided: &str| RolePaint {
        sides: [(Team::Red, "red"), (Team::Blue, "blue"), (Team::Yellow, "yellow"), (Team::Green, "green")]
            .into_iter()
            .map(|(team, color)| (team, format!("{prefix}{color}")))
            .collect(),
        unsided: Some(unsided.to_owned()),
    };
    BTreeMap::from([
        (TEAM_GRID.to_owned(), per_team("grid_", "orange")),
        (TEAM_BRICK.to_owned(), per_team("painted_brick_", "painted_brick_orange")),
        (TEAM_TILES.to_owned(), per_team("tiles_", "tiles_orange")),
        (
            TEAM_CLADDING.to_owned(),
            RolePaint {
                sides: BTreeMap::from([
                    (Team::Red, "wooden_fence".to_owned()),
                    (Team::Blue, "corrugated_steel".to_owned()),
                    (Team::Yellow, "stripes_yellow".to_owned()),
                    (Team::Green, "stripes_green".to_owned()),
                ]),
                unsided: Some("stripes_orange".to_owned()),
            },
        ),
    ])
});

/// The default grid in each team's color.
pub const TEAM_GRID: &str = "team_grid";
/// Painted brick in each team's color, orange where no Mirror claims the room.
pub const TEAM_BRICK: &str = "team_brick";
/// Glazed tiles in each team's color, orange where no Mirror claims the room.
pub const TEAM_TILES: &str = "team_tiles";
/// A different cladding per team: Red's wooden fence, Blue's corrugated steel, Yellow's and Green's
/// stripes, orange stripes where no Mirror claims the room.
pub const TEAM_CLADDING: &str = "team_cladding";

impl Palette {
    /// The map's roles, then the built-in ones it does not shadow.
    pub fn every_role(&self) -> impl Iterator<Item = (&String, &RolePaint)> {
        self.roles.iter().chain(BUILT_IN.iter().filter(|(name, _)| !self.roles.contains_key(*name)))
    }

    /// A material's name is refused, since the role would shadow that material on every slot
    /// naming it.
    pub fn may_name_a_role(&self, name: &str, is_material: impl Fn(&str) -> bool) -> Result<(), RoleNameRefused> {
        if self.roles.contains_key(name) {
            Err(RoleNameRefused::Taken)
        } else if is_material(name) {
            Err(RoleNameRefused::Material)
        } else {
            Ok(())
        }
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

    /// In [`RoomSlot::ALL`] order.
    pub fn paint_slots<'a>(&'a self, surfaces: Option<&'a RoomSurfaces>, side: Option<Side>) -> [Option<&'a str>; 3] {
        RoomSlot::ALL.map(|slot| surfaces.and_then(|surfaces| self.paint(surfaces, slot, side)))
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
    fn every_map_has_the_team_brick() {
        let palette = Palette::default();
        assert_eq!(palette.resolve(TEAM_BRICK, Some(Side(Team::Blue))), Some("painted_brick_blue"));
        assert_eq!(palette.resolve(TEAM_BRICK, None), Some("painted_brick_orange"));
    }

    #[test]
    fn every_map_has_the_team_tiles() {
        let palette = Palette::default();
        assert_eq!(palette.resolve(TEAM_TILES, Some(Side(Team::Yellow))), Some("tiles_yellow"));
        assert_eq!(palette.resolve(TEAM_TILES, None), Some("tiles_orange"));
    }

    #[test]
    fn every_map_has_the_team_cladding() {
        let palette = Palette::default();
        assert_eq!(palette.resolve(TEAM_CLADDING, Some(Side(Team::Red))), Some("wooden_fence"));
        assert_eq!(palette.resolve(TEAM_CLADDING, Some(Side(Team::Blue))), Some("corrugated_steel"));
        assert_eq!(palette.resolve(TEAM_CLADDING, Some(Side(Team::Green))), Some("stripes_green"));
        assert_eq!(palette.resolve(TEAM_CLADDING, None), Some("stripes_orange"));
    }

    /// Every material a built-in role names is one the default pack ships.
    #[test]
    fn every_built_in_role_paints_with_shipped_materials() {
        use crate::material::{FILE_EXTENSION, MATERIALS_DIR};
        let dir = std::path::Path::new("assets/default").join(MATERIALS_DIR);
        for (role, paint) in BUILT_IN.iter() {
            for name in paint.sides.values().chain(&paint.unsided) {
                let path = dir.join(format!("{name}.{FILE_EXTENSION}"));
                assert!(path.exists(), "{role} paints with {name}, which {} does not hold", dir.display());
            }
        }
    }

    #[test]
    fn a_role_may_not_take_a_materials_name() {
        let palette = spawn_walls();
        let is_material = |name: &str| ["plaster", "concrete"].contains(&name);
        assert_eq!(palette.may_name_a_role("plaster", is_material), Err(RoleNameRefused::Material));
        assert_eq!(palette.may_name_a_role("spawn_walls", is_material), Err(RoleNameRefused::Taken));
        assert_eq!(palette.may_name_a_role("team_plaster", is_material), Ok(()));
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
