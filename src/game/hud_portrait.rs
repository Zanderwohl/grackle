//! The HUD's portrait: the viewed body's class, in its colors and hat, idling with the weapon it
//! holds, on a [`Medallion`] behind the health panel, painted in the complement of the HUD's color
//! ([`Swatch::complement`]). Spectating, it is the watched body's.
//!
//! A copy of the body stands far below the map on [`render_layers::HUD_PORTRAIT`], drawn by an
//! orthographic camera into the medallion's image. The copy has only a `Class`, a [`BodyTint`]
//! and an `Equipped`, which the held-weapon systems hang a model from. It has no `Player` or
//! hitboxes, so nothing that looks for bodies finds it. A class change stands a new copy, since a
//! rig is built once.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{RenderTarget, ScalingMode};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::common::app_mode::AppMode;
use crate::common::class::{Class, ClassBuilds};
use crate::common::player_color::PlayerColor;
use crate::common::render_layers;
use crate::common::skeleton::state::{AnimationPhase, BodyRequests, SkeletonAnimator};
use crate::common::skeleton::Pose;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::common::weapon::Equipped;
use crate::game::body_mesh::{body_look, BodyTint};
use crate::game::class_picker::PortraitBody;
use crate::game::medallion::{cut_height, medallion_box, Medallion};
use crate::game::player::{LocalPlayer, Player, Viewed};
use crate::prop::thumbnail::{AMBIENT, BACK_FROM, BACK_LUX, FILL_FROM, FILL_LUX, KEY_FROM, KEY_LUX};

/// The medallion's diameter in logical pixels, before its drop shadow.
pub const PORTRAIT_SIZE: f32 = 150.0;
/// The share of the disk kept above the cut, which the health panel hides.
pub const PORTRAIT_KEPT: f32 = 0.8;
const IMAGE_SCALE: f32 = 2.0;
/// Logical pixels the image runs past the disk on the left, the right and the top, so a weapon
/// held up beside the head is drawn whole rather than cut at the disk's box. Not below, where the
/// health panel hides the cut. The body is framed against the disk as if this were zero.
const OVERHANG: f32 = 60.0;
/// Meters of the body shown top to bottom, at every class's scale alike.
const VIEW_HEIGHT: f32 = 1.05;
/// Above the highest bone, for the crown and a hat's brim.
const HEAD_ROOM: f32 = 0.22;
/// Far under the map, and clear of the class picker's studio on another `z`, so neither camera's
/// box reaches the other's body.
const STUDIO: Vec3 = Vec3::new(20_000.0, -10_000.0, 18_000.0);
const CAMERA_DISTANCE: f32 = 6.0;
/// Radians the body is turned from facing the camera, so the weapon held before its chest is
/// seen from the side rather than end on.
const TURN: f32 = -0.7;
/// Below the class picker's cameras.
const CAMERA_ORDER: isize = -40;

#[derive(Component)]
pub struct HudPortraitImage;

/// The medallion [`show_the_portrait`] paints, so other medallions keep their own swatch.
#[derive(Component)]
pub struct HudPortraitMedallion;

#[derive(Resource, Default)]
pub struct HudPortrait {
    image: Option<Handle<Image>>,
    camera: Option<Entity>,
    lights: Vec<Entity>,
    body: Option<(Entity, Class)>,
}

pub struct HudPortraitPlugin;

impl Plugin for HudPortraitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudPortrait>()
            // In `PreUpdate`, so a copy replaced on a class change is gone before `Update`'s
            // systems queue writes to its parts.
            .add_systems(
                PreUpdate,
                (stand_the_studio, sit_for_the_portrait, show_the_portrait).chain().run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), take_the_studio_down);
    }
}

/// Logical pixels: as tall as what is kept of the disk, so the body ends at the cut, behind the
/// health panel.
pub fn image_size() -> Vec2 {
    Vec2::new(PORTRAIT_SIZE, cut_height(medallion_box(PORTRAIT_SIZE), PORTRAIT_KEPT))
}

/// Logical pixels: [`image_size`] with the [`OVERHANG`] round it.
fn canvas_size() -> Vec2 {
    image_size() + Vec2::new(2.0 * OVERHANG, OVERHANG)
}

/// Meters of the body per logical pixel of the image.
fn meters_per_pixel() -> f32 {
    VIEW_HEIGHT / image_size().y
}

