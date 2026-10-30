//! Fixes by an agent that edits, through the binary: the `fix` tool against a
//! project that binds `edit` to a plain-text command. The agent is a small
//! shell script that edits the files of its working directory, so no real
//! agent is run and no network is used.

use std::{fs, path::Path};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    mcp::Client,
    models::{UNSANDBOXED, project},
    models_plugin::lighthouse,
};

pub(crate) const LIB: &str = "pub fn bad_one() {}\npub fn fine() {}\n";
pub(crate) const FIXED: &str = "pub fn fixed_one() {}\npub fn fine() {}\n";

/// Two functions with the name the decision bans, in modules of their own.
const PAIR: &str = "mod a {\n    pub fn bad_one() {}\n}\nmod b {\n    pub fn bad_one() {}\n}\n";

pub(crate) const OTHER: &str = "pub fn other_one() {}\n";

pub(crate) const LISTED: &str = "[fix]\ndecisions = [\"local/naming-ban\"]\n";

pub(crate) const ALLOWED: &str = "[fix]\nunsafe = true\ndecisions = [\"local/naming-ban\"]\n";

/// The agent that renames the banned function in `src/lib.rs`.
pub(crate) const RENAME: &str =
    "sed 's/bad_one/fixed_one/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\n";

/// A project that binds `edit` to the script `agent`, with `provider` among
/// the lines of the provider and `extra` after the `models` tables; and the
/// directory the script lives in.
pub(crate) fn edited(agent: &str, provider: &str, extra: &str) -> (TempDir, TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    write(scripts.path(), "agent.sh", agent);
    let script = scripts.path().join("agent.sh");
    let dir = project(&format!(
        "[models]\nagent = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n{provider}{extra}",
        script.to_str().unwrap()
    ));
    write(dir.path(), "src/lib.rs", &format!("mod other;\n{LIB}"));
    write(dir.path(), "src/other.rs", OTHER);
    (dir, scripts)
}

pub(crate) fn lib(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("src/lib.rs")).unwrap()
}

fn fix(dir: &Path, arguments: Value) -> Value {
    let mut arguments = arguments;
    arguments["decision"] = json!(["local/naming-ban"]);
    Client::trusting(dir).tool("fix", arguments).unwrap()
}

fn counts(fixed: &Value) -> &Value {
    &fixed["models"]["local/naming-ban"]
}

#[test]
fn an_edit_that_fixes_the_finding_is_applied_when_the_project_approves() {
    let (dir, _agent) = edited(RENAME, "", ALLOWED);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), format!("mod other;\n{FIXED}"));
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    let applied = &fixed["applied"][0];
    assert_eq!(applied["fixer"], "model");
    assert_eq!(applied["safety"], "unsafe");
    assert_eq!(applied["description"], "model edit by `agent/m`");
    assert_eq!(
        counts(&fixed),
        &json!({"asked": 1, "answered": 1, "verified": 1, "applied": 1, "previewed": 0, "declined": {}})
    );
    assert_eq!(fixed["usage"]["calls"], 1, "{fixed}");
    assert_eq!(fixed["usage"]["agent"]["runs"], 1, "{fixed}");
    assert!(fixed["usage"]["agent"]["seconds"].is_number(), "{fixed}");
}

#[test]
fn an_agent_edits_in_its_scratch_copy_with_a_home_of_its_own_and_writes_nowhere_else() {
    let outside = tempfile::tempdir().unwrap();
    let agent = format!(
        "{RENAME}echo \"// home=$HOME\" >> src/lib.rs\ntouch {:?} 2>/dev/null || true\n",
        outside.path().join("escaped")
    );
    let (dir, scripts) = edited(&agent, "", ALLOWED);
    write(dir.path(), ".env", "FAKE-ENV-SECRET\n");
    let reads = format!(
        "{{ cat .env; cat {:?}; }} 2>/dev/null >> src/lib.rs\ntrue\n",
        dir.path().join(".env")
    );
    let script = scripts.path().join("agent.sh");
    fs::write(&script, format!("{agent}{reads}")).unwrap();

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    let text = lib(&dir);
    assert!(text.contains("fixed_one"), "{text}");
    assert!(!text.contains("FAKE-ENV-SECRET"), "the agent read .env");
    let home = text.split("// home=").nth(1).unwrap().trim();
    assert_ne!(home, std::env::var("HOME").unwrap(), "the real home");
    assert!(
        !Path::new(home).exists(),
        "its home is removed with the run"
    );
    assert!(!outside.path().join("escaped").exists(), "wrote outside");
}

