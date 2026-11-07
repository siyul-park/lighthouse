//! The only place where wire types become code model (UCM) types.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Incomplete, Position, Span, ucm as core};
use lighthouse_plugin::{Fragments, Indexed, LanguageManifest, Source, Unit};
use lighthouse_protocol as wire;

/// `fn $name` converting a wire enum to the core enum of the same variants.
macro_rules! variants {
    ($($name:ident: $kind:ident { $($variant:ident),+ $(,)? })+) => {$(
        fn $name(kind: wire::$kind) -> core::$kind {
            match kind {
                $(wire::$kind::$variant => core::$kind::$variant,)+
            }
        }
    )+};
}

variants! {
    symbol_kind: SymbolKind {
        Function, Method, Type, Field, Variant, Const, Var, Interface, Test,
    }
    visibility: Visibility { Public, Private, Internal }
    role: SymbolRole { TestHelper, Fixture }
    edge_kind: EdgeKind { Calls, References, Imports, Contains, Implements, AccessesPrivate }
    resolution: Resolution { Semantic, Syntactic, Heuristic }
    flow_kind: FlowKind { If, ElseIf, Else, Switch, Loop, Catch, Jump, Logic, Recursion }
    event_kind: EventKind { Panic, Unwrap, ErrorCompare, ErrorAssert, ErrorfUnwrapped }
    test_style: TestStyle { Table, Scenario }
}

/// The core manifest of a language a plugin declared in `initialize`.
pub(crate) fn language_manifest(language: &wire::LanguageManifest) -> LanguageManifest {
    LanguageManifest {
        id: language.id.clone(),
        aliases: language.aliases.clone(),
        globs: language.globs.clone(),
        ignores: language.ignores.clone(),
        conventions: language.conventions.clone(),
        // A name this host does not know is a capability it cannot use.
        capabilities: language
            .capabilities
            .iter()
            .filter_map(|c| c.parse().ok())
            .collect(),
        fallback: language.fallback,
        priority: language.priority,
        ordered_declarations: language.ordered_declarations,
        units: language.units.clone(),
    }
}

/// Converts a result against the units that were requested. A unit nobody
/// asked about or a second result for one is dropped with a notice; in a
/// unit, a fragment for a file nobody asked about, a second fragment for one
/// file, and a fragment whose symbols or edges belong to another file are
/// dropped with a notice; a requested file without an accepted fragment is
/// incomplete. The `Err` side means the result is malformed.
pub(crate) fn indexed(
    result: wire::IndexResult,
    units: &[Unit],
    required_asts: &BTreeSet<PathBuf>,
) -> Result<Indexed, String> {
    let mut incomplete: BTreeSet<Incomplete> =
        result.incomplete.into_iter().map(incomplete_of).collect();
    let mut out = Indexed {
        notices: result.notices,
        timings: result
            .timings
            .into_iter()
            .map(|t| (t.phase, std::time::Duration::from_micros(t.micros)))
            .collect(),
        ..Indexed::default()
    };
    let asked: BTreeMap<&str, &Unit> = units.iter().map(|unit| (unit.id, unit)).collect();
    let mut answered = BTreeSet::new();
    for unit in result.units {
        let Some(asked) = asked.get(unit.id.as_str()) else {
            out.notices
                .push(format!("{}: ignored, the unit was not requested", unit.id));
            continue;
        };
        if !answered.insert(unit.id.clone()) {
            out.notices.push(format!(
                "{}: ignored, the plugin sent a second result for the unit",
                unit.id
            ));
            continue;
        }
        let fragments = unit_of(unit, asked, required_asts)?;
        out.units.push(fragments);
    }
    for unit in units.iter().filter(|unit| !answered.contains(unit.id)) {
        incomplete.extend(
            unit.files
                .iter()
                .map(|source| unanswered(&source.file.path)),
        );
    }
    out.incomplete = incomplete.into_iter().collect();
    Ok(out)
}

