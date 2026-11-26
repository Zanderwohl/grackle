//! The class picker: a row of buttons on a plate, one per class the mode allows, each showing
//! its choice key, a portrait and the class's name. Up while a class is owed (after a seat is
//! taken, when the mode offers more than one), and on `,` after. A digit or a click chooses at
//! once. What a choice does is [`crate::game::class_choice`]; this only asks.
//!
//! Each portrait is an idling body far below the map on [`render_layers::PORTRAIT`], drawn live
//! by an orthographic camera into the image its button shows. An orthographic view is a box, so
//! with the bodies [`SPACING`] apart no camera sees another's body. The studio stands only while
//! the picker is up. A body wears its viewer's team color, else their own, and loops its class's
//! `picker_taunt` while the pointer is over its button.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{RenderTarget, ScalingMode};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::common::app_mode::AppMode;
use crate::common::class::{Class, ClassBuilds};
use crate::common::fonts::DisplayFont;
use crate::common::keybind::{Action, Actions, Keymap};
use crate::common::render_layers;
use crate::common::skeleton::state::{AnimationPhase, BodyRequests, SkeletonAnimator};
use crate::common::skeleton::Pose;
use crate::common::swatch::Swatch;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{ClipStack, Layer, Playing};
use crate::common::skeleton::Skeleton;
use crate::common::taunt::{TauntId, Taunts};
use crate::common::team::Team;
use crate::common::weapon::{WeaponCatalog, DEFAULT_PICKER_TAUNT};
use crate::game::condition::ConditionClips;
use bevy::picking::hover::Hovered;
use crate::game::body_mesh::BodyTint;
use crate::game::hud::{WINDOW_MARGIN, ZONE_DROP, ZONE_STROKE};
use crate::game::menu::{menu_panel, menu_title, ITEM_SIZE};
use crate::game::pause_menu::{EscapeTaken, MouseHeld, PauseMenu, TakeEscape};
use crate::game::plate::{plate_text, FollowsTheLean, Plate};
use crate::game::plate_button::{plate_button_node, ButtonActivated, PlateButton};
use crate::game::player::LocalPlayer;
use crate::game::class_choice::{ChooseClass, ClassOptions};
use crate::game::team_choice::TeamOptions;
use crate::game::team_picker::TeamPicker;

/// Whether `,` asked for the picker; [`MouseHeld::class_picker`] is whether it is up.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClassPicker {
    pub wanted: bool,
}

impl ClassPicker {
    /// A seated player owed a class may not walk into the match without one.
    pub fn bars_the_way(seats: &TeamOptions, classes: &ClassOptions) -> bool {
        seats.seat.is_playing() && classes.must_choose()
    }

    /// Never over the team picker, which comes first.
    pub fn is_up(&self, seats: &TeamOptions, classes: &ClassOptions, team_picker_up: bool) -> bool {
        !team_picker_up && !classes.allowed.is_empty() && (self.wanted || Self::bars_the_way(seats, classes))
    }

    pub fn closable(&self, seats: &TeamOptions, classes: &ClassOptions) -> bool {
        self.wanted && !Self::bars_the_way(seats, classes)
    }
}

/// Under the pause menu, beside the team picker, which is never up at the same time.
const PICKER_LAYER: i32 = 10;
const BUTTON_GAP: f32 = 10.0;
/// Logical pixels, the same for every class, so the row's width is the count of classes alone.
const BUTTON_WIDTH: f32 = 150.0;
/// Inside the stroke, each side; the far side also clears the drop shadow.
const BUTTON_SIDE: f32 = 14.0;
/// The widest a name may be: the button less its padding. The lean needs no room, since
/// [`FollowsTheLean`] keeps a line its distance from the slanted sides.
const NAME_ROOM: f32 = BUTTON_WIDTH - 2.0 * (ZONE_STROKE + BUTTON_SIDE) - ZONE_DROP.x - 2.0 * NAME_CLEARANCE;
/// Logical pixels kept clear between a name and the stroke on each side.
const NAME_CLEARANCE: f32 = 4.0;
/// Bevy's line height, as a share of the font size.
const LINE_HEIGHT: f32 = 1.2;
/// Every name's largest size; the row is set at the size its longest name fits [`NAME_ROOM`] at.
const NAME_SIZE: f32 = 22.0;
const NAME_SMALLEST: f32 = 10.0;
/// Logical pixels; the image is drawn at [`PORTRAIT_SCALE`] times this.
const PORTRAIT_WIDTH: f32 = 100.0;
const PORTRAIT_HEIGHT: f32 = 140.0;
const PORTRAIT_SCALE: f32 = 2.0;
/// Meters of the body shown top to bottom, at every class's scale alike, so a Heavy stands taller
/// than a Scout. Past the tallest class, with room for the crown.
const VIEW_HEIGHT: f32 = 2.4;
/// Below the feet.
const FLOOR_MARGIN: f32 = 0.08;
/// Far under the map and the weapon editor's range, so no shot or blast reaches a body here.
const STUDIO: Vec3 = Vec3::new(20_000.0, -10_000.0, 20_000.0);
/// Meters between bodies, far wider than any portrait's view.
const SPACING: f32 = 20.0;
const CAMERA_DISTANCE: f32 = 6.0;
/// Radians the body is turned from facing the camera, so its silhouette has depth.
const TURN: f32 = -0.45;
/// Below the thumbnail studio's, each portrait one lower.
const FIRST_CAMERA_ORDER: isize = -10;
// The thumbnail studio's lighting, in the camera's frame, so a portrait and a prop's picture
// are lit alike.
use crate::prop::thumbnail::{AMBIENT, BACK_FROM, BACK_LUX, FILL_FROM, FILL_LUX, KEY_FROM, KEY_LUX};