pub fn portrait_image() -> impl Bundle {
    let size = canvas_size();
    (
        HudPortraitImage,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(-OVERHANG),
            top: Val::Px(-OVERHANG),
            width: Val::Px(size.x),
            height: Val::Px(size.y),
            ..default()
        },
        ImageNode::default(),
        Pickable::IGNORE,
    )
}

fn stand_the_studio(mut commands: Commands, mut portrait: ResMut<HudPortrait>, images: Option<ResMut<Assets<Image>>>) {
    if portrait.camera.is_some() {
        return;
    }
    // No renderer in the game's tests.
    let Some(mut images) = images else { return };
    let layer = RenderLayers::layer(render_layers::HUD_PORTRAIT);
    // Looking along +Z at a body whose rest faces -Z.
    let looking = Transform::default().looking_to(Vec3::Z, Vec3::Y).rotation;
    for (from, lux) in [(KEY_FROM, KEY_LUX), (FILL_FROM, FILL_LUX), (BACK_FROM, BACK_LUX)] {
        let light = commands
            .spawn((
                DirectionalLight { illuminance: lux, shadow_maps_enabled: false, ..default() },
                Transform::default().looking_to(-(looking * from), Vec3::Y),
                layer.clone(),
                Name::new("HUD portrait light"),
            ))
            .id();
        portrait.lights.push(light);
    }

    let logical = canvas_size();
    let pixels = (logical * IMAGE_SCALE).as_uvec2();
    let image = images.add(Image::new_target_texture(pixels.x, pixels.y, TextureFormat::Rgba8UnormSrgb, None));
    let view = logical * meters_per_pixel();
    // The same view settings as the class picker's portraits, whose pipelines are warmed up.
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera { order: CAMERA_ORDER, clear_color: ClearColorConfig::Custom(Color::NONE), is_active: false, ..default() },
            RenderTarget::Image(image.clone().into()),
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed { width: view.x, height: view.y },
                near: 0.0,
                far: CAMERA_DISTANCE * 3.0,
                ..OrthographicProjection::default_3d()
            }),
            AmbientLight { brightness: AMBIENT, ..default() },
            Transform::from_rotation(looking),
            layer,
            Name::new("HUD portrait camera"),
        ))
        .id();
    portrait.camera = Some(camera);
    portrait.image = Some(image);
}

#[allow(clippy::type_complexity)]
fn sit_for_the_portrait(
    mut commands: Commands,
    mut portrait: ResMut<HudPortrait>,
    builds: Res<ClassBuilds>,
    viewed: Query<((&Class, Option<&Team>, Option<&PlayerColor>, &Equipped), Has<LocalPlayer>), (Viewed, With<Player>)>,
    mut sitters: Query<(&mut Equipped, &mut BodyTint), (With<PortraitBody>, Without<Player>)>,
    mut cameras: Query<(&mut Camera, &mut Transform)>,
) {
    // Ours over a watched body in the frame both exist, as when a respawn hands us a body while
    // the view is still on our killer.
    let viewed = viewed.iter().max_by_key(|(_, ours)| *ours).map(|(body, _)| body);
    if let Some(camera) = portrait.camera
        && let Ok((mut camera, _)) = cameras.get_mut(camera)
    {
        let active = viewed.is_some();
        if camera.is_active != active {
            camera.is_active = active;
        }
    }
    let Some((&class, team, color, equipped)) = viewed else { return };
    let tint = BodyTint(body_look(team, color, None, None).color);

    if portrait.body.is_some_and(|(_, sitting)| sitting != class) {
        if let Some((body, _)) = portrait.body.take() {
            commands.entity(body).despawn();
        }
    }
    let Some((body, _)) = portrait.body else {
        let turned = Quat::from_rotation_y(TURN);
        let body = commands
            .spawn((
                PortraitBody,
                class,
                // Rigged here rather than by `dress_bodies_from_elsewhere`, which would give it
                // the hitboxes every shot and blast tests against.
                builds.rig_for(class),
                Pose::rest(),
                SkeletonAnimator::default(),
                BodyRequests::default(),
                AnimationPhase::default(),
                equipped.clone(),
                tint,
                Transform::from_translation(STUDIO).with_rotation(turned),
                Visibility::default(),
                RenderLayers::layer(render_layers::HUD_PORTRAIT),
                Name::new(format!("HUD portrait: {class:?}")),
            ))
            .id();
        portrait.body = Some((body, class));
        if let Some(camera) = portrait.camera
            && let Ok((_, mut transform)) = cameras.get_mut(camera)
        {
            transform.translation = eye(&builds, class, turned);
        }
        return;
    };

    let Ok((mut held, mut painted)) = sitters.get_mut(body) else { return };
    if *held != *equipped {
        *held = equipped.clone();
    }
    painted.set_if_neq(tint);
}

