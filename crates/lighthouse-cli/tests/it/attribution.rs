//! Whose change a written line is: every change a run writes belongs to one
//! fix it reports, and a preview shows what applying it writes. The fixes are
//! filters written as shell one-liners: `one` and `two` fix a line each and
//! leave a trailing space that `tidy`, the safe fix that comes last, removes.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use lighthouse_session::{
    Session,
    fix::{self, Selection, Targets},
    trust,
};

use crate::{cache_fixtures::write, checks::UserHome, mcp::Client};

/// A decision that flags the lines of `a.txt` that match `pattern`, and whose
/// fix is the `sed` script; `invalid` is an example it flags and `fixed` what
/// its fix makes of it.
fn decision(
    name: &str,
    pattern: &str,
    sed: &str,
    (priority, safety): (i32, &str),
    (invalid, fixed): (&str, &str),
) -> String {
    format!(
        r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/{name}
spec:
  title: Mend {name}
  context: A note says what the team decided.
  scope:
    subject: file
  requirement: A note MUST NOT hold {pattern}.
  severity: error
  check:
    type: command
    command: [sh, -c, "awk '/{pattern}/ {{ print FILENAME \":\" NR \": holds {name}\" }}' \"$1\"; exit 0", sh, "{{file}}"]
  fix:
    type: command
    safety: {safety}
    priority: {priority}
    command: [sed, "{sed}"]
    stdin: file
    output: stdout
  examples:
    - name: invalid
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: "{invalid}\n"
      expect:
        - line: 1
      fixed:
        - path: a.txt
          body: "{fixed}\n"
    - name: valid
      language: text
      kind: valid
      files:
        - path: a.txt
          body: "plain\n"
"#
    )
}

/// A project whose `a.txt` says `alpha` and `beta`, mended by `one` and `two`
/// (of the `safety` given) and by `tidy`, which is always safe.
fn notes(names: &[&str], safety: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let rules: String = names
        .iter()
        .chain(&["tidy"])
        .map(|name| format!("\"local/{name}\" = \"error\"\n"))
        .collect();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!("plugins = []\n[rules]\n{rules}")),
    );
    let decisions = [
        (
            "one",
            decision(
                "one",
                "alpha",
                "s/alpha/ONE /",
                (0, safety),
                ("alpha", "ONE "),
            ),
        ),
        (
            "two",
            decision("two", "beta", "s/beta/TWO /", (0, safety), ("beta", "TWO ")),
        ),
        (
            "tidy",
            decision("tidy", " $", "s/ *$//", (-100, "safe"), ("x ", "x")),
        ),
    ];
    for (name, text) in &decisions {
        if names.contains(name) || *name == "tidy" {
            write(
                dir.path(),
                &format!(".lighthouse/decisions/{name}.yaml"),
                text,
            );
        }
    }
    write(dir.path(), "a.txt", "alpha\nbeta\n");
    dir
}

fn fix(dir: &TempDir, dry_run: bool) -> Value {
    Client::trusting(dir.path())
        .tool("fix", json!({"paths": ["a.txt"], "dryRun": dry_run}))
        .unwrap()
}

fn note(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("a.txt")).unwrap()
}

/// The decisions of the applied fixes, a follow-up marked.
fn applied(fixed: &Value) -> Vec<String> {
    fixed["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let follow = if a["follow_up"] == json!(true) {
                " (follow-up)"
            } else {
                ""
            };
            format!("{}{follow}", a["decision"].as_str().unwrap())
        })
        .collect()
}

#[test]
fn the_follow_up_of_a_file_several_fixes_edited_is_reported_as_a_fix_of_its_own() {
    let dir = notes(&["one", "two"], "safe");

    let fixed = fix(&dir, false);

    assert_eq!(note(&dir), "ONE\nTWO\n", "{fixed}");
    let applied = applied(&fixed);
    assert_eq!(&applied[..2], ["local/one", "local/two"], "{fixed}");
    assert!(
        applied.len() > 2 && applied[2..].iter().all(|a| a == "local/tidy (follow-up)"),
        "what tidy wrote belongs to a fix of tidy: {fixed}"
    );
    let follow = fixed["applied"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["follow_up"] == json!(true))
        .unwrap();
    assert_eq!(follow["files"], json!(["a.txt"]), "{fixed}");
    assert_eq!(follow["safety"], "safe", "{fixed}");
}

