//! `lighthouse check --fix` through the binary, over Go and Rust projects with
//! the real language plugins and over a text project with project-local rules
//! that fix by operation and by command.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// The binary in `dir`, with the user's trust file kept inside the project's
/// own scratch space and no CI override: nothing leaks between tests.
fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir)
        .env("LIGHTHOUSE_HOME", dir.join(".home"))
        .env_remove("LIGHTHOUSE_TRUST");
    cmd
}

fn trust(dir: &Path) {
    lighthouse(dir).args(["trust", "--yes"]).assert().success();
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name)).unwrap()
}

/// A Go project with the design and testing rules; `None` after a skip
/// message when there is no Go toolchain.
fn go_project(files: &[(&str, &str)], extra: &str) -> Option<TempDir> {
    let plugin = lighthouse_test_support::lang_go()?;
    let dir = tempfile::tempdir().unwrap();
    let local = if files
        .iter()
        .any(|(name, _)| name.starts_with(".lighthouse"))
    {
        "\"local\", "
    } else {
        ""
    };
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"design\", \"testing\", {local}{{ id = \"lang-go\", path = {:?} }}]\nextends = [\"core/recommended\", \"design/recommended\", \"testing/recommended\"]\n{extra}",
            plugin.to_str().unwrap()
        )),
    );
    write(dir.path(), "go.mod", "module example.com/demo\n\ngo 1.26\n");
    for (name, text) in files {
        write(dir.path(), name, text);
    }
    Some(dir)
}

fn rust_project(source: &str, extra: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    let local = if extra.contains("local/") {
        "\"local\", "
    } else {
        ""
    };
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"design\", {local}{{ id = \"lang-rust\", path = {:?} }}]\nextends = [\"design/recommended\"]\n{extra}",
            plugin.to_str().unwrap()
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(dir.path(), "src/lib.rs", source);
    dir
}

const MISORDERED: &str = "package demo\n\nfunc helper() int { return 1 }\n\ntype Service struct{}\n\nfunc (s *Service) Run() int { return helper() }\n";
const ORDERED: &str = "package demo\n\ntype Service struct{}\n\nfunc (s *Service) Run() int { return helper() }\n\nfunc helper() int { return 1 }\n";

#[test]
fn dry_run_prints_the_diff_and_changes_nothing() {
    let Some(dir) = go_project(&[("a.go", MISORDERED)], "") else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--- a/a.go\n+++ b/a.go\n"))
        .stdout(predicate::str::contains("+func helper() int { return 1 }"))
        .stderr(predicate::str::contains(
            "would fix design/declaration-groups [safe]",
        ))
        .stderr(predicate::str::contains("1 fix(es) proposed"));

    assert_eq!(read(&dir, "a.go"), MISORDERED);
}

#[test]
fn fix_applies_safe_fixes_and_reports_what_is_left() {
    let Some(dir) = go_project(&[("a.go", MISORDERED)], "") else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("declaration-groups").not())
        .stderr(predicate::str::contains(
            "fixed design/declaration-groups [safe] a.go",
        ));

    assert_eq!(read(&dir, "a.go"), ORDERED);
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout(predicate::str::contains("declaration-groups").not());
}

#[test]
fn suggested_fixes_wait_for_unsafe_fixes() {
    let banner = "package demo\n\n// ---- Helpers ----\n\nfunc helper() int { return 1 }\n";
    let Some(dir) = go_project(&[("a.go", banner)], "") else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "not fixed a.go:3 design/no-banners",
        ))
        .stderr(predicate::str::contains("--unsafe-fixes"));
    assert_eq!(read(&dir, "a.go"), banner);

    lighthouse(dir.path())
        .args(["check", "--fix", "--unsafe-fixes"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "fixed design/no-banners [suggested]",
        ));
    assert_eq!(
        read(&dir, "a.go"),
        "package demo\n\nfunc helper() int { return 1 }\n"
    );
}

