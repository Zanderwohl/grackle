//! A tool as a list of steps, each wanting picks, shared by the editors.
//!
//! An editor describes its steps through [`Steps`]; filling them from a list
//! of picks, deciding whether the wizard is ready or finished, what to ask
//! for, and stepping back are written once here. The plan is
//! [documentation/tool-wizards.md](../../documentation/tool-wizards.md).

/// One editor's steps for one wizard.
pub trait Steps {
    type Pick: Clone + PartialEq;

    fn count(&self) -> usize;
    /// Whether `has` is enough for step `step` to build from.
    fn answered(&self, step: usize, has: &[Self::Pick]) -> bool;
    /// Whether another pick may still go into step `step`.
    fn open(&self, step: usize, has: &[Self::Pick]) -> bool;
    /// Whether an answered step `step` waits to be told it is done, since
    /// another pick is likely. Narrower than [`Steps::open`] where a step is
    /// complete as it stands but may still be grown.
    fn waits(&self, step: usize, has: &[Self::Pick]) -> bool {
        self.open(step, has)
    }
    /// Whether `pick` may join step `step`, given what every step holds.
    fn fits(&self, step: usize, pick: &Self::Pick, filled: &Filled<Self::Pick>) -> bool;
}

/// Each step's picks, in step order, and which steps were moved on from.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled<P> {
    pub has: Vec<Vec<P>>,
    /// Closed by moving on: no more picks go in, open or not.
    pub closed: Vec<bool>,
}

impl<P> Filled<P> {
    fn empty(count: usize) -> Self {
        Filled { has: (0..count).map(|_| Vec::new()).collect(), closed: vec![false; count] }
    }

    pub fn picks(&self) -> impl Iterator<Item = &P> {
        self.has.iter().flatten()
    }

    fn last_held(&self) -> Option<usize> {
        self.has.iter().rposition(|has| !has.is_empty())
    }

    /// Moving on closes every step up to the last one holding a pick.
    fn move_on(&mut self) {
        if let Some(last) = self.last_held() {
            self.closed[..=last].fill(true);
        }
    }
}

fn takes<S: Steps>(steps: &S, filled: &Filled<S::Pick>, step: usize, pick: &S::Pick) -> bool {
    !filled.closed[step] && steps.open(step, &filled.has[step]) && steps.fits(step, pick, filled)
}

/// Each pick, in order, into the first open step it fits. A pick that fits
/// nowhere is dropped, so a selection made for something else does not stop a
/// wizard starting. `moved_on` are the picks after which the wizard was moved
/// on to the next step ([`move_on`]).
pub fn fill<S: Steps>(steps: &S, picks: &[S::Pick], moved_on: &[S::Pick]) -> Filled<S::Pick> {
    let mut filled = Filled::empty(steps.count());
    for pick in picks {
        let Some(at) = (0..steps.count()).find(|&i| takes(steps, &filled, i, pick)) else { continue };
        filled.has[at].push(pick.clone());
        if moved_on.contains(pick) {
            filled.move_on();
        }
    }
    filled
}

/// Fills from a selection made before the wizard started, and says where it
/// moved on. A step that is answered yields a pick to a later step that is
/// not and would take it, so bodies A, B and C fed to a target step and a
/// tools step give A as the target and B and C as the tools.
pub fn seed<S: Steps>(steps: &S, picks: &[S::Pick]) -> (Filled<S::Pick>, Vec<S::Pick>) {
    let mut filled = Filled::empty(steps.count());
    let mut moved_on = Vec::new();
    let mut last: Option<S::Pick> = None;
    for pick in picks {
        let Some(mut at) = (0..steps.count()).find(|&i| takes(steps, &filled, i, pick)) else { continue };
        if steps.answered(at, &filled.has[at])
            && let Some(later) = (at + 1..steps.count())
                .find(|&j| !steps.answered(j, &filled.has[j]) && takes(steps, &filled, j, pick))
            // Moving on closes up to the last step holding a pick, which must
            // stop short of `later`, or filling again would not reach it.
            && filled.last_held().is_none_or(|held| held < later)
            && let Some(last) = last.clone()
        {
            filled.move_on();
            moved_on.push(last);
            at = later;
        }
        filled.has[at].push(pick.clone());
        last = Some(pick.clone());
    }
    (filled, moved_on)
}