fn unanswered(path: &std::path::Path) -> Incomplete {
    Incomplete {
        path: Some(path.to_owned()),
        provider: None,
        reason: "the plugin returned no usable fragment".to_owned(),
    }
}

/// The result of one unit, against the files it was given.
fn unit_of(
    result: wire::UnitResult,
    asked: &Unit,
    required_asts: &BTreeSet<PathBuf>,
) -> Result<Fragments, String> {
    let requested: BTreeMap<PathBuf, &core::File> = asked
        .files
        .iter()
        .map(|s| (s.file.path.clone(), s.file))
        .collect();
    let by_path: BTreeMap<PathBuf, &Source> = asked
        .files
        .iter()
        .map(|source| (source.file.path.clone(), source))
        .collect();
    let mut notices = result.notices;
    let mut incomplete: BTreeSet<Incomplete> =
        result.incomplete.into_iter().map(incomplete_of).collect();
    let mut out = Fragments {
        id: result.id,
        body: result.body,
        api: result.api,
        imports: result.imports,
        reads: result.reads,
        ..Fragments::default()
    };
    source_records(
        result.sources,
        &by_path,
        (&mut out.sources, &mut notices),
        &mut incomplete,
        required_asts,
    );
    let (mut offered, mut accepted) = (BTreeSet::new(), BTreeSet::new());
    for fragment in result.fragments {
        let path = PathBuf::from(&fragment.file.path);
        let Some(file) = requested.get(&path) else {
            notices.push(format!(
                "{}: ignored, the file was not requested",
                path.display()
            ));
            continue;
        };
        if !offered.insert(path.clone()) {
            notices.push(format!(
                "{}: ignored, the plugin sent a second fragment",
                path.display()
            ));
            continue;
        }
        if let Err(why) = belongs(&fragment) {
            notices.push(format!("{}: fragment dropped, {why}", path.display()));
            continue;
        }
        let facts = fragment.file.facts.clone();
        methods(&fragment, &mut out.methods)?;
        let mut converted = fragment_of(fragment)?;
        converted.files = vec![core::File {
            facts,
            ..(*file).clone()
        }];
        accepted.insert(path);
        out.fragments.push(converted);
    }
    incomplete.extend(
        requested
            .keys()
            .filter(|p| !accepted.contains(*p))
            .map(|path| unanswered(path)),
    );
    out.notices = notices;
    out.incomplete = incomplete.into_iter().collect();
    Ok(out)
}

fn source_records(
    sources: Vec<wire::Source>,
    by_path: &BTreeMap<PathBuf, &Source>,
    (kept, notices): (&mut Vec<wire::Source>, &mut Vec<String>),
    incomplete: &mut BTreeSet<Incomplete>,
    required: &BTreeSet<PathBuf>,
) {
    let mut seen = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    for source_ast in sources {
        let path = PathBuf::from(&source_ast.path);
        let Some(source) = by_path.get(&path) else {
            notices.push(format!(
                "{}: ignored source AST, the file was not requested",
                path.display()
            ));
            continue;
        };
        if !seen.insert(path.clone()) {
            kept.retain(|previous| previous.path != source_ast.path);
            incomplete.insert(Incomplete {
                path: Some(path.clone()),
                provider: None,
                reason: "duplicate source AST record for one physical file".to_owned(),
            });
            continue;
        }
        seen_paths.insert(path.clone());
        if let Err(reason) = source_ast.validate(source.text) {
            incomplete.insert(Incomplete {
                path: Some(path.clone()),
                provider: None,
                reason: format!("invalid source AST: {reason}"),
            });
            continue;
        }
        kept.push(source_ast);
    }
    for path in required {
        if by_path.contains_key(path) && !seen_paths.contains(path) {
            incomplete.insert(Incomplete {
                path: Some(path.clone()),
                provider: None,
                reason: "provider did not return a requested source AST".to_owned(),
            });
        }
    }
}

