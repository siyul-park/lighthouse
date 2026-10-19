//! Whose change a written line is: every change a run writes belongs to one
//! fix it reports, and a preview shows what applying it writes. The fixes are
//! filters written as shell one-liners: `one` and `two` fix a line each and
//! leave a trailing space that `tidy`, the safe fix that comes last, removes.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{cache_fixtures::write, mcp::Client};

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
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"local\"]\n[rules]\n{rules}"
        )),
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
