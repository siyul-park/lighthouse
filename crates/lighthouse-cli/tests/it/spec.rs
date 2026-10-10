//! `lighthouse spec validate`, `spec migrate` and `schema` through the binary,
//! and the schemas checked in under `schema/`.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const OLD_CONFIG: &str = "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n[rules]\n\"core/max-file-lines\" = { level = \"review\", max = 2 }\n";

const OLD_RULE: &str = "id: local/long
title: Long
intent: Long files are hard to read.
scope: file
requirement: A file MUST have at most three lines.
enforcement: mechanical
evidence: [path]
examples:
  - name: long
    language: text
    kind: invalid
    files: [{ path: a.txt, body: \"1\\n2\\n3\\n4\" }]
    expect: [{ line: 1 }]
  - name: short
    language: text
    kind: valid
    files: [{ path: a.txt, body: \"1\" }]
rule:
  select: file
  where: 'file.lines > 3'
  message: too long
";

#[test]
fn migrate_rewrites_old_documents_once_and_validate_accepts_the_result() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.toml", OLD_CONFIG);
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 file(s) written, 1 removed"));

    assert!(!dir.path().join(".lighthouse/rules/long.yaml").exists());
    let decision = fs::read_to_string(dir.path().join(".lighthouse/decisions/long.yaml")).unwrap();
    assert!(decision.contains("kind: Decision"), "{decision}");
    assert!(decision.contains("type: cel"), "{decision}");
    let config = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    assert!(config.contains("kind = \"Project\""), "{config}");
    assert!(
        config.contains("level = \"info\""),
        "review is info now: {config}"
    );

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written, 0 removed"));
    lighthouse(dir.path())
        .args(["spec", "validate", "--examples"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 problem(s)"));
}

const OTHER_RULE: &str = "id: local/short
title: Short
intent: Short files are easy to read.
scope: file
requirement: A file MUST have at least one line.
enforcement: mechanical
rule:
  select: file
  where: 'file.lines < 1'
  message: empty
";

#[test]
fn migrate_applies_every_write_before_it_removes_anything() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);
    // The target directory cannot be made: nothing may have been removed.
    write(dir.path(), ".lighthouse/decisions", "in the way");

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .failure();

    let rule = dir.path().join(".lighthouse/rules/long.yaml");
    assert_eq!(fs::read_to_string(rule).unwrap(), OLD_RULE);
}

const PROJECT: &str = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: demo\nspec:\n  plugins: [core]\n  extends: [core/recommended]\n";

const PRESET: &str = "apiVersion: lighthouse/v1alpha1\nkind: Preset\nmetadata:\n  name: team/strict\nspec:\n  extends: [core/recommended]\n  rules:\n    core/max-file-lines: error\n";

fn override_of(spec: &str) -> String {
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: DecisionOverride\nmetadata:\n  name: tweak\nspec:\n{spec}"
    )
}

#[test]
fn migrate_turns_a_preset_into_a_project() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "presets/strict.yaml", PRESET);

    lighthouse(dir.path())
        .args(["spec", "migrate", "presets/strict.yaml"])
        .assert()
        .success();

    let text = fs::read_to_string(dir.path().join("presets/strict.yaml")).unwrap();
    assert!(text.contains("kind: Project"), "{text}");
    assert!(text.contains("core/max-file-lines: error"), "{text}");
    lighthouse(dir.path())
        .args(["spec", "migrate", "presets/strict.yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written, 0 removed"));
}

#[test]
fn migrate_folds_an_override_into_the_rules_of_the_project() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.yaml", PROJECT);
    write(
        dir.path(),
        ".lighthouse/decisions/tweak.yaml",
        &override_of(
            "  extends: core/max-file-lines\n  severity: error\n  options: { max: 300 }\n",
        ),
    );

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success();

    assert!(!dir.path().join(".lighthouse/decisions/tweak.yaml").exists());
    let project = fs::read_to_string(dir.path().join("lighthouse.yaml")).unwrap();
    assert!(project.contains("core/max-file-lines"), "{project}");
    assert!(project.contains("level: error"), "{project}");
    assert!(project.contains("max: 300"), "{project}");
    lighthouse(dir.path())
        .args(["spec", "validate"])
        .assert()
        .success();
}