#[test]
fn paths_and_rules_select_what_is_fixed() {
    let Some(dir) = go_project(
        &[
            ("a.go", MISORDERED),
            ("sub/b.go", &MISORDERED.replace("demo", "sub")),
        ],
        "",
    ) else {
        return;
    };

    lighthouse(dir.path())
        .args([
            "check",
            "sub",
            "--fix",
            "--rules",
            "design/declaration-groups",
        ])
        .assert()
        .success();

    assert_eq!(read(&dir, "a.go"), MISORDERED, "outside the paths");
    assert_eq!(read(&dir, "sub/b.go"), ORDERED.replace("demo", "sub"));
}

#[test]
fn a_configured_formatter_runs_on_the_files_a_fix_changed() {
    let squashed = "package demo\n\nfunc helper() int { return 1 }\n\ntype Service struct{}\n\nfunc (s *Service) Run() int {\n        return helper()\n}\n";
    let Some(dir) = go_project(
        &[("a.go", squashed)],
        "[languages.go]\nformatter = [\"gofmt\", \"-w\"]\n",
    ) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("not formatted"));
    trust(dir.path());
    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .success();

    let fixed = read(&dir, "a.go");
    assert!(
        fixed.contains("\treturn helper()\n"),
        "gofmt indented it:\n{fixed}"
    );
    assert!(fixed.find("type Service").unwrap() < fixed.find("func helper").unwrap());
}

#[test]
fn flags_that_only_make_sense_with_fix_are_refused_without_it() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("plugins = [\"core\"]\n"),
    );

    for flag in ["--dry-run", "--unsafe-fixes"] {
        lighthouse(dir.path())
            .args(["check", flag])
            .assert()
            .code(2);
    }
}

#[test]
fn a_rename_is_declined_because_no_provider_has_complete_references_yet() {
    let source = "package demo\n\nfunc doOld() int { return 1 }\n\nfunc Use() int { return doOld() + doOld() }\n";
    let Some(dir) = go_project(
        &[
            ("a.go", source),
            (".lighthouse/decisions/old.yaml", RENAME_RULE),
        ],
        "[rules]\n\"local/old-suffix\" = \"error\"\n",
    ) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--rules", "local/old-suffix"])
        .assert()
        .stderr(predicate::str::contains("complete-references"));

    assert_eq!(read(&dir, "a.go"), source);
}

#[test]
fn a_rename_is_declined_in_rust_too() {
    let source = "pub fn do_old() -> u8 {\n    1\n}\n\npub fn run() -> u8 {\n    do_old()\n}\n";
    let dir = rust_project(source, "[rules]\n\"local/old-suffix\" = \"error\"\n");
    write(
        dir.path(),
        ".lighthouse/decisions/old.yaml",
        &RENAME_RULE.replace("endsWith(\\\"Old\\\")", "endsWith(\\\"_old\\\")"),
    );

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .stderr(predicate::str::contains("complete-references"));

    assert_eq!(read(&dir, "src/lib.rs"), source);
}

const RENAME_RULE: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/old-suffix
spec:
  title: Functions do not end in Old
  context: Names do not carry their history.
  scope:
    domain: code
    subject: symbol
  requirement: A function MUST NOT have a name that ends in Old.
  severity: error
  check:
    type: cel
    select: function
    where: "func.kind == \"function\" && func.name.endsWith(\"Old\")"
    message: "function {{ func.name }} ends in Old"
    evidence:
      name: func.name
  fix:
    safety: safe
    requires: [reference-sites, complete-references]
    type: ops
    ops:
      - op: rename
        symbol: finding.symbol
        name: "{{ symbol.name }}New"
  examples:
    - name: old
      language: go
      kind: invalid
      files:
        - path: go.mod
          body: |-
            module example.com/demo

            go 1.26
        - path: a.go
          body: |-
            package demo

            func DoOld() int { return 1 }

            func Use() int { return DoOld() }
      expect:
        - line: 3
      fixed:
        - path: a.go
          body: |-
            package demo

            func DoOldNew() int { return 1 }

            func Use() int { return DoOldNew() }
    - name: new
      language: go
      kind: valid
      files:
        - path: go.mod
          body: |-
            module example.com/demo

            go 1.26
        - path: a.go
          body: |-
            package demo

            func DoNew() int { return 1 }
