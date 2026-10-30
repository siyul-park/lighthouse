use std::collections::BTreeMap;

use lighthouse_model::{Fingerprint, Safety};

use super::*;
use crate::fix::{
    FixedBy,
    edits::TextEdit,
    propose::{Aim, Proposal},
};

fn proposal(edits: Vec<TextEdit>) -> Proposal {
    Proposal {
        fingerprint: Fingerprint::new("r", "a.txt", "k"),
        rule_id: "r".to_owned(),
        file: "a.txt".into(),
        line: 1,
        fixer: FixedBy::Fixer("tidy".to_owned()),
        description: "tidy".to_owned(),
        safety: Safety::Safe,
        priority: 0,
        edits,
        files: vec!["a.txt".into()],
        aim: Aim::Finding,
        chain: 0,
        bases: BTreeMap::new(),
        batch: Vec::new(),
    }
}

fn replace(range: std::ops::Range<usize>, text: &str) -> TextEdit {
    TextEdit {
        file: "a.txt".into(),
        range,
        text: text.to_owned(),
    }
}

#[test]
fn a_fix_that_changes_nothing_has_no_preview_and_one_that_changes_the_text_has() {
    let base = |_: &Path| Some("one \n".to_owned());

    assert!(preview_of(&proposal(vec![replace(0..3, "one")]), base).is_none());
    let tidy = preview_of(&proposal(vec![replace(0..4, "one")]), base).unwrap();
    assert_eq!(tidy.files[0].hunks, "@@ -1 +1 @@\n-one \n+one");
}
