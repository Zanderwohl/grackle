//! The match's music. The authority picks each track from [`MapMetadata::music`] and sends a
//! [`MusicCue`]; every machine, the authority's included, plays it if one of its packs holds the
//! track. No cue is sent while only bots play.
//!
//! - [`CueStart::Now`] once the pre-game has under [`OPENING_CUE_SECONDS`] to run.
//! - [`CueStart::IfQuiet`] each time somebody new comes within one frag of the limit, a track
//!   other than the opening. Each machine decides whether anything is playing, since the server
//!   cannot hear a client.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackMode, PlaybackSettings, Volume};
use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::match_phase::{Countdown, MatchNumber, MatchPhase};
use crate::common::match_state::MatchChannel;
use crate::common::music::MusicLibrary;
use crate::common::net::{has_authority, is_local, is_remote_client, NetRole};
use crate::common::net_transport::NetLink;
use crate::common::player_settings::PlayerSettings;
use crate::common::round_settings::RoundSettings;
use crate::editor::map_metadata::MapMetadata;
use crate::game::match_cycle::people_playing;
use crate::game::respawn::{Identity, OurIdentity};
use crate::game::score::Standing;

/// Seconds left in the pre-game when the opening track starts.
pub const OPENING_CUE_SECONDS: f32 = 5.0;

#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MusicCue {
    /// A [`Track::path`](crate::common::music::Track::path).
    pub track: String,
    pub start: CueStart,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueStart {
    /// Stops whatever is playing.
    Now,
    IfQuiet,
}

/// Despawned when its track ends, which is what makes a machine quiet again.
#[derive(Component)]
pub struct Soundtrack;

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OnLan(pub bool);

/// Reset when the match number moves on.
#[derive(Resource, Debug, Default)]
struct Cued {
    match_number: u32,
    opened: bool,
    reported_no_music: bool,
    /// Every track cued this match, the opening first.
    tracks: Vec<String>,
    closing_in: BTreeSet<PlayerId>,
}

impl Cued {
    fn for_match(&mut self, number: u32) -> &mut Self {
        if self.match_number != number {
            *self = Cued { match_number: number, ..default() };
        }
        self
    }
}

pub struct SoundtrackSyncPlugin;

impl Plugin for SoundtrackSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<MusicCue>();
        app.register_message::<MusicCue>().add_direction(NetworkDirection::ServerToClient);
        app.add_systems(Update, send_cues.run_if(has_authority));
        app.add_systems(PreUpdate, receive_cues.after(MessageSystems::Receive).run_if(is_remote_client));
    }
}

/// Added in `launch.rs`, since it plays audio.
pub struct SoundtrackPlugin;

impl Plugin for SoundtrackPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<MusicCue>()
            .init_resource::<Cued>()
            .init_resource::<OnLan>()
            .init_resource::<MusicLibrary>()
            .add_systems(
                Update,
                (
                    (cue_the_opening.run_if(in_state(MatchPhase::PreGame)), cue_the_closing_in)
                        .run_if(in_state(AppMode::Play))
                        .run_if(has_authority),
                    notice_the_network,
                    play_cues,
                    drop_what_will_not_load,
                    say_what_is_heard,
                    follow_the_volume,
                )
                    .chain(),
            )
            .add_systems(OnEnter(MatchPhase::Live), cue_the_opening.run_if(has_authority))
            .add_systems(OnExit(AppMode::Play), stop_the_music);
    }
}

/// From the tracks in `list` not in `avoid`, else any but `avoid`'s first, which is the opening.
/// `None` when that leaves nothing.
pub fn choose(list: &[String], avoid: &[String], rng: &mut impl rand::Rng) -> Option<String> {
    let fresh: Vec<&String> = list.iter().filter(|track| !avoid.contains(track)).collect();
    let pool: Vec<&String> = match fresh.is_empty() {
        false => fresh,
        true => list.iter().filter(|track| Some(*track) != avoid.first()).collect(),
    };
    pool.choose(rng).map(|track| (*track).clone())
}

