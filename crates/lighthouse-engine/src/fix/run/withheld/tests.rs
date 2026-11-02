use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::{Fingerprint, Safety};

use super::*;
use crate::fix::{FixedBy, edits::TextEdit, propose::Aim};

fn fix(key: &str, file: &str, range: std::ops::Range<usize>) -> Proposal {
    Proposal {
        fingerprint: Fingerprint::new("r", file, key),
        rule_id: "r".to_owned(),
        file: file.into(),
        line: 1,
        fixer: FixedBy::Fixer("tidy".to_owned()),
        description: "tidy".to_owned(),
        safety: Safety::Unsafe,
        priority: 0,
        edits: vec![TextEdit::delete(PathBuf::from(file), range)],
        files: vec![file.into()],
        aim: Aim::Finding,
        chain: 0,
        bases: BTreeMap::new(),
        batch: Vec::new(),
        moves_only: false,
    }
}

#[test]
fn fixes_of_one_file_that_do_not_overlap_are_one_group() {
    let groups = layers(vec![fix("1", "a.go", 0..4), fix("2", "a.go", 8..12)]);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);
}

#[test]
fn fixes_that_overlap_are_groups_of_their_own() {
    let groups = layers(vec![
        fix("1", "a.go", 0..8),
        fix("2", "a.go", 4..12),
        fix("3", "b.go", 0..4),
    ]);

    assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
}

#[test]
fn a_group_shares_no_file_when_each_file_has_one_fix() {
    assert!(files_unshared(&[
        fix("1", "a.go", 0..4),
        fix("2", "b.go", 0..4)
    ]));
    assert!(!files_unshared(&[
        fix("1", "a.go", 0..4),
        fix("2", "a.go", 8..12)
    ]));
}

#[test]
fn the_listing_runs_the_batches_of_one_fix_per_file_one_after_another() {
    let (first, second, third) = (
        fix("1", "a.go", 0..4),
        fix("2", "a.go", 8..12),
        fix("3", "b.go", 0..4),
    );

    let order = listing(&[first.clone(), second.clone(), third.clone()]);

    assert_eq!(
        order,
        [first.fingerprint, third.fingerprint, second.fingerprint]
    );
}
