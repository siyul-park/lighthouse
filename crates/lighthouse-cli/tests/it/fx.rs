//! One experience for every fix, whatever provider makes it: an ops fix, a
//! command, an agent that edits. The same words (`applied`, `previews`,
//! `declined`), the same permission (`[fix] unsafe` and `--unsafe-fixes`),
//! the same way to apply what was previewed. This file holds the project all
//! of them share and the outcome each provider gives under a policy.

use std::path::Path;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    fix::{lighthouse, read},
    mcp::Client,
    models::UNSANDBOXED,
};

/// A text decision on `<name>.txt` files of more than three lines, with the
/// fix `fix`.
fn decision(name: &str, severity: &str, fix: &str, fixed: &str) -> String {
    format!(
        r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/{name}
spec:
  title: Notes stay short
  context: Notes are read in one glance.
  scope:
    subject: file
  requirement: A note MUST NOT exceed three lines.
  severity: {severity}
  check:
    type: cel
    select: file
    where: "file.path == \"{name}.txt\" && file.lines > 3"
    message: "{{{{ file.path }}}} is too long"
    evidence:
      path: file.path
{fix}  examples:
    - name: long
      language: text
      kind: invalid
      files:
        - path: {name}.txt
          body: "a\nb\nc\nd\n"
      expect:
        - line: 1
{fixed}    - name: short
      language: text
      kind: valid
      files:
        - path: {name}.txt
          body: "a\n"
"#
    )
}

fn ops_fix(safety: &str) -> String {
    format!(
        "  fix:\n    safety: {safety}\n    type: cel\n    edits: \"[replace({{'start': {{'line': 1, 'col': 1}}, 'end': {{'line': 2, 'col': 1}}}}, '')]\"\n"
    )
}

fn command_fix(safety: &str) -> String {
    format!(
        "  fix:\n    safety: {safety}\n    type: command\n    command: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{{file}}\"]\n    output: stdout\n"
    )
}

fn fixed(body: &str) -> String {
    format!("      fixed:\n        - path: ops.txt\n          body: {body:?}\n")
}

/// What a provider kind is called in the tests, the decision that stands for
/// it and the file that decision flags.
pub(crate) struct Provider {
    pub(crate) kind: &'static str,
    pub(crate) id: &'static str,
    pub(crate) file: &'static str,
    /// The safety its fix has.
    pub(crate) safety: &'static str,
    /// What the file holds once the fix is written.
    pub(crate) fixed: &'static str,
}

pub(crate) const LONG: &str = "a\nb\nc\nd\n";

pub(crate) const PROVIDERS: [Provider; 5] = [
    Provider {
        kind: "cel",
        id: "local/ops-safe",
        file: "ops-safe.txt",
        safety: "safe",
        fixed: "b\nc\nd\n",
    },
    Provider {
        kind: "cel",
        id: "local/ops-unsafe",
        file: "ops-unsafe.txt",
        safety: "unsafe",
        fixed: "b\nc\nd\n",
    },
    Provider {
        kind: "command",
        id: "local/command-safe",
        file: "command-safe.txt",
        safety: "safe",
        fixed: "A\nB\nC\nD\n",
    },
    Provider {
        kind: "command",
        id: "local/command-unsafe",
        file: "command-unsafe.txt",
        safety: "unsafe",
        fixed: "A\nB\nC\nD\n",
    },
    Provider {
        kind: "model",
        id: "local/model",
        file: "model.txt",
        safety: "unsafe",
        fixed: "x\n",
    },
];

/// A project with one decision per provider kind, each flagging a file of its
/// own, and the agent that shortens `model.txt`; `approval` is the lines of
/// `lighthouse.toml` that say what is written without asking. The directory
/// the agent lives in is returned so it outlives the test.
pub(crate) fn project(approval: &str) -> (TempDir, TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    write(scripts.path(), "agent.sh", "printf 'x\\n' > model.txt\n");
    let dir = tempfile::tempdir().unwrap();
    let rules: String = PROVIDERS
        .iter()
        .map(|p| format!("{:?} = \"{}\"\n", p.id, level(p)))
        .collect();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = []\n[rules]\n{rules}[models]\nagent = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n[fix]\ndecisions = [\"local/model\"]\n{approval}{UNSANDBOXED}",
            scripts.path().join("agent.sh").to_str().unwrap()
        )),
    );
    let sources = [
        (
            "ops-safe",
            "error",
            ops_fix("safe"),
            fixed(PROVIDERS[0].fixed),
        ),
        (
            "ops-unsafe",
            "error",
            ops_fix("unsafe"),
            fixed(PROVIDERS[1].fixed),
        ),
        (
            "command-safe",
            "error",
            command_fix("safe"),
            fixed(PROVIDERS[2].fixed),
        ),
        (
            "command-unsafe",
            "error",
            command_fix("unsafe"),
            fixed(PROVIDERS[3].fixed),
        ),
        ("model", "warn", String::new(), String::new()),
    ];
    for (name, severity, fix, fixed) in sources {
        let fixed = fixed.replace("ops.txt", &format!("{name}.txt"));
        write(
            dir.path(),
            &format!(".lighthouse/decisions/{name}.yaml"),
            &decision(name, severity, &fix, &fixed),
        );
        write(dir.path(), &format!("{name}.txt"), LONG);
    }
    (dir, scripts)
}

