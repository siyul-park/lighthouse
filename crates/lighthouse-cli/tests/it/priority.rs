//! The order fixes are taken in when they overlap: the higher `priority`
//! first, a tie by the order of the findings; and the formatter, which has the
//! lowest priority, that follows the fix that won. The fixes are filters
//! written as shell one-liners, so no real tool but `rustfmt` is run.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{cache_fixtures::write, mcp::Client};

/// A decision that flags `a.txt` while it says `start`, and whose fix writes
/// `letter` in its place. `priority` is a line of the fix, or empty.
fn decision(name: &str, letter: &str, priority: &str, safety: &str) -> String {
    format!(
        r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/{name}
spec:
  title: Say {letter}
  context: A note says what the team decided.
  scope:
    subject: file
  requirement: A note MUST NOT say start.
  severity: error
  check:
    type: command
    command: [sh, -c, "grep -q start \"$1\" && echo \"$1:1: says start\"; exit 0", sh, "{{file}}"]
  fix:
    type: command
    safety: {safety}
{priority}    command: [sed, "s/start/{letter}/"]
    stdin: file
    output: stdout
  examples:
    - name: start
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: "start\n"
      expect:
        - line: 1
      fixed:
        - path: a.txt
          body: "{letter}\n"
    - name: done
      language: text
      kind: valid
      files:
        - path: a.txt
          body: "done\n"
"#
    )
}

/// A project whose note is flagged by the decisions `(name, letter, priority)`,
/// all with a safe fix.
fn notes(decisions: &[(&str, &str, i32)]) -> TempDir {
    let decisions: Vec<_> = decisions
        .iter()
        .map(|(name, letter, priority)| (*name, *letter, *priority, "safe"))
        .collect();
    notes_of(&decisions, "")
}

/// The same with the safety of each fix, and `fix` among the lines of `[fix]`.
fn notes_of(decisions: &[(&str, &str, i32, &str)], fix: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let rules: String = decisions
        .iter()
        .map(|(name, ..)| format!("\"local/{name}\" = \"error\"\n"))
        .collect();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"local\"]\n[rules]\n{rules}[fix]\n{fix}"
        )),
    );
    for (name, letter, priority, safety) in decisions {
        let line = if *priority == 0 {
            String::new()
        } else {
            format!("    priority: {priority}\n")
        };
        write(
            dir.path(),
            &format!(".lighthouse/decisions/{name}.yaml"),
            &decision(name, letter, &line, safety),
        );
    }
    write(dir.path(), "a.txt", "start\n");
    dir
}

fn fix(dir: &TempDir, path: &str) -> Value {
    Client::trusting(dir.path())
        .tool("fix", json!({"paths": [path]}))
        .unwrap()
}

fn note(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("a.txt")).unwrap()
}

#[test]
fn fixes_of_equal_priority_that_overlap_are_taken_in_the_order_of_the_findings() {
    for decisions in [
        [("first", "A", 0), ("second", "B", 0)],
        [("second", "B", 0), ("first", "A", 0)],
    ] {
        let dir = notes(&decisions);

        let fixed = fix(&dir, "a.txt");

        assert_eq!(note(&dir), "A\n", "{fixed}");
    }
}

#[test]
fn the_fix_of_the_higher_priority_is_taken_first_whatever_the_order_of_the_findings() {
    let dir = notes(&[("first", "A", 0), ("second", "B", 10)]);

    let fixed = fix(&dir, "a.txt");

    assert_eq!(note(&dir), "B\n", "{fixed}");

    let dir = notes(&[("first", "A", -5), ("second", "B", 0)]);

    let fixed = fix(&dir, "a.txt");

    assert_eq!(note(&dir), "B\n", "a negative priority comes last: {fixed}");
}

#[test]
fn a_fix_that_is_only_previewed_takes_no_span_from_one_that_is_written() {
    // The content fix is unsafe and has the higher priority, but the run may
    // not write it: the safe fix that overlaps it is written, and the preview
    // is made over what it left.
    let dir = notes_of(
        &[("content", "A", 0, "unsafe"), ("format", "B", -100, "safe")],
        "",
    );

    let fixed = fix(&dir, "a.txt");

    assert_eq!(note(&dir), "B\n", "{fixed}");
    assert_eq!(fixed["applied"][0]["decision"], "local/format", "{fixed}");
}

#[test]
fn once_both_are_written_priority_alone_orders_them() {
    let dir = notes_of(
        &[("content", "A", 0, "unsafe"), ("format", "B", -100, "safe")],
        "unsafe = true\n",
    );

    let fixed = fix(&dir, "a.txt");

    assert_eq!(
        note(&dir),
        "A\n",
        "the safe fix is not first for being safe: {fixed}"
    );
}

#[test]
fn a_fix_that_overlaps_the_formatter_wins_and_the_formatting_follows_it() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"core\", \"local\", \"rust\"]\n[rules]\n\"rust/formatted\" = \"error\"\n\"local/rename\" = \"error\"\n",
            plugin.to_str().unwrap()
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    let rename = decision("rename", "x", "", "safe")
        .replace("grep -q start", "grep -q bad_one")
        .replace("s/start/x/", "s/bad_one/good_one/")
        .replace("language: text", "language: rust")
        .replace("a.txt", "src/lib.rs")
        .replace("says start", "says bad_one")
        .replace("start\\n", "pub fn bad_one() {}\\n")
        .replace("\"x\\n\"", "\"pub fn good_one() {}\\n\"")
        .replace("\"done\\n\"", "\"pub fn done() {}\\n\"");
    write(dir.path(), ".lighthouse/decisions/rename.yaml", &rename);
    // Both decisions flag this file, and both fixes rewrite it whole.
    write(dir.path(), "src/lib.rs", "pub fn   bad_one( )  {  }\n");

    let fixed = fix(&dir, "src/lib.rs");

    let text = fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert_eq!(text, "pub fn good_one() {}\n", "{fixed}");
    // The content fix wins the overlap; the formatter, which lost it, is
    // taken in the round after.
    let applied: Vec<&Value> = fixed["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["decision"])
        .collect();
    assert_eq!(applied, ["local/rename", "rust/formatted"], "{fixed}");
}
