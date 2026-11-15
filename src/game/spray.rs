//! Sprays: a picture each player puts on a wall with `T`, one at a time, lasting until the round
//! ends or they disconnect. See "Sprays" in `documentation/decals.md`.
//!
//! The picture is painted by its owner's machine ([`paint_our_spray`]) and goes to the server as a
//! PNG, which numbers its owner and sends every picture to every client. Where a spray is, is
//! decided on the authority and replicated as a [`SprayAt`]; every machine cuts it from its own
//! walls once it holds that owner's picture.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::decal::Projection;
use crate::common::net::{has_authority, NetRole};
use crate::common::swatch::Swatch;
use crate::game::death::Dying;
use crate::game::decal::{blended_material, decal_image, surface_hit, Decal, DecalMaterial, Lit, Spray, Surfaces, Uncut};
use crate::game::hitscan::{aim, eye};
use crate::game::player::{Inputs, LocalPlayer, PhysicsBody, Player};
use crate::game::score::Standing;
use crate::get;

/// Pixels on a side.
pub const PICTURE_SIZE: u32 = 512;
/// A picture bigger than this is refused unread.
const MOST_BYTES: usize = 512 * 1024;
/// Meters from the eye.
pub const REACH: f32 = 2.0;
/// Meters across.
const SIZE: f32 = 0.9;
const DEPTH: f32 = 0.3;
/// The listen server's or solo player's own number; clients are numbered from 1.
const HOST: u64 = 0;
const FONT: &str = "fonts/Alfa_Slab_One/AlfaSlabOne-Regular.ttf";

/// The authority is the one writer.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SprayAt {
    pub owner: u64,
    pub projection: Projection,
}

/// A client's number for its spray, on its link entity, given when its first picture arrives.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SprayOwner(pub u64);

/// On the server: the link whose spray this is. Its disconnect despawns the spray, and the
/// despawn replicates.
#[derive(Component)]
#[relationship(relationship_target = Sprays)]
pub struct SprayedBy(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = SprayedBy, linked_spawn)]
pub struct Sprays(Vec<Entity>);

/// Client to server: this machine's picture, as a PNG.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SprayPicture {
    pub png: Vec<u8>,
}

/// Server to client: somebody's picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SprayPictureOf {
    pub owner: u64,
    pub png: Vec<u8>,
}

/// Reliable and ordered, so a newer picture is never overtaken by an older one.
pub struct SprayChannel;

#[derive(Resource, Default)]
pub struct SprayArt {
    pictures: HashMap<u64, Vec<u8>>,
    materials: HashMap<u64, Handle<StandardMaterial>>,
    next_owner: u64,
    /// Owners whose picture changed since the server last sent pictures out.
    to_send: Vec<u64>,
    /// On the server: which link each client's number belongs to, so its picture goes with it.
    links: HashMap<Entity, u64>,
}

impl SprayArt {
    pub fn material(&self, owner: u64) -> Option<&Handle<StandardMaterial>> {
        self.materials.get(&owner)
    }

    /// Holds `png` as `owner`'s picture if it is a PNG of [`PICTURE_SIZE`] on a side.
    fn adopt(&mut self, owner: u64, png: Vec<u8>, images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>) -> bool {
        if png.len() > MOST_BYTES {
            warn!("Refused a spray of {} bytes", png.len());
            return false;
        }
        let image = match decal_image(&png) {
            Ok(image) if image.width() == PICTURE_SIZE && image.height() == PICTURE_SIZE => image,
            Ok(image) => {
                warn!("Refused a spray {}×{} pixels", image.width(), image.height());
                return false;
            }
            Err(e) => {
                warn!("Refused a spray that will not decode: {e}");
                return false;
            }
        };
        self.materials.insert(owner, materials.add(blended_material(images.add(image))));
        self.pictures.insert(owner, png);
        self.to_send.push(owner);
        true
    }
}

#[derive(Resource, Default)]
struct SprayFont(Option<FontVec>);

#[derive(Resource, Default)]
pub struct OurSpray {
    painted_from: Option<(String, Swatch)>,
    pub png: Option<Vec<u8>>,
}

fn load_the_font(assets: Res<crate::common::assets::Assets>, mut font: ResMut<SprayFont>) {
    font.0 = assets.read_from_packs(FONT).and_then(|bytes| match FontVec::try_from_vec(bytes) {
        Ok(font) => Some(font),
        Err(e) => {
            warn!("{FONT} will not read, so sprays carry no name: {e}");
            None
        }
    });
}

