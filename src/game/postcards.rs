//! The map's postcards on screen while the match is not being fought: in the pre-game, between
//! rounds, and while the team picker is up. Cards no longer wanted fly out off the bottom. Each is the picture on a 3:2 card of off-white,
//! grained paper with an inked outline, the picture inset by an eighth of the card on every side
//! ([`crate::common::postcard_print::postcard`]), and up to three are laid in whatever space the rest of the UI
//! leaves, tilted ([`lay_out`]). What counts as taken is every visible node marked [`KeptClear`].
//!
//! The cards are [`PrintedPostcards`]: printed by the authority, and sent to a client as it joins.
//! A set is replaced whole, so the cards fly in together.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::match_phase::{phase_of, MatchPhase};
use crate::common::postcard_sync::PrintedPostcards;
use crate::game::hud::WINDOW_MARGIN;
use crate::game::pause_menu::MouseHeld;

/// A UI node postcards are laid out around.
#[derive(Component, Default)]
pub struct KeptClear;

/// Under the team and class pickers, over the HUD.
const LAYER: i32 = 9;

/// Logical pixels between a card and anything it is kept clear of, another card included.
const GAP: f32 = 12.0;
/// Shares of the window's height a card's height stays between.
const TALLEST: f32 = 0.34;
const SHORTEST: f32 = 0.12;
/// Candidate centers per window height; the same count at any resolution.
const STEPS: f32 = 48.0;
/// Halvings of the range of widths tried.
const SIZE_SEARCH: u32 = 10;

/// Real seconds the window must keep one size before the cards are laid out for it.
const SETTLE_SECONDS: f32 = 0.3;
/// Seconds of the cards' clock a card takes to fly in from below the screen.
const FLY_IN_SECONDS: f32 = 0.8;
/// Seconds of the cards' clock a card takes to move to its place in a new layout.
const MOVE_SECONDS: f32 = 0.5;
/// Seconds of the cards' clock a card takes to fly out off the bottom of the screen.
const FLY_OUT_SECONDS: f32 = 0.5;
/// How much later each card after the first sets off, flying in, moving or flying out.
const STAGGER: f32 = 0.25;
/// The most seconds the cards' clock moves in one frame, so a hitch slows a flight rather than
/// skipping it.
const MOST_STEP: f32 = 1.0 / 20.0;

/// Where a card would like to be, as shares of the window, its tilt, and which half of the window
/// holds it. Tried in order, so one postcard goes top left and two are either side.
struct Zone {
    anchor: Vec2,
    /// Which way its cards tilt: 1 clockwise, -1 counterclockwise.
    turn: f32,
    left: bool,
}

const ZONES: [Zone; 3] = [
    Zone { anchor: Vec2::new(0.17, 0.42), turn: 1.0, left: true },
    Zone { anchor: Vec2::new(0.83, 0.66), turn: -1.0, left: false },
    Zone { anchor: Vec2::new(0.17, 0.80), turn: -1.0, left: true },
];

/// Degrees a card tilts at [`TALLEST`] and at [`SHORTEST`], and the most further it leans at the
/// bottom of the window (and the least at the top).
const LEAST_TILT: f32 = 4.0;
const MOST_TILT: f32 = 20.0;
const LEAN: f32 = 8.0;
/// Degrees no card tilts past.
const STEEPEST: f32 = 30.0;

/// Radians a card of `width` centered at `center` tilts in a zone turning `turn`: more the smaller
/// it is and the lower it stands.
fn tilt(turn: f32, center: Vec2, width: f32, window: Vec2) -> f32 {
    let height_share = width / 1.5 / window.y;
    let smallness = 1.0 - ((height_share - SHORTEST) / (TALLEST - SHORTEST)).clamp(0.0, 1.0);
    let lowness = (center.y / window.y).clamp(0.0, 1.0) * 2.0 - 1.0;
    let degrees = LEAST_TILT + (MOST_TILT - LEAST_TILT) * smallness + LEAN * lowness;
    (turn * degrees).clamp(-STEEPEST, STEEPEST).to_radians()
}

