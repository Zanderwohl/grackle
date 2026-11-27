//! The in-game chat: what is said, the round's announcements, and the log every machine keeps of
//! them. The authority decides every line: a client sends what it typed as a [`SayChat`], and the
//! server writes it into the log as a [`ChatEntry`] with the speaker's name and colors, beside its
//! own announcements, and relays each entry to the clients. The window that shows it is
//! [`crate::game::chat_window`].

use std::collections::HashMap;

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::match_phase::{MatchNumber, MatchPhase};
use crate::common::net::{has_authority, is_remote_client, NetRole};
use crate::common::player_color::PlayerColor;
use crate::common::player_settings::{ClientSettings, PlayerSettings};
use crate::common::round_settings::RoundSettings;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::game::respawn::{HueStart, Identity, OurIdentity, Seat};
use crate::game::rounds::RoundHistory;
use crate::game::score::Standing;
use crate::get;

/// Seconds between two lines from one speaker; a line sooner is dropped.
const SECONDS_BETWEEN_LINES: f32 = 1.0;

/// Characters; the rest of a longer line is cut off by the server.
pub const CHAT_LIMIT: usize = 200;

const LOG_LIMIT: usize = 500;

/// Seconds left at which a timed round is announced.
const TIME_WARNINGS: [u32; 2] = [300, 60];
/// Frags the leader is short of the limit at which it is announced.
const FRAG_WARNINGS: [u32; 2] = [2, 1];

/// Who hears a line. Matched without a catch-all arm, so a new audience must be answered wherever
/// it is read.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audience {
    #[default]
    All,
}

/// What this machine's player typed. The authority answers its own; a client's goes over the
/// wire.
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SayChat {
    pub audience: Audience,
    pub text: String,
}

/// As the server knew them, so a line keeps its name and color after its speaker leaves.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Speaker {
    pub id: PlayerId,
    /// Empty is unnamed, which each machine words in its own language.
    pub name: String,
    pub team: Option<Team>,
    pub color: Option<PlayerColor>,
}

impl Speaker {
    pub fn display_name(&self) -> String {
        match self.name.is_empty() {
            true => get!("scoreboard.player", "id", self.id.0),
            false => self.name.clone(),
        }
    }

