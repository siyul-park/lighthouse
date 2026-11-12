//! The exact solver of the levels: minimum-cost tension over integer levels.
//!
//! Every term is a convex function of a difference of two levels (or of one
//! level), so the cost is L-natural-convex. The solver starts from the least
//! feasible levels (the longest paths) and raises sets of levels by one: each
//! round finds, with one minimum cut ([`Network`]), the inclusion-minimal set
//! whose raise lowers the cost most, and stops when no raise lowers it. The
//! least optimum is reached because the start is below it and the cuts are
//! minimal.

use super::network::Network;

/// The most rounds a solve may take: a guard, never reached by a well-formed
/// problem (a round raises at least one level, and the levels stay below the
/// number of variables).
const MAX_ROUNDS: usize = 1 << 20;

/// Levels `>= 1` to find, a linear cost `slope * (level - 1)` on each, and
/// terms on the differences of pairs. A frozen variable stays at its start.
#[derive(Debug, Clone, Default)]
pub struct Problem {
    pub slope: Vec<i64>,
    pub frozen: Vec<bool>,
    pub arcs: Vec<(usize, usize, Term)>,
}

/// What a solve found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Solved {
    pub start: Vec<i64>,
    pub levels: Vec<i64>,
    pub rounds: usize,
    pub start_cost: i128,
    pub cost: i128,
}

/// The problem has no levels: the floors of its terms cannot hold together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Infeasible;

/// A convex term of the difference `t = level(a) - level(b)` of an arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Term {
    /// Free for `t >= floor`, impossible below.
    Hard { floor: i64 },
    /// `weight * (t - floor)` for `t >= floor`, impossible below.
    Linear { floor: i64, weight: i64 },
    /// `weight * |t - target|`.
    Distance { target: i64, weight: i64 },
}

impl Problem {
    /// A problem of `vars` variables with no cost and no term.
    pub fn new(vars: usize) -> Self {
        Self {
            slope: vec![0; vars],
            frozen: vec![false; vars],
            arcs: Vec::new(),
        }
    }

    /// The cost of `levels`; `None` when one is below 1 or a floor is broken.
    pub fn cost(&self, levels: &[i64]) -> Option<i128> {
        if levels.iter().any(|&p| p < 1) {
            return None;
        }
        let unary: i128 = self
            .slope
            .iter()
            .zip(levels)
            .map(|(&s, &p)| i128::from(s) * i128::from(p - 1))
            .sum();
        self.arcs.iter().try_fold(unary, |sum, &(a, b, term)| {
            Some(sum + term.value(levels[a] - levels[b])?)
        })
    }

    /// The least feasible levels (the longest paths above the floor 1).
    pub fn start(&self) -> Result<Vec<i64>, Infeasible> {
        let mut p = vec![1i64; self.slope.len()];
        for _ in 0..=p.len() {
            let mut changed = false;
            for &(a, b, term) in &self.arcs {
                let floor = match term {
                    Term::Hard { floor } | Term::Linear { floor, .. } => floor,
                    Term::Distance { .. } => continue,
                };
                if p[a] < p[b] + floor {
                    p[a] = p[b] + floor;
                    changed = true;
                }
            }
            if !changed {
                return Ok(p);
            }
        }
        Err(Infeasible)
    }

    /// The least optimal levels, from the least feasible `start` (see
    /// [`Problem::start`]).
    pub fn solve(&self, start: Vec<i64>) -> Result<Solved, Infeasible> {
        let start_cost = self.cost(&start).ok_or(Infeasible)?;
        let mut levels = start.clone();
        let incident = self.incident();
        let mut rounds = 0;
        while rounds < MAX_ROUNDS {
            self.settle(&mut levels, &incident);
            let Some(raise) = Network::of(self, &levels).raise() else {
                break;
            };
            for (level, up) in levels.iter_mut().zip(raise) {
                *level += i64::from(up);
            }
            rounds += 1;
        }
        let cost = self.cost(&levels).ok_or(Infeasible)?;
        Ok(Solved {
            start,
            levels,
            rounds,
            start_cost,
            cost,
        })
    }

    /// The arcs each variable is on.
    fn incident(&self) -> Vec<Vec<usize>> {
        let mut at = vec![Vec::new(); self.slope.len()];
        for (i, &(a, b, _)) in self.arcs.iter().enumerate() {
            at[a].push(i);
            at[b].push(i);
        }
        at
    }

    /// Raises every variable to its least best level given the others, until
    /// none moves. A step that lowers the cost of one variable alone never
    /// passes the least optimum, so the cuts start from there.
    fn settle(&self, levels: &mut [i64], incident: &[Vec<usize>]) {
        let mut moved = true;
        while moved {
            moved = false;
            for v in (0..levels.len()).filter(|&v| !self.frozen[v]) {
                while self.gain(v, levels, &incident[v]).is_some_and(|g| g < 0) {
                    levels[v] += 1;
                    moved = true;
                }
            }
        }
    }

    /// The change of the cost of raising `v` by one; `None` when that breaks
    /// a floor.
    fn gain(&self, v: usize, levels: &[i64], arcs: &[usize]) -> Option<i64> {
        arcs.iter().try_fold(self.slope[v], |sum, &i| {
            let (a, b, term) = self.arcs[i];
            let (up, down) = term.moves(levels[a] - levels[b]);
            let mine = if a == v { Some(up) } else { down };
            Some(sum + mine?)
        })
    }
}

impl Term {
    fn value(self, t: i64) -> Option<i128> {
        match self {
            Self::Hard { floor } => (t >= floor).then_some(0),
            Self::Linear { floor, weight } => {
                (t >= floor).then(|| i128::from(weight) * i128::from(t - floor))
            }
            Self::Distance { target, weight } => {
                Some(i128::from(weight) * i128::from((t - target).abs()))
            }
        }
    }

    /// What raising `a` alone, and raising `b` alone, adds to the cost at
    /// `t`; `None` for the second when lowering `t` is impossible.
    pub(super) fn moves(self, t: i64) -> (i64, Option<i64>) {
        match self {
            Self::Hard { floor } => (0, (t != floor).then_some(0)),
            Self::Linear { floor, weight } => (weight, (t != floor).then_some(-weight)),
            Self::Distance { target, weight } => {
                let at = (t - target).abs();
                (
                    weight * ((t + 1 - target).abs() - at),
                    Some(weight * ((t - 1 - target).abs() - at)),
                )
            }
        }
    }
}