"#;

const UPPERCASE: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/shout
spec:
  title: Notes are upper case
  context: Notes are read from far away.
  scope:
    domain: code
    subject: file
  requirement: A note MUST be written in upper case.
  severity: error
  check:
    type: cel
    select: file
    where: "file.path.endsWith(\".txt\") && file.lines > 0"
    message: "{{ file.path }} is not shouting"
    evidence:
      path: file.path
  fix:
    safety: safe
    type: command
    argv: [sh, "-c", "tr a-z A-Z < \"$1\"", sh, "{file}"]
    output: text
  examples:
    - name: quiet
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: hello
      expect:
        - line: 1
      fixed:
        - path: a.txt
          body: HELLO
    - name: loud
      language: text
      kind: valid
      files:
        - path: a.txt
          body: HELLO
"#;

fn text_project(rule: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/shout\" = \"error\"\n",
        ),
    );
    write(dir.path(), ".lighthouse/decisions/shout.yaml", rule);
    write(dir.path(), "a.txt", "hello\n");
    dir
}

#[test]
fn a_command_runs_only_after_the_user_trusts_the_project() {
    let dir = text_project(UPPERCASE);

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .stderr(predicate::str::contains("not trusted"))
        .stderr(predicate::str::contains("tr a-z A-Z"));
    assert_eq!(read(&dir, "a.txt"), "hello\n");

    trust(dir.path());
    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .stderr(predicate::str::contains("fixed local/shout [safe] a.txt"));
    assert_eq!(read(&dir, "a.txt"), "HELLO\n");
}

#[test]
fn a_change_to_the_configuration_or_the_rules_withdraws_trust() {
    let dir = text_project(UPPERCASE);
    trust(dir.path());
    write(dir.path(), "a.txt", "hello\n");

    let config = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &format!("{config}# edited\n"),
    );
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("not trusted"));

    write(dir.path(), "lighthouse.toml", &config);
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));

    let rule = read(&dir, ".lighthouse/decisions/shout.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/shout.yaml",
        &rule.replace("tr a-z A-Z", "tr a-z B-Z"),
    );
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("not trusted"));
}

#[test]
fn trust_can_be_revoked_and_ci_can_override_it() {
    let dir = text_project(UPPERCASE);
    trust(dir.path());
    lighthouse(dir.path())
        .args(["trust", "--revoke"])
        .assert()
        .success();

    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("not trusted"));
    lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));
}

#[test]
fn a_fix_table_in_lighthouse_toml_cannot_grant_trust() {
    let dir = text_project(UPPERCASE);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[fix]\ncommands = \"allow\"\n[rules]\n\"local/shout\" = \"error\"\n",
        ),
    );

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .code(2);
    assert_eq!(read(&dir, "a.txt"), "hello\n");
}

#[test]
fn a_fixer_can_be_named_for_a_rule_that_has_none() {
    let dir = text_project(UPPERCASE);
    // A rule with no fix of its own flags the same file; the fixer of
    // `local/shout` is named for it and counts as a suggestion.
    write(dir.path(), ".lighthouse/decisions/plain.yaml", PLAIN);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/plain\" = \"error\"\n",
        ),
    );
    let named = [
        "check",
        "--fix",
        "--fixer",
        "local/shout",
        "--rules",
        "local/plain",
    ];

    lighthouse(dir.path())
        .args(named)
        .assert()
        .stderr(predicate::str::contains("--unsafe-fixes"));
    assert_eq!(read(&dir, "a.txt"), "hello\n");

    trust(dir.path());
    lighthouse(dir.path())
        .args(named)
        .arg("--unsafe-fixes")
        .assert()
        .stderr(predicate::str::contains("fixed local/plain [suggested]"));
    assert_eq!(read(&dir, "a.txt"), "HELLO\n");
}

