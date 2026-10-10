//! What the engine takes as subjects: test code, generated code and the
//! attributes that say which is which.

use tempfile::TempDir;

use std::path::PathBuf;

use crate::engine::{ALL, engine, project};

/// The files `fake/each` reported on.
fn reported(dir: &TempDir, toml: &str) -> Vec<String> {
    let out = engine(dir, toml)
        .unwrap()
        .check(&[], &["fake/each".to_owned()])
        .unwrap();
    let mut files: Vec<String> = out
        .diagnostics
        .iter()
        .map(|d| d.file.to_string_lossy().into_owned())
        .collect();
    files.dedup();
    files
}

#[test]
fn test_code_is_not_a_subject_unless_the_scope_says_so() {
    let dir = project(&[("a/x.txt", b"1"), ("tests/t.txt", b"2")]);

    assert_eq!(reported(&dir, ALL), ["a/x.txt"]);
}

#[test]
fn generated_code_is_told_apart_by_the_host_and_left_out_of_the_subjects() {
    let dir = project(&[
        ("a/x.txt", b"1"),
        ("gen/g.txt", b"2"),
        ("vendor/v.txt", b"3"),
        (
            ".gitattributes",
            b"gen/** linguist-generated=true\n*.md -linguist-generated\n",
        ),
    ]);

    assert_eq!(reported(&dir, ALL), ["a/x.txt", "vendor/v.txt"]);
    let by_config = format!("{ALL}[generated]\nfiles = [\"vendor/**\"]\n");
    assert_eq!(reported(&dir, &by_config), ["a/x.txt"]);
}

#[test]
fn the_project_decides_whether_generated_code_is_checked() {
    let dir = project(&[
        ("a/x.txt", b"1"),
        ("gen/g.txt", b"2"),
        (".gitattributes", b"gen/** linguist-generated\n"),
    ]);
    let include = "plugins = [\"fake\"]\n[generated]\ncheck = \"include\"\n[rules]\n\"fake/each\" = \"error\"\n";
    let one_rule =
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = { level = \"error\", generated = true }\n";
    let skip_over_rule = "plugins = [\"fake\"]\n[generated]\ncheck = \"skip\"\n[rules]\n\"fake/each\" = { level = \"error\", generated = true }\n";
    let all = ["a/x.txt", "gen/g.txt"];

    assert_eq!(reported(&dir, include), all);
    assert_eq!(reported(&dir, one_rule), all);
    assert_eq!(
        reported(&dir, skip_over_rule),
        all,
        "a rule's own setting wins over the project's"
    );
}

#[test]
fn a_rule_about_test_code_runs_on_test_files_only() {
    let dir = project(&[("a/x.txt", b"1"), ("tests/t.txt", b"2")]);
    let toml = "plugins = [\"fake\"]\n[rules]\n\"fake/tests\" = \"error\"\n";
    let out = engine(&dir, toml)
        .unwrap()
        .check(&[], &["fake/tests".to_owned()])
        .unwrap();

    let files: Vec<_> = out.diagnostics.iter().map(|d| d.file.clone()).collect();

    assert_eq!(
        files,
        [PathBuf::from("tests/t.txt"), PathBuf::from("tests/t.txt")]
    );
}

#[test]
fn a_pattern_of_gitattributes_that_is_not_a_glob_is_a_notice_not_silence() {
    let dir = project(&[
        ("a/x.txt", b"1"),
        (".gitattributes", b"[unclosed linguist-generated\n"),
    ]);

    let out = engine(&dir, ALL).unwrap().check(&[], &[]).unwrap();

    assert!(
        out.notices
            .iter()
            .any(|n| n.contains(".gitattributes") && n.contains("[unclosed")),
        "{:?}",
        out.notices
    );
}