#[derive(Component)]
struct PickerRoot;

#[derive(Component, Clone, Copy)]
struct ClassItem(Class);

/// The plate the title and the row stand on, scaled down to stay inside the window safe zone.
#[derive(Component)]
struct PickerPanel;

/// A class's name, set smaller until it fits [`NAME_ROOM`].
#[derive(Component)]
struct ClassName;

/// A body standing for a portrait, here or on the HUD. Its parts, hat and held weapon take its own
/// `RenderLayers` ([`render_layers::part_layers`]).
#[derive(Component)]
pub struct PortraitBody;

/// Everything the portraits stand on, despawned with the picker.
#[derive(Resource, Default)]
struct PortraitStudio {
    entities: Vec<Entity>,
    images: Vec<Handle<Image>>,
    bodies: Vec<(Class, Entity)>,
}

pub struct ClassPickerPlugin;

impl Plugin for ClassPickerPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<ClassPicker>()
            .init_resource::<PortraitStudio>()
            .init_resource::<EscapeTaken>()
            .init_resource::<MouseHeld>()
            .init_resource::<DisplayFont>()
            .init_resource::<TeamOptions>()
            .init_resource::<TeamPicker>()
            .init_resource::<ClassOptions>()
            .add_message::<ChooseClass>()
            .add_observer(choose_by_clicking)
            .add_systems(
                Update,
                (
                    (escape_closes_the_picker.in_set(TakeEscape), toggle_the_picker, decide_the_picker, choose_by_key)
                        .chain()
                        .run_if(in_state(AppMode::Play)),
                    // Ungated, so leaving Play takes it down.
                    show_or_hide.run_if(
                        resource_changed::<MouseHeld>
                            .or_else(resource_changed::<ClassOptions>)
                            .or_else(resource_changed::<TeamOptions>)
                            .or_else(resource_changed::<Keymap>),
                    ),
                    taunt_while_hovered,
                    report_the_choice,
                )
                    .chain(),
            )
            .add_systems(PostUpdate, (fit_the_names, fit_the_panel).after(bevy::ui::UiSystems::Layout))
            .add_systems(OnExit(AppMode::Play), put_it_down);
    }
}


fn escape_closes_the_picker(
    actions: Res<Actions>,
    menu: Option<Res<PauseMenu>>,
    seats: Res<TeamOptions>,
    classes: Res<ClassOptions>,
    held: Res<MouseHeld>,
    mut picker: ResMut<ClassPicker>,
    mut taken: ResMut<EscapeTaken>,
) {
    let menu_open = menu.is_some_and(|menu| menu.open);
    if actions.just_pressed(Action::Pause) && !menu_open && held.class_picker && picker.closable(&seats, &classes) {
        *picker = ClassPicker::default();
        taken.0 = true;
    }
}

fn toggle_the_picker(actions: Res<Actions>, held: Res<MouseHeld>, mut picker: ResMut<ClassPicker>) {
    if actions.just_pressed(Action::ClassPicker) && !held.pause && !held.settings && !held.team_picker {
        picker.wanted = !picker.wanted;
    }
}

/// A class picker asked for is put down under the team picker, and `,` is ignored there, so it
/// does not come back by itself once a team is chosen.
fn decide_the_picker(
    mut picker: ResMut<ClassPicker>,
    team_picker: Res<TeamPicker>,
    seats: Res<TeamOptions>,
    classes: Res<ClassOptions>,
    mut held: ResMut<MouseHeld>,
) {
    let team_picker_up = team_picker.is_up(&seats);
    if team_picker_up && picker.wanted {
        picker.wanted = false;
    }
    let up = picker.is_up(&seats, &classes, team_picker_up);
    if held.class_picker != up {
        held.class_picker = up;
    }
}

