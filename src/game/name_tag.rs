//! The names of whoever the crosshair is on, each on a panel in their color.
//!
//! A tag hangs under its body's feet as drawn on screen, centered on the
//! body's vertical axis, or over its head when there is no room below, and
//! stays inside the [`WindowSafeZone`]. It stays up for [`LINGER`] after the
//! crosshair leaves, so sweeping across a group leaves several up. The panel
//! is as wide as the name, so a new tag is drawn a frame late: the width comes
//! from layout.
//!
//! While `F8` shows the bots ([`ShowBots`]), every bot's tag is up whether
//! aimed at or not, with its goal on a second line.
//!
//! While `F8` is on, a spectator watching a bot has its name and goal on the same panel at the
//! bottom middle of the [`SafeZone`].
//!
//! Until a [`KillCam`] hands over to spectating, the same panel at the middle of the right edge
//! of the [`WindowSafeZone`] says who killed us and with what, in their color.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::fonts::DisplayFont;
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::trace;
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::game::bot::{BotOf, ShowBots};
use crate::common::weapon::{WeaponCatalog, WeaponId};
use crate::game::bot_think::{BotThought, Candidate, Want};
use crate::game::kill_cam::KillCam;
use crate::game::collision::CollisionWorld;
use crate::game::death::Dying;
use crate::game::hud::{SafeZone, WindowSafeZone, MARGIN_Y, WINDOW_MARGIN};
use crate::game::plate::{plate, plate_node, plate_text, Plate};
use crate::game::player::{LocalPlayer, PhysicsBody, Player, PlayerCamera, Viewed, Watched};
use crate::game::score::Standing;
use crate::get;

/// Meters.
const RANGE: f32 = 200.0;
/// Seconds a tag stays up once the crosshair is off its body.
const LINGER: f32 = 0.5;
/// Logical pixels between the panel and the feet or head it hangs from.
const GAP: f32 = 6.0;
const TEXT: f32 = 20.0;
const GOAL_TEXT: f32 = 14.0;

pub struct NameTagPlugin;

impl Plugin for NameTagPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShowBots>().add_systems(
            PostUpdate,
            (tag_whoever_is_aimed_at, place_the_tags, name_whoever_is_watched, name_our_killer)
                .chain()
                // Where bodies and the camera are drawn this frame, and a new
                // name laid out in the frame it is written.
                .after(TransformSystems::Propagate)
                .before(bevy::ui::UiSystems::Layout)
                .run_if(in_state(AppMode::Play)),
        );
    }
}

#[derive(Component)]
struct NameTag {
    body: Entity,
    /// Seconds since the crosshair was last on the body.
    unseen: f32,
    words: Words,
}

impl NameTag {
    fn of(body: Entity) -> Self {
        Self { body, unseen: 0.0, words: Words::default() }
    }

    /// Whether it stays up.
    fn age(&mut self, aimed_at: bool, seconds: f32) -> bool {
        self.unseen = if aimed_at { 0.0 } else { self.unseen + seconds };
        self.unseen <= LINGER
    }
}

#[derive(Component)]
struct NameTagText;

#[derive(Component)]
struct NameTagGoal;

/// The bottom-middle panel naming whoever a spectator watches.
#[derive(Component)]
struct WatchedBanner;

#[derive(Component, Default)]
struct WatchedPlate {
    words: Words,
}

/// The center-right panel naming who killed us.
#[derive(Component)]
struct KillerBanner;

#[derive(Component, Default)]
struct KillerPlate {
    words: Words,
    /// The killer's name, weapon and language the lines were built for.
    built_for: Option<(String, Option<WeaponId>, u64)>,
    killed_by: String,
    using: Option<String>,
}

impl KillerPlate {
    fn refresh(&mut self, killer: PlayerId, weapon: Option<WeaponId>, standings: &Query<&Standing>, catalog: Option<&WeaponCatalog>) {
        self.words.refresh_name(killer, standings);
        let generation = crate::common::lang::generation();
        if self.built_for.as_ref().is_some_and(|(name, built_weapon, built_generation)| {
            *name == self.words.name && *built_weapon == weapon && *built_generation == generation
        }) {
            return;
        }
        let key = (self.words.name.clone(), weapon, generation);
        self.killed_by = get!("kill_cam.killed_by", "name", self.words.name);
        self.using = weapon
            .and_then(|weapon| catalog?.get(weapon))
            .map(|weapon| get!("kill_cam.using", "weapon", get!(weapon.name_key)));
        self.built_for = Some(key);
    }
}

