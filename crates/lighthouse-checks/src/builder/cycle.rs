//! The `cycle` field of a module: the modules that depend on each other in a
//! circle with it.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::ucm::{EdgeKind, Model, Node, Target};
use serde_json::{Value, json};

use super::Builder;
use lighthouse_model::graph::Graph;

impl Builder<'_> {
    /// The paths of the modules in the strongly connected component of the
    /// import graph that holds `module`, itself included and ascending; empty
    /// when the module is on no cycle (a component of one module).
    pub(super) fn module_cycle(&self, module: &str) -> Value {
        json!(self.indexes.cycle(module))
    }
}

/// The component of every module that is on a cycle of import edges.
pub fn cycles(project: &dyn Model) -> BTreeMap<String, Vec<String>> {
    let unit = |node: &Node| match node {
        Node::Module(path) => path.clone(),
        Node::Symbol(id) => id.module().to_owned(),
    };
    let edges: Vec<(String, String)> = project
        .edges()
        .filter(|e| e.kind == EdgeKind::Imports)
        .filter_map(|edge| match &edge.to {
            Target::Resolved(to) => Some((unit(&edge.from), unit(to))),
            Target::Path(_) => None,
        })
        .collect();
    let units: Vec<&String> = edges
        .iter()
        .flat_map(|(from, to)| [from, to])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let at = |unit: &String| u32::try_from(units.binary_search(&unit).ok()?).ok();
    let graph = Graph::new(
        units.len(),
        edges
            .iter()
            .filter_map(|(from, to)| Some((at(from)?, at(to)?))),
    );
    let mut found = BTreeMap::new();
    for component in graph.cycles() {
        let members: Vec<String> = component
            .iter()
            .map(|&v| units[v as usize].clone())
            .collect();
        for member in &members {
            found.insert(member.clone(), members.clone());
        }
    }
    found
}