/// A card laid out, in logical pixels from the window's top left; its height is two thirds of its
/// width. `angle` is radians, positive clockwise on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    pub center: Vec2,
    pub width: f32,
    pub angle: f32,
}

impl Card {
    fn half(&self) -> Vec2 {
        Vec2::new(self.width / 2.0, self.width / 3.0)
    }

    fn axes(&self) -> [Vec2; 2] {
        let (sin, cos) = self.angle.sin_cos();
        [Vec2::new(cos, sin), Vec2::new(-sin, cos)]
    }

    /// Grown by `grow` on every side.
    fn corners(&self, grow: f32) -> [Vec2; 4] {
        let [x, y] = self.axes();
        let half = self.half() + grow;
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(sx, sy)| self.center + x * half.x * sx + y * half.y * sy)
    }

    /// Whether the card, grown by `gap` on every side, touches `rect`. Separating axes, so a
    /// tilted card's empty corners are not counted.
    fn touches(&self, rect: Rect, gap: f32) -> bool {
        let corners = self.corners(gap);
        let rect_corners = [rect.min, Vec2::new(rect.max.x, rect.min.y), rect.max, Vec2::new(rect.min.x, rect.max.y)];
        let [x, y] = self.axes();
        [Vec2::X, Vec2::Y, x, y].into_iter().all(|axis| {
            let (a_min, a_max) = extent(&corners, axis);
            let (b_min, b_max) = extent(&rect_corners, axis);
            a_max > b_min && b_max > a_min
        })
    }

    /// Whether the card touches `other`, each grown by half of `gap`.
    fn touches_card(&self, other: &Card, gap: f32) -> bool {
        let (a_corners, b_corners) = (self.corners(gap / 2.0), other.corners(gap / 2.0));
        self.axes().into_iter().chain(other.axes()).all(|axis| {
            let (a_min, a_max) = extent(&a_corners, axis);
            let (b_min, b_max) = extent(&b_corners, axis);
            a_max > b_min && b_max > a_min
        })
    }

    fn inside(&self, bounds: Rect) -> bool {
        self.corners(0.0).iter().all(|corner| bounds.contains(*corner))
    }
}

impl Card {
    /// Where `self` was `ease` of the way from `from`.
    fn between(from: &Card, to: &Card, ease: f32) -> Card {
        Card {
            center: from.center.lerp(to.center, ease),
            width: from.width + (to.width - from.width) * ease,
            angle: from.angle + (to.angle - from.angle) * ease,
        }
    }

    /// The same card, straight below the bottom of `window` with room to spare for any tilt.
    fn below(&self, window: Vec2) -> Card {
        Card { center: Vec2::new(self.center.x, window.y + self.width * 0.65), ..*self }
    }
}

pub fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(4)
}

pub fn ease_in_quart(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(4)
}

pub fn ease_in_out_quart(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match t < 0.5 {
        true => 8.0 * t.powi(4),
        false => 1.0 - (2.0 - 2.0 * t).powi(4) / 2.0,
    }
}

/// A card's move from one place to another over real time.
#[derive(Clone, Copy, Debug)]
struct Flight {
    from: Card,
    to: Card,
    /// May be in the future, for a card waiting its turn to fly in.
    started: f32,
    seconds: f32,
    ease: fn(f32) -> f32,
}

impl Flight {
    fn at(&self, now: f32) -> Card {
        Card::between(&self.from, &self.to, (self.ease)((now - self.started) / self.seconds))
    }
}

