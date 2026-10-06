//! The only place where wire types become core model types.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model as core;
use lighthouse_plugin::{Indexed, Source};
use lighthouse_protocol as wire;

/// Core capability for a wire capability name; unknown names are ignored.
pub(crate) fn capability(name: &str) -> Option<core::Capability> {
    (name == wire::SEMANTIC_EDGES).then_some(core::Capability::SemanticEdges)
}

/// Converts a result against the files that were requested. A fragment for a
/// file nobody asked about, a second fragment for one file, and a fragment
/// whose symbols or edges belong to another file are dropped with a notice; a
/// requested file without an accepted fragment is incomplete. The `Err` side
/// means the result is malformed.
pub(crate) fn indexed(result: wire::IndexResult, sources: &[Source]) -> Result<Indexed, String> {
    let requested: BTreeMap<PathBuf, &core::File> = sources
        .iter()
        .map(|s| (s.file.path.clone(), s.file))
        .collect();
    let mut incomplete: BTreeSet<core::Incomplete> = result
        .incomplete
        .into_iter()
        .map(self::incomplete)
        .collect();
    let mut out = Indexed {
        notices: result.notices,
        ..Indexed::default()
    };
    let mut offered = BTreeSet::new();
    let mut accepted = BTreeSet::new();
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
        let mut converted = self::fragment(fragment)?;
        converted.files = vec![core::File {
            generated,
            ..(*file).clone()
        }];
        accepted.insert(path);
        out.fragments.push(converted);
    }
    for path in requested.keys().filter(|p| !accepted.contains(*p)) {
        incomplete.insert(core::Incomplete {
            path: Some(path.clone()),
            reason: "the plugin returned no usable fragment".to_owned(),
        });
    }
    out.incomplete = incomplete.into_iter().collect();
    Ok(out)
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

fn incomplete(item: wire::Incomplete) -> core::Incomplete {
    core::Incomplete {
        path: item.path.map(PathBuf::from),
        reason: item.reason,
    }
}

fn fragment(f: wire::Fragment) -> Result<core::Fragment, String> {
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
        visibility: match s.visibility {
            wire::Visibility::Public => core::Visibility::Public,
            wire::Visibility::Private => core::Visibility::Private,
            wire::Visibility::Internal => core::Visibility::Internal,
        },
        owner: s.owner.map(id).transpose()?,
        file: PathBuf::from(s.file),
        span: span(s.span),
        doc: s.doc,
        name: s.name,
    })
}

fn symbol_kind(kind: wire::SymbolKind) -> core::SymbolKind {
    match kind {
        wire::SymbolKind::Function => core::SymbolKind::Function,
        wire::SymbolKind::Method => core::SymbolKind::Method,
        wire::SymbolKind::Type => core::SymbolKind::Type,
        wire::SymbolKind::Field => core::SymbolKind::Field,
        wire::SymbolKind::Const => core::SymbolKind::Const,
        wire::SymbolKind::Var => core::SymbolKind::Var,
        wire::SymbolKind::Interface => core::SymbolKind::Interface,
        wire::SymbolKind::Test => core::SymbolKind::Test,
    }
}

fn span(s: wire::Span) -> core::Span {
    let at = |p: wire::Position| core::Position {
        line: p.line,
        col: p.col,
    };
    core::Span {
        start: at(s.start),
        end: at(s.end),
    }
}

fn edge(e: wire::Edge) -> Result<core::Edge, String> {
    Ok(core::Edge {
        kind: match e.kind {
            wire::EdgeKind::Calls => core::EdgeKind::Calls,
            wire::EdgeKind::References => core::EdgeKind::References,
            wire::EdgeKind::Imports => core::EdgeKind::Imports,
            wire::EdgeKind::Contains => core::EdgeKind::Contains,
            wire::EdgeKind::Implements => core::EdgeKind::Implements,
            wire::EdgeKind::AccessesPrivate => core::EdgeKind::AccessesPrivate,
        },
        from: match e.from {
            wire::Node::Module(path) => core::Node::Module(path),
            wire::Node::Symbol(raw) => core::Node::Symbol(id(raw)?),
        },
        to: core::Target::Path(e.to),
        resolution: match e.resolution {
            wire::Resolution::Semantic => core::Resolution::Semantic,
            wire::Resolution::Syntactic => core::Resolution::Syntactic,
        },
    })
}

fn flow(f: wire::Flow) -> core::Flow {
    core::Flow {
        kind: match f.kind {
            wire::FlowKind::If => core::FlowKind::If,
            wire::FlowKind::ElseIf => core::FlowKind::ElseIf,
            wire::FlowKind::Else => core::FlowKind::Else,
            wire::FlowKind::Switch => core::FlowKind::Switch,
            wire::FlowKind::Loop => core::FlowKind::Loop,
            wire::FlowKind::Catch => core::FlowKind::Catch,
            wire::FlowKind::Jump => core::FlowKind::Jump,
            wire::FlowKind::Logic => core::FlowKind::Logic,
            wire::FlowKind::Recursion => core::FlowKind::Recursion,
        },
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
        clone_fingerprint: f.clone_fingerprint.map(core::Fingerprint::from_raw),
        forwards_to: f.forwards_to.map(core::Target::Path),
    })
}

fn test(t: wire::TestCase) -> Result<core::TestCase, String> {
    Ok(core::TestCase {
        symbol: id(t.symbol)?,
        nesting: t.nesting,
        style: match t.style {
            wire::TestStyle::Table => core::TestStyle::Table,
            wire::TestStyle::Scenario => core::TestStyle::Scenario,
        },
        targets: t.targets.into_iter().map(core::Target::Path).collect(),
    })
}
