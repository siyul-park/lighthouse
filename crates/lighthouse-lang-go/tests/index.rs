use std::fs;

use lighthouse_model::{
    Capability, EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Node, Project, Target,
    TestStyle, Visibility,
};
use lighthouse_plugin::{Error, LanguageProvider, Plugin, Workspace};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    provider: Box<dyn LanguageProvider>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
        Self {
            dir,
            provider: lighthouse_lang_go::LangGo.languages().remove(0),
        }
    }

    fn try_index(&self, path: &str, source: &str) -> Result<Fragment, Error> {
        let file = File {
            path: path.into(),
            lang: "go".to_owned(),
            hash: String::new(),
            generated: false,
            test: path.ends_with("_test.go"),
        };
        let ws = Workspace {
            root: self.dir.path().to_owned(),
        };
        self.provider.index(&ws, &file, source)
    }

    fn index(&self, path: &str, source: &str) -> Fragment {
        self.try_index(path, source).unwrap()
    }
}

fn summary<'f>(fragment: &'f Fragment, name: &str) -> &'f FunctionSummary {
    fragment
        .functions
        .iter()
        .find(|f| f.symbol.as_str().contains(name))
        .unwrap_or_else(|| panic!("no summary for {name}"))
}

fn target(to: &Target) -> String {
    match to {
        Target::Path(p) => p.clone(),
        Target::Resolved(Node::Symbol(id)) => id.as_str().to_owned(),
        Target::Resolved(Node::Module(m)) => m.clone(),
    }
}

/// Targets of edges of `kind` leaving the symbol or module whose id contains `from`.
fn edges(fragment: &Fragment, kind: EdgeKind, from: &str) -> Vec<String> {
    fragment
        .edges
        .iter()
        .filter(|e| e.kind == kind)
        .filter(|e| match &e.from {
            Node::Symbol(id) => id.as_str().contains(from),
            Node::Module(m) => m == from,
        })
        .map(|e| target(&e.to))
        .collect()
}

fn switch(arms: u32, returning: bool) -> Flow {
    Flow {
        arms,
        returning,
        ..Flow::new(FlowKind::Switch, 0)
    }
}

fn logic(operators: u32) -> Flow {
    Flow {
        operators,
        ..Flow::new(FlowKind::Logic, 0)
    }
}

fn flow(kind: FlowKind, nesting: u32) -> Flow {
    Flow::new(kind, nesting)
}

const STORE: &str = r#"package store

import (
	"fmt"
	util "example.com/app/internal/util"
)

// Store keeps values.
type Store struct {
	items map[string]int
	Name  string
}

type Reader interface{ Read() }

const Limit = 3

var cache = map[string]int{}

// Get returns a value.
func (s *Store) Get(k string) int {
	if k == "" {
		return util.Zero()
	}
	fmt.Println(helper(k))
	return s.items[k]
}

func helper(k string) int { return len(k) }

func New() *Store { return &Store{items: map[string]int{}} }

func init() {}
"#;

#[test]
fn provider_is_syntactic_and_claims_go_files() {
    let provider = lighthouse_lang_go::Go::new();
    assert_eq!(provider.id(), "go");
    assert_eq!(provider.globs(), ["**/*.go"]);
    assert_eq!(provider.conventions().test_globs, ["**/*_test.go"]);
    assert!(!provider.capabilities().contains(&Capability::SemanticEdges));
    assert_eq!(lighthouse_lang_go::LangGo.manifest().id, "lang-go");
}

#[test]
fn symbols_carry_kind_visibility_owner_doc_and_span() {
    let f = Fixture::new().index("internal/store/store.go", STORE);
    let table: Vec<String> = f
        .symbols
        .iter()
        .map(|s| {
            let vis = match s.visibility {
                Visibility::Public => "pub",
                _ => "priv",
            };
            let owner = s.owner.as_ref().map_or("-", |o| o.as_str());
            format!(
                "{} {vis} doc={} owner={owner} line={}",
                s.id.as_str(),
                s.doc.is_some(),
                s.span.start.line
            )
        })
        .collect();
    assert_eq!(
        table,
        [
            "internal/store::Store#type pub doc=true owner=- line=9",
            "internal/store::Store::items#field priv doc=false owner=internal/store::Store#type line=10",
            "internal/store::Store::Name#field pub doc=false owner=internal/store::Store#type line=11",
            "internal/store::Reader#interface pub doc=false owner=- line=14",
            "internal/store::Limit#const pub doc=false owner=- line=16",
            "internal/store::cache#var priv doc=false owner=- line=18",
            "internal/store::Store::Get#method pub doc=true owner=internal/store::Store#type line=21",
            "internal/store::helper#function priv doc=false owner=- line=29",
            "internal/store::New#function pub doc=false owner=- line=31",
            "internal/store::init:store.go:33#function priv doc=false owner=- line=33",
        ]
    );
    assert!(f.symbols.iter().all(|s| s.file.ends_with("store.go")));
    assert_eq!(f.symbols[0].doc.as_deref(), Some("Store keeps values."));
}