/// Checks that a fragment describes only its own file: symbols are declared in
/// it, and edges start at a module or symbol it declares (or at the owner of
/// a symbol it declares, which may live in another file of the same module).
fn belongs(f: &wire::Fragment) -> Result<(), String> {
    if let Some(s) = f.symbols.iter().find(|s| s.file != f.file.path) {
        return Err(format!("symbol `{}` is declared in `{}`", s.id, s.file));
    }
    let mut starts: BTreeSet<&str> = BTreeSet::new();
    starts.extend(f.symbols.iter().map(|s| s.id.as_str()));
    starts.extend(f.symbols.iter().filter_map(|s| s.owner.as_deref()));
    let modules: BTreeSet<&str> = f.modules.iter().map(|m| m.path.as_str()).collect();
    for edge in &f.edges {
        let ok = match &edge.from {
            wire::Node::Symbol(id) => starts.contains(id.as_str()),
            wire::Node::Module(path) => modules.contains(path.as_str()),
        };
        if !ok {
            return Err(format!(
                "an edge starts at `{:?}`, which the file does not declare",
                edge.from
            ));
        }
    }
    Ok(())
}

fn incomplete_of(item: wire::Incomplete) -> Incomplete {
    Incomplete {
        path: item.path.map(PathBuf::from),
        provider: None,
        reason: item.reason,
    }
}

fn fragment_of(f: wire::Fragment) -> Result<core::Fragment, String> {
    let path = PathBuf::from(&f.file.path);
    let mut symbols = Vec::new();
    let mut places = Vec::new();
    for wired in f.symbols {
        let (symbol, place) = symbol(wired)?;
        symbols.push(symbol);
        places.push(place);
    }
    let functions = f
        .functions
        .into_iter()
        .map(function)
        .collect::<Result<Vec<_>, _>>()?;
    for (summary, implementation) in &functions {
        if let Some(symbol) = symbols.iter_mut().find(|s| s.id == summary.symbol) {
            symbol.implementation = *implementation;
        }
    }
    Ok(core::Fragment {
        files: Vec::new(),
        modules: f.modules.into_iter().map(module).collect(),
        symbols,
        places,
        edges: f.edges.into_iter().map(edge).collect::<Result<_, _>>()?,
        functions: functions.into_iter().map(|(summary, _)| summary).collect(),
        tests: f.tests.into_iter().map(test).collect::<Result<_, _>>()?,
        comments: f
            .comments
            .into_iter()
            .map(|c| comment(&path, c))
            .collect::<Result<_, _>>()?,
    })
}

/// Adds what a fragment says of the method sets of its types and interfaces to
/// the unit's.
fn methods(f: &wire::Fragment, out: &mut core::Methods) -> Result<(), String> {
    let at = |s: &wire::Symbol| core::At {
        file: s.file.clone(),
        line: s.span.start.line,
        col: s.span.start.col,
    };
    for s in &f.symbols {
        if !s.provides.is_empty() {
            out.types.push(core::Provided {
                symbol: id(s.id.clone())?,
                at: at(s),
                keys: s.provides.clone(),
            });
        }
        if !s.requires.is_empty() {
            out.interfaces.push(core::Required {
                symbol: id(s.id.clone())?,
                at: at(s),
                keys: s.requires.clone(),
                edges: s.edges,
            });
        }
    }
    for pending in f
        .functions
        .iter()
        .filter_map(|f| f.pending.as_ref().map(|p| (&f.symbol, p)))
    {
        out.pending.push(core::Pending {
            method: id(pending.0.clone())?,
            key: pending.1.key.clone(),
            carriers: pending
                .1
                .carriers
                .iter()
                .cloned()
                .map(id)
                .collect::<Result<_, _>>()?,
        });
    }
    Ok(())
}

fn module(m: wire::Module) -> core::Module {
    core::Module {
        path: m.path,
        name: m.name,
        test_of: m.test_of,
        language: None,
        example: false,
    }
}

