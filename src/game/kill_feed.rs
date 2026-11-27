//! Recent deaths, at most [`LINES`], at the top right of the [`WindowSafeZone`], oldest at the top: each a
//! [`Plate`] in the killer's color reading "killer [icon] victim" or "killer + assist [icon]
//! victim", the icon being what killed ([`kill_icon::icon_for`]). A body that killed itself or was
//! killed by the map with somebody's help reads "assist finished off [icon] victim", and without
//! help "[icon] victim died", or "[icon] victim exploded" for the explode key, all in the neutral
//! cream. Every key may be an array in the lang file; [`Died::rand`], drawn by the authority, picks
//! the entry, so every machine on one language reads the same line.
//!
//! A line naming us as killer, assist or victim is up for [`OUR_LIFETIME`] and any other for
//! [`LIFETIME`]. The latest [`OURS_SHOWN`] of ours are always kept, and the rest of the slots go to
//! the latest others, all in the order they happened, so a line of ours outlasts the others around
//! it rather than being bumped by them. A line once bumped is gone, and never comes back when a
//! slot frees up.
//!
//! Built from `Died`, which every machine gets, so a spectator and every mode see it too. Names and
//! colors are taken as the death arrives, from the last standing seen for each id, so somebody who
//! has just left for Spectate or disconnected is still named and colored. Only deaths of a body
//! with a `PlayerId` are listed, so a crate breaking is not. A team switch kills the body at its
//! own hand and is listed as a death, as the scoreboard counts it.

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::{DamageSource, Died, How, PlayerId};
use crate::common::fonts::DisplayFont;
use crate::common::lang::fill_uncached_random;
use crate::common::swatch::Swatch;
use crate::game::hud::WindowSafeZone;
use crate::common::weapon::WeaponCatalog;
use crate::game::kill_cam::our_id;
use crate::game::kill_icon::{self, KillIcons};
use crate::game::player::{DeathCam, LocalPlayer, OurLastBody};
use crate::game::respawn::OurIdentity;
use crate::game::plate::{padding_with_room, plate_node, plate_text, Plate, PlateIcon};
use crate::game::score::Standing;
use crate::get;

pub const LINES: usize = 5;
/// Seconds a line not naming us stays up.
pub const LIFETIME: f32 = 10.0;
/// Seconds a line naming us stays up.
pub const OUR_LIFETIME: f32 = 20.0;
/// How many lines naming us are kept, at most.
pub const OURS_SHOWN: usize = 2;
const TEXT: f32 = 24.0;
/// Logical pixels on a side.
const ICON: f32 = 28.0;
/// Logical pixels between the icon and the words either side of it.
const ICON_GAP: f32 = 8.0;
/// Stands for the icon while a line's words are filled in, and is where they are split.
const ICON_MARK: char = '\u{E000}';
/// Logical pixels between a line's text and its panel's stroke, above and below.
const ROOM: f32 = 4.0;
/// Logical pixels from one plate's drop shadow to the next panel's stroke. A plate's box runs
/// [`STROKE_OVERHANG`] past its stroke at the top and its shadow at the bottom.
const GAP: f32 = 3.0;
/// Half the panel stroke, which is centered on an outline inset by the whole stroke.
const STROKE_OVERHANG: f32 = crate::game::hud::ZONE_STROKE * 0.5;

pub struct KillFeedPlugin;

impl Plugin for KillFeedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KillFeed>()
            .add_plugins(kill_icon::KillIconPlugin)
            .add_systems(OnEnter(AppMode::Play), forget_the_feed)
            .add_systems(Update, (list_the_dead, show_the_feed).chain().run_if(in_state(AppMode::Play)));
    }
}

/// The lines in the feed, oldest first.
#[derive(Resource, Default)]
pub struct KillFeed {
    lines: VecDeque<FeedLine>,
    /// The next line's number.
    next: u64,
    /// Each player's name and colors as their standing last stood.
    known: HashMap<PlayerId, (String, Swatch)>,
}

#[derive(Clone, Debug, PartialEq)]
struct Words {
    before: String,
    icon: String,
    after: String,
}

