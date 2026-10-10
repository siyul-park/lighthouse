use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_checks::metrics::{COGNITIVE, CYCLOMATIC};
use lighthouse_engine::Engine;
use lighthouse_model::{File, Project};
use lighthouse_plugin::{Ctx, Facts, Registry, Source, Workspace};
use lighthouse_spec::{Catalog, Config};
use serde_json::Value;

/// The bundled plugins plus the Go plugin built from source; `None` after a
/// skip message when no Go toolchain is available.
fn go_plugin() -> Option<PathBuf> {
    lighthouse_test_support::lang_go()
}

fn registry(plugin: &Path) -> Registry {
    let mut registry = lighthouse_checks::registry();
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = \"lang-go\", path = {:?} }}]",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    lighthouse_rpc::register(&mut registry, &config, Path::new("."), &[]).unwrap();
    registry
}

/// Metric of every function of one Go source, by function name.
fn measure(plugin: &Path, analyzer: &str, source: &str) -> BTreeMap<String, u32> {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "go.mod", "module example.com/app\n");
    write(dir.path(), "p.go", source);
    let registry = registry(plugin);
    let provider = registry
        .languages()
        .map(|(_, l)| l)
        .find(|l| l.manifest().id == "go")
        .unwrap();
    let file = File {
        path: "p.go".into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    };
    let ws = Workspace::new(dir.path().canonicalize().unwrap());
    let indexed = provider
        .index(
            &ws,
            &[Source {
                file: &file,
                text: source,
            }],
        )
        .unwrap();
    assert!(indexed.incomplete.is_empty(), "{:?}", indexed.incomplete);
    let project = Project::merge(indexed.fragments);
    let facts = Facts::new();
    let ctx = Ctx {
        ws: &ws,
        project: &project,
        file: Some((&file, source)),
        facts: &facts,
        keys: &lighthouse_plugin::NoKeys,
        trusted: false,
        memo: &lighthouse_plugin::Memo::default(),
        notices: &lighthouse_plugin::Notices::default(),
        applies: lighthouse_model::Applicability::default(),
    };
    let analyzer = registry.order([analyzer]).unwrap().remove(0);
    let value: Value = analyzer.run(&ctx).unwrap();
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let id = m["symbol"].as_str().unwrap();
            let name = id.split("::").nth(1).unwrap().split('#').next().unwrap();
            (
                name.to_owned(),
                u32::try_from(m["value"].as_u64().unwrap()).unwrap(),
            )
        })
        .collect()
}

const SPEC: &str = r#"package p

func sumOfPrimes(max int) int {
	total := 0
OUT:
	for i := 1; i <= max; i++ {
		for j := 2; j < i; j++ {
			if i%j == 0 {
				continue OUT
			}
		}
		total += i
	}
	return total
}

func words(n int) string {
	switch n {
	case 1:
		return "one"
	case 2, 3:
		return "few"
	default:
		return "many"
	}
}

func nested(a, b bool, xs []int) {
	if a {
		for range xs {
			for b {
			}
		}
	}
	if b {
		if a {
		}
	}
}

func lambda(a bool) {
	f := func() {
		if a {
		}
	}
	f()
}

func chain(a, b int) int {
	if a > 0 {
		return 1
	} else if b > 0 {
		return 2
	} else {
		return 3
	}
}

func logic(a, b, c, d bool) bool {
	if a && b && c || d {
		return true
	}
	return false
}

func fact(n int) int {
	if n <= 1 {
		return 1
	}
	return n * fact(n-1)
}

func straight() {}
"#;