fn id(raw: String) -> Result<core::SymbolId, String> {
    core::SymbolId::parse(&raw).ok_or_else(|| format!("malformed symbol id `{raw}`"))
}

/// The shape and the place of one wire symbol. The wire carries them in one
/// record; the model keeps them apart so a body edit moves only the place.
fn symbol(s: wire::Symbol) -> Result<(core::Symbol, core::Place), String> {
    let symbol_id = id(s.id)?;
    let place = core::Place {
        symbol: symbol_id.clone(),
        span: span(s.span),
        extent: s.extent.map(span),
        doc: s.doc,
    };
    let symbol = core::Symbol {
        id: symbol_id,
        kind: symbol_kind(s.kind),
        visibility: visibility(s.visibility),
        owner: s.owner.map(id).transpose()?,
        file: PathBuf::from(s.file),
        name: s.name,
        role: s.role.map(role),
        optional: s.optional,
        type_ref: s.type_ref.map(type_ref),
        implementation: false,
    };
    Ok((symbol, place))
}

fn span(s: wire::Span) -> Span {
    let at = |p: wire::Position| Position {
        line: p.line,
        col: p.col,
    };
    Span {
        start: at(s.start),
        end: at(s.end),
    }
}

fn edge(e: wire::Edge) -> Result<core::Edge, String> {
    Ok(core::Edge {
        kind: edge_kind(e.kind),
        from: match e.from {
            wire::Node::Module(path) => core::Node::Module(path),
            wire::Node::Symbol(raw) => core::Node::Symbol(id(raw)?),
        },
        to: core::Target::Path(e.to),
        resolution: resolution(e.resolution),
        site: e.site.map(span),
        when: match e.when {
            wire::When::Call => core::When::Call,
            wire::When::Load => core::When::Load,
        },
    })
}

fn flow(f: wire::Flow) -> core::Flow {
    core::Flow {
        kind: flow_kind(f.kind),
        nesting: f.nesting,
        arms: f.arms,
        operators: f.operators,
        returning: f.returning,
    }
}

/// The summary, and whether the function is an implementation, which the model
/// keeps on the symbol.
fn function(f: wire::FunctionSummary) -> Result<(core::FunctionSummary, bool), String> {
    let implementation = f.implementation;
    let summary = core::FunctionSummary {
        symbol: id(f.symbol)?,
        max_nesting: f.max_nesting,
        statements: f.statements,
        top_level: f.top_level,
        params: f.params,
        returns: f.returns,
        tokens: f.tokens,
        flow: f.flow.into_iter().map(flow).collect(),
        forwards_to: f.forwards_to.map(core::Target::Path),
        signature: core::Signature {
            params: f.signature.params.into_iter().map(type_ref).collect(),
            results: f.signature.results.into_iter().map(type_ref).collect(),
        },
        events: f.events.into_iter().map(event).collect(),
        manual_assertions: f.manual_assertions,
        constructs: f.constructs,
        configures: f.configures,
    };
    Ok((summary, implementation))
}

fn type_ref(t: wire::TypeRef) -> core::TypeRef {
    core::TypeRef {
        text: t.text,
        symbol: t.symbol,
        exported: t.exported,
    }
}

fn event(e: wire::Event) -> core::Event {
    core::Event {
        kind: event_kind(e.kind),
        span: span(e.span),
        detail: e.detail,
    }
}

fn test(t: wire::TestCase) -> Result<core::TestCase, String> {
    Ok(core::TestCase {
        symbol: id(t.symbol)?,
        nesting: t.nesting,
        style: test_style(t.style),
        targets: t.targets.into_iter().map(core::Target::Path).collect(),
    })
}

fn comment(file: &std::path::Path, c: wire::Comment) -> Result<core::Comment, String> {
    Ok(core::Comment {
        file: file.to_owned(),
        span: span(c.span),
        text: c.text,
        attached_to: c.attached_to.map(id).transpose()?,
    })
}
