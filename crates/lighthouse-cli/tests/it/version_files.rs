//! `decision test` gives each example the version files of the project root,
//! through the binary: a version manager's shim picks its toolchain from the
//! files above its directory, and an example's own directory has none.

use tempfile::TempDir;

use super::checks::{lighthouse, trust, write};

const VERSION_FILE: &str = ".go-version";

/// A local decision whose command passes only where `.go-version` says go1.22:
/// the valid example does not write the file, so it gets the project's, and
/// the invalid one writes its own, which wins.
const NEEDS_VERSION_FILE: &str = "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/probe
spec:
  title: Probe
  context: A tool picks its toolchain from the version file.
  scope: { subject: project }
  requirement: The toolchain MUST be selected.
  severity: error
  check:
    type: command
    command: [grep, -q, go1.22, .go-version]
    output: status
  examples:
    - name: good
      language: text
      kind: valid
      files: [{ path: good.txt, body: bye }]
    - name: bad
      language: text
      kind: invalid
      files: [{ path: .go-version, body: go1.21 }]
      expect: [{ line: 1 }]
";

/// A trusted project of the decision, with the plugin of Go listed and the
/// version file at its root when `versioned`.
fn project(versioned: bool) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = []\nversionFiles = [\".go-version\"]\n[rules]\n\"local/probe\" = \"error\"\n",
        ),
    );
    write(
        dir.path(),
        ".lighthouse/decisions/probe.yaml",
        NEEDS_VERSION_FILE,
    );
    if versioned {
        write(dir.path(), VERSION_FILE, "go1.22\n");
    }
    trust(dir.path());
    dir
}

#[test]
fn an_example_runs_with_the_version_file_of_the_project_root() {
    let dir = project(true);

    lighthouse(dir.path())
        .args(["decision", "test", "local/probe"])
        .assert()
        .success();
}

#[test]
fn an_example_fails_when_the_project_root_has_no_version_file() {
    let dir = project(false);

    lighthouse(dir.path())
        .args(["decision", "test", "local/probe"])
        .assert()
        .failure();
}
