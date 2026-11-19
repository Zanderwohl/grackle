//! A red dot where a sighted scope's shot would land, for every machine to see,
//! so a sniper holding a corner shows where they are aimed.
//!
//! The authority traces it each tick into the replicated [`LaserDot`] on the
//! body, so it is where the server would hit, walls and bodies alike. It is
//! drawn by [`LaserDotPlugin`] as one quad turned to face the view, moved a
//! little toward the eye so the surface under it does not cut it.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use serde::{Deserialize, Serialize};

use crate::common::weapon::{Equipped, WeaponAction, WeaponCatalog};
use crate::game::collision::CollisionWorld;
use crate::game::death::Dying;
use crate::common::class::Stance;
use crate::common::hitbox::Hitboxes;
use crate::game::hitscan::{aim, eye, trace_past};
use crate::game::player::{PhysicsBody, Player, PlayerCamera};
use crate::game::sights::Sights;
use crate::game::warm_up::{self, WarmUp};
use crate::game::weapon::Button;

/// Meters across, up close.
const SIZE: f32 = 0.06;

/// Meters across per meter from the eye, so a far dot stays a few pixels wide.
const SIZE_PER_METER: f32 = 0.003;

/// Meters toward the eye from where the ray stopped.
const LIFT: f32 = 0.04;

const TEXELS: u32 = 64;

/// Where the body's sighted scope would put its shot, in world space; `None`
/// while it is not sighted, has no [`ScopeSpec::laser_dot`](crate::common::weapon::ScopeSpec::laser_dot),
/// or its ray reaches nothing.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LaserDot(pub Option<Vec3>);

/// On the authority, after the move and the sights and with this tick's boxes,
/// so the dot is where a shot this tick would land.
#[allow(clippy::type_complexity)]
pub fn aim_the_laser_dots(
    mut commands: Commands,
    catalog: Res<WeaponCatalog>,
    world: Res<CollisionWorld>,
    mut bodies: Query<(Entity, &PhysicsBody, &Player, &Stance, &Equipped, &Sights, Option<&mut LaserDot>), Without<Dying>>,
    targets: Query<(Entity, &Hitboxes)>,
) {
    for (body, physics, player, stance, equipped, sights, dot) in &mut bodies {
        let range = sights
            .scope(equipped, &catalog)
            .filter(|scope| scope.laser_dot)
            .and_then(|_| catalog.held_by(equipped))
            .and_then(|weapon| match sights.mounted(weapon, Button::Primary).action {
                WeaponAction::Hitscan(spec) => Some(spec.range_m),
                _ => None,
            });
        let at = range.and_then(|range| {
            let ray = Ray3d::new(eye(physics.current, *stance, player.duck, player.eye), aim(player));
            let wall = world.ray_distance(&ray, range);
            let reach = wall.unwrap_or(range);
            match trace_past(&ray, reach, body, &targets) {
                Some(hit) => Some(hit.point),
                None => wall.map(|distance| ray.origin + *ray.direction * distance),
            }
        });
        // Replicated, so written only on change.
        match dot {
            Some(mut dot) if dot.0 != at => dot.0 = at,
            Some(_) => {}
            None if at.is_some() => {
                commands.entity(body).try_insert(LaserDot(at));
            }
            None => {}
        }
    }
}

/// The drawn dot of a body's [`LaserDot`].
#[derive(Component)]
struct DotOf(Entity);

