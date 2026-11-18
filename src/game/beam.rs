//! A line over the whole of a hitscan shot's path, one per [`ShotPath`] whose
//! weapon's trail is [`HitscanTrail::Beam`], in the shooter's color, with a
//! burst of sparks of that color around it. Both ends are fixed when the shot
//! arrives; only its opacity changes after that. Drawing only: the shot was
//! resolved on the tick.
//!
//! The line is two crossed ribbons, one turned about its length to face the
//! play camera, so a second camera (the weapon editor's stage) never sees it
//! edge-on. Each is textured with a profile that fades to nothing at both long
//! edges, so it has a soft edge from any side. It ages on the particles' clock,
//! as its sparks do, so the kill cam's slow motion slows both.

use bevy::camera::visibility::RenderLayers;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::common::weapon::HitscanTrail;
use crate::game::hitscan::{mark_tracers, ShotPath};
use crate::game::player::{Player, PlayerCamera};
use crate::game::replay::HeldBursts;
use crate::game::streak::widened;
use crate::game::warm_up::{self, WarmUp};

/// Seconds from nothing to full opacity.
const FADE_IN: f32 = 0.25;

/// Seconds from full opacity to nothing, starting the moment the fade-in ends.
const FADE_OUT: f32 = 1.0;

/// Opacity at the middle of the ribbon at its brightest.
const MOST_OPAQUE: f32 = 0.8;

/// Meters from the middle to either soft edge, up close.
const HALF_WIDTH: f32 = 0.03;

/// Meters of half-width per meter from the eye, so a far beam's bright middle
/// stays about two pixels wide at 1080p with a 90° view.
const HALF_WIDTH_PER_METER: f32 = 0.004;

/// Linear brightness over the swatch's base. Above one so the color survives
/// tonemapping as a glow rather than a flat stripe.
const GLOW: f32 = 2.0;

const PROFILE_TEXELS: u32 = 64;

const SPARKS: u32 = 300;

/// Seconds.
const SPARK_LIFETIME: f32 = 1.5;
const SPARK_FADE_IN: f32 = 0.125;
const SPARK_FADE_OUT: f32 = SPARK_LIFETIME - SPARK_FADE_IN;

/// Meters off the line a spark starts, at most.
const SPARK_SCATTER: f32 = 0.04;

/// Meters per second a spark starts at, least and most, in a random direction.
const SPARK_SPEED: (f32, f32) = (0.2, 1.2);

/// Per second, exponential: a spark keeps `e^(-BRAKE t)` of its speed, so it
/// is nearly still within a quarter second.
const SPARK_BRAKE: f32 = 10.0;

/// Meters across, least and most.
const SPARK_SIZE: (f32, f32) = (0.006, 0.014);

/// Seconds of the particles' clock an emitter is kept after its burst.
const SPARKS_GONE: f32 = SPARK_LIFETIME + 0.25;

#[derive(Component, Debug, Clone, Copy)]
struct Beam {
    from: Vec3,
    to: Vec3,
    /// Seconds.
    age: f32,
    color: LinearRgba,
}

impl Beam {
    /// Zero to one, or `None` once it has faded out.
    fn opacity(&self) -> Option<f32> {
        if self.age < FADE_IN {
            Some(self.age / FADE_IN)
        } else if self.age < FADE_IN + FADE_OUT {
            Some(1.0 - (self.age - FADE_IN) / FADE_OUT)
        } else {
            None
        }
    }

    /// Turned about its length so the first ribbon faces `eye`, and widened
    /// with the distance from `eye`.
    fn transform(&self, eye: Option<Vec3>) -> Transform {
        let length = self.from.distance(self.to);
        let direction = (self.to - self.from).normalize_or(Vec3::NEG_Z);
        let middle = (self.from + self.to) * 0.5;
        let half_width = widened(self.from, direction, (0.0, length), eye, HALF_WIDTH, HALF_WIDTH_PER_METER);
        let toward_eye = eye.map_or(Vec3::Z, |eye| eye - middle);
        // Looking straight down the beam leaves no side to face; any will do.
        let side = direction.cross(toward_eye).try_normalize().unwrap_or_else(|| direction.any_orthonormal_vector());
        let facing = side.cross(direction);
        Transform {
            translation: middle,
            rotation: Quat::from_mat3(&Mat3::from_cols(side, direction, facing)),
            scale: Vec3::new(half_width / HALF_WIDTH, length.max(1.0e-4), half_width / HALF_WIDTH),
        }
    }

