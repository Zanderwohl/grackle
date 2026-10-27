//! The crosshair, health and what you are holding. Plain Bevy UI, not egui,
//! so it exists in a build with no editor.
//!
//! Everything but the crosshair sits in the **safe zone**: a 4:3 box the
//! height of the window, centered. On a 4:3 window it is the whole screen; on
//! an ultrawide the readouts stay near the crosshair rather than out at the
//! edges where reading them means looking away from the fight. Narrower than
//! 4:3 it is clamped to the window. `F9` outlines it.
//!
//! The HUD has a camera of its own, drawing nothing but UI, a child of the
//! play camera and ordered above the viewmodel's. On the play camera it was
//! painted over by the arms; the viewmodel camera cannot carry it either,
//! being switched off in third person.
//!
//! Ids are resolved through the catalog every frame rather than cached:
//! `Equipped` and the catalog arrive on different channels, and an id that
//! does not resolve yet draws blank.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::Hdr;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::{Armor, Damageable};
use crate::common::fonts::DisplayFont;
use crate::common::player_color::PlayerColor;
use crate::common::swatch::{Ink, Swatch};
use crate::common::team::Team;
use crate::common::ui_shape::{Corner, Path, ShapeMaterial, ShapeStyle};
use crate::common::weapon::{Ammo, Equipped, WeaponCatalog};
use crate::game::player::LocalPlayer;
use crate::game::viewmodel::VIEWMODEL_CAMERA_ORDER;
use crate::get;

/// Half the gap in the middle of the crosshair, in logical pixels.
const CROSSHAIR_GAP: f32 = 4.0;
/// Logical pixels.
const CROSSHAIR_ARM: f32 = 7.0;
/// Logical pixels. Even, so an arm straddles the center line cleanly.
const CROSSHAIR_THICKNESS: f32 = 2.0;

pub const HUD_CAMERA_ORDER: isize = VIEWMODEL_CAMERA_ORDER + 1;

const SAFE_ZONE_ASPECT: f32 = 4.0 / 3.0;
/// Logical pixels in from the safe zone's edges.
const MARGIN_X: f32 = 24.0;
const MARGIN_Y: f32 = 20.0;

const BIG: f32 = 44.0;
const MEDIUM: f32 = 32.0;

/// Where the display font's baseline sits in a line box, in font sizes up
/// from its bottom: cosmic-text centers the glyphs' ascent plus descent
/// (1.036 + 0.333 for Alfa Slab One) in Bevy's default 1.2 line. A different font moves it.
const BASELINE_ABOVE_LINE_BOTTOM: f32 = 0.25;
const HEALTH_PADDING_BOTTOM: f32 = 8.0;
/// From the armor number's baseline down to its label's, in pixels: a
/// lowercase word's x-height and a few pixels clear.
const LABEL_DROP: f32 = 16.0;
const SMALL: f32 = 22.0;

/// Logical pixels, in each zone's own box.
pub(crate) const ZONE_STROKE: f32 = 5.0;
const ZONE_RIM: f32 = 5.0;
pub(crate) const ZONE_DROP: Vec2 = Vec2::new(5.0, 6.0);

/// Whether the safe zone is outlined. Survives rounds: it is a debug view,
/// and somebody laying out the HUD wants it to stay on.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SafeZoneOutline(pub bool);

#[derive(Component)]
pub struct HudCamera;

#[derive(Component)]
struct HudRoot;

/// On every root of the play HUD, which the Tab scoreboard hides while it is
/// up.
#[derive(Component)]
pub struct GameHud;

#[derive(Component)]
struct SafeZone;

/// The armor readout and its panel, hidden together while there is none.
#[derive(Component)]
struct ArmorReadout;

/// Between the health panel and the armor one, in pixels.
const ZONE_GAP: f32 = 6.0;

/// A panel a readout sits on. The shape is fixed; its colors follow the
/// local body's team.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum HudZone {
    Health,
    Armor,
    Ammo,
}

