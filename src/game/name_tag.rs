//! The names of whoever the crosshair is on, each on a panel in their color.
//!
//! A tag hangs under its body's feet as drawn on screen, centered on the
//! body's vertical axis, or over its head when there is no room below, and
//! stays inside the [`WindowSafeZone`]. It stays up for [`LINGER`] after the
//! crosshair leaves, so sweeping across a group leaves several up. The panel
//! is as wide as the name, so a new tag is drawn a frame late: the width comes
//! from layout.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::fonts::DisplayFont;
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::trace;
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::common::ui_shape::{Corner, ShapeMaterial};
use crate::game::collision::CollisionWorld;
use crate::game::death::Dying;
use crate::game::hud::{hud_text, leaning_panel, panel_style, WindowSafeZone, WINDOW_MARGIN, ZONE_DROP, ZONE_STROKE};
use crate::game::player::{LocalPlayer, PhysicsBody, Player, PlayerCamera, Watched};
use crate::game::score::Standing;
use crate::get;

/// Meters.
const RANGE: f32 = 200.0;
/// Seconds a tag stays up once the crosshair is off its body.
const LINGER: f32 = 0.5;
/// Logical pixels between the panel and the feet or head it hangs from.
const GAP: f32 = 6.0;
const TEXT: f32 = 20.0;
/// Logical pixels inside the panel's box. The far sides are wider by the drop
/// shadow, which centers the name on the panel rather than on panel and shadow.
const PADDING: UiRect = UiRect {
    left: Val::Px(ZONE_STROKE + 15.0),
    right: Val::Px(ZONE_STROKE + ZONE_DROP.x + 15.0),
    top: Val::Px(ZONE_STROKE + 1.0),
    bottom: Val::Px(ZONE_STROKE + ZONE_DROP.y + 1.0),
};

pub struct NameTagPlugin;

impl Plugin for NameTagPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (tag_whoever_is_aimed_at, place_the_tags)
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
    /// The size and colors the panel was built for.
    drawn: Option<(Vec2, Swatch)>,
}

impl NameTag {
    fn of(body: Entity) -> Self {
        Self { body, unseen: 0.0, drawn: None }
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
struct NameTagPanel;

/// Goes when the HUD does: it is the window safe zone's child.
fn name_tag(body: Entity, font: &Handle<Font>) -> impl Bundle {
    (
        NameTag::of(body),
        Node { position_type: PositionType::Absolute, padding: PADDING, ..default() },
        Visibility::Hidden,
        Pickable::IGNORE,
        Name::new("Name tag"),
        children![
            (
                NameTagPanel,
                // Left at its static position, it would start inside the padding.
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                Pickable::IGNORE,
            ),
            (NameTagText, hud_text(font, String::new(), TEXT, Color::WHITE)),
        ],
    )
}

/// What "Player {id}" a body with no standing yet is called, as on the scoreboard.
fn name_of<'a>(id: PlayerId, standings: impl IntoIterator<Item = &'a Standing>) -> String {
    standings
        .into_iter()
        .find(|standing| standing.id == id)
        .map_or_else(|| get!("scoreboard.player", "id", id.0), Standing::display_name)
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

/// Puts a tag up for the body under the crosshair, keeps it up while it
/// stays there, and takes down any left unseen too long or whose body went.
#[allow(clippy::too_many_arguments)]
fn tag_whoever_is_aimed_at(
    mut commands: Commands,
    time: Res<Time>,
    font: Res<DisplayFont>,
    world: Option<Res<CollisionWorld>>,
    camera: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
    us: Option<Single<Entity, With<LocalPlayer>>>,
    zone: Option<Single<Entity, With<WindowSafeZone>>>,
    targets: Query<(Entity, &Hitboxes), Without<Dying>>,
    people: Query<Person, People>,
    mut tags: Query<(Entity, &mut NameTag)>,
) {
    let (Some(world), Some(eye), Some(us), Some(zone)) = (world, camera, us, zone) else {
        for (tag, _) in &tags {
            commands.entity(tag).despawn();
        }
        return;
    };

    // Everybody but us, crates and dummies included, so they hide whoever is behind them.
    let ray = Ray3d::new(eye.translation(), eye.forward());
    let range = world.ray_distance(&ray, RANGE).unwrap_or(RANGE);
    let others = targets.iter().filter(|(entity, _)| *entity != *us).map(|(entity, boxes)| (entity, *boxes));
    let aimed = trace(&ray, range, others).map(|hit| hit.target).filter(|body| people.contains(*body));

    for (tag, mut name_tag) in &mut tags {
        let aimed_at = aimed == Some(name_tag.body);
        let stays = name_tag.age(aimed_at, time.delta_secs());
        if !stays || !people.contains(name_tag.body) {
            commands.entity(tag).despawn();
        }
    }
    if let Some(body) = aimed
        && !tags.iter().any(|(_, tag)| tag.body == body)
    {
        commands.entity(*zone).with_child(name_tag(body, &font.0));
    }
}

#[allow(clippy::type_complexity)]
fn place_the_tags(
    mut commands: Commands,
    camera: Option<Single<(&Camera, &GlobalTransform), With<PlayerCamera>>>,
    zone: Option<Single<&ComputedNode, With<WindowSafeZone>>>,
    people: Query<Person, People>,
    standings: Query<&Standing>,
    mut tags: Query<(&mut NameTag, &mut Node, &mut Visibility, &ComputedNode, &Children)>,
    mut texts: Query<(&mut Text, &mut TextColor), With<NameTagText>>,
    panels: Query<Entity, With<NameTagPanel>>,
    mut materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let (Some(camera), Some(zone)) = (camera, zone) else { return };
    let (camera, eye) = camera.into_inner();
    let zone = zone.size() * zone.inverse_scale_factor();

    for (mut tag, mut node, mut visibility, computed, children) in &mut tags {
        let Ok((&id, drawn, boxes, physics, team, color)) = people.get(tag.body) else { continue };

        let name = name_of(id, &standings);
        let swatch = Swatch::of_body(team.copied(), color.copied());
        let mut renamed = false;
        for &child in children {
            if let Ok((mut text, mut ink)) = texts.get_mut(child) {
                if text.0 != name {
                    text.0 = name.clone();
                    renamed = true;
                }
                ink.set_if_neq(TextColor(swatch.text()));
            }
        }
        let size = computed.size() * computed.inverse_scale_factor();
        if renamed || size == Vec2::ZERO {
            // Shown once layout has given the new name a width.
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }

        if tag.drawn != Some((size, swatch))
            && let Some(materials) = materials.as_deref_mut()
            && let Some(panel) = children.iter().find(|child| panels.contains(*child))
        {
            let outline = leaning_panel(size, true, Corner::squircle(16.0), Corner::squircle(6.0));
            let material = materials.add(ShapeMaterial::from_path(&outline, size, panel_style(&swatch)));
            commands.entity(panel).insert(MaterialNode(material));
            tag.drawn = Some((size, swatch));
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
        assert_eq!(name_of(PlayerId(3), [&named]), "Grackle");
        assert_eq!(name_of(PlayerId(4), [&named]), get!("scoreboard.player", "id", 4));
    }
}
