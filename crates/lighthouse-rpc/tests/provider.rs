//! Semantic behavior of the Go provider through the RPC adapter: what the
//! engine and rules receive as core model types.

use std::{fs, path::Path, time::Duration};

use lighthouse_model::{
    Capability, EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Node, Project,
    Resolution, Target, TestStyle,
};
use lighthouse_plugin::{Indexed, LanguageProvider, Plugin, Source, Workspace};
use lighthouse_rpc::RpcPlugin;
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    provider: Box<dyn LanguageProvider>,
}

impl Fixture {
    /// `None` after a skip message when no Go toolchain is available.
    fn new(files: &[(&str, &str)]) -> Option<Self> {
        let plugin = lighthouse_testkit::lang_go()?;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::write(root.join("go.mod"), "module example.com/app\n\ngo 1.26\n").unwrap();
        for (path, text) in files {
            write(&root, path, text);
        }
        let found = lighthouse_rpc::load(&plugin).unwrap();
        let rpc = RpcPlugin::connect(&found, &root, Duration::from_secs(120)).unwrap();
        let provider = rpc.languages().remove(0);
        Some(Self { dir, provider })
    }

    fn root(&self) -> std::path::PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn add(&self, path: &str, text: &str) {
        write(&self.root(), path, text);
    }

    fn index(&self) -> Indexed {
        let root = self.root();
        let mut paths = Vec::new();
        collect(&root, &root, &mut paths);
        paths.sort();
        let files: Vec<File> = paths
            .iter()
            .map(|p| File {
                path: p.into(),
                lang: "go".to_owned(),
                hash: String::new(),
                generated: false,
                test: p.ends_with("_test.go"),
            })
            .collect();
        let texts: Vec<String> = paths
            .iter()
            .map(|p| fs::read_to_string(root.join(p)).unwrap())
            .collect();
        let sources: Vec<Source> = files
            .iter()
            .zip(&texts)
            .map(|(file, text)| Source { file, text })
            .collect();
        self.provider
            .index(&Workspace::new(&root), &sources)
            .unwrap()
    }

    /// The indexed fragment of one file.
    fn fragment(&self, indexed: &Indexed, path: &str) -> Fragment {
        let want = Path::new(path);
        indexed
            .fragments
            .iter()
            .find(|f| f.files[0].path == want)
            .unwrap_or_else(|| panic!("no fragment for {path}"))
            .clone()
    }
}

fn write(root: &Path, path: &str, text: &str) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, text).unwrap();
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "go") {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
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

fn summary<'f>(fragment: &'f Fragment, name: &str) -> &'f FunctionSummary {
    fragment
        .functions
        .iter()
        .find(|f| f.symbol.as_str().contains(name))
        .unwrap_or_else(|| panic!("no summary for {name}"))
}