impl HudZone {
    fn size(self) -> Vec2 {
        match self {
            HudZone::Health => Vec2::new(200.0, 48.0),
            HudZone::Armor => Vec2::new(120.0, 48.0),
            HudZone::Ammo => Vec2::new(280.0, 64.0),
        }
    }

    /// How far the panel's box sits above its readout's bottom. The ammo
    /// panel's bottom edge runs through the middle of the weapon name.
    fn raised(self) -> f32 {
        match self {
            HudZone::Health | HudZone::Armor => 0.0,
            HudZone::Ammo => 2.0,
        }
    }

    /// Leaning trapezoids, the two leaning toward the middle of the screen,
    /// with big squircle corners on one diagonal and tight ones on the other.
    /// The box is inset by the stroke and the drop so neither is cut off.
    fn outline(self) -> Path {
        let (big, small) = (Corner::squircle(34.0), Corner::squircle(10.0));
        leaning_panel(self.size(), self != HudZone::Ammo, big, small)
    }

    fn style(swatch: &Swatch) -> ShapeStyle {
        panel_style(swatch)
    }

    /// Drawn behind the readout rather than being it, so the text can stand
    /// out past the panel's edge. First among its siblings, so it is under
    /// them.
    fn panel(self) -> impl Bundle {
        let size = self.size();
        (
            self,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(self.raised()),
                width: Val::Px(size.x),
                height: Val::Px(size.y),
                ..default()
            },
            Pickable::IGNORE,
        )
    }

    /// The readout over a panel: as wide, and centered on the panel rather
    /// than on its drop shadow.
    fn readout(self) -> Node {
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(MARGIN_Y),
            width: Val::Px(self.size().x),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::End,
            padding: UiRect::right(Val::Px(ZONE_DROP.x)),
            ..default()
        }
    }
}

/// Inset by the stroke and the drop so neither is cut off.
pub(crate) fn leaning_panel(size: Vec2, leans_right: bool, big: Corner, small: Corner) -> Path {
    let lo = Vec2::splat(ZONE_STROKE);
    let hi = size - Vec2::splat(ZONE_STROKE) - ZONE_DROP;
    let lean = 0.12 * (hi.y - lo.y);
    if leans_right {
        Path::rounded_polygon_each(
            &[
                Vec2::new(lo.x + lean, lo.y),
                Vec2::new(hi.x, lo.y),
                Vec2::new(hi.x - lean, hi.y),
                Vec2::new(lo.x, hi.y),
            ],
            &[small, big, small, big],
        )
    } else {
        Path::rounded_polygon_each(
            &[
                Vec2::new(lo.x, lo.y),
                Vec2::new(hi.x - lean, lo.y),
                Vec2::new(hi.x, hi.y),
                Vec2::new(lo.x + lean, hi.y),
            ],
            &[big, small, big, small],
        )
    }
}

pub(crate) fn panel_style(swatch: &Swatch) -> ShapeStyle {
    ShapeStyle {
        fill: swatch.base,
        highlight: swatch.highlight,
        shadow: swatch.shadow,
        rim: ZONE_RIM,
        stroke: Ink::Dark.on(swatch),
        stroke_width: ZONE_STROKE,
        drop: swatch.deep.with_alpha(0.75),
        drop_offset: ZONE_DROP,
    }
}

/// Which readout a `Text` is, so one system can write them all.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum HudText {
    Health,
    /// Its readout and panel are hidden while there is none.
    Armor,
    WeaponName,
    Reloading,
    /// What is in the clip; for a weapon with no clip, what is left at all.
    Loaded,
    Reserve,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SafeZoneOutline>()
            .init_resource::<DisplayFont>()
            .add_systems(
                Update,
                (
                    spawn_the_hud,
                    toggle_safe_zone_outline,
                    (outline_the_safe_zone, fill_in_the_readouts, dress_the_zones),
                    paint_the_zones,
                )
                    .chain()
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), despawn_the_hud)
            .add_plugins(crate::game::weapon_select_hud::WeaponSelectHudPlugin);
    }
}

