//! The chain of runs of a pass: a run sees what the earlier ones were
//! accepted with, and an edit made over texts that are not there any more is a
//! conflict. And the files a change reaches.

use std::{collections::BTreeMap, fs, path::PathBuf};

use lighthouse_model::ucm::{
    Edge, EdgeKind, File, Fragment, FunctionSummary, Node, Project, Resolution, Signature, Symbol,
    Target, TypeRef,
};
use lighthouse_protocol::hash;

use super::{
    edit::{World, conflict},
    reach::reach,
    scratch::Scratch,
    tests::{FILE, run, symbol},
};
use crate::{Overlays, Run, engine::Scopes};

fn function(name: &str, file: &str, result: &str) -> (Symbol, FunctionSummary) {
    let mut symbol = symbol(name, (1, 1), 2);
    symbol.file = file.into();
    let summary = FunctionSummary {
        symbol: symbol.id.clone(),
        max_nesting: 0,
        statements: 0,
        top_level: 0,
        params: 0,
        returns: 1,
        tokens: 0,
        flow: Vec::new(),
        forwards_to: None,
        signature: Signature {
            params: Vec::new(),
            results: vec![TypeRef {
                text: result.to_owned(),
                symbol: None,
                exported: None,
            }],
        },
        events: Vec::new(),
        manual_assertions: 0,
        implementation: false,
        constructs: false,
    };
    (symbol, summary)
}

/// A project of `a.go`, which declares `f` returning `result`, and `b.go`,
/// whose `g` calls it, and `c.go`, whose `h` does not.
fn project(result: &str) -> Project {
    let (f, f_summary) = function("f", "a.go", result);
    let (g, g_summary) = function("g", "b.go", "int");
    let (h, h_summary) = function("h", "c.go", "int");
    let file = |path: &str| File {
        path: path.into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    };
    let calls = Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(g.id.clone()),
        to: Target::Resolved(Node::Symbol(f.id.clone())),
        resolution: Resolution::Semantic,
        site: None,
    };
    Project::merge([Fragment {
        files: vec![file("a.go"), file("b.go"), file("c.go")],
        symbols: vec![f, g, h],
        edges: vec![calls],
        functions: vec![f_summary, g_summary, h_summary],
        ..Fragment::default()
    }])
}

fn with_project(project: Project) -> Run {
    Run {
        project,
        ..Run::default()
    }
}

#[test]
fn a_changed_signature_reaches_the_files_that_call_it_and_no_others() {
    let before = with_project(project("int"));
    let changed = [PathBuf::from("a.go")];

    let same = reach(&changed, &before, &with_project(project("int")));
    let other = reach(&changed, &before, &with_project(project("string")));

    assert_eq!(same, [PathBuf::from("a.go")]);
    assert_eq!(other, [PathBuf::from("a.go"), PathBuf::from("b.go")]);
}

#[test]
fn a_symbol_that_is_gone_reaches_its_callers() {
    let before = with_project(project("int"));
    let after = with_project(Project::default());

    let files = reach(&[PathBuf::from("a.go")], &before, &after);

    assert_eq!(files, [PathBuf::from("a.go"), PathBuf::from("b.go")]);
}

#[test]
fn a_run_sees_the_edits_accepted_before_it_and_a_stale_result_is_a_conflict() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(FILE), "disk\n").unwrap();
    let outcome = run(Vec::new(), Vec::new());
    let scope = Scopes::default();
    let first = Overlays::from([(PathBuf::from(FILE), "first run\n".to_owned())]);

    let scratch = Scratch::new(root.path(), &first).unwrap();
    assert_eq!(
        fs::read_to_string(scratch.path().join(FILE)).unwrap(),
        "first run\n",
        "the copy of the second run holds what the first was accepted with"
    );

    let world = World::new(root.path(), &outcome, &first, &scope);
    let found = |text: &str| BTreeMap::from([(PathBuf::from(FILE), hash::sha256(text))]);
    assert_eq!(conflict(&found("first run\n"), &world), None);
    let stale = conflict(&found("disk\n"), &world).unwrap();
    assert!(
        stale.contains(FILE) && stale.contains("earlier edit"),
        "{stale}"
    );
}
