//! How far a change travels through the project: the reach of every module
//! over the modules, and of every function and type over the symbols, after
//! MacCormack, Rusnak and Baldwin's visibility matrix and Martin's stable
//! dependencies. Production code only; generated code is left out.

use std::collections::BTreeSet;

use lighthouse_model::ucm::{EdgeKind, Node, Project, Symbol, SymbolId, SymbolKind, Target};

use super::graph::Graph;

/// The size from which the largest cycle is the core of a system.
const CORE_MIN: u32 = 2;

/// A graph of named nodes with what each one reaches.
#[derive(Debug, Clone, PartialEq)]
pub struct System {
    /// The names of the nodes, sorted.
    pub nodes: Vec<String>,
    /// What each node reaches, in the order of `nodes`.
    pub stability: Vec<Stability>,
    /// The share of node pairs where one reaches the other, a node counting
    /// as reaching itself.
    pub propagation_cost: f64,
    /// The share of the nodes in the core: the largest cycle.
    pub core_share: f64,
    /// The cycles of two nodes or more, their members sorted, in order.
    pub cycles: Vec<Vec<String>>,
}

impl System {
    /// What the node called `name` reaches; `None` when it is no node.
    pub fn of(&self, name: &str) -> Option<&Stability> {
        let at = self.nodes.binary_search_by(|n| n.as_str().cmp(name)).ok()?;
        self.stability.get(at)
    }
}

/// What one node reaches in its graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stability {
    /// Distinct nodes that use it directly.
    pub ca: u32,
    /// Distinct nodes it uses directly.
    pub ce: u32,
    /// Nodes that reach it, itself left out.
    pub vfi: u32,
    /// Nodes it reaches, itself left out.
    pub vfo: u32,
    /// The size of its cycle: 1 when it is on none.
    pub cyclic: u32,
    /// The smallest name of its cycle.
    pub component: String,
    /// Whether it is in the core of the system.
    pub core: bool,
}
/// The production modules, joined by the symbols of one that use a symbol of
/// the other and by imports.
pub fn modules(project: &Project) -> System {
    let nodes: BTreeSet<&str> = project
        .symbols
        .iter()
        .filter(|s| production(project, s))
        .map(|s| s.id.module())
        .filter(|m| project.module(m).is_some_and(|m| m.test_of.is_none()))
        .collect();
    let names: Vec<&str> = nodes.into_iter().collect();
    let at = |name: &str| names.binary_search(&name).ok();
    let used = project
        .symbols
        .iter()
        .filter(|s| production(project, s))
        .flat_map(|s| {
            let from = s.id.module();
            project
                .uses(&s.id)
                .iter()
                .map(move |id| (from, id.module()))
        });
    let imported = project
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .filter_map(|e| match (&e.from, &e.to) {
            (Node::Module(from), Target::Resolved(Node::Module(to))) => {
                Some((from.as_str(), to.as_str()))
            }
            _ => None,
        });
    let edges = used
        .chain(imported)
        .filter_map(|(from, to)| Some((at(from)? as u32, at(to)? as u32)));
    measure(&names, Graph::new(names.len(), edges))
}

/// The production functions, methods, types and interfaces, joined by what
/// each one uses.
pub fn symbols(project: &Project) -> System {
    let mut nodes: Vec<&Symbol> = project
        .symbols
        .iter()
        .filter(|s| production(project, s))
        .filter(|s| {
            matches!(
                s.kind,
                SymbolKind::Function
                    | SymbolKind::Method
                    | SymbolKind::Type
                    | SymbolKind::Interface
            )
        })
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let at = |id: &SymbolId| nodes.binary_search_by(|s| s.id.cmp(id)).ok();
    let edges = nodes.iter().enumerate().flat_map(|(from, s)| {
        project
            .uses(&s.id)
            .iter()
            .filter_map(at)
            .map(move |to| (from as u32, to as u32))
    });
    let names: Vec<&str> = nodes.iter().map(|s| s.id.as_str()).collect();
    measure(&names, Graph::new(nodes.len(), edges))
}

/// Whether the symbol is code that ships and a person wrote.
fn production(project: &Project, symbol: &Symbol) -> bool {
    !project.in_test(&symbol.id) && !project.file(&symbol.file).is_some_and(|f| f.generated)
}

fn measure(names: &[&str], graph: Graph) -> System {
    let reached = graph.reached();
    let n = names.len();
    // The smallest member of each cycle: the nodes are sorted by name.
    let mut smallest = vec![n; reached.sizes.len()];
    for (v, &c) in reached.scc.iter().enumerate().rev() {
        smallest[c as usize] = v;
    }
    let core = reached
        .sizes
        .iter()
        .enumerate()
        .filter(|&(_, &size)| size >= CORE_MIN)
        .max_by_key(|&(c, &size)| (size, std::cmp::Reverse(smallest[c])))
        .map(|(c, _)| c);
    let stability: Vec<Stability> = (0..n)
        .map(|v| {
            let c = reached.scc[v] as usize;
            Stability {
                ca: reached.ca[v],
                ce: reached.ce[v],
                vfi: reached.vfi[v],
                vfo: reached.vfo[v],
                cyclic: reached.sizes[c],
                component: names[smallest[c]].to_owned(),
                core: core == Some(c),
            }
        })
        .collect();
    let cycles: Vec<Vec<String>> = graph
        .cycles()
        .into_iter()
        .map(|cycle| {
            cycle
                .iter()
                .map(|&v| names[v as usize].to_owned())
                .collect()
        })
        .collect();
    let share = |count: usize| if n == 0 { 0.0 } else { count as f64 / n as f64 };
    let reach: u64 = reached.vfo.iter().map(|&v| u64::from(v)).sum();
    System {
        nodes: names.iter().map(|&s| s.to_owned()).collect(),
        propagation_cost: if n == 0 {
            0.0
        } else {
            (reach + n as u64) as f64 / (n * n) as f64
        },
        core_share: share(stability.iter().filter(|s| s.core).count()),
        stability,
        cycles,
    }
}