#[test]
fn the_follow_up_of_a_file_one_fix_edited_is_part_of_that_fix() {
    let dir = notes(&["one"], "safe");

    let fixed = fix(&dir, false);

    assert_eq!(note(&dir), "ONE\nbeta\n", "{fixed}");
    assert_eq!(applied(&fixed), ["local/one"], "{fixed}");
}

#[test]
fn a_dry_run_reports_the_follow_ups_it_would_write() {
    let dir = notes(&["one", "two"], "safe");

    let fixed = fix(&dir, true);

    assert_eq!(note(&dir), "alpha\nbeta\n", "{fixed}");
    assert!(
        applied(&fixed).contains(&"local/tidy (follow-up)".to_owned()),
        "{fixed}"
    );
}

#[test]
fn a_preview_carries_the_follow_ups_of_its_own_change() {
    // Neither unsafe fix is written. Both edit the file, and each is applied
    // later by itself, so each is shown with the tidying of its own change.
    let dir = notes(&["one", "two"], "unsafe");

    let fixed = fix(&dir, false);

    assert_eq!(note(&dir), "alpha\nbeta\n", "{fixed}");
    let previews = fixed["previews"].as_array().unwrap();
    assert_eq!(previews.len(), 2, "{fixed}");
    for (decision, line) in [("local/one", "ONE"), ("local/two", "TWO")] {
        let diff = previews
            .iter()
            .find(|p| p["decision"] == decision)
            .unwrap_or_else(|| panic!("{decision} is previewed: {fixed}"))["diff"]
            .as_str()
            .unwrap();
        assert!(diff.contains(&format!("+{line}\n")), "{diff}");
        assert!(
            !diff.contains(&format!("+{line} \n")),
            "the trailing space is tidied in the preview: {diff}"
        );
        assert!(
            !diff.contains("+TWO") || line == "TWO",
            "a preview holds only its own change: {diff}"
        );
    }
}

#[test]
fn a_dry_run_shows_the_follow_up_with_the_preview_of_its_own_change() {
    let dir = notes(&["one", "two"], "safe");
    let _user = UserHome::at(&dir.path().join(".home"));
    let session = Session::find_in(dir.path()).unwrap().unwrap();
    trust(&session.root, &session.basis).unwrap();

    let fixed = fix::run(
        &session,
        &Selection {
            targets: Targets {
                paths: vec![dir.path().join("a.txt")],
                ..Targets::default()
            },
            dry_run: true,
            ..Selection::default()
        },
    )
    .unwrap();

    assert_eq!(note(&dir), "alpha\nbeta\n", "nothing was written");
    let shown = |decision: &str| {
        let fix = fixed.applied.iter().find(|a| a.decision == decision);
        fixed.shown[&fix.unwrap().fingerprint].files[0]
            .hunks
            .clone()
    };
    assert!(shown("local/one").contains("+ONE "), "{fixed:?}");
    assert!(shown("local/two").contains("+TWO "), "{fixed:?}");
    let follow = fixed.applied.iter().find(|a| a.follow_up).unwrap();
    let composed = &fixed.shown[&follow.fingerprint].files[0];
    assert_eq!(
        composed.hunks, "@@ -1,2 +1,2 @@\n-alpha\n-beta\n+ONE\n+TWO",
        "relative to the text before any fix, so that it applies alone: {fixed:?}"
    );
    assert_eq!(
        applied_to("alpha\nbeta\n", &composed.edits),
        "ONE\nTWO\n",
        "the follow-up alone on the original is the final text"
    );
    let follow_ups = fixed.applied.iter().filter(|a| a.follow_up).count();
    let count = format!(
        "{} fix(es) and {follow_ups} follow-up(s) proposed",
        fixed.applied.len() - follow_ups
    );
    assert!(
        fixed.lines().iter().any(|l| l.starts_with(&count)),
        "{:?}",
        fixed.lines()
    );
}

/// `text` with the edits of a preview made over it, the positions being lines
/// and columns of an ASCII text.
fn applied_to(text: &str, edits: &[lighthouse_model::PreviewEdit]) -> String {
    let offset = |at: lighthouse_model::Position| {
        let line_start: usize = text
            .split_inclusive('\n')
            .take(at.line as usize - 1)
            .map(str::len)
            .sum();
        line_start + at.col as usize - 1
    };
    let mut made = text.to_owned();
    let mut ordered: Vec<_> = edits.iter().collect();
    ordered.sort_by_key(|e| std::cmp::Reverse(offset(e.start)));
    for edit in ordered {
        made.replace_range(offset(edit.start)..offset(edit.end), &edit.text);
    }
    made
}
