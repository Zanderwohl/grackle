//! The Rounds tab's Show Kills: the selected round file's kills drawn over the map, a red
//! capsule where the killer stood and a blue one where the victim stood, joined by a line.
//! Selecting a row of the kills table makes that kill opaque and fades the rest.
//!
//! Everything spawned here carries [`KillHistoryMannequin`], so a change of file, unchecking the
//! box or entering Play clears it with one query. Editor viewports only.

use std::path::PathBuf;

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::{CLASS_RADIUS, TALLEST_CLASS_HEIGHT};
use crate::editor::multicam;
use crate::editor::rounds_panel::{RoundArchive, Stamp};
use crate::game::round_file::KillPlace;

const KILLER_COLOR: Srgba = Srgba::rgb(0.9, 0.15, 0.15);
const VICTIM_COLOR: Srgba = Srgba::rgb(0.2, 0.4, 0.95);
const LINE_COLOR: Srgba = Srgba::rgb(0.5, 0.18, 0.18);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct KillHistoryMannequin {
    /// The row of the kills table, by its unsorted index.
    pub kill: usize,
    pub party: Party,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Party {
    Killer,
    Victim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emphasis {
    Resting,
    Selected,
    Faded,
}

impl Emphasis {
    const ALL: [Emphasis; 3] = [Emphasis::Resting, Emphasis::Selected, Emphasis::Faded];

    fn of(kill: usize, selected: Option<usize>) -> Self {
        match selected {
            None => Emphasis::Resting,
            Some(selected) if selected == kill => Emphasis::Selected,
            Some(_) => Emphasis::Faded,
        }
    }

    fn alpha(self) -> f32 {
        match self {
            Emphasis::Resting => 0.4,
            Emphasis::Selected => 1.0,
            Emphasis::Faded => 0.2,
        }
    }
}

/// The file and stamp drawn now; the places are kept for the lines.
#[derive(Resource, Default)]
struct ShownKills(Option<(PathBuf, Stamp, Vec<KillPlace>)>);

#[derive(Resource)]
struct KillHistoryLook {
    capsule: Handle<Mesh>,
    /// Per party, indexed by [`Emphasis`].
    killer: [Handle<StandardMaterial>; 3],
    victim: [Handle<StandardMaterial>; 3],
}

impl KillHistoryLook {
    fn material(&self, party: Party, emphasis: Emphasis) -> &Handle<StandardMaterial> {
        let set = match party {
            Party::Killer => &self.killer,
            Party::Victim => &self.victim,
        };
        &set[emphasis as usize]
    }
}

impl FromWorld for KillHistoryLook {
    fn from_world(world: &mut World) -> Self {
        let capsule = world.resource_mut::<Assets<Mesh>>().add(Capsule3d::new(CLASS_RADIUS, TALLEST_CLASS_HEIGHT - 2.0 * CLASS_RADIUS));
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        let mut set = |color: Srgba| {
            Emphasis::ALL.map(|emphasis| {
                materials.add(StandardMaterial {
                    base_color: color.with_alpha(emphasis.alpha()).into(),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                })
            })
        };
        let killer = set(KILLER_COLOR);
        let victim = set(VICTIM_COLOR);
        KillHistoryLook { capsule, killer, victim }
    }
}

pub struct KillHistoryPlugin;

impl Plugin for KillHistoryPlugin {
    fn build(&self, app: &mut App) {
        add_mannequins(app);
        app.add_systems(Update, draw_kill_lines.after(highlight_kills).run_if(in_state(AppMode::Editor)));
    }
}

/// Everything but the lines, which need gizmos.
fn add_mannequins(app: &mut App) {
    app.init_resource::<ShownKills>()
        .init_resource::<KillHistoryLook>()
        .add_systems(OnEnter(AppMode::Play), clear_kills)
        .add_systems(Update, (show_kills, highlight_kills).chain().run_if(in_state(AppMode::Editor)));
}

fn clear_kills(mut commands: Commands, mut shown: ResMut<ShownKills>, mannequins: Query<Entity, With<KillHistoryMannequin>>) {
    despawn_all(&mut commands, &mannequins);
    shown.0 = None;
}

fn despawn_all(commands: &mut Commands, mannequins: &Query<Entity, With<KillHistoryMannequin>>) {
    for entity in mannequins {
        commands.entity(entity).despawn();
    }
}

fn center(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * TALLEST_CLASS_HEIGHT * 0.5
}

fn show_kills(
    mut commands: Commands,
    archive: Res<RoundArchive>,
    mut shown: ResMut<ShownKills>,
    look: Res<KillHistoryLook>,
    mannequins: Query<Entity, With<KillHistoryMannequin>>,
) {
    let wanted = archive.shown_kills();
    let unchanged = match (wanted, &shown.0) {
        (Some((path, stamp, _)), Some((shown_path, shown_stamp, _))) => path == shown_path && stamp == *shown_stamp,
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return;
    }
    despawn_all(&mut commands, &mannequins);
    shown.0 = wanted.map(|(path, stamp, places)| (path.clone(), stamp, places.to_vec()));
    let Some((_, _, places)) = wanted else { return };
    // A kill with no victim's place would be a lone killer; the Rounds tab counts it instead.
    for (kill, place) in places.iter().enumerate().filter(|(_, place)| place.victim.is_some()) {
        for (party, feet) in [(Party::Killer, place.killer), (Party::Victim, place.victim)] {
            let Some(feet) = feet else { continue };
            commands.spawn((
                KillHistoryMannequin { kill, party },
                Name::new(format!("Kill {kill} {party:?}")),
                Mesh3d(look.capsule.clone()),
                MeshMaterial3d(look.material(party, Emphasis::Resting).clone()),
                Transform::from_translation(center(feet)),
                NotShadowCaster,
                multicam::editor_only(),
            ));
        }
    }
}

fn highlight_kills(
    archive: Res<RoundArchive>,
    look: Res<KillHistoryLook>,
    mut mannequins: Query<(&KillHistoryMannequin, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    for (mannequin, mut material) in &mut mannequins {
        let wanted = look.material(mannequin.party, Emphasis::of(mannequin.kill, archive.selected_kill));
        if material.0.id() != wanted.id() {
            material.0 = wanted.clone();
        }
    }
}

fn draw_kill_lines(archive: Res<RoundArchive>, shown: Res<ShownKills>, mut gizmos: Gizmos) {
    let Some((_, _, places)) = &shown.0 else { return };
    for (kill, place) in places.iter().enumerate() {
        let (Some(killer), Some(victim)) = (place.killer, place.victim) else { continue };
        let alpha = Emphasis::of(kill, archive.selected_kill).alpha();
        gizmos.line(center(killer), center(victim), LINE_COLOR.with_alpha(alpha));
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use bevy::state::app::StatesPlugin;

    use super::*;
    use crate::game::round_file::RoundTables;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .init_state::<AppMode>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>();
        let places = vec![
            KillPlace { victim: Some(Vec3::ZERO), killer: Some(Vec3::X * 4.0) },
            KillPlace { victim: Some(Vec3::Z), killer: None },
            KillPlace { victim: None, killer: Some(Vec3::X) },
        ];
        let tables = RoundTables { kill_places: places, ..default() };
        let mut archive = RoundArchive::with_file(PathBuf::from("round.jsonl"), (SystemTime::UNIX_EPOCH, 1), tables);
        archive.show_kills = true;
        app.insert_resource(archive);
        add_mannequins(&mut app);
        app.update();
        app
    }

    fn mannequins(app: &mut App) -> Vec<KillHistoryMannequin> {
        let mut found: Vec<KillHistoryMannequin> = app.world_mut().query::<&KillHistoryMannequin>().iter(app.world()).copied().collect();
        found.sort_by_key(|mannequin| (mannequin.kill, mannequin.party == Party::Victim));
        found
    }

    /// Breaks if a kill without a victim's place is drawn, or a party without a place is.
    #[test]
    fn each_placed_kill_stands_its_parties() {
        let mut app = app();
        let expected = vec![
            KillHistoryMannequin { kill: 0, party: Party::Killer },
            KillHistoryMannequin { kill: 0, party: Party::Victim },
            KillHistoryMannequin { kill: 1, party: Party::Victim },
        ];
        assert_eq!(mannequins(&mut app), expected);
    }

    /// Breaks if unchecking, choosing no file or entering Play leaves a mannequin standing.
    #[test]
    fn mannequins_go_when_the_kills_are_not_shown() {
        let mut app = app();
        app.world_mut().resource_mut::<RoundArchive>().show_kills = false;
        app.update();
        assert!(mannequins(&mut app).is_empty(), "unchecking left mannequins");

        let mut app = self::app();
        app.world_mut().resource_mut::<RoundArchive>().select_nothing();
        app.update();
        assert!(mannequins(&mut app).is_empty(), "choosing another round left mannequins");

        let mut app = self::app();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        assert!(mannequins(&mut app).is_empty(), "entering Play left mannequins");
    }

    /// Breaks if selecting a kill does not make it opaque and fade the rest.
    #[test]
    fn selecting_a_kill_fades_the_others() {
        let mut app = app();
        app.world_mut().resource_mut::<RoundArchive>().selected_kill = Some(1);
        app.update();
        let look_ids: Vec<(KillHistoryMannequin, AssetId<StandardMaterial>)> = app
            .world_mut()
            .query::<(&KillHistoryMannequin, &MeshMaterial3d<StandardMaterial>)>()
            .iter(app.world())
            .map(|(mannequin, material)| (*mannequin, material.0.id()))
            .collect();
        let look = app.world().resource::<KillHistoryLook>();
        for (mannequin, id) in look_ids {
            let wanted = if mannequin.kill == 1 { Emphasis::Selected } else { Emphasis::Faded };
            assert_eq!(id, look.material(mannequin.party, wanted).id(), "{mannequin:?}");
        }
    }
}