/// Whether `frags` is within one of `limit`. At least one frag, so a limit of one does not count
/// everybody at the start.
pub fn closing_in(frags: i32, limit: u32) -> bool {
    frags >= 1 && frags >= limit as i32 - 1
}

#[derive(bevy::ecs::system::SystemParam)]
struct People<'w, 's> {
    us: Option<Res<'w, OurIdentity>>,
    links: Query<'w, 's, &'static Identity, (With<ClientOf>, With<Connected>)>,
}

impl People<'_, '_> {
    fn any(&self) -> bool {
        people_playing(self.us.as_deref(), &self.links) > 0
    }
}

/// Runs through the pre-game and once on entering the live phase, for a pre-game shorter than the
/// mark. Not later: a person joining a match of bots counts as playing for a frame before the
/// cycle restarts it.
fn cue_the_opening(
    people: People,
    phase: Option<Res<State<MatchPhase>>>,
    countdown: Res<Countdown>,
    number: Res<MatchNumber>,
    metadata: Option<Res<MapMetadata>>,
    mut cued: ResMut<Cued>,
    mut cues: MessageWriter<MusicCue>,
) {
    let due = match phase.map(|phase| *phase.get()) {
        Some(MatchPhase::PreGame) => matches!(countdown.of(MatchPhase::PreGame), Some(Some(left)) if left < OPENING_CUE_SECONDS),
        Some(MatchPhase::Live) => true,
        _ => false,
    };
    let cued = cued.for_match(number.0);
    if cued.opened || !due || !people.any() {
        return;
    }
    // Marked only once a track is chosen, so a person arriving or a track added later still opens.
    let list = metadata.map(|metadata| metadata.music.clone()).unwrap_or_default();
    let Some(track) = choose(&list, &[], &mut rand::rng()) else {
        if !std::mem::replace(&mut cued.reported_no_music, true) {
            info!("The map lists no music, so none is initiated");
        }
        return;
    };
    cued.opened = true;
    info!("Initiating {track}");
    cued.tracks.push(track.clone());
    cues.write(MusicCue { track, start: CueStart::Now });
}

fn cue_the_closing_in(
    people: People,
    phase: Option<Res<State<MatchPhase>>>,
    number: Res<MatchNumber>,
    settings: Option<Res<RoundSettings>>,
    metadata: Option<Res<MapMetadata>>,
    standings: Query<&Standing>,
    mut cued: ResMut<Cued>,
    mut cues: MessageWriter<MusicCue>,
) {
    if phase.is_some_and(|phase| *phase.get() != MatchPhase::Live) || !people.any() {
        return;
    }
    let Some(limit) = settings.and_then(|settings| settings.frag_limit) else { return };
    let cued = cued.for_match(number.0);
    let mut fresh = false;
    for standing in &standings {
        if closing_in(standing.frags, limit) {
            fresh |= cued.closing_in.insert(standing.id);
        }
    }
    if !fresh {
        return;
    }
    let list = metadata.map(|metadata| metadata.music.clone()).unwrap_or_default();
    let Some(track) = choose(&list, &cued.tracks, &mut rand::rng()) else {
        info!("Somebody is one frag from the limit, but the map lists no track besides the opening");
        return;
    };
    info!("Initiating {track}, if nothing is playing");
    cued.tracks.push(track.clone());
    cues.write(MusicCue { track, start: CueStart::IfQuiet });
}

fn send_cues(
    mut cues: MessageReader<MusicCue>,
    server: Option<Single<&Endpoint, With<Server>>>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    for cue in cues.read() {
        sender.send::<_, MatchChannel>(cue, &server, &NetworkTarget::All)?;
    }
    Ok(())
}

fn receive_cues(mut receivers: Query<&mut MessageReceiver<MusicCue>>, mut cues: MessageWriter<MusicCue>) {
    for mut receiver in &mut receivers {
        for cue in receiver.receive() {
            cues.write(cue);
        }
    }
}