#[test]
fn go_cognitive_complexity_matches_the_specification_examples() {
    let Some(plugin) = go_plugin() else { return };
    let got = measure(&plugin, COGNITIVE, SPEC);
    let want: BTreeMap<String, u32> = [
        ("sumOfPrimes", 7),
        ("words", 1),
        ("nested", 9),
        ("lambda", 2),
        ("chain", 3),
        ("logic", 3),
        ("fact", 2),
        ("straight", 0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn go_cyclomatic_complexity_counts_every_decision_point() {
    let Some(plugin) = go_plugin() else { return };
    let got = measure(&plugin, CYCLOMATIC, SPEC);
    let want: BTreeMap<String, u32> = [
        ("sumOfPrimes", 4),
        ("words", 3),
        ("nested", 6),
        ("lambda", 2),
        ("chain", 3),
        ("logic", 5),
        ("fact", 2),
        ("straight", 1),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(got, want);
}

const STORE: &str = r#"package store

// Store keeps values.
type Store struct{ data map[string]int }

// New returns a store.
func New() *Store { return &Store{data: map[string]int{}} }

func (s *Store) hub() {
	s.left()
	s.right()
}

func (s *Store) Undocumented() {}

func (s *Store) read(k string) int { return s.data[k] }
"#;

const LOAD: &str = r#"package store

func (s *Store) left()  {}
func (s *Store) right() {}

func (s *Store) a() { s.hub() }
func (s *Store) b() { s.hub() }

func (s *Store) load(k string) int { return s.read(k) }

// Get returns a value.
func (s *Store) Get(k string) int { return s.load(k) }

func (s *Store) Put() {}
"#;

const STORE_TEST: &str = r#"package store

import "testing"

func TestHub(t *testing.T) {
	s := New()
	s.hub()
	s.hub()
	s.hub()
}
"#;

const MAIN: &str = r#"package main

import "example.com/app/store"

func main() {
	s := store.New()
	_ = s
}
"#;

fn config(plugin: &Path) -> String {
    format!(
        "plugins = [{{ id = \"lang-go\", path = {:?} }}, \"design\"]\n{CONFIG}",
        plugin.to_str().unwrap()
    )
}

const CONFIG: &str = r#"extends = ["design/recommended"]

[rules]
"design/coupling" = { level = "warn", options = { hubFanIn = 2, hubFanOut = 2, hubStatements = 1 } }
"#;

fn line_of(source: &str, needle: &str) -> u32 {
    let at = source
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not found"));
    u32::try_from(at + 1).unwrap()
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn engine(plugin: &Path, root: &Path, extra: &str) -> Engine {
    let config = Config::parse_inline(&format!("{}{extra}", config(plugin))).unwrap();
    Engine::new(registry(plugin), config, Catalog::bundled(), root).unwrap()
}

#[test]
fn engine_checks_a_multi_file_go_package() {
    let Some(plugin) = go_plugin() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "go.mod", "module example.com/app\n");
    write(root, "store/store.go", STORE);
    write(root, "store/load.go", LOAD);
    write(root, "store/store_test.go", STORE_TEST);
    write(root, "cmd/main.go", MAIN);
    // The original four rules; the ordering and naming rules have their own
    // examples and would also speak about this deliberately small package.
    let only = [
        "design/complexity",
        "design/coupling",
        "design/exported-doc",
        "design/no-single-use-wrapper",
    ]
    .map(str::to_owned);
    let outcome = engine(&plugin, root, "").check(&[], &only).unwrap();

    let found: Vec<(String, u32, String)> = outcome
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.file.to_string_lossy().into_owned(),
                d.span.start.line,
                d.rule_id.clone(),
            )
        })
        .collect();
    let want = [
        (
            "store/load.go",
            line_of(LOAD, "func (s *Store) load"),
            "design/no-single-use-wrapper",
        ),
        (
            "store/load.go",
            line_of(LOAD, "func (s *Store) Put"),
            "design/exported-doc",
        ),
        (
            "store/store.go",
            line_of(STORE, "func (s *Store) hub"),
            "design/coupling",
        ),
        (
            "store/store.go",
            line_of(STORE, "Undocumented"),
            "design/exported-doc",
        ),
    ]
    .map(|(file, line, rule)| (file.to_owned(), line, rule.to_owned()));
    assert_eq!(found, want);
    assert!(outcome.notices.is_empty(), "{:?}", outcome.notices);
    assert!(outcome.incomplete.is_empty(), "{:?}", outcome.incomplete);
    assert_eq!(outcome.exit_code(true, false), 1);
    assert_eq!(outcome.exit_code(false, false), 0);
}

/// The findings of a one-file Go package as (symbol, fingerprint).
fn exported_doc_findings(plugin: &Path, source: &str) -> Vec<(String, String)> {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "go.mod", "module example.com/app\n");
    write(dir.path(), "api.go", source);
    let only = ["design/exported-doc".to_owned()];
    let outcome = engine(plugin, dir.path(), "").check(&[], &only).unwrap();
    outcome
        .diagnostics
        .into_iter()
        .map(|d| (d.symbol.unwrap(), d.fingerprint.as_str().to_owned()))
        .collect()
}

