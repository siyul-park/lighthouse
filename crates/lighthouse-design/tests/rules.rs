use std::{collections::BTreeMap, fs};

use lighthouse_config::Config;
use lighthouse_design::Design;
use lighthouse_engine::Engine;
use lighthouse_metrics::Metrics;
use lighthouse_model::{
    Capability, Edge, EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Node, Position,
    Resolution, Span, Symbol, SymbolId, SymbolKind, Target, Visibility,
};
use lighthouse_plugin::{
    Conventions, Error, LanguageProvider, Manifest, Plugin, Registry, Workspace,
};
use serde_json::{Value, json};

/// Reads a UCM fragment as JSON, the way an out-of-process provider would send it.
struct Wire {
    globs: Vec<String>,
}

impl LanguageProvider for Wire {
    fn id(&self) -> &str {
        "wire"
    }
    fn globs(&self) -> &[String] {
        &self.globs
    }
    fn conventions(&self) -> Conventions {
        Conventions {
            test_globs: vec!["**/*_test.ucm".to_owned()],
        }
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
    fn index(&self, _: &Workspace, _: &File, text: &str) -> Result<Fragment, Error> {
        serde_json::from_str(text).map_err(|e| Error::Failed(e.to_string()))
    }
}

struct Fixture;

impl Plugin for Fixture {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "fixture".to_owned(),
            version: "0".to_owned(),
        }
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Wire {
            globs: vec!["**/*.ucm".to_owned()],
        })]
    }
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry.register(&Fixture).unwrap();
    registry.register(&Metrics).unwrap();
    registry.register(&Design).unwrap();
    registry
}

#[derive(Default)]
struct World {
    files: BTreeMap<String, bool>,
    symbols: Vec<Symbol>,
    edges: Vec<Edge>,
    summaries: Vec<FunctionSummary>,
}

fn at(line: u32) -> Span {
    let p = Position { line, col: 1 };
    Span { start: p, end: p }
}

impl World {
    fn symbol(&mut self, module: &str, name: &str, kind: SymbolKind, file: &str) -> Symbol {
        self.files.entry(file.to_owned()).or_insert(false);
        let line = u32::try_from(self.symbols.len()).unwrap() + 1;
        let symbol = Symbol {
            id: SymbolId::new(module, &[], name, kind),
            kind,
            visibility: Visibility::Public,
            owner: None,
            file: file.into(),
            span: at(line),
            doc: None,
            name: name.to_owned(),
        };
        self.symbols.push(symbol.clone());
        symbol
    }

    fn func(&mut self, module: &str, name: &str, file: &str) -> Symbol {
        let symbol = self.symbol(module, name, SymbolKind::Function, file);
        self.summarize(&symbol, 1, &[]);
        symbol
    }

    fn private(&mut self, symbol: &Symbol) {
        self.symbols
            .iter_mut()
            .find(|s| s.id == symbol.id)
            .unwrap()
            .visibility = Visibility::Private;
    }

    fn summarize(&mut self, symbol: &Symbol, statements: u32, flow: &[Flow]) {
        self.summaries.retain(|s| s.symbol != symbol.id);
        self.summaries.push(FunctionSummary {
            symbol: symbol.id.clone(),
            max_nesting: flow.iter().map(|f| f.nesting + 1).max().unwrap_or(0),
            statements,
            top_level: 1,
            params: 0,
            returns: 0,
            tokens: 0,
            flow: flow.to_vec(),
            clone_fingerprint: None,
            forwards_to: None,
        });
    }

    fn edge(&mut self, kind: EdgeKind, from: &Symbol, to: &Symbol) {
        self.edges.push(Edge {
            kind,
            from: Node::Symbol(from.id.clone()),
            to: Target::Path(to.id.as_str().to_owned()),
            resolution: Resolution::Syntactic,
        });
    }

    fn forward(&mut self, from: &Symbol, to: &Symbol) {
        let summary = self
            .summaries
            .iter_mut()
            .find(|s| s.symbol == from.id)
            .unwrap();
        summary.forwards_to = Some(Target::Path(to.id.as_str().to_owned()));
    }

