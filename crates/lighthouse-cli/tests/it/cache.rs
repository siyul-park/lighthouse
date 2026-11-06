//! The graph is invisible: a run that takes findings from it prints
//! the same bytes and exits the same as a run that does not, through an edit
//! script over a Rust and a Go project, and the cache is cleaned on request.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

use crate::{
    cache_fixtures::{Fixture, go, project, python, rust, typescript, write},
    checks::{home, trust},
};

const CACHE: &str = ".lighthouse/cache";

/// What one run printed.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    code: i32,
    stdout: String,
}

/// A run that also says how many findings the graph had.
struct Timed {
    run: Run,
    hits: usize,
    misses: usize,
}

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir)
        .envs(home(dir))
        .env_remove("LIGHTHOUSE_TRUST");
    cmd
}

fn check(dir: &Path, args: &[&str]) -> (Run, String) {
    let out = lighthouse(dir)
        .args(["check", "--no-store", "--format", "json"])
        .args(args)
        .output()
        .unwrap();
    let run = Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
    };
    (run, String::from_utf8(out.stderr).unwrap())
}

fn cold(dir: &Path) -> Run {
    check(dir, &["--no-cache"]).0
}

fn warm(dir: &Path) -> Timed {
    let (run, stderr) = check(dir, &["--timings"]);
    let line = stderr
        .lines()
        .find(|l| l.starts_with("findings: "))
        .unwrap_or_else(|| panic!("no findings line in:\n{stderr}"));
    let numbers: Vec<usize> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect();
    Timed {
        run,
        hits: numbers[1],
        misses: numbers[0],
    }
}

/// Runs the edit script and checks, after every step, that a run on the cache
/// prints what a run without it prints.
fn differential(fixture: &Fixture) {
    let dir = project(fixture);
    let dir = dir.path();
    let baseline = cold(dir);
    assert!(
        baseline.stdout.contains("local/probe"),
        "{}: the probe finds nothing:\n{}",
        fixture.name,
        baseline.stdout
    );

    let first = warm(dir);
    assert_eq!(first.run, baseline, "{}: first run", fixture.name);
    assert_eq!(first.hits, 0, "{}: nothing stored yet", fixture.name);
    let unchanged = warm(dir);
    assert_eq!(unchanged.run, baseline, "{}: no change", fixture.name);
    assert_eq!(unchanged.misses, 0, "{}: no change", fixture.name);

    for edit in &fixture.edits {
        (edit.apply)(dir);
        trust(dir);
        let expected = cold(dir);
        let got = warm(dir);
        let step = format!("{}: {}", fixture.name, edit.name);
        assert_eq!(got.run, expected, "{step}");
        // A provider option is in the key of every unit: all of them are asked.
        // A run that nothing moved for is answered whole and counts none.
        if edit.name != "provider option changed" && got.misses > 0 {
            assert!(
                got.hits > 0,
                "{step}: the cache answered nothing of {}",
                got.misses
            );
        }
        if edit.mostly_warm {
            assert!(
                got.misses < got.hits,
                "{step}: {} hits, {} misses",
                got.hits,
                got.misses
            );
        }
        let again = warm(dir);
        assert_eq!(again.run, expected, "{step}: again");
        assert_eq!(again.misses, 0, "{step}: again");
    }
    // A replace target lives beside the project.
    let _ = std::fs::remove_dir_all(dir.with_extension("dep"));
}

#[test]
fn rust_findings_are_the_same_with_and_without_the_cache() {
    differential(&rust());
}

#[test]
fn go_findings_are_the_same_with_and_without_the_cache() {
    if let Some(fixture) = go() {
        differential(&fixture);
    }
}

#[test]
fn typescript_findings_are_the_same_with_and_without_the_cache() {
    if let Some(fixture) = typescript() {
        differential(&fixture);
    }
}

#[test]
fn python_findings_are_the_same_with_and_without_the_cache() {
    if let Some(fixture) = python() {
        differential(&fixture);
    }
}

#[test]
fn clean_deletes_the_cache_directory_and_what_a_plugin_put_in_it() {
    let fixture = rust();
    let dir = project(&fixture);
    let cache: PathBuf = dir.path().join(CACHE);
    warm(dir.path());
    assert!(cache.join("results.db").exists());
    write(dir.path(), ".lighthouse/cache/lang-rust/units.json", "{}");

    lighthouse(dir.path())
        .args(["cache", "clean"])
        .assert()
        .success();
    assert!(!cache.exists());
    lighthouse(dir.path())
        .args(["cache", "clean"])
        .assert()
        .success();
}

#[test]
fn no_cache_neither_reads_nor_writes_the_cache() {
    let fixture = rust();
    let dir = project(&fixture);
    cold(dir.path());
    assert!(!dir.path().join(CACHE).exists());
}