#[test]
fn without_approval_a_verified_edit_is_a_preview_and_the_files_are_left_alone() {
    let (dir, _agent) = edited(RENAME, "", LISTED);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"));
    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    let preview = &fixed["previews"][0];
    assert_eq!(preview["files"][0], "src/lib.rs");
    let diff = preview["diff"].as_str().unwrap();
    assert!(
        diff.contains("-pub fn bad_one() {}") && diff.contains("+pub fn fixed_one() {}"),
        "{diff}"
    );
    assert_eq!(counts(&fixed)["verified"], 1);
    assert_eq!(counts(&fixed)["previewed"], 1);
}

#[test]
fn an_edit_outside_the_file_of_the_findings_is_rejected_whole_under_the_scope_file() {
    let agent = format!("{RENAME}echo 'pub fn extra() {{}}' >> src/other.rs\n");
    let narrow = "[fix]\nunsafe = true\nscope = \"file\"\ndecisions = [\"local/naming-ban\"]\n";
    let (dir, _agent) = edited(&agent, "", narrow);

    let fixed = fix(dir.path(), json!({"paths": ["src/lib.rs"]}));

    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/other.rs")).unwrap(),
        OTHER
    );
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("src/other.rs") && reason.contains("scope `file`"),
        "{reason}"
    );
    assert_eq!(counts(&fixed)["declined"], json!({"out-of-scope": 1}));
}

#[test]
fn a_file_the_agent_created_is_rejected_and_named() {
    let outside = format!("{RENAME}echo x > src/new.rs\n");
    let (dir, _agent) = edited(&outside, "", ALLOWED);

    let fixed = fix(dir.path(), json!({"paths": ["src/lib.rs"]}));

    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"));
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("src/new.rs"), "{reason}");
    assert!(!dir.path().join("src/new.rs").exists());

    let inside = format!("{RENAME}echo x > notes.txt\n");
    let (dir, _agent) = edited(&inside, "", ALLOWED);

    let fixed = fix(dir.path(), json!({}));

    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("created `notes.txt`"), "{reason}");
    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"));
}

#[test]
fn an_agent_that_times_out_is_declined_and_its_scratch_copy_is_removed() {
    let marks = tempfile::tempdir().unwrap();
    let cwd = marks.path().join("cwd");
    let agent = format!("pwd > {:?}\nsleep 30\n", cwd.to_str().unwrap());
    let (dir, _agent) = edited(
        &agent,
        "timeout = \"1s\"\n",
        &format!("{ALLOWED}{UNSANDBOXED}"),
    );

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"));
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("no model answered") && reason.contains("timed out"),
        "{reason}"
    );
    assert_eq!(counts(&fixed)["declined"], json!({"unanswered": 1}));
    let scratch = fs::read_to_string(&cwd).unwrap();
    assert!(
        !Path::new(scratch.trim()).exists(),
        "{scratch} is still there"
    );
}

#[test]
fn a_failing_agent_leaves_no_scratch_copy_either() {
    let marks = tempfile::tempdir().unwrap();
    let cwd = marks.path().join("cwd");
    let agent = format!("pwd > {:?}\nexit 3\n", cwd.to_str().unwrap());
    let (dir, _agent) = edited(&agent, "", &format!("{ALLOWED}{UNSANDBOXED}"));

    let fixed = fix(dir.path(), json!({}));

    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("exited 3"), "{reason}");
    let scratch = fs::read_to_string(&cwd).unwrap();
    assert!(!Path::new(scratch.trim()).exists());
}

#[test]
fn a_partial_fix_keeps_what_was_fixed_and_declines_what_is_left() {
    let agent = "awk '!done && /bad_one/ { sub(/bad_one/, \"fixed_one\"); done = 1 } { print }' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\n";
    let (dir, _agent) = edited(agent, "", ALLOWED);
    write(dir.path(), "src/lib.rs", &format!("mod other;\n{PAIR}"));

    let fixed = fix(dir.path(), json!({}));

    let text = lib(&dir);
    assert_eq!(text.matches("fixed_one").count(), 1, "{text}");
    assert_eq!(text.matches("bad_one").count(), 1, "{text}");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    assert_eq!(fixed["declined"].as_array().unwrap().len(), 1, "{fixed}");
    assert_eq!(
        fixed["declined"][0]["reason"],
        "did not resolve the finding"
    );
    assert_eq!(
        counts(&fixed),
        &json!({"asked": 2, "answered": 2, "verified": 1, "applied": 1, "previewed": 0, "declined": {"unresolved": 1}})
    );
    assert_eq!(fixed["usage"]["agent"]["runs"], 1, "one run for the file");
}

#[test]
fn a_second_run_takes_the_edit_from_the_cache_and_runs_no_agent() {
    let marks = tempfile::tempdir().unwrap();
    let calls = marks.path().join("calls");
    let agent = format!("{RENAME}echo x >> {:?}\n", calls.to_str().unwrap());
    let (dir, _agent) = edited(&agent, "", &format!("{LISTED}{UNSANDBOXED}"));

    let first = fix(dir.path(), json!({}));
    assert_eq!(first["usage"]["calls"], 1, "{first}");
    assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 1);

    let second = fix(dir.path(), json!({}));

    assert_eq!(second["usage"]["calls"], 0, "{second}");
    assert_eq!(second["usage"]["agent"]["runs"], 0, "{second}");
    assert_eq!(
        fs::read_to_string(&calls).unwrap().lines().count(),
        1,
        "no second run"
    );
    assert_eq!(second["previews"], first["previews"]);
}

