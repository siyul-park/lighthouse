//! Fixing the findings a request selects, and telling what was done.

use lighthouse_session::fix::{self, Fixed, Selection, Targets};

use crate::fixture::{LONG, project, read, trusted, write};

fn everything(root: &std::path::Path, dry_run: bool) -> Selection {
    Selection {
        targets: Targets {
            paths: vec![root.to_owned()],
            ..Targets::default()
        },
        dry_run,
        ..Selection::default()
    }
}

fn fixed(dry_run: bool) -> (tempfile::TempDir, Fixed) {
    let dir = project("error");
    write(dir.path(), "b.txt", LONG);
    let session = trusted(dir.path());
    let done = fix::run(&session, &everything(&session.root, dry_run)).unwrap();
    (dir, done)
}

#[test]
fn fixed_lines() {
    let (dir, dry) = fixed(true);
    let lines = dry.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("would fix local/short [safe] a.txt")),
        "{lines:?}"
    );
    assert_eq!(read(dir.path(), "a.txt"), LONG, "a dry run changes nothing");

    let (dir, done) = fixed(false);
    let lines = done.lines();
    assert_eq!(done.applied.len(), 2);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("fixed local/short [safe] b.txt"))
    );
    assert_eq!(read(dir.path(), "a.txt"), "a\nb\n");
    assert_eq!(read(dir.path(), "b.txt"), "a\nb\n");
}

#[test]
fn fixed_properties() {
    let (_, done) = fixed(true);
    // No agent ran, so there is nothing to put in a property bag.
    assert!(done.properties().is_empty());
    assert!(done.usage.is_none() && done.models.is_empty());

    let (_dir, _scripts, done) = agent_run(CUT, true);
    let bag = done.properties();
    assert_eq!(bag["agent"]["runs"], 1, "{bag:?}");
    assert_eq!(bag["agent"]["batches"][0]["file"], "a.txt");
}

/// A decision whose fix is made by a model: the note is cut to two lines.
const LONG_NOTE: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/long-note
  uid: 5b8e0c2d-4f6a-4b1c-8d3e-7a9f0b1c2d3e
spec:
  title: Notes are short
  context: Notes are read at a glance.
  scope:
    subject: file
  requirement: A note MUST NOT run past two lines.
  severity: warn
  check:
    type: cel
    select: file
    where: "file.path.endsWith(\".txt\") && file.lines > 2"
    message: "{{ file.path }} is too long"
  fix:
    type: model
    safety: unsafe
    scope: files
    prompt: Cut the note to two lines.
  examples:
    - name: long
      language: text
      kind: invalid
      files: [{ path: a.txt, body: "a\nb\nc\n" }]
      expect: [{ line: 1 }]
      fixed: [{ path: a.txt, body: "a\nb\n" }]
    - name: short
      language: text
      kind: valid
      files: [{ path: a.txt, body: "a\nb\n" }]
"#;

/// An agent that cuts the file of the finding (the first `Where:` line of its
/// brief) to two lines and prints what it did.
const CUT: &str = "prompt=$(cat)
file=$(printf '%s\\n' \"$prompt\" | sed -n 's/^Where: \\([^:]*\\):.*/\\1/p' | head -1)
head -n 2 \"$file\" > \"$file.new\" && mv \"$file.new\" \"$file\"
echo \"{\\\"did\\\":\\\"open\\\",\\\"file\\\":\\\"$PWD/$file\\\"}\"
echo \"{\\\"did\\\":\\\"open\\\",\\\"file\\\":\\\"$PWD/other.txt\\\"}\"
";

/// An agent that changes nothing.
const IDLE: &str = "cat > /dev/null\n";

const EVENTS: &str = "[models.providers.agent.events]
select = 'line.?did.orValue(\"\") != \"\"'
tool = 'tools[line.did]'
path = 'line.?file.orValue(\"\")'
tools = { open = \"read\" }
";

/// A project that lists `local/long-note` for a model to fix and binds the
/// agent to the script, with `[fix] unsafe` as given.
fn agent_project(script: &str, allow_unsafe: bool) -> (tempfile::TempDir, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    write(scripts.path(), "agent.sh", script);
    let dir = project("warn");
    write(
        dir.path(),
        ".lighthouse/decisions/long-note.yaml",
        LONG_NOTE,
    );
    write(dir.path(), "other.txt", "x\n");
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "[rules]\n\"local/long-note\" = \"warn\"\n[fix]\nunsafe = {allow_unsafe}\ndecisions = [\"local/long-note\"]\n[models]\nagent = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n{EVENTS}[sandbox]\nenabled = false\n",
            scripts.path().join("agent.sh").to_str().unwrap()
        )),
    );
    // The short note is `local/short`'s, not the model's.
    std::fs::remove_file(dir.path().join(".lighthouse/decisions/short.yaml")).unwrap();
    (dir, scripts)
}

