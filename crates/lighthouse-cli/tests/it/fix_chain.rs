//! The runs of an agent in a pass, one after another: each over what the ones
//! before it left, each judged and undone by itself, and the safe fixes of the
//! project applied to what an agent leaves before it is judged. The agent is a
//! small shell script that edits the files of its working directory.

use std::{fs, path::PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    fix_scope::{ALLOW, BAN, FIX_OWN_FILE, WIDE, agent_project, fix, listed, read, with_agent},
    mcp::Client,
    models::UNSANDBOXED,
};

const RUST_FORMATTER: &str = "[languages.rust]\nformatter = { argv = [\"rustfmt\", \"--edition\", \"2024\", \"--emit\", \"stdout\"], stdin = \"file\", output = \"text\" }\n";

/// Two modules with a banned function each, and a shared file.
fn chain_project(agent: &str, rest: &str) -> (TempDir, TempDir) {
    let (dir, scripts) = with_agent(agent, rest);
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(dir.path(), "src/lib.rs", "mod a;\nmod b;\nmod shared;\n");
    write(dir.path(), "src/a.rs", "pub fn bad_one() {}\n");
    write(dir.path(), "src/b.rs", "pub fn bad_one() {}\n");
    write(dir.path(), "src/shared.rs", "// log\n");
    (dir, scripts)
}

#[test]
fn a_later_run_starts_from_what_the_earlier_ones_were_accepted_with() {
    let agent = format!("{FIX_OWN_FILE}echo '// run' >> src/shared.rs\n");
    let (dir, _agent) = chain_project(&agent, &listed("", ALLOW));

    let fixed = fix(&dir, json!({}));

    assert_eq!(read(&dir, "src/a.rs"), "pub fn fixed_one() {}\n");
    assert_eq!(read(&dir, "src/b.rs"), "pub fn fixed_one() {}\n");
    assert_eq!(
        read(&dir, "src/shared.rs"),
        "// log\n// run\n// run\n",
        "the second run saw what the first wrote"
    );
    assert_eq!(fixed["usage"]["edit"]["runs"], 2, "{fixed}");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 2, "{fixed}");
}

#[test]
fn a_failing_verify_rolls_the_newest_run_back_and_keeps_the_ones_before() {
    let verify = "verify = [[\"sh\", \"-c\", \"! grep -q fixed_one src/b.rs\"]]\n";
    let (dir, _agent) = chain_project(FIX_OWN_FILE, &listed(verify, ALLOW));

    let fixed = fix(&dir, json!({}));

    assert_eq!(read(&dir, "src/a.rs"), "pub fn fixed_one() {}\n");
    assert_eq!(read(&dir, "src/b.rs"), "pub fn bad_one() {}\n");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("rolled back"), "{reason}");
    assert_eq!(fixed["models"][BAN]["declined"], json!({"rolled-back": 1}));
}

const BAN2: &str = "local/naming-ban2";

/// A file with a banned function of a decision that keeps its agent to the
/// symbol and one of a decision that lets it go to any file: two runs over
/// one file, the narrower scope first. The agent fixes the function of the
/// decision it is told about.
fn scoped_project(rest: &str) -> (TempDir, TempDir) {
    let agent = "prompt=$(cat)\nfile=src/lib.rs\ncase \"$prompt\" in\n  *\"Decision: local/naming-ban2\"*) sed 's/bad_two/fixed_two/' \"$file\" > \"$file.new\" ;;\n  *) sed 's/bad_one/fixed_one/' \"$file\" > \"$file.new\" ;;\nesac\nmv \"$file.new\" \"$file\"\n";
    let (dir, scripts) = agent_project(
        &format!("\"{BAN2}\" = \"warn\"\n"),
        agent,
        &scoped_models(rest),
    );
    let narrow = WIDE.replace("scope: files", "scope: symbol");
    let second = WIDE
        .replace(BAN, BAN2)
        .replace("bad_one", "bad_two")
        .replace("fixed_one", "fixed_two");
    write(dir.path(), ".lighthouse/decisions/ban.yaml", &narrow);
    write(dir.path(), ".lighthouse/decisions/ban2.yaml", &second);
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn bad_one() {}\npub fn bad_two() {}\n",
    );
    (dir, scripts)
}

/// The `[fix]` table that lists both decisions, with `rest` in it.
fn scoped_models(rest: &str) -> String {
    format!("{ALLOW}[fix]\ndecisions = [\"local/naming-*\"]\n{rest}")
}

fn fix_both(dir: &TempDir) -> Value {
    Client::trusting(dir.path())
        .tool("fix", json!({"decision": [BAN, BAN2]}))
        .unwrap()
}

#[test]
fn findings_of_two_scopes_in_one_file_are_two_runs_each_with_its_own_scope() {
    let (dir, _agent) = scoped_project("");

    let fixed = fix_both(&dir);

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn fixed_one() {}\npub fn fixed_two() {}\n",
        "{fixed}"
    );
    assert_eq!(fixed["usage"]["edit"]["runs"], 2, "{fixed}");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 2, "{fixed}");
}