/// A disk in `swatch`'s color with `name` across it in black, as a PNG.
pub fn paint(name: &str, swatch: Swatch, font: Option<&FontVec>) -> Option<Vec<u8>> {
    use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke, Transform};
    let size = PICTURE_SIZE as f32;
    let mut pixmap = Pixmap::new(PICTURE_SIZE, PICTURE_SIZE)?;
    let rgba = |color: Color| {
        let c = color.to_srgba();
        tiny_skia::Color::from_rgba(c.red, c.green, c.blue, c.alpha).unwrap_or(tiny_skia::Color::BLACK)
    };
    let disk = PathBuilder::from_circle(size / 2.0, size / 2.0, size / 2.0 - 16.0)?;
    let mut paint = Paint { anti_alias: true, ..default() };
    paint.set_color(rgba(swatch.base));
    pixmap.fill_path(&disk, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    paint.set_color(rgba(swatch.shadow));
    pixmap.stroke_path(&disk, &paint, &Stroke { width: 14.0, ..default() }, Transform::identity(), None);

    if let Some(font) = font {
        write_centered(&mut pixmap, font, name);
    }
    pixmap.encode_png().ok()
}

fn name_on_the_spray(name: &str) -> String {
    match name.trim().is_empty() {
        true => get!("spray.nameless"),
        false => name.trim().to_owned(),
    }
}

/// Pixels. A name too wide at this size is cut short.
const SMALLEST: f32 = 28.0;

/// Black, on one line, as large as fits across the disk.
fn write_centered(pixmap: &mut tiny_skia::Pixmap, font: &FontVec, text: &str) {
    const WIDEST: f32 = 400.0;
    const TALLEST: f32 = 150.0;
    let width_at = |text: &str, px: f32| {
        let scaled = font.as_scaled(PxScale::from(px));
        let mut previous = None;
        text.chars().fold(0.0, |x, c| {
            let id = scaled.glyph_id(c);
            let kern = previous.map_or(0.0, |p| scaled.kern(p, id));
            previous = Some(id);
            x + kern + scaled.h_advance(id)
        })
    };
    let mut text = text.to_owned();
    let width = width_at(&text, TALLEST);
    if width <= 0.0 {
        return;
    }
    let px = (TALLEST * (WIDEST / width).min(1.0)).max(SMALLEST);
    let mut kept: Vec<char> = text.chars().collect();
    while width_at(&text, px) > WIDEST && kept.len() > 1 {
        kept.pop();
        text = format!("{}\u{2026}", kept.iter().collect::<String>().trim_end());
    }
    let scaled = font.as_scaled(PxScale::from(px));
    let size = PICTURE_SIZE as f32;
    let mut x = (size - width_at(&text, px)) / 2.0;
    let baseline = (size + scaled.ascent() + scaled.descent()) / 2.0;
    let mut previous = None;
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let pixels = pixmap.data_mut();
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        x += previous.map_or(0.0, |p| scaled.kern(p, id));
        previous = Some(id);
        let glyph = id.with_scale_and_position(px, ab_glyph::point(x, baseline));
        x += scaled.h_advance(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let corner = outline.px_bounds().min;
        outline.draw(|gx, gy, coverage| {
            let (px, py) = (corner.x as i32 + gx as i32, corner.y as i32 + gy as i32);
            if px < 0 || py < 0 || px >= w || py >= h {
                return;
            }
            // Black over premultiplied RGBA.
            let i = ((py * w + px) * 4) as usize;
            let keep = 1.0 - coverage.clamp(0.0, 1.0);
            for c in 0..3 {
                pixels[i + c] = (pixels[i + c] as f32 * keep).round() as u8;
            }
            pixels[i + 3] = (255.0 - (255.0 - pixels[i + 3] as f32) * keep).round() as u8;
        });
    }
}

/// From the standing, so a dealt name is painted too.
fn paint_our_spray(
    ours: Option<Single<&PlayerId, (With<LocalPlayer>, With<Player>)>>,
    standings: Query<&Standing>,
    font: Res<SprayFont>,
    mut spray: ResMut<OurSpray>,
) {
    let Some(id) = ours else { return };
    let Some(standing) = standings.iter().find(|standing| standing.id == **id) else { return };
    let swatch = Swatch::of_body(standing.team, Some(standing.color));
    if spray.painted_from.as_ref().is_some_and(|(name, painted)| *painted == swatch && *name == name_on_the_spray(&standing.name)) {
        return;
    }
    let wanted = (name_on_the_spray(&standing.name), swatch);
    spray.png = paint(&wanted.0, wanted.1, font.0.as_ref());
    spray.painted_from = Some(wanted);
}

