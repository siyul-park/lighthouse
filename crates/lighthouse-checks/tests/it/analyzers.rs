use std::collections::BTreeMap;

use lighthouse_checks::metrics::{MEASURES, Measures, Metrics};
use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Node, Project, Resolution,
        Symbol, SymbolId, SymbolKind, Target, Visibility,
    },
};
use lighthouse_plugin::{Context, Facts, Plugin, Workspace};

const FILE: &str = "m/m.go";

fn symbol(module: &str, name: &str, kind: SymbolKind, file: &str, lines: (u32, u32)) -> Symbol {
    let at = |line| Position { line, col: 1 };
    Symbol {
        id: SymbolId::new(module, &[], name, kind),
        kind,
        visibility: Visibility::Private,
        owner: None,
        file: file.into(),
        span: Span {
            start: at(lines.0),
            end: at(lines.1),
        },
        extent: None,
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn summary(symbol: &Symbol, nesting: u32, statements: u32, flow: &[Flow]) -> FunctionSummary {
    FunctionSummary {
        symbol: symbol.id.clone(),
        max_nesting: nesting,
        statements,
        top_level: 1,
        params: 0,
        returns: 0,
        tokens: 0,
        flow: flow.to_vec(),
        forwards_to: None,
        signature: Default::default(),
        events: Vec::new(),
        manual_assertions: 0,
        implementation: false,
        constructs: false,
    }
}

fn file(path: &str, test: bool) -> File {
    File {
        path: path.into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test,
    }
}

fn call(from: &Symbol, to: &Symbol) -> Edge {
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from.id.clone()),
        to: Target::Path(to.id.as_str().to_owned()),
        resolution: Resolution::Syntactic,
        site: None,
    }
}

fn run(project: &Project, path: &str) -> BTreeMap<SymbolId, Measures> {
    let metrics = Metrics.analyzers();
    let analyzer = metrics
        .iter()
        .find(|a| a.manifest().id == MEASURES)
        .unwrap();
    let ws = Workspace::new(".");
    let file = file(path, false);
    let facts = Facts::new();
    let ctx = Context {
        workspace: &ws,
        project,
        file: Some((&file, "")),
        facts: &facts,
        keys: &lighthouse_plugin::NoKeys,
        trusted: false,
        memo: &lighthouse_plugin::Memo::default(),
        notices: &lighthouse_plugin::Notices::default(),
        applies: lighthouse_model::ucm::Applicability::default(),
        models: &lighthouse_plugin::NoModels,
    };
    serde_json::from_value(analyzer.run(&ctx).unwrap()).unwrap()
}

fn with_summary(f: Symbol, summary: FunctionSummary) -> Project {
    Project::merge([Fragment {
        files: vec![file(FILE, false)],
        functions: vec![summary],
        symbols: vec![f],
        ..Fragment::default()
    }])
}

fn one(flow: &[Flow]) -> Project {
    let f = symbol("m", "f", SymbolKind::Function, FILE, (1, 1));
    let summary = summary(&f, 0, 1, flow);
    with_summary(f, summary)
}

fn at(kind: FlowKind, nesting: u32) -> Flow {
    Flow::new(kind, nesting)
}

fn measures(project: &Project) -> Measures {
    run(project, FILE).into_values().next().unwrap()
}

fn cognitive(flow: &[Flow]) -> u32 {
    measures(&one(flow)).cognitive
}

#[test]
fn cyclomatic_counts_one_path_plus_every_decision_point() {
    use FlowKind::*;
    let switch = Flow {
        arms: 3,
        ..Flow::new(Switch, 0)
    };
    let logic = Flow {
        operators: 2,
        ..Flow::new(Logic, 0)
    };
    let cyclomatic = |flow: &[Flow]| measures(&one(flow)).cyclomatic;
    assert_eq!(cyclomatic(&[]), 1);
    assert_eq!(cyclomatic(&[at(If, 0)]), 2);
    let all = [
        at(If, 0),
        at(ElseIf, 0),
        at(Else, 0),
        at(Loop, 0),
        at(Catch, 0),
        at(Jump, 0),
        at(Recursion, 0),
        switch,
        logic,
    ];
    assert_eq!(cyclomatic(&all), 1 + 4 + 3 + 2);
}

#[test]
fn dispatchers_are_single_switches_whose_arms_only_return() {
    use FlowKind::*;
    let table = Flow {
        arms: 2,
        returning: true,
        ..Flow::new(Switch, 0)
    };
    let with = |top_level: u32, flow: &[Flow]| {
        let f = symbol("m", "f", SymbolKind::Function, FILE, (1, 1));
        let mut s = summary(&f, 0, 1, flow);
        s.top_level = top_level;
        measures(&with_summary(f, s)).dispatcher
    };
    assert!(with(1, &[table, at(Logic, 1)]));
    assert!(!with(2, &[table]));
    assert!(!with(
        1,
        &[Flow {
            returning: false,
            ..table
        }]
    ));
    assert!(!with(1, &[at(If, 0), table]));
    assert!(!with(
        1,
        &[Flow {
            nesting: 1,
            ..table
        }]
    ));
    assert!(!with(1, &[]));
}

