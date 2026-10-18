//! One experience for every fix, whatever provider makes it: an ops fix, a
//! command, an agent that edits. The same words (`applied`, `previews`,
//! `declined`), the same permission (`[approval] fix` and `--unsafe-fixes`),
//! the same way to apply what was previewed.

use std::{fs, path::Path};

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
        "  fix:\n    safety: {safety}\n    type: ops\n    ops:\n      - op: replace\n        file: finding.file\n        span: \"{{\\\"start\\\": {{\\\"line\\\": 1, \\\"col\\\": 1}}, \\\"end\\\": {{\\\"line\\\": 2, \\\"col\\\": 1}}}}\"\n        text: \"\"\n"
    )
}

fn command_fix(safety: &str) -> String {
    format!(
        "  fix:\n    safety: {safety}\n    type: command\n    argv: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{{file}}\"]\n    output: text\n"
    )
}

fn fixed(body: &str) -> String {
    format!("      fixed:\n        - path: ops.txt\n          body: {body:?}\n")
}

/// What a provider kind is called in the tests, the decision that stands for
/// it and the file that decision flags.
struct Provider {
    kind: &'static str,
    id: &'static str,
    file: &'static str,
    /// The safety its fix has.
    safety: &'static str,
    /// What the file holds once the fix is written.
    fixed: &'static str,
}

const LONG: &str = "a\nb\nc\nd\n";