    fn tinted(&self, opacity: f32) -> Color {
        Color::LinearRgba(LinearRgba { alpha: opacity * MOST_OPAQUE, ..self.color })
    }
}

fn glow_of(team: Option<Team>, color: Option<PlayerColor>) -> LinearRgba {
    let base = LinearRgba::from(Swatch::of_body(team, color).base);
    LinearRgba { red: base.red * GLOW, green: base.green * GLOW, blue: base.blue * GLOW, alpha: 1.0 }
}

/// Opacity across the ribbon, `-1` at one edge to `1` at the other: one in the
/// middle, under one percent at the edges.
fn profile(across: f32) -> f32 {
    (-(across * 2.2).powi(2)).exp()
}

/// White, with [`profile`] as its alpha across `U`.
fn profile_image() -> Image {
    let rgba = (0..PROFILE_TEXELS)
        .flat_map(|x| {
            let across = (x as f32 + 0.5) / PROFILE_TEXELS as f32 * 2.0 - 1.0;
            [255, 255, 255, (profile(across) * 255.0).round() as u8]
        })
        .collect();
    crate::game::flame_jet::sheet_image_as(rgba, PROFILE_TEXELS, 1, bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb)
}

#[derive(Resource)]
struct BeamLook {
    mesh: Handle<Mesh>,
    profile: Handle<Image>,
}

impl BeamLook {
    fn material(&self, color: Color) -> StandardMaterial {
        StandardMaterial {
            base_color: color,
            base_color_texture: Some(self.profile.clone()),
            unlit: true,
            alpha_mode: bevy::prelude::AlphaMode::Blend,
            double_sided: true,
            cull_mode: None,
            ..default()
        }
    }
}

fn crossed_ribbons() -> Mesh {
    let ribbon = || Mesh::from(Rectangle::new(HALF_WIDTH * 2.0, 1.0));
    let mut mesh = ribbon();
    mesh.merge(&ribbon().rotated_by(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)))
        .expect("two rectangles share their attributes");
    mesh
}

fn make_the_look(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(BeamLook {
        mesh: meshes.add(crossed_ribbons()),
        profile: images.add(profile_image()),
    });
}

/// The beam material for [`crate::game::warm_up`].
fn warm_up(mut commands: Commands, look: Res<BeamLook>, mut materials: ResMut<Assets<StandardMaterial>>) {
    use bevy::camera::visibility::NoFrustumCulling;

    commands.spawn((
        WarmUp,
        Mesh3d(look.mesh.clone()),
        MeshMaterial3d(materials.add(look.material(Color::WHITE.with_alpha(0.5)))),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        NoFrustumCulling,
        NotShadowCaster,
        // Too small to cover a pixel of the 4×4 target.
        Transform::from_xyz(0.0, 0.0, -1.0).with_scale(Vec3::splat(1.0e-4)),
        Visibility::default(),
    ));
}

fn raise_beams(
    mut commands: Commands,
    look: Res<BeamLook>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut paths: MessageReader<ShotPath>,
    shooters: Query<(&PlayerId, Option<&Team>, Option<&PlayerColor>), With<Player>>,
    eye: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
) {
    let eye = eye.map(|eye| eye.translation());
    for path in paths.read().filter(|path| path.trail == HitscanTrail::Beam) {
        let shooter = path.by.and_then(|by| shooters.iter().find(|(id, _, _)| **id == by));
        let color = shooter.map_or_else(|| glow_of(None, None), |(_, team, color)| glow_of(team.copied(), color.copied()));
        let beam = Beam { from: path.from, to: path.to, age: 0.0, color };
        commands.spawn((
            beam,
            Mesh3d(look.mesh.clone()),
            MeshMaterial3d(materials.add(look.material(beam.tinted(0.0)))),
            beam.transform(eye),
            NotShadowCaster,
            Name::new("Beam"),
        ));
    }
}

fn fade_beams(
    mut commands: Commands,
    time: Res<Time<EffectSimulation>>,
    eye: Option<Single<&GlobalTransform, With<PlayerCamera>>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut beams: Query<(Entity, &mut Beam, &mut Transform, &MeshMaterial3d<StandardMaterial>)>,
) {
    let eye = eye.map(|eye| eye.translation());
    for (entity, mut beam, mut transform, look) in &mut beams {
        beam.age += time.delta_secs();
        let Some(opacity) = beam.opacity() else {
            commands.entity(entity).despawn();
            continue;
        };
        *transform = beam.transform(eye);
        if let Some(mut material) = materials.get_mut(&look.0) {
            material.base_color = beam.tinted(opacity);
        }
    }
}