pub fn starts(cue: CueStart, playing: bool) -> bool {
    cue == CueStart::Now || !playing
}

fn music_gain(settings: Option<&PlayerSettings>, lan: OnLan) -> f32 {
    let sound = settings.map(|settings| settings.sound).unwrap_or_default();
    match sound.mute_music_on_lan && lan.0 {
        true => 0.0,
        false => sound.music_gain(),
    }
}

fn play_cues(
    mut commands: Commands,
    mut cues: MessageReader<MusicCue>,
    library: Res<MusicLibrary>,
    packs: Res<crate::common::assets::Assets>,
    asset_server: Option<Res<AssetServer>>,
    settings: Option<Res<PlayerSettings>>,
    lan: Res<OnLan>,
    playing: Query<Entity, With<Soundtrack>>,
) {
    let mut playing: Vec<Entity> = playing.iter().collect();
    for cue in cues.read() {
        if !starts(cue.start, !playing.is_empty()) {
            info!("Music is playing, so {} does not start", cue.track);
            continue;
        }
        // First, so an opening this machine lacks is silence rather than the last match's track.
        for entity in playing.drain(..) {
            commands.entity(entity).despawn();
        }
        let Some(track) = library.track(&cue.track) else {
            info!("No pack here holds {}, so no music plays", cue.track);
            continue;
        };
        let Some(path) = track.asset_path(&packs) else {
            match crate::common::resource::session::is_session_pack(&track.pack) {
                true => warn!("{} is not in the server's manifest, so it cannot be loaded", cue.track),
                false => warn!("{} is in {}, outside {}, where the AssetServer cannot reach it", cue.track, track.pack.display(), crate::common::assets::ASSET_ROOT),
            }
            continue;
        };
        let Some(asset_server) = &asset_server else { continue };
        info!("Starting {path}");
        let volume = Volume::Linear(music_gain(settings.as_deref(), *lan));
        let entity = commands
            .spawn((
                Name::new("Soundtrack"),
                Soundtrack,
                AudioPlayer::new(asset_server.load(path)),
                PlaybackSettings { mode: PlaybackMode::Despawn, volume, ..default() },
            ))
            .id();
        playing.push(entity);
    }
}

/// A track that will not decode would otherwise count as playing for the rest of the process.
fn drop_what_will_not_load(
    mut commands: Commands,
    asset_server: Option<Res<AssetServer>>,
    waiting: Query<(Entity, &AudioPlayer), (With<Soundtrack>, Without<AudioSink>)>,
) {
    let Some(asset_server) = asset_server else { return };
    for (entity, player) in &waiting {
        if asset_server.load_state(&player.0).is_failed() {
            warn!("A music track would not load: {:?}", player.0.path());
            commands.entity(entity).despawn();
        }
    }
}

/// So a silent speaker can be told from a track that never started.
fn say_what_is_heard(started: Query<(&AudioPlayer, &AudioSink), (With<Soundtrack>, Added<AudioSink>)>) {
    for (player, sink) in &started {
        let path = player.0.path().map_or_else(|| "a track".to_owned(), |path| path.to_string());
        info!("Playing {path} at {:.0}% volume", sink.volume().to_linear() * 100.0);
    }
}

/// `PlaybackSettings` are read once, so a playing track's volume changes only through its sink.
fn follow_the_volume(settings: Option<Res<PlayerSettings>>, lan: Res<OnLan>, mut sinks: Query<&mut AudioSink, With<Soundtrack>>) {
    let gain = music_gain(settings.as_deref(), *lan);
    for mut sink in &mut sinks {
        if (sink.volume().to_linear() - gain).abs() > 1e-4 {
            sink.set_volume(Volume::Linear(gain));
        }
    }
}

fn stop_the_music(mut commands: Commands, playing: Query<Entity, With<Soundtrack>>) {
    for entity in &playing {
        commands.entity(entity).despawn();
    }
}

