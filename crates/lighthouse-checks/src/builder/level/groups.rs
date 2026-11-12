//! The variables the project's functions share: functions that call one
//! another are one group and share a level, and the hard differences between
//! groups.

use std::collections::BTreeMap;

use lighthouse_model::{
    graph::Graph,
    ucm::{Model, SymbolId, SymbolKind},
};

use crate::builder::height::implementations;

/// What a step weighs in total, split over its callees; the least common
/// multiple of 1 to 12, so the split is exact up to 12 callees.
pub(super) const STEP_WEIGHT: i64 = 27_720;

/// A pair of variables with the floor of their difference and the weight of
/// the cost on it.
pub(super) type Pairs = BTreeMap<(usize, usize), (i64, i64)>;

/// The functions and methods of a project, each in the group of the functions
/// it shares a cycle of calls with.
pub(super) struct Groups {
    pub(super) ids: Vec<SymbolId>,
    group: Vec<usize>,
    pub(super) count: usize,
}

impl Groups {
    pub(super) fn of(project: &dyn Model) -> Self {
        let mut ids: Vec<SymbolId> = project
            .symbols()
            .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
            .map(|s| s.id.clone())
            .collect();
        ids.sort();
        let edges: Vec<(u32, u32)> = ids
            .iter()
            .enumerate()
            .flat_map(|(from, id)| {
                targets(project, id)
                    .into_iter()
                    .filter_map(|to| ids.binary_search(&to).ok())
                    .map(move |to| (from as u32, to as u32))
            })
            .collect();
        let (group, count) = Graph::new(ids.len(), edges).components();
        Self {
            ids,
            group: group.into_iter().map(|g| g as usize).collect(),
            count,
        }
    }

    /// The variable of the group of a function.
    pub(super) fn var(&self, id: &SymbolId) -> Option<usize> {
        self.ids.binary_search(id).ok().map(|at| self.group[at])
    }

    /// The differences the code forces: a function is one level above what it
    /// calls, and an abstract method is as high as each implementation it
    /// stands for, at a cost for each level above the highest.
    pub(super) fn pairs(&self, project: &dyn Model) -> Pairs {
        let mut pairs = Pairs::new();
        for (at, id) in self.ids.iter().enumerate() {
            let from = self.group[at];
            if let Some(found) = implementations(project, id) {
                let each = (STEP_WEIGHT / found.len().max(1) as i64).max(1);
                for to in found.iter().filter_map(|i| self.var(i)) {
                    add(&mut pairs, (from, to), 0, each);
                }
                continue;
            }
            for to in project.callees(id).iter().filter_map(|c| self.var(c)) {
                add(&mut pairs, (from, to), 1, 0);
            }
        }
        pairs
    }
}

/// Adds `weight` to the pair, keeping the highest floor; a pair of one
/// variable is no constraint.
pub(super) fn add(pairs: &mut Pairs, (a, b): (usize, usize), floor: i64, weight: i64) {
    if a == b {
        return;
    }
    let held = pairs.entry((a, b)).or_insert((floor, 0));
    held.0 = held.0.max(floor);
    held.1 += weight;
}

/// What a function leads to: its callees, or the implementations of an
/// abstract method.
fn targets(project: &dyn Model, id: &SymbolId) -> Vec<SymbolId> {
    implementations(project, id).unwrap_or_else(|| project.callees(id).to_vec())
}
