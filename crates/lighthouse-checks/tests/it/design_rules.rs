use crate::support::World;
use lighthouse_model::{
    EdgeKind, Flow, FlowKind, Node, Symbol, SymbolId, SymbolKind, Target, Visibility,
};
use serde_json::{Value, json};

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
    let found = w.check(DOC, json!({ "includeInternal": true }));
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
        json!({ "exemptMethods": [], "kinds": ["method", "field"] }),
    );
    assert_eq!(w.names(&strict), ["String", "Field", "Flagged"]);
}

#[test]
fn exported_doc_can_leave_interface_implementations_to_the_interface() {
    let mut w = World::default();
    let ty = w.symbol("m", "T", SymbolKind::Type, "m/a.ucm");
    w.symbols[0].doc = Some("T is documented.".to_owned());
    let iface = w.symbol("m", "Reader", SymbolKind::Interface, "m/a.ucm");
    w.symbols[1].doc = Some("Reader reads.".to_owned());
    let mut member = |owner: &Symbol, name: &str| {
        w.symbol("m", name, SymbolKind::Method, "m/a.ucm");
        let added = w.symbols.last_mut().unwrap();
        added.id = SymbolId::new("m", &[owner.name.as_str()], name, SymbolKind::Method);
        added.owner = Some(owner.id.clone());
    };
    member(&ty, "Read");
    member(&ty, "Other");
    member(&iface, "Read");
    w.edge(EdgeKind::Implements, &ty, &iface);
    let by_default = w.check(DOC, json!({}));
    assert_eq!(w.names(&by_default), ["Read", "Other"]);
    // The interface's own method must carry the documentation.
    let exempt = w.check(DOC, json!({ "exemptInterfaceMethods": true }));
    assert_eq!(w.names(&exempt), ["Read", "Other"]);
    w.symbols.last_mut().unwrap().doc = Some("Read reads.".to_owned());
    let exempt = w.check(DOC, json!({ "exemptInterfaceMethods": true }));
    assert_eq!(w.names(&exempt), ["Other"]);
}

#[test]
fn exported_doc_skips_generated_and_test_files() {
    let mut w = World::default();
    w.symbol("m", "Gen", SymbolKind::Function, "m/gen.ucm");
    w.symbol("m", "InTest", SymbolKind::Function, "m/a_test.ucm");
    w.files.insert("m/gen.ucm".to_owned(), true);
    assert!(w.check(DOC, json!({})).is_empty());
}

const WRAP: &str = "design/no-single-use-wrapper";

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

const COMPLEX: &str = "design/complexity";

fn branches(n: usize) -> Vec<Flow> {
    vec![Flow::new(FlowKind::If, 0); n]
}

const COGNITIVE: &str = "design/cognitive-complexity";
const STATEMENTS: &str = "design/max-statements";
const DEPTH: &str = "design/max-depth";
const PARAMS: &str = "design/max-params";
const RESULTS: &str = "design/max-results";

#[test]
fn complexity_is_one_independent_limit() {
    let mut w = World::default();
    let wide = w.func("m", "wide", "m/a.ucm");
    w.summarize(&wide, 1, &branches(14));
    let narrow = w.func("m", "narrow", "m/a.ucm");
    w.summarize(&narrow, 99, &branches(13));
    let found = w.check(COMPLEX, json!({ "max": 14 }));
    assert_eq!(w.names(&found), ["wide"]);
}

#[test]
fn cognitive_complexity_and_depth_have_their_own_limits() {
    let mut w = World::default();
    let tangled = w.func("m", "tangled", "m/a.ucm");
    let nested: Vec<Flow> = (0..5).map(|n| Flow::new(FlowKind::Loop, n)).collect();
    w.summarize(&tangled, 5, &nested);
    assert_eq!(
        w.names(&w.check(COGNITIVE, json!({ "max": 14 }))),
        ["tangled"]
    );
    assert!(w.check(COGNITIVE, json!({ "max": 15 })).is_empty());
    assert_eq!(w.names(&w.check(DEPTH, json!({ "max": 4 }))), ["tangled"]);
    assert!(w.check(DEPTH, json!({ "max": 5 })).is_empty());
}

#[test]
fn statements_are_limited_per_role() {
    let mut w = World::default();
    let long = w.func("m", "long", "m/a.ucm");
    w.summarize(&long, 12, &[]);
    assert_eq!(
        w.names(&w.check(STATEMENTS, json!({ "max": 10 }))),
        ["long"]
    );
    let by_role = json!({ "max": { "default": 10, "function": 20 } });
    assert!(w.check(STATEMENTS, by_role).is_empty());
    let no_default = json!({ "max": { "default": -1, "method": 1 } });
    assert!(
        w.check(STATEMENTS, no_default).is_empty(),
        "no limit for a function"
    );
    assert!(
        w.check(STATEMENTS, json!({ "max": -1 })).is_empty(),
        "-1 is no limit"
    );
}

