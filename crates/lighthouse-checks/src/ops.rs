//! The standard operations of a `builtin` check: `order`, `proximity` and
//! `cycle`. They judge what an expression over one value cannot, the shape of
//! a whole file or of the dependency graph, and know nothing about any
//! decision: the keys, the groups, the wording and the evidence come from the
//! check that uses them.

use std::collections::{BTreeMap, BTreeSet};

use cel::Program;
use lighthouse_model::{
    Diagnostic, EdgeKind, Fingerprint, Node, Project, Symbol, SymbolId, Target,
};
use lighthouse_plugin::{Ctx, Error as PluginError, KeyCtx, OrderKey, RuleManifest};
use lighthouse_spec::{BuiltinOp, CycleLevel, OrderReport, OrderScope};
use serde_json::{Map, Value, json};

use crate::{
    Error,
    builder::Builder,
    eval::{Frame, Template, compile, compile_all},
    layout::declarations,
    library::{self, Needs},
};

/// `order`: declarations follow the order of registered order keys.
pub(crate) struct OrderRule {
    clauses: Vec<Clause>,
    needs: Needs,
}

struct Clause {
    within: OrderScope,
    by: Vec<String>,
    report: OrderReport,
    when: Option<Program>,
    message: Template,
    evidence: Vec<(String, Program)>,
}

