//! The result cache is invisible: a run that takes findings from it prints
//! the same bytes and exits the same as a run that does not, through an edit
//! script over a Rust and a Go project, and the cache is cleaned on request.

use std::{
    fs,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use tempfile::TempDir;

const CACHE: &str = ".lighthouse/cache";

/// What one run printed.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    code: i32,
    stdout: String,
}

/// A run that also says how many rule runs the cache answered.
struct Timed {
    run: Run,
    hits: usize,
    misses: usize,
}

/// One edit of the script: what it does to the project, and whether it leaves
/// most of the findings of the rules where they were.
struct Edit {
    name: &'static str,
    apply: fn(&Path),
    /// The edit touches one function body: the cache must still answer more
    /// rule runs than it misses, though the rules that read the whole project
    /// run again.
    mostly_warm: bool,
}

/// A project of one language and the edit script that goes with it.
struct Fixture {
    name: &'static str,
    language: &'static str,
    toml: String,
    files: &'static [(&'static str, &'static str)],
    edits: Vec<Edit>,
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn replace(dir: &Path, name: &str, from: &str, to: &str) {
    let path = dir.join(name);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{name} has no `{from}`");
    fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn check(dir: &Path, args: &[&str]) -> (Run, String) {
    let out = lighthouse(dir)
        .args(["check", "--no-store", "--format", "json"])
        .args(args)
        .output()
        .unwrap();
    let run = Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
    };
    (run, String::from_utf8(out.stderr).unwrap())
}

fn cold(dir: &Path) -> Run {
    check(dir, &["--no-cache"]).0
}

fn warm(dir: &Path) -> Timed {
    let (run, stderr) = check(dir, &["--timings"]);
    let line = stderr
        .lines()
        .find(|l| l.starts_with("timings: hashing "))
        .unwrap_or_else(|| panic!("no hashing line in:\n{stderr}"));
    let numbers: Vec<usize> = line
        .split_once(" (")
        .unwrap()
        .1
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect();
    Timed {
        run,
        hits: numbers[0],
        misses: numbers[1],
    }
}

/// The local decision that the edit script changes in the middle: functions
/// whose names start with `prefix` are findings.
fn probe(prefix: &str, language: &str) -> String {
    let (path, bad, good) = match language {
        "go" => (
            "a.go",
            format!("package a\n\nfunc {prefix}x() {{}}\n"),
            "package a\n\nfunc zz() {}\n".to_owned(),
        ),
        _ => (
            "src/lib.rs",
            format!("pub fn {prefix}x() {{}}\n"),
            "pub fn zz() {}\n".to_owned(),
        ),
    };
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: symbol }}\n  requirement: A function MUST NOT start with a probe letter.\n  severity: warn\n  check:\n    type: cel\n    where: symbol.name.startsWith(\"{prefix}\") && symbol.kind == \"function\"\n    message: probe {{{{ symbol.name }}}}\n  examples:\n    - name: bad\n      language: {language}\n      kind: invalid\n      files: [{{ path: {path}, body: {bad:?} }}]\n      expect: [{{ line: 3 }}]\n    - name: good\n      language: {language}\n      kind: valid\n      files: [{{ path: {path}, body: {good:?} }}]\n"
    )
}

fn toml(language: &str, plugin: &Path) -> String {
    lighthouse_test_support::project(&format!(
        "plugins = [{{ id = \"{language}\", path = {:?} }}, \"core\", \"design\", \"testing\", \"local\"]\nextends = [\"core/recommended\", \"design/recommended\", \"design/strict\", \"testing/recommended\"]\n[rules]\n\"local/probe\" = \"warn\"\n\"design/coupling\" = {{ level = \"warn\", options = {{ hubFanIn = 2, hubFanOut = 0, hubStatements = 1 }} }}\n",
        plugin.to_str().unwrap()
    ))
}

fn lighthouse_toml(fixture: &Fixture, dir: &Path) {
    write(dir, "lighthouse.toml", &fixture.toml);
}