fn clear_beams(mut commands: Commands, beams: Query<Entity, With<Beam>>) {
    for beam in &beams {
        commands.entity(beam).despawn();
    }
}

/// Ungated, as streaks are: a client draws every shot. After `mark_tracers`,
/// so a beam starts the frame its shot arrives.
fn add_beams(app: &mut App) {
    app.add_message::<ShotPath>()
        .add_systems(Update, (raise_beams.after(mark_tracers), fade_beams).chain());
}

#[derive(Resource)]
struct SparkLook(Handle<EffectAsset>);

/// One burst's emitter; `age` is seconds of the particles' clock since it fired.
#[derive(Component)]
#[require(crate::game::replay::Unreplayed)]
struct BeamSparks {
    age: f32,
}

fn spark_effect() -> EffectAsset {
    let writer = ExprWriter::new();
    // From the emitter, which stands at the barrel, to where the shot stopped.
    let span = writer.add_property("span", Vec3::NEG_Z.into());
    let color = writer.add_property("color", Vec3::ONE.into());

    let age = || writer.attr(Attribute::AGE);
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr());
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(SPARK_LIFETIME).expr());

    // A random direction: a normal deviate per axis, normalized, is uniform on the sphere.
    let random_direction = || {
        let normal = || {
            let u = writer.rand(ScalarType::Float).max(writer.lit(1.0e-6));
            let v = writer.rand(ScalarType::Float);
            (writer.lit(-2.0) * u.log()).sqrt() * (v * writer.lit(std::f32::consts::TAU)).cos()
        };
        writer.lit(Vec3::X) * normal() + writer.lit(Vec3::Y) * normal() + writer.lit(Vec3::Z) * normal()
    };
    let along = writer.prop(span) * writer.rand(ScalarType::Float);
    let off = random_direction().normalized() * writer.rand(ScalarType::Float) * writer.lit(SPARK_SCATTER);
    let init_position = SetAttributeModifier::new(Attribute::POSITION, (along + off).expr());
    let speed = writer.lit(SPARK_SPEED.0).uniform(writer.lit(SPARK_SPEED.1));
    let init_velocity = SetAttributeModifier::new(Attribute::VELOCITY, (random_direction().normalized() * speed).expr());
    let init_size = SetAttributeModifier::new(Attribute::SIZE, writer.lit(SPARK_SIZE.0).uniform(writer.lit(SPARK_SIZE.1)).expr());

    // Not `LinearDragModifier`, whose `v *= 1 - k * dt` depends on frame rate.
    let braked = writer.attr(Attribute::VELOCITY) * (writer.lit(-SPARK_BRAKE) * writer.delta_time()).exp();
    let update_velocity = SetAttributeModifier::new(Attribute::VELOCITY, braked.expr());

    let fade_in = age() / writer.lit(SPARK_FADE_IN);
    let fade_out = (writer.lit(SPARK_LIFETIME) - age()) / writer.lit(SPARK_FADE_OUT);
    let alpha = fade_in.min(fade_out).saturate() * writer.lit(MOST_OPAQUE);
    let update_color = SetAttributeModifier::new(Attribute::HDR_COLOR, writer.prop(color).vec4_xyz_w(alpha).expr());

    let module = writer.finish();
    EffectAsset::new(SPARKS, SpawnerSettings::once((SPARKS as f32).into()), module)
        .with_name("beam_sparks")
        .with_alpha_mode(bevy_hanabi::AlphaMode::Blend)
        .init(init_age)
        .init(init_lifetime)
        .init(init_position)
        .init(init_velocity)
        .init(init_size)
        .update(update_velocity)
        .update(update_color)
        .render(OrientModifier::new(OrientMode::FaceCameraPosition))
}

fn spark_properties(beam: &Beam) -> EffectProperties {
    let mut properties = EffectProperties::default();
    properties.set("span", (beam.to - beam.from).into());
    properties.set("color", Vec3::new(beam.color.red, beam.color.green, beam.color.blue).into());
    properties
}

fn make_the_sparks(mut commands: Commands, mut effects: ResMut<Assets<EffectAsset>>) {
    commands.insert_resource(SparkLook(effects.add(spark_effect())));
}

