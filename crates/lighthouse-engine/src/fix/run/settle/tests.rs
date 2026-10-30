use std::collections::BTreeMap;

use lighthouse_model::{Fingerprint, Safety};

use super::*;
use crate::fix::propose::Aim;

fn proposal(file: &str, edits: Vec<TextEdit>) -> Proposal {
    Proposal {
        fingerprint: Fingerprint::new("r", file, "k"),
        rule_id: "r".to_owned(),
        file: file.into(),
        line: 1,
        fixer: FixedBy::Fixer("tidy".to_owned()),
        description: "tidy".to_owned(),
        safety: Safety::Safe,
        priority: 0,
        edits,
        files: vec![file.into()],
        aim: Aim::Finding,
        chain: 0,
        bases: BTreeMap::new(),
        batch: Vec::new(),
    }
}

fn replace(file: &str, range: std::ops::Range<usize>, text: &str) -> TextEdit {
    TextEdit {
        file: file.into(),
        range,
        text: text.to_owned(),
    }
}

#[test]
fn a_follow_up_with_nothing_to_report_in_a_shared_file_is_not_applied() {
    // Both `a.txt` and `b.txt` are edited by two fixes; `c.txt` by one.
    let edits = |file: &str| proposal(file, vec![replace(file, 0..1, "x")]);
    let fixes = [
        edits("a.txt"),
        edits("a.txt"),
        edits("b.txt"),
        edits("b.txt"),
    ];
    let standing = |_: &Path| Some("one\n".to_owned());
    let safe = vec![
        // Changes a shared file.
        proposal("a.txt", vec![replace("a.txt", 0..3, "ONE")]),
        // Changes it to what it is: nothing to report.
        proposal("a.txt", vec![replace("a.txt", 0..3, "one")]),
        // Its edit does not apply to the text that stands.
        proposal("b.txt", vec![replace("b.txt", 40..43, "x")]),
        // Changes a file one fix edited: part of that fix.
        proposal("c.txt", vec![replace("c.txt", 40..43, "x")]),
    ];

    let kept = Orchestrator::reportable(safe, &fixes, standing);

    let files: Vec<&Path> = kept.iter().map(|p| p.file.as_path()).collect();
    assert_eq!(files, [Path::new("a.txt"), Path::new("c.txt")]);
    assert_eq!(kept[0].edits[0].text, "ONE");
}
