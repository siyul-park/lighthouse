//! The `cycle` field of a module: the modules that depend on each other in a
//! circle with it.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::ucm::{EdgeKind, Node, Project, Target};
use serde_json::{Value, json};

use super::Builder;
use crate::metrics::graph::Graph;

impl Builder<'_> {
    /// The paths of the modules in the strongly connected component of the
    /// import graph that holds `module`, itself included and ascending; empty
    /// when the module is on no cycle (a component of one module).
    pub(super) fn module_cycle(&self, module: &str) -> Value {
        let cycles = self.indexes.cycles.get_or_init(|| index(self.project));
        json!(cycles.get(module).map_or(&[][..], Vec::as_slice))
    }
}

/// The component of every module that is on a cycle of import edges.
fn index(project: &Project) -> BTreeMap<String, Vec<String>> {
    let unit = |node: &Node| match node {
        Node::Module(path) => path.clone(),
        Node::Symbol(id) => id.module().to_owned(),
    };
    let edges: Vec<(String, String)> = project
        .edges
        .iter()
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
