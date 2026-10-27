//! A weapon kicking when it goes off: drawing, not simulation. See
//! `documentation/animation-layers.md`.
//!
//! Every shot the authority decides becomes an `Effect::Fired`, relayed like a
//! tracer. A client guesses its own kick from its own press and ignores its own
//! `Fired`; a shot the server refuses still kicks.
//!
//! A prop may ship `recoil`/`alt_recoil` for the body and `fire`/`alt_fire`
//! for the viewmodel; anything missing is the shared `animations/recoil.gpa`.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::effects::Effect;
use crate::common::net::{has_authority, is_remote_client, NetRole};
use crate::common::weapon::{Ammo, AmmoState, Cadence, Equipped, Slot, WeaponAction, WeaponCatalog};
use crate::game::condition::Condition;
use crate::game::held::{perform_weapon_clips, HeldModel, Performance, WeaponClip};
use crate::game::player::LocalPlayer;
use crate::game::viewmodel::Viewmodel;
use crate::game::weapon::{pull_trigger, spend, Button, WeaponActionFired};

/// A body's weapon went off, as far as this machine is concerned: from the
/// wire, from the authority's own queue, or guessed from our own press.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct ShotSeen {
    pub body: Entity,
    pub button: Button,
}

pub struct RecoilPlugin;

impl Plugin for RecoilPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ShotSeen>()
            .add_systems(
                FixedUpdate,
                announce_shots
                    .after(pull_trigger)
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(
                Update,
                (
                    (see_shots, guess_our_shots.run_if(is_remote_client)),
                    start_recoil,
                )
                    .chain()
                    .before(perform_weapon_clips)
                    .run_if(in_state(AppMode::Play)),
            );
    }
}

fn announce_shots(
    mut fired: MessageReader<WeaponActionFired>,
    shooters: Query<&PlayerId>,
    mut effects: MessageWriter<Effect>,
) {
    for shot in fired.read() {
        let Ok(by) = shooters.get(shot.shooter) else { continue };
        effects.write(Effect::Fired { by: *by, button: shot.button });
    }
}

/// A remote client skips its own: it guessed that kick from its own press.
fn see_shots(
    role: Option<Res<NetRole>>,
    mut effects: MessageReader<Effect>,
    bodies: Query<(Entity, &PlayerId, Has<LocalPlayer>)>,
    mut seen: MessageWriter<ShotSeen>,
) {
    let guessing_ours = role.is_some_and(|role| !role.is_authority());
    for effect in effects.read() {
        let Effect::Fired { by, button } = *effect else { continue };
        let Some((body, _, ours)) = bodies.iter().find(|(_, id, _)| **id == by) else { continue };
        if ours && guessing_ours {
            continue;
        }
        seen.write(ShotSeen { body, button });
    }
}

/// Our own shot, decided as `pull_trigger` would from what this machine can
/// see. Reads `ButtonInput` in `Update`, where nothing replays it.
fn guess_our_shots(
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    paused: Option<Res<crate::game::pause_menu::PauseMenu>>,
    selection: Option<Res<crate::game::weapon_select::WeaponSelection>>,
    catalog: Res<WeaponCatalog>,
    ours: Option<Single<(Entity, &Equipped, &Ammo, &Condition), With<LocalPlayer>>>,
    mut cooldown: Local<[f32; 2]>,
    // The replicated state last guessed from, and what our guesses left of it.
    mut guessed: Local<Option<(Slot, AmmoState, AmmoState)>>,
    // Timed from when the switch is seen, so it ends a little after the
    // server's.
    mut drawing: Local<(Option<(Entity, Slot)>, f32)>,
    mut seen: MessageWriter<ShotSeen>,
) {
    for left in cooldown.iter_mut() {
        *left = (*left - time.delta_secs()).max(0.0);
    }
    drawing.1 = (drawing.1 - time.delta_secs()).max(0.0);
    let Some(ours) = ours else { return };
    let (body, equipped, ammo, condition) = ours.into_inner();
    if paused.is_some_and(|menu| menu.open)
        || selection.is_some_and(|selection| selection.holds_the_buttons())
        || !condition.may_fire()
    {
        return;
    }
    let Some(weapon) = catalog.held_by(equipped) else { return };
    // A new body spawns ready; only a switch on the same body draws.
    match drawing.0 {
        Some((was_body, was_slot)) if was_body == body && was_slot != equipped.held => {
            drawing.1 = weapon.deploy;
        }
        _ => {}
    }
    drawing.0 = Some((body, equipped.held));
    if drawing.1 > 0.0 {
        return;
    }
    let told = *ammo.at(equipped.held);
    let mut state = match *guessed {
        Some((slot, was, left)) if slot == equipped.held && was == told => left,
        _ => told,
    };

    for (button, mouse) in [(Button::Primary, MouseButton::Left), (Button::Secondary, MouseButton::Right)] {
        let mounted = button.of(weapon);
        if matches!(mounted.action, WeaponAction::None) {
            continue;
        }
        let index = button as usize;
        let asked = cooldown[index] <= 0.0
            && match mounted.cadence {
                Cadence::Semi { .. } => buttons.just_pressed(mouse),
                Cadence::Automatic { .. } => buttons.pressed(mouse),
            };
        if !asked || !spend(&mut state, &weapon.magazine, mounted.cost) {
            continue;
        }
        cooldown[index] = mounted.cadence.interval();
        seen.write(ShotSeen { body, button });
    }
    *guessed = Some((equipped.held, told, state));
}