    fn check(&self, rule: &str, options: Value) -> Vec<(String, u32)> {
        let dir = tempfile::tempdir().unwrap();
        for (path, generated) in &self.files {
            let file = File {
                path: path.into(),
                lang: "wire".to_owned(),
                hash: String::new(),
                generated: *generated,
                test: path.ends_with("_test.ucm"),
            };
            let own = |s: &Symbol| s.file.to_string_lossy() == path.as_str();
            let ids: Vec<&SymbolId> = self
                .symbols
                .iter()
                .filter(|s| own(s))
                .map(|s| &s.id)
                .collect();
            let fragment = Fragment {
                files: vec![file],
                symbols: self.symbols.iter().filter(|s| own(s)).cloned().collect(),
                functions: self
                    .summaries
                    .iter()
                    .filter(|f| ids.contains(&&f.symbol))
                    .cloned()
                    .collect(),
                edges: self
                    .edges
                    .iter()
                    .filter(|e| matches!(&e.from, Node::Symbol(id) if ids.contains(&id)))
                    .cloned()
                    .collect(),
                ..Fragment::default()
            };
            let target = dir.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, serde_json::to_string(&fragment).unwrap()).unwrap();
        }
        let mut level = toml::Table::new();
        level.insert("level".to_owned(), "warn".into());
        for (key, value) in options.as_object().unwrap() {
            level.insert(key.clone(), toml::Value::try_from(value).unwrap());
        }
        let mut rules = toml::Table::new();
        rules.insert(rule.to_owned(), level.into());
        let mut root = toml::Table::new();
        root.insert(
            "plugins".to_owned(),
            vec!["fixture", "metrics", "design"].into(),
        );
        root.insert("rules".to_owned(), rules.into());
        let config = Config::parse(&root.to_string()).unwrap();
        let outcome = Engine::new(registry(), config, dir.path())
            .unwrap()
            .check(&[], &[rule.to_owned()])
            .unwrap();
        assert!(outcome.notices.is_empty(), "{:?}", outcome.notices);
        outcome
            .diagnostics
            .iter()
            .map(|d| (d.file.to_string_lossy().into_owned(), d.span.start.line))
            .collect()
    }

    fn names(&self, found: &[(String, u32)]) -> Vec<String> {
        found
            .iter()
            .map(|(file, line)| {
                self.symbols
                    .iter()
                    .find(|s| {
                        s.file.to_string_lossy() == file.as_str() && s.span.start.line == *line
                    })
                    .map(|s| s.name.clone())
                    .unwrap()
            })
            .collect()
    }
}

const DOC: &str = "design/exported-doc";

#[test]
fn exported_doc_flags_only_public_documented_less_symbols_of_public_owners() {
    let mut w = World::default();
    w.symbol("m", "Flagged", SymbolKind::Function, "m/a.ucm");
    let mut documented = w.symbol("m", "Documented", SymbolKind::Function, "m/a.ucm");
    documented.doc = Some("x".to_owned());
    w.symbols.last_mut().unwrap().doc = documented.doc;
    let hidden = w.symbol("m", "hidden", SymbolKind::Function, "m/a.ucm");
    w.private(&hidden);
    let internal = w.symbol("m", "Internal", SymbolKind::Function, "m/a.ucm");
    w.symbols
        .iter_mut()
        .find(|s| s.id == internal.id)
        .unwrap()
        .visibility = Visibility::Internal;
    let found = w.check(DOC, json!({}));
    assert_eq!(w.names(&found), ["Flagged"]);
}

#[test]
fn exported_doc_can_include_internal_symbols() {
    let mut w = World::default();
    let internal = w.symbol("m", "Internal", SymbolKind::Function, "m/a.ucm");
    w.symbols[0].visibility = Visibility::Internal;
    assert!(w.check(DOC, json!({})).is_empty());
    let found = w.check(DOC, json!({ "include_internal": true }));
    assert_eq!(w.names(&found), [internal.name]);
}

#[test]
fn exported_doc_skips_exempt_methods_interface_members_private_owners_and_kinds() {
    let mut w = World::default();
    let public = w.symbol("m", "T", SymbolKind::Type, "m/a.ucm");
    w.symbols[0].doc = Some("T is documented.".to_owned());
    let hidden = w.symbol("m", "hidden", SymbolKind::Type, "m/a.ucm");
    w.private(&hidden);
    let iface = w.symbol("m", "Reader", SymbolKind::Interface, "m/a.ucm");
    w.symbols[2].doc = Some("Reader reads.".to_owned());
    let member = |w: &mut World, owner: &Symbol, name: &str, kind: SymbolKind| {
        let mut s = w.symbol("m", name, kind, "m/a.ucm");
        s.owner = Some(owner.id.clone());
        w.symbols.last_mut().unwrap().owner = s.owner;
    };
    member(&mut w, &public, "String", SymbolKind::Method);
    member(&mut w, &public, "Field", SymbolKind::Field);
    member(&mut w, &hidden, "Run", SymbolKind::Method);
    member(&mut w, &iface, "Read", SymbolKind::Method);
    member(&mut w, &public, "Flagged", SymbolKind::Method);
    let found = w.check(DOC, json!({}));
    assert_eq!(w.names(&found), ["Flagged"]);
    let strict = w.check(
        DOC,
        json!({ "exempt_methods": [], "kinds": ["method", "field"] }),
    );
    assert_eq!(w.names(&strict), ["String", "Field", "Flagged"]);
}