/// See [`crate::game::warm_up`].
fn warm_up_the_sparks(mut commands: Commands, look: Res<SparkLook>) {
    let beam = Beam { from: Vec3::ZERO, to: Vec3::NEG_Z, age: 0.0, color: LinearRgba::WHITE };
    commands.spawn((
        WarmUp,
        ParticleEffect::new(look.0.clone()),
        spark_properties(&beam),
        Transform::from_translation(Vec3::NEG_Z * 4.0),
        RenderLayers::layer(crate::common::render_layers::WARM_UP),
        Name::new("Beam sparks warm-up"),
    ));
}

/// Held until the particles' clock reaches the shot, so in the kill cam the
/// sparks start with the beam.
fn spark_beams(
    mut commands: Commands,
    mode: Res<State<AppMode>>,
    time: Res<Time>,
    particles: Res<Time<EffectSimulation>>,
    look: Res<SparkLook>,
    new: Query<&Beam, Added<Beam>>,
    mut held: Local<HeldBursts<Beam>>,
) {
    // A burst held through a kill cam is not set off after the round.
    if !mode.get().simulates() {
        held.clear();
        return;
    }
    for beam in &new {
        held.hold(time.elapsed_secs_f64(), *beam);
    }
    for beam in held.due(particles.elapsed_secs_f64()) {
        commands.spawn((
            BeamSparks { age: 0.0 },
            ParticleEffect::new(look.0.clone()),
            spark_properties(&beam),
            Transform::from_translation(beam.from),
            Name::new("Beam sparks"),
        ));
    }
}

fn end_the_sparks(mut commands: Commands, time: Res<Time<EffectSimulation>>, mut sparks: Query<(Entity, &mut BeamSparks)>) {
    for (entity, mut spark) in &mut sparks {
        spark.age += time.delta_secs();
        if spark.age >= SPARKS_GONE {
            commands.entity(entity).despawn();
        }
    }
}

fn clear_the_sparks(mut commands: Commands, sparks: Query<Entity, With<BeamSparks>>) {
    for spark in &sparks {
        commands.entity(spark).despawn();
    }
}

/// Added in `main.rs` beside `StreakPlugin`: it builds a mesh, an image,
/// materials and a Hanabi effect, which the game's tests have no renderer for.
pub struct BeamPlugin;

impl Plugin for BeamPlugin {
    fn build(&self, app: &mut App) {
        warm_up::ensure(app);
        if !app.is_plugin_added::<HanabiPlugin>() {
            app.add_plugins(HanabiPlugin);
        }
        add_beams(app);
        app.add_systems(Startup, (make_the_look, warm_up, make_the_sparks, warm_up_the_sparks).chain())
            .add_systems(Update, (spark_beams.after(raise_beams), end_the_sparks))
            .add_systems(OnExit(AppMode::Play), (clear_beams, clear_the_sparks))
            .add_systems(OnExit(AppMode::Weapon), (clear_beams, clear_the_sparks));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::effects::Effect;

    fn a_beam() -> Beam {
        Beam { from: Vec3::ZERO, to: Vec3::new(0.0, 0.0, -40.0), age: 0.0, color: LinearRgba::WHITE }
    }

    /// Fails if the beam does not rise to full over a quarter second, fall
    /// straight away over the next second, and end.
    #[test]
    fn a_beam_fades_in_then_out_and_ends() {
        let mut beam = a_beam();
        assert_eq!(beam.opacity(), Some(0.0));
        beam.age = FADE_IN * 0.5;
        assert!((beam.opacity().unwrap() - 0.5).abs() < 1.0e-5);
        beam.age = FADE_IN;
        assert!((beam.opacity().unwrap() - 1.0).abs() < 1.0e-5);
        beam.age = FADE_IN + FADE_OUT * 0.5;
        assert!((beam.opacity().unwrap() - 0.5).abs() < 1.0e-5);
        beam.age = FADE_IN + FADE_OUT + 0.01;
        assert_eq!(beam.opacity(), None);
    }

    /// Fails if the beam is ever drawn more opaque than 80%.
    #[test]
    fn a_beam_is_never_more_than_four_fifths_opaque() {
        let mut beam = a_beam();
        beam.age = FADE_IN;
        assert!((beam.tinted(beam.opacity().unwrap()).alpha() - MOST_OPAQUE).abs() < 1.0e-5);
    }

    /// Fails if the edges are hard, or the middle is dim.
    #[test]
    fn the_ribbon_is_solid_in_the_middle_and_gone_at_the_edges() {
        assert_eq!(profile(0.0), 1.0);
        assert!(profile(1.0) < 0.01 && profile(-1.0) < 0.01);
        assert!(profile(0.5) > profile(0.8));
    }

    /// Fails if the beam does not span the whole path, end to end, or does not
    /// turn its face toward the eye.
    #[test]
    fn a_beam_runs_from_the_barrel_to_where_the_shot_stopped_facing_the_eye() {
        let drawn = a_beam().transform(None);
        assert_eq!(drawn.translation, Vec3::new(0.0, 0.0, -20.0));
        assert_eq!(drawn.scale.y, 40.0);
        assert!((drawn.rotation * Vec3::Y).abs_diff_eq(Vec3::NEG_Z, 1.0e-5));

        let eye = Vec3::new(3.0, 0.0, -10.0);
        let drawn = a_beam().transform(Some(eye));
        assert!((drawn.rotation * Vec3::Y).abs_diff_eq(Vec3::NEG_Z, 1.0e-5));
        assert!((drawn.rotation * Vec3::Z).abs_diff_eq(Vec3::X, 1.0e-5), "{}", drawn.rotation * Vec3::Z);
    }

    /// Fails if the beam is a single ribbon, which a second camera can see edge-on.
    #[test]
    fn the_beam_is_two_crossed_ribbons() {
        let mesh = crossed_ribbons();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else {
            panic!("no normals");
        };
        assert_eq!(normals.len(), 8);
        assert!(normals[..4].iter().all(|n| Vec3::from(*n).abs_diff_eq(Vec3::Z, 1.0e-5)));
        assert!(normals[4..].iter().all(|n| Vec3::from(*n).abs_diff_eq(Vec3::X, 1.0e-5)));
    }

    fn an_app_that_draws_beams() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<EffectSimulation>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_message::<Effect>()
            .insert_resource(BeamLook { mesh: Handle::default(), profile: Handle::default() })
            .add_systems(Update, mark_tracers);
        add_beams(&mut app);
        app
    }

