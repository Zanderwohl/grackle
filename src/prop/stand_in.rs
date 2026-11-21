//! A body standing in the Model workspace for scale, in its rest pose (an A-pose on the
//! humanoid), placed and turned by hand.
//!
//! Saved with the prop so it comes back where it was left, but never part of the prop: nothing
//! outside the prop editor reads it.
//!
//! On a held or worn prop the same record (shown, class) is the Model workspace's
//! [`crate::prop::figure`], which holds or wears the prop, and the placement is unused.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::class::{Class, ClassBuilds};
use crate::common::skeleton::Pose;
use crate::editor::input::CurrentMouseInput;
use crate::game::body_mesh::BodyTint;
use crate::prop::archetype::Archetype;
use crate::prop::document::{PropDoc, PropEditor};
use crate::prop::gizmo::{AXIS_COLORS, HIGHLIGHT};
use crate::tool::tool_helpers::{closest_param_on_axis, ray_point_distance};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StandIn {
    #[serde(default)]
    pub shown: bool,
    #[serde(default, with = "class_key")]
    pub class: Class,
    /// Where the feet are, in the prop's frame.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub at: [f32; 3],
    /// Radians about `+Y`; zero faces `-Z`, as the prop's forward.
    #[serde(default, with = "crate::prop::nice_f32::scalar")]
    pub yaw: f32,
}

impl StandIn {
    /// Beside the origin: a held prop's origin at about hand height, a placed one's on the same
    /// floor.
    pub fn beside(placed: bool) -> StandIn {
        StandIn {
            shown: true,
            class: Class::Mercenary,
            at: match placed {
                true => [1.0, 0.0, 0.0],
                false => [0.6, -1.0, 0.0],
            },
            yaw: 0.0,
        }
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.at)).with_rotation(Quat::from_rotation_y(self.yaw))
    }
}

/// By `Class::key`, as `per_class` keys a class.
mod class_key {
    use super::*;

    pub fn serialize<S: serde::Serializer>(class: &Class, serializer: S) -> Result<S::Ok, S::Error> {
        Class::key(*class).serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Class, D::Error> {
        let key = String::deserialize(deserializer)?;
        Class::named(&key).ok_or_else(|| serde::de::Error::custom(format!("no class is called {key:?}")))
    }
}

/// Whether the Model workspace is up. Fit has a body of its own.
#[derive(Resource, Default)]
pub struct Modeling(pub bool);

#[derive(Component)]
pub struct StandInMarker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Move(usize),
    Turn,
}

/// What a drag started from, so the body does not jump to the pointer.
#[derive(Resource, Default)]
pub struct StandInGrab {
    held: Option<Held>,
    /// A handle is under the pointer: a press there belongs to the stand-in.
    hovered: bool,
}

impl StandInGrab {
    /// The feature handles and viewport picking stand back while this is true.
    pub fn busy(&self) -> bool {
        self.held.is_some() || self.hovered
    }
}

#[derive(Clone, Copy, Debug)]
struct Held {
    handle: Handle,
    from: StandIn,
    /// Along the axis for a move, radians about the feet for a turn.
    grabbed_at: f32,
}

/// Meters.
const ARROW_LENGTH: f32 = 0.5;
/// Short of the arrow tips by more than twice [`PICK_RADIUS`], so a press never lands on both.
const RING_RADIUS: f32 = 0.35;
/// Meters from a handle that still grabs it.
const PICK_RADIUS: f32 = 0.06;

/// The sine of the shallowest angle to the floor the ring is read at. Shallower, the crossing
/// lies far off and a pixel of pointer turns the body a long way.
const STEEPEST_GRAZE: f32 = 0.15;

const RING_COLOR: Color = Color::srgb(0.85, 0.85, 0.4);

/// Not a team color, and apart from the Fit figure's.
const STAND_IN_TINT: Color = Color::srgb(0.42, 0.40, 0.46);

pub struct StandInPlugin;

impl Plugin for StandInPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Modeling>()
            .init_resource::<StandInGrab>()
            .add_systems(
                Update,
                // After the drag, so the body is where its handles are drawn.
                stand_the_stand_in.after(drag_the_stand_in).run_if(in_state(AppMode::Prop)),
            )
            .add_systems(OnExit(AppMode::Prop), clear_the_stand_in);
    }
}

/// On these, Model's reference figure takes the stand-in's place.
pub fn figure_holds(archetype: Archetype) -> bool {
    matches!(archetype, Archetype::Held | Archetype::Hat)
}

/// The Model workspace's [`crate::prop::figure::ScaleFigure`].
pub fn model_figure(doc: &PropDoc) -> Option<Class> {
    doc.stand_in
        .filter(|stand_in| stand_in.shown && figure_holds(doc.archetype))
        .map(|stand_in| stand_in.class)
}