fn put_it_down(mut picker: ResMut<ClassPicker>, mut held: ResMut<MouseHeld>) {
    *picker = ClassPicker::default();
    held.class_picker = false;
}

/// Not while the pause menu or the Settings window is over the picker.
fn choose_by_key(
    actions: Res<Actions>,
    held: Res<MouseHeld>,
    classes: Res<ClassOptions>,
    mut picker: ResMut<ClassPicker>,
    mut requests: MessageWriter<ChooseClass>,
) {
    if !held.class_picker || held.pause || held.settings {
        return;
    }
    let Some(index) = Action::CHOICES.iter().position(|action| actions.just_pressed(*action)) else { return };
    let Some(&class) = classes.allowed.get(index) else { return };
    ask_for(class, &mut picker, &mut requests);
}

fn choose_by_clicking(
    activated: On<ButtonActivated>,
    items: Query<&ClassItem>,
    mut picker: ResMut<ClassPicker>,
    mut requests: MessageWriter<ChooseClass>,
) {
    let Ok(&ClassItem(class)) = items.get(activated.entity) else { return };
    ask_for(class, &mut picker, &mut requests);
}

/// Closes a picker that was asked for; one that bars the way stays up until the choice lands.
fn ask_for(class: Class, picker: &mut ClassPicker, requests: &mut MessageWriter<ChooseClass>) {
    requests.write(ChooseClass(class));
    picker.wanted = false;
}

/// Told once the authority has taken a choice, so a refused one says nothing.
fn report_the_choice(classes: Res<ClassOptions>, ours: Query<(), With<LocalPlayer>>, mut last: Local<Option<Class>>) {
    if !classes.is_changed() || *last == classes.chosen {
        return;
    }
    // A toast once there are toasts: documentation/notifications_and_toasts.md.
    if let Some(class) = classes.chosen {
        match ours.is_empty() {
            false => info!("You will play the {} from your next spawn", class.name()),
            true => info!("You will spawn as the {}", class.name()),
        }
    }
    *last = classes.chosen;
}

/// The label of the first key bound to each of the first ten choices, blank where none is.
fn choice_labels(keymap: &Keymap) -> Vec<String> {
    Action::CHOICES.iter().map(|action| keymap.chords(*action).first().map(|chord| chord.label()).unwrap_or_default()).collect()
}

/// Rebuilt only when what it shows changes, so a hover is not reset.
#[allow(clippy::too_many_arguments)]
fn show_or_hide(
    mut commands: Commands,
    held: Res<MouseHeld>,
    classes: Res<ClassOptions>,
    seats: Res<TeamOptions>,
    keymap: Res<Keymap>,
    font: Res<DisplayFont>,
    builds: Option<Res<ClassBuilds>>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut studio: ResMut<PortraitStudio>,
    existing: Query<Entity, With<PickerRoot>>,
    mut shown: Local<Option<(bool, Vec<Class>, Option<Color>, Vec<String>)>>,
) {
    let labels = choice_labels(&keymap);
    let color = seats.seat.team().map(Team::color).or(classes.color.map(|color| color.color()));
    let wanted = (held.class_picker, classes.allowed.clone(), color, labels.clone());
    if shown.as_ref() == Some(&wanted) {
        return;
    }
    *shown = Some(wanted);
    for root in &existing {
        commands.entity(root).despawn();
    }
    take_the_studio_down(&mut commands, &mut studio, images.as_deref_mut());
    if !held.class_picker {
        return;
    }

    let portraits: Vec<Option<Handle<Image>>> = match (images.as_deref_mut(), builds.as_deref()) {
        (Some(images), Some(builds)) => {
            stand_the_portraits(&mut commands, &mut studio, images, builds, &classes.allowed, color)
        }
        _ => vec![None; classes.allowed.len()],
    };

    let font = &font.0;
    commands
        .spawn((
            PickerRoot,
            GlobalZIndex(PICKER_LAYER),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        ))
        .with_children(|root| {
            root.spawn((menu_panel(), PickerPanel, crate::game::postcards::KeptClear)).with_children(|menu| {
                menu.spawn(menu_title(font, crate::get!("class.picker.title")));
                menu.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(BUTTON_GAP), ..default() })
                    .with_children(|row| {
                        for (index, (&class, portrait)) in classes.allowed.iter().zip(portraits).enumerate() {
                            let label = labels.get(index).cloned().unwrap_or_default();
                            row.spawn((
                                ClassItem(class),
                                PlateButton::new(Swatch::neutral()),
                                Plate::new(Swatch::neutral()),
                                Pickable::default(),
                                Node {
                                    width: Val::Px(BUTTON_WIDTH),
                                    padding: UiRect {
                                        left: Val::Px(ZONE_STROKE + BUTTON_SIDE),
                                        right: Val::Px(ZONE_STROKE + ZONE_DROP.x + BUTTON_SIDE),
                                        ..plate_button_node().padding
                                    },
                                    ..plate_button_node()
                                },
                            ))
                            .with_children(|button| {
                                // A line tall even when no key is bound, so every button's rows line up.
                                button.spawn((
                                    plate_text(font, label, ITEM_SIZE),
                                    Node { min_height: Val::Px(ITEM_SIZE * LINE_HEIGHT), ..default() },
                                    FollowsTheLean,
                                ));
                                let mut picture = button.spawn((
                                    Node { width: Val::Px(PORTRAIT_WIDTH), height: Val::Px(PORTRAIT_HEIGHT), ..default() },
                                    FollowsTheLean,
                                    Pickable::IGNORE,
                                ));
                                if let Some(portrait) = portrait {
                                    picture.insert(ImageNode::new(portrait));
                                }
                                button.spawn((plate_text(font, class.name(), NAME_SIZE), TextLayout { linebreak: bevy::text::LineBreak::NoWrap, ..default() }, ClassName, FollowsTheLean));
                            });
                        }
                    });
            });
        });
}