/// For the play camera's children. Every in-game UI node lands here, the
/// pause menu included, because it carries `IsDefaultUiCamera`.
pub fn hud_camera() -> impl Bundle {
    (
        HudCamera,
        bevy::ui::IsDefaultUiCamera,
        Camera2d,
        Camera {
            order: HUD_CAMERA_ORDER,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        // Must match the cameras under it, or it composites as nothing.
        Hdr,
        // It draws no scene; tonemapping again would grade the world twice.
        Tonemapping::None,
        RenderLayers::none(),
        Name::new("HUD view"),
    )
}

/// Put the HUD up once there is a body for it to describe. Keyed on
/// `Added<LocalPlayer>` because a client's body arrives from the server.
/// No camera target: the HUD camera is `IsDefaultUiCamera`.
fn spawn_the_hud(
    mut commands: Commands,
    bodies: Query<Entity, Added<LocalPlayer>>,
    existing: Query<(), With<HudRoot>>,
    font: Res<DisplayFont>,
) {
    if bodies.is_empty() || !existing.is_empty() {
        return;
    }

    commands.spawn((
        HudRoot,
        GameHud,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        // A full-screen node would otherwise swallow the pause menu's clicks.
        Pickable::IGNORE,
        children![crosshair(), safe_zone(&font.0)],
    ));
}

/// Full height, and a width stated in viewport heights rather than through
/// `aspect_ratio`: with that, Taffy re-derived the height from a clamped width,
/// and on a portrait window the readouts floated halfway up the screen.
/// Narrower than 4:3, `max_width` makes it the window.
fn safe_zone(font: &Handle<Font>) -> impl Bundle {
    (
        SafeZone,
        Node {
            height: Val::Percent(100.0),
            width: Val::Vh(100.0 * SAFE_ZONE_ASPECT),
            max_width: Val::Percent(100.0),
            ..default()
        },
        // Drawn inside the box, so it is on screen when the box is the window.
        Outline::new(Val::Px(2.0), Val::Px(-2.0), Color::NONE),
        Pickable::IGNORE,
        children![health_readout(font), armor_readout(font), weapon_readout(font)],
    )
}

/// Four bars around a gap, dead center. The arms hang off a zero-size node
/// centered by flexbox, so each is a pixel offset from the exact middle rather
/// than a percentage that lands half a pixel out.
fn crosshair() -> impl Bundle {
    fn arm(node: Node) -> impl Bundle {
        (
            Node { position_type: PositionType::Absolute, ..node },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
        )
    }

    let gap = Val::Px(CROSSHAIR_GAP);
    let long = Val::Px(CROSSHAIR_ARM);
    let thick = Val::Px(CROSSHAIR_THICKNESS);
    let centered = Val::Px(-CROSSHAIR_THICKNESS * 0.5);

    (
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        Pickable::IGNORE,
        children![(
            Node::default(),
            children![
                arm(Node { right: gap, top: centered, width: long, height: thick, ..default() }),
                arm(Node { left: gap, top: centered, width: long, height: thick, ..default() }),
                arm(Node { bottom: gap, left: centered, width: thick, height: long, ..default() }),
                arm(Node { top: gap, left: centered, width: thick, height: long, ..default() }),
            ],
        )],
    )
}

pub(crate) fn hud_text(font: &Handle<Font>, words: String, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(words),
        TextFont { font: font.into(), font_size: bevy::text::FontSize::Px(size), ..default() },
        TextColor(color),
        Pickable::IGNORE,
    )
}

fn text(font: &Handle<Font>, kind: HudText, size: f32, alpha: f32) -> impl Bundle {
    (kind, hud_text(font, String::new(), size, Color::srgba(1.0, 1.0, 1.0, alpha)))
}

fn health_readout(font: &Handle<Font>) -> impl Bundle {
    (
        Node {
            left: Val::Px(MARGIN_X),
            // Sits low enough that the panel cuts across the number.
            padding: UiRect { right: Val::Px(ZONE_DROP.x), bottom: Val::Px(HEALTH_PADDING_BOTTOM), ..default() },
            ..HudZone::Health.readout()
        },
        Pickable::IGNORE,
        children![HudZone::Health.panel(), text(font, HudText::Health, BIG, 1.0)],
    )
}

