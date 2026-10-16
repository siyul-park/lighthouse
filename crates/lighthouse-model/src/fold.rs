//! The folds of a cross-validation: subjects of one group, one file, are
//! never split between the part a model is fitted on and the part it is
//! judged on. The trainer of a learned model and the host that grades it on
//! the labels it learned from assign folds by this one rule.

use std::collections::BTreeSet;

use crate::hash::short;

/// The number of folds, when there are groups enough.
pub const FOLDS: usize = 5;

/// The fold of a group: the first 8 hex digits of its SHA-256, modulo
/// `folds`.
pub fn fold(group: &str, folds: usize) -> usize {
    u32::from_str_radix(&short(group, 4), 16).unwrap_or(0) as usize % folds
}

/// The fold of each of `groups`, in order: by [`fold`] into [`FOLDS`] folds
/// and, when there are fewer distinct groups than that, one fold a group
/// (numbered by the order of the groups).
pub fn assign(groups: &[&str]) -> Vec<usize> {
    let distinct: BTreeSet<&str> = groups.iter().copied().collect();
    if distinct.len() >= FOLDS {
        return groups.iter().map(|group| fold(group, FOLDS)).collect();
    }
    groups
        .iter()
        .map(|group| distinct.iter().position(|d| d == group).unwrap_or(0))
        .collect()
}