fn take_the_studio_down(commands: &mut Commands, studio: &mut PortraitStudio, images: Option<&mut Assets<Image>>) {
    for entity in studio.entities.drain(..) {
        commands.entity(entity).despawn();
    }
    studio.bodies.clear();
    let images_gone = std::mem::take(&mut studio.images);
    if let Some(images) = images {
        for image in images_gone {
            images.remove(&image);
        }
    }
}

/// A body, a camera and an image per class, and one set of lights for them all.
fn stand_the_portraits(
    commands: &mut Commands,
    studio: &mut PortraitStudio,
    images: &mut Assets<Image>,
    builds: &ClassBuilds,
    classes: &[Class],
    color: Option<Color>,
) -> Vec<Option<Handle<Image>>> {
    let layer = RenderLayers::layer(render_layers::PORTRAIT);
    // Looking along +Z at a body whose rest faces -Z.
    let looking = Transform::default().looking_to(Vec3::Z, Vec3::Y).rotation;
    for (from, lux) in [(KEY_FROM, KEY_LUX), (FILL_FROM, FILL_LUX), (BACK_FROM, BACK_LUX)] {
        let light = commands
            .spawn((
                DirectionalLight { illuminance: lux, shadow_maps_enabled: false, ..default() },
                Transform::default().looking_to(-(looking * from), Vec3::Y),
                layer.clone(),
                Name::new("Portrait light"),
            ))
            .id();
        studio.entities.push(light);
    }

    let turned = Quat::from_rotation_y(TURN);
    let size = UVec2::new((PORTRAIT_WIDTH * PORTRAIT_SCALE) as u32, (PORTRAIT_HEIGHT * PORTRAIT_SCALE) as u32);
    let view = Vec2::new(VIEW_HEIGHT * PORTRAIT_WIDTH / PORTRAIT_HEIGHT, VIEW_HEIGHT);
    classes
        .iter()
        .enumerate()
        .map(|(index, &class)| {
            let feet = STUDIO + Vec3::X * SPACING * index as f32;
            let (middle, floor) = framing(builds, class, turned);

            // Rigged here rather than by `dress_bodies_from_elsewhere`, which would give it the
            // hitboxes every shot and blast then tests against.
            let mut body = commands.spawn((
                PortraitBody,
                class,
                builds.rig_for(class),
                Pose::rest(),
                SkeletonAnimator::default(),
                BodyRequests::default(),
                AnimationPhase::default(),
                Transform::from_translation(feet).with_rotation(turned),
                Visibility::default(),
                layer.clone(),
                Name::new(format!("Portrait: {class:?}")),
            ));
            if let Some(color) = color {
                body.insert(BodyTint(color));
            }
            studio.entities.push(body.id());
            studio.bodies.push((class, body.id()));

            let image = images.add(Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None));
            let eye = feet + Vec3::new(middle, floor - FLOOR_MARGIN + view.y * 0.5, -CAMERA_DISTANCE);
            let camera = commands
                .spawn((
                    Camera3d::default(),
                    Camera {
                        order: FIRST_CAMERA_ORDER - index as isize,
                        clear_color: ClearColorConfig::Custom(Color::NONE),
                        ..default()
                    },
                    RenderTarget::Image(image.clone().into()),
                    Projection::Orthographic(OrthographicProjection {
                        scaling_mode: ScalingMode::Fixed { width: view.x, height: view.y },
                        near: 0.0,
                        far: CAMERA_DISTANCE * 3.0,
                        ..OrthographicProjection::default_3d()
                    }),
                    AmbientLight { brightness: AMBIENT, ..default() },
                    Transform::from_translation(eye).with_rotation(looking),
                    layer.clone(),
                    Name::new(format!("Portrait camera: {class:?}")),
                ))
                .id();
            studio.entities.push(camera);
            studio.images.push(image.clone());
            Some(image)
        })
        .collect()
}