/// Right of the health panel, the points on the health number's baseline and
/// what they are hanging under it, out of the flow so they move nothing.
fn armor_readout(font: &Handle<Font>) -> impl Bundle {
    // A line box puts its baseline this far above its bottom, so a smaller
    // number sits that much higher to share the bigger one's baseline.
    let lift = (BIG - MEDIUM) * BASELINE_ABOVE_LINE_BOTTOM;
    let baseline = HEALTH_PADDING_BOTTOM + BIG * BASELINE_ABOVE_LINE_BOTTOM;
    (
        ArmorReadout,
        Node {
            left: Val::Px(MARGIN_X + HudZone::Health.size().x + ZONE_GAP),
            padding: UiRect {
                right: Val::Px(ZONE_DROP.x),
                bottom: Val::Px(HEALTH_PADDING_BOTTOM + lift),
                ..default()
            },
            display: Display::None,
            ..HudZone::Armor.readout()
        },
        Pickable::IGNORE,
        children![
            HudZone::Armor.panel(),
            text(font, HudText::Armor, MEDIUM, 1.0),
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(ZONE_DROP.x),
                    bottom: Val::Px(baseline - LABEL_DROP - SMALL * BASELINE_ABOVE_LINE_BOTTOM),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Pickable::IGNORE,
                children![kind_label(font)],
            ),
        ],
    )
}

/// Marks the "armor" under the points, so a change of language reaches it.
#[derive(Component)]
struct ArmorLabel;

fn kind_label(font: &Handle<Font>) -> impl Bundle {
    (ArmorLabel, hud_text(font, get!("hud.armor"), SMALL, Color::BLACK))
}

/// A reload notice when there is one, the loaded count large and the reserve
/// small beside it on one baseline, then the name. The notice goes on top so
/// it pushes nothing the panel is lined up against.
fn weapon_readout(font: &Handle<Font>) -> impl Bundle {
    (
        Node {
            right: Val::Px(MARGIN_X),
            row_gap: Val::Px(2.0),
            ..HudZone::Ammo.readout()
        },
        Pickable::IGNORE,
        children![
            HudZone::Ammo.panel(),
            (text(font, HudText::Reloading, SMALL, 0.85), Node { display: Display::None, ..default() }),
            (
                Node {
                    align_items: AlignItems::Baseline,
                    column_gap: Val::Px(8.0),
                    ..default()
                },
                Pickable::IGNORE,
                children![
                    text(font, HudText::Loaded, BIG, 1.0),
                    text(font, HudText::Reserve, SMALL, 0.85),
                ],
            ),
            text(font, HudText::WeaponName, SMALL, 0.85),
        ],
    )
}

/// Separate from spawning because there are no material assets in a test app
/// with no renderer; the readouts still work without their panels.
fn dress_the_zones(
    mut commands: Commands,
    zones: Query<(Entity, &HudZone), Added<HudZone>>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let Some(mut materials) = materials else { return };
    for (entity, &zone) in &zones {
        let material = ShapeMaterial::from_path(
            &zone.outline(),
            zone.size(),
            HudZone::style(&Swatch::neutral()),
        );
        commands.entity(entity).insert(MaterialNode(materials.add(material)));
    }
}

