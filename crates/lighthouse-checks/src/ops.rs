//! The standard operations of a `builtin` check: `order` and
//! `cycle`. They judge what an expression over one value cannot, the shape of
//! a whole file or of the dependency graph, and know nothing about any
//! decision: the keys, the groups, the wording and the evidence come from the
//! check that uses them.

use std::collections::BTreeSet;

use cel::Program;
use lighthouse_model::{
    Diagnostic, Reach,
    ucm::{EdgeKind, Node, Project, Symbol, SymbolId, Target},
};
use lighthouse_plugin::{Context, Error as PluginError, KeyContext, OrderKey, RuleManifest};
use lighthouse_spec::{BuiltinOp, CycleLevel, OrderReport, OrderScope};
use serde_json::{Map, Value, json};

use crate::{
    Error,
    builder::Builder,
    caching,
    eval::{Frame, Template, compile, compile_all},
    layout::declarations,
    library::{self, Needs},
    metrics::graph::Graph,
    order,
};

/// `order`: declarations follow the order of registered order keys.
pub(crate) struct OrderRule {
    clauses: Vec<Clause>,
    needs: Needs,
    library: cel::Context<'static, 'static>,
}

struct Clause {
    within: OrderScope,
    by: Vec<String>,
    report: OrderReport,
    when: Option<Program>,
    message: Template,
    evidence: Vec<(String, Program)>,
}

/// `cycle`: no dependency cycle over edges of one kind.
pub(crate) struct CycleRule {
    edge: EdgeKind,
    level: CycleLevel,
}

impl OrderRule {
    pub(crate) fn new(id: &str, op: &BuiltinOp) -> Result<Self, Error> {
        let BuiltinOp::Order { clauses } = op else {
            return Err(Error::invalid(id, "not an order check"));
        };
        let mut sources: Vec<&str> = Vec::new();
        let mut compiled = Vec::new();
        for clause in clauses {
            sources.push(&clause.message);
            sources.extend(clause.when.as_deref());
            sources.extend(clause.evidence.values().map(String::as_str));
            compiled.push(Clause {
                within: clause.within,
                by: clause.by.clone(),
                report: clause.report,
                when: clause
                    .when
                    .as_deref()
                    .map(|w| compile(id, "when", w))
                    .transpose()?,
                message: Template::compile(id, "message", &clause.message)?,
                evidence: compile_all(id, "evidence", &clause.evidence)?,
            });
        }
        Ok(Self {
            clauses: compiled,
            needs: Needs::of(sources),
            library: library::context().map_err(|e| Error::invalid(id, e))?,
        })
    }

    /// The declarations of a file are ordered by what the file says of them,
    /// besides what the expressions mention; a key from another plugin may
    /// read the project.
    pub(crate) fn reach(&self) -> (Reach, bool) {
        let core = |id: &String| order::CORE_KEYS.contains(&id.as_str());
        if self.clauses.iter().all(|c| c.by.iter().all(core)) {
            caching::reach(&self.needs)
        } else {
            (Reach::Global, false)
        }
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Context,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let builder = Builder::new(ctx, &self.needs, options, &meta.id)?;
        let mut found = Vec::new();
        for (at, clause) in self.clauses.iter().enumerate() {
            if !self.applies(&meta.id, clause.when.as_ref(), &builder, ctx, options)? {
                continue;
            }
            found.extend(clause.check(at, meta, ctx, &builder, &self.library, options)?);
        }
        Ok(found)
    }

    /// Whether the clause runs on the focused file: the file is a subject of
    /// the decision's scope and the clause's own `when` holds.
    fn applies(
        &self,
        rule: &str,
        when: Option<&Program>,
        builder: &Builder,
        ctx: &Context,
        options: &Map<String, Value>,
    ) -> Result<bool, PluginError> {
        if ctx
            .file
            .is_some_and(|(file, _)| !ctx.applies.admits_file(file))
        {
            return Ok(false);
        }
        let Some(when) = when else {
            return Ok(true);
        };
        let Some((file, text)) = ctx.file else {
            return Ok(false);
        };
        let mut frame = Frame::new(rule, &self.library);
        frame.set("file", &builder.file(file, text));
        frame.options(options);
        frame.truth(when)
    }
}