/// Where a class at rest stands in its portrait, turned by `turned`, from its feet: the middle
/// of its bones across, and the lowest point of them.
fn framing(builds: &ClassBuilds, class: Class, turned: Quat) -> (f32, f32) {
    let bones = builds.rig_for(class).posed_bones(&Pose::rest(), &Transform::from_rotation(turned));
    let points = bones.iter().flat_map(|bone| [bone.head, bone.tail]);
    let (low, high, floor) = points.fold((f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY), |(low, high, floor), p| {
        (low.min(p.x), high.max(p.x), floor.min(p.y))
    });
    match low.is_finite() {
        true => ((low + high) * 0.5, floor),
        false => (0.0, 0.0),
    }
}

/// The clip and claims of a class's picker taunt on its own rig, if this machine has the clip.
fn picker_taunt(
    class: Class,
    skeleton: &Skeleton,
    catalog: Option<&WeaponCatalog>,
    taunts: &Taunts,
    clips: &ConditionClips,
) -> Option<(Handle<Clip>, crate::common::skeleton::play::Regions)> {
    let key = catalog
        .and_then(|catalog| catalog.loadout(class)?.picker_taunt.clone())
        .unwrap_or_else(|| DEFAULT_PICKER_TAUNT.to_owned());
    let taunt = taunts.get(TauntId::of(&key))?;
    let clip = clips.taunt(taunt.clip_for(skeleton.kind())?)?;
    Some((clip.clone(), skeleton.anatomy().claims(taunt.claims.iter().map(String::as_str))))
}

/// Loops while hovered, from the start each time the pointer arrives.
#[allow(clippy::too_many_arguments)]
fn taunt_while_hovered(
    time: Res<Time<Real>>,
    studio: Res<PortraitStudio>,
    buttons: Query<(&ClassItem, &Hovered)>,
    catalog: Option<Res<WeaponCatalog>>,
    taunts: Option<Res<Taunts>>,
    clips: Option<Res<ConditionClips>>,
    lengths: Option<Res<Assets<Clip>>>,
    mut bodies: Query<(&Skeleton, &mut ClipStack), With<PortraitBody>>,
) {
    let taunting = |playing: &Playing| playing.layer == Layer::Taunt;
    for (item, hovered) in &buttons {
        let Some(&(_, body)) = studio.bodies.iter().find(|(class, _)| *class == item.0) else { continue };
        let Ok((skeleton, mut stack)) = bodies.get_mut(body) else { continue };
        if !hovered.get() {
            if stack.0.iter().any(taunting) {
                stack.0.retain(|playing| !taunting(playing));
            }
            continue;
        }
        if let Some(playing) = stack.0.iter_mut().find(|playing| taunting(playing)) {
            let length = lengths.as_deref().and_then(|clips| clips.get(&playing.clip)).map_or(0.0, |clip| clip.length);
            playing.time += time.delta_secs();
            if length > 0.0 {
                playing.time %= length;
            }
            continue;
        }
        let (Some(taunts), Some(clips)) = (taunts.as_deref(), clips.as_deref()) else { continue };
        if let Some((clip, claims)) = picker_taunt(item.0, skeleton, catalog.as_deref(), taunts, clips) {
            stack.0.push(Playing { claims, ..Playing::new(Layer::Taunt, clip, 0.0) });
        }
    }
}

/// The size a name of `width` at `size` is set at to fit [`NAME_ROOM`]; never larger than
/// [`NAME_SIZE`], and whole pixels so it settles.
fn fitted_size(size: f32, width: f32) -> f32 {
    match width > NAME_ROOM {
        true => (size * NAME_ROOM / width).floor().max(NAME_SMALLEST),
        false => size,
    }
}

/// The margin above and below a name set at `size`, which keeps its middle where a name at
/// [`NAME_SIZE`] would have it.
fn centering_margin(size: f32) -> f32 {
    (NAME_SIZE - size).max(0.0) * LINE_HEIGHT * 0.5
}

