//! The layers of the configuration through the binary: the user's file and the
//! repository-local file over the project's, `config show`, and the agent each
//! decision binds for its own fixes. Every test points the user's directory at
//! the temporary project, never at the real home.

use std::fs;

use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    checks::{lighthouse, write},
    fix::{UPPERCASE, text_project, trust},
    fix_scope::{ALLOW, BAN, WIDE, agent_project, read},
    mcp::Client,
};

const BAN2: &str = "local/naming-ban2";
const USER_FILE: &str = ".home/lighthouse/config.toml";
const LOCAL_FILE: &str = ".lighthouse/local.toml";

fn user(dir: &TempDir, text: &str) {
    write(dir.path(), USER_FILE, text);
}

fn local(dir: &TempDir, text: &str) {
    write(dir.path(), LOCAL_FILE, text);
}

fn shown(dir: &TempDir, args: &[&str]) -> String {
    let out = lighthouse(dir.path())
        .args(["config", "show"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// A project with an agent that fixes the first banned function.
fn agent_dir() -> (TempDir, TempDir) {
    let agent =
        "sed 's/bad_one/fixed_one/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\n";
    agent_project("", agent, "")
}

#[test]
fn a_meaning_key_in_the_local_file_is_a_load_error_that_names_the_key_and_the_file() {
    let (dir, _agent) = agent_dir();
    local(&dir, "plugins = [\"core\"]\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("`plugins`"))
        .stderr(predicate::str::contains(LOCAL_FILE));
}

#[test]
fn a_meaning_key_in_the_user_file_is_a_load_error_in_every_repository() {
    let (dir, _agent) = agent_dir();
    user(&dir, "[rules.\"local/naming-ban\"]\nlevel = \"off\"\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "`rules.\"local/naming-ban\".level`",
        ))
        .stderr(predicate::str::contains("config.toml"));
}

#[test]
fn a_decision_the_local_file_binds_must_exist() {
    let (dir, _agent) = agent_dir();
    local(&dir, "[rules.\"local/typo\"]\nedit = \"agent/m\"\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "`rules.\"local/typo\"` names no decision",
        ));
}

#[test]
fn config_show_origin_names_the_layer_of_each_value() {
    let (dir, _agent) = agent_dir();
    user(&dir, "[models]\nbudget = 7\n[approval]\nfix = \"allow\"\n");
    local(
        &dir,
        "[models]\nedit = \"second/m\"\n[models.providers.second]\ncommand = [\"sh\", \"x.sh\"]\n[rules.\"local/naming-ban\"]\nedit = \"second/m\"\n",
    );

    let text = shown(&dir, &["--origin"]);

    let line = |origin: &str, rest: &str| {
        text.lines().any(|line| {
            line.split_once('\t')
                .is_some_and(|(found, key)| found.ends_with(origin) && key.starts_with(rest))
        })
    };
    assert!(line("bundled", "models.explore = 0.05"), "{text}");
    assert!(
        line("lighthouse.toml", "models.providers.agent = "),
        "{text}"
    );
    assert!(line(USER_FILE, "models.budget = 7"), "{text}");
    assert!(line(USER_FILE, "approval.fix = \"allow\""), "{text}");
    assert!(line(LOCAL_FILE, "models.edit = \"second/m\""), "{text}");
    assert!(
        line(
            LOCAL_FILE,
            "models.providers.second = { command = [\"sh\", \"x.sh\"] }"
        ),
        "{text}"
    );
    assert!(
        line(LOCAL_FILE, "rules.\"local/naming-ban\".edit = \"second/m\""),
        "{text}"
    );
}

#[test]
fn config_show_without_origin_prints_the_effective_keys_only() {
    let (dir, _agent) = agent_dir();
    local(&dir, "[models]\nbudget = 3\n");

    let text = shown(&dir, &[]);

    assert!(
        text.lines().any(|line| line == "models.budget = 3"),
        "{text}"
    );
    assert!(!text.contains('\t'), "{text}");
}

#[test]
fn config_show_in_json_is_the_tool_config_show() {
    let (dir, _agent) = agent_dir();
    local(&dir, "[models]\nbudget = 3\n");

    let from_cli: Value =
        serde_json::from_str(&shown(&dir, &["--origin", "--format", "json"])).unwrap();
    let from_tool = Client::start(dir.path())
        .tool("config_show", json!({"origin": true}))
        .unwrap();

    assert_eq!(from_cli, from_tool);
    let budget = from_cli["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["key"] == "models.budget")
        .unwrap();
    assert_eq!(budget["value"], 3);
    assert!(budget["origin"].as_str().unwrap().ends_with(LOCAL_FILE));
    let plain: Value = serde_json::from_str(&shown(&dir, &["--format", "json"])).unwrap();
    assert!(plain["settings"][0].get("origin").is_none(), "{plain}");
}

#[test]
fn files_that_say_nothing_change_nothing() {
    let (dir, _agent) = agent_dir();
    let before = (shown(&dir, &["--origin"]), shown(&dir, &[]));

    user(&dir, "# mine\n");
    local(&dir, "");
    let after = (shown(&dir, &["--origin"]), shown(&dir, &[]));

    assert_eq!(before, after);
}

#[test]
fn a_personal_file_withdraws_the_trust_given_to_the_project_without_it() {
    let dir = text_project(UPPERCASE);
    trust(dir.path());
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));

    local(&dir, "[approval]\nfix = \"allow\"\n");
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("not trusted"));

    fs::remove_file(dir.path().join(LOCAL_FILE)).unwrap();
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));
}

