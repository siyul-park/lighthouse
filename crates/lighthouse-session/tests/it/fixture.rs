//! What the tests of the session share: a text project with one local
//! decision, a session over it, a git repository around it, and the user-level
//! directories moved out of the real ones.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, Once},
};

use lighthouse_session::Session;
use tempfile::TempDir;

/// The process's working directory is shared, so whoever changes or reads it
/// holds this.
static CWD: Mutex<()> = Mutex::new(());

/// The trust file is shared by every test of the process and a write replaces
/// it whole, so whoever writes it holds this.
static TRUST: Mutex<()> = Mutex::new(());

/// A note must be short: a text file of more than two lines is a finding, and
/// the fix keeps its first two lines with a command.
const SHORT: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/short
  uid: 3f2a9c1e-5b7d-4e8f-9a1b-2c3d4e5f6a7b
spec:
  title: Notes are short
  context: Notes are read at a glance.
  scope:
    subject: file
  requirement: A note MUST NOT run past two lines.
  severity: SEVERITY
  check:
    type: cel
    select: file
    where: "file.path.endsWith(\".txt\") && file.lines > 2"
    message: "{{ file.path }} is too long"
    evidence:
      path: file.path
  fix:
    safety: SAFETY
    type: command
    command: [sh, "-c", "head -n 2 \"$1\"", sh, "{file}"]
    output: stdout
  examples:
    - name: long
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: "a\nb\nc\n"
      expect: [{ line: 1 }]
      fixed:
        - path: a.txt
          body: "a\nb\n"
    - name: short
      language: text
      kind: valid
      files:
        - path: a.txt
          body: "a\nb\n"
"#;

/// The text of the file the decision fails.
pub const LONG: &str = "a\nb\nc\n";

/// Moves the user-level directories (the user's file, trust, plugins) of this
/// process into a directory of its own, once, and takes the CI override of
/// trust away, so that no test reads or writes the real ones.
pub fn isolate() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("home");
        std::mem::forget(home);
        let moved = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"]
            .map(|name| (name.to_owned(), Some(path.clone().into_os_string())));
        let unset =
            ["LIGHTHOUSE_TRUST", "LIGHTHOUSE_ATTRIBUTED_TO"].map(|name| (name.to_owned(), None));
        assert!(lighthouse_resource::replace_vars(
            moved.into_iter().chain(unset)
        ));
    });
}

/// Runs `f` with `dir` as the working directory of the process.
pub fn in_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _held = CWD.lock().unwrap_or_else(|e| e.into_inner());
    let before = env::current_dir().unwrap();
    env::set_current_dir(dir).unwrap();
    let out = f();
    env::set_current_dir(before).unwrap();
    out
}

pub fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

pub fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap()
}

/// A text project whose local decision `local/short` is authored at
/// `severity` and fails `a.txt` ([`LONG`]). Nothing else is in it.
pub fn project(severity: &str) -> TempDir {
    project_with(severity, "")
}

/// `project`, with `extra` lines of TOML at the end of the project's spec.
pub fn project_with(severity: &str, extra: &str) -> TempDir {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "[rules]\n\"local/short\" = \"{severity}\"\n{extra}"
        )),
    );
    write(
        dir.path(),
        ".lighthouse/decisions/short.yaml",
        &SHORT.replace("SEVERITY", severity).replace(
            "SAFETY",
            if severity == "error" {
                "safe"
            } else {
                "unsafe"
            },
        ),
    );
    write(dir.path(), "a.txt", LONG);
    dir
}

pub fn session(dir: &Path) -> Session {
    Session::load_in(dir, None).unwrap()
}

/// Records trust in `session`'s project.
pub fn grant(session: &Session) -> PathBuf {
    let _held = TRUST.lock().unwrap_or_else(|e| e.into_inner());
    lighthouse_session::trust(&session.root, &session.basis).unwrap()
}

/// Withdraws trust in the project at `root`.
pub fn withdraw(root: &Path) -> bool {
    let _held = TRUST.lock().unwrap_or_else(|e| e.into_inner());
    lighthouse_session::revoke(root).unwrap()
}

/// The session of `dir` after the user trusted it.
pub fn trusted(dir: &Path) -> Session {
    grant(&session(dir));
    session(dir)
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `project`, committed in a git repository of its own.
pub fn repo(severity: &str) -> TempDir {
    let dir = project(severity);
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Kim"]);
    git(dir.path(), &["config", "user.email", "kim@example.com"]);
    write(
        dir.path(),
        ".gitignore",
        ".lighthouse/*.db*\n.lighthouse/cache\n",
    );
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "first"]);
    dir
}

pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}