/// The whole row at the size its longest name fits at, each name centered on a full-size name's
/// line. Measured after layout, so a shrunk row is laid out again while its plates are hidden.
fn fit_the_names(mut names: Query<(&ComputedNode, &mut TextFont, &mut Node), With<ClassName>>) {
    let px = |font: &TextFont| match font.font_size {
        bevy::text::FontSize::Px(size) => Some(size),
        _ => None,
    };
    let fitted = names
        .iter()
        .filter_map(|(computed, font, _)| {
            let width = computed.size().x * computed.inverse_scale_factor();
            (width > 0.0).then_some(fitted_size(px(font)?, width))
        })
        .reduce(f32::min);
    let Some(fitted) = fitted else { return };
    let margin = Val::Px(centering_margin(fitted));
    for (_, mut font, mut node) in &mut names {
        if px(&font).is_some_and(|size| size != fitted) {
            font.font_size = bevy::text::FontSize::Px(fitted);
        }
        if node.margin.top != margin || node.margin.bottom != margin {
            node.margin.top = margin;
            node.margin.bottom = margin;
        }
    }
}

/// The scale that keeps a plate of `size` inside a window of `window` less [`WINDOW_MARGIN`] on
/// every side; never above one.
fn fitting_scale(size: Vec2, window: Vec2) -> f32 {
    let room = (window - Vec2::splat(2.0 * WINDOW_MARGIN)).max(Vec2::ONE);
    (room / size.max(Vec2::ONE)).min_element().min(1.0)
}

/// A transform, not a layout change, so the scale cannot feed back into the size it is
/// measured from. Centered in the window, it shrinks toward the middle.
fn fit_the_panel(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut panels: Query<(&ComputedNode, &mut UiTransform), With<PickerPanel>>,
) {
    let Ok(window) = windows.single() else { return };
    for (computed, mut transform) in &mut panels {
        let size = computed.size() * computed.inverse_scale_factor();
        if size == Vec2::ZERO {
            continue;
        }
        let scale = Vec2::splat(fitting_scale(size, window.size()));
        if transform.scale != scale {
            transform.scale = scale;
        }
    }
}

/// Draws a body through the warm-up's portrait camera at startup, so the first picker does not
/// compile its pipelines on the frame. Added in `launch.rs`, as the game's tests have no renderer.
pub struct PortraitWarmUpPlugin;

impl Plugin for PortraitWarmUpPlugin {
    fn build(&self, app: &mut App) {
        crate::game::warm_up::ensure(app);
        app.add_systems(Startup, stand_a_body_for_the_warm_up).add_systems(Update, arm_the_warm_up_body);
    }
}

/// Drawn as the portraits are; [`arm_the_warm_up_body`] gives it a weapon.
fn stand_a_body_for_the_warm_up(mut commands: Commands) {
    commands.spawn((
        crate::game::warm_up::WarmUp,
        Class::Scout,
        Transform::default(),
        Visibility::default(),
        RenderLayers::layer(render_layers::WARM_UP),
        Name::new("Warm-up body"),
    ));
}