pub(crate) fn start_recoil(
    mut seen: MessageReader<ShotSeen>,
    mut bodies: Query<(&mut Performance, Option<&HeldModel>, Has<LocalPlayer>), Without<Viewmodel>>,
    mut views: Query<&mut Performance, With<Viewmodel>>,
) {
    for shot in seen.read() {
        let Ok((mut performance, held, ours)) = bodies.get_mut(shot.body) else { continue };
        let (body, view) = match shot.button {
            Button::Primary => ("recoil", "fire"),
            Button::Secondary => ("alt_recoil", "alt_fire"),
        };
        let own_or_shared = |name: &str| match held.is_some_and(|held| held.has_clip(name)) {
            true => WeaponClip::Prop(name.to_string()),
            false => WeaponClip::Recoil,
        };
        performance.start(own_or_shared(body));
        if !ours {
            continue;
        }
        for mut viewmodel in &mut views {
            viewmodel.start(own_or_shared(view));
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::Class;
    use crate::common::projectile::ProjectileSpec;
    use crate::common::weapon::{Cost, Loadout, Magazine, Mounted, Slot, SlotChoices, Weapon, WeaponId};

    /// A Mercenary holding one weapon, firing `cadence` from a clip of `clip`.
    fn holding(world: &mut World, cadence: Cadence, clip: u32) -> Entity {
        let id = WeaponId::of("test");
        let mut catalog = WeaponCatalog::default();
        catalog.insert(Weapon {
            id,
            key: "test".into(),
            name_key: "weapon.names.test".into(),
            model: None,
            bucket: 1,
            deploy: 0.0,
            primary: Mounted {
                action: WeaponAction::Projectile(ProjectileSpec::ROCKET),
                cadence,
                cost: Cost::Ammo { rounds: 1 },
            },
            secondary: Mounted::EMPTY,
            magazine: Magazine { clip: Some(4), reserve: 8, reload: 1.0, reload_one_at_a_time: false },
        });
        let mut loadout = Loadout::default();
        loadout.set(Slot::Primary, SlotChoices { permitted: vec![id], default: id });
        catalog.set_loadout(Class::Mercenary, loadout);
        let (equipped, mut ammo) = catalog.starting_equipment(Class::Mercenary, None);
        ammo.at_mut(Slot::Primary).clip = clip;
        world.insert_resource(catalog);
        world.init_resource::<Time>();
        world.init_resource::<ButtonInput<MouseButton>>();
        world.init_resource::<Messages<ShotSeen>>();
        world.spawn((LocalPlayer, equipped, ammo, Condition::default())).id()
    }

    fn seen(world: &mut World) -> Vec<ShotSeen> {
        let messages = world.resource::<Messages<ShotSeen>>();
        let shots = messages.get_cursor().read(messages).copied().collect();
        world.resource_mut::<Messages<ShotSeen>>().clear();
        shots
    }

    fn press(world: &mut World) {
        let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
        buttons.release(MouseButton::Left);
        buttons.clear();
        buttons.press(MouseButton::Left);
    }

    /// Guessed the way the server would decide it: a press with a round in
    /// the clip kicks, an empty clip or a humiliated body does not.
    #[test]
    fn our_own_shot_is_guessed_from_what_we_can_see() {
        let mut world = World::new();
        let body = holding(&mut world, Cadence::Semi { interval: 0.0 }, 2);
        press(&mut world);
        world.run_system_once(guess_our_shots).unwrap();
        assert_eq!(seen(&mut world), vec![ShotSeen { body, button: Button::Primary }]);

        // Still held: semi-automatic asks for a fresh press.
        world.resource_mut::<ButtonInput<MouseButton>>().clear();
        world.run_system_once(guess_our_shots).unwrap();
        assert!(seen(&mut world).is_empty(), "holding the button kicked twice");

        world.get_mut::<Ammo>(body).unwrap().at_mut(Slot::Primary).clip = 0;
        press(&mut world);
        world.run_system_once(guess_our_shots).unwrap();
        assert!(seen(&mut world).is_empty(), "an empty clip kicked");

        world.get_mut::<Ammo>(body).unwrap().at_mut(Slot::Primary).clip = 2;
        *world.get_mut::<Condition>(body).unwrap() = Condition::HUMILIATED;
        press(&mut world);
        world.run_system_once(guess_our_shots).unwrap();
        assert!(seen(&mut world).is_empty(), "a humiliated body kicked");
    }

    /// Held down, an automatic weapon kicks on its own interval, not per frame.
    #[test]
    fn an_automatic_weapon_is_guessed_on_its_interval() {
        let mut world = World::new();
        holding(&mut world, Cadence::Automatic { interval: 0.1 }, 4);
        world.resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);

        // One system across frames, as in the game: the cooldown is its own.
        let guess = world.register_system(guess_our_shots);
        let mut kicks = 0;
        for _ in 0..10 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.03));
            world.run_system(guess).unwrap();
            kicks += seen(&mut world).len();
        }
        // 0.3 s at one every 0.1 s, the first at once.
        assert_eq!(kicks, 3, "{kicks} kicks in 0.3 s");
    }

    /// With one round left and the server's count not yet down, holding the
    /// trigger kicks once; a new count from the server is believed again.
    #[test]
    fn a_guessed_shot_spends_its_round_until_the_server_says() {
        let mut world = World::new();
        let body = holding(&mut world, Cadence::Automatic { interval: 0.1 }, 1);
        world.resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
        let guess = world.register_system(guess_our_shots);
        let mut kicks = 0;
        for _ in 0..10 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.03));
            world.run_system(guess).unwrap();
            kicks += seen(&mut world).len();
        }
        assert_eq!(kicks, 1, "one round kicked {kicks} times");

        world.get_mut::<Ammo>(body).unwrap().at_mut(Slot::Primary).clip = 3;
        world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.2));
        world.run_system(guess).unwrap();
        assert_eq!(seen(&mut world).len(), 1, "a reloaded clip from the server was not believed");
    }

    /// Every shot is seen from its effect, except a client's own.
    #[test]
    fn a_shot_is_announced_and_seen_except_our_own_on_a_client() {
        let mut world = World::new();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Effect>>();
        world.init_resource::<Messages<ShotSeen>>();
        let ours = world.spawn((LocalPlayer, PlayerId(1))).id();
        let theirs = world.spawn(PlayerId(2)).id();

        for shooter in [ours, theirs] {
            world.write_message(WeaponActionFired {
                shooter,
                button: Button::Primary,
                action: WeaponAction::None,
            });
        }
        world.run_system_once(announce_shots).unwrap();
        let messages = world.resource::<Messages<Effect>>();
        let effects: Vec<Effect> = messages.get_cursor().read(messages).copied().collect();
        assert_eq!(effects.len(), 2);
        assert!(matches!(effects[0], Effect::Fired { by: PlayerId(1), button: Button::Primary }));

        world.insert_resource(NetRole::Solo);
        world.run_system_once(see_shots).unwrap();
        let bodies: Vec<Entity> = seen(&mut world).iter().map(|shot| shot.body).collect();
        assert_eq!(bodies, vec![ours, theirs], "the authority should see every shot from the effect");

        world.insert_resource(NetRole::Client { host: "here".into(), port: 1 });
        let mut fresh = world.resource::<Messages<Effect>>().get_cursor();
        fresh.clear(world.resource::<Messages<Effect>>());
        for by in [PlayerId(1), PlayerId(2)] {
            world.write_message(Effect::Fired { by, button: Button::Primary });
        }
        world.run_system_once(see_shots).unwrap();
        let bodies: Vec<Entity> = seen(&mut world).iter().map(|shot| shot.body).collect();
        assert!(bodies.contains(&theirs));
        assert!(!bodies.contains(&ours), "a client kicked twice for its own shot");
    }

    /// Each view plays the prop's own kick where it ships one, else the shared.
    #[test]
    fn each_view_plays_the_weapons_own_kick_or_the_shared_one() {
        let shoot = |held: HeldModel| {
            let mut world = World::new();
            world.init_resource::<Messages<ShotSeen>>();
            let body = world.spawn((LocalPlayer, Performance::default(), held)).id();
            let view = world.spawn((Viewmodel::default(), Performance::default())).id();
            world.write_message(ShotSeen { body, button: Button::Primary });
            world.run_system_once(start_recoil).unwrap();
            let clip = |entity| world.get::<Performance>(entity).unwrap().clip().cloned();
            (clip(body), clip(view))
        };

        assert_eq!(
            shoot(HeldModel::with_clips(&["recoil", "fire"])),
            (Some(WeaponClip::Prop("recoil".into())), Some(WeaponClip::Prop("fire".into()))),
        );
        assert_eq!(
            shoot(HeldModel::default()),
            (Some(WeaponClip::Recoil), Some(WeaponClip::Recoil)),
            "a weapon with no kicks of its own should fall back to the shared one",
        );
        // Each view falls back on its own.
        assert_eq!(
            shoot(HeldModel::with_clips(&["fire"])),
            (Some(WeaponClip::Recoil), Some(WeaponClip::Prop("fire".into()))),
        );
    }

    /// Somebody else's shot kicks their body and leaves our viewmodel alone.
    #[test]
    fn somebody_elses_shot_does_not_kick_our_viewmodel() {
        let mut world = World::new();
        world.init_resource::<Messages<ShotSeen>>();
        let other = world.spawn((Performance::default(), HeldModel::with_clips(&["recoil"]))).id();
        let view = world.spawn((Viewmodel::default(), Performance::default())).id();
        world.write_message(ShotSeen { body: other, button: Button::Primary });
        world.run_system_once(start_recoil).unwrap();
        assert_eq!(world.get::<Performance>(other).unwrap().clip(), Some(&WeaponClip::Prop("recoil".into())));
        assert_eq!(world.get::<Performance>(view).unwrap().clip(), None);
    }

    /// The shipped kicks move the weapon and put it back by the end, so a
    /// fade-out starts from where the weapon rests.
    #[test]
    fn the_shipped_kicks_kick_and_come_home() {
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::{humanoid, Pose};

        let packs = [std::path::PathBuf::from("assets/default")];
        let recoil = crate::common::clip_file::load_named_clip(&Default::default(), &packs, "recoil")
            .expect("the default pack ships no recoil");
        let doc = crate::prop::document::parse(
            &std::fs::read_to_string("assets/default/props/rocket_launcher.gpp").unwrap(),
        )
        .unwrap();
        let fire = doc.animations.get("fire").expect("the rocket launcher has no fire clip").clone();
        let own = doc.animations.get("recoil").expect("the rocket launcher has no recoil clip").clone();

        let rig = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Soldier));
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        for clip in [recoil, fire, own] {
            let weapon_at = |time: f32| {
                let mut weapon = Transform::IDENTITY;
                apply(&clip, time, &rig, &mut Pose::rest(), &frames, Some(&mut weapon));
                weapon
            };
            let peak = weapon_at(0.05);
            assert!(peak.translation.z > 0.03, "{} did not kick back: {}", clip.name_key, peak.translation);
            assert!(peak.rotation.angle_between(Quat::IDENTITY) > 0.1, "{} did not lift", clip.name_key);
            let end = weapon_at(clip.length);
            assert!(end.translation.length() < 1e-4 && end.rotation.angle_between(Quat::IDENTITY) < 1e-4);
        }
    }
}