/// Where the camera stands for `class` turned by `turned`: level with the middle of the disk's
/// view, whose top is [`HEAD_ROOM`] over the highest bone, across from the middle of what it
/// shows, and raised by half the [`OVERHANG`], which runs only above it.
fn eye(builds: &ClassBuilds, class: Class, turned: Quat) -> Vec3 {
    let bones = builds.rig_for(class).posed_bones(&Pose::rest(), &Transform::from_rotation(turned));
    let points: Vec<Vec3> = bones.iter().flat_map(|bone| [bone.head, bone.tail]).collect();
    let top = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    let top = if top.is_finite() { top + HEAD_ROOM } else { VIEW_HEIGHT };
    let bottom = top - VIEW_HEIGHT;
    let shown = points.iter().filter(|p| p.y >= bottom);
    let (low, high) = shown.fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), p| (low.min(p.x), high.max(p.x)));
    let across = if low.is_finite() { (low + high) * 0.5 } else { 0.0 };
    let raised = OVERHANG * 0.5 * meters_per_pixel();
    STUDIO + Vec3::new(across, top - VIEW_HEIGHT * 0.5 + raised, -CAMERA_DISTANCE)
}

#[allow(clippy::type_complexity)]
fn show_the_portrait(
    portrait: Res<HudPortrait>,
    viewed: Query<((Option<&Team>, Option<&PlayerColor>), Has<LocalPlayer>), (Viewed, With<Player>)>,
    mut medallions: Query<&mut Medallion, With<HudPortraitMedallion>>,
    mut images: Query<&mut ImageNode, With<HudPortraitImage>>,
    mut painted: Local<Option<(Option<Team>, Option<PlayerColor>, Swatch)>>,
) {
    let viewed = viewed.iter().max_by_key(|(_, ours)| *ours).map(|(body, _)| body);
    let (team, color) = viewed.map_or((None, None), |(team, color)| (team.copied(), color.copied()));
    // Rebuilt only on change, since each tone may bisect for its gamut.
    let swatch = match *painted {
        Some((was_team, was_color, swatch)) if (was_team, was_color) == (team, color) => swatch,
        _ => {
            let swatch = Swatch::of_body(team, color).complement();
            *painted = Some((team, color, swatch));
            swatch
        }
    };
    for mut medallion in &mut medallions {
        if medallion.swatch != swatch {
            medallion.swatch = swatch;
        }
    }
    let Some(image) = &portrait.image else { return };
    for mut node in &mut images {
        if node.image != *image {
            node.image = image.clone();
        }
    }
}