#[test]
fn migrate_keeps_an_override_that_needs_a_local_decision_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.yaml", PROJECT);
    let wording = override_of("  extends: core/max-file-lines\n  exceptions: Vendored code.\n");
    write(dir.path(), ".lighthouse/decisions/tweak.yaml", &wording);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stderr(predicate::str::contains("write a local decision"));

    let kept = fs::read_to_string(dir.path().join(".lighthouse/decisions/tweak.yaml")).unwrap();
    assert_eq!(kept, wording);
}

const OUTDATED: &str = "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/long
spec:
  title: Long
  intent: Long files are hard to read.
  scope: { subject: file }
  requirement: A file MUST have at most three lines.
  severity: warn
  evidence: [path]
  exceptions: Vendored code is exempt.
  citation: Fowler 1999
  strict: true
  options:
    type: object
    properties:
      max_lines:
        type: integer
        default: 3
        description: The most lines a file may have.
    additionalProperties: false
  languages:
    go:
      options: { max_lines: 5 }
      tuning: Count Go lines.
  check:
    type: cel
    select: file
    where: 'file.lines > options.max_lines'
    message: too long
    evidence: { path: file.path }
  examples:
    - name: long
      language: text
      kind: invalid
      files: [{ path: a.txt, body: \"1\\n2\\n3\\n4\" }]
      expect: [{ line: 1 }]
    - name: short
      language: text
      kind: valid
      files: [{ path: a.txt, body: \"1\" }]
";

#[test]
fn migrate_rewrites_the_fields_revision_28_removed_and_camel_cases_options() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.yaml", PROJECT);
    write(dir.path(), ".lighthouse/decisions/long.yaml", OUTDATED);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success();

    let text = fs::read_to_string(dir.path().join(".lighthouse/decisions/long.yaml")).unwrap();
    for gone in [
        "intent:",
        "evidence: [",
        "\n  exceptions:",
        "citation:",
        "strict:",
        "tuning:",
        "select:",
    ] {
        assert!(!text.contains(gone), "{gone} survived:\n{text}");
    }
    for kept in [
        "context:",
        "Go: Count Go lines.",
        "Vendored code is exempt.",
        "wasDerivedFrom",
        "lighthouse/preset: strict",
        "lighthouse/was-exceptions",
        "maxLines",
        "options.maxLines",
    ] {
        assert!(text.contains(kept), "{kept} missing:\n{text}");
    }
    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written, 0 removed"));
    lighthouse(dir.path())
        .args(["spec", "validate", "--examples"])
        .assert()
        .success();
}

#[test]
fn migrate_renames_the_options_a_project_sets_to_camel_case() {
    let dir = tempfile::tempdir().unwrap();
    let project = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: demo\nspec:\n  plugins: [core, design]\n  rules:\n    design/coupling-signal: { level: warn, options: { hub_fan_in: 9 } }\n";
    write(dir.path(), "lighthouse.yaml", project);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success();

    let text = fs::read_to_string(dir.path().join("lighthouse.yaml")).unwrap();
    assert!(text.contains("hubFanIn"), "{text}");
    assert!(!text.contains("hub_fan_in"), "{text}");
}

/// A local decision in the shape before Revision 28 with the option names
/// that `camel` and `snake_case` do not map back and forth: `p_95`, a name
/// that is camelCase already and `a_b` next to `a_b_c`.
const SNAKY: &str = "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/snaky
spec:
  title: Snaky
  context: Options with awkward names.
  scope: { subject: file }
  requirement: A file MUST have few lines.
  severity: warn
  options:
    type: object
    properties:
      p_95:
        type: integer
        default: 3
        description: The `p_95` limit.
      fooBar:
        type: integer
        default: 1
        description: Left as it is.
      a_b:
        type: integer
        default: 1
        description: Short name.
      a_b_c:
        type: integer
        default: 2
        description: Long name.
    additionalProperties: false
  check:
    type: cel
    where: 'file.lines > options.p_95 + options.fooBar + options.a_b + options.a_b_c + options[\"a_b\"]'
    message: too long
  examples:
    - name: long
      language: text
      kind: invalid
      files: [{ path: a.txt, body: \"1\\n2\\n3\\n4\\n5\\n6\\n7\\n8\\n9\" }]
      expect: [{ line: 1 }]
    - name: short
      language: text
      kind: valid
      files: [{ path: a.txt, body: \"1\" }]
";

#[test]
fn migrating_option_names_keeps_the_meaning_version_of_a_local_decision() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.yaml", PROJECT);
    let path = ".lighthouse/decisions/snaky.yaml";
    write(dir.path(), path, SNAKY);
    let before = |text: &str| {
        let layer = lighthouse_spec::Catalog::from_local(
            [("snaky.yaml".to_owned(), text.to_owned())].into(),
        )
        .unwrap();
        layer.decision("local/snaky").unwrap().clone()
    };
    let old = before(SNAKY).meaning_version();

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success();

    let text = fs::read_to_string(dir.path().join(path)).unwrap();
    let migrated = before(&text);
    assert_eq!(migrated.earlier_meaning_version(), old, "{text}");
    assert_ne!(migrated.meaning_version(), old);
    for renamed in [
        "p95",
        "aB:",
        "aBC:",
        "options.aBC",
        "options[\"aB\"]",
        "fooBar",
    ] {
        assert!(text.contains(renamed), "{renamed} missing:\n{text}");
    }
    let spec = text.split("\nspec:").nth(1).unwrap();
    assert!(!spec.contains("a_b"), "{text}");
    assert!(text.contains("`p95` limit"), "{text}");
    lighthouse(dir.path())
        .args(["spec", "validate", "--examples"])
        .assert()
        .success();
}

#[test]
fn migrate_renames_the_options_a_project_sets_for_its_local_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let project = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: demo\nspec:\n  plugins: [core, local]\n  rules:\n    local/snaky: { level: warn, options: { p_95: 7, a_b_c: 4 } }\n    core/max-file-lines: { level: warn, options: { max: 9, mAx: 1 } }\n    design/coupling-signal: { level: warn, options: { hub_fan_in: 1, hubFanIn: 2 } }\n    other/unknown: { level: warn, options: { some_thing: 1 } }\n";
    write(dir.path(), "lighthouse.yaml", project);
    write(dir.path(), ".lighthouse/decisions/snaky.yaml", SNAKY);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "sets both `hub_fan_in` and `hubFanIn`",
        ))
        .stderr(predicate::str::contains("no decision of that id is known"));

    let text = fs::read_to_string(dir.path().join("lighthouse.yaml")).unwrap();
    assert!(text.contains("p95: 7") && text.contains("aBC: 4"), "{text}");
    assert!(
        text.contains("hubFanIn: 2") && !text.contains("hub_fan_in"),
        "{text}"
    );
    assert!(text.contains("some_thing"), "{text}");
}

#[test]
fn migrate_converts_the_options_of_a_preset() {
    let dir = tempfile::tempdir().unwrap();
    let preset = "apiVersion: lighthouse/v1alpha1\nkind: Preset\nmetadata:\n  name: team/strict\nspec:\n  rules:\n    design/coupling-signal: { level: warn, options: { hub_fan_in: 3 } }\n";
    write(dir.path(), "presets/strict.yaml", preset);

    lighthouse(dir.path())
        .args(["spec", "migrate", "presets/strict.yaml"])
        .assert()
        .success();

    let text = fs::read_to_string(dir.path().join("presets/strict.yaml")).unwrap();
    assert!(
        text.contains("kind: Project") && text.contains("hubFanIn: 3"),
        "{text}"
    );
}

#[test]
fn folding_an_override_keeps_what_the_rule_already_says_and_reports_disagreement() {
    let dir = tempfile::tempdir().unwrap();
    let project = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: demo\nspec:\n  plugins: [core]\n  rules:\n    core/max-file-lines: { level: warn, generated: true }\n";
    write(dir.path(), "lighthouse.yaml", project);
    write(
        dir.path(),
        ".lighthouse/decisions/one.yaml",
        &override_of("  extends: core/max-file-lines\n  options: { max: 300 }\n"),
    );
    write(
        dir.path(),
        ".lighthouse/decisions/two.yaml",
        &override_of("  extends: core/max-file-lines\n  options: { max: 400 }\n"),
    );

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stderr(predicate::str::contains("disagree"));

    let text = fs::read_to_string(dir.path().join("lighthouse.yaml")).unwrap();
    assert!(text.contains("generated: true"), "{text}");
    assert!(text.contains("max: 400"), "{text}");
}

#[test]
fn migrate_refuses_a_file_of_several_documents_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let two = format!("{OLD_RULE}---\n{OTHER_RULE}");
    write(dir.path(), ".lighthouse/rules/two.yaml", &two);
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("holds 2 documents"));

    assert_eq!(
        fs::read_to_string(dir.path().join(".lighthouse/rules/two.yaml")).unwrap(),
        two
    );
    assert!(dir.path().join(".lighthouse/rules/long.yaml").exists());
    assert!(!dir.path().join(".lighthouse/decisions").exists());
}

#[test]
fn migrate_moves_a_rules_directory_without_a_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file(s) written, 1 removed"));

    assert!(!dir.path().join(".lighthouse").join("rules").exists());
    assert!(dir.path().join(".lighthouse/decisions/long.yaml").exists());
}

