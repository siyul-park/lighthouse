//! `decision ask`: what an edit would do, through the binary. The answer to a
//! what-if is the difference of two checks, so the ground truth is a check of
//! a copy of the project with the edit applied.

use std::{collections::BTreeSet, fs, path::Path, process::Output};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

use crate::checks::{home, trust};
use crate::review::{HELPER, rust_project};

const DOCUMENTED: &str = "/// Runs.\npub fn run(x: u8) -> u8 {\n    x + 1\n}\n";
const BROKEN: &str = "pub fn run( {\n";

/// Six lines, over the three the project allows.
const BIG: &str = "a\nb\nc\nd\ne\nf\n";
const SMALL: &str = "a\n";

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir)
        .envs(home(dir))
        .env_remove("LIGHTHOUSE_TRUST");
    cmd
}

/// A text project that fails files over three lines at `level`, with `big.txt`
/// failing and `small.txt` not.
fn project(level: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = []\n[rules]\n\"core/max-lines\" = {{ level = \"{level}\", options = {{ max = 3 }} }}\n"
        )),
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), BIG).unwrap();
    fs::write(dir.path().join("small.txt"), SMALL).unwrap();
    dir
}

/// The git-style diff from `old` to `new` for `path`, as one hunk with a line
/// of context on each side; `None` is a file that does not exist.
pub(crate) fn diff(path: &str, old: Option<&str>, new: &str) -> String {
    let (before, after): (Vec<&str>, Vec<&str>) =
        (old.unwrap_or("").lines().collect(), new.lines().collect());
    let shared = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let rest = before.len().min(after.len()) - shared;
    let tail = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(rest)
        .take_while(|(a, b)| a == b)
        .count();
    let (above, below) = (shared.min(1), tail.min(1));
    let (gone, added) = (
        &before[shared..before.len() - tail],
        &after[shared..after.len() - tail],
    );
    let (old_count, new_count) = (above + gone.len() + below, above + added.len() + below);
    let start = |count: usize| {
        if count == 0 {
            shared
        } else {
            shared - above + 1
        }
    };
    let from = old.map_or("/dev/null".to_owned(), |_| format!("a/{path}"));
    let mut out = format!("--- {from}\n+++ b/{path}\n");
    out += &format!(
        "@@ -{},{old_count} +{},{new_count} @@\n",
        start(old_count),
        start(new_count)
    );
    for line in &before[shared - above..shared] {
        out += &format!(" {line}\n");
    }
    for line in gone {
        out += &format!("-{line}\n");
    }
    for line in added {
        out += &format!("+{line}\n");
    }
    for line in &before[before.len() - tail..before.len() - tail + below] {
        out += &format!(" {line}\n");
    }
    out
}

fn ask(dir: &Path, patch: &str, extra: &[&str]) -> Output {
    lighthouse(dir)
        .args(["decision", "ask", "--edit", "-"])
        .args(extra)
        .write_stdin(patch.to_owned())
        .output()
        .unwrap()
}