/// Whether the cards are laid out again this frame. A new window size always is, once the window
/// has held it for [`SETTLE_SECONDS`], so cards a small window had no room for come back. With the
/// window still, a layout is redone when a card stops fitting, or when what is kept clear has
/// changed while a card is still waiting for room.
fn lays_out_again(laid_out_for: Vec2, window: Vec2, settled: bool, fits: bool, waiting: bool, taken_changed: bool) -> bool {
    settled && (laid_out_for != window || !fits || (waiting && taken_changed))
}

fn extent(points: &[Vec2; 4], axis: Vec2) -> (f32, f32) {
    points.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), point| {
        let along = point.dot(axis);
        (lo.min(along), hi.max(along))
    })
}

fn fits(card: &Card, bounds: Rect, taken: &[Rect], placed: &[Card]) -> bool {
    card.inside(bounds)
        && !taken.iter().any(|rect| card.touches(*rect, GAP))
        && !placed.iter().any(|other| card.touches_card(other, GAP))
}

/// Up to `count` cards in `window` clear of `taken`, as large as fit up to [`TALLEST`], each as
/// near its zone's anchor as its size allows. A zone with no room for a card [`SHORTEST`] high is
/// passed over, so fewer may come back.
pub fn lay_out(window: Vec2, taken: &[Rect], count: usize) -> Vec<Card> {
    let bounds = Rect::from_corners(Vec2::splat(WINDOW_MARGIN), window - WINDOW_MARGIN);
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return Vec::new();
    }
    let step = (window.y / STEPS).max(1.0);
    let mut placed: Vec<Card> = Vec::new();
    for zone in &ZONES {
        if placed.len() == count {
            break;
        }
        let half = window.x / 2.0;
        let (low, high) = if zone.left { (bounds.min.x, half) } else { (half, bounds.max.x) };
        let anchor = zone.anchor * window;
        let spot = |width: f32| -> Option<Card> {
            let mut best: Option<(f32, Card)> = None;
            let mut y = bounds.min.y;
            while y <= bounds.max.y {
                let mut x = low;
                while x <= high {
                    let center = Vec2::new(x, y);
                    let card = Card { center, width, angle: tilt(zone.turn, center, width, window) };
                    let distance = card.center.distance_squared(anchor);
                    if best.is_none_or(|(nearest, _)| distance < nearest) && fits(&card, bounds, taken, &placed) {
                        best = Some((distance, card));
                    }
                    x += step;
                }
                y += step;
            }
            best.map(|(_, card)| card)
        };
        let (mut small, mut large) = (SHORTEST * window.y * 1.5, TALLEST * window.y * 1.5);
        let Some(mut found) = spot(small) else { continue };
        if let Some(card) = spot(large) {
            found = card;
        } else {
            for _ in 0..SIZE_SEARCH {
                let middle = (small + large) / 2.0;
                match spot(middle) {
                    Some(card) => {
                        found = card;
                        small = middle;
                    }
                    None => large = middle,
                }
            }
        }
        placed.push(found);
    }
    placed
}

/// Whether `cards` still fit among `taken`, so a layout is kept while it does rather than jumping
/// as a kill feed line comes and goes.
fn still_fits(window: Vec2, cards: &[Card], taken: &[Rect]) -> bool {
    let bounds = Rect::from_corners(Vec2::splat(WINDOW_MARGIN), window - WINDOW_MARGIN);
    cards.iter().enumerate().all(|(index, card)| fits(card, bounds, taken, &cards[..index]))
}

/// `leaving_until` is set once its cards are flying out, to when the last is off the screen.
#[derive(Component, Default)]
struct PostcardRoot {
    leaving_until: Option<f32>,
}

#[derive(Component)]
struct PostcardCard {
    index: usize,
    flight: Flight,
    leaving: bool,
}

/// What the cards were last laid out for.
#[derive(Default)]
struct Layout {
    pictures: Vec<Handle<Image>>,
    window: Vec2,
    /// The cards placed, in picture order; a picture past the end has no room.
    cards: Vec<Card>,
    taken: Vec<Rect>,
    /// The window's size last frame, and since when it has been that.
    seen_window: Vec2,
    window_since: f32,
    /// Seconds the cards' flights are timed by: real time, but never more than [`MOST_STEP`] a frame.
    clock: f32,
}