#[test]
fn flat_dispatch_is_one_top_level_switch_nested_at_most_two_levels() {
    use FlowKind::*;
    let flat = |nesting: u32, flow: &[Flow]| {
        let f = symbol("m", "f", SymbolKind::Function, FILE, (1, 1));
        let s = summary(&f, nesting, 1, flow);
        measures(&with_summary(f, s)).flat_dispatch
    };
    let arms = [Flow::new(Switch, 0), at(If, 1), at(Logic, 1), at(Loop, 2)];
    assert!(flat(2, &arms));
    assert!(!flat(3, &arms));
    assert!(!flat(0, &[at(If, 0), at(Switch, 0)]));
    assert!(!flat(0, &[at(Switch, 0), at(Loop, 0)]));
    assert!(!flat(0, &[at(Switch, 0), at(Switch, 0)]));
    assert!(!flat(0, &[at(If, 1)]));
}

#[test]
fn cognitive_matches_the_sonarsource_specification_examples() {
    use FlowKind::*;
    let sum_of_primes = [at(Loop, 0), at(Loop, 1), at(If, 2), at(Jump, 3)];
    assert_eq!(cognitive(&sum_of_primes), 7);
    let try_catch = [at(If, 0), at(Loop, 1), at(Loop, 2), at(Catch, 0), at(If, 1)];
    assert_eq!(cognitive(&try_catch), 9);
    assert_eq!(cognitive(&[at(Switch, 0)]), 1);
    assert_eq!(cognitive(&[at(If, 1)]), 2, "lambda adds nesting only");
    assert_eq!(cognitive(&[at(If, 0), at(ElseIf, 0), at(Else, 0)]), 3);
    assert_eq!(cognitive(&[at(If, 0), at(If, 1), at(If, 2)]), 6);
    assert_eq!(cognitive(&[at(If, 0), at(Recursion, 0)]), 2);
    assert_eq!(
        cognitive(&[at(If, 0), at(Logic, 0)]),
        2,
        "a && b && c is one run"
    );
    assert_eq!(
        cognitive(&[at(If, 0), at(Logic, 0), at(Logic, 0)]),
        3,
        "a && b || c starts a second run"
    );
    assert_eq!(cognitive(&[]), 0);
}

#[test]
fn size_reports_statements_and_lines_and_nesting_the_deepest_level() {
    let f = symbol("m", "f", SymbolKind::Function, FILE, (10, 24));
    let project = Project::merge([Fragment {
        files: vec![file(FILE, false)],
        functions: vec![summary(&f, 3, 12, &[])],
        symbols: vec![f],
        ..Fragment::default()
    }]);
    let measured = measures(&project);
    assert_eq!((measured.statements, measured.lines), (12, 15));
    assert_eq!(measured.nesting, 3);
}

#[test]
fn metrics_cover_only_functions_of_the_focused_file() {
    let a = symbol("m", "a", SymbolKind::Function, FILE, (1, 1));
    let b = symbol("m", "b", SymbolKind::Function, "m/other.go", (1, 1));
    let t = symbol("m", "T", SymbolKind::Type, FILE, (3, 3));
    let project = Project::merge([Fragment {
        files: vec![file(FILE, false), file("m/other.go", false)],
        functions: vec![summary(&a, 0, 1, &[]), summary(&b, 0, 1, &[])],
        symbols: vec![a.clone(), b, t],
        ..Fragment::default()
    }]);
    let found = run(&project, FILE);
    assert_eq!(found.keys().collect::<Vec<_>>(), [&a.id]);
}

#[test]
fn fan_counts_same_module_functions_and_ignores_tests_types_and_self() {
    let hub = symbol("m", "hub", SymbolKind::Function, FILE, (1, 1));
    let up1 = symbol("m", "up1", SymbolKind::Function, "m/a.go", (1, 1));
    let up2 = symbol("m", "up2", SymbolKind::Function, "m/a.go", (2, 2));
    let in_test = symbol("m", "TestHub", SymbolKind::Test, "m/a_test.go", (1, 1));
    let down1 = symbol("m", "down1", SymbolKind::Function, "m/b.go", (1, 1));
    let down2 = symbol("m", "down2", SymbolKind::Method, "m/b.go", (2, 2));
    let ty = symbol("m", "T", SymbolKind::Type, "m/b.go", (3, 3));
    let abroad = symbol("n", "far", SymbolKind::Function, "n/n.go", (1, 1));
    let all = [&hub, &up1, &up2, &in_test, &down1, &down2, &ty, &abroad];
    let project = Project::merge([Fragment {
        files: vec![
            file(FILE, false),
            file("m/a.go", false),
            file("m/a_test.go", true),
            file("m/b.go", false),
            file("n/n.go", false),
        ],
        edges: vec![
            call(&up1, &hub),
            call(&up2, &hub),
            call(&in_test, &hub),
            call(&hub, &hub),
            call(&hub, &down1),
            call(&hub, &down2),
            call(&hub, &ty),
            call(&hub, &abroad),
        ],
        functions: vec![summary(&hub, 0, 1, &[])],
        symbols: all.iter().map(|s| (*s).clone()).collect(),
        ..Fragment::default()
    }]);
    let measured = measures(&project);
    assert_eq!((measured.fan_in, measured.fan_out), (2, 2));
}