#[derive(Resource)]
struct DotLook {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// A soft disk in every channel, white at the middle and clear at the rim.
fn glow(texels: u32) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    let mut rgba = Vec::with_capacity((texels * texels * 4) as usize);
    for y in 0..texels {
        for x in 0..texels {
            let p = (Vec2::new(x as f32, y as f32) + 0.5) / texels as f32 * 2.0 - Vec2::ONE;
            // A bright core in a fainter halo.
            let r = p.length();
            let core = (1.0 - r / 0.35).clamp(0.0, 1.0);
            let halo = (1.0 - r).clamp(0.0, 1.0).powi(2) * 0.5;
            let value = ((core + halo).min(1.0) * 255.0).round() as u8;
            rgba.extend([value, value, value, value]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: texels, height: texels, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

fn make_the_look(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.insert_resource(DotLook {
        mesh: meshes.add(Rectangle::new(1.0, 1.0)),
        material: materials.add(StandardMaterial {
            // Above one so the core stays bright once tonemapped.
            base_color: Color::linear_rgb(6.0, 0.15, 0.1),
            base_color_texture: Some(images.add(glow(TEXELS))),
            unlit: true,
            alpha_mode: AlphaMode::Add,
            cull_mode: None,
            ..default()
        }),
    });
}

/// See [`crate::game::warm_up`].
fn warm_up_the_dot(mut commands: Commands, look: Res<DotLook>) {
    use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};

    commands.spawn((
        WarmUp,
        Mesh3d(look.mesh.clone()),
        MeshMaterial3d(look.material.clone()),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        NoFrustumCulling,
        NotShadowCaster,
        // Too small to cover a pixel of the 4×4 target.
        Transform::from_xyz(0.0, 0.0, -1.0).with_scale(Vec3::splat(1.0e-4)),
        Visibility::default(),
    ));
}

/// Facing `eye`, lifted toward it and widened with distance from it.
fn dot_transform(at: Vec3, eye: Option<Vec3>) -> Transform {
    let Some(eye) = eye else {
        return Transform::from_translation(at).with_scale(Vec3::splat(SIZE));
    };
    let toward = (eye - at).normalize_or(Vec3::Y);
    let distance = eye.distance(at);
    let lifted = at + toward * LIFT.min(distance * 0.5);
    Transform::from_translation(lifted)
        .looking_to(-toward, Vec3::Y)
        .with_scale(Vec3::splat(SIZE.max(distance * SIZE_PER_METER)))
}

/// Ungated, as tracers are: a client draws every body's dot.
fn draw_laser_dots(
    mut commands: Commands,
    look: Res<DotLook>,
    // The view is a root, so its `Transform` is this frame's, where a
    // `GlobalTransform` before propagation is the last frame's.
    eye: Option<Single<&Transform, (With<PlayerCamera>, Without<DotOf>)>>,
    bodies: Query<&LaserDot>,
    mut dots: Query<(Entity, &DotOf, &mut Transform)>,
    owners: Query<(Entity, &LaserDot), Without<DotOf>>,
) {
    let eye = eye.map(|eye| eye.translation);
    let mut drawn = Vec::new();
    for (entity, of, mut transform) in &mut dots {
        match bodies.get(of.0) {
            Ok(LaserDot(Some(at))) => {
                *transform = dot_transform(*at, eye);
                drawn.push(of.0);
            }
            _ => commands.entity(entity).despawn(),
        }
    }
    for (body, dot) in &owners {
        let Some(at) = dot.0 else { continue };
        if drawn.contains(&body) {
            continue;
        }
        commands.spawn((
            DotOf(body),
            Mesh3d(look.mesh.clone()),
            MeshMaterial3d(look.material.clone()),
            dot_transform(at, eye),
            NotShadowCaster,
            Name::new("Laser dot"),
        ));
    }
}

/// Added in `main.rs` beside `StreakPlugin`: it builds a mesh, an image and a
/// material, which the game's tests have no assets for. The dot itself is
/// aimed by `WeaponPlugin`.
pub struct LaserDotPlugin;

impl Plugin for LaserDotPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        app.add_systems(Startup, (make_the_look, warm_up_the_dot).chain())
            .add_systems(PostUpdate, draw_laser_dots.before(TransformSystems::Propagate));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::weapon::WeaponId;
    use crate::common::weapon_file::default_catalog;
    use crate::common::world_surface::Volume;
    use crate::tool::room::Room;

    /// Fails if the dot is not on the wall the sighted rifle faces, or stays up
    /// once the sights start down.
    #[test]
    fn the_dot_is_where_the_sighted_shot_would_land_and_only_while_sighted() {
        let mut world = World::new();
        world.insert_resource(default_catalog());
        let mut walls = CollisionWorld::default();
        walls.rebuild_map(&[Volume::air(Room::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0)))], &[]);
        world.insert_resource(walls);
        let rifle = WeaponId::of("sniper_rifle");
        let sighted = Sights { up: true, raised: 1.0, weapon: Some(rifle), button_down: false };
        let body = world
            .spawn((Player::default(), PhysicsBody::at(Vec3::new(0.0, 1.0, 0.0)), Stance::default(), Equipped::carrying([rifle]), sighted))
            .id();

        world.run_system_once(aim_the_laser_dots).unwrap();
        let at = world.get::<LaserDot>(body).and_then(|dot| dot.0).expect("no dot while sighted");
        assert!((at.z + 5.0).abs() < 1.0e-3 && at.x.abs() < 1.0e-3, "dot at {at}");

        world.get_mut::<Sights>(body).unwrap().raised = 0.9;
        world.run_system_once(aim_the_laser_dots).unwrap();
        assert_eq!(world.get::<LaserDot>(body), Some(&LaserDot(None)));
    }

    /// Fails if the dot does not face the eye, sits behind the surface, or
    /// shrinks below its near size far away.
    #[test]
    fn the_dot_faces_the_eye_from_just_in_front_of_the_surface() {
        let at = Vec3::new(0.0, 1.0, -20.0);
        let eye = Vec3::new(0.0, 1.0, 0.0);
        let drawn = dot_transform(at, Some(eye));
        let facing = drawn.rotation * Vec3::Z;
        assert!(facing.abs_diff_eq(Vec3::Z, 1.0e-5), "faces {facing}");
        assert!(drawn.translation.z > at.z && drawn.translation.z < at.z + 0.1, "at {}", drawn.translation);
        assert!(drawn.scale.x >= SIZE);
        assert!((drawn.scale.x - 20.0 * SIZE_PER_METER).abs() < 1.0e-5);
    }
}
