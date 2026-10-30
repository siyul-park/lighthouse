//! What a file's findings were before the session first touched it.

use std::{fs, path::Path};

use lighthouse_model::Diagnostic;
use lighthouse_session::{
    CheckRequest,
    baseline::{self, Baseline},
};
use serde_json::json;

use crate::fixture::{LONG, canonical, project, session, write};

/// The findings of the project: `a.txt` and `b.txt`, both too long.
fn findings(dir: &Path) -> Vec<Diagnostic> {
    write(dir, "b.txt", LONG);
    let session = session(dir);
    let mut found = lighthouse_session::check(
        &session,
        &CheckRequest {
            paths: vec![session.root.clone()],
            ..CheckRequest::default()
        },
    )
    .unwrap()
    .findings();
    found.sort_by(|a, b| a.file.cmp(&b.file));
    found
}

#[test]
fn baseline_of() {
    let dir = project("error");
    let found = findings(dir.path());
    let own: Vec<_> = found
        .iter()
        .filter(|d| d.file == Path::new("a.txt"))
        .cloned()
        .collect();

    let a = Baseline::of(Path::new("a.txt"), &own);
    assert_eq!(a.path(), Path::new("a.txt"));
    let none = Baseline::of(Path::new("new.txt"), &[]);
    assert_eq!(none.path(), Path::new("new.txt"));
    assert!(baseline::gone(&[a], &[]).len() == 1);
    assert!(baseline::gone(&[none], &[]).is_empty());
}

#[test]
fn baseline_path() {
    let dir = project("error");
    let nested = Baseline::of(Path::new("src/deep/a.txt"), &[]);
    assert_eq!(nested.path(), Path::new("src/deep/a.txt"));

    // The path survives the file it is kept in, and names the file to look up.
    baseline::record(dir.path(), "s1", &nested).unwrap();
    let loaded = baseline::load(dir.path(), "s1", Path::new("src/deep/a.txt")).unwrap();
    assert_eq!(loaded.path(), nested.path());
}

#[test]
fn session_id() {
    assert_eq!(baseline::session_id(Some("abc-1_2")), Some("abc-1_2"));
    assert_eq!(baseline::session_id(None), None);
    assert_eq!(baseline::session_id(Some("")), None);
    assert_eq!(baseline::session_id(Some("../escape")), None);
    assert_eq!(baseline::session_id(Some("a/b")), None);
    assert_eq!(baseline::session_id(Some(&"x".repeat(129))), None);
    assert!(baseline::session_id(Some(&"x".repeat(128))).is_some());
}

#[test]
fn exists() {
    let dir = project("error");
    let root = dir.path();
    assert!(!baseline::exists(root, "s1", Path::new("a.txt")));
    baseline::record(root, "s1", &Baseline::of(Path::new("a.txt"), &[])).unwrap();
    assert!(baseline::exists(root, "s1", Path::new("a.txt")));
    assert!(
        !baseline::exists(root, "s2", Path::new("a.txt")),
        "per session"
    );
    assert!(
        !baseline::exists(root, "s1", Path::new("b.txt")),
        "per file"
    );
}

#[test]
fn load() {
    let dir = project("error");
    let root = dir.path();
    assert!(baseline::load(root, "s1", Path::new("a.txt")).is_none());

    let found = findings(root);
    baseline::record(root, "s1", &Baseline::of(Path::new("a.txt"), &found[..1])).unwrap();
    let loaded = baseline::load(root, "s1", Path::new("a.txt")).unwrap();
    assert_eq!(loaded.path(), Path::new("a.txt"));
    // What was loaded is what was recorded: it holds the finding as it was.
    assert!(baseline::gone(&[loaded], &[]).len() == 1);

    // A file that is not a baseline is no baseline.
    let dir_of = fs::read_dir(root.join(".lighthouse/cache/hooks/s1")).unwrap();
    for entry in dir_of.flatten() {
        fs::write(entry.path(), "not json").unwrap();
    }
    assert!(baseline::load(root, "s1", Path::new("a.txt")).is_none());
}