fn flow(kind: FlowKind, nesting: u32) -> Flow {
    Flow::new(kind, nesting)
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

const UTIL: &str = "package util\n\n// Zero is zero.\nfunc Zero() int { return 0 }\n";

fn store() -> [(&'static str, &'static str); 2] {
    [
        ("internal/store/store.go", STORE),
        ("internal/util/util.go", UTIL),
    ]
}

#[test]
fn provider_claims_go_files_and_offers_semantic_edges() {
    let Some(f) = Fixture::new(&[]) else { return };
    assert_eq!(f.provider.id(), "go");
    assert_eq!(f.provider.globs(), ["**/*.go"]);
    assert_eq!(f.provider.conventions().test_globs, ["**/*_test.go"]);
    assert_eq!(f.provider.capabilities(), [Capability::SemanticEdges]);
    assert!(!f.provider.fallback());
}

#[test]
fn symbols_carry_kind_visibility_owner_doc_and_span() {
    let Some(f) = Fixture::new(&store()) else {
        return;
    };
    let indexed = f.index();
    assert!(indexed.incomplete.is_empty(), "{:?}", indexed.incomplete);
    let fragment = f.fragment(&indexed, "internal/store/store.go");
    let table: Vec<String> = fragment
        .symbols
        .iter()
        .map(|s| {
            let owner = s.owner.as_ref().map_or("-", |o| o.as_str());
            format!(
                "{} {:?} doc={} owner={owner} line={}",
                s.id.as_str(),
                s.visibility,
                s.doc.is_some(),
                s.span.start.line
            )
        })
        .collect();
    assert_eq!(
        table,
        [
            "internal/store::Store#type Internal doc=true owner=- line=10",
            "internal/store::Store::items#field Private doc=false owner=internal/store::Store#type line=11",
            "internal/store::Store::Name#field Internal doc=false owner=internal/store::Store#type line=12",
            "internal/store::Reader#interface Internal doc=false owner=- line=15",
            "internal/store::Reader::Read#method Internal doc=false owner=internal/store::Reader#interface line=15",
            "internal/store::Limit#const Internal doc=false owner=- line=17",
            "internal/store::cache#var Private doc=false owner=- line=19",
            "internal/store::Store::Get#method Internal doc=true owner=internal/store::Store#type line=22",
            "internal/store::helper#function Private doc=false owner=- line=30",
            "internal/store::New#function Internal doc=false owner=- line=32",
            "internal/store::init:store.go:34#function Private doc=false owner=- line=34",
        ]
    );
    assert_eq!(
        fragment.symbols[0].doc.as_deref(),
        Some("Store keeps values.")
    );
    let span = fragment.symbols[0].span;
    assert_eq!((span.start.line, span.end.line), (10, 13));
}

#[test]
fn group_docs_cover_every_spec_and_trailing_comments_do_not_leak() {
    let source = "package p\n\n// Limits bound things.\nconst (\n\tMin = 1\n\tMax = 3\n)\n\nvar a = 1 // trailing\nvar B = 2\n\n/* Block form. */\nfunc C() {}\n";
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let doc = |name: &str| {
        fragment
            .symbols
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
fn module_is_the_package_directory_and_imports_resolve_to_project_modules() {
    let Some(f) = Fixture::new(&store()) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "internal/store/store.go");
    assert_eq!(fragment.modules.len(), 1);
    assert_eq!(fragment.modules[0].path, "internal/store");
    assert_eq!(fragment.modules[0].name.as_deref(), Some("store"));
    assert_eq!(fragment.modules[0].test_of, None);
    assert_eq!(
        edges(&fragment, EdgeKind::Imports, "internal/store"),
        ["fmt", "internal/util"]
    );
    assert_eq!(
        edges(&fragment, EdgeKind::Contains, "internal/store::Store#type"),
        [
            "internal/store::Store::items#field",
            "internal/store::Store::Name#field",
            "internal/store::Store::Get#method"
        ]
    );
}

#[test]
fn imports_of_packages_that_failed_to_load_still_map_into_the_project() {
    let source = "package p\n\nimport _ \"example.com/app/internal/missing\"\n";
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    assert_eq!(
        edges(&f.fragment(&indexed, "p.go"), EdgeKind::Imports, "."),
        ["internal/missing"]
    );
    assert!(
        indexed
            .incomplete
            .iter()
            .any(|i| i.path.as_deref() == Some(Path::new("p.go")))
    );
}

#[test]
fn calls_and_references_resolve_through_types_and_leave_out_targets_outside_the_project() {
    let Some(f) = Fixture::new(&store()) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "internal/store/store.go");
    assert_eq!(
        edges(&fragment, EdgeKind::Calls, "Store::Get"),
        ["internal/util::Zero", "internal/store::helper"]
    );
    assert_eq!(
        edges(&fragment, EdgeKind::References, "Store::Get"),
        ["internal/store::Store::items"]
    );
    assert!(
        fragment
            .edges
            .iter()
            .all(|e| e.resolution == Resolution::Semantic)
    );
}

#[test]
fn local_declarations_shadow_package_symbols() {
    let source = "package p\n\nfunc helper() {}\n\nfunc f(helper func()) {\n\thelper()\n\tvalue := 1\n\t_ = value\n}\n\nfunc g() { helper() }\n";
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    assert!(edges(&fragment, EdgeKind::Calls, "::f#").is_empty());
    assert_eq!(edges(&fragment, EdgeKind::Calls, "::g#"), [".::helper"]);
}

#[test]
fn methods_resolve_by_receiver_and_privacy_is_not_an_edge_in_go() {
    let source = r#"package p

type A struct{ secret int }
type B struct{ hidden int }

func (a *A) Use(b *B) {
	b.run()
	_ = a.secret
	_ = b.hidden
	c := &A{secret: 1}
	c.Use(b)
}

func (b *B) run() {}

func New() *A { return &A{secret: 2} }
"#;
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    assert_eq!(
        edges(&fragment, EdgeKind::Calls, "A::Use"),
        [".::B::run", ".::A::Use"]
    );
    assert!(
        fragment
            .edges
            .iter()
            .all(|e| e.kind != EdgeKind::AccessesPrivate),
        "unexported access across types cannot cross a package in Go"
    );
    assert_eq!(
        edges(&fragment, EdgeKind::References, "::New#"),
        [".::A::secret", ".::A"]
    );
}

#[test]
fn merge_resolves_same_package_targets_across_files() {
    let Some(f) = Fixture::new(&[
        ("pkg/a.go", "package pkg\n\nfunc A() { b() }\n"),
        ("pkg/b.go", "package pkg\n\nfunc b() {}\n"),
        (
            "cmd/main.go",
            "package main\n\nimport \"example.com/app/pkg\"\n\nfunc main() { pkg.A() }\n",
        ),
    ]) else {
        return;
    };
    let project = Project::merge(f.index().fragments);
    let calls: Vec<_> = project
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| target(&e.to))
        .collect();
    assert_eq!(calls, ["pkg::A#function", "pkg::b#function"]);
    assert!(
        project.edges.iter().any(|e| e.kind == EdgeKind::Imports
            && e.to == Target::Resolved(Node::Module("pkg".to_owned())))
    );
    let b = project.symbols.iter().find(|s| s.name == "b").unwrap();
    assert_eq!(project.callers(&b.id).len(), 1);
}