#[test]
fn migrate_keeps_a_rule_file_no_decision_consumed_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = "select: file\nwhere: 'true'\nmessage: m\n";
    write(dir.path(), ".lighthouse/rules/orphan.yaml", orphan);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stderr(predicate::str::contains("orphan.yaml: kept"));

    let kept = fs::read_to_string(dir.path().join(".lighthouse/rules/orphan.yaml")).unwrap();
    assert_eq!(kept, orphan);
}

#[test]
fn migrate_resolves_a_declared_rule_by_path_not_by_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let rule = "select: file\nwhere: 'true'\nmessage: m\n";
    write(
        dir.path(),
        "cat/core/pack.yaml",
        "id: core\ntitle: Core\nintro: i\nsections: [s]\n",
    );
    write(
        dir.path(),
        "cat/core/s/section.yaml",
        "id: s\ntitle: S\nintro: i\npatterns: [a]\n",
    );
    write(
        dir.path(),
        "cat/core/s/a.yaml",
        "id: core/a\ntitle: A\nintent: i\nscope: file\nrequirement: A MUST b.\nenforcement: mechanical\nimplementation:\n  declarative: core/s/rules/a.yaml\n",
    );
    write(dir.path(), "cat/core/s/rules/a.yaml", rule);
    // Its path ends with the declared one but is another file.
    write(dir.path(), "cat/core/s/extra/core/s/rules/a.yaml", rule);

    lighthouse(dir.path())
        .args(["spec", "migrate", "cat"])
        .assert()
        .success()
        .stderr(predicate::str::contains("extra/core/s/rules/a.yaml: kept"));

    assert!(!dir.path().join("cat/core/s/rules/a.yaml").exists());
    assert!(!dir.path().join("cat/core/s/section.yaml").exists());
    assert!(
        dir.path()
            .join("cat/core/s/extra/core/s/rules/a.yaml")
            .exists()
    );
    let decision = fs::read_to_string(dir.path().join("cat/core/s/a.yaml")).unwrap();
    assert!(decision.contains("type: cel"), "{decision}");
}