#[test]
fn an_object_limit_keeps_the_roles_of_the_default() {
    let mut w = World::default();
    let plain = w.func("m", "build", "m/a.ucm");
    params(&mut w, &plain, 9);
    let ctor = w.func("m", "NewServer", "m/a.ucm");
    params(&mut w, &ctor, 10);
    let found = w.check(PARAMS, json!({ "max": { "constructor": 12 } }));
    assert_eq!(w.names(&found), ["build"], "the default of 8 still applies");
    let only_default = w.check(PARAMS, json!({ "max": { "default": 20 } }));
    assert_eq!(
        w.names(&only_default),
        ["NewServer"],
        "the constructor keeps its 9"
    );
}

fn flag(
    w: &mut World,
    symbol: &lighthouse_model::Symbol,
    set: impl Fn(&mut lighthouse_model::FunctionSummary),
) {
    set(w
        .summaries
        .iter_mut()
        .find(|s| s.symbol == symbol.id)
        .unwrap());
}

#[test]
fn a_role_is_the_first_flag_that_fits() {
    let mut w = World::default();
    let plain = w.func("m", "plain", "m/a.ucm");
    let owner = w.symbol("m", "Server", SymbolKind::Type, "m/a.ucm");
    let method = w.member(&owner, "serve", SymbolKind::Method, "m/a.ucm");
    let ctor = w.func("m", "NewThing", "m/a.ucm");
    let built = w.func("m", "make", "m/a.ucm");
    let both = w.func("m", "NewBoth", "m/a.ucm");
    for symbol in [&plain, &method, &ctor, &built, &both] {
        w.summarize(symbol, 10, &[]);
    }
    flag(&mut w, &built, |s| s.constructs = true);
    flag(&mut w, &both, |s| {
        s.constructs = true;
        s.implementation = true;
    });
    // One limit per role, each below the 10 statements: every role is told
    // apart by the limit it gets.
    let limits = |role: &str| {
        let mut all = serde_json::Map::new();
        all.insert("default".to_owned(), json!(100));
        all.insert(role.to_owned(), json!(5));
        json!({ "max": all })
    };
    let flagged = |w: &World, role: &str| w.names(&w.check(STATEMENTS, limits(role)));
    assert_eq!(flagged(&w, "function"), ["plain"]);
    assert_eq!(flagged(&w, "method"), ["serve"]);
    assert_eq!(
        flagged(&w, "constructor"),
        ["NewThing", "make"],
        "by name or by flag"
    );
    assert_eq!(
        flagged(&w, "implementation"),
        ["NewBoth"],
        "implementation beats constructor"
    );
}

#[test]
fn complexity_skips_dispatchers_unless_asked() {
    let mut w = World::default();
    let table = w.func("m", "table", "m/a.ucm");
    let flow = vec![Flow {
        arms: 40,
        returning: true,
        ..Flow::new(FlowKind::Switch, 0)
    }];
    w.summarize(&table, 90, &flow);
    assert!(w.check(COMPLEX, json!({ "max": 5 })).is_empty());
    let counted = json!({ "max": 5, "ignoreDispatch": false });
    assert_eq!(w.names(&w.check(COMPLEX, counted)), ["table"]);
}

fn params(w: &mut World, symbol: &lighthouse_model::Symbol, n: u32) {
    w.summaries
        .iter_mut()
        .find(|s| s.symbol == symbol.id)
        .unwrap()
        .params = n;
}

#[test]
fn a_constructor_may_take_one_more_parameter_and_an_implementation_any() {
    let mut w = World::default();
    let plain = w.func("m", "build", "m/a.ucm");
    params(&mut w, &plain, 9);
    let ctor = w.func("m", "NewServer", "m/a.ucm");
    params(&mut w, &ctor, 10);
    let fine = w.func("m", "NewClient", "m/a.ucm");
    params(&mut w, &fine, 9);
    let object = w.symbol("m", "Object", SymbolKind::Type, "m/a.ucm");
    let hook = w.member(&object, "fmt", SymbolKind::Method, "m/a.ucm");
    params(&mut w, &hook, 9);
    w.summaries
        .iter_mut()
        .find(|s| s.symbol == hook.id)
        .unwrap()
        .implementation = true;
    let found = w.check(PARAMS, json!({}));
    assert_eq!(w.names(&found), ["build", "NewServer"]);
}

#[test]
fn results_are_counted_except_for_a_constructor() {
    let mut w = World::default();
    let many = w.func("m", "split", "m/a.ucm");
    let ctor = w.func("m", "NewParser", "m/a.ucm");
    for symbol in [&many, &ctor] {
        w.summaries
            .iter_mut()
            .find(|s| s.symbol == symbol.id)
            .unwrap()
            .returns = 4;
    }
    assert_eq!(w.names(&w.check(RESULTS, json!({}))), ["split"]);
}

#[test]
fn a_limit_refuses_what_is_not_a_limit() {
    let w = World::default();
    for bad in [
        json!(-2),
        json!("3"),
        json!(null),
        json!({ "closure": 3 }),
        json!({ "default": -2 }),
    ] {
        let result = std::panic::catch_unwind(|| w.check(PARAMS, json!({ "max": bad })));
        assert!(result.is_err(), "{bad}");
    }
}