#[test]
fn external_test_package_is_its_own_module_of_the_package_under_test() {
    let Some(f) = Fixture::new(&[
        ("pkg/pkg.go", "package pkg\n\nfunc Run() {}\n"),
        (
            "pkg/pkg_test.go",
            "package pkg_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/app/pkg\"\n)\n\nfunc TestRun(t *testing.T) { pkg.Run() }\n",
        ),
        (
            "pkg/in_test.go",
            "package pkg\n\nimport \"testing\"\n\nfunc TestIn(t *testing.T) {}\n",
        ),
    ]) else {
        return;
    };
    let indexed = f.index();
    let external = f.fragment(&indexed, "pkg/pkg_test.go");
    assert_eq!(external.modules[0].path, "pkg[test]");
    assert_eq!(external.modules[0].name.as_deref(), Some("pkg_test"));
    assert_eq!(external.modules[0].test_of.as_deref(), Some("pkg"));
    assert_eq!(
        f.fragment(&indexed, "pkg/in_test.go").modules[0].path,
        "pkg"
    );
    let project = Project::merge(indexed.fragments);
    let case = project
        .tests
        .iter()
        .find(|t| t.symbol.as_str().starts_with("pkg[test]"))
        .unwrap();
    let run = project.symbols.iter().find(|s| s.name == "Run").unwrap();
    assert!(
        case.targets
            .contains(&Target::Resolved(Node::Symbol(run.id.clone())))
    );
}