fn level(provider: &Provider) -> &'static str {
    if provider.id == "local/model" {
        "warn"
    } else {
        "error"
    }
}

pub(crate) fn fix(dir: &Path, arguments: Value) -> Value {
    Client::trusting(dir).tool("fix", arguments).unwrap()
}

/// The keys of an object, sorted.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn every_provider_gets_the_same_outcome_under_ask() {
    for provider in &PROVIDERS {
        let (dir, _agent) = project("");

        let done = fix(dir.path(), json!({"decision": [provider.id]}));

        let on_disk = read(&dir, provider.file);
        if provider.safety == "safe" {
            assert_eq!(on_disk, provider.fixed, "{} {done}", provider.id);
            assert_eq!(done["applied"].as_array().unwrap().len(), 1, "{done}");
            assert_eq!(done["applied"][0]["safety"], "safe", "{done}");
            assert!(done["previews"].is_null(), "{done}");
        } else {
            assert_eq!(on_disk, LONG, "{} is previewed, not written", provider.id);
            assert!(done["applied"].as_array().unwrap().is_empty(), "{done}");
            assert!(done["declined"].as_array().unwrap().is_empty(), "{done}");
            let preview = &done["previews"][0];
            assert_eq!(preview["decision"], provider.id, "{done}");
            assert_eq!(preview["safety"], "unsafe", "{done}");
            let diff = preview["diff"].as_str().unwrap();
            assert!(diff.contains(&format!("+++ b/{}", provider.file)), "{diff}");
            assert!(diff.contains("@@"), "{diff}");
            let apply = preview["apply"].as_str().unwrap();
            assert!(
                apply.starts_with("lighthouse check --fix --unsafe-fixes --fingerprint "),
                "{apply}"
            );
            assert_eq!(preview["tool"]["name"], "fix", "{preview}");
            assert_eq!(preview["tool"]["arguments"]["unsafeFixes"], true);
        }
    }
}

#[test]
fn every_provider_shows_a_preview_with_the_same_fields() {
    let mut shapes = Vec::new();
    for provider in PROVIDERS.iter().filter(|p| p.safety == "unsafe") {
        let (dir, _agent) = project("");

        let done = fix(dir.path(), json!({"decision": [provider.id]}));

        shapes.push((provider.kind, keys(&done["previews"][0]).join(",")));
    }
    assert_eq!(shapes.len(), 3);
    assert!(
        shapes.iter().all(|(_, shape)| *shape == shapes[0].1),
        "{shapes:?}"
    );
}

#[test]
fn every_provider_writes_an_unsafe_fix_when_the_project_allows_it() {
    for provider in &PROVIDERS {
        let (dir, _agent) = project("unsafe = true\n");

        let done = fix(dir.path(), json!({"decision": [provider.id]}));

        assert_eq!(read(&dir, provider.file), provider.fixed, "{}", provider.id);
        assert_eq!(done["applied"].as_array().unwrap().len(), 1, "{done}");
        assert_eq!(done["applied"][0]["safety"], provider.safety, "{done}");
        assert!(done["previews"].is_null(), "{done}");
    }
}

#[test]
fn every_provider_writes_an_unsafe_fix_when_the_run_asks_for_unsafe_fixes() {
    for provider in &PROVIDERS {
        let (dir, _agent) = project("");

        let done = fix(
            dir.path(),
            json!({"decision": [provider.id], "unsafeFixes": true}),
        );

        assert_eq!(read(&dir, provider.file), provider.fixed, "{}", provider.id);
        assert_eq!(done["applied"].as_array().unwrap().len(), 1, "{done}");
    }
}

#[test]
fn a_dry_run_writes_nothing_and_says_what_each_provider_would_do() {
    for (approval, unsafe_written) in [("", false), ("unsafe = true\n", true)] {
        for provider in &PROVIDERS {
            let (dir, _agent) = project(approval);

            let done = fix(
                dir.path(),
                json!({"decision": [provider.id], "dryRun": true}),
            );

            assert_eq!(read(&dir, provider.file), LONG, "{}", provider.id);
            assert_eq!(done["dryRun"], true);
            let written = provider.safety == "safe" || unsafe_written;
            let key = if written { "applied" } else { "previews" };
            assert_eq!(
                done[key].as_array().unwrap().len(),
                1,
                "{} {done}",
                provider.id
            );
            assert!(done["diff"].as_str().unwrap().is_empty() != written);
        }
    }
}

#[test]
fn an_unsafe_deterministic_fix_under_ask_is_previewed_on_the_command_line() {
    let (dir, _agent) = project("");

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--fix", "--decision", "local/ops-unsafe"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("preview local/ops-unsafe [unsafe] ops-unsafe.txt"),
        "{stderr}"
    );
    assert!(stderr.contains("-a"), "{stderr}");
    assert!(
        stderr.contains("apply: lighthouse check --fix --unsafe-fixes --fingerprint "),
        "{stderr}"
    );
    assert!(!stderr.contains("not fixed"), "{stderr}");
    assert_eq!(read(&dir, "ops-unsafe.txt"), LONG);
}