#[test]
fn an_untrusted_project_declines_with_the_trust_message() {
    let (dir, _agent) = edited(RENAME, "", ALLOWED);

    let fixed = Client::start(dir.path())
        .tool("fix", json!({"decision": ["local/naming-ban"]}))
        .unwrap();

    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"));
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("lighthouse trust"), "{reason}");
}

#[test]
fn only_a_command_without_a_protocol_can_be_bound_to_edit() {
    for (provider, wanted) in [
        (
            "protocol = \"openai\"\nurl = \"http://127.0.0.1:9\"\n",
            "provider `fake` does not serve `agent`: only a command without a protocol, or a plugin that offers agents, does",
        ),
        (
            "protocol = \"typesafe\"\ncommand = [\"cat\"]\n",
            "provider `fake` does not serve `agent`: only a command without a protocol, or a plugin that offers agents, does",
        ),
        (
            "protocol = \"openai\"\ncommand = [\"cat\"]\n",
            "a `command` speaks `typesafe` (classification) or plain text (agent)",
        ),
    ] {
        let dir = project(&format!(
            "[models]\nagent = \"fake/any\"\n[models.providers.fake]\n{provider}"
        ));
        let config = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
        let plugin = lighthouse_test_support::model_http();
        let listed = format!(
            "{{ id = \"model-http\", path = {:?} }}",
            plugin.to_str().unwrap()
        );
        fs::write(
            dir.path().join("lighthouse.toml"),
            lighthouse_test_support::listing(&config, &listed),
        )
        .unwrap();

        let out = lighthouse(dir.path(), &["check", "--no-store", "--no-cache"]);

        assert!(!out.status.success(), "{provider}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(wanted), "{stderr}");
    }
}

#[test]
fn the_recipes_skip_no_permission_check() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/models.md");
    let text = fs::read_to_string(docs).unwrap();
    let mut recipes: Vec<&str> = text.split("```").skip(1).step_by(2).collect();
    let shipped: Vec<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes"))
            .unwrap()
            .map(|dir| fs::read_to_string(dir.unwrap().path().join("recipe.toml")).unwrap())
            .collect();
    recipes.extend(shipped.iter().map(String::as_str));

    assert!(recipes.iter().any(|r| r.contains("\"claude\", \"-p\"")));
    assert!(recipes.iter().any(|r| r.contains("\"codex\", \"exec\"")));
    assert!(recipes.iter().any(|r| r.contains("\"opencode\", \"run\"")));
    for flag in [
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
        "bypassPermissions",
        "--yolo",
        "--full-auto",
        "--auto",
        "danger-full-access",
        "--dangerously-bypass-approvals-and-sandbox",
    ] {
        assert!(
            recipes.iter().all(|recipe| !recipe.contains(flag)),
            "a recipe uses {flag}"
        );
    }
}

/// The project says the fix of the decision is `safe`: the agent is trusted
/// here, whatever `[fix] unsafe` says.
fn trusted_agent(rest: &str) -> (TempDir, TempDir) {
    let (dir, scripts) = edited(RENAME, "", rest);
    let config = dir.path().join("lighthouse.toml");
    let text = fs::read_to_string(&config).unwrap();
    let text = text.replace(
        "\"local/naming-ban\" = \"warn\"",
        "\"local/naming-ban\" = { level = \"warn\", safety = \"safe\" }",
    );
    fs::write(&config, text).unwrap();
    (dir, scripts)
}

#[test]
fn an_edit_the_project_made_safe_is_written_without_unsafe_fixes() {
    let (dir, _agent) = trusted_agent(LISTED);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), format!("mod other;\n{FIXED}"), "{fixed}");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    assert_eq!(fixed["applied"][0]["safety"], "safe", "{fixed}");
    assert!(fixed["previews"].is_null(), "{fixed}");
}

#[test]
fn an_edit_made_safe_that_the_checks_of_the_project_reject_is_not_reported_applied() {
    let verify = "verify = [[\"sh\", \"-c\", \"! grep -q fixed_one src/lib.rs\"]]\n";
    let (dir, _agent) = trusted_agent(&format!(
        "[fix]\ndecisions = [\"local/naming-ban\"]\n{verify}"
    ));

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), format!("mod other;\n{LIB}"), "{fixed}");
    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    assert_eq!(fixed["declined"].as_array().unwrap().len(), 1, "{fixed}");
}