impl Clause {
    /// The findings of clause number `at`; the first clause fingerprints on the
    /// declaration alone, the others also on their number, so two clauses that
    /// report the same declaration are two findings.
    fn check(
        &self,
        at_clause: usize,
        meta: &RuleManifest,
        ctx: &Context,
        builder: &Builder,
        base: &cel::Context<'static, 'static>,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let keys = self.keys(ctx, &meta.id)?;
        let language = ctx.file.map_or("", |(file, _)| file.lang.as_str());
        let key_ctx = KeyContext {
            project: ctx.project,
            language,
            rule: &meta.id,
            options,
            constructors: ctx.workspace.constructor_prefixes(language),
        };
        let mut found = Vec::new();
        for module in declarations(ctx) {
            for container in self.containers(&module) {
                let ranked = ranked(&key_ctx, &keys, &container)?;
                for (at, earlier) in self.reported(&ranked) {
                    let (symbol, ranks) = &ranked[at];
                    let mut frame = Frame::new(&meta.id, base);
                    frame.set("symbol", &builder.symbol(symbol));
                    let other = node_value(builder, earlier.map(|i| ranked[i].0));
                    frame.set("after", &other);
                    frame.set("earlier", &other);
                    frame.set("rank", &json!(ranks.first()));
                    frame.set("ranks", &json!(ranks));
                    frame.options(options);
                    found.push(finding(
                        meta,
                        symbol,
                        frame.text(&self.message)?,
                        frame.evidence(&self.evidence)?,
                        &if at_clause == 0 {
                            String::new()
                        } else {
                            format!("clause {at_clause}")
                        },
                    ));
                }
            }
        }
        Ok(found)
    }

    /// The positions to report, each with the position of the declaration it
    /// is reported against.
    fn reported(&self, ranked: &[(&Symbol, Vec<u64>)]) -> Vec<(usize, Option<usize>)> {
        match self.report {
            OrderReport::Displaced => {
                let kept =
                    longest_in_order(&ranked.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>());
                (0..ranked.len())
                    .filter(|at| !kept[*at])
                    .map(|at| {
                        // The declaration to follow is one of an earlier group
                        // that still lies below this one: the first such that
                        // stays in place, else the first such.
                        let earlier = |i: &usize| ranked[*i].1 < ranked[at].1;
                        let after = (at + 1..ranked.len())
                            .find(|i| kept[*i] && earlier(i))
                            .or_else(|| (at + 1..ranked.len()).find(earlier));
                        (at, after)
                    })
                    .collect()
            }
            OrderReport::Late => (0..ranked.len())
                .filter_map(|at| {
                    let earlier = ranked[..at].iter().position(|(_, r)| *r > ranked[at].1)?;
                    Some((at, Some(earlier)))
                })
                .collect(),
        }
    }

    fn keys<'c>(
        &self,
        ctx: &Context<'c>,
        rule: &str,
    ) -> Result<Vec<&'c dyn OrderKey>, PluginError> {
        self.by
            .iter()
            .map(|id| {
                ctx.keys.key(id).ok_or_else(|| {
                    PluginError::Incomplete(format!(
                        "{rule}: execution error: unknown order key `{id}`"
                    ))
                })
            })
            .collect()
    }

    /// The lists of a module's declarations that are ordered on their own.
    fn containers<'a>(&self, module: &[&'a Symbol]) -> Vec<Vec<&'a Symbol>> {
        match self.within {
            OrderScope::File => vec![module.to_vec()],
            OrderScope::Owner => {
                let mut by_owner: Vec<(Option<String>, Vec<&Symbol>)> = Vec::new();
                for symbol in module {
                    let owner = crate::layout::owner_key(symbol);
                    match by_owner.iter_mut().find(|(o, _)| *o == owner) {
                        Some((_, list)) => list.push(symbol),
                        None => by_owner.push((owner, vec![symbol])),
                    }
                }
                by_owner.into_iter().map(|(_, list)| list).collect()
            }
        }
    }
}

impl CycleRule {
    pub(crate) fn new(id: &str, op: &BuiltinOp) -> Result<Self, Error> {
        let BuiltinOp::Cycle { edge, level } = op else {
            return Err(Error::invalid(id, "not a cycle check"));
        };
        Ok(Self {
            edge: edge
                .parse()
                .map_err(|_| Error::invalid(id, format!("`{edge}` is not an edge kind")))?,
            level: *level,
        })
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Context,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let edges = self.edges(project);
        let units: BTreeSet<&String> = edges.iter().flat_map(|(from, to)| [from, to]).collect();
        let units: Vec<&String> = units.into_iter().collect();
        let at = |unit: &String| u32::try_from(units.binary_search(&unit).ok()?).ok();
        let graph = Graph::new(
            units.len(),
            edges
                .iter()
                .filter_map(|(from, to)| Some((at(from)?, at(to)?))),
        );
        let mut found = Vec::new();
        for cycle in graph.cycles() {
            let component: Vec<&str> = cycle.iter().map(|&v| units[v as usize].as_str()).collect();
            let Some(anchor) = self.anchor(project, component[0]) else {
                continue;
            };
            let unit = match self.level {
                CycleLevel::Module => "modules",
                CycleLevel::Symbol => "symbols",
            };
            let message = format!(
                "dependency cycle of {} {unit} over {} edges: {}",
                component.len(),
                self.edge.as_str(),
                component.join(" -> ")
            );
            let fingerprint = meta.fingerprint(&component.join(","), "");
            let mut diagnostic = Diagnostic::new(
                &meta.id,
                meta.severity,
                message,
                &anchor.file,
                anchor.span,
                fingerprint,
            );
            diagnostic.symbol = Some(anchor.id.as_str().to_owned());
            diagnostic.evidence = json!({ "members": component });
            found.push(diagnostic);
        }
        Ok(found)
    }