/// Two decisions in one file, bound to different agents: the project's agent
/// fixes the first and the one the local file names for the second records
/// what it was given. The project fixes the way it was before.
fn two_agents() -> (TempDir, TempDir) {
    let agent =
        "sed 's/bad_one/fixed_one/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\n";
    let rest = format!("{ALLOW}[fix]\ndecisions = [\"local/naming-*\"]\n");
    let (dir, scripts) = agent_project(&format!("\"{BAN2}\" = \"warn\"\n"), agent, &rest);
    let second = WIDE
        .replace(BAN, BAN2)
        .replace("bad_one", "bad_two")
        .replace("fixed_one", "fixed_two");
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(dir.path(), ".lighthouse/decisions/ban2.yaml", &second);
    let seen = scripts.path().join("seen");
    write(
        scripts.path(),
        "second.sh",
        &format!(
            "cp src/lib.rs {seen:?}\nsed 's/bad_two/fixed_two/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\n"
        ),
    );
    let script = scripts.path().join("second.sh");
    local(
        &dir,
        &format!(
            "[models.providers.second]\ncommand = [\"sh\", {:?}]\n[rules.\"{BAN2}\"]\nedit = \"second/m\"\n",
            script.to_str().unwrap()
        ),
    );
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn bad_one() {}\npub fn bad_two() {}\n",
    );
    (dir, scripts)
}

#[test]
fn a_decision_bound_to_another_agent_is_fixed_by_a_run_of_its_own_over_what_the_first_left() {
    let (dir, scripts) = two_agents();

    let fixed = Client::trusting(dir.path())
        .tool("fix", json!({"decision": [BAN, BAN2]}))
        .unwrap();

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn fixed_one() {}\npub fn fixed_two() {}\n",
        "{fixed}"
    );
    assert_eq!(fixed["usage"]["edit"]["runs"], 2, "{fixed}");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 2, "{fixed}");
    let seen = fs::read_to_string(scripts.path().join("seen")).unwrap();
    assert_eq!(
        seen, "pub fn fixed_one() {}\npub fn bad_two() {}\n",
        "the second agent started from the first one's accepted text"
    );
}

#[test]
fn a_decision_is_fixed_by_the_project_agent_without_a_binding_of_its_own() {
    let (dir, scripts) = two_agents();
    fs::remove_file(dir.path().join(LOCAL_FILE)).unwrap();

    let fixed = Client::trusting(dir.path())
        .tool("fix", json!({"decision": [BAN, BAN2]}))
        .unwrap();

    assert_eq!(fixed["usage"]["edit"]["runs"], 1, "{fixed}");
    assert!(!scripts.path().join("seen").exists());
    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn fixed_one() {}\npub fn bad_two() {}\n"
    );
}