    pub fn swatch(&self) -> Swatch {
        Swatch::of_body(self.team, self.color)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RoundEvent {
    Started,
    TimeLeft { seconds: u32 },
    /// How far the leader is from the frag limit.
    FragsLeft { frags: u32 },
    Streak { who: Speaker, streak: Streak },
    /// Two or more kills within [`crate::game::score::MULTI_KILL_SECONDS`].
    MultiKill { who: Speaker, multi: MultiKill },
    /// Several sides is a tie.
    Won { sides: Vec<Winner> },
    NextRoundIn { seconds: u32 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Winner {
    Team(Team),
    Player(Speaker),
}

impl Winner {
    pub fn name(&self) -> String {
        match self {
            Winner::Team(team) => team.name(),
            Winner::Player(who) => who.display_name(),
        }
    }

    pub fn swatch(&self) -> Swatch {
        match self {
            Winner::Team(team) => Swatch::team(*team),
            Winner::Player(who) => who.swatch(),
        }
    }

    pub fn words(sides: &[Winner]) -> Option<String> {
        match sides {
            [] => None,
            [side] => Some(get!("match.wins", "winner", side.name())),
            several => {
                let names: Vec<String> = several.iter().map(Winner::name).collect();
                Some(get!("match.tie", "winners", names.join(&get!("match.and"))))
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streak {
    HatTrick,
    Spree,
    Dominating,
    Unstoppable,
}

impl Streak {
    pub fn at(kills: u32) -> Option<Self> {
        match kills {
            3 => Some(Streak::HatTrick),
            5 => Some(Streak::Spree),
            10 => Some(Streak::Dominating),
            15 => Some(Streak::Unstoppable),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiKill {
    Double,
    /// Each kill past the second is announced again.
    Multi,
}

impl MultiKill {
    /// `kills` within the window, this one included.
    pub fn of(kills: u32) -> Option<Self> {
        match kills {
            0 | 1 => None,
            2 => Some(MultiKill::Double),
            _ => Some(MultiKill::Multi),
        }
    }
}

impl RoundEvent {
    pub fn words(&self) -> String {
        match self {
            RoundEvent::Started => get!("chat.round_started"),
            &RoundEvent::TimeLeft { seconds } if seconds % 60 == 0 => match seconds / 60 {
                1 => get!("chat.one_minute_left"),
                minutes => get!("chat.minutes_left", "minutes", minutes),
            },
            &RoundEvent::TimeLeft { seconds } => match seconds {
                1 => get!("chat.one_second_left"),
                seconds => get!("chat.seconds_left", "seconds", seconds),
            },
            &RoundEvent::FragsLeft { frags } => match frags {
                1 => get!("chat.one_frag_left"),
                frags => get!("chat.frags_left", "frags", frags),
            },
            RoundEvent::Streak { who, streak } => {
                let name = who.display_name();
                match streak {
                    Streak::HatTrick => get!("chat.hat_trick", "name", name),
                    Streak::Spree => get!("chat.spree", "name", name),
                    Streak::Dominating => get!("chat.dominating", "name", name),
                    Streak::Unstoppable => get!("chat.unstoppable", "name", name),
                }
            }
            RoundEvent::Won { sides } => Winner::words(sides).unwrap_or_default(),
            &RoundEvent::NextRoundIn { seconds } => match seconds {
                1 => get!("chat.next_round_in_a_second"),
                seconds => get!("chat.next_round_in", "seconds", seconds),
            },
            RoundEvent::MultiKill { who, multi } => {
                let name = who.display_name();
                match multi {
                    MultiKill::Double => get!("chat.double_kill", "name", name),
                    MultiKill::Multi => get!("chat.multi_kill", "name", name),
                }
            }
        }
    }
}

#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ChatEntry {
    Said { from: Speaker, audience: Audience, text: String },
    Round(RoundEvent),
    /// Empties the log at a pre-game. Sent with the chat rather than read off the phase, which
    /// comes on another channel and could arrive before or after lines sent around it.
    Clear,
}

/// `at` is on [`Time<Real>`], which the closed log fades by.
#[derive(Clone, Debug, PartialEq)]
pub struct LoggedLine {
    pub entry: ChatEntry,
    pub at: f32,
}

/// Oldest first. Emptied by [`ChatEntry::Clear`] and on leaving Play.
#[derive(Resource, Default, Debug)]
pub struct ChatLog {
    pub lines: Vec<LoggedLine>,
}

impl ChatLog {
    fn push(&mut self, entry: ChatEntry, at: f32) {
        self.lines.push(LoggedLine { entry, at });
        if self.lines.len() > LOG_LIMIT {
            let over = self.lines.len() - LOG_LIMIT;
            self.lines.drain(..over);
        }
    }
}

/// Without control characters, which would break the log over several lines, trimmed and cut to
/// [`CHAT_LIMIT`]. `None` if nothing is left.
pub fn cleaned(text: &str) -> Option<String> {
    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
    let kept: String = printable.trim().chars().take(CHAT_LIMIT).collect();
    let kept = kept.trim_end().to_owned();
    (!kept.is_empty()).then_some(kept)
}

/// The warning to give, if any: the smallest threshold `left` has reached that `limit` was above,
/// so a round that starts inside a threshold is not warned of it. Each threshold is given once, and
/// a jump past several gives only the last.
fn warning(thresholds: &[u32], left: f32, limit: f32, given: &mut Vec<u32>) -> Option<u32> {
    let reached = thresholds.iter().copied().filter(|&at| left <= at as f32 && limit > at as f32).min()?;
    if given.contains(&reached) {
        return None;
    }
    given.extend(thresholds.iter().copied().filter(|&at| at >= reached));
    Some(reached)
}

#[derive(Resource, Default, Debug)]
struct Warned {
    number: u32,
    time: Vec<u32>,
    frags: Vec<u32>,
}

impl Warned {
    fn for_match(&mut self, number: u32) -> &mut Self {
        if self.number != number {
            *self = Self { number, ..default() };
        }
        self
    }
}

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SayChat>()
            .add_message::<ChatEntry>()
            .init_resource::<ChatLog>()
            .init_resource::<Warned>()
            .init_resource::<LastSaid>()
            .add_systems(
                Update,
                (
                    // Ungated by Play, so lines sent outside it are drained rather than heard next round.
                    hear_what_was_said,
                    (warn_of_the_time, warn_of_the_frags).run_if(in_state(AppMode::Play)).run_if(in_state(MatchPhase::Live)),
                )
                    .run_if(has_authority),
            )
            .add_systems(OnEnter(MatchPhase::Live), announce_the_start.run_if(has_authority))
            .add_systems(OnEnter(MatchPhase::PreGame), start_the_log_over.run_if(has_authority))
            .add_systems(Update, log_the_chat.after(hear_what_was_said))
            .add_systems(OnExit(AppMode::Play), forget_the_chat);
    }
}

fn forget_the_chat(mut log: ResMut<ChatLog>) {
    log.lines.clear();
}

fn start_the_log_over(mut entries: MessageWriter<ChatEntry>) {
    entries.write(ChatEntry::Clear);
}

fn log_the_chat(mut entries: MessageReader<ChatEntry>, time: Res<Time<Real>>, mut log: ResMut<ChatLog>) {
    for entry in entries.read() {
        match entry {
            ChatEntry::Clear => log.lines.clear(),
            ChatEntry::Said { .. } | ChatEntry::Round(_) => log.push(entry.clone(), time.elapsed_secs()),
        }
    }
}

/// Their standing's name and colors while they play, else their own name and their identity's
/// color. A spectator or somebody still choosing is unassigned, so has no color.
fn speaker(who: &Identity, settings_name: &str, standing: Option<&Standing>, start: Option<&HueStart>) -> Speaker {
    if matches!(who.seat, Seat::Spectating | Seat::Choosing) {
        return Speaker { id: who.id, name: settings_name.to_owned(), team: None, color: None };
    }
    match standing {
        Some(standing) => Speaker { id: who.id, name: standing.name.clone(), team: standing.team, color: Some(standing.color) },
        None => Speaker { id: who.id, name: settings_name.to_owned(), team: who.team(), color: start.map(|start| who.color(start)) },
    }
}

/// When each speaker's last line was heard, on [`Time<Real>`].
#[derive(Resource, Default, Debug)]
struct LastSaid(HashMap<PlayerId, f32>);

impl LastSaid {
    /// Whether `id` may speak at `now`, noting it if so.
    fn may_speak(&mut self, id: PlayerId, now: f32) -> bool {
        if self.0.get(&id).is_some_and(|&last| now - last < SECONDS_BETWEEN_LINES) {
            return false;
        }
        self.0.insert(id, now);
        true
    }
}

/// Lines outside a round, or from somebody with no identity or speaking too soon, are dropped.
#[allow(clippy::too_many_arguments)]
fn hear_what_was_said(
    mode: Res<State<AppMode>>,
    time: Res<Time<Real>>,
    mut last: ResMut<LastSaid>,
    mut ours: MessageReader<SayChat>,
    mut receivers: Query<(Entity, &mut MessageReceiver<SayChat>), With<ClientOf>>,
    us: Option<Res<OurIdentity>>,
    our_settings: Option<Res<PlayerSettings>>,
    links: Query<(&Identity, Option<&ClientSettings>), With<ClientOf>>,
    standings: Query<&Standing>,
    start: Option<Res<HueStart>>,
    mut entries: MessageWriter<ChatEntry>,
) {
    let mut said: Vec<(Option<Entity>, SayChat)> = ours.read().map(|say| (None, say.clone())).collect();
    for (link, mut receiver) in &mut receivers {
        said.extend(receiver.receive().map(|say| (Some(link), say)));
    }
    if *mode.get() != AppMode::Play {
        return;
    }
    for (link, say) in said {
        let Some(text) = cleaned(&say.text) else { continue };
        let (who, name) = match link {
            None => match us.as_deref() {
                Some(us) => (us.0, our_settings.as_deref().map(|settings| settings.name.clone()).unwrap_or_default()),
                None => continue,
            },
            Some(link) => match links.get(link) {
                Ok((who, settings)) => (*who, settings.map(|settings| settings.0.name.clone()).unwrap_or_default()),
                Err(_) => continue,
            },
        };
        if !last.may_speak(who.id, time.elapsed_secs()) {
            debug!("{} spoke again within {SECONDS_BETWEEN_LINES} s; dropped", who.id);
            continue;
        }
        let standing = standings.iter().find(|standing| standing.id == who.id);
        let from = speaker(&who, &name, standing, start.as_deref());
        info!("{} says to {:?}: {text}", from.display_name(), say.audience);
        entries.write(ChatEntry::Said { from, audience: say.audience, text });
    }
}

fn announce_the_start(mut entries: MessageWriter<ChatEntry>) {
    entries.write(ChatEntry::Round(RoundEvent::Started));
}

fn warn_of_the_time(
    settings: Option<Res<RoundSettings>>,
    history: Option<Res<RoundHistory>>,
    time: Res<Time<Real>>,
    number: Res<MatchNumber>,
    mut warned: ResMut<Warned>,
    mut entries: MessageWriter<ChatEntry>,
) {
    let Some(limit) = settings.and_then(|settings| settings.time_limit) else { return };
    let Some(elapsed) = history.and_then(|history| history.elapsed(time.elapsed_secs())) else { return };
    let warned = warned.for_match(number.0);
    if let Some(seconds) = warning(&TIME_WARNINGS, limit - elapsed, limit, &mut warned.time) {
        entries.write(ChatEntry::Round(RoundEvent::TimeLeft { seconds }));
    }
}

fn warn_of_the_frags(
    settings: Option<Res<RoundSettings>>,
    standings: Query<&Standing>,
    number: Res<MatchNumber>,
    mut warned: ResMut<Warned>,
    mut entries: MessageWriter<ChatEntry>,
) {
    let Some(limit) = settings.and_then(|settings| settings.frag_limit) else { return };
    let Some(best) = standings.iter().map(|standing| standing.frags).max() else { return };
    let left = (limit as i32 - best).max(0) as f32;
    if left == 0.0 {
        return;
    }
    let warned = warned.for_match(number.0);
    if let Some(frags) = warning(&FRAG_WARNINGS, left, limit as f32, &mut warned.frags) {
        entries.write(ChatEntry::Round(RoundEvent::FragsLeft { frags }));
    }
}

/// Reliable and ordered, so a conversation reads in the order it was said.
pub struct ChatChannel;

/// The wire half, beside the transport's other plugins.
pub struct ChatSyncPlugin;

impl Plugin for ChatSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SayChat>().add_message::<ChatEntry>();
        app.add_channel::<ChatChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<SayChat>().add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ChatEntry>().add_direction(NetworkDirection::ServerToClient);
        app.add_systems(Update, (send_what_we_said, relay_the_chat.run_if(has_authority)));
        app.add_systems(PreUpdate, receive_the_chat.after(MessageSystems::Receive).run_if(is_remote_client));
    }
}

fn send_what_we_said(
    role: Res<NetRole>,
    mut ours: MessageReader<SayChat>,
    mut clients: Query<&mut MessageSender<SayChat>, (With<Client>, With<Connected>)>,
) {
    let said: Vec<SayChat> = ours.read().cloned().collect();
    if role.is_authority() {
        return;
    }
    for mut sender in &mut clients {
        for say in &said {
            sender.send::<ChatChannel>(say.clone());
        }
    }
}

fn relay_the_chat(
    mut entries: MessageReader<ChatEntry>,
    server: Option<Single<&Endpoint, With<Server>>>,
    mut sender: crate::common::net_transport::ToClients,
) -> Result {
    let Some(server) = server else {
        entries.clear();
        return Ok(());
    };
    for entry in entries.read() {
        let target = match entry {
            ChatEntry::Said { audience: Audience::All, .. } => NetworkTarget::All,
            ChatEntry::Round(_) | ChatEntry::Clear => NetworkTarget::All,
        };
        sender.send::<_, ChatChannel>(entry, &server, &target)?;
    }
    Ok(())
}

fn receive_the_chat(mut receivers: Query<&mut MessageReceiver<ChatEntry>>, mut entries: MessageWriter<ChatEntry>) {
    for mut receiver in &mut receivers {
        for entry in receiver.receive() {
            entries.write(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_kept_printable_trimmed_and_short() {
        assert_eq!(cleaned("  hi\nthere \t"), Some("hithere".to_owned()));
        assert_eq!(cleaned(" \u{7} "), None);
        assert_eq!(cleaned(&"a".repeat(CHAT_LIMIT + 50)).map(|text| text.chars().count()), Some(CHAT_LIMIT));
    }

    /// Fails if a warning repeats, a jump past two thresholds gives both, or a short round is
    /// warned of a threshold it started inside.
    #[test]
    fn each_warning_is_given_once_and_only_after_the_limit_was_above_it() {
        let mut given = Vec::new();
        assert_eq!(warning(&TIME_WARNINGS, 600.0, 600.0, &mut given), None);
        assert_eq!(warning(&TIME_WARNINGS, 300.0, 600.0, &mut given), Some(300));
        assert_eq!(warning(&TIME_WARNINGS, 299.0, 600.0, &mut given), None);
        assert_eq!(warning(&TIME_WARNINGS, 59.5, 600.0, &mut given), Some(60));
        assert_eq!(warning(&TIME_WARNINGS, 10.0, 600.0, &mut given), None);

        let mut given = Vec::new();
        assert_eq!(warning(&FRAG_WARNINGS, 1.0, 10.0, &mut given), Some(1), "a jump from three to one");
        assert_eq!(warning(&FRAG_WARNINGS, 2.0, 10.0, &mut given), None, "two after one");

        let mut given = Vec::new();
        assert_eq!(warning(&TIME_WARNINGS, 120.0, 120.0, &mut given), None, "five minutes in a two-minute round");
        assert_eq!(warning(&TIME_WARNINGS, 60.0, 120.0, &mut given), Some(60));
    }

    #[test]
    fn round_events_read_in_their_units() {
        assert_eq!(RoundEvent::TimeLeft { seconds: 300 }.words(), get!("chat.minutes_left", "minutes", 5));
        assert_eq!(RoundEvent::TimeLeft { seconds: 60 }.words(), get!("chat.one_minute_left"));
        assert_eq!(RoundEvent::TimeLeft { seconds: 30 }.words(), get!("chat.seconds_left", "seconds", 30));
        assert_eq!(RoundEvent::FragsLeft { frags: 2 }.words(), get!("chat.frags_left", "frags", 2));
        assert_eq!(RoundEvent::FragsLeft { frags: 1 }.words(), get!("chat.one_frag_left"));
        assert_eq!(RoundEvent::NextRoundIn { seconds: 1 }.words(), get!("chat.next_round_in_a_second"));
        assert_eq!(RoundEvent::NextRoundIn { seconds: 15 }.words(), get!("chat.next_round_in", "seconds", 15));
        let tie = RoundEvent::Won { sides: vec![Winner::Team(Team::Red), Winner::Team(Team::Blue)] };
        let names = format!("{}{}{}", Team::Red.name(), get!("match.and"), Team::Blue.name());
        assert_eq!(tie.words(), get!("match.tie", "winners", names));
    }

    /// Fails if an entry stops surviving postcard, which Lightyear sends it with.
    #[test]
    fn an_entry_survives_the_wire_encoding() {
        let said = ChatEntry::Said {
            from: Speaker { id: PlayerId(7), name: "Ana".into(), team: Some(Team::Blue), color: Some(PlayerColor { hue: 40.0 }) },
            audience: Audience::All,
            text: "gg".into(),
        };
        let ana = Speaker { id: PlayerId(7), name: "Ana".into(), team: None, color: Some(PlayerColor { hue: 40.0 }) };
        let streak = ChatEntry::Round(RoundEvent::Streak { who: ana.clone(), streak: Streak::Spree });
        let multi = ChatEntry::Round(RoundEvent::MultiKill { who: ana, multi: MultiKill::Double });
        for entry in [said, ChatEntry::Round(RoundEvent::FragsLeft { frags: 2 }), streak, multi, ChatEntry::Clear] {
            let bytes = postcard::to_allocvec(&entry).unwrap();
            assert_eq!(postcard::from_bytes::<ChatEntry>(&bytes).unwrap(), entry);
        }
        let say = SayChat { audience: Audience::All, text: "hello".into() };
        assert_eq!(postcard::from_bytes::<SayChat>(&postcard::to_allocvec(&say).unwrap()).unwrap(), say);
    }

    /// Breaks if a line from somebody on no team yet is painted in a color they would play in.
    #[test]
    fn a_spectator_or_chooser_speaks_in_the_unassigned_cream() {
        let start = HueStart(0.25);
        let watcher = Identity { id: PlayerId(2), seat: Seat::Spectating, color: 1, class: None };
        let from = speaker(&watcher, "Ana", None, Some(&start));
        assert_eq!((from.team, from.color), (None, None));
        assert_eq!(from.swatch(), Swatch::neutral());
        let chooser = Identity { seat: Seat::Choosing, ..watcher };
        assert_eq!(speaker(&chooser, "Ana", None, Some(&start)).swatch(), Swatch::neutral());
        let player = Identity { seat: Seat::Playing(None), ..watcher };
        assert!(speaker(&player, "Ana", None, Some(&start)).color.is_some(), "a free-for-all player lost their color");
    }

    #[test]
    fn a_speaker_says_at_most_one_line_a_second() {
        let mut last = LastSaid::default();
        let (ana, bo) = (PlayerId(1), PlayerId(2));
        assert!(last.may_speak(ana, 10.0));
        assert!(!last.may_speak(ana, 10.5));
        assert!(last.may_speak(bo, 10.5), "one speaker held back another");
        assert!(last.may_speak(ana, 11.0));
    }

    #[test]
    fn the_log_drops_its_oldest_past_the_limit() {
        let mut log = ChatLog::default();
        for n in 0..LOG_LIMIT + 3 {
            log.push(ChatEntry::Round(RoundEvent::FragsLeft { frags: n as u32 }), 0.0);
        }
        assert_eq!(log.lines.len(), LOG_LIMIT);
        assert_eq!(log.lines[0].entry, ChatEntry::Round(RoundEvent::FragsLeft { frags: 3 }));
    }
}