    /// The edges of the graph, between the units they join.
    fn edges(&self, project: &Project) -> Vec<(String, String)> {
        project
            .edges
            .iter()
            .filter(|e| e.kind == self.edge)
            .filter_map(|edge| {
                let Target::Resolved(to) = &edge.to else {
                    return None;
                };
                Some((self.unit(&edge.from)?, self.unit(to)?))
            })
            .collect()
    }

    fn unit(&self, node: &Node) -> Option<String> {
        match (self.level, node) {
            (CycleLevel::Module, Node::Module(m)) => Some(m.clone()),
            (CycleLevel::Module, Node::Symbol(id)) => Some(id.module().to_owned()),
            (CycleLevel::Symbol, Node::Symbol(id)) => Some(id.as_str().to_owned()),
            (CycleLevel::Symbol, Node::Module(_)) => None,
        }
    }

    /// Where the cycle is reported: the first symbol of its first unit.
    fn anchor<'p>(&self, project: &'p Project, unit: &str) -> Option<&'p Symbol> {
        match self.level {
            CycleLevel::Symbol => SymbolId::parse(unit).and_then(|id| project.symbol(&id)),
            CycleLevel::Module => project
                .symbols
                .iter()
                .filter(|s| s.id.module() == unit)
                .min_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start))),
        }
    }
}

fn node_value(builder: &Builder, symbol: Option<&Symbol>) -> Value {
    symbol.map_or_else(
        || json!({ "id": "", "name": "", "kind": "", "owner": "", "file": "", "line": 0 }),
        |s| builder.symbol(s),
    )
}

/// The declarations the first key orders, each with its ranks by every key; a
/// key that does not order a declaration the first key does ranks it last.
fn ranked<'a>(
    ctx: &KeyContext,
    keys: &[&dyn OrderKey],
    symbols: &[&'a Symbol],
) -> Result<Vec<(&'a Symbol, Vec<u64>)>, PluginError> {
    let mut ranked = Vec::new();
    for symbol in symbols {
        if let Some(ranks) = order::ranks(ctx, keys, symbol)? {
            ranked.push((*symbol, ranks));
        }
    }
    Ok(ranked)
}

/// Marks a longest non-decreasing subsequence of `ranks`; the first of equally
/// long ones, so the verdict does not change between runs. What it leaves
/// unmarked is the fewest declarations that, moved, would put the file in
/// order.
fn longest_in_order(ranks: &[Vec<u64>]) -> Vec<bool> {
    let n = ranks.len();
    let mut best = vec![1usize; n];
    let mut from = vec![usize::MAX; n];
    for i in 0..n {
        for j in 0..i {
            if ranks[j] <= ranks[i] && best[j] + 1 > best[i] {
                best[i] = best[j] + 1;
                from[i] = j;
            }
        }
    }
    let mut keep = vec![false; n];
    let Some(mut at) = (0..n).max_by_key(|&i| (best[i], std::cmp::Reverse(i))) else {
        return keep;
    };
    loop {
        keep[at] = true;
        match from[at] {
            usize::MAX => break,
            previous => at = previous,
        }
    }
    keep
}

fn finding(
    meta: &RuleManifest,
    symbol: &Symbol,
    message: String,
    evidence: Value,
    snippet: &str,
) -> Diagnostic {
    let fingerprint = meta.fingerprint(symbol.id.as_str(), snippet);
    let mut diagnostic = Diagnostic::new(
        &meta.id,
        meta.severity,
        message,
        &symbol.file,
        symbol.span,
        fingerprint,
    );
    diagnostic.symbol = Some(symbol.id.as_str().to_owned());
    diagnostic.evidence = evidence;
    diagnostic
}