/// Every step answered, so confirming builds.
pub fn is_ready<S: Steps>(steps: &S, filled: &Filled<S::Pick>) -> bool {
    (0..steps.count()).all(|i| steps.answered(i, &filled.has[i]))
}

/// Ready, and the last step does not wait for more. An earlier step that is
/// still open was moved past by a pick that went into a later one.
pub fn is_finished<S: Steps>(steps: &S, filled: &Filled<S::Pick>) -> bool {
    let last_waits = steps.count().checked_sub(1).is_some_and(|last| steps.waits(last, &filled.has[last]));
    is_ready(steps, filled) && !last_waits
}

/// The first step not answered, which is the one under way; `None` once all
/// are.
pub fn under_way<S: Steps>(steps: &S, filled: &Filled<S::Pick>) -> Option<usize> {
    (0..steps.count()).find(|&i| !steps.answered(i, &filled.has[i]))
}

/// Whether Enter or Next would move on. A full step moves on by itself.
pub fn can_move_on<S: Steps>(steps: &S, filled: &Filled<S::Pick>) -> bool {
    let (Some(under_way), Some(last)) = (under_way(steps, filled), filled.last_held()) else { return false };
    last < under_way && !filled.closed[last] && steps.open(last, &filled.has[last])
}

/// Moves on after the last pick, so the next goes to a later step.
pub fn move_on<S: Steps>(steps: &S, picks: &[S::Pick], moved_on: &mut Vec<S::Pick>) {
    let filled = fill(steps, picks, moved_on);
    // The last pick a step holds: a marker on a dropped pick would close nothing.
    if can_move_on(steps, &filled)
        && let Some(last) = picks.iter().rev().find(|pick| filled.picks().any(|held| held == *pick))
    {
        moved_on.push(last.clone());
    }
}

/// Whether clicking `candidate` would do anything: unpick it, or fill a step.
pub fn accepts<S: Steps>(steps: &S, picks: &[S::Pick], moved_on: &[S::Pick], candidate: &S::Pick) -> bool {
    if picks.contains(candidate) {
        return true;
    }
    let mut with = picks.to_vec();
    with.push(candidate.clone());
    fill(steps, &with, moved_on).picks().any(|pick| pick == candidate)
}

/// What Escape does to a wizard.
#[derive(Clone, Debug, PartialEq)]
pub enum Back<P> {
    /// The last pick, in pick order, was taken back.
    Took(P),
    /// Moving on was taken back, so that step is open again.
    Reopened,
    /// Nothing was picked, so the wizard is left.
    Exit,
}