/// A host's own picture is held under [`HOST`]; a client's goes to the server, on connecting and
/// on every change.
fn share_our_spray(
    role: Res<NetRole>,
    spray: Res<OurSpray>,
    mut art: ResMut<SprayArt>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clients: Query<(&mut MessageSender<SprayPicture>, Ref<Connected>), With<Client>>,
) {
    let Some(png) = &spray.png else { return };
    if role.is_authority() {
        if spray.is_changed() {
            art.adopt(HOST, png.clone(), &mut images, &mut materials);
        }
        return;
    }
    for (mut sender, connected) in &mut clients {
        if connected.is_added() || spray.is_changed() {
            sender.send::<SprayChannel>(SprayPicture { png: png.clone() });
        }
    }
}

/// On the server: each client's picture, checked, under the number its link was given.
fn take_client_pictures(
    mut commands: Commands,
    mut art: ResMut<SprayArt>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut links: Query<(Entity, &mut MessageReceiver<SprayPicture>, Option<&SprayOwner>), With<ClientOf>>,
) {
    for (link, mut receiver, owner) in &mut links {
        let Some(latest) = receiver.receive().last() else { continue };
        let owner = owner.map_or_else(
            || {
                art.next_owner += 1;
                let owner = art.next_owner;
                commands.entity(link).insert(SprayOwner(owner));
                art.links.insert(link, owner);
                owner
            },
            |owner| owner.0,
        );
        art.adopt(owner, latest.png, &mut images, &mut materials);
    }
}

/// A client gone takes its picture with it, so a joiner is not sent it.
fn forget_departed_pictures(mut art: ResMut<SprayArt>, mut gone: RemovedComponents<SprayOwner>) {
    for link in gone.read() {
        if let Some(owner) = art.links.remove(&link) {
            art.pictures.remove(&owner);
            art.materials.remove(&owner);
        }
    }
}

/// A new role means new people: the pictures held for the last one are dropped, and our own is
/// shared again under this role's number.
fn forget_pictures_on_a_new_role(mut art: ResMut<SprayArt>, mut spray: ResMut<OurSpray>) {
    *art = SprayArt::default();
    spray.set_changed();
}

fn send_pictures(
    mut art: ResMut<SprayArt>,
    server: Option<Single<&Endpoint, With<Server>>>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let changed = std::mem::take(&mut art.to_send);
    let Some(server) = server else { return Ok(()) };
    for owner in changed {
        if let Some(png) = art.pictures.get(&owner) {
            sender.send::<_, SprayChannel>(&SprayPictureOf { owner, png: png.clone() }, &server, &NetworkTarget::All)?;
        }
    }
    for client in &joined {
        for (owner, png) in &art.pictures {
            sender.send_to_entities::<_, SprayChannel>(&SprayPictureOf { owner: *owner, png: png.clone() }, core::iter::once(client))?;
        }
    }
    Ok(())
}

fn take_the_servers_pictures(
    role: Res<NetRole>,
    mut art: ResMut<SprayArt>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut receivers: Query<&mut MessageReceiver<SprayPictureOf>>,
) {
    if role.is_authority() {
        return;
    }
    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            art.adopt(message.owner, message.png, &mut images, &mut materials);
        }
    }
}

/// Bots have no picture and do not spray.
#[allow(clippy::type_complexity)]
fn spray(
    mut commands: Commands,
    bodies: Query<(&Inputs, &PhysicsBody, &Player, &crate::common::class::Stance, Option<&ControlledBy>, Has<LocalPlayer>), Without<Dying>>,
    owners: Query<&SprayOwner>,
    bakes: Surfaces,
    mut sprays: Query<(Entity, &mut SprayAt)>,
) {
    for (input, body, player, stance, controlled, ours) in &bodies {
        if !input.spray {
            continue;
        }
        let (owner, link) = match (controlled, ours) {
            (Some(controlled), _) => match owners.get(controlled.owner) {
                Ok(owner) => (owner.0, Some(controlled.owner)),
                Err(_) => continue,
            },
            (None, true) => (HOST, None),
            (None, false) => continue,
        };
        let from = eye(body.current, *stance, player.duck, player.eye);
        let looking = *aim(player);
        let Some((at, normal)) = surface_hit(&bakes, from, looking, REACH) else { continue };
        let placed = SprayAt { owner, projection: Projection::on(at, normal, SIZE, DEPTH, looking) };
        match sprays.iter_mut().find(|(_, spray)| spray.owner == owner) {
            // Compared first: a written `SprayAt` is replicated and recut everywhere.
            Some((_, mut spray)) => {
                if *spray != placed {
                    *spray = placed;
                }
            }
            None => {
                let mut spawned = commands.spawn((placed, Replicate::to_clients(NetworkTarget::All)));
                if let Some(link) = link {
                    spawned.insert(SprayedBy(link));
                }
            }
        }
    }
}