#[test]
fn fingerprints_survive_edits_above_a_go_finding() {
    let Some(plugin) = go_plugin() else { return };
    let before = exported_doc_findings(
        &plugin,
        "package app\n\nfunc Open() {}\n\nfunc Close() {}\n",
    );
    let after = exported_doc_findings(
        &plugin,
        "package app\n\nimport \"fmt\"\n\n// Added is documented.\nfunc Added() { fmt.Println() }\n\n\nfunc Open() {}\n\nfunc Close() {}\n",
    );
    assert_eq!(before.len(), 2, "{before:?}");
    for finding in &before {
        assert!(after.contains(finding), "{finding:?} not in {after:?}");
    }
}

#[test]
fn engine_reports_files_with_syntax_errors_as_incomplete() {
    let Some(plugin) = go_plugin() else { return };
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "go.mod", "module example.com/app\n");
    write(dir.path(), "bad.go", "package p\n\nfunc f( {\n");
    let outcome = engine(&plugin, dir.path(), "").check(&[], &[]).unwrap();
    assert!(outcome.diagnostics.is_empty());
    let gap = outcome
        .incomplete
        .iter()
        .find(|i| i.path.as_deref() == Some(Path::new("bad.go")))
        .expect("bad.go is incomplete");
    assert!(gap.reason.contains("expected"), "{}", gap.reason);
    assert_eq!(outcome.exit_code(true, false), 3);
}

#[test]
fn engine_analyzes_the_build_context_and_notes_excluded_variants() {
    let Some(plugin) = go_plugin() else { return };
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "go.mod", "module example.com/app\n");
    write(
        dir.path(),
        "p/os_linux.go",
        "package p\n\nfunc Name() string { return \"linux\" }\n",
    );
    write(
        dir.path(),
        "p/os_windows.go",
        "package p\n\nfunc Name() string { return \"windows\" }\n",
    );
    let host = engine(&plugin, dir.path(), "").check(&[], &[]).unwrap();
    assert!(host.incomplete.is_empty(), "{:?}", host.incomplete);
    assert_eq!(host.notices.len(), 1, "{:?}", host.notices);
    assert!(
        host.notices
            .iter()
            .next()
            .unwrap()
            .contains("excluded by build constraints")
    );

    let windows = engine(
        &plugin,
        dir.path(),
        "\n[languages.go]\nenv = { GOOS = \"windows\", GOARCH = \"amd64\" }\n",
    )
    .check(&[], &[])
    .unwrap();
    assert!(windows.incomplete.is_empty(), "{:?}", windows.incomplete);
    assert!(
        windows
            .notices
            .iter()
            .next()
            .unwrap()
            .contains("p/os_linux.go")
    );
}

#[test]
fn every_checked_decision_passes_its_catalog_examples() {
    if go_plugin().is_none() {
        return;
    }
    let registry = || registry(&go_plugin().expect("checked above"));
    let failures =
        lighthouse_engine::RuleTester::new(registry, lighthouse_spec::Catalog::bundled())
            .language("go")
            .check_all();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
