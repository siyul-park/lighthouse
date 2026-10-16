//! The statistics a learned model and the host that grades it share: the
//! folds of a cross-validation and the Wilson interval of a proportion. They
//! live in L0 because plugins cannot import the crates that grade.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

/// The number of folds, when there are groups enough.
pub const FOLDS: usize = 5;

/// z for a 95% interval.
const Z: f64 = 1.959_963_984_540_054;

/// The fold of a group: the first 4 bytes of its SHA-256, modulo `folds`.
/// Subjects of one group, one file, are never split between the part a model
/// is fitted on and the part it is judged on.
pub fn fold(group: &str, folds: usize) -> usize {
    let digest = Sha256::digest(group.as_bytes());
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize % folds
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

/// The 95% Wilson score interval of `hits` out of `of` (weights are fine:
/// they need not be whole numbers); `of` must be above zero.
pub fn wilson(hits: f64, of: f64) -> (f64, f64) {
    let p = hits / of;
    let z2 = Z * Z;
    let centre = (p + z2 / (2.0 * of)) / (1.0 + z2 / of);
    let half = Z * (p * (1.0 - p) / of + z2 / (4.0 * of * of)).sqrt() / (1.0 + z2 / of);
    ((centre - half).max(0.0), (centre + half).min(1.0))
}
