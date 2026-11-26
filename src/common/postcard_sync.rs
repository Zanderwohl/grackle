//! The map's chosen postcards, printed on the authority whenever [`CapturedPostcards`] or
//! [`MapMetadata::postcards`] changes, and sent to clients as PNGs, so a client prints nothing and
//! has them from the moment it joins. [`PrintedPostcards`] is replaced only once every card of a
//! set is ready, so a set never arrives a card at a time.

use bevy::prelude::*;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::net::NetRole;
use crate::common::postcard_print;
use crate::editor::map_metadata::MapMetadata;
use crate::game::decal::with_mips;
use crate::tool::postcard_bake::CapturedPostcards;

/// A received card larger than this is refused.
const MOST_BYTES: usize = 4 * 1024 * 1024;

/// Ordered and reliable: the last set sent is the true one.
pub struct PostcardChannel;

/// The PNG of each printed card, in the map's order.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct PrintedCards(pub Vec<Vec<u8>>);

/// The cards on show, in the map's order.
#[derive(Resource, Default)]
pub struct PrintedPostcards {
    cards: Vec<Handle<Image>>,
}

impl PrintedPostcards {
    pub fn cards(&self) -> &[Handle<Image>] {
        &self.cards
    }

    fn replace(&mut self, cards: Vec<Image>, images: &mut Assets<Image>) {
        for handle in self.cards.drain(..) {
            images.remove(&handle);
        }
        self.cards = cards.into_iter().map(|card| images.add(card)).collect();
    }
}

/// Tasks on the compute pool, one per card, taken only once all are done.
struct Batch<T> {
    tasks: Vec<Task<T>>,
    done: Vec<Option<T>>,
}

impl<T: Send + 'static> Batch<T> {
    fn new(tasks: Vec<Task<T>>) -> Self {
        let done = tasks.iter().map(|_| None).collect();
        Batch { tasks, done }
    }

    fn poll(&mut self) -> Option<Vec<T>> {
        for (task, done) in self.tasks.iter_mut().zip(&mut self.done) {
            if done.is_none() {
                *done = check_ready(task);
            }
        }
        self.done.iter().all(Option::is_some).then(|| self.done.drain(..).flatten().collect())
    }
}

#[derive(Resource, Default)]
struct Press {
    /// The kept set's generation and the choice last printed from, on the authority.
    printed_from: Option<(u64, Vec<String>)>,
    printing: Option<Batch<(Vec<u8>, Image)>>,
    /// The server's cards, on a client; a card that will not decode is `None`.
    decoding: Option<Batch<Option<Image>>>,
    /// What the authority printed last, as sent.
    pngs: PrintedCards,
    /// Bumped whenever `pngs` is replaced, so sending compares a number, not megabytes.
    pngs_generation: u64,
    /// What a client was sent last.
    received: PrintedCards,
}

pub struct PostcardSyncPlugin;

impl Plugin for PostcardSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<PostcardChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<PrintedCards>().add_direction(NetworkDirection::ServerToClient);

        app.init_resource::<Press>().init_resource::<PrintedPostcards>().add_systems(
            Update,
            (
                forget_the_postcards.run_if(resource_changed::<NetRole>),
                print_the_postcards,
                receive_the_postcards,
                take_the_finished_cards,
                send_postcards,
            )
                .chain(),
        );
    }
}

/// Each chosen postcard `captured` holds a picture of, once, in the map's order.
fn chosen_pictures(metadata: &MapMetadata, captured: &CapturedPostcards) -> Vec<(String, image::RgbaImage)> {
    let mut pictures: Vec<(String, image::RgbaImage)> = Vec::new();
    for choice in metadata.postcards.iter().flatten() {
        let stem = choice.key();
        if pictures.iter().any(|(taken, _)| *taken == stem) {
            continue;
        }
        let Some(picture) = captured.picture(&stem) else { continue };
        pictures.push((stem, picture.clone()));
    }
    pictures
}

fn print_one(stem: String, picture: image::RgbaImage) -> (Vec<u8>, Image) {
    let card = postcard_print::postcard(&picture);
    let mut png = Vec::new();
    if let Err(e) = card.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
        error!("postcard {stem} will not encode: {e}");
    }
    (png, with_mips(postcard_print::WIDTH, postcard_print::HEIGHT, card.into_raw()))
}

/// Cards the authority printed stay while it stays the authority, as from solo to hosting.
fn forget_the_postcards(
    role: Res<NetRole>,
    mut press: ResMut<Press>,
    mut printed: ResMut<PrintedPostcards>,
    mut images: ResMut<Assets<Image>>,
) {
    if role.is_authority() && press.printed_from.is_some() {
        return;
    }
    *press = Press::default();
    printed.replace(Vec::new(), &mut images);
}

/// A new set or choice drops whatever was being printed from the old one.
fn print_the_postcards(
    role: Option<Res<NetRole>>,
    metadata: Option<Res<MapMetadata>>,
    captured: Option<Res<CapturedPostcards>>,
    mut press: ResMut<Press>,
) {
    if !role.is_none_or(|role| role.is_authority()) {
        return;
    }
    let (Some(metadata), Some(captured)) = (metadata, captured) else { return };
    let chosen: Vec<String> = metadata.postcards.iter().flatten().map(|choice| choice.key()).collect();
    let printed_from = Some((captured.generation(), chosen));
    if press.printed_from == printed_from {
        return;
    }
    press.printed_from = printed_from;
    let tasks = chosen_pictures(&metadata, &captured)
        .into_iter()
        .map(|(stem, picture)| AsyncComputeTaskPool::get().spawn(async move { print_one(stem, picture) }))
        .collect();
    press.printing = Some(Batch::new(tasks));
}