fn wanted(editor: &PropEditor, modeling: bool) -> Option<StandIn> {
    let doc = editor.doc();
    doc.stand_in.filter(|stand_in| stand_in.shown && modeling && !figure_holds(doc.archetype))
}

pub fn stand_the_stand_in(
    mut commands: Commands,
    editor: Res<PropEditor>,
    modeling: Res<Modeling>,
    builds: Res<ClassBuilds>,
    mut existing: Query<(Entity, &mut Transform), With<StandInMarker>>,
    mut last: Local<Option<Class>>,
) {
    let wanted = wanted(&editor, modeling.0);
    let class = wanted.map(|stand_in| stand_in.class);
    if *last != class || builds.is_changed() || (class.is_some() && existing.is_empty()) {
        *last = class;
        for (entity, _) in &existing {
            commands.entity(entity).despawn();
        }
        if let Some(stand_in) = wanted {
            commands.spawn((
                StandInMarker,
                crate::prop::view::PropScene,
                stand_in.transform(),
                Pose::rest(),
                Visibility::Inherited,
                BodyTint(STAND_IN_TINT),
                Name::new("Stand-in"),
                builds.rig_for(stand_in.class),
            ));
        }
        return;
    }
    let Some(stand_in) = wanted else { return };
    let placed = stand_in.transform();
    for (_, mut transform) in &mut existing {
        if *transform != placed {
            *transform = placed;
        }
    }
}

fn clear_the_stand_in(
    mut commands: Commands,
    existing: Query<Entity, With<StandInMarker>>,
    mut grab: ResMut<StandInGrab>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    *grab = StandInGrab::default();
}

/// Along the world axes, so an arrow moves the feet the way it points whatever the body's turn.
fn arrow_tip(feet: Vec3, axis: usize) -> Vec3 {
    feet + Vec3::AXES[axis] * ARROW_LENGTH
}

/// The angle about the feet where the ray crosses their floor, and how far from the ring.
fn on_the_ring(ray: Ray3d, feet: Vec3) -> Option<(f32, f32)> {
    if ray.direction.y.abs() < STEEPEST_GRAZE {
        return None;
    }
    let across = ray.intersect_plane(feet, InfinitePlane3d::new(Vec3::Y))?;
    let offset = ray.get_point(across) - feet;
    Some((angle_of(offset), (offset.xz().length() - RING_RADIUS).abs()))
}

/// Radians about `+Y`, measured as a yaw is: zero along `-Z`.
fn angle_of(offset: Vec3) -> f32 {
    (-offset.x).atan2(-offset.z)
}

fn nearest(ray: Ray3d, feet: Vec3) -> Option<Handle> {
    let arrows = (0..3).map(|axis| (Handle::Move(axis), ray_point_distance(&ray, arrow_tip(feet, axis))));
    let ring = on_the_ring(ray, feet).map(|(_, distance)| (Handle::Turn, distance));
    arrows
        .chain(ring)
        .filter(|(_, distance)| *distance < PICK_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle)
}