/// Takes back the last thing done: moving on, or else the last pick,
/// wherever it went. With nothing picked, the wizard exits. Markers in
/// `moved_on` whose pick is gone are dropped.
pub fn back<P: PartialEq>(picks: &mut Vec<P>, moved_on: &mut Vec<P>) -> Back<P> {
    moved_on.retain(|marker| picks.contains(marker));
    if picks.last().is_some() && moved_on.last() == picks.last() {
        moved_on.pop();
        return Back::Reopened;
    }
    match picks.pop() {
        Some(pick) => {
            moved_on.retain(|marker| picks.contains(marker));
            Back::Took(pick)
        }
        None => Back::Exit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Even numbers then odd ones: one of each, then any number of odd ones.
    struct EvenThenOdds;

    impl Steps for EvenThenOdds {
        type Pick = u32;
        fn count(&self) -> usize {
            2
        }
        fn answered(&self, _: usize, has: &[u32]) -> bool {
            !has.is_empty()
        }
        fn open(&self, step: usize, has: &[u32]) -> bool {
            step == 1 || has.is_empty()
        }
        fn fits(&self, step: usize, pick: &u32, filled: &Filled<u32>) -> bool {
            pick % 2 == step as u32 && !filled.picks().any(|p| p == pick)
        }
    }

    /// Two lists of any numbers but 9, each wanting one at least.
    struct TwoLists;

    impl Steps for TwoLists {
        type Pick = u32;
        fn count(&self) -> usize {
            2
        }
        fn answered(&self, _: usize, has: &[u32]) -> bool {
            !has.is_empty()
        }
        fn open(&self, _: usize, _: &[u32]) -> bool {
            true
        }
        fn fits(&self, _: usize, pick: &u32, filled: &Filled<u32>) -> bool {
            *pick != 9 && !filled.picks().any(|p| p == pick)
        }
    }

    #[test]
    fn picks_go_to_the_first_open_step_they_fit_and_the_rest_are_dropped() {
        let filled = fill(&EvenThenOdds, &[3, 2, 4, 5, 3], &[]);
        assert_eq!(filled.has, vec![vec![2], vec![3, 5]]);
        assert!(is_ready(&EvenThenOdds, &filled));
        assert!(!is_finished(&EvenThenOdds, &filled), "the last step takes any number");
        assert_eq!(under_way(&EvenThenOdds, &fill(&EvenThenOdds, &[3], &[])), Some(0));
    }

    #[test]
    fn escape_takes_picks_back_one_at_a_time_then_exits() {
        let mut picks = vec![2, 3, 5];
        let mut moved_on = vec![];
        assert_eq!(back(&mut picks, &mut moved_on), Back::Took(5));
        assert_eq!(back(&mut picks, &mut moved_on), Back::Took(3));
        assert_eq!(fill(&EvenThenOdds, &picks, &moved_on).has, vec![vec![2], vec![]]);
        assert_eq!(back(&mut picks, &mut moved_on), Back::Took(2));
        assert_eq!(back(&mut picks, &mut moved_on), Back::Exit);
    }

    #[test]
    fn unpicking_is_always_accepted() {
        assert!(accepts(&EvenThenOdds, &[2], &[], &2));
        assert!(!accepts(&EvenThenOdds, &[2], &[], &4), "the even step is full");
        assert!(accepts(&EvenThenOdds, &[2], &[], &7));
    }

    #[test]
    fn a_selection_of_three_into_two_lists_fills_one_then_two() {
        let (filled, moved_on) = seed(&TwoLists, &[1, 2, 3]);
        assert_eq!(filled.has, vec![vec![1], vec![2, 3]]);
        assert_eq!(fill(&TwoLists, &[1, 2, 3], &moved_on), filled, "filling again with the marker agrees");
    }

    #[test]
    fn moving_on_sends_the_next_pick_to_the_next_step() {
        let mut moved_on = vec![];
        assert!(!can_move_on(&TwoLists, &fill(&TwoLists, &[], &moved_on)), "nothing to move on from");
        assert!(can_move_on(&TwoLists, &fill(&TwoLists, &[1, 2], &moved_on)));
        move_on(&TwoLists, &[1, 2], &mut moved_on);
        assert_eq!(fill(&TwoLists, &[1, 2, 3], &moved_on).has, vec![vec![1, 2], vec![3]]);
        assert!(!can_move_on(&TwoLists, &fill(&TwoLists, &[1, 2, 3], &moved_on)), "the last step has nowhere to move on to");
        assert!(!can_move_on(&EvenThenOdds, &fill(&EvenThenOdds, &[2], &[])), "a full step moves on by itself");
    }

    #[test]
    fn escape_after_a_steps_first_pick_then_reopens_the_step_before() {
        let mut picks = vec![1, 2, 3];
        let mut moved_on = vec![2];
        assert_eq!(back(&mut picks, &mut moved_on), Back::Took(3));
        assert_eq!(back(&mut picks, &mut moved_on), Back::Reopened);
        assert_eq!(fill(&TwoLists, &[1, 2, 3], &moved_on).has, vec![vec![1, 2, 3], vec![]]);
        assert_eq!(back(&mut picks, &mut moved_on), Back::Took(2));
    }

    #[test]
    fn moving_on_marks_the_last_pick_a_step_holds() {
        let mut moved_on = vec![];
        move_on(&EvenThenOdds, &[1, 3, 2], &mut moved_on);
        assert!(moved_on.is_empty(), "the even step is full, so there is no moving on");
        move_on(&TwoLists, &[1, 9], &mut moved_on);
        assert_eq!(moved_on, vec![1], "9 fits nowhere, so the marker is the pick before it");
    }
}