#[test]
fn load_all() {
    let dir = project("error");
    let root = dir.path();
    assert!(baseline::load_all(root, "s1").is_empty());
    for name in ["b.txt", "a.txt", "c/d.txt"] {
        baseline::record(root, "s1", &Baseline::of(Path::new(name), &[])).unwrap();
    }
    baseline::record(root, "s2", &Baseline::of(Path::new("z.txt"), &[])).unwrap();
    let all = baseline::load_all(root, "s1");
    let paths: Vec<_> = all.iter().map(|b| b.path().to_owned()).collect();
    assert_eq!(paths, ["a.txt", "b.txt", "c/d.txt"].map(Path::new));
}

#[test]
fn record() {
    let dir = project("error");
    let root = dir.path();
    baseline::record(root, "old", &Baseline::of(Path::new("a.txt"), &[])).unwrap();
    // A session directory untouched for over a week is deleted by the next write.
    let old = root.join(".lighthouse/cache/hooks/old");
    let eight_days = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 86_400);
    fs::File::open(&old)
        .unwrap()
        .set_modified(eight_days)
        .unwrap();

    baseline::record(root, "now", &Baseline::of(Path::new("a.txt"), &[])).unwrap();
    assert!(!old.exists());
    assert!(baseline::exists(root, "now", Path::new("a.txt")));
}

#[test]
fn introduced() {
    let dir = project("error");
    let found = findings(dir.path());
    assert_eq!(found.len(), 2);

    // The baseline holds `a.txt`'s finding: only `b.txt`'s is the session's.
    let before = Baseline::of(Path::new("a.txt"), &found[..1]);
    let mut kept = found.clone();
    baseline::introduced(&mut kept, &[before]);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].file, Path::new("b.txt"));

    // A file with no baseline keeps its findings: nothing says they were there.
    let mut all = found.clone();
    baseline::introduced(&mut all, &[]);
    assert_eq!(all.len(), 2);

    // The same finding with other evidence is an update.
    let mut changed = found[..1].to_vec();
    changed[0].evidence = json!({ "path": "a.txt", "lines": 99 });
    let before = Baseline::of(Path::new("a.txt"), &found[..1]);
    baseline::introduced(&mut changed, &[before]);
    assert_eq!(changed.len(), 1);
}

#[test]
fn gone() {
    let dir = project("error");
    let found = findings(dir.path());
    let before = [
        Baseline::of(Path::new("a.txt"), &found[..1]),
        Baseline::of(Path::new("b.txt"), &found[1..]),
    ];
    assert!(baseline::gone(&before, &found).is_empty());
    let resolved = baseline::gone(&before, &found[1..]);
    assert_eq!(resolved, [found[0].fingerprint.as_str().to_owned()]);
}

#[test]
fn editable() {
    let dir = project("error");
    let root = canonical(dir.path());
    write(&root, "src/x.rs", "fn main() {}\n");
    write(&root, ".lighthouse/cache/y.txt", "x");
    fs::write(root.join("blob.bin"), [0u8, 1, 2]).unwrap();

    let relative = |path: &str| baseline::editable(&root.join(path), &root);
    assert_eq!(relative("src/x.rs"), Some("src/x.rs".into()));
    // A file the tool is about to create counts.
    assert_eq!(relative("src/new/z.rs"), Some("src/new/z.rs".into()));
    assert_eq!(
        relative(".lighthouse/cache/y.txt"),
        None,
        "state is not code"
    );
    assert_eq!(relative("blob.bin"), None, "binary");
    assert_eq!(relative("../escape.txt"), None, "outside the project");
    assert_eq!(baseline::editable(Path::new("/etc/hosts"), &root), None);
    assert_eq!(relative("target/debug/x.txt"), None, "skipped directory");
}