#[test]
fn two_runs_over_one_file_are_undone_newest_first_and_the_first_stays() {
    let verify = "verify = [[\"sh\", \"-c\", \"! grep -q fixed_two src/lib.rs\"]]\n";
    let (dir, _agent) = scoped_project(verify);

    let fixed = fix_both(&dir);

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn fixed_one() {}\npub fn bad_two() {}\n",
        "{fixed}"
    );
    let applied = fixed["applied"].as_array().unwrap();
    assert_eq!(applied.len(), 1, "{fixed}");
    assert_eq!(applied[0]["decision"], BAN);
    assert_eq!(fixed["models"][BAN]["applied"], 1);
    assert_eq!(fixed["models"][BAN2]["declined"], json!({"rolled-back": 1}));
    let declined = fixed["declined"].as_array().unwrap();
    assert!(
        declined
            .iter()
            .any(|d| d["decision"] == BAN2
                && d["reason"].as_str().unwrap().starts_with("rolled back")),
        "{fixed}"
    );
}

#[test]
fn a_fix_that_leaves_one_of_its_files_as_it_was_is_still_previewed_under_ask() {
    // The formatter writes the second file back as it was.
    let agent = format!(
        "{FIX_OWN_FILE}sed 's/other()/other( )/' src/other.rs > src/other.rs.new && mv src/other.rs.new src/other.rs\n"
    );
    let (dir, _agent) = with_agent(&agent, &format!("{}{RUST_FORMATTER}", listed("", "")));
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(
        dir.path(),
        "src/lib.rs",
        "mod other;\npub fn bad_one() {}\n",
    );
    write(dir.path(), "src/other.rs", "pub fn other() {}\n");

    let fixed = fix(&dir, json!({}));

    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    assert_eq!(fixed["previews"].as_array().unwrap().len(), 1, "{fixed}");
    let counts = &fixed["models"][BAN];
    assert_eq!(counts["verified"], 1, "{fixed}");
    assert_eq!(counts["previewed"], 1, "{fixed}");
    assert_eq!(
        read(&dir, "src/lib.rs"),
        "mod other;\npub fn bad_one() {}\n"
    );
}

/// A project whose agent fixes the banned function and adds a type after it,
/// which `design/declaration-groups` puts before the functions; and the file
/// the agent keeps its brief in.
fn reorder_project(approval: &str) -> (TempDir, TempDir, PathBuf) {
    let scripts = tempfile::tempdir().unwrap();
    let told = scripts.path().join("told");
    let keep = format!(
        "prompt=$(cat)\nprintf '%s\\n' \"$prompt\" > {:?}\n",
        told.to_str().unwrap()
    );
    let agent = format!(
        "{}printf 'pub struct Helper;\\n' >> src/lib.rs\n",
        FIX_OWN_FILE.replacen("prompt=$(cat)\n", &keep, 1)
    );
    write(scripts.path(), "agent.sh", &agent);
    let script = scripts.path().join("agent.sh");
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"core\", \"design\", \"local\"]\n[rules]\n\"local/naming-ban\" = \"warn\"\n\"design/declaration-groups\" = \"error\"\n[models]\nedit = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n{}{}",
            plugin.to_str().unwrap(),
            script.to_str().unwrap(),
            listed("", approval),
            UNSANDBOXED
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");
    (dir, scripts, told)
}

/// Where `needle` first is in `text`.
fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle} in {text}"))
}

#[test]
fn the_safe_fixes_of_the_project_reorder_an_edit_before_it_is_judged() {
    let (dir, _agent, told) = reorder_project(ALLOW);

    let fixed = fix(&dir, json!({}));

    let text = read(&dir, "src/lib.rs");
    assert!(
        at(&text, "pub struct Helper") < at(&text, "pub fn fixed_one"),
        "the type comes before the function:\n{text}"
    );
    assert_eq!(fixed["models"][BAN]["verified"], 1, "{fixed}");
    assert_eq!(fixed["models"][BAN]["applied"], 1, "{fixed}");
    let brief = fs::read_to_string(&told).unwrap();
    assert!(
        brief.contains("The project's tools fix these after you")
            && brief.contains("design/declaration-groups"),
        "{brief}"
    );
}

#[test]
fn the_preview_of_an_edit_shows_the_text_the_safe_fixes_left() {
    let (dir, _agent, _told) = reorder_project("");

    let fixed = fix(&dir, json!({}));

    assert_eq!(read(&dir, "src/lib.rs"), "pub fn bad_one() {}\n");
    let preview = fixed["previews"][0].to_string();
    let (struct_at, fn_at) = (
        at(&preview, "pub struct Helper"),
        at(&preview, "pub fn fixed_one"),
    );
    assert!(struct_at < fn_at, "{preview}");
}
