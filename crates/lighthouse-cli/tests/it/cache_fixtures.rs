//! The projects and the edit script of the cache differential tests: a Rust
//! and a Go project, and the edits that change one thing each.

use std::{fs, path::Path};

use tempfile::TempDir;

use crate::checks::trust;

/// One edit of the script: what it does to the project, and whether it leaves
/// most of the findings of the rules where they were.
pub(crate) struct Edit {
    pub(crate) name: &'static str,
    pub(crate) apply: fn(&Path),
    /// The edit touches one function body: the cache must still answer more
    /// rule runs than it misses, though the rules that read the whole project
    /// run again.
    pub(crate) mostly_warm: bool,
}

/// A project of one language and the edit script that goes with it.
pub(crate) struct Fixture {
    pub(crate) name: &'static str,
    pub(crate) language: &'static str,
    pub(crate) toml: String,
    pub(crate) files: &'static [(&'static str, &'static str)],
    pub(crate) edits: Vec<Edit>,
}

pub(crate) fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

pub(crate) fn replace(dir: &Path, name: &str, from: &str, to: &str) {
    let path = dir.join(name);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{name} has no `{from}`");
    fs::write(path, text.replacen(from, to, 1)).unwrap();
}

/// The local decision that the edit script changes in the middle: functions
/// whose names start with `prefix` are findings.
pub(crate) fn probe(prefix: &str, language: &str) -> String {
    let (path, bad, good) = match language {
        "go" => (
            "a.go",
            format!("package a\n\nfunc {prefix}x() {{}}\n"),
            "package a\n\nfunc zz() {}\n".to_owned(),
        ),
        "typescript" => (
            "a.ts",
            format!("// probe\n\nexport function {prefix}x() {{}}\n"),
            "// probe\n\nexport function zz() {}\n".to_owned(),
        ),
        "python" => (
            "a.py",
            format!("# probe\n\ndef {prefix}x():\n    pass\n"),
            "# probe\n\ndef zz():\n    pass\n".to_owned(),
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

pub(crate) fn toml(language: &str, plugin: &Path) -> String {
    lighthouse_test_support::project(&format!(
        "plugins = [{{ id = \"{language}\", path = {:?} }}]\nextends = [\"core/recommended\", \"design/recommended\", \"design/strict\", \"testing/recommended\"]\n[rules]\n\"local/probe\" = \"warn\"\n\"design/coupling\" = {{ level = \"warn\", options = {{ hub = {{ fanIn = 2, fanOut = 0, statements = 1, coordinatorFanOut = 12, coordinatorMaxFanIn = 1, coordinatorStatements = 15 }} }} }}\n",
        plugin.to_str().unwrap()
    ))
}

pub(crate) fn lighthouse_toml(fixture: &Fixture, dir: &Path) {
    write(dir, "lighthouse.toml", &fixture.toml);
}

pub(crate) fn rust() -> Fixture {
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
                        "\"local/probe\" = \"warn\", \"design/max-name-words\" = { level = \"warn\", options = { enabled = true, maxWords = 1 } }",
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
                name: "signature edit",
                apply: |d| {
                    replace(
                        d,
                        "src/server.rs",
                        "pub fn double(n: usize) -> usize {\n    n * 2",
                        "pub fn double(n: usize, m: usize) -> usize {\n    n * m",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "file added",
                apply: |d| {
                    write(
                        d,
                        "src/extra.rs",
                        "/// Extra.\npub fn extra() -> usize {\n    1\n}\n",
                    );
                    replace(
                        d,
                        "src/lib.rs",
                        "mod util;\n",
                        "mod util;\npub mod extra;\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "file renamed",
                apply: |d| {
                    fs::rename(d.join("src/extra.rs"), d.join("src/more.rs")).unwrap();
                    replace(d, "src/lib.rs", "pub mod extra;", "pub mod more;");
                },
                mostly_warm: false,
            },
            Edit {
                name: "Cargo.toml edited",
                apply: |d| replace(d, "Cargo.toml", "edition = \"2024\"", "edition = \"2021\""),
                mostly_warm: false,
            },
            Edit {
                name: "path dependency added",
                apply: |d| {
                    let dep = d.with_extension("dep");
                    write(
                        &dep,
                        "Cargo.toml",
                        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
                    );
                    write(
                        &dep,
                        "src/lib.rs",
                        "/// One.\npub fn value() -> usize {\n    1\n}\n",
                    );
                    let name = dep.file_name().unwrap().to_string_lossy().into_owned();
                    replace(
                        d,
                        "Cargo.toml",
                        "edition = \"2021\"\n",
                        &format!(
                            "edition = \"2021\"\n\n[dependencies]\ndep = {{ path = \"../{name}\" }}\n"
                        ),
                    );
                    write(
                        d,
                        "src/outer.rs",
                        "/// From the dependency.\npub fn outer() -> usize {\n    dep::value()\n}\n",
                    );
                    replace(
                        d,
                        "src/lib.rs",
                        "mod util;\n",
                        "mod util;\npub mod outer;\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "path dependency edited",
                apply: |d| {
                    let dep = d.with_extension("dep");
                    write(
                        &dep,
                        "src/lib.rs",
                        "/// One, as text.\npub fn value() -> String {\n    String::new()\n}\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "provider option changed",
                apply: |d| {
                    replace(
                        d,
                        "lighthouse.toml",
                        "spec = { ",
                        "spec = { languages = { rust = { unpublished = \"internal\" } }, ",
                    )
                },
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

pub(crate) fn go() -> Option<Fixture> {
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
                        "\"local/probe\" = \"warn\", \"design/max-name-words\" = { level = \"warn\", options = { enabled = true, maxWords = 1 } }",
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
                name: "signature edit",
                apply: |d| {
                    replace(
                        d,
                        "server/server.go",
                        "func Double(n int) int {\n\treturn n * 2",
                        "func Double(n, m int) int {\n\treturn n * m",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "file added",
                apply: |d| {
                    write(
                        d,
                        "util/more.go",
                        "package util\n\n// More is more.\nfunc More() int {\n\treturn 1\n}\n",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "file renamed",
                apply: |d| fs::rename(d.join("util/more.go"), d.join("util/extra.go")).unwrap(),
                mostly_warm: false,
            },
            Edit {
                name: "new package",
                apply: |d| {
                    write(
                        d,
                        "extra/extra.go",
                        "package extra\n\n// Extra is extra.\nfunc Extra() int {\n\treturn 1\n}\n",
                    );
                    replace(
                        d,
                        "app/app.go",
                        "// Run runs it all.",
                        "var _ = extra.Extra\n\n// Run runs it all.",
                    );
                    replace(
                        d,
                        "app/app.go",
                        "import (\n",
                        "import (\n\t\"example.com/app/extra\"\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "go.mod edited",
                apply: |d| replace(d, "go.mod", "go 1.26", "go 1.25"),
                mostly_warm: false,
            },
            Edit {
                name: "replace target added",
                apply: |d| {
                    let dep = d.with_extension("dep");
                    write(&dep, "go.mod", "module example.com/dep\n\ngo 1.22\n");
                    write(
                        &dep,
                        "dep.go",
                        "package dep\n\n// Value is one.\nfunc Value() int {\n\treturn 1\n}\n",
                    );
                    let name = dep.file_name().unwrap().to_string_lossy().into_owned();
                    replace(
                        d,
                        "go.mod",
                        "go 1.25\n",
                        &format!(
                            "go 1.25\n\nrequire example.com/dep v0.0.0\n\nreplace example.com/dep => ../{name}\n"
                        ),
                    );
                    write(
                        d,
                        "util/dep.go",
                        "package util\n\nimport \"example.com/dep\"\n\n// FromDep reads the dependency.\nfunc FromDep() int {\n\treturn dep.Value()\n}\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "replace target edited",
                apply: |d| {
                    let dep = d.with_extension("dep");
                    write(
                        &dep,
                        "dep.go",
                        "package dep\n\n// Value is text now, which the importer cannot return as a number.\nfunc Value() string {\n\treturn \"two\"\n}\n",
                    );
                },
                mostly_warm: false,
            },
            Edit {
                name: "provider option changed",
                apply: |d| {
                    replace(
                        d,
                        "lighthouse.toml",
                        "spec = { ",
                        "spec = { languages = { go = { tags = [\"extra\"] } }, ",
                    )
                },
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

pub(crate) fn project(fixture: &Fixture) -> TempDir {
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
    trust(dir.path());
    dir
}

/// The edits every fixture shares: they are about the configuration, not the
/// language.
fn common(language: &'static str) -> [Edit; 1] {
    [Edit {
        name: "the decision's check edited",
        apply: match language {
            "typescript" => |d| {
                write(
                    d,
                    ".lighthouse/decisions/probe.yaml",
                    &probe("d", "typescript"),
                )
            },
            _ => |d| write(d, ".lighthouse/decisions/probe.yaml", &probe("d", "python")),
        },
        mostly_warm: false,
    }]
}

pub(crate) fn typescript() -> Option<Fixture> {
    let plugin = lighthouse_test_support::lang_typescript()?;
    let [decision] = common("typescript");
    Some(Fixture {
        name: "typescript",
        language: "typescript",
        toml: toml("lang-typescript", &plugin),
        files: &[
            (
                "tsconfig.json",
                "{\"compilerOptions\":{\"strict\":true,\"target\":\"es2022\",\"module\":\"esnext\",\"moduleResolution\":\"bundler\"},\"include\":[\"src\"]}\n",
            ),
            (
                "src/store.ts",
                "/** A store of numbers. */\nexport class Store {\n  private items: number[] = [];\n\n  /** Adds one. */\n  add(n: number): void {\n    this.items.push(n);\n  }\n\n  /** Sums them. */\n  sum(): number {\n    return this.items.reduce((a, b) => a + b, 0);\n  }\n}\n\n/** Loads the default items. */\nexport function load(): Store {\n  const s = new Store();\n  s.add(1);\n  s.add(2);\n  return s;\n}\n\nfunction bump(n: number): number {\n  return n + 1;\n}\n\n/** Bumps. */\nexport function bumped(): number {\n  return bump(1);\n}\n",
            ),
            (
                "src/server.ts",
                "import { load } from \"./store\";\n\n/** Serves. */\nexport function serve(): number {\n  return load().sum();\n}\n\n/** Doubles. */\nexport function double(n: number): number {\n  return n * 2;\n}\n",
            ),
            (
                "src/util.ts",
                "import { Store } from \"./store\";\n\n/** Totals. */\nexport function total(): number {\n  const s = new Store();\n  s.add(3);\n  return s.sum();\n}\n",
            ),
            (
                "src/app.ts",
                "import { serve } from \"./server\";\nimport { total } from \"./util\";\n\n/** Runs it all. */\nexport function run(): number {\n  return serve() + total();\n}\n",
            ),
        ],
        edits: vec![
            Edit {
                name: "body-only edit",
                apply: |d| replace(d, "src/store.ts", "n + 1", "n + 2"),
                mostly_warm: true,
            },
            Edit {
                name: "signature edit",
                apply: |d| {
                    replace(
                        d,
                        "src/server.ts",
                        "double(n: number): number {\n  return n * 2",
                        "double(n: number, m: number): number {\n  return n * m",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "rename of a called function",
                apply: |d| {
                    replace(
                        d,
                        "src/store.ts",
                        "export function load()",
                        "export function fetch()",
                    );
                    replace(d, "src/server.ts", "import { load }", "import { fetch }");
                    replace(d, "src/server.ts", "return load()", "return fetch()");
                },
                mostly_warm: false,
            },
            Edit {
                name: "file added",
                apply: |d| {
                    write(
                        d,
                        "src/extra.ts",
                        "/** Extra. */\nexport function extra(): number {\n  return 1;\n}\n",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "file renamed",
                apply: |d| fs::rename(d.join("src/extra.ts"), d.join("src/more.ts")).unwrap(),
                mostly_warm: false,
            },
            Edit {
                name: "tsconfig edited",
                apply: |d| replace(d, "tsconfig.json", "\"strict\":true", "\"strict\":false"),
                mostly_warm: false,
            },
            decision,
            Edit {
                name: "file deleted",
                apply: |d| {
                    fs::remove_file(d.join("src/util.ts")).unwrap();
                    replace(d, "src/app.ts", "import { total } from \"./util\";\n", "");
                    replace(d, "src/app.ts", "serve() + total()", "serve()");
                },
                mostly_warm: false,
            },
        ],
    })
}

pub(crate) fn python() -> Option<Fixture> {
    let plugin = lighthouse_test_support::lang_python()?;
    let [decision] = common("python");
    Some(Fixture {
        name: "python",
        language: "python",
        toml: toml("lang-python", &plugin),
        files: &[
            ("pkg/__init__.py", ""),
            (
                "pkg/store.py",
                "\"\"\"A store of numbers.\"\"\"\n\n\nclass Store:\n    \"\"\"Holds numbers.\"\"\"\n\n    def __init__(self) -> None:\n        self.items: list[int] = []\n\n    def add(self, n: int) -> None:\n        \"\"\"Adds one.\"\"\"\n        self.items.append(n)\n\n    def sum(self) -> int:\n        \"\"\"Sums them.\"\"\"\n        return sum(self.items)\n\n\ndef load() -> Store:\n    \"\"\"Loads the default items.\"\"\"\n    s = Store()\n    s.add(1)\n    s.add(2)\n    return s\n\n\ndef bump(n: int) -> int:\n    return n + 1\n\n\ndef bumped() -> int:\n    \"\"\"Bumps.\"\"\"\n    return bump(1)\n",
            ),
            (
                "pkg/server.py",
                "from pkg.store import load\n\n\ndef serve() -> int:\n    \"\"\"Serves.\"\"\"\n    return load().sum()\n\n\ndef double(n: int) -> int:\n    \"\"\"Doubles.\"\"\"\n    return n * 2\n",
            ),
            (
                "pkg/util.py",
                "from pkg.store import Store\n\n\ndef total() -> int:\n    \"\"\"Totals.\"\"\"\n    s = Store()\n    s.add(3)\n    return s.sum()\n",
            ),
            (
                "pkg/app.py",
                "from pkg.server import serve\nfrom pkg.util import total\n\n\ndef run() -> int:\n    \"\"\"Runs it all.\"\"\"\n    return serve() + total()\n",
            ),
        ],
        edits: vec![
            Edit {
                name: "body-only edit",
                apply: |d| replace(d, "pkg/store.py", "n + 1", "n + 2"),
                mostly_warm: true,
            },
            Edit {
                name: "signature edit",
                apply: |d| {
                    replace(
                        d,
                        "pkg/server.py",
                        "def double(n: int) -> int:\n    \"\"\"Doubles.\"\"\"\n    return n * 2",
                        "def double(n: int, m: int) -> int:\n    \"\"\"Doubles.\"\"\"\n    return n * m",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "rename of a called function",
                apply: |d| {
                    replace(d, "pkg/store.py", "def load()", "def fetch()");
                    replace(d, "pkg/server.py", "import load", "import fetch");
                    replace(d, "pkg/server.py", "return load()", "return fetch()");
                },
                mostly_warm: false,
            },
            Edit {
                name: "file added",
                apply: |d| {
                    write(
                        d,
                        "pkg/extra.py",
                        "def extra() -> int:\n    \"\"\"Extra.\"\"\"\n    return 1\n",
                    )
                },
                mostly_warm: false,
            },
            Edit {
                name: "file renamed",
                apply: |d| fs::rename(d.join("pkg/extra.py"), d.join("pkg/more.py")).unwrap(),
                mostly_warm: false,
            },
            decision,
            Edit {
                name: "file deleted",
                apply: |d| {
                    fs::remove_file(d.join("pkg/util.py")).unwrap();
                    replace(d, "pkg/app.py", "from pkg.util import total\n", "");
                    replace(d, "pkg/app.py", "serve() + total()", "serve()");
                },
                mostly_warm: false,
            },
        ],
    })
}
