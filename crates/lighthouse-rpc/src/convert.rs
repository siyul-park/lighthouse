//! The only place where wire types become code model (UCM) types.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Incomplete, Position, Span, ucm as core};
use lighthouse_plugin::{Indexed, ProviderManifest, Source};
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
pub(crate) fn provider_manifest(language: &wire::ProviderManifest) -> ProviderManifest {
    ProviderManifest {
        id: language.id.clone(),
        globs: language.globs.clone(),
        conventions: language.conventions.clone(),
        // A name this host does not know is a capability it cannot use.
        capabilities: language
            .capabilities
            .iter()
            .filter_map(|c| c.parse().ok())
            .collect(),
        fallback: language.fallback,
        priority: language.priority,
    }
}

/// Converts a result against the files that were requested. A fragment for a
/// file nobody asked about, a second fragment for one file, and a fragment
/// whose symbols or edges belong to another file are dropped with a notice; a
/// requested file without an accepted fragment is incomplete. The `Err` side
/// means the result is malformed.
pub(crate) fn indexed(
    result: wire::IndexResult,
    sources: &[Source],
    required_asts: &BTreeSet<PathBuf>,
) -> Result<Indexed, String> {
    let requested: BTreeMap<PathBuf, &core::File> = sources
        .iter()
        .map(|s| (s.file.path.clone(), s.file))
        .collect();
    let mut incomplete: BTreeSet<Incomplete> =
        result.incomplete.into_iter().map(incomplete_of).collect();
    let mut out = Indexed {
        notices: result.notices,
        ..Indexed::default()
    };
    let mut offered = BTreeSet::new();
    let mut accepted = BTreeSet::new();
    let by_path: BTreeMap<PathBuf, &Source> = sources
        .iter()
        .map(|source| (source.file.path.clone(), source))
        .collect();
    source_records(
        result.sources,
        &by_path,
        &mut out,
        &mut incomplete,
        required_asts,
    );
    for fragment in result.fragments {
        let path = PathBuf::from(&fragment.file.path);
        let Some(file) = requested.get(&path) else {
            out.notices.push(format!(
                "{}: ignored, the file was not requested",
                path.display()
            ));
            continue;
        };
        if !offered.insert(path.clone()) {
            out.notices.push(format!(
                "{}: ignored, the plugin sent a second fragment",
                path.display()
            ));
            continue;
        }
        if let Err(why) = belongs(&fragment) {
            out.notices
                .push(format!("{}: fragment dropped, {why}", path.display()));
            continue;
        }
        let generated = fragment.file.generated;
        let facts = fragment.file.facts.clone();
        let mut converted = fragment_of(fragment)?;
        converted.files = vec![core::File {
            generated: generated || file.generated,
            facts,
            ..(*file).clone()
        }];
        accepted.insert(path);
        out.fragments.push(converted);
    }
    for path in requested.keys().filter(|p| !accepted.contains(*p)) {
        incomplete.insert(Incomplete {
            path: Some(path.clone()),
            reason: "the plugin returned no usable fragment".to_owned(),
        });
    }
    out.incomplete = incomplete.into_iter().collect();
    Ok(out)
}

fn source_records(
    sources: Vec<wire::Source>,
    by_path: &BTreeMap<PathBuf, &Source>,
    out: &mut Indexed,
    incomplete: &mut BTreeSet<Incomplete>,
    required: &BTreeSet<PathBuf>,
) {
    let mut seen = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    for source_ast in sources {
        let path = PathBuf::from(&source_ast.path);
        let Some(source) = by_path.get(&path) else {
            out.notices.push(format!(
                "{}: ignored source AST, the file was not requested",
                path.display()
            ));
            continue;
        };
        if !seen.insert(path.clone()) {
            out.sources
                .retain(|previous| previous.path != source_ast.path);
            incomplete.insert(Incomplete {
                path: Some(path.clone()),
                reason: "duplicate source AST record for one physical file".to_owned(),
            });
            continue;
        }
        seen_paths.insert(path.clone());
        if let Err(reason) = source_ast.validate(source.text) {
            incomplete.insert(Incomplete {
                path: Some(path.clone()),
                reason: format!("invalid source AST: {reason}"),
            });
            continue;
        }
        out.sources.push(source_ast);
    }
    for path in required {
        if by_path.contains_key(path) && !seen_paths.contains(path) {
            incomplete.insert(Incomplete {
                path: Some(path.clone()),
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
        reason: item.reason,
    }
}

fn fragment_of(f: wire::Fragment) -> Result<core::Fragment, String> {
    let path = PathBuf::from(&f.file.path);
    Ok(core::Fragment {
        files: Vec::new(),
        modules: f.modules.into_iter().map(module).collect(),
        symbols: f
            .symbols
            .into_iter()
            .map(symbol)
            .collect::<Result<_, _>>()?,
        edges: f.edges.into_iter().map(edge).collect::<Result<_, _>>()?,
        functions: f
            .functions
            .into_iter()
            .map(function)
            .collect::<Result<_, _>>()?,
        tests: f.tests.into_iter().map(test).collect::<Result<_, _>>()?,
        comments: f
            .comments
            .into_iter()
            .map(|c| comment(&path, c))
            .collect::<Result<_, _>>()?,
    })
}

fn module(m: wire::Module) -> core::Module {
    core::Module {
        path: m.path,
        name: m.name,
        test_of: m.test_of,
    }
}

fn id(raw: String) -> Result<core::SymbolId, String> {
    core::SymbolId::parse(&raw).ok_or_else(|| format!("malformed symbol id `{raw}`"))
}

fn symbol(s: wire::Symbol) -> Result<core::Symbol, String> {
    Ok(core::Symbol {
        id: id(s.id)?,
        kind: symbol_kind(s.kind),
        visibility: visibility(s.visibility),
        owner: s.owner.map(id).transpose()?,
        file: PathBuf::from(s.file),
        span: span(s.span),
        extent: s.extent.map(span),
        doc: s.doc,
        name: s.name,
        role: s.role.map(role),
        optional: s.optional,
        type_ref: s.type_ref.map(type_ref),
    })
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

fn function(f: wire::FunctionSummary) -> Result<core::FunctionSummary, String> {
    Ok(core::FunctionSummary {
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
        implementation: f.implementation,
        constructs: f.constructs,
    })
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