#[test]
fn group_docs_cover_every_spec_and_trailing_comments_do_not_leak() {
    let source = "package p\n\n// Limits bound things.\nconst (\n\tMin = 1\n\tMax = 3\n)\n\nvar a = 1 // trailing\nvar B = 2\n\n/* Block form. */\nfunc C() {}\n";
    let f = Fixture::new().index("p.go", source);
    let doc = |name: &str| {
        f.symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap()
            .doc
            .clone()
    };
    assert_eq!(doc("Min").as_deref(), Some("Limits bound things."));
    assert_eq!(doc("Max").as_deref(), Some("Limits bound things."));
    assert_eq!(doc("B"), None);
    assert_eq!(doc("C").as_deref(), Some("Block form."));
}

#[test]
fn module_is_the_package_directory_and_imports_resolve_through_go_mod() {
    let f = Fixture::new().index("internal/store/store.go", STORE);
    assert_eq!(f.modules.len(), 1);
    assert_eq!(f.modules[0].path, "internal/store");
    assert_eq!(f.modules[0].test_of, None);
    assert_eq!(
        edges(&f, EdgeKind::Imports, "internal/store"),
        ["fmt", "internal/util"]
    );
    let contained = edges(&f, EdgeKind::Contains, "internal/store::Store#type");
    assert_eq!(
        contained,
        [
            "internal/store::Store::items#field",
            "internal/store::Store::Name#field",
            "internal/store::Store::Get#method"
        ]
    );
}

#[test]
fn calls_and_references_use_syntactic_paths() {
    let f = Fixture::new().index("internal/store/store.go", STORE);
    assert_eq!(
        edges(&f, EdgeKind::Calls, "Store::Get"),
        [
            "internal/util::Zero",
            "fmt::Println",
            "internal/store::helper"
        ]
    );
    assert_eq!(
        edges(&f, EdgeKind::References, "Store::Get"),
        ["internal/store::Store::items"]
    );
    assert!(
        f.edges
            .iter()
            .all(|e| e.resolution == lighthouse_model::Resolution::Syntactic)
    );
}

#[test]
fn local_declarations_shadow_package_symbols() {
    let source = "package p\n\nfunc helper() {}\n\nfunc f(helper func()) {\n\thelper()\n\tvalue := 1\n\t_ = value\n}\n\nfunc g() { helper() }\n";
    let f = Fixture::new().index("p.go", source);
    assert!(edges(&f, EdgeKind::Calls, "::f#").is_empty());
    assert_eq!(edges(&f, EdgeKind::Calls, "::g#"), [".::helper"]);
}

#[test]
fn typed_locals_resolve_method_calls_and_flag_foreign_private_access() {
    let source = r#"package p

type A struct{ secret int }
type B struct{}

func (a *A) Use(b *B) {
	b.run()
	_ = a.secret
	_ = b.hidden
	c := &A{secret: 1}
	c.Use(b)
}

func (b *B) run() {}
"#;
    let f = Fixture::new().index("p.go", source);
    assert_eq!(
        edges(&f, EdgeKind::Calls, "A::Use"),
        [".::B::run", ".::A::Use"]
    );
    assert_eq!(
        edges(&f, EdgeKind::AccessesPrivate, "A::Use"),
        [".::B::run", ".::B::hidden"]
    );
}