const PROVIDERS: [Provider; 5] = [
    Provider {
        kind: "ops",
        id: "local/ops-safe",
        file: "ops-safe.txt",
        safety: "safe",
        fixed: "b\nc\nd\n",
    },
    Provider {
        kind: "ops",
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
            "plugins = [\"core\", \"local\"]\n[rules]\n{rules}[models]\nedit = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n[fix]\ndecisions = [\"local/model\"]\n{approval}{UNSANDBOXED}",
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

fn fix(dir: &Path, arguments: Value) -> Value {
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
        let (dir, _agent) = project("[approval]\nfix = \"allow\"\n");

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
    for (approval, unsafe_written) in [("", false), ("[approval]\nfix = \"allow\"\n", true)] {
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

/// The fingerprint of the one preview of `done`, and its apply command as
/// arguments of the binary.
fn previewed(done: &Value) -> (String, Vec<String>) {
    let preview = &done["previews"][0];
    let command = preview["apply"].as_str().unwrap();
    let args = command
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect();
    (preview["fingerprint"].as_str().unwrap().to_owned(), args)
}

fn calls(marks: &Path) -> usize {
    fs::read_to_string(marks).map_or(0, |text| text.lines().count())
}

/// The model fix project of `edited`, with an agent that counts its runs.
fn counted() -> (TempDir, TempDir, TempDir) {
    let marks = tempfile::tempdir().unwrap();
    let agent = format!(
        "printf 'x\\n' > model.txt\necho x >> {:?}\n",
        marks.path().join("calls").to_str().unwrap()
    );
    let (dir, scripts) = project("");
    write(scripts.path(), "agent.sh", &agent);
    (dir, scripts, marks)
}

#[test]
fn a_preview_is_applied_by_its_fingerprint_and_the_model_is_asked_once() {
    let (dir, _agent, marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let (fingerprint, args) = previewed(&done);
    assert_eq!(calls(&marks.path().join("calls")), 1);

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("1 fix(es) applied"),
        "{out:?}"
    );

    assert_eq!(read(&dir, "model.txt"), "x\n", "{fingerprint}");
    assert_eq!(read(&dir, "ops-unsafe.txt"), LONG, "only that fix");
    assert_eq!(
        calls(&marks.path().join("calls")),
        1,
        "the model is asked once"
    );
}

#[test]
fn the_tool_call_of_a_preview_applies_exactly_that_fix() {
    let (dir, _agent, marks) = counted();
    let mut client = Client::trusting(dir.path());
    let done = client.tool(
        "fix",
        json!({"decision": ["local/model", "local/ops-unsafe"]}),
    );
    let done = done.unwrap();
    assert_eq!(done["previews"].as_array().unwrap().len(), 2, "{done}");
    let model = done["previews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["decision"] == "local/model")
        .unwrap();

    let applied = client
        .tool("fix", model["tool"]["arguments"].clone())
        .unwrap();

    assert_eq!(applied["applied"].as_array().unwrap().len(), 1, "{applied}");
    assert_eq!(applied["applied"][0]["decision"], "local/model");
    assert_eq!(read(&dir, "model.txt"), "x\n");
    assert_eq!(read(&dir, "ops-unsafe.txt"), LONG);
    assert_eq!(calls(&marks.path().join("calls")), 1);
}

#[test]
fn a_deterministic_preview_is_recomputed_when_applied() {
    let (dir, _agent) = project("");
    let done = fix(dir.path(), json!({"decision": ["local/command-unsafe"]}));
    let (_, args) = previewed(&done);

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("1 fix(es) applied"),
        "{out:?}"
    );

    assert_eq!(read(&dir, "command-unsafe.txt"), "A\nB\nC\nD\n");
}

#[test]
fn a_changed_base_gives_a_conflict_and_nothing_is_written() {
    let (dir, _agent, marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let (_, args) = previewed(&done);
    write(dir.path(), "model.txt", "a\nb\nc\nd\ne\n");

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not fixed model.txt"), "{stderr}");
    assert_eq!(read(&dir, "model.txt"), "a\nb\nc\nd\ne\n");
    assert_eq!(calls(&marks.path().join("calls")), 1, "no new model call");
}

#[test]
fn a_conflict_is_the_class_of_the_decline() {
    let (dir, _agent, _marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let preview = done["previews"][0]["tool"]["arguments"].clone();
    write(dir.path(), "model.txt", "a\nb\nc\nd\ne\n");

    let applied = fix(dir.path(), preview);

    assert!(
        applied["applied"].as_array().unwrap().is_empty(),
        "{applied}"
    );
    assert_eq!(
        applied["models"]["local/model"]["declined"],
        json!({"conflict": 1}),
        "{applied}"
    );
}

#[test]
fn no_models_asks_no_model_and_leaves_the_finding_to_review() {
    let (dir, _agent, marks) = counted();

    let done = fix(
        dir.path(),
        json!({"decision": ["local/model"], "noModels": true}),
    );

    assert_eq!(calls(&marks.path().join("calls")), 0);
    assert_eq!(read(&dir, "model.txt"), LONG);
    assert!(done["applied"].as_array().unwrap().is_empty(), "{done}");
    let reason = done["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("left for review"), "{reason}");
    assert!(done["usage"].is_null(), "{done}");
}

#[test]
fn the_command_line_asks_no_model_with_the_flag_check_has() {
    let (dir, _agent, marks) = counted();

    lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--fix", "--no-models", "--decision", "local/model"])
        .assert()
        .stderr(predicates::str::contains("left for review"));

    assert_eq!(calls(&marks.path().join("calls")), 0);
}

fn listed(client: &mut Client) -> Value {
    let list = client.tool("decision_list", json!({})).unwrap();
    list["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["id"].as_str().unwrap().to_owned(), d.clone()))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

fn call(id: &str, unsafe_fixes: bool) -> Value {
    let mut arguments = json!({"decision": [id]});
    let mut command = format!("lighthouse check --fix --decision {id}");
    if unsafe_fixes {
        arguments["unsafeFixes"] = json!(true);
        command = format!("lighthouse check --fix --unsafe-fixes --decision {id}");
    }
    json!({"apply": command, "tool": {"name": "fix", "arguments": arguments}})
}

/// The `fix` of a descriptor: `by`, `safety` and `approval` as given, and the
/// call that applies it.
fn fix_json(by: &str, safety: &str, runs: bool, why: Option<&str>, id: &str) -> Value {
    let mut fix = json!({"by": by, "safety": safety, "runs": runs});
    if let Some(why) = why {
        fix["why"] = json!(why);
    }
    if safety == "unsafe" {
        fix["approval"] = json!("ask");
    }
    for (key, value) in call(id, safety == "unsafe").as_object().unwrap() {
        fix[key] = value.clone();
    }
    fix
}

#[test]
fn the_decision_list_describes_every_provider_the_same_way() {
    let (dir, _agent) = project("");

    let decisions = listed(&mut Client::trusting(dir.path()));

    let cel = json!({"by": "cel", "runs": true});
    for (id, by, safety) in [
        ("local/ops-safe", "ops", "safe"),
        ("local/ops-unsafe", "ops", "unsafe"),
        ("local/command-safe", "command", "safe"),
        ("local/command-unsafe", "command", "unsafe"),
        ("local/model", "model", "unsafe"),
    ] {
        assert_eq!(decisions[id]["check"], cel, "{id}");
        assert_eq!(
            decisions[id]["fix"],
            fix_json(by, safety, true, None, id),
            "{id}"
        );
    }
}

#[test]
fn an_untrusted_project_says_why_its_programs_do_not_run() {
    let (dir, _agent) = project("");

    let decisions = listed(&mut Client::start(dir.path()));

    assert_eq!(decisions["local/ops-safe"]["fix"]["runs"], true);
    for (id, by, safety) in [
        ("local/command-safe", "command", "safe"),
        ("local/command-unsafe", "command", "unsafe"),
        ("local/model", "model", "unsafe"),
    ] {
        assert_eq!(
            decisions[id]["fix"],
            fix_json(by, safety, false, Some("untrusted project"), id),
            "{id}"
        );
    }
}

#[test]
fn a_decision_a_model_may_fix_without_an_agent_bound_says_no_edit_binding() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    let bare = toml.replace("\"edit\" = \"agent/m\", ", "");
    assert_ne!(bare, toml);
    write(dir.path(), "lighthouse.toml", &bare);

    let decisions = listed(&mut Client::trusting(dir.path()));

    assert_eq!(
        decisions["local/model"]["fix"],
        fix_json(
            "model",
            "unsafe",
            false,
            Some("no edit binding"),
            "local/model"
        )
    );
}

#[test]
fn a_decision_listed_for_a_model_without_a_fix_of_its_own_gets_one_only_when_listed() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &toml.replace("\"decisions\" = [\"local/model\"]", "\"decisions\" = []"),
    );

    let decisions = listed(&mut Client::trusting(dir.path()));

    assert!(decisions["local/model"].get("fix").is_none());
    assert_eq!(decisions["local/ops-safe"]["fix"]["by"], "ops");
}

#[test]
fn decision_show_gives_the_whole_descriptor() {
    let (dir, _agent) = project("");
    let mut client = Client::trusting(dir.path());

    let safe = client
        .tool("decision_show", json!({"id": "local/ops-safe"}))
        .unwrap();
    let unsafe_fix = client
        .tool("decision_show", json!({"id": "local/ops-unsafe"}))
        .unwrap();

    assert_eq!(safe["check"], json!({"by": "cel", "runs": true}));
    assert_eq!(
        safe["fix"],
        fix_json("ops", "safe", true, None, "local/ops-safe")
    );
    assert!(
        safe.get("brief").is_none(),
        "a safe fix that runs needs no brief"
    );
    assert_eq!(unsafe_fix["brief"], true, "an unsafe fix has a brief");
}

#[test]
fn a_finding_of_a_decision_without_an_edit_agent_has_a_brief_and_no_running_fix() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &toml.replace("\"edit\" = \"agent/m\", ", ""),
    );
    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--format", "json", "--decision", "local/model"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let group = &report["groups"][0];
    let fingerprint = group["files"]["model.txt"][0][2]
        .as_str()
        .unwrap()
        .to_owned();

    let shown = Client::trusting(dir.path())
        .tool("review_show", json!({"fingerprint": fingerprint}))
        .unwrap();

    assert_eq!(group["fix"]["runs"], false, "{group}");
    assert_eq!(group["fix"]["why"], "no edit binding");
    assert_eq!(group["brief"], true);
    assert_eq!(shown["fix"]["why"], "no edit binding");
    assert!(
        shown["brief"]
            .as_str()
            .unwrap()
            .contains("Decision: local/model")
    );
}