const PLAIN: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/plain
spec:
  title: Notes are not lowercase
  context: Notes are read from far away.
  scope:
    domain: code
    subject: file
  requirement: A note MUST NOT be written in lower case.
  severity: error
  check:
    type: cel
    select: file
    where: "file.path == \"a.txt\" && file.lines > 0"
    message: "{{ file.path }} is lower case"
    evidence:
      path: file.path
  examples:
    - name: quiet
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: hello
      expect:
        - line: 1
    - name: loud
      language: text
      kind: valid
      files:
        - path: a.txt
          body: HELLO
"#;

#[test]
fn an_unknown_fixer_is_a_usage_error() {
    let dir = text_project(UPPERCASE);

    lighthouse(dir.path())
        .args(["check", "--fix", "--fixer", "nope/nothing"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown fixer `nope/nothing`"));
}

#[test]
fn a_command_that_fails_is_declined_and_leaves_the_file() {
    let failing = UPPERCASE
        .replace(
            "argv: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{file}\"]",
            "argv: [sh, \"-c\", \"exit 3\"]",
        )
        .replace(
            "fixed:\n      - path: a.txt\n        body: HELLO\n",
            "fixed:\n      - path: a.txt\n        body: hello\n",
        );
    let dir = text_project(&failing);
    trust(dir.path());

    let out = lighthouse(dir.path())
        .args(["check", "--fix"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not fixed a.txt"), "{stderr}");
    assert!(stderr.contains("failed"), "{stderr}");
    assert_eq!(read(&dir, "a.txt"), "hello\n");
}

const DOCUMENT_RULE: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/documented
spec:
  title: Public functions have a doc line
  context: A reader should not have to guess what an exported function is for.
  scope:
    domain: code
    subject: symbol
  requirement: A public function MUST have a doc comment.
  severity: error
  check:
    type: cel
    select: function
    where: "func.kind == \"function\" && func.visibility == \"public\" && !func.documented"
    message: "function {{ func.name }} has no doc comment"
    evidence:
      name: func.name
  fix:
    safety: safe
    type: ops
    ops:
      - op: replace
        file: finding.file
        span: "{\"start\": finding.span.start, \"end\": finding.span.start}"
        text: |
          // {{ symbol.name }} is documented.
  examples:
    - name: bare
      language: go
      kind: invalid
      files:
        - path: go.mod
          body: |-
            module example.com/demo

            go 1.26
        - path: a.go
          body: |-
            package demo

            func Run() int { return 1 }
      expect:
        - line: 3
      fixed:
        - path: a.go
          body: |-
            package demo

            // Run is documented.
            func Run() int { return 1 }
    - name: documented
      language: go
      kind: valid
      files:
        - path: go.mod
          body: |-
            module example.com/demo

            go 1.26
        - path: a.go
          body: |-
            package demo

            // Run runs.
            func Run() int { return 1 }
"#;

#[test]
fn a_replace_inserts_a_doc_line_in_go() {
    let Some(dir) = go_project(
        &[
            ("a.go", "package demo\n\nfunc Run() int { return 1 }\n"),
            (".lighthouse/decisions/doc.yaml", DOCUMENT_RULE),
        ],
        "[rules]\n\"local/documented\" = \"error\"\n",
    ) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .success();

    assert_eq!(
        read(&dir, "a.go"),
        "package demo\n\n// Run is documented.\nfunc Run() int { return 1 }\n"
    );
}

#[test]
fn a_replace_inserts_a_doc_line_in_rust() {
    let dir = rust_project(
        "pub fn run() -> u8 {\n    1\n}\n",
        "[rules]\n\"local/documented\" = \"error\"\n",
    );
    write(
        dir.path(),
        ".lighthouse/decisions/doc.yaml",
        &DOCUMENT_RULE.replace("// {{ symbol.name }}", "/// {{ symbol.name }}"),
    );

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .success();

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "/// run is documented.\npub fn run() -> u8 {\n    1\n}\n"
    );
}

#[test]
fn a_run_without_the_store_still_honors_the_committed_decisions() {
    let banner = "package demo\n\n// ---- Helpers ----\n\nfunc helper() int { return 1 }\n";
    let Some(dir) = go_project(&[("a.go", banner)], "") else {
        return;
    };
    let found = lighthouse(dir.path())
        .args(["check", "--format", "json", "--rules", "design/no-banners"])
        .output()
        .unwrap();
    let finding: serde_json::Value =
        serde_json::from_slice(found.stdout.split(|&b| b == b'\n').next().unwrap()).unwrap();
    let fingerprint = finding["fingerprint"].as_str().unwrap();
    lighthouse(dir.path())
        .args([
            "review",
            "resolve",
            fingerprint,
            "--verdict",
            "rejected",
            "--reason",
            "intentional-exception",
            "--note",
            "the banner is a map for readers",
        ])
        .assert()
        .success();

    lighthouse(dir.path())
        .args(["check", "--fix", "--unsafe-fixes", "--no-store"])
        .assert()
        .success();

    assert_eq!(read(&dir, "a.go"), banner, "the team's decision stands");
}

#[test]
fn a_rust_file_that_declares_a_module_file_is_formatted_through_stdin() {
    let source =
        "pub fn run() -> u8 {\n    1\n}\n\npub  struct Store;\n\n#[cfg(test)]\nmod tests;\n";
    let formatter = "[languages.rust]\nformatter = { argv = [\"rustfmt\", \"--edition\", \"2024\", \"--emit\", \"stdout\"], stdin = \"file\", output = \"text\" }\n";
    let dir = rust_project(source, formatter);
    write(dir.path(), "src/tests.rs", "use super::*;\n");
    trust(dir.path());

    lighthouse(dir.path())
        .args(["check", "--fix", "--rules", "design/declaration-groups"])
        .assert()
        .success()
        .stderr(predicate::str::contains("rolled back").not())
        .stderr(predicate::str::contains("fixed design/declaration-groups"));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub struct Store;\n\npub fn run() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests;\n",
        "reordered and formatted, with its module file untouched"
    );
}

#[test]
fn a_formatter_that_cannot_use_stdin_still_works_on_a_scratch_copy() {
    let source = "pub fn run() -> u8 {\n    1\n}\n\npub  struct Store;\n";
    let dir = rust_project(
        source,
        "[languages.rust]\nformatter = [\"rustfmt\", \"--edition\", \"2024\"]\n",
    );
    trust(dir.path());

    lighthouse(dir.path())
        .args(["check", "--fix", "--rules", "design/declaration-groups"])
        .assert()
        .success();

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub struct Store;\n\npub fn run() -> u8 {\n    1\n}\n"
    );
}

#[test]
fn trust_lists_what_it_trusts_and_needs_a_terminal_or_yes() {
    let dir = text_project(UPPERCASE);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/shout\" = \"error\"\n[languages.go]\nformatter = [\"gofmt\"]\n",
        ),
    );

    lighthouse(dir.path())
        .arg("trust")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("formatter for go: gofmt"))
        .stdout(predicate::str::contains(
            "fixer of local/shout: sh -c tr a-z A-Z",
        ))
        .stderr(predicate::str::contains("--yes"));
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("not trusted"));

    lighthouse(dir.path())
        .args(["trust", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("trusting"));
}

#[test]
fn a_program_inside_the_project_is_trusted_by_its_content() {
    let dir = text_project(UPPERCASE);
    write(
        dir.path(),
        "tools/shout.sh",
        "#!/bin/sh\ntr a-z A-Z < \"$1\"\n",
    );
    let rule = read(&dir, ".lighthouse/decisions/shout.yaml").replace(
        "argv: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{file}\"]",
        "argv: [\"./tools/shout.sh\", \"{file}\"]",
    );
    write(dir.path(), ".lighthouse/decisions/shout.yaml", &rule);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("tools/shout.sh");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    trust(dir.path());
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));

    write(
        dir.path(),
        "tools/shout.sh",
        "#!/bin/sh\ncat \"$1\"; echo pwned\n",
    );

    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("not trusted"));
}