impl Words {
    /// A translation without the `{ icon }` slot puts the icon last. `seed` picks from an array.
    fn fill(key: &str, seed: u64, names: &[(&str, &str)], icon: &str) -> Self {
        let names: Vec<(&str, String)> = names.iter().map(|(slot, name)| (*slot, name.replace(ICON_MARK, ""))).collect();
        let mark = ICON_MARK.to_string();
        let mut pairs: Vec<(&str, &str)> = names.iter().map(|(slot, name)| (*slot, name.as_str())).collect();
        pairs.push(("icon", &mark));
        let filled = fill_uncached_random(key, seed, &pairs);
        let (before, after) = filled.split_once(ICON_MARK).unwrap_or((&filled, ""));
        // A second slot would draw the mark as a missing glyph.
        Words { before: before.trim_end().to_owned(), icon: icon.to_owned(), after: after.replace(ICON_MARK, "").trim_start().to_owned() }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct FeedLine {
    number: u64,
    words: Words,
    swatch: Swatch,
    /// Whether it names us.
    ours: bool,
    /// Seconds up.
    age: f32,
}

impl FeedLine {
    fn lifetime(&self) -> f32 {
        if self.ours { OUR_LIFETIME } else { LIFETIME }
    }
}

impl KillFeed {
    fn push(&mut self, words: Words, swatch: Swatch, ours: bool) {
        self.lines.push_back(FeedLine { number: self.next, words, swatch, ours, age: 0.0 });
        self.next += 1;
        self.bump();
    }

    /// Keeps the latest [`OURS_SHOWN`] of ours and as many of the latest others as fit beside them,
    /// by dropping the oldest of each.
    fn bump(&mut self) {
        let ours = self.lines.iter().filter(|line| line.ours).count();
        let others = self.lines.len() - ours;
        let kept_ours = ours.min(OURS_SHOWN);
        let (mut drop_ours, mut drop_others) = (ours - kept_ours, others.saturating_sub(LINES - kept_ours));
        self.lines.retain(|line| {
            let drop = if line.ours { &mut drop_ours } else { &mut drop_others };
            let kept = *drop == 0;
            *drop = drop.saturating_sub(1);
            kept
        });
    }

    fn learn(&mut self, standing: &Standing) {
        self.known.insert(standing.id, (standing.display_name(), standing.hud_swatch()));
    }

    /// Dropping a line never frees a slot for one already bumped, since that one is gone.
    fn age(&mut self, seconds: f32) {
        for line in &mut self.lines {
            line.age += seconds;
        }
        self.lines.retain(|line| line.age < line.lifetime());
    }
}

/// The column the plates stand in. Goes when the HUD does: it is the window safe zone's child.
#[derive(Component)]
struct FeedColumn;

/// Which [`FeedLine`] a plate shows.
#[derive(Component)]
struct ShowsLine(u64);

pub(crate) fn forget_the_feed(mut feed: ResMut<KillFeed>) {
    feed.lines.clear();
    feed.known.clear();
}

/// What a death reads as, and the color it is painted in; `None` for something that was nobody.
/// `seed` picks the wording where the lang file offers several.
fn line_for(died: &Died, known: &HashMap<PlayerId, (String, Swatch)>, catalog: Option<&WeaponCatalog>, seed: u64) -> Option<(Words, Swatch)> {
    let icon = kill_icon::icon_for(&died.cause, catalog);
    let victim_id = died.victim_id?;
    let name = |id: PlayerId| known.get(&id).map_or_else(|| get!("scoreboard.player", "id", id.0), |(name, _)| name.clone());
    let victim = name(victim_id);
    let helper = match died.assist {
        Some(DamageSource::Player(helper)) if helper != victim_id && died.killer != DamageSource::Player(helper) => Some(helper),
        _ => None,
    };

    Some(match (died.killer, helper) {
        (DamageSource::Player(killer), helper) if killer != victim_id => {
            let swatch = known.get(&killer).map_or_else(Swatch::neutral, |(_, swatch)| *swatch);
            let words = match helper {
                Some(helper) => Words::fill("kill_feed.killed_with_help", seed, &[("killer", &name(killer)), ("assist", &name(helper)), ("victim", &victim)], icon),
                None => Words::fill("kill_feed.killed", seed, &[("killer", &name(killer)), ("victim", &victim)], icon),
            };
            (words, swatch)
        }
        (_, Some(helper)) => (Words::fill("kill_feed.finished_off", seed, &[("assist", &name(helper)), ("victim", &victim)], icon), Swatch::neutral()),
        (_, None) => {
            let key = if died.cause.how == How::Exploded { "kill_feed.exploded" } else { "kill_feed.died" };
            (Words::fill(key, seed, &[("victim", &victim)], icon), Swatch::neutral())
        }
    })
}

/// Whether a death names `us` as victim, killer or assist.
fn names(died: &Died, us: Option<PlayerId>) -> bool {
    let Some(us) = us else { return false };
    died.victim_id == Some(us) || died.killer == DamageSource::Player(us) || died.assist == Some(DamageSource::Player(us))
}

#[allow(clippy::too_many_arguments)]
/// Ages the lines and adds the new deaths. The feed reads as changed only when a line comes or goes.
fn list_the_dead(
    time: Res<Time>,
    mut deaths: MessageReader<Died>,
    standings: Query<&Standing, Changed<Standing>>,
    identity: Option<Res<OurIdentity>>,
    body: Option<Single<&PlayerId, With<LocalPlayer>>>,
    last: Option<Res<OurLastBody>>,
    death_cam: Option<Res<DeathCam>>,
    catalog: Option<Res<WeaponCatalog>>,
    mut feed: ResMut<KillFeed>,
) {
    let quiet = feed.bypass_change_detection();
    for standing in &standings {
        quiet.learn(standing);
    }
    let before = quiet.lines.len();
    quiet.age(time.delta_secs());
    if quiet.lines.len() != before {
        feed.set_changed();
    }
    // A client spectating since it joined has no body to know itself by, and no line is its own.
    let us = our_id(body.map(|body| **body), identity.as_deref(), last.as_deref(), death_cam.as_deref());
    for died in deaths.read() {
        if let Some((words, swatch)) = line_for(died, &feed.known, catalog.as_deref(), died.rand.into()) {
            feed.push(words, swatch, names(died, us));
        }
    }
}

/// Puts the column up once there is a safe zone, takes down the plates of lines bumped off and
/// adds a plate at the bottom for each new one.
#[allow(clippy::too_many_arguments)]
fn show_the_feed(
    mut commands: Commands,
    font: Res<DisplayFont>,
    feed: Res<KillFeed>,
    mut icons: ResMut<KillIcons>,
    pack: Option<Res<crate::common::assets::Assets>>,
    mut images: Option<ResMut<Assets<Image>>>,
    zone: Option<Single<Entity, With<WindowSafeZone>>>,
    column: Option<Single<(Entity, Option<&Children>, Ref<FeedColumn>), With<FeedColumn>>>,
    plates: Query<&ShowsLine>,
) {
    let Some(zone) = zone else { return };
    let Some((column, children, made)) = column.map(Single::into_inner) else {
        commands.entity(*zone).with_child((
            FeedColumn,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                right: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                ..default()
            },
            Pickable::IGNORE,
            Name::new("Kill feed"),
        ));
        return;
    };
    if !feed.is_changed() && !made.is_added() {
        return;
    }

    let mut shown = Vec::new();
    for &child in children.into_iter().flatten() {
        let Ok(ShowsLine(number)) = plates.get(child) else { continue };
        if feed.lines.iter().any(|line| line.number == *number) {
            shown.push(*number);
        } else {
            commands.entity(child).despawn();
        }
    }
    for line in feed.lines.iter().filter(|line| !shown.contains(&line.number)) {
        let Words { before, icon, after } = &line.words;
        let image = match (pack.as_deref(), images.as_deref_mut()) {
            (Some(pack), Some(images)) => icons.get(icon, |relative| pack.read_from_packs(relative), images),
            _ => None,
        };
        let mut plate = commands.spawn((
            ShowsLine(line.number),
            Plate::new(line.swatch),
            Node {
                padding: padding_with_room(ROOM),
                // Gaps cannot be negative, so the plates overlap their empty edges by margin.
                margin: UiRect::top(Val::Px(GAP - 2.0 * STROKE_OVERHANG)),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(ICON_GAP),
                ..plate_node()
            },
            Name::new("Kill feed line"),
        ));
        plate.with_children(|plate| {
            if !before.is_empty() {
                plate.spawn(plate_text(&font.0, before.clone(), TEXT));
            }
            // Without an image, the node holds the icon's place.
            let mut slot = plate.spawn((PlateIcon, Node { width: Val::Px(ICON), height: Val::Px(ICON), ..default() }, Pickable::IGNORE));
            if let Some(image) = image {
                slot.insert(ImageNode::new(image));
            }
            if !after.is_empty() {
                plate.spawn(plate_text(&font.0, after.clone(), TEXT));
            }
        });
        let plate = plate.id();
        commands.entity(column).add_child(plate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::player_color::PlayerColor;
    use crate::common::team::Team;
    use crate::common::damage::{Cause, How};
    use crate::common::weapon::WeaponId;
    use crate::game::kill_icon::{FLAME, SKULL};
    use bevy::ecs::system::RunSystemOnce;

    impl From<&str> for Words {
        fn from(words: &str) -> Self {
            Words { before: words.into(), icon: SKULL.into(), after: String::new() }
        }
    }

    impl From<String> for Words {
        fn from(words: String) -> Self {
            words.as_str().into()
        }
    }

    fn standing(id: u64, name: &str, team: Option<Team>) -> Standing {
        Standing {
            id: PlayerId(id),
            name: name.into(),
            team,
            color: PlayerColor { hue: 40.0 },
            frags: 0,
            kills: 0,
            assists: 0,
            deaths: 0,
            damage_dealt: 0,
            damage_taken: 0,
            healing: 0,
        }
    }

    fn died(victim: u64, killer: DamageSource, assist: Option<DamageSource>) -> Died {
        Died { victim: Entity::PLACEHOLDER, victim_name: None, victim_id: Some(PlayerId(victim)), killer, assist, at: Vec3::ZERO, cause: Default::default(), killer_health: None, rand: 0 }
    }

    fn lines(deaths: &[Died]) -> Vec<Option<(Words, Swatch)>> {
        let mut feed = KillFeed::default();
        feed.learn(&standing(1, "Ana", Some(Team::Red)));
        feed.learn(&standing(2, "Bo", Some(Team::Blue)));
        feed.learn(&standing(3, "Cy", None));
        deaths.iter().map(|died| line_for(died, &feed.known, None, 0)).collect()
    }

    /// Breaks if somebody whose standing went before their death was read (gone to Spectate,
    /// disconnected) loses their name or color, or a line is listed twice.
    #[test]
    fn somebody_who_just_left_keeps_their_name_and_color() {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<KillFeed>();
        world.init_resource::<Messages<Died>>();
        let ana = world.spawn(standing(1, "Ana", Some(Team::Red))).id();
        world.spawn(standing(2, "Bo", Some(Team::Blue)));
        world.run_system_once(list_the_dead).unwrap();
        world.despawn(ana);
        world.write_message(died(2, DamageSource::Player(PlayerId(1)), None));
        world.run_system_once(list_the_dead).unwrap();

        let lines: Vec<(Words, Swatch)> = world.resource::<KillFeed>().lines.iter().map(|line| (line.words.clone(), line.swatch)).collect();
        let killed = Words::fill("kill_feed.killed", 0, &[("killer", "Ana"), ("victim", "Bo")], SKULL);
        assert_eq!(lines, [(killed, Swatch::team(Team::Red))]);
    }

    /// Breaks if a kill, a kill with help, a finish, or a death alone reads wrong or takes the
    /// wrong color.
    #[test]
    fn each_kind_of_death_reads_and_is_colored_as_it_should() {
        let player = |id| DamageSource::Player(PlayerId(id));
        let got = lines(&[
            died(2, player(1), None),
            died(2, player(1), Some(player(3))),
            died(2, player(2), Some(player(1))),
            died(2, DamageSource::World, Some(player(1))),
            died(2, DamageSource::World, None),
            died(1, player(3), Some(player(3))),
            died(3, player(9), None),
        ]);
        let fill = |key: &str, pairs: &[(&str, &str)]| Words::fill(key, 0, pairs, SKULL);
        let red = Swatch::team(Team::Red);
        assert_eq!(got[0], Some((fill("kill_feed.killed", &[("killer", "Ana"), ("victim", "Bo")]), red)));
        assert_eq!(got[1], Some((fill("kill_feed.killed_with_help", &[("killer", "Ana"), ("assist", "Cy"), ("victim", "Bo")]), red)));
        assert_eq!(got[2], Some((fill("kill_feed.finished_off", &[("assist", "Ana"), ("victim", "Bo")]), Swatch::neutral())));
        assert_eq!(got[3], got[2], "the map's kill with help is not a finish");
        assert_eq!(got[4], Some((fill("kill_feed.died", &[("victim", "Bo")]), Swatch::neutral())));
        let teamless = Swatch::player(PlayerColor { hue: 40.0 });
        assert_eq!(got[5], Some((fill("kill_feed.killed", &[("killer", "Cy"), ("victim", "Ana")]), teamless)), "the killer counted as their own assist");
        let nobody = get!("scoreboard.player", "id", 9);
        assert_eq!(got[6], Some((fill("kill_feed.killed", &[("killer", &nobody), ("victim", "Cy")]), Swatch::neutral())));
        let killed = &got[0].as_ref().unwrap().0;
        assert_eq!((killed.before.as_str(), killed.icon.as_str(), killed.after.as_str()), ("Ana", SKULL, "Bo"), "split wrong, or a missing lang key");
    }

    /// Breaks if an explode-key death does not read from `exploded`, or the seed does not vary the
    /// wording.
    #[test]
    fn a_blameless_death_reads_by_how_it_happened_and_its_seed() {
        use crate::game::kill_icon::EXPLOSION;
        // en-US: another test may have a translation with one wording loaded.
        let _guard = crate::common::lang::switching_language();
        let mut feed = KillFeed::default();
        feed.learn(&standing(2, "Bo", Some(Team::Blue)));
        let own = DamageSource::Player(PlayerId(2));
        let exploded = Died { cause: Cause::of(None, How::Exploded), ..died(2, own, None) };
        let suicide = Died { cause: Cause::of(None, How::Kill), ..died(2, own, None) };
        let line = |died: &Died, seed: u64| line_for(died, &feed.known, None, seed).unwrap().0;

        assert_eq!(line(&exploded, 7), Words::fill("kill_feed.exploded", 7, &[("victim", "Bo")], EXPLOSION));
        assert_eq!(line(&suicide, 7), Words::fill("kill_feed.died", 7, &[("victim", "Bo")], SKULL));
        for died in [&exploded, &suicide] {
            let wordings: std::collections::BTreeSet<String> = (0..64).map(|seed| line(died, seed).after).collect();
            assert!(wordings.len() > 1, "every seed read {wordings:?}");
            for wording in (0..64).map(|seed| line(died, seed)) {
                assert!(wording.before.is_empty() && wording.after.starts_with("Bo "), "{wording:?}");
            }
        }
    }

    #[test]
    fn a_crate_breaking_is_not_listed() {
        let mut crate_ = died(0, DamageSource::Player(PlayerId(1)), None);
        crate_.victim_id = None;
        assert_eq!(lines(&[crate_]), vec![None]);
    }

    fn words(feed: &KillFeed) -> Vec<&str> {
        feed.lines.iter().map(|line| line.words.before.as_str()).collect()
    }

    /// Breaks if lines naming us are bumped by others, more than two of them are kept, they are not
    /// in time order with the rest, or a bumped line comes back.
    #[test]
    fn our_latest_two_lines_outlast_the_others_in_time_order() {
        let mut feed = KillFeed::default();
        for words in ["a", "b"] {
            feed.push(words.into(), Swatch::neutral(), false);
        }
        feed.push("ours 1".into(), Swatch::neutral(), true);
        feed.push("ours 2".into(), Swatch::neutral(), true);
        feed.push("ours 3".into(), Swatch::neutral(), true);
        for words in ["c", "d", "e"] {
            feed.push(words.into(), Swatch::neutral(), false);
        }
        assert_eq!(words(&feed), ["ours 2", "ours 3", "c", "d", "e"]);
        feed.push("f".into(), Swatch::neutral(), false);
        assert_eq!(words(&feed), ["ours 2", "ours 3", "d", "e", "f"]);

        feed.age(LIFETIME);
        assert_eq!(words(&feed), ["ours 2", "ours 3"], "ours went with the others, or a bumped line came back");
        feed.age(OUR_LIFETIME - LIFETIME);
        assert!(feed.lines.is_empty());
    }

    #[test]
    fn a_death_names_us_as_victim_killer_or_assist() {
        let us = DamageSource::Player(PlayerId(1));
        let other = DamageSource::Player(PlayerId(3));
        assert!(names(&died(1, other, None), Some(PlayerId(1))));
        assert!(names(&died(2, us, None), Some(PlayerId(1))));
        assert!(names(&died(2, other, Some(us)), Some(PlayerId(1))));
        assert!(!names(&died(2, other, None), Some(PlayerId(1))));
        assert!(!names(&died(1, other, None), None));
    }

    /// Breaks if a line outlives its lifetime or goes before it, or takes a younger one with it.
    #[test]
    fn a_line_goes_after_its_lifetime() {
        let mut feed = KillFeed::default();
        feed.push("old".into(), Swatch::neutral(), false);
        feed.age(LIFETIME * 0.6);
        feed.push("new".into(), Swatch::neutral(), false);
        feed.age(LIFETIME * 0.3);
        assert_eq!(feed.lines.len(), 2, "gone early");
        feed.age(LIFETIME * 0.2);
        let left: Vec<&str> = feed.lines.iter().map(|line| line.words.before.as_str()).collect();
        assert_eq!(left, ["new"]);
    }

    /// Breaks if the feed holds more than five, drops the newest instead of the oldest, or the
    /// plates on screen do not follow it in order.
    #[test]
    fn a_sixth_line_bumps_the_oldest_and_the_plates_follow() {
        let mut world = World::new();
        world.init_resource::<DisplayFont>();
        world.init_resource::<KillFeed>();
        world.init_resource::<KillIcons>();
        world.spawn(WindowSafeZone);
        world.run_system_once(show_the_feed).unwrap();

        let shown = |world: &mut World| -> Vec<String> {
            let mut column = world.query_filtered::<&Children, With<FeedColumn>>();
            let plates: Vec<Entity> = column.single(world).map(|children| children.to_vec()).unwrap_or_default();
            plates.iter().map(|plate| world.get::<Children>(*plate).unwrap()[0]).map(|text| world.get::<Text>(text).unwrap().0.clone()).collect()
        };
        for index in 0..LINES + 1 {
            world.resource_mut::<KillFeed>().push(format!("line {index}").into(), Swatch::neutral(), false);
            world.run_system_once(show_the_feed).unwrap();
        }
        let expected: Vec<String> = (1..=LINES).map(|index| format!("line {index}")).collect();
        assert_eq!(shown(&mut world), expected);
    }

    /// Breaks if a line stops drawing what killed, or a name could move the icon.
    #[test]
    fn a_line_draws_what_killed_between_the_names() {
        let mut feed = KillFeed::default();
        feed.learn(&standing(1, "An\u{E000}a", None));
        feed.learn(&standing(2, "Bo", None));
        let catalog = crate::common::weapon_file::default_catalog();
        let mut burned = died(2, DamageSource::Player(PlayerId(1)), None);
        burned.cause = Cause::of(Some(WeaponId::of("flamethrower")), How::Burn);
        let (words, _) = line_for(&burned, &feed.known, Some(&catalog), 0).unwrap();
        assert_eq!(words, Words { before: "Ana".into(), icon: FLAME.into(), after: "Bo".into() });
        burned.cause.how = How::Flame;
        assert_eq!(line_for(&burned, &feed.known, Some(&catalog), 0).unwrap().0.icon, "kill_flamethrower");
    }

    /// Breaks if a plate stops standing its icon between its two lots of words.
    #[test]
    fn the_icon_stands_between_the_words() {
        let mut world = World::new();
        world.init_resource::<DisplayFont>();
        world.init_resource::<KillFeed>();
        world.init_resource::<KillIcons>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<crate::common::assets::Assets>();
        world.spawn(WindowSafeZone);
        world.run_system_once(show_the_feed).unwrap();
        let words = Words { before: "Ana".into(), icon: "kill_rocket".into(), after: "Bo".into() };
        world.resource_mut::<KillFeed>().push(words, Swatch::neutral(), false);
        world.run_system_once(show_the_feed).unwrap();

        let plate = world.query_filtered::<&Children, With<ShowsLine>>().single(&world).unwrap().to_vec();
        assert_eq!(plate.len(), 3);
        assert_eq!(world.get::<Text>(plate[0]).unwrap().0, "Ana");
        assert!(world.get::<PlateIcon>(plate[1]).is_some() && world.get::<ImageNode>(plate[1]).is_some(), "no icon drawn");
        assert_eq!(world.get::<Text>(plate[2]).unwrap().0, "Bo");
    }
}