#[test]
fn merge_resolves_same_package_targets_across_files() {
    let fixture = Fixture::new();
    let a = fixture.index("pkg/a.go", "package pkg\n\nfunc A() { b() }\n");
    let b = fixture.index("pkg/b.go", "package pkg\n\nfunc b() {}\n");
    let other = fixture.index(
        "cmd/main.go",
        "package main\n\nimport \"example.com/app/pkg\"\n\nfunc main() { pkg.A() }\n",
    );
    let project = Project::merge([a, b, other]);
    let calls: Vec<_> = project
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| target(&e.to))
        .collect();
    assert_eq!(calls, ["pkg::b#function", "pkg::A#function"]);
    assert!(
        project.edges.iter().any(|e| e.kind == EdgeKind::Imports
            && e.to == Target::Resolved(Node::Module("pkg".to_owned())))
    );
    let callers = project.callers(&project.symbols.iter().find(|s| s.name == "b").unwrap().id);
    assert_eq!(callers.len(), 1);
}

#[test]
fn external_test_package_is_its_own_module_of_the_package_under_test() {
    let fixture = Fixture::new();
    let tested = fixture.index("pkg/pkg.go", "package pkg\n\nfunc Run() {}\n");
    let tests = fixture.index(
        "pkg/pkg_test.go",
        "package pkg_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/app/pkg\"\n)\n\nfunc TestRun(t *testing.T) { pkg.Run() }\n",
    );
    assert_eq!(tests.modules[0].path, "pkg[test]");
    assert_eq!(tests.modules[0].test_of.as_deref(), Some("pkg"));
    let internal = fixture.index(
        "pkg/in_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestIn(t *testing.T) {}\n",
    );
    assert_eq!(internal.modules[0].path, "pkg");
    let project = Project::merge([tested, tests]);
    let case = &project.tests[0];
    assert_eq!(
        case.targets,
        [Target::Resolved(Node::Symbol(
            project.symbols[0].id.clone()
        ))]
    );
}

#[test]
fn test_cases_report_run_depth_style_and_targets() {
    let source = r#"package p

import "testing"

func TestTable(t *testing.T) {
	tests := []struct{ in int }{{1}, {2}}
	for _, tc := range tests {
		t.Run("case", func(t *testing.T) { Run(tc.in) })
	}
}

func TestNested(t *testing.T) {
	t.Run("outer", func(t *testing.T) {
		t.Run("inner", func(t *testing.T) { Run(1) })
	})
}

func TestPlain(t *testing.T) { helper(t) }

func Testing() {}

func helper(t *testing.T) {}
"#;
    let f = Fixture::new().index("p_test.go", source);
    let cases: Vec<_> = f
        .tests
        .iter()
        .map(|c| (c.symbol.as_str(), c.nesting, c.style))
        .collect();
    assert_eq!(
        cases,
        [
            (".::TestTable#test", 1, TestStyle::Table),
            (".::TestNested#test", 2, TestStyle::Scenario),
            (".::TestPlain#test", 0, TestStyle::Scenario),
        ]
    );
    let table: Vec<_> = f.tests[0].targets.iter().map(target).collect();
    assert_eq!(table, ["testing::T::Run", ".::Run"]);
    assert_eq!(f.tests[2].targets, [Target::Path(".::helper".to_owned())]);
    let kinds: Vec<_> = f.symbols.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["test", "test", "test", "function", "function"]);
}

#[test]
fn summary_counts_decisions_statements_signature_and_tokens() {
    let source = r#"package p

func f(a, b int, xs ...string) (int, error) {
	for _, x := range xs {
		if x == "" {
			return 0, nil
		}
	}
	return a + b, nil
}
"#;
    let f = Fixture::new().index("p.go", source);
    let s = summary(&f, "::f#");
    assert_eq!((s.max_nesting, s.statements, s.top_level), (2, 4, 2));
    assert_eq!((s.params, s.returns), (3, 2));
    assert!(s.tokens > 20);
    assert!(s.forwards_to.is_none());
}

