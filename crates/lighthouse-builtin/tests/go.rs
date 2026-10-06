use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_config::Config;
use lighthouse_engine::Engine;
use lighthouse_metrics::{COGNITIVE, CYCLOMATIC};
use lighthouse_model::{File, Project};
use lighthouse_plugin::{Ctx, Facts, Workspace};
use serde_json::Value;

/// Metric of every function of one Go source, by function name.
fn measure(analyzer: &str, source: &str) -> BTreeMap<String, u32> {
    let registry = lighthouse_builtin::registry();
    let provider = registry
        .languages()
        .map(|(_, l)| l)
        .find(|l| l.id() == "go")
        .unwrap();
    let file = File {
        path: "p.go".into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    };
    let ws = Workspace { root: ".".into() };
    let project = Project::merge([provider.index(&ws, &file, source).unwrap()]);
    let facts = Facts::new();
    let ctx = Ctx {
        ws: &ws,
        project: &project,
        file: Some((&file, source)),
        facts: &facts,
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
    let got = measure(COGNITIVE, SPEC);
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
    let got = measure(CYCLOMATIC, SPEC);
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

const CONFIG: &str = r#"plugins = ["lang-go", "design"]
extends = ["design/recommended"]

[rules]
"design/coupling-signal" = { level = "warn", hub_fan_in = 2, hub_fan_out = 2, hub_statements = 1 }
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

#[test]
fn engine_checks_a_multi_file_go_package() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "go.mod", "module example.com/app\n");
    write(root, "store/store.go", STORE);
    write(root, "store/load.go", LOAD);
    write(root, "store/store_test.go", STORE_TEST);
    write(root, "cmd/main.go", MAIN);
    let config = Config::parse(CONFIG).unwrap();
    let engine = Engine::new(lighthouse_builtin::registry(), config, root).unwrap();
    let outcome = engine.check(&[], &[]).unwrap();

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
            "design/single-use-wrapper",
        ),
        (
            "store/load.go",
            line_of(LOAD, "func (s *Store) Put"),
            "design/exported-doc",
        ),
        (
            "store/store.go",
            line_of(STORE, "func (s *Store) hub"),
            "design/coupling-signal",
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
    assert_eq!(outcome.exit_code(true), 1);
    assert_eq!(outcome.exit_code(false), 0);
}

#[test]
fn engine_skips_files_with_syntax_errors_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "bad.go", "package p\n\nfunc f( {\n");
    let config = Config::parse(CONFIG).unwrap();
    let engine = Engine::new(lighthouse_builtin::registry(), config, dir.path()).unwrap();
    let outcome = engine.check(&[], &[]).unwrap();
    assert!(outcome.diagnostics.is_empty());
    assert!(
        outcome
            .notices
            .iter()
            .any(|n| n.contains("bad.go") && n.contains("syntax error"))
    );
}

#[test]
fn engine_reports_symbols_declared_in_several_build_variants() {
    let dir = tempfile::tempdir().unwrap();
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
    let config = Config::parse(CONFIG).unwrap();
    let engine = Engine::new(lighthouse_builtin::registry(), config, dir.path()).unwrap();
    let outcome = engine.check(&[], &[]).unwrap();
    let notice = outcome.notices.iter().next().expect("a notice");
    assert!(
        notice.contains("p::Name#function (p/os_linux.go, p/os_windows.go)"),
        "{notice}"
    );
}
