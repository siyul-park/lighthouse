//! The `cycle` operation of a `builtin` check: no dependency cycle over edges
//! of one kind. It judges what an expression over one value cannot, the shape
//! of the dependency graph, and knows nothing about any decision.

use std::collections::BTreeSet;

use lighthouse_model::{
    Diagnostic,
    ucm::{EdgeKind, Node, Project, Symbol, SymbolId, Target},
};
use lighthouse_plugin::{Context, Error as PluginError, RuleManifest};
use lighthouse_spec::CycleLevel;
use serde_json::json;

use crate::{Error, metrics::graph::Graph};

/// `cycle`: no dependency cycle over edges of one kind.
pub(crate) struct CycleRule {
    edge: EdgeKind,
    level: CycleLevel,
}

impl CycleRule {
    pub(crate) fn new(id: &str, edge: &str, level: CycleLevel) -> Result<Self, Error> {
        Ok(Self {
            edge: edge
                .parse()
                .map_err(|_| Error::invalid(id, format!("`{edge}` is not an edge kind")))?,
            level,
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