#[test]
fn flow_events_normalize_go_control_flow() {
    let source = r#"package p

func f(n int, xs []int) int {
	if n > 0 && n < 10 || n == 42 {
		for _, x := range xs {
			if x == 0 {
				continue
			} else if x < 0 {
				break
			} else {
				n++
			}
		}
	}
	switch n {
	case 1:
		n = 2
	}
	g := func() {
		if n > 3 {
			n--
		}
	}
	g()
	if n == 7 {
		goto done
	}
done:
	return f(n-1, xs)
}
"#;
    let f = Fixture::new().index("p.go", source);
    let s = summary(&f, "::f#");
    assert_eq!(
        s.flow,
        [
            flow(FlowKind::If, 0),
            logic(1),
            logic(1),
            flow(FlowKind::Loop, 1),
            flow(FlowKind::If, 2),
            flow(FlowKind::ElseIf, 2),
            flow(FlowKind::Else, 2),
            switch(1, false),
            flow(FlowKind::If, 1),
            flow(FlowKind::If, 0),
            flow(FlowKind::Jump, 1),
            flow(FlowKind::Recursion, 0),
        ]
    );
    assert_eq!(s.max_nesting, 3);
    assert_eq!(s.statements, 16);
}

#[test]
fn labeled_jumps_and_method_recursion_are_flow_events() {
    let source = "package p\n\ntype T struct{}\n\nfunc (t *T) walk(n int) {\nouter:\n\tfor i := 0; i < n; i++ {\n\t\tfor {\n\t\t\tcontinue outer\n\t\t}\n\t}\n\tt.walk(n - 1)\n}\n";
    let f = Fixture::new().index("p.go", source);
    let s = summary(&f, "walk");
    assert_eq!(
        s.flow,
        [
            flow(FlowKind::Loop, 0),
            flow(FlowKind::Loop, 1),
            flow(FlowKind::Jump, 2),
            flow(FlowKind::Recursion, 0),
        ]
    );
}

#[test]
fn select_and_type_switch_count_arms_as_decisions() {
    let source = r#"package p

func f(v any, a, b chan int) {
	switch v.(type) {
	case int:
	case string:
	default:
	}
	select {
	case <-a:
	case <-b:
	default:
	}
}
"#;
    let f = Fixture::new().index("p.go", source);
    let s = summary(&f, "::f#");
    assert_eq!(s.flow, [switch(2, false), switch(2, false)]);
}

#[test]
fn dispatchers_and_forwarders_are_recognized() {
    let source = r#"package p

type S struct{}

func name(k int) string {
	switch k {
	case 0:
		return "a"
	default:
		return "b"
	}
}

func mixed(k int) string {
	switch k {
	case 0:
		return "a"
	default:
		k++
		return "b"
	}
}

func (s *S) load(k string) error { return s.read(k) }

func (s *S) read(k string) error { return nil }

func wrap(a int, rest ...int) { inner(a, rest...) }

func reorder(a, b int) { inner(b, a) }

func inner(a int, rest ...int) {}
"#;
    let f = Fixture::new().index("p.go", source);
    let name = summary(&f, "::name#");
    assert_eq!(
        (name.top_level, name.flow.clone()),
        (1, vec![switch(1, true)])
    );
    let mixed = summary(&f, "::mixed#");
    assert_eq!(mixed.flow, [switch(1, false)]);
    assert_eq!(
        summary(&f, "S::load").forwards_to,
        Some(Target::Path(".::S::read".to_owned()))
    );
    assert_eq!(
        summary(&f, "::wrap#").forwards_to,
        Some(Target::Path(".::inner".to_owned()))
    );
    assert_eq!(summary(&f, "::reorder#").forwards_to, None);
    assert_eq!(summary(&f, "S::read").forwards_to, None);
}

#[test]
fn generated_files_are_flagged_and_testdata_is_not_indexed() {
    let fixture = Fixture::new();
    let generated = fixture.index(
        "gen.go",
        "// Code generated by tool. DO NOT EDIT.\n\npackage p\n\nfunc F() {}\n",
    );
    assert!(generated.files[0].generated);
    assert_eq!(generated.symbols.len(), 1);
    let plain = fixture.index("plain.go", "package p\n");
    assert!(!plain.files[0].generated);
    let fixture_dir = fixture.index("p/testdata/x.go", "package x\n\nfunc F() {}\n");
    assert!(fixture_dir.symbols.is_empty());
    assert_eq!(fixture_dir.files.len(), 1);
}

#[test]
fn syntax_errors_fail_the_file_with_a_line() {
    let err = Fixture::new()
        .try_index("bad.go", "package p\n\nfunc f( {\n")
        .unwrap_err();
    assert!(err.to_string().contains("syntax error at line"), "{err}");
}

#[test]
fn fragments_are_deterministic() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.index("internal/store/store.go", STORE),
        fixture.index("internal/store/store.go", STORE)
    );
}
