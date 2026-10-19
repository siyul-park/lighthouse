//! The rewrite path: the scope check, the scratch copy and the verification
//! programs that run in it.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use lighthouse_model::{Position, Span};
use lighthouse_process::Policy;
use lighthouse_spec::Program;

use super::{Change, Scratch, run_verify, scope::first_outside};
use crate::Overlays;

fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span {
        start: Position {
            line: from.0,
            col: from.1,
        },
        end: Position {
            line: to.0,
            col: to.1,
        },
    }
}

#[test]
fn a_change_is_inside_the_scope_symbol_when_every_changed_line_is_in_an_extent() {
    let old = "a\nb\nc\nd\ne\n";
    let extents = [span((2, 1), (3, 2)), span((5, 1), (5, 2))];
    let outside = |new: &str| first_outside(old, new, &extents);

    assert_eq!(outside("a\nB\nC\nd\ne\n"), None);
    assert_eq!(outside("a\nb\nc\nd\nE\n"), None);
    assert_eq!(outside("a\nb\nc\nD\ne\n"), Some(4));
    assert_eq!(outside("A\nb\nc\nd\ne\n"), Some(1));
    assert_eq!(outside("a\nb\nc\nx\nd\ne\n"), None, "next to an extent");
    assert_eq!(outside("a\nb\nx\nc\nd\ne\n"), None, "inside an extent");
    assert_eq!(outside("x\na\nb\nc\nd\ne\n"), Some(1));
    assert_eq!(outside(old), None);
}

#[test]
fn the_scratch_copy_holds_the_files_and_texts_of_the_run_and_tells_what_was_changed() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/a.txt"), "on disk\n").unwrap();
    fs::write(root.path().join("src/b.txt"), "b\n").unwrap();
    fs::write(root.path().join("src/c.txt"), "c\n").unwrap();
    fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.path().join("ignored.txt"), "ignored\n").unwrap();
    fs::write(root.path().join(".env"), "KEY=fake\n").unwrap();
    fs::write(root.path().join(".env.local"), "KEY=fake\n").unwrap();
    fs::create_dir_all(root.path().join(".lighthouse")).unwrap();
    fs::write(root.path().join(".lighthouse/local.toml"), "x = 1\n").unwrap();
    let texts = Overlays::from([("src/a.txt".into(), "overlay\n".to_owned())]);

    let scratch = Scratch::new(root.path(), &texts).unwrap();
    let at = scratch.path().to_owned();

    assert_eq!(
        fs::read_to_string(at.join("src/a.txt")).unwrap(),
        "overlay\n"
    );
    assert!(!at.join("ignored.txt").exists());
    for secret in [".env", ".env.local", ".lighthouse/local.toml"] {
        assert!(!at.join(secret).exists(), "{secret} was copied");
    }
    assert!(scratch.changes(root.path(), &texts).unwrap().is_empty());

    fs::write(at.join("src/b.txt"), "changed\n").unwrap();
    fs::write(at.join("src/a.txt"), "on disk\n").unwrap();
    fs::remove_file(at.join("src/c.txt")).unwrap();
    fs::write(at.join("new.txt"), "new\n").unwrap();
    let changes = scratch.changes(root.path(), &texts).unwrap();

    assert_eq!(
        changes[Path::new("src/b.txt")],
        Change::Text("changed\n".to_owned())
    );
    assert_eq!(
        changes[Path::new("src/a.txt")],
        Change::Text("on disk\n".to_owned())
    );
    assert_eq!(changes[Path::new("src/c.txt")], Change::Deleted);
    assert_eq!(changes[Path::new("new.txt")], Change::Created);
    assert_eq!(changes.len(), 4);

    drop(scratch);
    assert!(!at.exists(), "the copy is deleted when it is dropped");
}

#[test]
fn the_scratch_copy_is_a_clean_repository_of_its_own() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "disk\n").unwrap();
    let texts = Overlays::from([("a.txt".into(), "overlay\n".to_owned())]);

    let scratch = Scratch::new(root.path(), &texts).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(scratch.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };

    let top = PathBuf::from(git(&["rev-parse", "--show-toplevel"]));
    assert_eq!(
        top.canonicalize().unwrap(),
        scratch.path().canonicalize().unwrap(),
        "the copy is its own repository"
    );
    assert_eq!(git(&["status", "--porcelain"]), "", "clean before any edit");
    assert_eq!(git(&["rev-list", "--count", "HEAD"]), "1", "one commit");
    assert_eq!(git(&["show", "HEAD:a.txt"]), "overlay", "of the base state");
    assert!(
        !root.path().join(".git").exists(),
        "the project is not touched"
    );
}