fn take_the_studio_down(mut commands: Commands, mut portrait: ResMut<HudPortrait>, images: Option<ResMut<Assets<Image>>>) {
    let portrait = std::mem::take(&mut *portrait);
    for entity in portrait.lights.into_iter().chain(portrait.camera).chain(portrait.body.map(|(body, _)| body)) {
        commands.entity(entity).despawn();
    }
    if let (Some(image), Some(mut images)) = (portrait.image, images) {
        images.remove(&image);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::WeaponId;
    use crate::game::player::{LocalPlayer, Watched};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .insert_resource(ClassBuilds::shipped())
            .add_plugins(HudPortraitPlugin);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    fn sitter(app: &mut App) -> Option<(Class, Equipped, BodyTint)> {
        let world = app.world_mut();
        let mut sitters = world.query_filtered::<(&Class, &Equipped, &BodyTint), With<PortraitBody>>();
        let found: Vec<_> = sitters.iter(world).map(|(c, e, t)| (*c, e.clone(), *t)).collect();
        assert!(found.len() <= 1, "{} portrait bodies", found.len());
        found.into_iter().next()
    }

    /// Breaks if the portrait does not copy the viewed body's class, color and weapon, follows a
    /// switch late, or keeps a body of the old class after a class change.
    #[test]
    fn the_portrait_is_the_viewed_body_and_follows_its_weapon_and_class() {
        let mut app = app();
        assert_eq!(sitter(&mut app), None, "a portrait with nobody to sit for it");

        let rifle = WeaponId::of("rifle");
        let pistol = WeaponId::of("pistol");
        let body = app
            .world_mut()
            .spawn((Player::default(), LocalPlayer, Class::Scout, Team::Blue, Equipped::carrying([rifle, pistol])))
            .id();
        app.update();
        let (class, equipped, tint) = sitter(&mut app).expect("no portrait of our body");
        assert_eq!((class, equipped.held(), tint.0), (Class::Scout, Some(rifle), Team::Blue.color()));

        app.world_mut().get_mut::<Equipped>(body).unwrap().select(pistol);
        app.update();
        assert_eq!(sitter(&mut app).unwrap().1.held(), Some(pistol), "the switch did not reach the portrait");

        app.world_mut().entity_mut(body).insert(Class::Heavy);
        app.update();
        app.update();
        assert_eq!(sitter(&mut app).unwrap().0, Class::Heavy);
    }

    /// Breaks if, in the frame the class changes, `Update` still sees the old copy, whose parts
    /// a system there may write to after its despawn.
    #[test]
    fn a_class_change_swaps_the_copy_before_update() {
        #[derive(Resource, Default)]
        struct SeenInUpdate(Vec<Class>);
        let mut app = app();
        app.init_resource::<SeenInUpdate>().add_systems(
            Update,
            |sitters: Query<&Class, With<PortraitBody>>, mut seen: ResMut<SeenInUpdate>| {
                seen.0 = sitters.iter().copied().collect();
            },
        );
        let body = app.world_mut().spawn((Player::default(), LocalPlayer, Class::Scout, Equipped::default())).id();
        app.update();
        assert_eq!(app.world().resource::<SeenInUpdate>().0, [Class::Scout]);

        app.world_mut().entity_mut(body).insert(Class::Heavy);
        app.update();
        assert_eq!(app.world().resource::<SeenInUpdate>().0, [Class::Heavy]);
    }

    /// Breaks if a spectator sees anybody but the watched body.
    #[test]
    fn a_spectator_sees_the_watched_bodys_portrait() {
        let mut app = app();
        app.world_mut().spawn((Player::default(), Class::Scout, Team::Red, Equipped::default()));
        app.world_mut().spawn((Player::default(), Watched, Class::Heavy, Team::Blue, Equipped::default()));
        app.update();
        let (class, _, tint) = sitter(&mut app).unwrap();
        assert_eq!((class, tint.0), (Class::Heavy, Team::Blue.color()));
    }

    /// Breaks if the frame both our body and a watched one exist shows the watched one, or none.
    #[test]
    fn our_body_wins_over_a_watched_one_in_the_frame_both_exist() {
        let mut app = app();
        app.world_mut().spawn((Player::default(), Watched, Class::Heavy, Equipped::default()));
        app.world_mut().spawn((Player::default(), LocalPlayer, Class::Scout, Equipped::default()));
        app.update();
        assert_eq!(sitter(&mut app).map(|(class, ..)| class), Some(Class::Scout));
    }

    /// Breaks if a medallion that is not the portrait's is repainted.
    #[test]
    fn only_the_portraits_medallion_is_painted() {
        let mut app = app();
        let theirs = app.world_mut().spawn(Medallion::new(Swatch::neutral(), 1.0)).id();
        let ours = app.world_mut().spawn((HudPortraitMedallion, Medallion::new(Swatch::neutral(), 1.0))).id();
        app.world_mut().spawn((Player::default(), LocalPlayer, Class::Scout, Team::Red, Equipped::default()));
        app.update();
        let swatch = |entity| app.world().get::<Medallion>(entity).unwrap().swatch;
        assert_eq!(swatch(ours), Swatch::team(Team::Red).complement());
        assert_eq!(swatch(theirs), Swatch::neutral());
    }

    /// Breaks if leaving the round leaves the copy standing.
    #[test]
    fn leaving_play_takes_the_portrait_down() {
        let mut app = app();
        app.world_mut().spawn((Player::default(), LocalPlayer, Class::Scout, Equipped::default()));
        app.update();
        assert!(sitter(&mut app).is_some());
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        assert_eq!(sitter(&mut app), None);
    }
}