/// A host never is, since it may be the machine on the room's speakers.
pub fn on_a_local_network(role: &NetRole, peers: impl IntoIterator<Item = SocketAddr>) -> bool {
    let mut peers = peers.into_iter();
    match role {
        NetRole::Solo => false,
        NetRole::Client { host, .. } => {
            host.trim_end_matches('.').to_ascii_lowercase().ends_with(".local") || peers.any(|peer| is_local(peer.ip()))
        }
        NetRole::Listen { .. } => false,
    }
}

fn notice_the_network(role: Res<NetRole>, server: Query<&PeerAddr, With<NetLink>>, mut lan: ResMut<OnLan>) {
    let now = OnLan(on_a_local_network(&role, server.iter().map(|peer| peer.0)));
    if *lan != now {
        info!("{}", if now.0 { "This game is on a local network" } else { "This game is not on a local network" });
        *lan = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn tracks(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Breaks if a later track can repeat the opening, or one repeats while another is unheard.
    #[test]
    fn a_later_track_is_never_the_opening_and_prefers_one_not_yet_heard() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let list = tracks(&["a", "b", "c"]);
        for _ in 0..50 {
            assert_eq!(choose(&list, &tracks(&["a", "b"]), &mut rng).as_deref(), Some("c"));
            let again = choose(&list, &tracks(&["a", "b", "c"]), &mut rng).unwrap();
            assert_ne!(again, "a", "the opening came round again");
        }
        assert_eq!(choose(&tracks(&["a"]), &tracks(&["a"]), &mut rng), None);
        assert_eq!(choose(&[], &[], &mut rng), None);
        assert!(choose(&list, &[], &mut rng).is_some());
    }

    #[derive(Resource, Default)]
    struct Heard(Vec<MusicCue>);

    fn hear(mut cues: MessageReader<MusicCue>, mut heard: ResMut<Heard>) {
        heard.0.extend(cues.read().cloned());
    }

    fn match_app(settings: RoundSettings) -> App {
        let mut app = App::new();
        app.add_plugins((bevy::time::TimePlugin, bevy::state::app::StatesPlugin))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)))
            .init_state::<AppMode>()
            .insert_resource(settings)
            .init_resource::<crate::common::game_rules::GameRules>()
            .init_resource::<crate::common::skeleton::AnimationClock>()
            .init_resource::<crate::game::player::InputLatch>()
            .init_resource::<crate::game::kill_feed::KillFeed>()
            .init_resource::<crate::game::rounds::RoundHistory>()
            .init_resource::<MapMetadata>()
            .add_plugins(crate::game::match_cycle::MatchCyclePlugin)
            .add_message::<MusicCue>()
            .init_resource::<Cued>()
            .init_resource::<Heard>()
            .add_systems(OnEnter(MatchPhase::Live), cue_the_opening)
            .add_systems(Update, ((cue_the_opening.run_if(in_state(MatchPhase::PreGame)), cue_the_closing_in).run_if(in_state(AppMode::Play)), hear).chain());
        app.world_mut().resource_mut::<MapMetadata>().music = tracks(&["music/a.ogg", "music/b.ogg"]);
        app.update();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds * 10.0).ceil() as usize {
            app.update();
        }
    }

    /// Breaks if bots alone hear music, or a person joining them gets other than one opening.
    #[test]
    fn a_match_is_opened_once_and_only_for_a_person_playing() {
        use crate::game::respawn::Seat;
        let settings = RoundSettings { pre_game_length: 6.0, fill_with_bots: true, bot_only_matches: true, frag_limit: None, ..default() };
        let mut app = match_app(settings);
        app.world_mut().spawn(crate::game::bot::BotSeat);
        app.world_mut().insert_resource(OurIdentity(Identity { id: PlayerId(1), seat: Seat::Spectating, color: 0, class: None }));
        run(&mut app, 8.0);
        assert_eq!(*app.world().resource::<State<MatchPhase>>().get(), MatchPhase::Live);
        assert!(app.world().resource::<Heard>().0.is_empty(), "bots and a spectator heard music");

        app.world_mut().resource_mut::<OurIdentity>().0.seat = Seat::Playing(None);
        run(&mut app, 8.0);
        assert_eq!(*app.world().resource::<State<MatchPhase>>().get(), MatchPhase::Live);
        let heard = &app.world().resource::<Heard>().0;
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert_eq!(heard[0].start, CueStart::Now);
    }

    /// Breaks if a pre-game shorter than the mark goes unopened.
    #[test]
    fn a_pre_game_of_no_length_is_still_opened() {
        let mut app = match_app(RoundSettings { pre_game_length: 0.0, ..default() });
        app.world_mut().insert_resource(OurIdentity(Identity { id: PlayerId(1), seat: crate::game::respawn::Seat::Playing(None), color: 0, class: None }));
        run(&mut app, 1.0);
        assert_eq!(*app.world().resource::<State<MatchPhase>>().get(), MatchPhase::Live);
        assert_eq!(app.world().resource::<Heard>().0.len(), 1);
    }

    /// Breaks if tracks added during the pre-game do not open that match.
    #[test]
    fn tracks_added_late_in_the_pre_game_still_open_it() {
        let mut app = match_app(RoundSettings { pre_game_length: 6.0, ..default() });
        app.world_mut().resource_mut::<MapMetadata>().music.clear();
        app.world_mut().insert_resource(OurIdentity(Identity { id: PlayerId(1), seat: crate::game::respawn::Seat::Playing(None), color: 0, class: None }));
        run(&mut app, 2.0);
        assert!(app.world().resource::<Heard>().0.is_empty());
        app.world_mut().resource_mut::<MapMetadata>().music = tracks(&["music/a.ogg"]);
        run(&mut app, 0.5);
        assert_eq!(app.world().resource::<Heard>().0.len(), 1);
    }

    #[test]
    fn within_one_of_the_limit_takes_a_frag() {
        assert!(closing_in(19, 20) && closing_in(20, 20));
        assert!(!closing_in(18, 20));
        assert!(!closing_in(0, 1) && closing_in(1, 1));
    }

    #[test]
    fn the_opening_starts_over_anything_and_a_later_one_only_in_quiet() {
        assert!(starts(CueStart::Now, true) && starts(CueStart::Now, false));
        assert!(starts(CueStart::IfQuiet, false) && !starts(CueStart::IfQuiet, true));
    }

    #[test]
    fn who_is_on_a_local_network() {
        let lan: SocketAddr = "192.168.1.4:27100".parse().unwrap();
        let wan: SocketAddr = "8.8.8.8:27100".parse().unwrap();
        let client = |host: &str| NetRole::Client { host: host.into(), port: 27100 };
        assert!(!on_a_local_network(&NetRole::Solo, [lan]));
        assert!(on_a_local_network(&client("192.168.1.4"), [lan]));
        assert!(!on_a_local_network(&client("example.com"), [wan]));
        assert!(on_a_local_network(&client("Grackle-Box.local."), [wan]));
        assert!(!on_a_local_network(&NetRole::Listen { port: 27100 }, [wan, lan]), "the host was muted");
    }

    #[test]
    fn a_cue_survives_the_wire_encoding() {
        let sent = MusicCue { track: "music/Pump/Pump.mp3".into(), start: CueStart::IfQuiet };
        let bytes = postcard::to_allocvec(&sent).unwrap();
        assert_eq!(postcard::from_bytes::<MusicCue>(&bytes).unwrap(), sent);
    }

    #[test]
    fn muting_on_a_local_network_silences_only_there() {
        let mut settings = PlayerSettings::default();
        assert!(settings.sound.mute_music_on_lan, "not muted on a local network by default");
        assert_eq!(music_gain(Some(&settings), OnLan(true)), 0.0);
        assert!(music_gain(Some(&settings), OnLan(false)) > 0.0);
        settings.sound.mute_music_on_lan = false;
        assert!(music_gain(Some(&settings), OnLan(true)) > 0.0);
    }
}
