//! The levels of the functions of a project: the call-graph height adjusted
//! so that what a step uses together sits on one level (design/abstraction-level
//! H2). A variable is the level of a group of mutually calling functions, of
//! an abstract method or of an external symbol; the longest-path levels
//! (`height + 1`) are the start, and the solver moves the levels to the least
//! cost of the steps' distance from the level below their function.

mod groups;
mod network;
mod slots;
pub mod solve;

use std::collections::BTreeMap;

use globset::{Glob, GlobSet, GlobSetBuilder};
use lighthouse_model::ucm::{Model, SymbolId};

use self::{
    groups::{Groups, Pairs, STEP_WEIGHT, add},
    slots::{Slots, classify},
    solve::{Problem, Term},
};

/// What shapes the instance; the options of the decision that reads the levels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelOptions {
    /// The primitive operations a step with no named callee needs to count.
    pub min_ops: u32,
    /// Globs on the callee names that are never a named operation.
    pub neutral_calls: Vec<String>,
    pub edges: Edges,
    pub externals: Externals,
}

/// How the callees of a step pull on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Edges {
    /// One arc to each callee, the step's weight split between them.
    #[default]
    Mean,
    /// One arc to the callees of the highest longest-path level only.
    Top,
}

/// Whether the levels of external symbols move.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Externals {
    #[default]
    Float,
    /// They stay at the floor, as the longest-path levels have them.
    Fixed,
}

/// The levels of a project.
#[derive(Debug, Clone)]
pub struct Levels {
    start: Vec<i64>,
    level: Vec<i64>,
    functions: BTreeMap<SymbolId, usize>,
    externals: BTreeMap<(String, String), usize>,
    steps: BTreeMap<SymbolId, Vec<Slot>>,
    pub stats: Stats,
}

/// The counts of an instance and of its solve.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub groups: usize,
    pub externals: usize,
    pub pairs: usize,
    pub distances: usize,
    pub atom_steps: usize,
    pub named_steps: usize,
    /// Named steps with more than 12 callees, whose split is rounded.
    pub rounded_steps: usize,
    pub rounds: usize,
    pub start_cost: i128,
    pub cost: i128,
}

/// What a step is to the levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// Counts for nothing: a guard, or no named callee and few primitives.
    Neutral,
    /// Mechanics: primitive operations only, at level 0.
    Atom,
    /// Named operations: the variables of the groups and external symbols it
    /// calls.
    Named(Vec<usize>),
}

impl Levels {
    /// The levels of the functions and methods of `project`.
    pub fn of(project: &dyn Model, options: &LevelOptions) -> Result<Self, globset::Error> {
        let groups = Groups::of(project);
        let slots = classify(project, &groups, options, &neutral_set(options)?);
        let vars = groups.count + slots.externals.len();
        let mut pairs = groups.pairs(project);
        let mut start = Problem::new(vars);
        start.arcs = pairs
            .iter()
            .map(|(&(a, b), &(floor, _))| (a, b, Term::Hard { floor }))
            .collect();
        let start = start
            .start()
            .expect("groups form a DAG, so the floors hold");
        let (problem, stats) = arcs(&groups, &slots, options, (&mut pairs, &start));
        let solved = problem
            .solve(start.clone())
            .expect("the floors hold at the start, so the problem is feasible");
        let stats = Stats {
            rounds: solved.rounds,
            start_cost: solved.start_cost,
            cost: solved.cost,
            ..stats
        };
        Ok(Self {
            start,
            level: solved.levels,
            functions: groups
                .ids
                .iter()
                .map(|id| (id.clone(), groups.var(id).expect("an id of the groups")))
                .collect(),
            externals: slots.externals,
            steps: slots.of,
            stats,
        })
    }

    /// The solved level of a function, method or abstract method.
    pub fn level(&self, id: &SymbolId) -> Option<i64> {
        self.functions.get(id).map(|&v| self.level[v])
    }