/// `proximity`: declarations that belong together stay together.
pub(crate) struct ProximityRule {
    when: Option<Program>,
    group: Program,
    separator: Option<Program>,
    max_distance: usize,
    message: Template,
    evidence: Vec<(String, Program)>,
    needs: Needs,
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
        })
    }

    pub(crate) fn analyzers(&self) -> Vec<String> {
        Vec::new()
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Ctx,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let builder = Builder::new(ctx, &self.needs, options, &meta.id)?;
        let base = library::context();
        let mut found = Vec::new();
        for (at, clause) in self.clauses.iter().enumerate() {
            if !applies(&meta.id, clause.when.as_ref(), &builder, ctx, options)? {
                continue;
            }
            found.extend(clause.check(at, meta, ctx, &builder, &base, options)?);
        }
        Ok(found)
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
        ctx: &Ctx,
        builder: &Builder,
        base: &cel::Context<'static>,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let keys = self.keys(ctx, &meta.id)?;
        let language = ctx.file.map_or("", |(file, _)| file.lang.as_str());
        let key_ctx = KeyCtx {
            project: ctx.project,
            language,
            rule: &meta.id,
            options,
        };
        let mut found = Vec::new();
        for module in declarations(ctx) {
            for container in self.containers(&module) {
                let ranked = ranked(&key_ctx, &keys, &container)?;
                for (at, earlier) in self.reported(&ranked) {
                    let (symbol, ranks) = &ranked[at];
                    let mut frame = Frame::new(&meta.id, base);
                    frame.set("symbol", &builder.symbol(symbol))?;
                    let other = node_value(builder, earlier.map(|i| ranked[i].0));
                    frame.set("after", &other)?;
                    frame.set("earlier", &other)?;
                    frame.set("rank", &json!(ranks.first()))?;
                    frame.set("ranks", &json!(ranks))?;
                    frame.options(options)?;
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
                        let after = ranked[..at]
                            .iter()
                            .enumerate()
                            .rev()
                            .find(|(i, (_, r))| kept[*i] && *r <= ranked[at].1)
                            .map(|(i, _)| i);
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

    fn keys<'c>(&self, ctx: &Ctx<'c>, rule: &str) -> Result<Vec<&'c dyn OrderKey>, PluginError> {
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

impl ProximityRule {
    pub(crate) fn new(id: &str, op: &BuiltinOp) -> Result<Self, Error> {
        let BuiltinOp::Proximity {
            when,
            group,
            separator,
            max_distance,
            message,
            evidence,
            ..
        } = op
        else {
            return Err(Error::invalid(id, "not a proximity check"));
        };
        let mut sources: Vec<&str> = vec![group.as_str(), message.as_str()];
        sources.extend(separator.as_deref());
        sources.extend(evidence.values().map(String::as_str));
        Ok(Self {
            when: when
                .as_deref()
                .map(|w| compile(id, "when", w))
                .transpose()?,
            group: compile(id, "group", group)?,
            separator: separator
                .as_deref()
                .map(|s| compile(id, "separator", s))
                .transpose()?,
            max_distance: max_distance.unwrap_or(0) as usize,
            message: Template::compile(id, "message", message)?,
            evidence: compile_all(id, "evidence", evidence)?,
            needs: Needs::of(sources),
        })
    }

    pub(crate) fn analyzers(&self) -> Vec<String> {
        Vec::new()
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Ctx,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let builder = Builder::new(ctx, &self.needs, options, &meta.id)?;
        if !applies(&meta.id, self.when.as_ref(), &builder, ctx, options)? {
            return Ok(Vec::new());
        }
        let base = library::context();
        let mut found = Vec::new();
        for module in declarations(ctx) {
            let values: Vec<Value> = module.iter().map(|s| builder.symbol(s)).collect();
            let mut keys = Vec::new();
            for value in &values {
                let mut frame = Frame::new(&meta.id, &base);
                frame.set("symbol", value)?;
                frame.options(options)?;
                keys.push(frame.string(&self.group, "group")?);
            }
            for (at, key) in keys.iter().enumerate() {
                if key.is_empty() {
                    continue;
                }
                let Some(before) = keys[..at].iter().rposition(|k| k == key) else {
                    continue;
                };
                let mut separators = Vec::new();
                for between in before + 1..at {
                    if self.separates(
                        &base,
                        meta,
                        options,
                        &values[between],
                        &values[at],
                        key,
                        &keys[between],
                    )? {
                        separators.push(between);
                    }
                }
                if separators.len() <= self.max_distance {
                    continue;
                }
                let mut frame = Frame::new(&meta.id, &base);
                frame.set("symbol", &values[at])?;
                frame.set("previous", &values[before])?;
                frame.set(
                    "separators",
                    &Value::Array(separators.iter().map(|i| values[*i].clone()).collect()),
                )?;
                frame.set("count", &json!(separators.len()))?;
                frame.options(options)?;
                found.push(finding(
                    meta,
                    module[at],
                    frame.text(&self.message)?,
                    frame.evidence(&self.evidence)?,
                    "",
                ));
            }
        }
        Ok(found)
    }

    #[allow(clippy::too_many_arguments)]
    fn separates(
        &self,
        base: &cel::Context<'static>,
        meta: &RuleManifest,
        options: &Map<String, Value>,
        between: &Value,
        member: &Value,
        key: &str,
        between_key: &str,
    ) -> Result<bool, PluginError> {
        let Some(separator) = &self.separator else {
            return Ok(between_key != key);
        };
        let mut frame = Frame::new(&meta.id, base);
        frame.set("between", between)?;
        frame.set("member", member)?;
        frame.set("key", &json!(key))?;
        frame.options(options)?;
        frame.truth(separator)
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
                .map_err(|()| Error::invalid(id, format!("`{edge}` is not an edge kind")))?,
            level: *level,
        })
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Ctx,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let graph = self.graph(project);
        let mut found = Vec::new();
        for component in strongly_connected(&graph) {
            if component.len() < 2 {
                continue;
            }
            let Some(anchor) = self.anchor(project, &component[0]) else {
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
            let fingerprint = Fingerprint::of(&meta.id, &component.join(","), "");
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

    /// The adjacency of the graph, by sorted unit name.
    fn graph(&self, project: &Project) -> BTreeMap<String, BTreeSet<String>> {
        let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for edge in project.edges.iter().filter(|e| e.kind == self.edge) {
            let Target::Resolved(to) = &edge.to else {
                continue;
            };
            let (Some(from), Some(to)) = (self.unit(&edge.from), self.unit(to)) else {
                continue;
            };
            graph.entry(to.clone()).or_default();
            if from != to {
                graph.entry(from).or_default().insert(to);
            }
        }
        graph
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

/// The state of one Tarjan walk.
struct Walk<'g> {
    graph: &'g BTreeMap<String, BTreeSet<String>>,
    index: BTreeMap<&'g str, usize>,
    low: BTreeMap<&'g str, usize>,
    /// Nodes entered whose component is not yet closed.
    open: Vec<&'g str>,
    on_open: BTreeSet<&'g str>,
    components: Vec<Vec<String>>,
}

impl<'g> Walk<'g> {
    fn from(&mut self, root: &'g str) {
        // Each frame is a node and the successors it has yet to visit.
        let mut frames = vec![(root, self.enter(root))];
        while let Some((node, successors)) = frames.last_mut() {
            let node = *node;
            match successors.next() {
                Some(successor) => {
                    let successor = successor.as_str();
                    if self.index.contains_key(successor) {
                        self.lower(node, successor);
                    } else {
                        let following = self.enter(successor);
                        frames.push((successor, following));
                    }
                }
                None => {
                    frames.pop();
                    if let Some((parent, _)) = frames.last() {
                        let lowest = self.low[parent].min(self.low[node]);
                        self.low.insert(parent, lowest);
                    }
                    self.close(node);
                }
            }
        }
    }

    /// Numbers `node` and gives its successors.
    fn enter(&mut self, node: &'g str) -> std::collections::btree_set::Iter<'g, String> {
        let number = self.index.len();
        self.index.insert(node, number);
        self.low.insert(node, number);
        self.open.push(node);
        self.on_open.insert(node);
        self.graph[node].iter()
    }

    /// A node already seen lowers `node` only while its component is open.
    fn lower(&mut self, node: &'g str, seen: &'g str) {
        if self.on_open.contains(seen) {
            let lowest = self.low[node].min(self.index[seen]);
            self.low.insert(node, lowest);
        }
    }

    /// Closes the component `node` roots, if it roots one.
    fn close(&mut self, node: &'g str) {
        if self.low[node] != self.index[node] {
            return;
        }
        let at = self.open.iter().rposition(|n| *n == node).unwrap_or(0);
        let closed: Vec<&str> = self.open.drain(at..).collect();
        for member in &closed {
            self.on_open.remove(member);
        }
        let mut component: Vec<String> = closed.into_iter().map(str::to_owned).collect();
        component.sort();
        self.components.push(component);
    }
}

/// Whether the check runs on the focused file.
fn applies(
    frame_rule: &str,
    when: Option<&Program>,
    builder: &Builder,
    ctx: &Ctx,
    options: &Map<String, Value>,
) -> Result<bool, PluginError> {
    let Some(when) = when else {
        return Ok(true);
    };
    let Some((file, text)) = ctx.file else {
        return Ok(false);
    };
    let base = library::context();
    let mut frame = Frame::new(frame_rule, &base);
    frame.set("file", &builder.file(file, text))?;
    frame.options(options)?;
    frame.truth(when)
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
    ctx: &KeyCtx,
    keys: &[&dyn OrderKey],
    symbols: &[&'a Symbol],
) -> Result<Vec<(&'a Symbol, Vec<u64>)>, PluginError> {
    let mut ranked = Vec::new();
    for symbol in symbols {
        let mut ranks = Vec::new();
        for (n, key) in keys.iter().enumerate() {
            match key.rank(ctx, symbol)? {
                Some(rank) => ranks.push(rank),
                None if n == 0 => break,
                None => ranks.push(u64::MAX),
            }
        }
        if ranks.len() == keys.len() {
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
    let fingerprint = Fingerprint::of(&meta.id, symbol.id.as_str(), snippet);
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

/// Tarjan's algorithm, without recursion so a long chain cannot overflow the
/// stack: the strongly connected components of `graph`, each sorted, in the
/// order of their first member.
fn strongly_connected(graph: &BTreeMap<String, BTreeSet<String>>) -> Vec<Vec<String>> {
    let mut walk = Walk {
        graph,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        open: Vec::new(),
        on_open: BTreeSet::new(),
        components: Vec::new(),
    };
    for root in graph.keys() {
        if !walk.index.contains_key(root.as_str()) {
            walk.from(root);
        }
    }
    walk.components.sort();
    walk.components
}