const COUPLE: &str = "design/coupling";

fn hub_options() -> Value {
    json!({ "hubFanIn": 2, "hubFanOut": 2, "hubStatements": 1 })
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
        "hubFanIn": 100,
        "coordinatorFanOut": 3,
        "coordinatorMaxFanIn": 0,
        "coordinatorStatements": 1,
    });
    let found = w.check(COUPLE, options);
    assert_eq!(w.names(&found), ["run"]);
}

fn type_ref(symbol: &str) -> lighthouse_model::TypeRef {
    lighthouse_model::TypeRef {
        text: symbol.to_owned(),
        symbol: Some(symbol.to_owned()),
        exported: None,
    }
}

fn takes(w: &mut World, function: &lighthouse_model::Symbol, ty: &str, n: u32) {
    let summary = w
        .summaries
        .iter_mut()
        .find(|s| s.symbol == function.id)
        .unwrap();
    summary.params = n;
    summary.signature.params = vec![type_ref(ty)];
}

#[test]
fn a_struct_used_only_by_one_function_counts_its_fields() {
    let mut w = World::default();
    let request = w.symbol("m", "Request", SymbolKind::Type, "m/a.ucm");
    for field in ["a", "b", "c"] {
        w.member(&request, field, SymbolKind::Field, "m/a.ucm");
    }
    let build = w.func("m", "build", "m/a.ucm");
    takes(&mut w, &build, "m::Request", 7);
    assert_eq!(
        w.names(&w.check(PARAMS, json!({}))),
        ["build"],
        "6 + 3 fields"
    );

    let other = w.func("m", "other", "m/a.ucm");
    takes(&mut w, &other, "m::Request", 1);
    assert!(
        w.check(PARAMS, json!({})).is_empty(),
        "shared by two functions"
    );
}

#[test]
fn an_options_struct_counts_only_its_required_fields() {
    let mut w = World::default();
    let options = w.symbol("m", "BuildOptions", SymbolKind::Type, "m/a.ucm");
    for field in ["a", "b", "c"] {
        w.member(&options, field, SymbolKind::Field, "m/a.ucm");
        w.symbols.last_mut().unwrap().optional = true;
    }
    let build = w.func("m", "build", "m/a.ucm");
    takes(&mut w, &build, "m::BuildOptions", 7);
    assert!(
        w.check(PARAMS, json!({})).is_empty(),
        "6 + 0 required fields"
    );
    let strict = json!({ "allowSuffixes": [] });
    assert_eq!(w.names(&w.check(PARAMS, strict)), ["build"], "6 + 3 fields");
}

/// A `build` of 7 parameters whose last one is a 3-field `Request` that only
/// it takes: 6 + 3 = 9 parameters, over the limit of 8.
fn requested() -> (World, lighthouse_model::Symbol, lighthouse_model::Symbol) {
    let mut w = World::default();
    let request = w.symbol("m", "Request", SymbolKind::Type, "m/a.ucm");
    for field in ["a", "b", "c"] {
        w.member(&request, field, SymbolKind::Field, "m/a.ucm");
    }
    let build = w.func("m", "build", "m/a.ucm");
    takes(&mut w, &build, "m::Request", 7);
    (w, request, build)
}

#[test]
fn a_reference_from_a_caller_or_a_test_keeps_the_struct_single_use() {
    let (mut w, request, build) = requested();
    let caller = w.func("m", "main_loop", "m/a.ucm");
    w.edge(EdgeKind::Calls, &caller, &build);
    w.edge(EdgeKind::References, &caller, &request);
    let test = w.func("m", "TestRequest", "m/a_test.ucm");
    w.edge(EdgeKind::References, &test, &request);
    assert_eq!(w.names(&w.check(PARAMS, json!({}))), ["build"]);
}

#[test]
fn a_reference_from_anywhere_else_makes_the_struct_shared() {
    let (mut w, request, _) = requested();
    let other = w.func("m", "elsewhere", "m/a.ucm");
    w.edge(EdgeKind::References, &other, &request);
    assert!(w.check(PARAMS, json!({})).is_empty());

    let (mut w, request, _) = requested();
    let field = w.symbols.iter().find(|s| s.name == "b").unwrap().clone();
    let other = w.func("m", "elsewhere", "m/a.ucm");
    w.edge(EdgeKind::References, &other, &field);
    assert!(w.check(PARAMS, json!({})).is_empty(), "a field counts too");
    let _ = request;
}

#[test]
fn a_struct_counts_once_per_parameter_that_takes_it() {
    let (mut w, _, build) = requested();
    let summary = w
        .summaries
        .iter_mut()
        .find(|s| s.symbol == build.id)
        .unwrap();
    summary.params = 5;
    summary.param_types = vec!["m::Request".to_owned(), "m::Request".to_owned()];
    assert_eq!(
        w.names(&w.check(PARAMS, json!({}))),
        ["build"],
        "3 + 2 * 3 = 9"
    );
}
