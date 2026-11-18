//! A loft's sections: one profile per sketch, matched loop to loop and point
//! to point so [`skin`](crate::prop::skin::skin) can join them. See
//! `documentation/prop-modeling.md`, "Loft".
//!
//! Loops with the same number of entities on every profile are matched
//! entity to entity, each entity cut into as many pieces as its most finely
//! cut counterpart. Otherwise every loop is cut to the largest point count,
//! the extra points going into the longest segments so corners stay put.
//! Each loop then starts at a picked point, or at the turn of its points
//! nearest the previous profile's.

use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::prop::skin::{Run, Section, TaggedLoop};

/// One profile in its sketch's plane: the outside loop, then holes.
#[derive(Clone, Debug)]
pub struct Profile {
    pub frame: Affine3A,
    pub loops: Vec<TaggedLoop>,
    /// Where the outside loop should start, in the prop.
    pub start: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoftProblem {
    /// Fewer than two profiles, or three when closed.
    TooFew,
    /// Profiles with different numbers of holes.
    Holes,
    /// Two profiles in a row centered on one point, so there is no telling
    /// which way the loft runs.
    Coincident,
}

impl LoftProblem {
    pub fn message(self) -> String {
        match self {
            LoftProblem::TooFew => crate::get!("prop.problems.loft_too_few"),
            LoftProblem::Holes => crate::get!("prop.problems.loft_holes"),
            LoftProblem::Coincident => crate::get!("prop.problems.loft_coincident"),
        }
    }
}

/// Meters between two profiles' centers below which they are one place.
const COINCIDENT: f32 = 1e-5;

pub fn run(mut profiles: Vec<Profile>, closed: bool) -> Result<Run, LoftProblem> {
    let least = if closed { 3 } else { 2 };
    if profiles.len() < least || profiles.iter().any(|profile| profile.loops.is_empty()) {
        return Err(LoftProblem::TooFew);
    }
    if profiles.iter().any(|profile| profile.loops.len() != profiles[0].loops.len()) {
        return Err(LoftProblem::Holes);
    }
    face_along_the_run(&mut profiles, closed)?;
    for profile in &mut profiles {
        for tagged in &mut profile.loops {
            if area_x2(&tagged.points) < 0.0 {
                *tagged = reversed(tagged);
            }
        }
    }
    match_holes(&mut profiles);

    let mut matched: Vec<Vec<TaggedLoop>> = vec![Vec::new(); profiles.len()];
    for l in 0..profiles[0].loops.len() {
        let loops: Vec<&TaggedLoop> = profiles.iter().map(|profile| &profile.loops[l]).collect();
        let frames: Vec<Affine3A> = profiles.iter().map(|profile| profile.frame).collect();
        let starts: Vec<Option<Vec3>> = match l {
            0 => profiles.iter().map(|profile| profile.start).collect(),
            _ => vec![None; profiles.len()],
        };
        let runs: Vec<Vec<Piece>> = loops.iter().map(|tagged| pieces(tagged)).collect();
        let by_entity = runs[0].len() > 1 && runs.iter().all(|r| r.len() == runs[0].len());
        let out = match by_entity {
            true => match_by_entity(runs, &frames, &starts),
            false => match_by_point(&loops, &frames, &starts),
        };
        for (k, tagged) in out.into_iter().enumerate() {
            matched[k].push(tagged);
        }
    }
    let span_count = if closed { profiles.len() } else { profiles.len() - 1 };
    Ok(Run {
        sections: profiles.iter().zip(matched).map(|(profile, loops)| Section { loops, frame: profile.frame }).collect(),
        spans: (0..span_count as u32).collect(),
        closed,
    })
}

fn area_x2(points: &[Vec2]) -> f32 {
    (0..points.len()).map(|i| points[i].perp_dot(points[(i + 1) % points.len()])).sum()
}

fn centroid(points: &[Vec2]) -> Vec2 {
    points.iter().copied().sum::<Vec2>() / points.len().max(1) as f32
}

fn world(frame: &Affine3A, p: Vec2) -> Vec3 {
    frame.transform_point3(p.extend(0.0))
}

/// Segment `i` runs from point `i` to `i + 1`, so reversing the points moves
/// each segment's tag to the segment between the same two points.
fn reversed(tagged: &TaggedLoop) -> TaggedLoop {
    let n = tagged.points.len();
    TaggedLoop {
        points: tagged.points.iter().rev().copied().collect(),
        tags: (0..n).map(|j| tagged.tags[(2 * n - 2 - j) % n]).collect(),
    }
}

/// Turns each frame so its normal points the way the loft travels, flipping
/// it over its own x axis where it faced back. A sketch's normal is wherever
/// its plane happened to face; the skin needs every section facing one way.
fn face_along_the_run(profiles: &mut [Profile], closed: bool) -> Result<(), LoftProblem> {
    let centers: Vec<Vec3> =
        profiles.iter().map(|profile| world(&profile.frame, centroid(&profile.loops[0].points))).collect();
    let n = profiles.len();
    for pair in 0..if closed { n } else { n - 1 } {
        if centers[pair].distance(centers[(pair + 1) % n]) < COINCIDENT {
            return Err(LoftProblem::Coincident);
        }
    }
    for (k, profile) in profiles.iter_mut().enumerate() {
        let travel = match (closed, k) {
            (true, _) => centers[(k + 1) % n] - centers[(k + n - 1) % n],
            (false, 0) => centers[1] - centers[0],
            (false, k) if k == n - 1 => centers[k] - centers[k - 1],
            (false, k) => centers[k + 1] - centers[k - 1],
        };
        if Vec3::from(profile.frame.matrix3.z_axis).dot(travel) < 0.0 {
            profile.frame = profile.frame * Affine3A::from_rotation_x(std::f32::consts::PI);
            for tagged in &mut profile.loops {
                for p in &mut tagged.points {
                    p.y = -p.y;
                }
            }
        }
    }
    Ok(())
}

/// Puts each profile's holes in the order of the nearest holes on the
/// profile before it.
fn match_holes(profiles: &mut [Profile]) {
    let center = |profile: &Profile, l: usize| world(&profile.frame, centroid(&profile.loops[l].points));
    for k in 1..profiles.len() {
        let before: Vec<Vec3> = (1..profiles[k - 1].loops.len()).map(|l| center(&profiles[k - 1], l)).collect();
        let mut left: Vec<TaggedLoop> = profiles[k].loops.drain(1..).collect();
        let frame = profiles[k].frame;
        for target in before {
            let nearest = (0..left.len())
                .min_by(|&a, &b| {
                    let d = |l: usize| world(&frame, centroid(&left[l].points)).distance(target);
                    d(a).total_cmp(&d(b))
                })
                .expect("as many holes as the profile before");
            profiles[k].loops.push(left.remove(nearest));
        }
    }
}

/// A run of segments sharing one tag: its points, first to last, and the tag.
#[derive(Clone, Debug)]
struct Piece {
    points: Vec<Vec2>,
    tag: u32,
}

/// The loop cut where its tag changes, starting at a change. A loop of one
/// tag is one piece.
fn pieces(tagged: &TaggedLoop) -> Vec<Piece> {
    let n = tagged.points.len();
    let Some(first) = (0..n).find(|&i| tagged.tags[i] != tagged.tags[(i + n - 1) % n]) else {
        let mut points = tagged.points.clone();
        points.push(tagged.points[0]);
        return vec![Piece { points, tag: tagged.tags.first().copied().unwrap_or(0) }];
    };
    let mut out: Vec<Piece> = Vec::new();
    for step in 0..n {
        let i = (first + step) % n;
        let tag = tagged.tags[i];
        match out.last_mut() {
            Some(piece) if piece.tag == tag => piece.points.push(tagged.points[(i + 1) % n]),
            _ => out.push(Piece { points: vec![tagged.points[i], tagged.points[(i + 1) % n]], tag }),
        }
    }
    out
}

/// The open polyline `points` cut into `count` segments, the extra points
/// going one at a time into whichever segment is then longest per piece.
fn cut(points: &[Vec2], count: usize) -> Vec<Vec2> {
    let segments = points.len() - 1;
    let lengths: Vec<f32> = points.windows(2).map(|w| w[0].distance(w[1])).collect();
    let mut split = vec![1usize; segments];
    for _ in segments..count {
        let longest = (0..segments)
            .max_by(|&a, &b| (lengths[a] / split[a] as f32).total_cmp(&(lengths[b] / split[b] as f32)))
            .expect("a segment");
        split[longest] += 1;
    }
    let mut out = Vec::with_capacity(count + 1);
    for (i, &k) in split.iter().enumerate() {
        for m in 0..k {
            out.push(points[i].lerp(points[i + 1], m as f32 / k as f32));
        }
    }
    out.push(points[segments]);
    out
}

/// The rotation of `points` (all `n` of them, centered on `center`) starting
/// nearest `start` if there is one, else lying nearest `previous`.
fn best_turn(points: &[Vec3], center: Vec3, previous: Option<&[Vec3]>, start: Option<Vec3>) -> usize {
    let n = points.len();
    if let Some(start) = start {
        return (0..n).min_by(|&a, &b| points[a].distance(start).total_cmp(&points[b].distance(start))).unwrap_or(0);
    }
    let Some(previous) = previous else { return 0 };
    let previous_center = previous.iter().copied().sum::<Vec3>() / previous.len() as f32;
    let cost = |turn: usize| -> f32 {
        (0..n).map(|i| ((points[(i + turn) % n] - center) - (previous[i] - previous_center)).length_squared()).sum()
    };
    (0..n).min_by(|&a, &b| cost(a).total_cmp(&cost(b))).unwrap_or(0)
}

fn match_by_point(loops: &[&TaggedLoop], frames: &[Affine3A], starts: &[Option<Vec3>]) -> Vec<TaggedLoop> {
    let count = loops.iter().map(|tagged| tagged.points.len()).max().unwrap_or(0);
    let mut out: Vec<TaggedLoop> = Vec::with_capacity(loops.len());
    let mut previous: Option<Vec<Vec3>> = None;
    for (k, tagged) in loops.iter().enumerate() {
        let n = tagged.points.len();
        let mut closed = tagged.points.clone();
        closed.push(tagged.points[0]);
        let points = cut(&closed, count);
        // Which original segment each new one lies on, for its tag.
        let mut tags = Vec::with_capacity(count);
        let mut segment = 0;
        for p in &points[..count] {
            while segment + 1 < n && *p == closed[segment + 1] {
                segment += 1;
            }
            tags.push(tagged.tags[segment]);
        }
        let points = &points[..count];
        let placed: Vec<Vec3> = points.iter().map(|p| world(&frames[k], *p)).collect();
        let center = placed.iter().copied().sum::<Vec3>() / count as f32;
        let turn = best_turn(&placed, center, previous.as_deref(), starts[k]);
        let turned: Vec<Vec2> = (0..count).map(|i| points[(i + turn) % count]).collect();
        let tags: Vec<u32> = (0..count).map(|i| tags[(i + turn) % count]).collect();
        previous = Some((0..count).map(|i| placed[(i + turn) % count]).collect());
        out.push(TaggedLoop { points: turned, tags });
    }
    out
}

fn match_by_entity(runs: Vec<Vec<Piece>>, frames: &[Affine3A], starts: &[Option<Vec3>]) -> Vec<TaggedLoop> {
    let r = runs[0].len();
    let mut turned: Vec<Vec<Piece>> = Vec::with_capacity(runs.len());
    let mut previous: Option<Vec<Vec3>> = None;
    for (k, pieces) in runs.into_iter().enumerate() {
        let firsts: Vec<Vec3> = pieces.iter().map(|piece| world(&frames[k], piece.points[0])).collect();
        let every: Vec<Vec3> = pieces.iter().flat_map(|piece| &piece.points[..piece.points.len() - 1]).map(|p| world(&frames[k], *p)).collect();
        let center = every.iter().copied().sum::<Vec3>() / every.len() as f32;
        let turn = best_turn(&firsts, center, previous.as_deref(), starts[k]);
        previous = Some((0..r).map(|i| firsts[(i + turn) % r]).collect());
        turned.push((0..r).map(|i| pieces[(i + turn) % r].clone()).collect());
    }
    let counts: Vec<usize> = (0..r).map(|j| turned.iter().map(|pieces| pieces[j].points.len() - 1).max().unwrap_or(1)).collect();
    turned
        .into_iter()
        .map(|pieces| {
            let mut points = Vec::new();
            let mut tags = Vec::new();
            for (piece, &count) in pieces.iter().zip(&counts) {
                let cut = cut(&piece.points, count);
                points.extend_from_slice(&cut[..count]);
                tags.extend(std::iter::repeat_n(piece.tag, count));
            }
            TaggedLoop { points, tags }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(half: f32) -> TaggedLoop {
        TaggedLoop::numbered(vec![
            Vec2::new(-half, -half),
            Vec2::new(half, -half),
            Vec2::new(half, half),
            Vec2::new(-half, half),
        ])
    }

    fn circle(radius: f32, n: usize) -> TaggedLoop {
        let points = (0..n).map(|i| Vec2::from_angle(std::f32::consts::TAU * i as f32 / n as f32) * radius).collect();
        TaggedLoop { points, tags: vec![7; n] }
    }

    fn at(z: f32, loops: Vec<TaggedLoop>) -> Profile {
        Profile { frame: Affine3A::from_translation(Vec3::Z * z), loops, start: None }
    }

    #[test]
    fn a_reversed_loop_keeps_each_tag_on_its_segment() {
        let tagged = TaggedLoop { points: vec![Vec2::ZERO, Vec2::X, Vec2::Y], tags: vec![10, 11, 12] };
        let back = reversed(&tagged);
        // Segment 0 of the reversed loop runs from (0,1) to (1,0): segment 1.
        assert_eq!(back.points, vec![Vec2::Y, Vec2::X, Vec2::ZERO]);
        assert_eq!(back.tags, vec![11, 10, 12]);
    }

    #[test]
    fn cutting_keeps_every_corner() {
        let points = vec![Vec2::ZERO, Vec2::X, Vec2::new(1.0, 3.0)];
        let cut = cut(&points, 8);
        assert_eq!(cut.len(), 9);
        assert!(cut.contains(&Vec2::X));
        assert_eq!(cut.last(), Some(&Vec2::new(1.0, 3.0)));
    }

    #[test]
    fn a_square_and_a_circle_are_cut_to_one_count() {
        let run = run(vec![at(0.0, vec![square(0.05)]), at(0.1, vec![circle(0.04, 24)])], false).unwrap();
        let counts: Vec<usize> = run.sections.iter().map(|s| s.loops[0].points.len()).collect();
        assert_eq!(counts, vec![24, 24]);
        // The square's tags survive the cut.
        let mut tags = run.sections[0].loops[0].tags.clone();
        tags.dedup();
        assert_eq!(tags, vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_turned_square_starts_at_the_corner_nearest_the_first() {
        let mut turned = square(0.05);
        turned.points.rotate_left(2);
        let run = run(vec![at(0.0, vec![square(0.05)]), at(0.1, vec![turned])], false).unwrap();
        let (a, b) = (&run.sections[0].loops[0].points, &run.sections[1].loops[0].points);
        assert_eq!(a[0], b[0]);
    }

    #[test]
    fn a_picked_start_wins_over_the_nearest_turn() {
        let mut second = at(0.1, vec![square(0.05)]);
        second.start = Some(Vec3::new(0.05, 0.05, 0.1));
        let run = run(vec![at(0.0, vec![square(0.05)]), second], false).unwrap();
        assert_eq!(run.sections[1].loops[0].points[0], Vec2::new(0.05, 0.05));
    }

    #[test]
    fn a_profile_facing_back_is_turned_round() {
        let mut back = at(0.1, vec![square(0.05)]);
        back.frame = back.frame * Affine3A::from_rotation_y(std::f32::consts::PI);
        let run = run(vec![at(0.0, vec![square(0.05)]), back], false).unwrap();
        for section in &run.sections {
            assert!(section.frame.matrix3.z_axis.z > 0.0);
            assert!(area_x2(&section.loops[0].points) > 0.0);
        }
    }

    #[test]
    fn entities_are_matched_one_to_one() {
        // Four sides, the second cut into three pieces on the far profile.
        let near = square(0.05);
        let far = TaggedLoop {
            points: vec![
                Vec2::new(-0.05, -0.05),
                Vec2::new(0.05, -0.05),
                Vec2::new(0.06, 0.0),
                Vec2::new(0.06, 0.03),
                Vec2::new(0.05, 0.05),
                Vec2::new(-0.05, 0.05),
            ],
            tags: vec![0, 1, 1, 1, 2, 3],
        };
        let run = run(vec![at(0.0, vec![near]), at(0.1, vec![far])], false).unwrap();
        let tags = &run.sections[0].loops[0].tags;
        assert_eq!(tags.len(), 6);
        assert_eq!(tags.iter().filter(|t| **t == 1).count(), 3);
    }

    #[test]
    fn profiles_need_the_same_holes() {
        let problem = run(vec![at(0.0, vec![square(0.05), square(0.01)]), at(0.1, vec![square(0.05)])], false);
        assert_eq!(problem.unwrap_err(), LoftProblem::Holes);
    }
}