#[test]
fn the_verify_programs_run_over_a_copy_that_holds_the_fixed_texts() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "old\n").unwrap();
    fs::write(root.path().join(".gitignore"), "skipped/\n").unwrap();
    fs::create_dir(root.path().join("skipped")).unwrap();
    fs::write(root.path().join("skipped/b.txt"), "x\n").unwrap();
    let texts = crate::Overlays::from([(PathBuf::from("a.txt"), "new\n".to_owned())]);
    let sh = |script: &str| {
        let mut program = Program::of(vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()]);
        program.timeout = Some(lighthouse_resource::Duration::from_secs(20));
        program
    };

    let passed = run_verify(
        root.path(),
        &texts,
        &[sh("grep -q new a.txt"), sh("test ! -e skipped/b.txt")],
        &Policy::default(),
    );
    assert_eq!(passed, Ok(()));
    assert_eq!(
        fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "old\n"
    );

    let failed = run_verify(
        root.path(),
        &texts,
        &[sh("echo one >&2; echo two >&2; exit 3"), sh("exit 0")],
        &Policy::default(),
    )
    .unwrap_err();
    assert!(failed.contains("exited 3"), "{failed}");
    assert!(failed.ends_with("one\ntwo"), "{failed}");

    fs::write(root.path().join(".env"), "KEY=fake\n").unwrap();
    let read = run_verify(
        root.path(),
        &texts,
        &[sh(&format!(
            "test ! -e .env && ! cat '{}/.env' 2>/dev/null",
            root.path().display()
        ))],
        &Policy::default(),
    );
    assert_eq!(read, Ok(()), "a command read the .env of the project");

    let escaping = crate::Overlays::from([(PathBuf::from("../x"), String::new())]);
    assert!(run_verify(root.path(), &escaping, &[], &Policy::default()).is_err());
}

/// Set in the child process the test below starts, with the `GIT_*`
/// variables a user's shell may have.
const DECOY: &str = "LIGHTHOUSE_SCRATCH_DECOY";

#[test]
fn a_git_directory_in_the_environment_never_receives_the_base_commit() {
    if std::env::var_os(DECOY).is_some() {
        // The child: GIT_DIR names a repository of the user.
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "x\n").unwrap();
        let scratch = Scratch::new(root.path(), &Overlays::new()).unwrap();
        assert!(scratch.path().join(".git").exists(), "its own repository");
        return;
    }
    let decoy = tempfile::tempdir().unwrap();
    let ok = Command::new("git")
        .args(["init", "--quiet", "--bare"])
        .arg(decoy.path())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    fs::create_dir(decoy.path().join("tree")).unwrap();
    let before = fs::read_dir(decoy.path().join("objects")).unwrap().count();

    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fix::rewrite::tests::a_git_directory_in_the_environment_never_receives_the_base_commit",
        ])
        .env(DECOY, "1")
        .env("GIT_DIR", decoy.path())
        .env("GIT_WORK_TREE", decoy.path().join("tree"))
        .env("GIT_INDEX_FILE", decoy.path().join("index"))
        .output()
        .unwrap();

    let said = String::from_utf8_lossy(&child.stdout);
    assert!(
        child.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(said.contains("1 passed"), "the child test ran: {said}");
    assert_eq!(
        fs::read_dir(decoy.path().join("objects")).unwrap().count(),
        before,
        "the repository of GIT_DIR is as it was"
    );
    assert!(!decoy.path().join("index").exists());
}

#[test]
fn a_secret_a_verify_program_prints_is_not_in_the_note_of_its_failure() {
    // SAFETY: this variable belongs to this test.
    unsafe { std::env::set_var("LIGHTHOUSE_VERIFY_TEST_SECRET", "sk-in-a-note") };
    let root = tempfile::tempdir().unwrap();
    let mut program = Program::of(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "echo \"key $TOKEN\"; echo \"stderr $TOKEN\" >&2; exit 3".to_owned(),
    ]);
    program.env = [(
        "TOKEN".to_owned(),
        "${LIGHTHOUSE_VERIFY_TEST_SECRET}".to_owned(),
    )]
    .into();
    program.output = Some(lighthouse_spec::Output::Lines);

    let failed = run_verify(
        root.path(),
        &Overlays::new(),
        &[program],
        &Policy::default(),
    )
    .unwrap_err();

    assert!(
        failed.contains("${LIGHTHOUSE_VERIFY_TEST_SECRET}"),
        "{failed}"
    );
    assert!(!failed.contains("sk-in-a-note"), "{failed}");
}
