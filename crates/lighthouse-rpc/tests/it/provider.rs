//! Semantic behavior of the Go provider through the RPC adapter: what the
//! engine and rules receive as core model types.

mod fixture;
mod shape;

use std::path::Path;

use fixture::*;
use lighthouse_model::ucm::{Capability, EdgeKind, Node, Project, Resolution, Target};

#[test]
fn provider_claims_go_files_and_offers_semantic_edges() {
    let Some(f) = Fixture::new(&[]) else { return };
    let manifest = f.provider.manifest();
    assert_eq!(manifest.id, "go");
    assert_eq!(manifest.globs, ["**/*.go"]);
    assert_eq!(manifest.conventions.test_globs, ["**/*_test.go"]);
    assert_eq!(
        manifest.capabilities,
        [
            Capability::SemanticEdges,
            Capability::Extent,
            Capability::ReferenceSites,
            Capability::Overlays
        ]
    );
    assert!(!manifest.fallback);
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