/// What a bot's goal line is built from.
#[derive(Clone, Copy, PartialEq)]
struct GoalKey {
    chosen: Option<(Want, f32)>,
    runner_up: Option<(Want, f32)>,
}

impl GoalKey {
    fn of(thought: &BotThought) -> Self {
        let pair = |candidate: Candidate| (candidate.want, candidate.score);
        Self { chosen: thought.chosen.map(pair), runner_up: thought.runner_up.map(pair) }
    }
}

/// A plate's lines, kept with what each was built from so a still frame builds no string.
#[derive(Default)]
struct Words {
    name: String,
    /// The id and language an unnamed player's "Player {id}" was built for; `None` while named.
    unnamed: Option<(PlayerId, u64)>,
    goal: Option<String>,
    goal_from: Option<(GoalKey, u64)>,
}

impl Words {
    fn refresh(&mut self, id: PlayerId, standings: &Query<&Standing>, thought: Option<&BotThought>) {
        self.refresh_name(id, standings);
        self.refresh_goal(thought);
    }

    fn refresh_name(&mut self, id: PlayerId, standings: &Query<&Standing>) {
        let named = standings.iter().find(|standing| standing.id == id).map(|standing| standing.name.as_str()).filter(|name| !name.is_empty());
        match named {
            Some(name) => {
                self.unnamed = None;
                if self.name != name {
                    name.clone_into(&mut self.name);
                }
            }
            None => {
                let key = Some((id, crate::common::lang::generation()));
                if self.unnamed != key {
                    self.unnamed = key;
                    self.name = get!("scoreboard.player", "id", id.0);
                }
            }
        }
    }

    fn refresh_goal(&mut self, thought: Option<&BotThought>) {
        let Some(thought) = thought else {
            self.goal_from = None;
            self.goal = None;
            return;
        };
        let key = Some((GoalKey::of(thought), crate::common::lang::generation()));
        if self.goal_from != key || self.goal.is_none() {
            self.goal_from = key;
            self.goal = Some(thought_line(thought));
        }
    }
}

fn plate_lines(font: &Handle<Font>) -> impl Bundle {
    children![
        (NameTagText, plate_text(font, String::new(), TEXT)),
        (NameTagGoal, plate_text(font, String::new(), GOAL_TEXT), Node { display: Display::None, ..default() }),
    ]
}

/// Goes when the HUD does: it is the safe zone's child.
fn watched_banner(font: &Handle<Font>) -> impl Bundle {
    (
        WatchedBanner,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            bottom: Val::Px(MARGIN_Y),
            justify_content: JustifyContent::Center,
            ..default()
        },
        Visibility::Hidden,
        Pickable::IGNORE,
        Name::new("Watched banner"),
        children![(WatchedPlate::default(), plate(Swatch::neutral()), plate_lines(font))],
    )
}

/// Goes when the HUD does: it is the window safe zone's child.
fn killer_banner(font: &Handle<Font>) -> impl Bundle {
    (
        KillerBanner,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            right: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            ..default()
        },
        Visibility::Hidden,
        Pickable::IGNORE,
        Name::new("Killer banner"),
        children![(KillerPlate::default(), plate(Swatch::neutral()), plate_lines(font))],
    )
}

/// What a tag or a banner writes into its lines; [`Plate`] paints the panel and inks them.
#[derive(SystemParam)]
struct Plates<'w, 's> {
    commands: Commands<'w, 's>,
    texts: Query<'w, 's, &'static mut Text, (With<NameTagText>, Without<NameTagGoal>)>,
    goals: Query<'w, 's, (&'static mut Text, &'static mut Node), (With<NameTagGoal>, Without<NameTagText>, Without<NameTag>, Without<Plate>)>,
}