/// Before the feature handles and picking, which stand back while [`StandInGrab::busy`].
pub fn drag_the_stand_in(
    mut editor: ResMut<PropEditor>,
    mouse: Res<CurrentMouseInput>,
    modeling: Res<Modeling>,
    mut grab: ResMut<StandInGrab>,
    mut gizmos: Gizmos,
) {
    let Some(stand_in) = wanted(&editor, modeling.0) else {
        if grab.held.take().is_some() {
            editor.end_gesture();
        }
        grab.hovered = false;
        return;
    };
    let feet = Vec3::from_array(stand_in.at);

    let let_go = mouse.world_pos.is_none()
        || mouse.released == Some(MouseButton::Left)
        || mouse.pressed != Some(MouseButton::Left);
    if let_go && grab.held.take().is_some() {
        editor.end_gesture();
    }

    let under = mouse.world_pos.and_then(|ray| nearest(ray, feet));
    if grab.hovered != under.is_some() {
        grab.hovered = under.is_some();
    }

    if let (Some(ray), Some(handle)) = (mouse.world_pos, under)
        && grab.held.is_none()
        && mouse.just_pressed
        && mouse.pressed == Some(MouseButton::Left)
    {
        let grabbed_at = match handle {
            Handle::Move(axis) => closest_param_on_axis(ray, feet, Vec3::AXES[axis]),
            Handle::Turn => on_the_ring(ray, feet).map(|(angle, _)| angle),
        };
        if let Some(grabbed_at) = grabbed_at {
            grab.held = Some(Held { handle, from: stand_in, grabbed_at });
            editor.begin_gesture();
        }
    }

    if let (Some(ray), Some(held)) = (mouse.world_pos, grab.held) {
        let from = held.from;
        let start = Vec3::from_array(from.at);
        let mut moved = stand_in;
        match held.handle {
            Handle::Move(axis) => {
                if let Some(along) = closest_param_on_axis(ray, start, Vec3::AXES[axis]) {
                    moved.at = (start + Vec3::AXES[axis] * (along - held.grabbed_at)).to_array();
                }
            }
            Handle::Turn => {
                if let Some((angle, _)) = on_the_ring(ray, start) {
                    moved.yaw = wrap(from.yaw + angle - held.grabbed_at);
                }
            }
        }
        // A held button with the pointer still is not an edit.
        if moved != stand_in {
            editor.doc_mut().stand_in = Some(moved);
        }
    }

    let lit = grab.held.map(|held| held.handle).or(under);
    let feet = Vec3::from_array(editor.doc().stand_in.map_or(stand_in.at, |stand_in| stand_in.at));
    let color = |handle: Handle, otherwise: Color| match lit == Some(handle) {
        true => HIGHLIGHT,
        false => otherwise,
    };
    for axis in 0..3 {
        gizmos.arrow(feet, arrow_tip(feet, axis), color(Handle::Move(axis), AXIS_COLORS[axis]));
    }
    gizmos.circle(
        Isometry3d::new(feet, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        RING_RADIUS,
        color(Handle::Turn, RING_COLOR),
    );
}

/// Into `(-π, π]`, so a few turns of dragging do not pile up in the file.
fn wrap(radians: f32) -> f32 {
    let turn = std::f32::consts::TAU;
    let wrapped = (radians + std::f32::consts::PI).rem_euclid(turn) - std::f32::consts::PI;
    if wrapped <= -std::f32::consts::PI { wrapped + turn } else { wrapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if a yaw measured off the ring disagrees with the turn the body is given.
    #[test]
    fn the_rings_angle_is_the_bodys_yaw() {
        for yaw in [0.0, 0.7, -1.9, 3.0] {
            let facing = StandIn { yaw, ..StandIn::beside(false) }.transform().rotation * Vec3::NEG_Z;
            assert!((angle_of(facing) - yaw).abs() < 1e-5, "{yaw} read back as {}", angle_of(facing));
        }
    }

    #[test]
    fn a_stand_in_round_trips_through_the_file() {
        let mut doc = crate::prop::document::PropDoc::new("prop.untitled");
        doc.stand_in = Some(StandIn { shown: false, class: Class::Raptor, at: [0.25, -1.0, 0.5], yaw: 0.3 });
        let text = crate::prop::document::to_text(&doc).unwrap();
        assert!(text.contains("class = \"raptor\""), "{text}");
        assert_eq!(crate::prop::document::parse(&text).unwrap(), doc, "{text}");
    }

    #[test]
    fn a_prop_without_one_writes_none() {
        let text = crate::prop::document::to_text(&crate::prop::document::PropDoc::new("prop.untitled")).unwrap();
        assert!(!text.contains("stand_in"), "{text}");
    }

    /// Fails if one record drives both bodies at once.
    #[test]
    fn the_archetype_decides_which_body_stands() {
        let mut doc = PropDoc::new("prop.untitled");
        doc.stand_in = Some(StandIn { class: Class::Heavy, ..StandIn::beside(false) });
        let editor = |doc: &PropDoc| {
            let mut editor = PropEditor::default();
            editor.open(doc.clone(), None);
            editor
        };
        for archetype in [Archetype::Held, Archetype::Hat] {
            doc.archetype = archetype;
            assert_eq!(model_figure(&doc), Some(Class::Heavy), "{archetype:?}");
            assert!(wanted(&editor(&doc), true).is_none(), "{archetype:?} stood a free body too");
        }
        doc.archetype = Archetype::Obstacle;
        assert_eq!(model_figure(&doc), None);
        assert!(wanted(&editor(&doc), true).is_some());
        doc.stand_in = doc.stand_in.map(|stand_in| StandIn { shown: false, ..stand_in });
        assert!(wanted(&editor(&doc), true).is_none());
    }

    #[test]
    fn wrapping_keeps_the_angle() {
        for radians in [0.0, 4.0, -4.0, 10.0, std::f32::consts::PI] {
            let wrapped = wrap(radians);
            assert!(wrapped > -std::f32::consts::PI - 1e-6 && wrapped <= std::f32::consts::PI + 1e-6);
            assert!((wrapped.sin() - radians.sin()).abs() < 1e-5 && (wrapped.cos() - radians.cos()).abs() < 1e-5);
        }
    }
}