fn rust() -> Fixture {
    let plugin = lighthouse_test_support::lang_rust();
    Fixture {
        name: "rust",
        language: "rust",
        toml: toml("lang-rust", &plugin),
        files: &[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            ),
            (
                "src/lib.rs",
                "pub mod server;\npub mod store;\nmod util;\n\n/// Runs it all.\npub fn run() -> usize {\n    server::serve() + util::total()\n}\n",
            ),
            (
                "src/store.rs",
                "/// A store of numbers.\npub struct Store {\n    items: Vec<usize>,\n}\n\nimpl Store {\n    /// An empty store.\n    pub fn new() -> Self {\n        Self { items: Vec::new() }\n    }\n\n    /// Adds one.\n    pub fn add(&mut self, n: usize) {\n        self.items.push(n);\n    }\n\n    /// Sums them.\n    pub fn sum(&self) -> usize {\n        self.items.iter().sum()\n    }\n}\n\n/// Loads the default items.\npub fn load() -> Store {\n    let mut s = Store::new();\n    s.add(1);\n    s.add(2);\n    s\n}\n\nfn bump(n: usize) -> usize {\n    n + 1\n}\n\npub fn bumped() -> usize {\n    bump(1)\n}\n",
            ),
            (
                "src/server.rs",
                "use crate::store::load;\n\n/// Serves.\npub fn serve() -> usize {\n    let s = load();\n    s.sum()\n}\n\n/// Doubles.\npub fn double(n: usize) -> usize {\n    n * 2\n}\n",
            ),
            (
                "src/util.rs",
                "use crate::store::Store;\n\npub(crate) fn total() -> usize {\n    let mut s = Store::new();\n    s.add(3);\n    s.sum()\n}\n\nfn spare() {}\n",
            ),
            (
                "tests/run.rs",
                "#[test]\nfn run() {\n    assert!(demo::run() > 0);\n}\n",
            ),
        ],
        edits: vec![
            Edit {
                name: "body-only edit",
                apply: |d| replace(d, "src/store.rs", "n + 1", "n + 2"),
                mostly_warm: true,
            },
            Edit {
                name: "rename of a called function",
                apply: |d| {
                    replace(d, "src/store.rs", "pub fn load()", "pub fn fetch()");
                    replace(
                        d,
                        "src/server.rs",
                        "use crate::store::load;",
                        "use crate::store::fetch;",
                    );
                    replace(d, "src/server.rs", "load()", "fetch()");
                },
                mostly_warm: false,
            },
            Edit {
                name: "new callers in another file of the module",
                apply: |d| {
                    replace(
                        d,
                        "src/server.rs",
                        "/// Doubles.",
                        "impl crate::store::Store {\n    /// Twice.\n    pub fn twice(&self) -> usize {\n        self.sum() * 2\n    }\n\n    /// Thrice.\n    pub fn thrice(&self) -> usize {\n        self.sum() * 3\n    }\n}\n\n/// Doubles.",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "new exported type with a homonym",
                apply: |d| {
                    replace(
                        d,
                        "src/store.rs",
                        "fn bump(",
                        "/// An entry.\npub struct Entry;\n\nfn bump(",
                    );
                    replace(
                        d,
                        "src/server.rs",
                        "/// Doubles.",
                        "/// An entry.\npub struct Entry;\n\n/// Doubles.",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "rule option changed",
                apply: |d| {
                    replace(
                        d,
                        "lighthouse.toml",
                        "\"local/probe\" = \"warn\"",
                        "\"local/probe\" = \"warn\", \"design/max-name-words\" = { level = \"warn\", options = { max = 1 } }",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "the decision's check edited",
                apply: |d| write(d, ".lighthouse/decisions/probe.yaml", &probe("d", "rust")),
                mostly_warm: false,
            },
            Edit {
                name: "file deleted",
                apply: |d| {
                    fs::remove_file(d.join("src/util.rs")).unwrap();
                    replace(d, "src/lib.rs", "mod util;\n", "");
                    replace(
                        d,
                        "src/lib.rs",
                        "server::serve() + util::total()",
                        "server::serve()",
                    );
                },
                mostly_warm: false,
            },
        ],
    }
}

fn go() -> Option<Fixture> {
    let plugin = lighthouse_test_support::lang_go()?;
    Some(Fixture {
        name: "go",
        language: "go",
        toml: toml("lang-go", &plugin),
        files: &[
            ("go.mod", "module example.com/app\n\ngo 1.26\n"),
            (
                "app/app.go",
                "package app\n\nimport (\n\t\"example.com/app/server\"\n\t\"example.com/app/util\"\n)\n\n// Run runs it all.\nfunc Run() int {\n\treturn server.Serve() + util.Total()\n}\n",
            ),
            (
                "store/store.go",
                "package store\n\n// Store holds numbers.\ntype Store struct {\n\titems []int\n}\n\n// New returns an empty store.\nfunc New() *Store {\n\treturn &Store{}\n}\n\n// Add adds one.\nfunc (s *Store) Add(n int) {\n\ts.items = append(s.items, n)\n}\n\n// Sum sums them.\nfunc (s *Store) Sum() int {\n\ttotal := 0\n\tfor _, n := range s.items {\n\t\ttotal += n\n\t}\n\treturn total\n}\n\n// Load loads the default items.\nfunc Load() *Store {\n\ts := New()\n\ts.Add(1)\n\ts.Add(2)\n\treturn s\n}\n\nfunc bump(n int) int {\n\treturn n + 1\n}\n\n// Bumped bumps.\nfunc Bumped() int {\n\treturn bump(1)\n}\n",
            ),
            (
                "server/server.go",
                "package server\n\nimport \"example.com/app/store\"\n\n// Serve serves.\nfunc Serve() int {\n\treturn store.Load().Sum()\n}\n\n// Double doubles.\nfunc Double(n int) int {\n\treturn n * 2\n}\n",
            ),
            (
                "util/util.go",
                "package util\n\nimport \"example.com/app/store\"\n\n// Total totals.\nfunc Total() int {\n\ts := store.New()\n\ts.Add(3)\n\treturn s.Sum()\n}\n\nfunc spare() {}\n",
            ),
            (
                "app/app_test.go",
                "package app\n\nimport \"testing\"\n\nfunc TestRun(t *testing.T) {\n\tif Run() == 0 {\n\t\tt.Fatal(\"zero\")\n\t}\n}\n",
            ),
        ],
        edits: vec![
            Edit {
                name: "body-only edit",
                apply: |d| replace(d, "store/store.go", "n + 1", "n + 2"),
                mostly_warm: true,
            },
            Edit {
                name: "rename of a called function",
                apply: |d| {
                    replace(d, "store/store.go", "func Load()", "func Fetch()");
                    replace(d, "server/server.go", "store.Load()", "store.Fetch()");
                },
                mostly_warm: false,
            },
            Edit {
                name: "new callers in another file of the package",
                apply: |d| {
                    write(
                        d,
                        "store/extra.go",
                        "package store\n\nfunc extraOne() int {\n\treturn Fetch().Sum()\n}\n\nfunc extraTwo() int {\n\treturn Fetch().Sum() + 1\n}\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "new exported type with a homonym",
                apply: |d| {
                    replace(
                        d,
                        "store/store.go",
                        "func bump(",
                        "// Entry is an entry.\ntype Entry struct{}\n\nfunc bump(",
                    );
                    replace(
                        d,
                        "server/server.go",
                        "// Double doubles.",
                        "// Entry is an entry.\ntype Entry struct{}\n\n// Double doubles.",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "rule option changed",
                apply: |d| {
                    replace(
                        d,
                        "lighthouse.toml",
                        "\"local/probe\" = \"warn\"",
                        "\"local/probe\" = \"warn\", \"design/max-name-words\" = { level = \"warn\", options = { max = 1 } }",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "the decision's check edited",
                apply: |d| write(d, ".lighthouse/decisions/probe.yaml", &probe("d", "go")),
                mostly_warm: false,
            },
            Edit {
                name: "file deleted",
                apply: |d| {
                    fs::remove_file(d.join("util/util.go")).unwrap();
                    replace(d, "app/app.go", "\t\"example.com/app/util\"\n", "");
                    replace(
                        d,
                        "app/app.go",
                        "server.Serve() + util.Total()",
                        "server.Serve()",
                    );
                },
                mostly_warm: false,
            },
        ],
    })
}

fn project(fixture: &Fixture) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    lighthouse_toml(fixture, dir.path());
    for (name, text) in fixture.files {
        write(dir.path(), name, text);
    }
    write(
        dir.path(),
        ".lighthouse/decisions/probe.yaml",
        &probe("s", fixture.language),
    );
    dir
}

/// Runs the edit script and checks, after every step, that a run on the cache
/// prints what a run without it prints.
fn differential(fixture: &Fixture) {
    let dir = project(fixture);
    let dir = dir.path();
    let baseline = cold(dir);
    assert!(
        baseline.stdout.contains("local/probe"),
        "{}: the probe finds nothing:\n{}",
        fixture.name,
        baseline.stdout
    );

    let first = warm(dir);
    assert_eq!(first.run, baseline, "{}: first run", fixture.name);
    assert_eq!(first.hits, 0, "{}: nothing stored yet", fixture.name);
    let unchanged = warm(dir);
    assert_eq!(unchanged.run, baseline, "{}: no change", fixture.name);
    assert_eq!(unchanged.misses, 0, "{}: no change", fixture.name);

    for edit in &fixture.edits {
        (edit.apply)(dir);
        let expected = cold(dir);
        let got = warm(dir);
        let step = format!("{}: {}", fixture.name, edit.name);
        assert_eq!(got.run, expected, "{step}");
        assert!(got.hits > 0, "{step}: the cache answered nothing");
        if edit.mostly_warm {
            assert!(
                got.misses < got.hits,
                "{step}: {} hits, {} misses",
                got.hits,
                got.misses
            );
        }
        let again = warm(dir);
        assert_eq!(again.run, expected, "{step}: again");
        assert_eq!(again.misses, 0, "{step}: again");
    }
}

#[test]
fn rust_findings_are_the_same_with_and_without_the_cache() {
    differential(&rust());
}

#[test]
fn go_findings_are_the_same_with_and_without_the_cache() {
    if let Some(fixture) = go() {
        differential(&fixture);
    }
}

#[test]
fn clean_deletes_the_cache_directory_and_what_a_plugin_put_in_it() {
    let fixture = rust();
    let dir = project(&fixture);
    let cache: PathBuf = dir.path().join(CACHE);
    warm(dir.path());
    assert!(cache.join("results.db").exists());
    write(dir.path(), ".lighthouse/cache/lang-rust/units.json", "{}");

    lighthouse(dir.path())
        .args(["cache", "clean"])
        .assert()
        .success();
    assert!(!cache.exists());
    lighthouse(dir.path())
        .args(["cache", "clean"])
        .assert()
        .success();
}

#[test]
fn no_cache_neither_reads_nor_writes_the_cache() {
    let fixture = rust();
    let dir = project(&fixture);
    cold(dir.path());
    assert!(!dir.path().join(CACHE).exists());
}