    fn tracer(by: Option<PlayerId>, trail: HitscanTrail) -> Effect {
        Effect::Tracer { from: Vec3::ZERO, to: Vec3::NEG_Z * 20.0, hit: false, by, trail }
    }

    /// Fails if a streak weapon's shot raises a beam, or a beam weapon's does not.
    #[test]
    fn only_a_beam_weapons_shot_raises_a_beam() {
        let mut app = an_app_that_draws_beams();
        app.world_mut().write_message(tracer(None, HitscanTrail::Streak));
        app.world_mut().write_message(tracer(None, HitscanTrail::Beam));
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&Beam>().iter(world).count(), 1);
    }

    /// Fails if the beam is not in its shooter's team color, or their player
    /// color without a team.
    #[test]
    fn a_beam_is_its_shooters_color() {
        let mut app = an_app_that_draws_beams();
        let purple = PlayerColor::nth(0.0, 3);
        app.world_mut().spawn((Player::default(), PlayerId(1), Team::Blue, Transform::default()));
        app.world_mut().spawn((Player::default(), PlayerId(2), purple, Transform::default()));
        app.world_mut().write_message(tracer(Some(PlayerId(1)), HitscanTrail::Beam));
        app.world_mut().write_message(tracer(Some(PlayerId(2)), HitscanTrail::Beam));
        app.update();
        let world = app.world_mut();
        let mut colors: Vec<LinearRgba> = world.query::<&Beam>().iter(world).map(|beam| beam.color).collect();
        let mut wanted = vec![glow_of(Some(Team::Blue), None), glow_of(None, Some(purple))];
        let key = |color: &LinearRgba| (color.red * 1000.0) as i32;
        colors.sort_by_key(key);
        wanted.sort_by_key(key);
        assert_eq!(colors, wanted);
        assert_ne!(colors[0], colors[1]);
    }

    /// Fails if a spark's emitter ages on `Time` rather than the particles'
    /// clock, so the kill cam would take its sparks away while they are frozen.
    #[test]
    fn sparks_are_kept_while_the_particles_clock_is_stopped() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)))
            .init_resource::<Time<EffectSimulation>>()
            .add_systems(Update, end_the_sparks);
        let sparks = app.world_mut().spawn(BeamSparks { age: 0.0 }).id();
        for _ in 0..30 {
            app.update();
        }
        assert!(app.world().get_entity(sparks).is_ok());
    }
}