#[test]
fn migrate_leaves_other_tools_files_alone() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = [
        (
            ".eslintrc.yml",
            "extends: [eslint:recommended]\nrules: {}\n",
        ),
        ("ci/workflow.yaml", "id: build\nrequirement: none\n"),
        ("docker-compose.yml", "services: {}\n"),
        ("lighthouse.json", "{\"name\": \"not ours\"}\n"),
    ];
    for (name, text) in foreign {
        write(dir.path(), name, text);
    }
    write(dir.path(), "lighthouse.toml", OLD_CONFIG);

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file(s) written, 0 removed"));

    for (name, text) in foreign {
        assert_eq!(fs::read_to_string(dir.path().join(name)).unwrap(), text);
    }
}

#[test]
fn migrate_refuses_keys_it_does_not_know_instead_of_dropping_them() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = format!("{OLD_RULE}bogus: 1\n");
    write(dir.path(), ".lighthouse/rules/long.yaml", &bogus);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown key `bogus`"));
    assert!(dir.path().join(".lighthouse/rules/long.yaml").exists());
}

#[test]
fn a_project_that_still_has_legacy_rules_must_migrate_before_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("plugins = [\"core\"]\n"),
    );
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);
    write(dir.path(), "a.txt", "1\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .failure()
        .stderr(predicate::str::contains("lighthouse spec migrate"));
}

#[test]
fn a_dry_run_of_migrate_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.toml", OLD_CONFIG);

    lighthouse(dir.path())
        .args(["spec", "migrate", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("would write"));

    assert_eq!(
        fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap(),
        OLD_CONFIG
    );
}