impl Plates<'_, '_> {
    /// Writes the lines and the color. False while layout has yet to give what was just written a
    /// width, which is when the plate should stay hidden.
    fn fill(&mut self, plate: &mut Mut<Plate>, children: &Children, name: &str, goal: Option<&str>, swatch: Swatch, size: Vec2) -> bool {
        if plate.swatch != swatch {
            plate.swatch = swatch;
        }
        let mut rewritten = false;
        for &child in children {
            if let Ok(mut text) = self.texts.get_mut(child)
                && text.0 != name
            {
                text.0 = name.to_owned();
                rewritten = true;
            }
            if let Ok((mut text, mut node)) = self.goals.get_mut(child) {
                let display = if goal.is_some() { Display::Flex } else { Display::None };
                let words = goal.unwrap_or_default();
                if text.0 != words || node.display != display {
                    words.clone_into(&mut text.0);
                    node.display = display;
                    rewritten = true;
                }
            }
        }
        !rewritten && size != Vec2::ZERO
    }
}

/// Goes when the HUD does: it is the window safe zone's child.
fn name_tag(body: Entity, font: &Handle<Font>) -> impl Bundle {
    (
        NameTag::of(body),
        Plate::new(Swatch::neutral()),
        Node { position_type: PositionType::Absolute, ..plate_node() },
        Visibility::Hidden,
        Name::new("Name tag"),
        plate_lines(font),
    )
}

/// What a bot wants, scored, and the different want that came closest. Filled uncached: a
/// string per pair of scores would fill the lang cache.
fn thought_line(thought: &BotThought) -> String {
    use crate::common::lang::fill_uncached;
    let score = |score: f32| format!("{score:.2}");
    match (thought.chosen, thought.runner_up) {
        (None, _) => get!("bot.thought.none"),
        (Some(chosen), None) => fill_uncached("bot.thought.alone", &[("want", &chosen.want.name()), ("score", &score(chosen.score))]),
        (Some(chosen), Some(other)) => fill_uncached(
            "bot.thought.over",
            &[
                ("want", &chosen.want.name()),
                ("score", &score(chosen.score)),
                ("other", &other.want.name()),
                ("against", &score(other.score)),
            ],
        ),
    }
}

/// Top left of a panel of `size` in a zone of `zone`, all in the zone's
/// logical pixels. Under the feet if it fits there, else over the head, and
/// never out of the zone.
fn placement(feet: Option<Vec2>, head: Option<Vec2>, size: Vec2, zone: Vec2) -> Option<Vec2> {
    let under = feet.filter(|feet| feet.y + GAP + size.y <= zone.y).map(|feet| feet + Vec2::Y * GAP);
    let over = || head.map(|head| head - Vec2::Y * (GAP + size.y));
    let anchor = under.or_else(over).or(feet)?;
    let corner = Vec2::new(anchor.x - size.x * 0.5, anchor.y);
    Some(corner.clamp(Vec2::ZERO, (zone - size).max(Vec2::ZERO)))
}

type Person<'a> = (&'a PlayerId, &'a GlobalTransform, &'a Hitboxes, Option<&'a PhysicsBody>, Option<&'a Team>, Option<&'a PlayerColor>);
type People = (With<Player>, Without<Dying>, Without<Watched>, Without<LocalPlayer>);