fn agent_run(script: &str, allow_unsafe: bool) -> (tempfile::TempDir, tempfile::TempDir, Fixed) {
    let (dir, scripts) = agent_project(script, allow_unsafe);
    let session = trusted(dir.path());
    let done = fix::run(&session, &everything(&session.root, false)).unwrap();
    (dir, scripts, done)
}

#[test]
fn applied() {
    let (dir, _scripts, done) = agent_run(CUT, true);
    assert_eq!(done.applied.len(), 1, "{:?}", done.lines());
    let fix = &done.applied[0];
    assert_eq!(fix.decision, "local/long-note");
    assert_eq!(fix.files, ["a.txt"]);
    assert_eq!(fix.safety, "unsafe");
    assert!(!fix.follow_up);
    assert_eq!(fix.fingerprint.len(), 64);
    assert_eq!(read(dir.path(), "a.txt"), "a\nb\n");
}

#[test]
fn previewed() {
    // `[fix] unsafe` is off: the verified fix is shown, not written.
    let (dir, _scripts, done) = agent_run(CUT, false);
    assert!(done.applied.is_empty());
    assert_eq!(done.previews.len(), 1, "{:?}", done.lines());
    let preview = &done.previews[0];
    assert_eq!(preview.decision, "local/long-note");
    assert_eq!(preview.safety, "unsafe");
    assert_eq!(preview.files, ["a.txt"]);
    assert!(preview.diff.contains("-c"), "{}", preview.diff);
    assert!(
        preview.apply.apply.contains("--fingerprint"),
        "{}",
        preview.apply.apply
    );
    assert_eq!(read(dir.path(), "a.txt"), LONG, "a preview changes nothing");
    assert!(done.shown.contains_key(&preview.fingerprint));
}

#[test]
fn declined() {
    let (dir, _scripts, done) = agent_run(IDLE, true);
    assert!(done.applied.is_empty());
    assert_eq!(done.declined.len(), 1, "{:?}", done.lines());
    let left = &done.declined[0];
    assert_eq!(
        (left.decision.as_str(), left.path.as_str(), left.line),
        ("local/long-note", "a.txt", 1)
    );
    assert_eq!(left.reason, "the agent changed no file");
    // What is left alone is told with the reason.
    assert!(
        done.lines()
            .contains(&"not fixed a.txt:1 local/long-note: the agent changed no file".to_owned()),
        "{:?}",
        done.lines()
    );
    assert_eq!(read(dir.path(), "a.txt"), LONG);
}

#[test]
fn models() {
    let (_dir, _scripts, done) = agent_run(CUT, true);
    let counts = &done.models["local/long-note"];
    assert_eq!((counts.asked, counts.answered, counts.verified), (1, 1, 1));
    assert_eq!((counts.applied, counts.previewed), (1, 0));
    assert!(counts.declined.is_empty());

    let (_dir, _scripts, idle) = agent_run(IDLE, true);
    let counts = &idle.models["local/long-note"];
    assert_eq!((counts.asked, counts.verified, counts.applied), (1, 0, 0));
    assert_eq!(
        counts.declined.values().sum::<usize>(),
        1,
        "{:?}",
        counts.declined
    );
    assert!(idle.lines().iter().any(|l| l.contains("local/long-note")));
}

#[test]
fn batch() {
    let (_dir, _scripts, done) = agent_run(CUT, true);
    let usage = done.usage.as_ref().unwrap();
    assert_eq!(usage.usage.calls, 1);
    let runs = usage.agent.as_ref().unwrap();
    assert_eq!(runs.runs, 1);
    let batch = &runs.batches[0];
    assert_eq!(batch.file, "a.txt");
    // It opened the file of the finding, which the brief includes, and
    // another one, which it does not.
    assert_eq!(batch.reads, ["other.txt"]);
    assert_eq!(batch.calls.read, 2);
    assert!(!batch.incomplete);
    assert!(batch.note.is_none());
}