#[test]
fn validate_names_what_is_wrong_across_documents() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\nextends = [\"core/nope\"]\n[rules]\n\"core/nonesuch\" = \"warn\"\n",
        ),
    );
    write(
        dir.path(),
        ".lighthouse/decisions/old.yaml",
        "id: local/old\n",
    );

    lighthouse(dir.path())
        .args(["spec", "validate"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "extends unknown project `core/nope`",
        ))
        .stdout(predicate::str::contains(
            "rule `core/nonesuch` is not registered",
        ))
        .stdout(predicate::str::contains("lighthouse spec migrate"));
}

const UNIDENTIFIED: &str = "# yaml-language-server: $schema=../../schema/decision.schema.json
apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/plain # keep this comment
  labels: {}
spec:
  title: Plain
  context: A plain decision.
  scope: { subject: file }
  requirement: A file MAY be plain.
";

fn uid_of(text: &str) -> String {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("uid: "))
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn migrate_gives_every_decision_a_uid_once_and_keeps_the_rest_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/decisions/plain.yaml", UNIDENTIFIED);

    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file(s) written"));

    let path = dir.path().join(".lighthouse/decisions/plain.yaml");
    let migrated = fs::read_to_string(&path).unwrap();
    let uid = uid_of(&migrated);
    assert!(lighthouse_resource::is_uid(&uid), "{migrated}");
    assert_eq!(
        migrated.replace(&format!("  uid: {uid}\n"), ""),
        UNIDENTIFIED,
        "only the uid line is new"
    );

    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written"));
    assert_eq!(uid_of(&fs::read_to_string(&path).unwrap()), uid);
}

#[test]
fn migrate_rewrites_allow_comments_in_source_files_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/lib.rs",
        "// lighthouse:allow design/exported-doc -- shim\npub fn open() {}\n\n/* lighthouse:allow a, b -- two */\nfn quoted() -> &'static str {\n    \"// lighthouse:allow x -- in a string\"\n}\n// Write lighthouse:allow x -- in prose\n",
    );
    write(
        dir.path(),
        "main.go",
        "package main\n\n\t// lighthouse:allow design/exported-doc\nfunc Open() {}\n",
    );
    write(dir.path(), "NOTES.md", "lighthouse:allow design/a -- doc\n");

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success();

    let rust = fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        rust.starts_with("// lighthouse-disable-next-line design/exported-doc -- shim\n"),
        "{rust}"
    );
    assert!(
        rust.contains("/* lighthouse-disable-next-line a, b -- two */"),
        "{rust}"
    );
    assert!(
        rust.contains("\"// lighthouse:allow x -- in a string\""),
        "{rust}"
    );
    assert!(
        rust.contains("// Write lighthouse:allow x -- in prose"),
        "{rust}"
    );
    let go = fs::read_to_string(dir.path().join("main.go")).unwrap();
    assert!(
        go.contains("\t// lighthouse-disable-next-line design/exported-doc\n"),
        "{go}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("NOTES.md")).unwrap(),
        "lighthouse:allow design/a -- doc\n"
    );

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written"));
}

/// A local decision named `name`, with a uid, as `migrate` leaves one.
fn named(name: &str) -> String {
    UNIDENTIFIED
        .replace(
            "name: local/plain # keep this comment",
            &format!("name: local/{name}\n  uid: 3f1c4a52-9b0e-4e6a-8f55-6a1d0c2b7e90"),
        )
        .replace("# keep this comment", "")
}

#[test]
fn check_and_validate_hold_decision_names_to_the_convention() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n",
        ),
    );
    write(
        dir.path(),
        ".lighthouse/decisions/long.yaml",
        &named("order-is-not-a-reason"),
    );

    lighthouse(dir.path())
        .args(["check", "--no-store"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            ".lighthouse/decisions/long.yaml:5:9: warn core/decision-naming: decision name `local/order-is-not-a-reason` has 5 words, at most 3; uses `is`",
        ));
    lighthouse(dir.path())
        .args(["spec", "validate", ".lighthouse/decisions"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "long.yaml:5: decision name `local/order-is-not-a-reason`",
        ));

    write(
        dir.path(),
        ".lighthouse/decisions/long.yaml",
        &named("order"),
    );
    lighthouse(dir.path())
        .args(["check", "--no-store"])
        .assert()
        .success()
        .stdout("");
    lighthouse(dir.path())
        .args(["spec", "validate", ".lighthouse/decisions"])
        .assert()
        .success();
}