/// The answer in JSON, and the exit code.
fn answered(dir: &Path, patch: &str, extra: &[&str]) -> (Value, i32) {
    let out = ask(dir, patch, &[extra, &["--format", "json"]].concat());
    let text = String::from_utf8(out.stdout).unwrap();
    let json = serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!(
            "{e}: {text} {}\n{patch}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (json, out.status.code().unwrap())
}

fn fingerprints(delta: &Value) -> BTreeSet<String> {
    delta
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["fingerprint"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn an_edit_that_makes_a_file_too_long_is_forbidden() {
    let dir = project("error");
    let (answer, code) = answered(
        dir.path(),
        &diff("small.txt", Some(SMALL), BIG),
        &["--detail", "full"],
    );
    assert_eq!(code, 1);
    assert_eq!(answer["kind"], "whatIf");
    assert_eq!(answer["verdict"], "forbidden");
    assert_eq!(answer["basis"], "evaluated");
    assert_eq!(answer["fired"].as_array().unwrap().len(), 1);
    assert_eq!(answer["fired"][0]["path"], "small.txt");
    assert_eq!(answer["fired"][0]["decision"], "core/max-lines");
    assert_eq!(answer["fired"][0]["severity"], "error");
    assert_eq!(answer["cleared"], serde_json::json!([]));
    assert_eq!(answer["premises"][0]["decision"], "core/max-lines");
    // `big.txt` fails before and after, but is not what the edit touches.
    assert_eq!(answer["unchanged"], 0);
    // Nothing is written, nothing is remembered.
    assert_eq!(
        fs::read_to_string(dir.path().join("small.txt")).unwrap(),
        SMALL
    );
    assert!(
        !dir.path()
            .join(".lighthouse")
            .join("lighthouse.db")
            .exists()
    );
}

#[test]
fn an_edit_that_shortens_the_failing_file_is_permitted_and_clears_its_finding() {
    let dir = project("error");
    let (answer, code) = answered(
        dir.path(),
        &diff("big.txt", Some(BIG), SMALL),
        &["--detail", "full"],
    );
    assert_eq!(code, 0);
    assert_eq!(answer["verdict"], "permitted");
    assert_eq!(answer["fired"], serde_json::json!([]));
    assert_eq!(answer["cleared"].as_array().unwrap().len(), 1);
    assert_eq!(answer["cleared"][0]["path"], "big.txt");
}

#[test]
fn a_finding_at_info_does_not_forbid() {
    let dir = project("info");
    let (answer, code) = answered(
        dir.path(),
        &diff("small.txt", Some(SMALL), BIG),
        &["--detail", "full"],
    );
    assert_eq!((code, answer["verdict"].as_str()), (0, Some("permitted")));
    assert_eq!(answer["fired"].as_array().unwrap().len(), 1);
}

#[test]
fn paths_widen_the_files_the_answer_is_about() {
    let dir = project("error");
    let patch = diff("small.txt", Some(SMALL), "a\nb\n");
    let (alone, _) = answered(dir.path(), &patch, &[]);
    assert_eq!(alone["unchanged"], 0);
    let (wider, _) = answered(dir.path(), &patch, &["big.txt"]);
    assert_eq!(wider["unchanged"], 1);
    let outside = ask(dir.path(), &patch, &["../elsewhere"]);
    assert_eq!(outside.status.code(), Some(2));
}

#[test]
fn compact_groups_the_findings_by_decision() {
    let dir = project("error");
    let mut patch = diff("small.txt", Some(SMALL), BIG);
    patch += &diff("new.txt", None, BIG);
    let (answer, code) = answered(dir.path(), &patch, &[]);
    assert_eq!(code, 1);
    let group = &answer["fired"][0];
    assert_eq!(group["decision"], "core/max-lines");
    assert_eq!(group["severity"], "error");
    let rows = group["files"].as_object().unwrap();
    assert_eq!(
        rows.keys().map(String::as_str).collect::<Vec<_>>(),
        ["new.txt", "small.txt"]
    );
    let row = &rows["small.txt"][0];
    assert_eq!(row[0], 4);
    assert_eq!(row[3].as_str().unwrap().len(), 7);
}

#[test]
fn the_text_form_is_one_line_per_finding() {
    let dir = project("error");
    let out = ask(dir.path(), &diff("small.txt", Some(SMALL), BIG), &[]);
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "forbidden");
    assert!(
        lines[1].starts_with("fired small.txt:4:1: error core/max-lines: "),
        "{text}"
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn the_edit_comes_from_a_file_too() {
    let dir = project("error");
    fs::write(
        dir.path().join("edit.patch"),
        diff("small.txt", Some(SMALL), BIG),
    )
    .unwrap();
    let out = lighthouse(dir.path())
        .args([
            "decision",
            "ask",
            "--edit",
            "edit.patch",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let missing = lighthouse(dir.path())
        .args(["decision", "ask", "--edit", "nope.patch"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
}

#[test]
fn an_edit_that_cannot_be_applied_is_a_usage_error_that_names_the_file() {
    let dir = project("error");
    let stale = diff("big.txt", Some("x\ny\n"), "x\nz\n");
    let refused = [
        (stale.as_str(), "big.txt"),
        (
            "diff --git a/a.txt b/b.txt\nsimilarity index 100%\nrename from a.txt\nrename to b.txt\n",
            "rename",
        ),
        (
            "diff --git a/small.txt b/small.txt\ndeleted file mode 100644\n--- a/small.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\n",
            "deletion",
        ),
        (
            "diff --git a/small.txt b/small.txt\nold mode 100644\nnew mode 100755\n",
            "mode",
        ),
        ("Binary files a/x.bin and b/x.bin differ\n", "binary"),
        (
            "--- a/lighthouse.toml\n+++ b/lighthouse.toml\n@@ -1 +1 @@\n-x\n+y\n",
            "lighthouse.toml",
        ),
        (
            "--- /dev/null\n+++ b/.lighthouse/decisions/x.yaml\n@@ -0,0 +1 @@\n+x\n",
            ".lighthouse/decisions/x.yaml",
        ),
        (
            "--- /dev/null\n+++ b/../outside.txt\n@@ -0,0 +1 @@\n+x\n",
            "not a path under the project",
        ),
        (
            "--- /dev/null\n+++ b/small.txt\n@@ -0,0 +1 @@\n+x\n",
            "a new file is one that does not exist",
        ),
        (
            "--- /dev/null\n+++ b/nowhere/new.txt\n@@ -0,0 +1 @@\n+x\n",
            "in a directory the project has",
        ),
        (
            "--- a/absent.txt\n+++ b/absent.txt\n@@ -1 +1 @@\n-x\n+y\n",
            "absent.txt",
        ),
        ("", "no file"),
        (
            "diff --git a/empty.txt b/empty.txt\nnew file mode 100644\nindex 0000000..e69de29\n",
            "empty new file",
        ),
    ];
    for (patch, says) in refused {
        let out = ask(dir.path(), patch, &[]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{patch}: {err}");
        assert!(err.contains(says), "{patch}: {err}");
    }
    assert_eq!(fs::read_to_string(dir.path().join("big.txt")).unwrap(), BIG);
}

#[test]
fn what_comes_before_the_first_file_of_a_mail_is_not_a_file() {
    let dir = project("error");
    let edit = diff("small.txt", Some(SMALL), "a\nb\nc\nd\n");
    let mail = format!(
        "From 1a2b3c Mon Sep 17 00:00:00 2001\nSubject: [PATCH] lengthen\n\nA note.\n---\n small.txt | 3 +++\n 1 file changed, 3 insertions(+)\n\n{edit}-- \n2.50.0\n"
    );
    let (plain, plain_code) = answered(dir.path(), &edit, &[]);
    let (mailed, mailed_code) = answered(dir.path(), &mail, &[]);
    assert_eq!(mailed_code, plain_code, "{mailed}");
    assert_eq!(mailed["fired"], plain["fired"]);
}

#[test]
fn a_line_that_only_moves_is_neither_fired_nor_cleared() {
    let dir = project("error");
    // The finding of `big.txt` is the file's, wherever its lines are.
    let moved = diff("big.txt", Some(BIG), &format!("{BIG}g\n"));
    let (answer, code) = answered(dir.path(), &moved, &["--detail", "full"]);
    assert_eq!(code, 0, "{answer}");
    assert_eq!(answer["fired"], serde_json::json!([]));
    assert_eq!(answer["cleared"], serde_json::json!([]));
    assert_eq!(answer["unchanged"], 1);
}

/// A deterministic stream of numbers, so that a failing case can be rerun.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % below
    }

    fn text(&mut self) -> String {
        let lines = self.next(7);
        (0..lines)
            .map(|_| format!("{}\n", ["a", "b", "c"][self.next(3) as usize]))
            .collect()
    }
}

/// The findings of a check of `dir` in the files `scope` names, by fingerprint.
fn found(dir: &Path, scope: &BTreeSet<String>) -> BTreeSet<String> {
    let out = lighthouse(dir)
        .args([
            "check",
            ".",
            "--no-store",
            "--format",
            "json",
            "--detail",
            "full",
        ])
        .args(["--limit", "100000", "--no-serve"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| scope.contains(f["location"]["path"].as_str().unwrap()))
        .map(|f| f["fingerprint"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn what_if_is_a_check_of_the_patched_copy_diffed_against_a_check_of_the_original() {
    let mut random = Lcg(0x5eed);
    for case in 0..24 {
        let dir = project("error");
        let copy = project("error");
        let mut patch = String::new();
        let mut scope = BTreeSet::new();
        let mut files = [
            ("big.txt", Some(BIG)),
            ("small.txt", Some(SMALL)),
            ("new.txt", None),
        ];
        // Each file is edited with the odds of one in two, and one at least.
        for (at, (path, old)) in files.iter_mut().enumerate() {
            if random.next(2) == 0 && !(at == 2 && patch.is_empty()) {
                continue;
            }
            let new = random.text();
            if old.unwrap_or("") == new {
                continue;
            }
            patch += &diff(path, *old, &new);
            fs::write(copy.path().join(*path), &new).unwrap();
            scope.insert((*path).to_owned());
        }
        if patch.is_empty() {
            continue;
        }
        let before = found(dir.path(), &scope);
        let after = found(copy.path(), &scope);
        let (answer, code) = answered(dir.path(), &patch, &["--detail", "full"]);
        let fired: BTreeSet<_> = after.difference(&before).cloned().collect();
        let cleared: BTreeSet<_> = before.difference(&after).cloned().collect();
        assert_eq!(
            fingerprints(&answer["fired"]),
            fired,
            "case {case}:\n{patch}"
        );
        assert_eq!(
            fingerprints(&answer["cleared"]),
            cleared,
            "case {case}:\n{patch}"
        );
        assert_eq!(
            answer["unchanged"].as_u64().unwrap() as usize,
            before.intersection(&after).count(),
            "case {case}"
        );
        assert_eq!(code, i32::from(!fired.is_empty()), "case {case}");
    }
}

#[test]
fn a_line_added_above_a_finding_neither_fires_nor_clears_it() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let moved = format!("// the helper\n{HELPER}");
    let (answer, code) = answered(
        dir.path(),
        &diff("src/lib.rs", Some(HELPER), &moved),
        &["--detail", "full"],
    );
    assert_eq!(code, 0, "{answer}");
    assert_eq!(answer["fired"], serde_json::json!([]), "{answer}");
    assert_eq!(answer["cleared"], serde_json::json!([]), "{answer}");
    assert!(answer["unchanged"].as_u64().unwrap() >= 1);
}

#[test]
fn an_edit_the_language_provider_cannot_parse_leaves_the_answer_unknown() {
    let dir = rust_project(&[("src/lib.rs", DOCUMENTED)]);
    let (answer, code) = answered(
        dir.path(),
        &diff("src/lib.rs", Some(DOCUMENTED), BROKEN),
        &[],
    );
    assert_eq!(
        (code, answer["verdict"].as_str()),
        (3, Some("unknown")),
        "{answer}"
    );
    assert_eq!(answer["incomplete"][0]["path"], "src/lib.rs");
}

#[test]
fn a_file_that_could_not_be_parsed_before_the_edit_does_not_make_a_clean_edit_unknown() {
    let dir = rust_project(&[("src/lib.rs", BROKEN)]);
    let (answer, code) = answered(
        dir.path(),
        &diff("src/lib.rs", Some(BROKEN), DOCUMENTED),
        &[],
    );
    assert_eq!(
        (code, answer["verdict"].as_str()),
        (0, Some("permitted")),
        "{answer}"
    );
}

#[test]
fn a_finding_that_fires_is_forbidden_although_another_file_is_incomplete() {
    let root = format!("mod broken;\n{DOCUMENTED}");
    let dir = rust_project(&[("src/lib.rs", &root), ("src/broken.rs", BROKEN)]);
    let undocumented = format!("{root}pub fn bare() {{}}\n");
    let (answer, code) = answered(
        dir.path(),
        &diff("src/lib.rs", Some(&root), &undocumented),
        &["--detail", "full"],
    );
    assert_eq!(
        (code, answer["verdict"].as_str()),
        (1, Some("forbidden")),
        "{answer}"
    );
    assert_eq!(answer["fired"][0]["decision"], "design/exported-doc");
    let gaps = answer["incomplete"].as_array().unwrap();
    assert!(
        gaps.iter().any(|gap| gap["path"] == "src/broken.rs"),
        "{answer}"
    );
}

#[test]
fn a_project_that_is_not_trusted_is_not_asked() {
    let dir = rust_project(&[("src/lib.rs", DOCUMENTED)]);
    let untrusted = tempfile::tempdir().unwrap();
    for entry in ["lighthouse.toml", "Cargo.toml"] {
        fs::copy(dir.path().join(entry), untrusted.path().join(entry)).unwrap();
    }
    fs::create_dir(untrusted.path().join("src")).unwrap();
    fs::write(untrusted.path().join("src/lib.rs"), DOCUMENTED).unwrap();
    let out = ask(
        untrusted.path(),
        &diff("src/lib.rs", Some(DOCUMENTED), "pub fn run() {}\n"),
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("lighthouse trust"), "{err}");
    trust(untrusted.path());
    let trusted = ask(
        untrusted.path(),
        &diff("src/lib.rs", Some(DOCUMENTED), "pub fn run() {}\n"),
        &[],
    );
    assert_ne!(trusted.status.code(), Some(2));
}

#[test]
fn a_question_leaves_the_next_check_as_a_plain_check_would_find_it() {
    let dir = project("error");
    let check = || {
        let out = lighthouse(dir.path())
            .args(["check", "--no-store", "--format", "json", "--limit", "100"])
            .output()
            .unwrap();
        (out.stdout, out.status.code())
    };
    let plain = check();
    let patch = diff("big.txt", Some(BIG), SMALL) + &diff("new.txt", None, BIG);
    for _ in 0..2 {
        let (answer, _) = answered(dir.path(), &patch, &["--detail", "full"]);
        assert_eq!(answer["cleared"].as_array().unwrap().len(), 1);
        assert_eq!(answer["fired"].as_array().unwrap().len(), 1);
        assert_eq!(check(), plain);
    }
    assert!(!dir.path().join("new.txt").exists());
    assert!(!dir.path().join(".lighthouse/lighthouse.db").exists());
}

#[test]
fn gaps_name_the_enabled_decisions_only_a_model_decides() {
    let dir = rust_project(&[("src/lib.rs", DOCUMENTED)]);
    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}]\nextends = [\"design/recommended\"]\n[rules]\n\"design/predicate-names\" = \"info\"\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    trust(dir.path());
    let patch = diff("src/lib.rs", Some(DOCUMENTED), "pub fn run() {}\n");
    let (answer, _) = answered(dir.path(), &patch, &[]);
    assert_eq!(
        answer["gaps"],
        serde_json::json!(["design/predicate-names"]),
        "{answer}"
    );
}