#[test]
fn test_cases_report_run_depth_style_and_targets() {
    let source = r#"package p

import "testing"

func Run(int) {}

func TestTable(t *testing.T) {
	tests := []struct{ in int }{{1}, {2}}
	for _, tc := range tests {
		t.Run("case", func(t *testing.T) { Run(tc.in) })
	}
}

func TestNamedTable(t *testing.T) {
	type tc struct{ in int }
	for _, c := range []tc{{1}} {
		t.Run("case", func(t *testing.T) { Run(c.in) })
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
    let Some(f) = Fixture::new(&[("p_test.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p_test.go");
    let cases: Vec<_> = fragment
        .tests
        .iter()
        .map(|c| (c.symbol.as_str(), c.nesting, c.style))
        .collect();
    assert_eq!(
        cases,
        [
            (".::TestTable#test", 1, TestStyle::Table),
            (".::TestNamedTable#test", 1, TestStyle::Table),
            (".::TestNested#test", 2, TestStyle::Scenario),
            (".::TestPlain#test", 0, TestStyle::Scenario),
        ]
    );
    let table: Vec<_> = fragment.tests[0].targets.iter().map(target).collect();
    assert_eq!(table, [".::Run"]);
    assert_eq!(
        fragment.tests[3].targets,
        [Target::Path(".::helper".to_owned())]
    );
    let kinds: Vec<_> = fragment.symbols.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "function", "test", "test", "test", "test", "function", "function"
        ]
    );
}

#[test]
fn summary_counts_statements_signature_and_tokens() {
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
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let s = summary(&fragment, "::f#");
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
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let s = summary(&fragment, "::f#");
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
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    assert_eq!(
        summary(&fragment, "walk").flow,
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
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    assert_eq!(
        summary(&fragment, "::f#").flow,
        [switch(2, false), switch(2, false)]
    );
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
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let name = summary(&fragment, "::name#");
    assert_eq!(
        (name.top_level, name.flow.clone()),
        (1, vec![switch(1, true)])
    );
    assert_eq!(summary(&fragment, "::mixed#").flow, [switch(1, false)]);
    assert_eq!(
        summary(&fragment, "S::load").forwards_to,
        Some(Target::Path(".::S::read".to_owned()))
    );
    assert_eq!(
        summary(&fragment, "::wrap#").forwards_to,
        Some(Target::Path(".::inner".to_owned()))
    );
    assert_eq!(summary(&fragment, "::reorder#").forwards_to, None);
    assert_eq!(summary(&fragment, "S::read").forwards_to, None);
}

#[test]
fn generated_files_are_flagged_and_testdata_is_not_indexed() {
    let Some(f) = Fixture::new(&[
        (
            "gen.go",
            "// Code generated by tool. DO NOT EDIT.\n\npackage p\n\nfunc F() {}\n",
        ),
        ("plain.go", "package p\n"),
        ("p/testdata/x.go", "package x\n\nfunc F() {}\n"),
    ]) else {
        return;
    };
    let indexed = f.index();
    let generated = f.fragment(&indexed, "gen.go");
    assert!(generated.files[0].generated);
    assert_eq!(generated.symbols.len(), 1);
    assert!(!f.fragment(&indexed, "plain.go").files[0].generated);
    let ignored = f.fragment(&indexed, "p/testdata/x.go");
    assert!(ignored.symbols.is_empty());
    assert!(indexed.incomplete.is_empty() && indexed.notices.is_empty());
}

#[test]
fn syntax_errors_are_incomplete_with_a_position_and_never_crash_the_run() {
    let Some(f) = Fixture::new(&[
        ("p/bad.go", "package p\n\nfunc f( {\n"),
        ("q/ok.go", "package q\n\nfunc G() {}\n"),
    ]) else {
        return;
    };
    let indexed = f.index();
    assert_eq!(indexed.incomplete.len(), 1);
    let gap = &indexed.incomplete[0];
    assert_eq!(gap.path.as_deref(), Some(Path::new("p/bad.go")));
    assert!(gap.reason.starts_with("3:"), "{}", gap.reason);
    assert_eq!(f.fragment(&indexed, "q/ok.go").symbols.len(), 1);
}

#[test]
fn interfaces_are_implemented_by_value_or_pointer_and_generics_resolve() {
    let source = r#"package p

type Shape interface {
	Area() float64
	Perimeter() float64
}

type Namer interface{ Name() string }

type Square struct{ side float64 }

func (s Square) Area() float64 { return s.side * s.side }

func (s Square) Perimeter() float64 { return 4 * s.side }

func (s Square) Name() string { return "square" }

type Circle struct{ r float64 }

func (c *Circle) Area() float64 { return c.r }

func (c *Circle) Perimeter() float64 { return c.r }

type Plain struct{}

func Map[T, U any](in []T, f func(T) U) []U { return nil }

func use() []int { return Map[string, int](nil, func(s string) int { return len(s) }) }
"#;
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let implements: Vec<_> = fragment
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .map(|e| format!("{:?} -> {}", e.from, target(&e.to)))
        .collect();
    assert_eq!(
        implements.len(),
        2,
        "single-method interfaces are skipped: {implements:?}"
    );
    assert!(implements.iter().all(|e| e.ends_with("-> .::Shape")));
    assert_eq!(edges(&fragment, EdgeKind::Calls, "::use#"), [".::Map"]);
}

#[test]
fn fragments_are_deterministic() {
    let Some(f) = Fixture::new(&store()) else {
        return;
    };
    assert_eq!(f.index(), f.index());
    f.add("internal/util/extra.go", "package util\n");
    assert_eq!(f.index().fragments.len(), 3);
}
