//! The chain of runs of a pass: a run sees what the earlier ones were
//! accepted with, and an edit made over texts that are not there any more is a
//! conflict. And the files a change reaches.

use std::{collections::BTreeMap, fs, path::PathBuf, sync::Mutex};

use lighthouse_model::{
    Fingerprint, Severity,
    ucm::{
        Edge, EdgeKind, File, Fragment, FunctionSummary, Node, Project, Resolution, Signature,
        Symbol, Target, TypeRef,
    },
};
use lighthouse_plugin::{Capabilities, EditRequest, Error, FixDecision, Model, ModelClient};
use lighthouse_protocol::hash;
use lighthouse_spec::{FixScope, Models};

use super::{
    Account, ModelFixer,
    edit::conflict,
    reach::reach,
    tests::{FILE, finding, proposal, run, symbol},
    verify::judge,
};
use crate::{
    Overlays, Run,
    cache::KeyBuilder,
    engine::{Member, Refusal, Route},
    fix::{
        propose::{Aim, Proposal},
        rewrite::{Scratch, World},
    },
};

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
    project_calling(result, true)
}

/// The same project, `g` calling `f` only when `calls`.
fn project_calling(result: &str, calls: bool) -> Project {
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
    let calls_f = Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(g.id.clone()),
        to: Target::Resolved(Node::Symbol(f.id.clone())),
        resolution: Resolution::Semantic,
        site: None,
    };
    Project::merge([Fragment {
        files: vec![file("a.go"), file("b.go"), file("c.go")],
        symbols: vec![f, g, h],
        edges: if calls { vec![calls_f] } else { Vec::new() },
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
fn a_change_reaches_the_files_that_use_its_symbols_and_those_they_use_before_or_after_it() {
    let calling = || with_project(project("int"));
    let alone = || with_project(project_calling("int", false));
    let (a, b, c) = (
        PathBuf::from("a.go"),
        PathBuf::from("b.go"),
        PathBuf::from("c.go"),
    );

    // `g` stops calling `f`, or starts to: the file of `f` is reached.
    assert_eq!(
        reach(std::slice::from_ref(&b), &calling(), &alone()),
        [a.clone(), b.clone()]
    );
    assert_eq!(
        reach(std::slice::from_ref(&b), &alone(), &calling()),
        [a.clone(), b.clone()]
    );
    // A change of `f` that keeps its signature reaches its callers.
    assert_eq!(
        reach(std::slice::from_ref(&a), &calling(), &calling()),
        [a.clone(), b.clone()]
    );
    assert_eq!(
        reach(
            std::slice::from_ref(&a),
            &calling(),
            &with_project(project("string"))
        ),
        [a.clone(), b.clone()]
    );
    // A file with no edge to the changed one is left out.
    assert_eq!(reach(std::slice::from_ref(&c), &calling(), &calling()), [c]);
}

/// `a.go` declares `f` and `b.go` refers to it at module level, as an
/// import or a call outside any function does; `without_f` leaves `f` out.
fn with_module_level_use(without_f: bool) -> Project {
    let (f, summary) = function("f", "a.go", "int");
    let file = |path: &str| File {
        path: path.into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    };
    let uses = Edge {
        kind: EdgeKind::Imports,
        from: Node::Module("m".to_owned()),
        to: Target::Resolved(Node::Symbol(f.id.clone())),
        resolution: Resolution::Semantic,
        site: Some(crate::fix::model::tests::span((1, 1), (1, 9))),
    };
    let (symbols, functions) = if without_f {
        (Vec::new(), Vec::new())
    } else {
        (vec![f], vec![summary])
    };
    Project::merge([
        Fragment {
            files: vec![file("a.go")],
            symbols,
            functions,
            ..Fragment::default()
        },
        Fragment {
            files: vec![file("b.go")],
            edges: vec![uses],
            ..Fragment::default()
        },
    ])
}

#[test]
fn a_change_reaches_the_files_that_refer_to_its_symbols_outside_any_function() {
    let before = with_project(with_module_level_use(false));
    let after = with_project(with_module_level_use(true));

    let files = reach(&[PathBuf::from("a.go")], &before, &after);

    assert_eq!(files, [PathBuf::from("a.go"), PathBuf::from("b.go")]);
}

#[test]
fn an_edit_that_removes_the_last_use_of_a_symbol_is_judged_by_what_the_symbol_gets() {
    let place = |rule: &str, file: &str| {
        let mut d = finding(rule, 3, Severity::Warn, "x");
        d.file = file.into();
        d.fingerprint = Fingerprint::new(rule, file, "x");
        d
    };
    let target = place("r", "b.go");
    let unused = place("design/unused", "a.go");
    let before = Run {
        project: project("int"),
        diagnostics: vec![target.clone()],
        ..Run::default()
    };
    let after = Run {
        project: project_calling("int", false),
        diagnostics: vec![unused],
        ..Run::default()
    };
    for aim in [Aim::Finding, Aim::File] {
        let edit = Proposal {
            aim,
            ..proposal(&target)
        };

        let verdict = judge(&[edit], &[], &before, &after);

        assert_eq!(
            verdict.files[&PathBuf::from("b.go")],
            "introduced design/unused at line 3",
            "{aim:?}"
        );
    }
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
    let first = Overlays::from([(PathBuf::from(FILE), "first run\n".to_owned())]);

    let scratch = Scratch::new(root.path(), &first).unwrap();
    assert_eq!(
        fs::read_to_string(scratch.path().join(FILE)).unwrap(),
        "first run\n",
        "the copy of the second run holds what the first was accepted with"
    );

    let world = World::new(root.path(), &outcome, &first);
    let found = |text: &str| BTreeMap::from([(PathBuf::from(FILE), hash::sha256(text))]);
    assert_eq!(conflict(&found("first run\n"), &world), None);
    let stale = conflict(&found("disk\n"), &world).unwrap();
    assert!(
        stale.contains(FILE) && stale.contains("earlier edit"),
        "{stale}"
    );
}

/// An agent that leaves `agent` in the file of the project.
struct Agent(Model);

impl ModelClient for Agent {
    fn manifest(&self) -> &Model {
        &self.0
    }

    fn edit(&self, request: &EditRequest<'_>) -> Result<String, Error> {
        fs::write(request.dir.join(FILE), "agent\n").map_err(|e| Error::Failed(e.to_string()))?;
        Ok(String::new())
    }
}

fn agent() -> Agent {
    Agent(Model {
        id: "agent/m".to_owned(),
        object: "model".to_owned(),
        created: 0,
        owned_by: "agent".to_owned(),
        capabilities: Capabilities::default(),
        version: "1".to_owned(),
        deterministic: false,
        abstains: false,
        cost: 0.0,
    })
}

fn indexed(hash: &str) -> Run {
    let file = File {
        path: FILE.into(),
        lang: "go".to_owned(),
        hash: hash.to_owned(),
        generated: false,
        test: false,
    };
    Run {
        project: Project::merge([Fragment {
            files: vec![file],
            ..Fragment::default()
        }]),
        ..Run::default()
    }
}

#[test]
fn an_answer_kept_over_texts_that_are_not_there_any_more_is_declined_as_a_conflict() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(FILE), "disk\n").unwrap();
    let outcome = indexed("h");
    let texts = Overlays::new();
    let fixes = BTreeMap::new();
    let mut bound = Models::default();
    bound.agent.push("agent/m".to_owned());
    let fixer = ModelFixer {
        route: Route::of(
            vec![Member {
                reference: "agent/m".to_owned(),
                client: Box::new(agent()),
            }],
            &bound,
        ),
        fixes: &fixes,
        account: Account::default(),
        said: Mutex::default(),
        context: 4000,
    };
    let finding = finding("r", 1, Severity::Warn, "a");
    let decision = FixDecision {
        id: "r".to_owned(),
        requirement: String::new(),
        context: String::new(),
    };
    let world = || World::new(root.path(), &outcome, &texts);
    let batch = fixer.batch(
        &world(),
        std::path::Path::new(FILE),
        &[(&finding, &decision, None)],
        Vec::new(),
    );

    let first = fixer.edit(&batch, &world()).unwrap();
    assert_eq!(first.texts[std::path::Path::new(FILE)], "agent\n");

    // The file changes while the digest of the files stays the one the answer
    // is kept under, as when an earlier edit of the pass was not kept.
    fs::write(root.path().join(FILE), "moved\n").unwrap();
    let reason = fixer.edit(&batch, &world()).err().expect("a conflict");

    assert!(reason.contains("not as the agent found it"), "{reason}");
    assert_eq!(fixer.scope_of("r"), FixScope::Files);
    let counts = &fixer.account.counts()["r"];
    assert_eq!(counts.declined["conflict"], 1, "{counts:?}");
    assert_eq!(counts.asked, 2);
}

#[test]
fn a_binding_that_names_no_model_of_the_route_is_not_a_spent_budget() {
    let mut bound = Models::default();
    bound.agent.push("agent/m".to_owned());
    let route = Route::of(
        vec![Member {
            reference: "agent/m".to_owned(),
            client: Box::new(agent()),
        }],
        &bound,
    );

    let refused = route
        .through::<String>(&[], &|_, _| KeyBuilder::new().finish(), &|_| {
            Ok("never asked".to_owned())
        })
        .unwrap_err();

    assert_eq!(refused, Refusal::Failed("no model is bound".to_owned()));
    assert_eq!(route.usage().calls, 0);
}