fn decode(png: &[u8]) -> Option<Image> {
    if png.len() > MOST_BYTES {
        warn!("Refused a postcard of {} bytes from the server", png.len());
        return None;
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(postcard_print::WIDTH);
    limits.max_image_height = Some(postcard_print::HEIGHT);
    reader.limits(limits);
    let card = match reader.decode() {
        Ok(card) => card.to_rgba8(),
        Err(e) => {
            warn!("A postcard from the server will not decode: {e}");
            return None;
        }
    };
    if card.dimensions() != (postcard_print::WIDTH, postcard_print::HEIGHT) {
        warn!("Refused a postcard of {}x{} from the server", card.width(), card.height());
        return None;
    }
    Some(with_mips(postcard_print::WIDTH, postcard_print::HEIGHT, card.into_raw()))
}

fn receive_the_postcards(role: Option<Res<NetRole>>, mut receivers: Query<&mut MessageReceiver<PrintedCards>>, mut press: ResMut<Press>) {
    if role.is_none_or(|role| role.is_authority()) {
        return;
    }
    let Some(latest) = receivers.iter_mut().flat_map(|mut receiver| receiver.receive().collect::<Vec<_>>()).last() else {
        return;
    };
    if latest == press.received {
        return;
    }
    press.received = latest.clone();
    let tasks = latest.0.into_iter().map(|png| AsyncComputeTaskPool::get().spawn(async move { decode(&png) })).collect();
    press.decoding = Some(Batch::new(tasks));
}

fn take_the_finished_cards(mut press: ResMut<Press>, mut printed: ResMut<PrintedPostcards>, mut images: ResMut<Assets<Image>>) {
    if let Some(cards) = press.printing.as_mut().and_then(Batch::poll) {
        press.printing = None;
        let (pngs, cards): (Vec<_>, Vec<_>) = cards.into_iter().unzip();
        if press.pngs.0 != pngs {
            press.pngs = PrintedCards(pngs);
            press.pngs_generation += 1;
            printed.replace(cards, &mut images);
        }
    }
    if let Some(cards) = press.decoding.as_mut().and_then(Batch::poll) {
        press.decoding = None;
        printed.replace(cards.into_iter().flatten().collect(), &mut images);
        info!("Took {} postcards from the server", printed.cards.len());
    }
}

fn send_postcards(
    press: Res<Press>,
    server: Option<Single<&Endpoint, With<Server>>>,
    joined: Query<Entity, (With<ClientOf>, Added<Connected>)>,
    mut sent: Local<u64>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    if press.pngs_generation != *sent {
        *sent = press.pngs_generation;
        info!("Postcards changed; sending {} to everyone", press.pngs.0.len());
        sender.send::<_, PostcardChannel>(&press.pngs, &server, &NetworkTarget::All)?;
        return Ok(());
    }
    for client in &joined {
        sender.send_to_entities::<_, PostcardChannel>(&press.pngs, core::iter::once(client))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// Fails if the set cannot cross the wire, which Lightyear encodes with postcard.
    #[test]
    fn the_cards_survive_the_wire_encoding() {
        let sent = PrintedCards(vec![png(30, 20), png(30, 20)]);
        let bytes = postcard::to_allocvec(&sent).expect("the cards would not encode");
        let received: PrintedCards = postcard::from_bytes(&bytes).expect("the cards would not decode");
        assert_eq!(received, sent);
    }

    #[test]
    fn only_a_card_of_a_card_s_size_is_taken() {
        assert!(decode(&png(postcard_print::WIDTH, postcard_print::HEIGHT)).is_some());
        assert!(decode(&png(30, 20)).is_none());
        assert!(decode(b"not a png").is_none());
    }

    #[test]
    fn a_printed_card_reads_back_as_a_card() {
        let picture = image::RgbaImage::from_pixel(postcard_print::PICTURE_WIDTH, postcard_print::PICTURE_HEIGHT, image::Rgba([40, 90, 160, 255]));
        let (png, _) = print_one("top".into(), picture);
        assert!(decode(&png).is_some());
    }

    #[test]
    fn only_chosen_postcards_with_pictures_are_printed_in_the_maps_order() {
        use crate::editor::editable::FeatureId;
        use crate::editor::map_metadata::{BuiltInPostcard, PostcardChoice};
        let mut captured = CapturedPostcards::default();
        captured.keep_for_test("top", image::RgbaImage::new(30, 20));
        captured.keep_for_test("camera_4", image::RgbaImage::new(30, 20));
        let mut metadata = MapMetadata::default();
        metadata.postcards = [
            Some(PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(4), copy: None }),
            Some(PostcardChoice::SpectatorCamera { feature: FeatureId::from_raw(9), copy: None }),
            Some(PostcardChoice::BuiltIn(BuiltInPostcard::Top)),
        ];
        let chosen = chosen_pictures(&metadata, &captured);
        let stems: Vec<&str> = chosen.iter().map(|(stem, _)| stem.as_str()).collect();
        assert_eq!(stems, ["camera_4", "top"]);
    }
}