/// Every frame, comparing first: a client is told its team after its body.
#[allow(clippy::type_complexity)]
fn paint_the_zones(
    ours: Option<Single<(Option<&Team>, Option<&PlayerColor>), With<LocalPlayer>>>,
    zones: Query<&MaterialNode<ShapeMaterial>, With<HudZone>>,
    mut texts: Query<&mut TextColor, With<HudText>>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
) {
    let swatch = ours.map_or_else(Swatch::neutral, |ours| {
        let (team, color) = ours.into_inner();
        Swatch::of_body(team.copied(), color.copied())
    });
    let style = HudZone::style(&swatch);
    if let Some(mut materials) = materials {
        for zone in &zones {
            if materials.get(&zone.0).is_some_and(|m| m.style != style) {
                if let Some(mut material) = materials.get_mut(&zone.0) {
                    material.style = style;
                }
            }
        }
    }

    let ink = swatch.text();
    for mut color in &mut texts {
        let wanted = ink.with_alpha(color.0.alpha());
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

fn toggle_safe_zone_outline(
    keys: Res<ButtonInput<KeyCode>>,
    mut outline: ResMut<SafeZoneOutline>,
) {
    if keys.just_pressed(KeyCode::F9) {
        outline.0 = !outline.0;
    }
}

/// Color rather than insert-and-remove, which would move tables each toggle.
/// Every frame rather than on change, so a HUD spawned while it is on starts
/// outlined.
fn outline_the_safe_zone(
    shown: Res<SafeZoneOutline>,
    mut zones: Query<&mut Outline, With<SafeZone>>,
) {
    let color = if shown.0 { Color::srgba(1.0, 0.8, 0.0, 0.8) } else { Color::NONE };
    for mut outline in &mut zones {
        if outline.color != color {
            outline.color = color;
        }
    }
}

/// What each readout should say.
#[derive(Debug, Default, PartialEq)]
struct Readout {
    health: String,
    armor: String,
    name: String,
    reloading: bool,
    loaded: String,
    reserve: String,
}

impl Readout {
    fn of(
        catalog: &WeaponCatalog,
        health: &Damageable,
        armor: Option<&Armor>,
        equipped: &Equipped,
        ammo: &Ammo,
    ) -> Self {
        let health = health.health().to_string();
        let armor = match armor.map(Armor::points) {
            Some(points) if points > 0 => points.to_string(),
            _ => String::new(),
        };
        let Some(weapon) = catalog.held_by(equipped) else {
            return Self { health, armor, ..default() };
        };

        let state = ammo.at(equipped.held);
        let (loaded, reserve) = match weapon.magazine.clip {
            // Nothing is loaded, so the reserve is the number that matters.
            None => (state.reserve.to_string(), String::new()),
            Some(_) => (state.clip.to_string(), state.reserve.to_string()),
        };

        Self {
            health,
            armor,
            name: get!(weapon.name_key),
            reloading: state.reloading > 0.0,
            loaded,
            reserve,
        }
    }

    fn text(&self, kind: HudText) -> &str {
        match kind {
            HudText::Health => &self.health,
            HudText::Armor => &self.armor,
            HudText::WeaponName => &self.name,
            HudText::Reloading => "",
            HudText::Loaded => &self.loaded,
            HudText::Reserve => &self.reserve,
        }
    }
}

/// Compares before writing: a written `Text` or `Node` counts as changed and
/// would be re-laid-out every frame.
fn fill_in_the_readouts(
    catalog: Res<WeaponCatalog>,
    body: Option<Single<(&Damageable, Option<&Armor>, &Equipped, &Ammo), With<LocalPlayer>>>,
    mut texts: Query<(&HudText, &mut Text, &mut Node)>,
    mut armor_readout: Query<&mut Node, (With<ArmorReadout>, Without<HudText>)>,
    mut armor_label: Query<&mut Text, (With<ArmorLabel>, Without<HudText>)>,
) {
    let readout = match body.as_deref() {
        Some((health, armor, equipped, ammo)) => Readout::of(&catalog, health, *armor, equipped, ammo),
        None => Readout::default(),
    };

    // The panel goes with the number, so both are hidden from one node.
    let display = if readout.armor.is_empty() { Display::None } else { Display::Flex };
    for mut node in &mut armor_readout {
        if node.display != display {
            node.display = display;
        }
    }
    let label = get!("hud.armor");
    for mut text in &mut armor_label {
        if text.0 != label {
            text.0 = label.clone();
        }
    }

    for (&kind, mut text, mut node) in &mut texts {
        if kind == HudText::Reloading {
            // Looked up while showing rather than once, so a change of
            // language reaches it.
            if readout.reloading {
                let label = get!("hud.reloading");
                if text.0 != label {
                    text.0 = label;
                }
            }
            let display = if readout.reloading { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
            continue;
        }

        let wanted = readout.text(kind);
        if text.0 != wanted {
            text.0 = wanted.to_owned();
        }
    }
}

fn despawn_the_hud(mut commands: Commands, roots: Query<Entity, With<HudRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::Slot;
    use crate::common::weapon_file::default_catalog;

    fn mercenary() -> (WeaponCatalog, Equipped, Ammo) {
        let catalog = default_catalog();
        let (equipped, ammo) =
            catalog.starting_equipment(crate::common::class::Class::Mercenary, None);
        (catalog, equipped, ammo)
    }

    #[test]
    fn armor_is_shown_only_while_there_is_some() {
        let (catalog, equipped, ammo) = mercenary();
        let health = Damageable::default();
        let mut armor = Armor::default();

        let none = Readout::of(&catalog, &health, Some(&armor), &equipped, &ammo);
        assert!(none.armor.is_empty());
        armor.add(35);
        let some = Readout::of(&catalog, &health, Some(&armor), &equipped, &ammo);
        assert!(some.armor.contains("35"), "{}", some.armor);
    }

    #[test]
    fn the_armor_panel_shows_only_while_there_is_armor() {
        use bevy::ecs::system::RunSystemOnce;

        let (catalog, equipped, ammo) = mercenary();
        let mut world = World::new();
        world.insert_resource(catalog);
        let body = world.spawn((LocalPlayer, Damageable::default(), Armor::default(), equipped, ammo)).id();
        let readout = world.spawn((ArmorReadout, Node::default())).id();
        let shown = |world: &mut World| {
            world.run_system_once(fill_in_the_readouts).unwrap();
            world.get::<Node>(readout).unwrap().display != Display::None
        };

        assert!(!shown(&mut world), "shown with no armor");
        world.get_mut::<Armor>(body).unwrap().add(50);
        assert!(shown(&mut world), "hidden with armor");
    }

    /// Breaks if the lookup panics when `Equipped` arrives before the
    /// catalog, which is the ordinary order on a client.
    #[test]
    fn a_body_whose_weapon_has_not_arrived_yet_is_drawn_blank() {
        let (_, equipped, ammo) = mercenary();
        let health = Damageable::with_health(125);

        let readout = Readout::of(&WeaponCatalog::default(), &health, None, &equipped, &ammo);
        assert_eq!(
            readout,
            Readout { health: "125".into(), ..default() },
            "a weapon nobody can name was described anyway",
        );
    }

    #[test]
    fn a_weapon_with_a_clip_shows_both_counts() {
        let (catalog, mut equipped, ammo) = mercenary();
        equipped.select(Slot::Secondary);
        let state = ammo.at(Slot::Secondary);

        let readout = Readout::of(&catalog, &Damageable::default(), None, &equipped, &ammo);
        assert_eq!(readout.loaded, state.clip.to_string());
        assert_eq!(readout.reserve, state.reserve.to_string());
    }

    #[test]
    fn a_weapon_with_no_clip_does_not_read_as_zero_of_something() {
        let (catalog, mut equipped, ammo) = mercenary();
        equipped.select(Slot::Utility);
        let state = ammo.at(Slot::Utility);

        let readout = Readout::of(&catalog, &Damageable::default(), None, &equipped, &ammo);
        assert_eq!(readout.loaded, state.reserve.to_string());
        assert_eq!(readout.reserve, "", "a weapon with no clip showed a second count");
    }

    #[test]
    fn a_reloading_weapon_says_so_and_keeps_its_counts() {
        let (catalog, equipped, mut ammo) = mercenary();
        ammo.at_mut(equipped.held).reloading = 0.5;

        let readout = Readout::of(&catalog, &Damageable::default(), None, &equipped, &ammo);
        assert!(readout.reloading);
        assert!(!readout.loaded.is_empty(), "the count vanished during a reload");
    }
}