/// On the authority only: a client's copies go with the despawns it is sent.
fn clear_sprays(mut commands: Commands, sprays: Query<Entity, With<SprayAt>>) {
    for spray in &sprays {
        commands.entity(spray).despawn();
    }
}

/// On every machine: a spray is drawn once its owner's picture is here, and cut again when it moves
/// or its picture changes.
fn dress_sprays(mut commands: Commands, sprays: Query<(Entity, Ref<SprayAt>, Option<&DecalMaterial>)>, art: Res<SprayArt>) {
    for (entity, at, drawn) in &sprays {
        let Some(material) = art.material(at.owner) else { continue };
        if at.is_changed() || drawn.is_none_or(|drawn| drawn.0 != *material) {
            commands
                .entity(entity)
                .insert((Decal(at.projection), DecalMaterial(material.clone()), Spray { owner: at.owner }, Lit, Uncut));
        }
    }
}

/// The wire half: added beside the transport's other plugins, on every process, so both ends
/// register the same channel and messages.
pub struct SpraySyncPlugin;

impl Plugin for SpraySyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<SprayChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<SprayPicture>().add_direction(NetworkDirection::ClientToServer);
        app.register_message::<SprayPictureOf>().add_direction(NetworkDirection::ServerToClient);
    }
}

/// Added in `main.rs` beside [`crate::game::decal::DecalPlugin`].
pub struct SprayPlugin;

impl Plugin for SprayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SprayArt>()
            .init_resource::<SprayFont>()
            .init_resource::<OurSpray>()
            .add_systems(Startup, load_the_font)
            .add_systems(
                Update,
                (
                    forget_pictures_on_a_new_role.run_if(resource_changed::<NetRole>),
                    paint_our_spray,
                    share_our_spray,
                    take_client_pictures,
                    forget_departed_pictures,
                    send_pictures,
                    take_the_servers_pictures,
                    dress_sprays,
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Play), clear_sprays.run_if(has_authority))
            .add_systems(
                FixedUpdate,
                spray.run_if(has_authority).run_if(in_state(AppMode::Play)).after(crate::game::player::step_player),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_painted_spray_is_a_square_picture_with_the_name_on_it() {
        let font = crate::common::assets::Assets::default().read_from_packs(FONT).and_then(|bytes| FontVec::try_from_vec(bytes).ok());
        assert!(font.is_some(), "{FONT} is missing");
        let png = paint("Grackle", Swatch::team(crate::common::team::Team::Red), font.as_ref()).unwrap();
        let image = decal_image(&png).unwrap();
        assert_eq!((image.width(), image.height()), (PICTURE_SIZE, PICTURE_SIZE));
        let data = image.data.as_ref().unwrap();
        let at = |x: u32, y: u32| &data[((y * PICTURE_SIZE + x) * 4) as usize..][..4];
        assert_eq!(at(2, 2)[3], 0, "outside the disk is clear");
        let row = PICTURE_SIZE / 2;
        assert!((0..PICTURE_SIZE).any(|x| at(x, row)[..3].iter().all(|c| *c < 40) && at(x, row)[3] == 255), "the name is black");
    }

    #[test]
    fn leaving_play_clears_every_spray() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let projection = Projection::on(Vec3::ZERO, Vec3::Z, SIZE, DEPTH, Vec3::NEG_Z);
        world.spawn(SprayAt { owner: HOST, projection });
        world.spawn(SprayAt { owner: 3, projection });
        world.run_system_once(clear_sprays).unwrap();
        assert_eq!(world.query::<&SprayAt>().iter(&world).count(), 0);
    }

    #[test]
    fn a_long_name_is_cut_short_rather_than_shrunk_to_a_dot() {
        let font = crate::common::assets::Assets::default().read_from_packs(FONT).and_then(|bytes| FontVec::try_from_vec(bytes).ok()).unwrap();
        assert!(paint(&"W".repeat(200), Swatch::neutral(), Some(&font)).is_some());
    }

    #[test]
    fn a_blank_name_is_written_as_player() {
        assert_eq!(name_on_the_spray("   "), "Player");
        assert_eq!(name_on_the_spray(" Ada "), "Ada");
    }

    #[test]
    fn a_picture_of_the_wrong_size_is_refused() {
        let mut art = SprayArt::default();
        let (mut images, mut materials) = (Assets::<Image>::default(), Assets::<StandardMaterial>::default());
        let small = tiny_skia::Pixmap::new(64, 64).unwrap().encode_png().unwrap();
        assert!(!art.adopt(1, small, &mut images, &mut materials));
        assert!(art.material(1).is_none());
    }
}
