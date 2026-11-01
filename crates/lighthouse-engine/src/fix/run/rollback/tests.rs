use std::collections::BTreeMap;

use lighthouse_model::{Fingerprint, Safety};

use super::*;
use crate::fix::propose::Aim;

const TEXT: &str = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";

fn moving(key: &str, edits: Vec<TextEdit>) -> Proposal {
    Proposal {
        fingerprint: Fingerprint::new("r", "f.rs", key),
        rule_id: "r".to_owned(),
        file: "f.rs".into(),
        line: 1,
        fixer: FixedBy::Fixer("tidy".to_owned()),
        description: "tidy".to_owned(),
        safety: Safety::Safe,
        priority: 0,
        edits,
        files: vec!["f.rs".into()],
        aim: Aim::Finding,
        chain: 0,
        bases: BTreeMap::new(),
        batch: Vec::new(),
        moves_only: true,
    }
}

/// `c` taken out, and `c` put before `a`: a move that is valid on its own.
fn c_before_a() -> Vec<TextEdit> {
    vec![
        TextEdit::delete("f.rs".into(), 21..32),
        TextEdit::insert("f.rs".into(), 0, "fn c() {}\n\n".to_owned()),
    ]
}

#[test]
fn proposals_that_each_keep_the_declarations_and_together_repeat_one_are_found() {
    // The second only puts `c` before `b`: a copy, since the first took it
    // out of a range the second does not touch.
    let first = moving("1", c_before_a());
    let second = moving(
        "2",
        vec![TextEdit::insert(
            "f.rs".into(),
            11,
            "fn c() {}\n\n".to_owned(),
        )],
    );
    let text = |_: &Path| Some(TEXT.to_owned());
    assert!(scattering(std::slice::from_ref(&first), text).is_empty());
    let found = scattering(&[first, second], text);
    assert_eq!(
        found[Path::new("f.rs")],
        "the reorder would not keep the file's declarations: it repeats `fn c() {}` 1 time(s)"
    );
}

#[test]
fn a_file_another_kind_of_fix_edits_is_not_judged() {
    let first = moving("1", c_before_a());
    let mut other = moving(
        "2",
        vec![TextEdit::insert(
            "f.rs".into(),
            11,
            "fn c() {}\n\n".to_owned(),
        )],
    );
    other.moves_only = false;
    assert!(scattering(&[first, other], |_| Some(TEXT.to_owned())).is_empty());
}