#[test]
fn exported_doc_skips_generated_and_test_files() {
    let mut w = World::default();
    w.symbol("m", "Gen", SymbolKind::Function, "m/gen.ucm");
    w.symbol("m", "InTest", SymbolKind::Function, "m/a_test.ucm");
    w.files.insert("m/gen.ucm".to_owned(), true);
    assert!(w.check(DOC, json!({})).is_empty());
}

const WRAP: &str = "design/single-use-wrapper";

/// `Get` calls the wrapper `load`, which only forwards to `read`.
fn wrapped() -> (World, Symbol, Symbol, Symbol) {
    let mut w = World::default();
    let get = w.func("m", "Get", "m/a.ucm");
    let load = w.func("m", "load", "m/a.ucm");
    let read = w.func("m", "read", "m/a.ucm");
    w.private(&load);
    w.private(&read);
    w.edge(EdgeKind::Calls, &get, &load);
    w.edge(EdgeKind::Calls, &load, &read);
    w.forward(&load, &read);
    (w, get, load, read)
}

#[test]
fn single_use_wrapper_flags_a_private_forwarder_with_one_caller() {
    let (w, ..) = wrapped();
    let found = w.check(WRAP, json!({}));
    assert_eq!(w.names(&found), ["load"]);
}

#[test]
fn single_use_wrapper_ignores_value_uses_and_second_callers() {
    let (mut w, get, load, _) = wrapped();
    w.edge(EdgeKind::References, &get, &load);
    assert!(w.check(WRAP, json!({})).is_empty(), "function value use");

    let (mut w, _, load, _) = wrapped();
    let test = w.func("m", "TestLoad", "m/a_test.ucm");
    w.edge(EdgeKind::Calls, &test, &load);
    assert!(w.check(WRAP, json!({})).is_empty(), "called from a test");
}

#[test]
fn single_use_wrapper_ignores_interface_methods_docs_and_shared_targets() {
    let (mut w, _, _, read) = wrapped();
    let shared = w.func("m", "other", "m/a.ucm");
    w.edge(EdgeKind::Calls, &shared, &read);
    assert!(
        w.check(WRAP, json!({})).is_empty(),
        "target has two callers"
    );

    let (mut w, _, load, _) = wrapped();
    w.symbols.iter_mut().find(|s| s.id == load.id).unwrap().doc = Some("load explains.".to_owned());
    assert!(w.check(WRAP, json!({})).is_empty(), "documented");

    let (mut w, _, load, read) = wrapped();
    for symbol in w
        .symbols
        .iter_mut()
        .filter(|s| s.id == load.id || s.id == read.id)
    {
        symbol.kind = SymbolKind::Method;
    }
    let iface = w.symbol("m", "loader", SymbolKind::Interface, "m/a.ucm");
    let mut member = w.symbol("m", "load", SymbolKind::Method, "m/a.ucm");
    member.owner = Some(iface.id);
    w.symbols.last_mut().unwrap().owner = member.owner;
    assert!(
        w.check(WRAP, json!({})).is_empty(),
        "named like an interface method"
    );
}

#[test]
fn single_use_wrapper_ignores_cycles_and_public_wrappers() {
    let (mut w, _, load, read) = wrapped();
    w.edge(EdgeKind::Calls, &read, &load);
    assert!(w.check(WRAP, json!({})).is_empty(), "target leads back");

    let (mut w, _, load, _) = wrapped();
    w.symbols
        .iter_mut()
        .find(|s| s.id == load.id)
        .unwrap()
        .visibility = Visibility::Public;
    assert!(w.check(WRAP, json!({})).is_empty(), "public wrapper");
}

const COMPLEX: &str = "design/complexity-signal";

fn branches(n: usize) -> Vec<Flow> {
    vec![Flow::new(FlowKind::If, 0); n]
}

