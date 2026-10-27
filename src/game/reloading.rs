//! A weapon being reloaded, drawn from the replicated `Ammo` rather than an
//! event, so nothing new crosses the wire and a body seen mid-reload starts
//! the clip partway.
//!
//! The clip is fitted to the weapon's reload time. The body plays the prop's
//! `body_reload`, the viewmodel its `reload`, each falling back to the shared
//! `animations/reload.gpa`.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::weapon::{Ammo, Equipped, WeaponCatalogue};
use crate::game::held::{perform_weapon_clips, HeldModel, Performance, WeaponClip};
use crate::game::player::LocalPlayer;
use crate::game::viewmodel::Viewmodel;

/// Seconds of reload left last frame, to tell a start from a countdown.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ReloadSeen(pub f32);

pub struct ReloadingPlugin;

impl Plugin for ReloadingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            perform_reloads
                .after(crate::game::recoil::start_recoil)
                .before(perform_weapon_clips)
                .run_if(in_state(AppMode::Play)),
        );
    }
}

/// A rise of more than this is a reload beginning; otherwise it only counts down.
const STARTED: f32 = 1e-4;

pub(crate) fn perform_reloads(
    catalogue: Res<WeaponCatalogue>,
    mut bodies: Query<
        (&Equipped, &Ammo, &HeldModel, &mut Performance, &mut ReloadSeen, Has<LocalPlayer>),
        Without<Viewmodel>,
    >,
    mut views: Query<&mut Performance, With<Viewmodel>>,
) {
    for (equipped, ammo, held, mut performance, mut seen, ours) in &mut bodies {
        let left = ammo.at(equipped.held).reloading;
        let was = seen.0;
        // The reload the last shot starts waits out that shot's kick. `seen`
        // stays behind, so the rise is still seen and the reload joins partway.
        if left > was + STARTED && performance.clip().is_some_and(WeaponClip::is_kick) {
            continue;
        }
        if was != left {
            seen.0 = left;
        }
        let Some(whole) = catalogue.held_by(equipped).map(|weapon| weapon.magazine.reload) else {
            continue;
        };

        let own_or_shared = |name: &str| match held.has_clip(name) {
            true => WeaponClip::Prop(name.to_string()),
            false => WeaponClip::Reload,
        };
        let (body, view) = (own_or_shared("body_reload"), own_or_shared("reload"));

        if left > was + STARTED && whole > 0.0 {
            let from = 1.0 - left / whole;
            performance.start_fitted(body, from, whole);
            if ours {
                for mut viewmodel in &mut views {
                    viewmodel.start_fitted(view.clone(), from, whole);
                }
            }
        } else if left <= 0.0 && was > 0.0 {
            // Cut short by a shot or a switch.
            performance.stop(&body);
            if ours {
                for mut viewmodel in &mut views {
                    viewmodel.stop(&view);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::Class;
    use crate::common::weapon::{Loadout, Magazine, Mounted, Slot, SlotChoices, Weapon, WeaponId};

    const RELOAD_SECONDS: f32 = 2.0;

    /// Our body and our viewmodel, holding a weapon that reloads in
    /// [`RELOAD_SECONDS`] and ships no clips of its own.
    fn a_reloader(world: &mut World) -> (Entity, Entity) {
        let id = WeaponId::of("test");
        let mut catalogue = WeaponCatalogue::default();
        catalogue.insert(Weapon {
            id,
            name_key: "weapon.names.test".into(),
            model: None,
            primary: Mounted::EMPTY,
            secondary: Mounted::EMPTY,
            magazine: Magazine { clip: Some(4), reserve: 8, reload: RELOAD_SECONDS, reload_one_at_a_time: false },
        });
        let mut loadout = Loadout::default();
        loadout.set(Slot::Primary, SlotChoices { permitted: vec![id], default: id });
        catalogue.set_loadout(Class::Mercenary, loadout);
        let (equipped, ammo) = catalogue.starting_equipment(Class::Mercenary, None);
        world.insert_resource(catalogue);
        let body = world.spawn((LocalPlayer, equipped, ammo, HeldModel::default())).id();
        let view = world.spawn((Viewmodel::default(), Performance::default())).id();
        (body, view)
    }

    fn reloading(world: &mut World, body: Entity, left: f32) {
        world.get_mut::<Ammo>(body).unwrap().at_mut(Slot::Primary).reloading = left;
        world.run_system_once(perform_reloads).unwrap();
    }

    fn playing(world: &World, entity: Entity) -> Option<WeaponClip> {
        world.get::<Performance>(entity).unwrap().clip().cloned()
    }

    /// A reload starts both views' clips; one cut short ends them.
    #[test]
    fn a_reload_plays_while_it_lasts() {
        let mut world = World::new();
        let (body, view) = a_reloader(&mut world);

        reloading(&mut world, body, RELOAD_SECONDS);
        assert_eq!(playing(&world, body), Some(WeaponClip::Reload));
        assert_eq!(playing(&world, view), Some(WeaponClip::Reload), "our viewmodel did not reload");
        assert_eq!(world.get::<Performance>(body).unwrap().time_into(1.0), Some(0.0));

        reloading(&mut world, body, 1.5);
        assert_eq!(playing(&world, body), Some(WeaponClip::Reload), "counting down restarted nothing");

        reloading(&mut world, body, 0.0);
        assert_eq!(playing(&world, body), None, "a reload cut short left its clip playing");
        assert_eq!(playing(&world, view), None);
    }

    /// Seen halfway through, the clip starts halfway through; another shell
    /// going in starts it again.
    #[test]
    fn a_reload_seen_late_starts_partway_and_a_shell_restarts_it() {
        let mut world = World::new();
        let (body, _) = a_reloader(&mut world);

        reloading(&mut world, body, RELOAD_SECONDS / 2.0);
        let at = world.get::<Performance>(body).unwrap().time_into(1.0).unwrap();
        assert!((at - 0.5).abs() < 1e-5, "joined halfway, started at {at}");

        reloading(&mut world, body, 0.1);
        reloading(&mut world, body, RELOAD_SECONDS);
        let at = world.get::<Performance>(body).unwrap().time_into(1.0).unwrap();
        assert_eq!(at, 0.0, "the next shell did not start the clip again");
    }

    /// The reload waits out the last shot's kick, then joins partway.
    #[test]
    fn a_reload_waits_for_the_last_shots_kick() {
        let mut world = World::new();
        let (body, _) = a_reloader(&mut world);
        world.get_mut::<Performance>(body).unwrap().start(WeaponClip::Recoil);

        reloading(&mut world, body, RELOAD_SECONDS);
        assert_eq!(playing(&world, body), Some(WeaponClip::Recoil), "the reload cut the kick");

        world.get_mut::<Performance>(body).unwrap().stop(&WeaponClip::Recoil);
        reloading(&mut world, body, RELOAD_SECONDS * 0.75);
        assert_eq!(playing(&world, body), Some(WeaponClip::Reload), "the reload never started");
        let at = world.get::<Performance>(body).unwrap().time_into(1.0).unwrap();
        assert!((at - 0.25).abs() < 1e-5, "joined at {at}, not a quarter in");
    }

    /// A fitted clip takes the reload's length, however long it was authored.
    #[test]
    fn a_fitted_clip_runs_at_the_weapons_pace() {
        let mut performance = Performance::default();
        performance.start_fitted(WeaponClip::Reload, 0.25, 4.0);
        assert_eq!(performance.time_into(1.0), Some(0.25));
        assert_eq!(performance.time_into(3.0), Some(0.75), "the start is a fraction, not seconds");
    }

    /// The shipped reloads move the weapon and put it back by the end.
    #[test]
    fn the_shipped_reloads_move_the_weapon_and_bring_it_back() {
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::{humanoid, Pose};

        let packs = [std::path::PathBuf::from("assets/default")];
        let shared = crate::common::clip_file::load_named_clip(&Default::default(), &packs, "reload")
            .expect("the default pack ships no reload");
        let doc = crate::prop::document::parse(
            &std::fs::read_to_string("assets/default/props/rocket_launcher.gpp").unwrap(),
        )
        .unwrap();
        let own = doc.animations.get("reload").expect("the rocket launcher has no reload").clone();

        let rig = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Soldier));
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        for clip in [shared, own] {
            let weapon_at = |time: f32| {
                let mut weapon = Transform::IDENTITY;
                apply(&clip, time, &rig, &mut Pose::rest(), &frames, Some(&mut weapon));
                weapon
            };
            assert!(weapon_at(0.5).translation.y < -0.05, "the weapon did not come down");
            let end = weapon_at(clip.length);
            assert!(end.translation.length() < 1e-4 && end.rotation.angle_between(Quat::IDENTITY) < 1e-4);
        }
    }
}
