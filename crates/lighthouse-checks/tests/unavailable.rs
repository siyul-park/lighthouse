//! Programs a project names when the sandbox cannot be had, in a test binary of
//! its own: the simulation is process-wide. A check whose program cannot run
//! leaves the analysis incomplete, never clean, and never runs unsandboxed.

use std::{collections::BTreeMap, fs};

use lighthouse_checks::Checks;
use lighthouse_engine::{Engine, Run};
use lighthouse_process::{Simulation, simulate_unavailable};
use lighthouse_spec::{Catalog, Config};

/// A command check of every file that marks the project when it runs.
const PROBE: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/probe
spec:
  title: Probe
  context: A probe.
  scope: { subject: file }
  requirement: A file MUST NOT say hello.
  severity: error
  check:
    type: command
    command: [sh, -c, "touch ran; exit 1", sh, "{file}"]
  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: bad.txt, body: hello }]
      expect: [{ line: 1 }]
    - name: good
      language: text
      kind: valid
      files: [{ path: good.txt, body: bye }]
"#;

/// Checks a project with the command check `local/probe` and the bundled
/// `core/formatted`, whose program is `gofmt`.
fn check(dir: &std::path::Path) -> Run {
    let layer = Catalog::from_local(
        BTreeMap::from([("probe.yaml".to_owned(), PROBE.to_owned())]),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let plugin = Checks::new(&lighthouse_test_support::composition(), "local", &layer).unwrap();
    let mut registry = lighthouse_test_support::registry();
    registry.register(&plugin).unwrap();
    let config = Config::parse_inline(
        "plugins = [\"core\", \"go\", \"local\"]\n[rules]\n\"local/probe\" = \"error\"\n\"core/formatted\" = \"error\"\n",
    )
    .unwrap();
    let catalog = Catalog::overlay(lighthouse_test_support::bundled(), &layer).unwrap();
    Engine::new(registry, config, &catalog, dir)
        .unwrap()
        .with_trust(true)
        .check(&[], &[])
        .unwrap()
}

#[test]
fn a_command_check_and_a_bundled_one_leave_the_analysis_incomplete_when_the_sandbox_is_unavailable()
{
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    fs::write(dir.path().join("a.go"), "package a\nfunc  F( ) {}\n").unwrap();
    let _gone: Simulation = simulate_unavailable("no backend here");

    let run = check(dir.path());

    assert!(run.diagnostics.is_empty(), "{:?}", run.diagnostics);
    let reasons: Vec<&str> = run.incomplete.iter().map(|i| i.reason.as_str()).collect();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("sandbox unavailable: no backend here")),
        "{reasons:?}"
    );
    assert!(
        run.incomplete.len() >= 2,
        "the check and the bundled one both: {reasons:?}"
    );
    assert!(!dir.path().join("ran").exists(), "the program ran");
}