#[test]
fn complexity_needs_both_cyclomatic_and_statements() {
    let mut w = World::default();
    let big = w.func("m", "big", "m/a.ucm");
    w.summarize(&big, 30, &branches(14));
    let short = w.func("m", "short", "m/a.ucm");
    w.summarize(&short, 29, &branches(14));
    let flat = w.func("m", "flat", "m/a.ucm");
    w.summarize(&flat, 80, &branches(13));
    let found = w.check(COMPLEX, json!({}));
    assert_eq!(w.names(&found), ["big"]);
}

#[test]
fn complexity_reports_nesting_and_cognitive_signals_with_their_own_thresholds() {
    let mut w = World::default();
    let deep = w.func("m", "deep", "m/a.ucm");
    let mut flow = branches(9);
    flow.push(Flow::new(FlowKind::If, 4));
    w.summarize(&deep, 25, &flow);
    let tangled = w.func("m", "tangled", "m/a.ucm");
    let nested: Vec<Flow> = (0..5).map(|n| Flow::new(FlowKind::Loop, n)).collect();
    w.summarize(&tangled, 30, &nested);
    let found = w.check(COMPLEX, json!({ "cognitive": 15 }));
    assert_eq!(w.names(&found), ["deep", "tangled"]);
    let quiet = w.check(COMPLEX, json!({}));
    assert_eq!(w.names(&quiet), ["deep"]);
}

#[test]
fn complexity_skips_dispatchers_and_tests() {
    let mut w = World::default();
    let table = w.func("m", "table", "m/a.ucm");
    let mut flow = vec![Flow {
        arms: 40,
        returning: true,
        ..Flow::new(FlowKind::Switch, 0)
    }];
    flow.extend(branches(0));
    w.summarize(&table, 90, &flow);
    let test = w.func("m", "TestBig", "m/a_test.ucm");
    w.summarize(&test, 99, &branches(30));
    assert!(w.check(COMPLEX, json!({})).is_empty());
}

const COUPLE: &str = "design/coupling-signal";

fn hub_options() -> Value {
    json!({ "hub_fan_in": 2, "hub_fan_out": 2, "hub_statements": 1 })
}

fn hub_world() -> (World, Symbol) {
    let mut w = World::default();
    let hub = w.func("m", "hub", "m/a.ucm");
    let left = w.func("m", "left", "m/a.ucm");
    let right = w.func("m", "right", "m/a.ucm");
    let a = w.func("m", "a", "m/a.ucm");
    let b = w.func("m", "b", "m/a.ucm");
    w.edge(EdgeKind::Calls, &hub, &left);
    w.edge(EdgeKind::Calls, &hub, &right);
    w.edge(EdgeKind::Calls, &a, &hub);
    w.edge(EdgeKind::Calls, &b, &hub);
    (w, hub)
}

#[test]
fn coupling_flags_an_intra_package_hub() {
    let (w, _) = hub_world();
    let found = w.check(COUPLE, hub_options());
    assert_eq!(w.names(&found), ["hub"]);
    assert!(w.check(COUPLE, json!({})).is_empty(), "defaults are high");
}

#[test]
fn coupling_counts_neither_other_packages_nor_tests() {
    let (mut w, hub) = hub_world();
    for name in ["a", "b"] {
        w.edges
            .retain(|e| e.to != Target::Path(hub.id.as_str().to_owned()) || !matches!(&e.from, Node::Symbol(id) if id.as_str().contains(&format!("::{name}#"))));
    }
    let far1 = w.func("other", "far1", "other/a.ucm");
    let far2 = w.func("other", "far2", "other/a.ucm");
    let test1 = w.func("m", "TestA", "m/a_test.ucm");
    let test2 = w.func("m", "TestB", "m/a_test.ucm");
    for caller in [&far1, &far2, &test1, &test2] {
        w.edge(EdgeKind::Calls, caller, &hub);
    }
    assert!(w.check(COUPLE, hub_options()).is_empty());
}

#[test]
fn coupling_flags_a_coordinator_with_many_callees() {
    let mut w = World::default();
    let run = w.func("m", "run", "m/a.ucm");
    for name in ["one", "two", "three"] {
        let step = w.func("m", name, "m/a.ucm");
        w.edge(EdgeKind::Calls, &run, &step);
    }
    let options = json!({
        "hub_fan_in": 100,
        "coordinator_fan_out": 3,
        "coordinator_max_fan_in": 0,
        "coordinator_statements": 1,
    });
    let found = w.check(COUPLE, options);
    assert_eq!(w.names(&found), ["run"]);
}