/// The HUD's portrait draws a held weapon, whose meshes are laid out unlike a body part's, so
/// any modeled weapon is warmed up in the portrait view too.
fn arm_the_warm_up_body(
    mut commands: Commands,
    catalog: Option<Res<WeaponCatalog>>,
    bodies: Query<Entity, (With<crate::game::warm_up::WarmUp>, With<Class>, Without<crate::common::weapon::Equipped>)>,
) {
    let Some(weapon) = catalog.as_ref().and_then(|catalog| catalog.weapons().iter().find(|weapon| weapon.model.is_some())) else {
        return;
    };
    for body in &bodies {
        commands.entity(body).insert(crate::common::weapon::Equipped::carrying([weapon.id]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::respawn::Seat;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.init_state::<AppMode>();
        app.add_plugins(ClassPickerPlugin);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.insert_resource(TeamOptions { seat: Seat::Playing(None), ..default() });
        app.update();
        app
    }

    fn offer(app: &mut App, allowed: Vec<Class>, chosen: Option<Class>) {
        app.insert_resource(ClassOptions { allowed, chosen, color: None });
        app.update();
    }

    fn up(app: &App) -> bool {
        app.world().resource::<MouseHeld>().class_picker
    }

    fn press(app: &mut App, key: KeyCode) {
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(key);
        crate::common::keybind::resolve_now(app.world_mut());
        app.world_mut().run_schedule(Update);
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(key);
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
        crate::common::keybind::resolve_now(app.world_mut());
    }

    fn asked(app: &App) -> Vec<Class> {
        app.world().resource::<Messages<ChooseClass>>().iter_current_update_messages().map(|ask| ask.0).collect()
    }

    #[test]
    fn it_bars_the_way_in_until_a_class_is_chosen() {
        let mut app = app();
        offer(&mut app, vec![Class::Mercenary, Class::Scout], None);
        assert!(up(&app), "a seated player with no class walked into the match");
        offer(&mut app, vec![Class::Mercenary, Class::Scout], Some(Class::Scout));
        assert!(!up(&app));
    }

    #[test]
    fn a_mode_of_one_class_never_asks() {
        let mut app = app();
        offer(&mut app, vec![Class::Mercenary], None);
        assert!(!up(&app));
    }

    /// Breaks if the class picker covers the team picker, which has to be answered first.
    #[test]
    fn the_team_picker_comes_first() {
        let mut app = app();
        app.insert_resource(TeamOptions { seat: Seat::Choosing, ..default() });
        offer(&mut app, vec![Class::Mercenary, Class::Scout], None);
        assert!(!up(&app), "both pickers were up");
    }

    #[test]
    fn comma_brings_it_up_and_a_digit_chooses_at_once() {
        let mut app = app();
        offer(&mut app, vec![Class::Mercenary, Class::Scout, Class::Raptor], Some(Class::Mercenary));
        assert!(!up(&app));

        press(&mut app, KeyCode::Comma);
        assert!(up(&app), "`,` did not open the picker");

        press(&mut app, KeyCode::Digit3);
        assert_eq!(asked(&app), vec![Class::Raptor]);
        app.update();
        assert!(!up(&app), "the picker stayed up after a choice");
    }

    /// Breaks if a digit past the classes offered chooses anything.
    #[test]
    fn a_digit_with_no_class_does_nothing() {
        let mut app = app();
        offer(&mut app, vec![Class::Mercenary, Class::Scout], None);
        press(&mut app, KeyCode::Digit0);
        assert!(asked(&app).is_empty());
        assert!(up(&app));
    }

    #[test]
    fn escape_closes_one_asked_for_but_not_one_that_bars_the_way() {
        let mut app = app();
        offer(&mut app, vec![Class::Mercenary, Class::Scout], Some(Class::Scout));
        press(&mut app, KeyCode::Comma);
        press(&mut app, KeyCode::Escape);
        app.update();
        assert!(!up(&app));

        // The pause menu clears it, and is not in this app.
        app.world_mut().resource_mut::<EscapeTaken>().0 = false;
        offer(&mut app, vec![Class::Mercenary, Class::Scout], None);
        press(&mut app, KeyCode::Escape);
        assert!(!app.world().resource::<EscapeTaken>().0, "no way to the pause menu while choosing");
        assert!(up(&app));
    }

    /// Breaks if a button does not ask for its own class.
    #[test]
    fn each_button_asks_for_its_class() {
        let mut app = app();
        offer(&mut app, vec![Class::Mercenary, Class::Scout], None);
        let buttons: Vec<(Entity, Class)> =
            app.world_mut().query::<(Entity, &ClassItem)>().iter(app.world()).map(|(entity, item)| (entity, item.0)).collect();
        assert_eq!(buttons.iter().map(|(_, class)| *class).collect::<Vec<_>>(), vec![Class::Mercenary, Class::Scout]);
        app.world_mut().trigger(ButtonActivated { entity: buttons[1].0 });
        app.world_mut().flush();
        assert_eq!(asked(&app), vec![Class::Scout]);
    }

    #[test]
    fn the_buttons_are_numbered_one_to_nine_then_zero() {
        let labels = choice_labels(&Keymap::default());
        assert_eq!(labels, ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]);
    }

    /// Breaks if a class's feet are not at the bottom of its portrait, or it is off to one side.
    #[test]
    fn a_body_stands_at_the_bottom_middle_of_its_portrait() {
        let builds = ClassBuilds::shipped();
        for class in [Class::Scout, Class::Heavy, Class::Raptor] {
            let (middle, floor) = framing(&builds, class, Quat::from_rotation_y(TURN));
            assert!(floor.abs() < 0.2, "{class:?}'s lowest bone is {floor} m from its feet");
            assert!(middle.abs() < 0.6, "{class:?} is {middle} m off center");
        }
    }

    /// Breaks if a class plays some other taunt than its own, or the raptor is handed a clip for
    /// the humanoid's bones.
    #[test]
    fn each_class_hovers_with_its_own_taunt_on_its_own_rig() {
        let builds = ClassBuilds::shipped();
        let catalog = crate::common::weapon_file::default_catalog();
        let taunts = Taunts::shipped();
        let names = ["taunt_wave", "taunt_shadowbox", "raptor/taunt_wag", "raptor/taunt_lunge"];
        let mut handles = Assets::<Clip>::default();
        let made: Vec<(String, Handle<Clip>)> = names.iter().map(|name| (name.to_string(), handles.add(Clip::default()))).collect();
        let clips = ConditionClips::with_taunts(made.clone());
        let played = |class| {
            let (clip, _) = picker_taunt(class, &builds.rig_for(class), Some(&catalog), &taunts, &clips).unwrap();
            made.iter().find(|(_, handle)| *handle == clip).unwrap().0.clone()
        };
        assert_eq!(played(Class::Mercenary), "taunt_wave");
        assert_eq!(played(Class::Scout), "taunt_shadowbox");
        assert_eq!(played(Class::Raptor), "raptor/taunt_lunge");
    }

    /// Breaks if the warm-up body is not given a modeled weapon to hold, so no held weapon is drawn
    /// in the portrait view before a round.
    #[test]
    fn the_warm_up_body_holds_a_modeled_weapon() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let catalog = crate::common::weapon_file::default_catalog();
        let body = world.spawn((crate::game::warm_up::WarmUp, Class::Scout)).id();
        world.insert_resource(catalog.clone());
        world.run_system_once(arm_the_warm_up_body).unwrap();
        let held = world.get::<crate::common::weapon::Equipped>(body).and_then(|equipped| equipped.held());
        assert!(held.and_then(|id| catalog.get(id)).is_some_and(|weapon| weapon.model.is_some()), "holding {held:?}");
    }

    /// Breaks if the taunt lingers after the pointer leaves, fading or not.
    #[test]
    fn hovering_taunts_and_leaving_stops_at_once() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        let mut handles = Assets::<Clip>::default();
        let wave = handles.add(Clip { length: 1.0, ..Clip::default() });
        world.insert_resource(handles);
        world.insert_resource(Taunts::shipped());
        world.insert_resource(ConditionClips::with_taunts([("taunt_wave".to_string(), wave)]));
        let builds = ClassBuilds::shipped();
        let body = world.spawn((PortraitBody, builds.rig_for(Class::Mercenary), ClipStack::default())).id();
        world.insert_resource(PortraitStudio { bodies: vec![(Class::Mercenary, body)], ..default() });
        let button = world.spawn((ClassItem(Class::Mercenary), Hovered(true))).id();

        world.run_system_once(taunt_while_hovered).unwrap();
        let taunts = |world: &World| world.get::<ClipStack>(body).unwrap().0.iter().filter(|playing| playing.layer == Layer::Taunt).count();
        assert_eq!(taunts(&world), 1);

        world.entity_mut(button).insert(Hovered(false));
        world.run_system_once(taunt_while_hovered).unwrap();
        assert_eq!(taunts(&world), 0, "the taunt outlived the hover");
    }

    /// Breaks if a plate wider or taller than the safe zone is left to run off the window, or one
    /// that fits is shrunk anyway.
    #[test]
    fn the_panel_shrinks_only_to_stay_inside_the_window_safe_zone() {
        let window = Vec2::new(1280.0, 720.0);
        assert_eq!(fitting_scale(Vec2::new(600.0, 400.0), window), 1.0);
        let wide = Vec2::new(2000.0, 400.0);
        let scale = fitting_scale(wide, window);
        assert!((wide.x * scale - (window.x - 2.0 * WINDOW_MARGIN)).abs() < 1e-3, "{scale}");
        let tall = Vec2::new(500.0, 1000.0);
        assert!(tall.y * fitting_scale(tall, window) <= window.y - 2.0 * WINDOW_MARGIN + 1e-3);
    }

    /// Breaks if a long name is left wider than its button, or a short one is shrunk.
    #[test]
    fn a_long_name_is_set_smaller_to_fit() {
        assert_eq!(fitted_size(NAME_SIZE, NAME_ROOM * 0.5), NAME_SIZE);
        let width = NAME_ROOM * 1.5;
        let size = fitted_size(NAME_SIZE, width);
        assert!(width * size / NAME_SIZE <= NAME_ROOM, "the name is still too wide at {size} px");
    }

    /// Breaks if a button is too narrow to leave room for a name.
    #[test]
    fn there_is_room_for_a_name_in_a_button() {
        assert!(NAME_ROOM > 60.0, "{NAME_ROOM}");
    }

    /// Breaks if a smaller name's line is not centered on a full-size name's.
    #[test]
    fn a_smaller_name_is_centered_where_a_full_size_one_would_be() {
        let line = |size: f32| size * LINE_HEIGHT;
        for size in [NAME_SIZE, 17.0, NAME_SMALLEST] {
            let margin = centering_margin(size);
            assert!((margin + line(size) * 0.5 - line(NAME_SIZE) * 0.5).abs() < 1e-4, "{size} px sits off center");
            assert!((2.0 * margin + line(size) - line(NAME_SIZE)).abs() < 1e-4, "{size} px moves the rows below");
        }
    }
}