#[test]
fn validate_asks_a_decision_without_a_uid_to_be_migrated() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/decisions/plain.yaml", UNIDENTIFIED);

    lighthouse(dir.path())
        .args(["spec", "validate", ".lighthouse/decisions"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "local/plain: has no `metadata.uid`",
        ));

    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success();
    lighthouse(dir.path())
        .args(["spec", "validate", ".lighthouse/decisions"])
        .assert()
        .success();
}

#[test]
fn schema_prints_a_kind_in_any_case_and_lists_them() {
    let dir = tempfile::tempdir().unwrap();
    let out = lighthouse(dir.path())
        .args(["schema", "decision"])
        .output()
        .unwrap();
    let schema: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["kind"]["const"], "Decision");

    lighthouse(dir.path())
        .arg("schema")
        .assert()
        .success()
        .stdout(predicate::str::contains("Project\tproject.schema.json"));
    lighthouse(dir.path())
        .args(["schema", "nope"])
        .assert()
        .code(2);
}

/// The checked-in schemas are what editors validate documents against, so
/// they must track the Rust types. `UPDATE_SCHEMA=1 cargo test -p
/// lighthouse-cli --test spec` rewrites them.
#[test]
fn checked_in_schemas_match_the_types() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        lighthouse(root.parent().unwrap())
            .args(["schema", "--write", "schema"])
            .assert()
            .success();
    }
    let schemas = lighthouse_session::schemas();
    assert_eq!(schemas.len(), 7);
    let on_disk = fs::read_dir(&root)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy().ends_with(".schema.json")
        })
        .count();
    assert_eq!(on_disk, 7, "a schema file nothing generates is stale");
    for descriptor in schemas.into_values() {
        let file = root.join(lighthouse_resource::schema_file(descriptor.kind));
        let mut want = serde_json::to_string_pretty(&descriptor.schema).unwrap();
        want.push('\n');
        let have = fs::read_to_string(&file).unwrap_or_default();
        assert!(
            have == want,
            "{} is stale; run `UPDATE_SCHEMA=1 cargo test -p lighthouse-cli --test spec`",
            file.display()
        );
    }
}

/// A fix refuses the fields its `type` does not have, so does the schema.
#[test]
fn the_fix_schema_allows_only_the_fields_of_its_type() {
    let schema = lighthouse_session::schema_of("decision").unwrap().schema;
    let branches = schema["$defs"]["Fix"]["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 3);
    for branch in branches {
        assert_eq!(branch["additionalProperties"], false, "{branch}");
        for field in ["safety", "requires"] {
            assert!(branch["properties"][field].is_object(), "{field}: {branch}");
        }
        assert!(
            branch["required"]
                .as_array()
                .unwrap()
                .contains(&"safety".into()),
            "{branch}"
        );
    }
}

#[test]
fn max_warnings_fails_a_run_with_more_warnings_than_allowed() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 2 } }\n",
        ),
    );
    write(dir.path(), "a.txt", "1\n2\n3\n");
    lighthouse(dir.path()).arg("check").assert().code(0);
    lighthouse(dir.path())
        .args(["check", "--max-warnings", "1"])
        .assert()
        .code(1);
    lighthouse(dir.path())
        .args(["check", "--max-warnings", "2"])
        .assert()
        .code(0);
    lighthouse(dir.path())
        .args(["check", "--strict"])
        .assert()
        .code(1);
}

#[test]
fn sarif_links_a_rule_to_its_decision_page() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 2 } }\n",
        ),
    );
    write(dir.path(), "a.txt", "1\n2\n3\n");
    let out = lighthouse(dir.path())
        .args(["check", "--format", "sarif"])
        .output()
        .unwrap();
    let sarif: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rule = &sarif["runs"][0]["tool"]["driver"]["rules"][0];
    assert!(
        rule["helpUri"]
            .as_str()
            .unwrap()
            .ends_with("docs/decisions/core.md#files-stay-below-a-line-limit")
    );
    assert_eq!(sarif["runs"][0]["results"][0]["level"], "warning");
}