pub struct PostcardHudPlugin;

impl Plugin for PostcardHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MouseHeld>()
            .add_systems(Update, show_the_postcards.run_if(in_state(AppMode::Play)))
            .add_systems(OnExit(AppMode::Play), put_the_postcards_away);
    }
}

/// Whether the match is between fights or somebody is choosing, which is when postcards show.
fn wanted(phase: Option<MatchPhase>, held: &MouseHeld) -> bool {
    held.team_picker || matches!(phase, Some(MatchPhase::PreGame | MatchPhase::BetweenRounds))
}

/// A node's on-screen box in logical pixels, its `UiTransform` included.
fn screen_rect(computed: &ComputedNode, transform: &UiGlobalTransform) -> Option<Rect> {
    let size = computed.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    let half = size / 2.0;
    let corners = [Vec2::new(-half.x, -half.y), Vec2::new(half.x, -half.y), half, Vec2::new(-half.x, half.y)]
        .map(|corner| transform.transform_point2(corner) * computed.inverse_scale_factor());
    let min = corners.iter().copied().fold(Vec2::INFINITY, Vec2::min);
    let max = corners.iter().copied().fold(Vec2::NEG_INFINITY, Vec2::max);
    Some(Rect::from_corners(min, max))
}

#[allow(clippy::too_many_arguments)]
fn show_the_postcards(
    mut commands: Commands,
    phase: Option<Res<State<MatchPhase>>>,
    held: Res<MouseHeld>,
    printed: Option<Res<PrintedPostcards>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    kept_clear: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<KeptClear>>,
    mut roots: Query<(Entity, &mut PostcardRoot)>,
    mut cards: Query<(&mut PostcardCard, &mut Node, &mut UiTransform)>,
    time: Res<Time<Real>>,
    mut layout: Local<Layout>,
) {
    let shown = match (printed, wanted(phase_of(&phase), &held)) {
        (Some(printed), true) => printed.cards().to_vec(),
        _ => Vec::new(),
    };
    let Ok(window) = windows.single() else { return };
    let window = window.size();
    let taken: Vec<Rect> = kept_clear
        .iter()
        .filter(|(_, _, visible)| visible.get())
        .filter_map(|(computed, transform, _)| screen_rect(computed, transform))
        .collect();

    let real = time.elapsed_secs();
    if layout.seen_window != window {
        layout.seen_window = window;
        layout.window_since = real;
    }
    let settled = real - layout.window_since >= SETTLE_SECONDS;
    layout.clock += time.delta_secs().min(MOST_STEP);
    let now = layout.clock;

    for (entity, root) in &roots {
        if root.leaving_until.is_some_and(|until| now >= until) {
            commands.entity(entity).despawn();
        }
    }
    let staying = roots.iter().filter(|(_, root)| root.leaving_until.is_none()).count();
    if layout.pictures != shown || (staying == 0) == !shown.is_empty() {
        for (_, mut root) in &mut roots {
            if root.leaving_until.is_none() {
                let last = layout.pictures.len().saturating_sub(1) as f32;
                root.leaving_until = Some(now + last * STAGGER + FLY_OUT_SECONDS);
            }
        }
        for (mut card, _, _) in &mut cards {
            if !card.leaving {
                card.leaving = true;
                let from = card.flight.at(now);
                let started = now + card.index as f32 * STAGGER;
                card.flight = Flight { from, to: from.below(window), started, seconds: FLY_OUT_SECONDS, ease: ease_in_quart };
            }
        }
        layout.pictures = shown.clone();
        layout.window = window;
        layout.cards = lay_out(window, &taken, shown.len());
        layout.taken = taken;
        if !shown.is_empty() {
            fly_in(&mut commands, &layout, &shown, window, now);
        }
    } else if !shown.is_empty()
        && lays_out_again(
            layout.window,
            window,
            settled,
            layout.window != window || still_fits(window, &layout.cards, &taken),
            layout.cards.len() < shown.len(),
            layout.taken != taken,
        )
    {
        layout.window = window;
        layout.cards = lay_out(window, &taken, shown.len());
        layout.taken = taken;
        for (mut card, _, _) in cards.iter_mut().filter(|(card, _, _)| !card.leaving) {
            let from = card.flight.at(now);
            let to = layout.cards.get(card.index).copied().unwrap_or_else(|| from.below(window));
            let started = now + card.index as f32 * STAGGER;
            card.flight = Flight { from, to, started, seconds: MOVE_SECONDS, ease: ease_in_out_quart };
        }
    }

    for (card, mut node, mut transform) in &mut cards {
        let at = card.flight.at(now);
        let placed = card_node(&at);
        if node.left != placed.left || node.top != placed.top || node.width != placed.width {
            *node = placed;
        }
        let rotation = Rot2::radians(at.angle);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

fn card_node(card: &Card) -> Node {
    let size = card.half() * 2.0;
    let corner = card.center - size / 2.0;
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(corner.x),
        top: Val::Px(corner.y),
        width: Val::Px(size.x),
        height: Val::Px(size.y),
        ..default()
    }
}

/// Every card starts below the screen and flies to its place, one after another. A card with no
/// place waits below the screen.
fn fly_in(commands: &mut Commands, layout: &Layout, shown: &[Handle<Image>], window: Vec2, now: f32) {
    let waiting_place = Card { center: Vec2::new(window.x / 2.0, window.y), width: SHORTEST * window.y * 1.5, angle: 0.0 };
    commands
        .spawn((
            PostcardRoot::default(),
            GlobalZIndex(LAYER),
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
            Pickable::IGNORE,
            Name::new("Postcards"),
        ))
        .with_children(|root| {
            for (index, image) in shown.iter().enumerate() {
                let to = layout.cards.get(index).copied().unwrap_or_else(|| waiting_place.below(window));
                let flight = Flight {
                    from: to.below(window),
                    to,
                    started: now + index as f32 * STAGGER,
                    seconds: FLY_IN_SECONDS,
                    ease: ease_out_quart,
                };
                root.spawn((
                    PostcardCard { index, flight, leaving: false },
                    card_node(&flight.from),
                    UiTransform { rotation: Rot2::radians(flight.from.angle), ..default() },
                    ImageNode::new(image.clone()),
                    Pickable::IGNORE,
                ));
            }
        });
}

fn put_the_postcards_away(mut commands: Commands, roots: Query<Entity, With<PostcardRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Vec2 = Vec2::new(1280.0, 720.0);

    /// The team picker in the middle and the standings in the top left, as in a pre-game.
    fn pre_game() -> Vec<Rect> {
        vec![
            Rect::from_corners(Vec2::new(430.0, 230.0), Vec2::new(850.0, 490.0)),
            Rect::from_corners(Vec2::new(20.0, 20.0), Vec2::new(350.0, 210.0)),
            Rect::from_corners(Vec2::new(370.0, 20.0), Vec2::new(1090.0, 270.0)),
        ]
    }

    #[test]
    fn three_cards_fit_round_a_picker_without_touching_anything() {
        let taken = pre_game();
        let cards = lay_out(WINDOW, &taken, 3);
        assert_eq!(cards.len(), 3);
        assert!(still_fits(WINDOW, &cards, &taken));
        for card in &cards {
            assert!(card.width / 1.5 >= SHORTEST * WINDOW.y, "a card was laid out smaller than the shortest");
            assert!(card.angle.abs() <= 30f32.to_radians());
        }
    }

    #[test]
    fn two_cards_go_either_side() {
        let cards = lay_out(WINDOW, &pre_game(), 2);
        assert!(cards[0].center.x < WINDOW.x / 2.0);
        assert!(cards[1].center.x > WINDOW.x / 2.0);
    }

    #[test]
    fn a_window_that_grows_back_is_laid_out_again_once_settled() {
        let small = Vec2::new(200.0, 150.0);
        // Laid out for a small window with no room, so no card fits there and none is placed.
        assert!(lay_out(small, &pre_game(), 3).is_empty());
        assert!(!lays_out_again(small, WINDOW, false, true, true, false), "laid out again mid-drag");
        assert!(lays_out_again(small, WINDOW, true, true, true, false), "the cards never came back");
        assert!(!lays_out_again(WINDOW, WINDOW, true, true, false, true));
        assert!(lays_out_again(WINDOW, WINDOW, true, false, false, false));
        assert!(lays_out_again(WINDOW, WINDOW, true, true, true, true));
    }

    #[test]
    fn a_smaller_or_lower_card_tilts_more_and_none_past_the_steepest() {
        let center = Vec2::new(200.0, 300.0);
        let big = tilt(1.0, center, TALLEST * WINDOW.y * 1.5, WINDOW);
        let small = tilt(1.0, center, SHORTEST * WINDOW.y * 1.5, WINDOW);
        assert!(small > big && big > 0.0);
        let lower = tilt(1.0, center + Vec2::Y * 300.0, TALLEST * WINDOW.y * 1.5, WINDOW);
        assert!(lower > big);
        assert!(tilt(-1.0, center, SHORTEST * WINDOW.y * 1.5, WINDOW) < 0.0, "a zone's turn was ignored");
        for y in [0.0, WINDOW.y] {
            for width in [0.0, 2000.0] {
                assert!(tilt(1.0, Vec2::new(0.0, y), width, WINDOW).abs() <= STEEPEST.to_radians() + 1e-6);
            }
        }
    }

    #[test]
    fn the_eases_start_at_zero_and_end_at_one() {
        for ease in [ease_out_quart, ease_in_quart, ease_in_out_quart] {
            assert_eq!(ease(0.0), 0.0);
            assert_eq!(ease(1.0), 1.0);
            assert_eq!(ease(-1.0), 0.0, "a card waiting its turn moved");
        }
        assert!((ease_in_out_quart(0.5) - 0.5).abs() < 1e-6);
        assert!(ease_out_quart(0.25) > 0.5, "ease out does not start fast");
    }

    #[test]
    fn a_card_flies_in_from_below_the_window_to_its_place() {
        let to = Card { center: Vec2::new(300.0, 400.0), width: 300.0, angle: 0.2 };
        let flight = Flight { from: to.below(WINDOW), to, started: 1.0, seconds: FLY_IN_SECONDS, ease: ease_out_quart };
        let start = flight.at(0.5);
        let corners = start.corners(0.0);
        assert!(corners.iter().all(|corner| corner.y > WINDOW.y), "a card showed before it flew in");
        assert_eq!(flight.at(1.0 + FLY_IN_SECONDS), to);
    }

    #[test]
    fn a_window_with_no_room_lays_out_nothing() {
        let everything = vec![Rect::from_corners(Vec2::ZERO, WINDOW)];
        assert!(lay_out(WINDOW, &everything, 3).is_empty());
    }

    #[test]
    fn a_tilted_card_is_tested_by_its_corners_not_its_box() {
        let card = Card { center: Vec2::ZERO, width: 300.0, angle: 30f32.to_radians() };
        // Inside the card's bounding box, beyond its rotated corner.
        let near_the_corner = Rect::from_corners(Vec2::new(-170.0, 120.0), Vec2::new(-160.0, 130.0));
        assert!(!card.touches(near_the_corner, 0.0));
        assert!(card.touches(Rect::from_corners(Vec2::splat(-5.0), Vec2::splat(5.0)), 0.0));
    }
}