/// Puts a tag up for the body under the crosshair, and for every bot while
/// they are shown, keeps it up while it stays there, and takes down any left
/// unseen too long or whose body went.
#[allow(clippy::too_many_arguments)]
fn tag_whoever_is_aimed_at(
    mut commands: Commands,
    time: Res<Time>,
    font: Res<DisplayFont>,
    show: Res<ShowBots>,
    bots: Query<Entity, With<BotOf>>,
    world: Option<Res<CollisionWorld>>,
    camera: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
    viewed: Query<Entity, Viewed>,
    zone: Option<Single<Entity, With<WindowSafeZone>>>,
    targets: Query<(Entity, &Hitboxes), Without<Dying>>,
    people: Query<Person, People>,
    mut tags: Query<(Entity, &mut NameTag)>,
) {
    let (Some(world), Some(eye), Some(zone)) = (world, camera, zone) else {
        for (tag, _) in &tags {
            commands.entity(tag).despawn();
        }
        return;
    };

    // Everybody but the body the view is in, ours or a watched one, crates and dummies included,
    // so they hide whoever is behind them.
    let ray = Ray3d::new(eye.translation(), eye.forward());
    let range = world.ray_distance(&ray, RANGE).unwrap_or(RANGE);
    let others = targets.iter().filter(|(entity, _)| !viewed.contains(*entity)).map(|(entity, boxes)| (entity, *boxes));
    let aimed = trace(&ray, range, others).map(|hit| hit.target).filter(|body| people.contains(*body));

    let shown = |body: Entity| show.0 && bots.contains(body);
    for (tag, mut name_tag) in &mut tags {
        let aimed_at = aimed == Some(name_tag.body);
        let stays = name_tag.age(aimed_at, time.delta_secs()) || shown(name_tag.body);
        if !stays || !people.contains(name_tag.body) {
            commands.entity(tag).despawn();
        }
    }
    let mut wanted: Vec<Entity> = if show.0 { bots.iter().filter(|body| people.contains(*body)).collect() } else { Vec::new() };
    wanted.extend(aimed);
    wanted.sort();
    wanted.dedup();
    for body in wanted {
        if !tags.iter().any(|(_, tag)| tag.body == body) {
            commands.entity(*zone).with_child(name_tag(body, &font.0));
        }
    }
}