    /// The longest-path level of the same (its height plus one).
    pub fn start(&self, id: &SymbolId) -> Option<i64> {
        self.functions.get(id).map(|&v| self.start[v])
    }

    /// What each step of a function is, in source order.
    pub fn slots(&self, id: &SymbolId) -> &[Slot] {
        self.steps.get(id).map_or(&[], Vec::as_slice)
    }

    /// The level of a step: its highest named operation, 0 for mechanics,
    /// none for a neutral one. `solved` picks the solved or the start levels.
    pub fn step_level(&self, slot: &Slot, solved: bool) -> Option<i64> {
        let levels = if solved { &self.level } else { &self.start };
        match slot {
            Slot::Neutral => None,
            Slot::Atom => Some(0),
            Slot::Named(vars) => vars.iter().map(|&v| levels[v]).max(),
        }
    }

    /// The external symbols, as `(language, name, start, level)`.
    pub fn externals(&self) -> impl Iterator<Item = (&str, &str, i64, i64)> {
        self.externals
            .iter()
            .map(|((lang, name), &v)| (lang.as_str(), name.as_str(), self.start[v], self.level[v]))
    }
}

/// The matcher of the neutral callee names of `options`, `*` being any run of
/// characters.
pub fn neutral_set(options: &LevelOptions) -> Result<GlobSet, globset::Error> {
    let mut set = GlobSetBuilder::new();
    for pattern in &options.neutral_calls {
        set.add(Glob::new(pattern)?);
    }
    set.build()
}

/// The problem the steps pose over the groups' `pairs`, with its counts.
fn arcs(
    groups: &Groups,
    slots: &Slots,
    options: &LevelOptions,
    (pairs, start): (&mut Pairs, &[i64]),
) -> (Problem, Stats) {
    let mut problem = Problem::new(groups.count + slots.externals.len());
    let (groups_n, externals_n) = (groups.count, slots.externals.len());
    let mut stats = Stats {
        groups: groups_n,
        externals: externals_n,
        ..Stats::default()
    };
    let mut distance = Pairs::new();
    for (id, list) in &slots.of {
        let from = groups.var(id).expect("a function of the groups");
        for slot in list {
            let named = match slot {
                Slot::Neutral => continue,
                Slot::Atom => {
                    stats.atom_steps += 1;
                    problem.slope[from] += STEP_WEIGHT;
                    continue;
                }
                Slot::Named(named) => pulled(options, named, start),
            };
            stats.named_steps += 1;
            stats.rounded_steps += usize::from(named.len() > 12);
            let each = (STEP_WEIGHT / named.len() as i64).max(1);
            for to in named {
                let (floor, into) = if to < groups.count {
                    (1, &mut *pairs)
                } else {
                    (0, &mut distance)
                };
                add(into, (from, to), floor, each);
            }
        }
    }
    stats.pairs = pairs.len();
    stats.distances = distance.len();
    let linear = |(&(a, b), &(floor, weight)): (&(usize, usize), &(i64, i64))| {
        let term = if weight == 0 {
            Term::Hard { floor }
        } else {
            Term::Linear { floor, weight }
        };
        (a, b, term)
    };
    let near = |(&(a, b), &(_, weight)): (&(usize, usize), &(i64, i64))| {
        (a, b, Term::Distance { target: 1, weight })
    };
    problem.arcs.extend(pairs.iter().map(linear));
    problem.arcs.extend(distance.iter().map(near));
    if options.externals == Externals::Fixed {
        problem.frozen[groups.count..].fill(true);
    }
    (problem, stats)
}

/// The callees of a step that pull on it.
fn pulled(options: &LevelOptions, named: &[usize], start: &[i64]) -> Vec<usize> {
    let top = named.iter().map(|&v| start[v]).max().unwrap_or(1);
    let keep = |v: &usize| options.edges == Edges::Mean || start[*v] == top;
    named.iter().copied().filter(keep).collect()
}