#[allow(clippy::type_complexity)]
fn place_the_tags(
    camera: Option<Single<(&Camera, &GlobalTransform), With<PlayerCamera>>>,
    zone: Option<Single<&ComputedNode, With<WindowSafeZone>>>,
    people: Query<Person, People>,
    standings: Query<&Standing>,
    show: Res<ShowBots>,
    thoughts: Query<&BotThought>,
    mut tags: Query<(&mut NameTag, &mut Plate, &mut Node, &mut Visibility, &ComputedNode, &Children)>,
    mut plates: Plates,
) {
    let (Some(camera), Some(zone)) = (camera, zone) else { return };
    let (camera, eye) = camera.into_inner();
    let zone = zone.size() * zone.inverse_scale_factor();

    for (mut tag, mut plate, mut node, mut visibility, computed, children) in &mut tags {
        let Ok((&id, drawn, boxes, physics, team, color)) = people.get(tag.body) else { continue };

        let swatch = Swatch::of_body(team.copied(), color.copied());
        let size = computed.size() * computed.inverse_scale_factor();
        let tag = &mut *tag;
        tag.words.refresh(id, &standings, thoughts.get(tag.body).ok().filter(|_| show.0));
        if !plates.fill(&mut plate, children, &tag.words.name, tag.words.goal.as_deref(), swatch, size) {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }

        // Hitboxes are where the step left the body; drawn, it is interpolated
        // between steps.
        let lag = physics.map_or(Vec3::ZERO, |physics| drawn.translation() - physics.current);
        let axis = boxes.body.center + lag;
        let top = boxes.head.map_or(boxes.body.max().y, |head| head.max().y.max(boxes.body.max().y)) + lag.y;
        let feet = Vec3::new(axis.x, boxes.body.min().y + lag.y, axis.z);
        let head = Vec3::new(axis.x, top, axis.z);

        // `world_to_viewport` is already in logical pixels; the zone is inset from the window.
        let on_screen = |point: Vec3| camera.world_to_viewport(eye, point).ok().map(|at| at - Vec2::splat(WINDOW_MARGIN));
        let Some(corner) = placement(on_screen(feet), on_screen(head), size, zone) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        if node.left != Val::Px(corner.x) || node.top != Val::Px(corner.y) {
            node.left = Val::Px(corner.x);
            node.top = Val::Px(corner.y);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

/// Puts the banner up in the safe zone once there is one, and fills it with the watched bot and
/// its goal while `F8` shows the bots. The top banner names whoever is watched.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn name_whoever_is_watched(
    font: Res<DisplayFont>,
    show: Res<ShowBots>,
    zone: Option<Single<Entity, With<SafeZone>>>,
    watched: Option<Single<(Entity, &PlayerId, Option<&Team>, Option<&PlayerColor>), (With<Watched>, Without<Dying>)>>,
    thoughts: Query<&BotThought>,
    standings: Query<&Standing>,
    mut banners: Query<(&mut Visibility, &Children), With<WatchedBanner>>,
    mut plate: Query<(&mut WatchedPlate, &mut Plate, &ComputedNode, &Children)>,
    mut plates: Plates,
) {
    let Some(zone) = zone else { return };
    let Some((mut visibility, children)) = banners.iter_mut().next() else {
        plates.commands.entity(*zone).with_child(watched_banner(&font.0));
        return;
    };
    let watched = watched.map(Single::into_inner).filter(|_| show.0);
    let Some(((_, &id, team, color), thought)) = watched.and_then(|watched| Some((watched, thoughts.get(watched.0).ok()?))) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let Some((mut drawn, mut shape, computed, lines)) = children.first().and_then(|inner| plate.get_mut(*inner).ok()) else { return };
    let size = computed.size() * computed.inverse_scale_factor();
    let swatch = Swatch::of_body(team.copied(), color.copied());
    let plate = &mut *drawn;
    plate.words.refresh(id, &standings, Some(thought));
    let ready = plates.fill(&mut shape, lines, &plate.words.name, plate.words.goal.as_deref(), swatch, size);
    visibility.set_if_neq(if ready { Visibility::Inherited } else { Visibility::Hidden });
}

#[allow(clippy::too_many_arguments)]
fn name_our_killer(
    font: Res<DisplayFont>,
    zone: Option<Single<Entity, With<WindowSafeZone>>>,
    kill_cam: Option<Res<KillCam>>,
    catalog: Option<Res<WeaponCatalog>>,
    standings: Query<&Standing>,
    mut banners: Query<(&mut Visibility, &Children), With<KillerBanner>>,
    mut plate: Query<(&mut KillerPlate, &mut Plate, &ComputedNode, &Children)>,
    mut plates: Plates,
) {
    let Some(zone) = zone else { return };
    let Some((mut visibility, children)) = banners.iter_mut().next() else {
        plates.commands.entity(*zone).with_child(killer_banner(&font.0));
        return;
    };
    // Spectating, the bottom banner names whoever is watched.
    let Some(kill_cam) = kill_cam.filter(|kill_cam| !kill_cam.spectating()) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let Some((mut drawn, mut shape, computed, lines)) = children.first().and_then(|inner| plate.get_mut(*inner).ok()) else { return };
    let size = computed.size() * computed.inverse_scale_factor();
    let standing = standings.iter().find(|standing| standing.id == kill_cam.killer);
    let swatch = standing.map_or_else(Swatch::neutral, Standing::hud_swatch);
    let plate = &mut *drawn;
    plate.refresh(kill_cam.killer, kill_cam.weapon, &standings, catalog.as_deref());
    let ready = plates.fill(&mut shape, lines, &plate.killed_by, plate.using.as_deref(), swatch, size);
    visibility.set_if_neq(if ready { Visibility::Inherited } else { Visibility::Hidden });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE: Vec2 = Vec2::new(800.0, 600.0);
    const SIZE: Vec2 = Vec2::new(120.0, 40.0);

    #[test]
    fn it_hangs_under_the_feet_centered_on_them() {
        let corner = placement(Some(Vec2::new(400.0, 300.0)), Some(Vec2::new(400.0, 100.0)), SIZE, ZONE).unwrap();
        assert_eq!(corner, Vec2::new(340.0, 300.0 + GAP));
    }

    #[test]
    fn with_no_room_under_the_feet_it_goes_over_the_head() {
        let corner = placement(Some(Vec2::new(410.0, 580.0)), Some(Vec2::new(400.0, 200.0)), SIZE, ZONE).unwrap();
        assert_eq!(corner, Vec2::new(340.0, 200.0 - GAP - SIZE.y));
    }

    #[test]
    fn it_never_leaves_the_zone() {
        let corner = placement(Some(Vec2::new(5.0, 590.0)), Some(Vec2::new(5.0, 10.0)), SIZE, ZONE).unwrap();
        assert_eq!(corner, Vec2::ZERO);
        let corner = placement(Some(Vec2::new(795.0, 100.0)), None, SIZE, ZONE).unwrap();
        assert_eq!(corner.x, ZONE.x - SIZE.x);
    }

    #[test]
    fn a_tag_lingers_after_the_crosshair_leaves_and_is_kept_by_coming_back() {
        let mut tag = NameTag::of(Entity::PLACEHOLDER);
        assert!(tag.age(false, LINGER * 0.6));
        assert!(tag.age(true, 0.1), "aiming back at it did not keep it");
        assert!(tag.age(false, LINGER * 0.6), "coming back did not restart the wait");
        assert!(!tag.age(false, LINGER * 0.6), "it outstayed its linger");
    }


    /// Breaks if the bottom banner is up while `F8` is off, or with it on does not name the
    /// watched bot and its goal.
    #[test]
    fn the_banner_names_a_watched_bot_and_its_goal_while_bots_are_shown() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<DisplayFont>();
        world.init_resource::<ShowBots>();
        world.spawn(SafeZone);
        world.spawn((Watched, PlayerId(7), BotThought::default()));
        world.run_system_once(name_whoever_is_watched).unwrap();
        world.run_system_once(name_whoever_is_watched).unwrap();
        let mut banners = world.query_filtered::<&Visibility, With<WatchedBanner>>();
        assert_eq!(banners.single(&world).unwrap(), &Visibility::Hidden, "up with F8 off");

        world.resource_mut::<ShowBots>().0 = true;
        world.run_system_once(name_whoever_is_watched).unwrap();

        let mut lines = world.query::<(&Text, &Node, Has<NameTagGoal>)>();
        let lines: Vec<(String, Display, bool)> = lines.iter(&world).map(|(text, node, goal)| (text.0.clone(), node.display, goal)).collect();
        assert!(lines.contains(&(get!("scoreboard.player", "id", 7), Display::Flex, false)), "{lines:?}");
        assert!(lines.contains(&(get!("bot.thought.none"), Display::Flex, true)), "{lines:?}");
    }

    /// Breaks if the kill cam does not say who killed us and with what, or the banner stays up
    /// without one.
    #[test]
    fn the_killer_banner_names_the_killer_and_the_weapon() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<DisplayFont>();
        world.spawn(WindowSafeZone);
        world.insert_resource(KillCam::new(PlayerId(7), None, 0.0, Vec3::ZERO));
        world.run_system_once(name_our_killer).unwrap();
        world.run_system_once(name_our_killer).unwrap();

        let mut lines = world.query::<(&Text, &Node, Has<NameTagGoal>)>();
        let lines: Vec<(String, Display, bool)> = lines.iter(&world).map(|(text, node, goal)| (text.0.clone(), node.display, goal)).collect();
        let named = get!("kill_cam.killed_by", "name", get!("scoreboard.player", "id", 7));
        assert!(lines.contains(&(named, Display::Flex, false)), "{lines:?}");
        assert!(lines.iter().any(|(_, display, goal)| *goal && *display == Display::None), "a weapon line with no weapon: {lines:?}");

        world.remove_resource::<KillCam>();
        world.run_system_once(name_our_killer).unwrap();
        let mut banners = world.query_filtered::<&Visibility, With<KillerBanner>>();
        assert_eq!(banners.single(&world).unwrap(), &Visibility::Hidden);
    }

    #[test]
    fn somebody_with_no_standing_is_named_by_id() {
        let named = Standing {
            id: PlayerId(3),
            name: "Grackle".into(),
            team: None,
            color: PlayerColor { hue: 0.0 },
            frags: 0,
            kills: 0,
            assists: 0,
            deaths: 0,
            damage_dealt: 0,
            damage_taken: 0,
            healing: 0,
        };
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.spawn(named);
        let names = world
            .run_system_once(|standings: Query<&Standing>| {
                let mut words = Words::default();
                words.refresh_name(PlayerId(3), &standings);
                let three = words.name.clone();
                words.refresh_name(PlayerId(4), &standings);
                (three, words.name.clone())
            })
            .unwrap();
        assert_eq!(names, ("Grackle".to_owned(), get!("scoreboard.player", "id", 4)));
    }
}
