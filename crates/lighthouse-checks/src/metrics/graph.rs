//! Reachability over a directed graph of dense `u32` node indexes: strongly
//! connected components (Tarjan, iterative, for graphs as deep as the symbols
//! of a project) and the transitive closure of the condensation.

/// Not visited yet, not in a component yet.
const NONE: u32 = u32::MAX;

/// A directed graph without parallel edges or self-loops.
pub(crate) struct Graph {
    succ: Vec<Vec<u32>>,
}

/// What a node reaches, per node.
pub(crate) struct Reached {
    /// Distinct direct successors.
    pub ce: Vec<u32>,
    /// Distinct direct predecessors.
    pub ca: Vec<u32>,
    /// Nodes reachable from it, itself left out, the rest of its component in.
    pub vfo: Vec<u32>,
    /// Nodes that reach it, with the same convention.
    pub vfi: Vec<u32>,
    /// Its component, numbered sinks first.
    pub scc: Vec<u32>,
    /// The number of nodes of each component.
    pub sizes: Vec<u32>,
}

impl Graph {
    pub(crate) fn new(nodes: usize, edges: impl IntoIterator<Item = (u32, u32)>) -> Self {
        let mut succ = vec![Vec::new(); nodes];
        for (from, to) in edges.into_iter().filter(|(from, to)| from != to) {
            succ[from as usize].push(to);
        }
        for list in &mut succ {
            list.sort_unstable();
            list.dedup();
        }
        Self { succ }
    }

    pub(crate) fn reached(&self) -> Reached {
        let back = self.reversed();
        let (scc, count) = self.components();
        let mut sizes = vec![0; count];
        for &c in &scc {
            sizes[c as usize] += 1;
        }
        let (back_scc, back_count) = back.components();
        Reached {
            ce: self.succ.iter().map(|s| s.len() as u32).collect(),
            ca: back.succ.iter().map(|s| s.len() as u32).collect(),
            vfo: self.closure(&scc, count),
            vfi: back.closure(&back_scc, back_count),
            scc,
            sizes,
        }
    }

    /// The cycles: the components of two nodes or more, each with its nodes
    /// ascending, in the order of their first nodes.
    pub(crate) fn cycles(&self) -> Vec<Vec<u32>> {
        let (scc, count) = self.components();
        let mut members = vec![Vec::new(); count];
        for (v, &c) in (0..).zip(&scc) {
            members[c as usize].push(v);
        }
        members.retain(|m| m.len() > 1);
        members.sort();
        members
    }

    fn reversed(&self) -> Self {
        let edges = (0..)
            .zip(&self.succ)
            .flat_map(|(from, list)| list.iter().map(move |&to| (to, from)));
        Self::new(self.succ.len(), edges)
    }

    /// The component of each node and the number of components, numbered in
    /// the order Tarjan completes them: every edge leaves a component for a
    /// lower one.
    fn components(&self) -> (Vec<u32>, usize) {
        let mut walk = Walk::new(&self.succ);
        for root in 0..self.succ.len() as u32 {
            if walk.index[root as usize] == NONE {
                walk.from(root);
            }
        }
        (walk.scc, walk.count as usize)
    }

    /// How many nodes each node reaches, itself left out: a bitset per
    /// component, the union of its members and of the components they lead
    /// to, which are all numbered lower.
    fn closure(&self, scc: &[u32], count: usize) -> Vec<u32> {
        let words = self.succ.len().div_ceil(64);
        let mut members = vec![Vec::new(); count];
        for (v, &c) in scc.iter().enumerate() {
            members[c as usize].push(v);
        }
        let mut sets: Vec<Vec<u64>> = Vec::with_capacity(count);
        for (c, nodes) in members.iter().enumerate() {
            let mut set = vec![0u64; words];
            for &v in nodes {
                set[v / 64] |= 1 << (v % 64);
            }
            let leads = nodes
                .iter()
                .flat_map(|&v| &self.succ[v])
                .map(|&w| scc[w as usize] as usize)
                .filter(|&d| d != c);
            for d in leads {
                set.iter_mut().zip(&sets[d]).for_each(|(a, b)| *a |= b);
            }
            sets.push(set);
        }
        let sizes: Vec<u32> = sets
            .iter()
            .map(|set| set.iter().map(|w| w.count_ones()).sum::<u32>() - 1)
            .collect();
        scc.iter().map(|&c| sizes[c as usize]).collect()
    }
}

/// Tarjan's walk, with the stack of calls kept apart so that its depth is not
/// that of the graph.
struct Walk<'g> {
    succ: &'g [Vec<u32>],
    index: Vec<u32>,
    low: Vec<u32>,
    scc: Vec<u32>,
    open: Vec<u32>,
    next: u32,
    count: u32,
}

impl<'g> Walk<'g> {
    fn new(succ: &'g [Vec<u32>]) -> Self {
        let n = succ.len();
        Self {
            succ,
            index: vec![NONE; n],
            low: vec![0; n],
            scc: vec![NONE; n],
            open: Vec::new(),
            next: 0,
            count: 0,
        }
    }

    /// Visits what `root` leads to, completing components as it goes.
    fn from(&mut self, root: u32) {
        self.enter(root);
        let mut calls = vec![(root, 0)];
        while let Some(frame) = calls.last_mut() {
            let (v, at) = *frame;
            frame.1 += 1;
            match self.succ[v as usize].get(at) {
                Some(&w) if self.index[w as usize] == NONE => {
                    self.enter(w);
                    calls.push((w, 0));
                }
                Some(&w) if self.scc[w as usize] == NONE => {
                    self.low[v as usize] = self.low[v as usize].min(self.index[w as usize]);
                }
                Some(_) => {}
                None => {
                    calls.pop();
                    self.leave(v);
                    if let Some(&(parent, _)) = calls.last() {
                        self.low[parent as usize] =
                            self.low[parent as usize].min(self.low[v as usize]);
                    }
                }
            }
        }
    }

    fn enter(&mut self, v: u32) {
        self.index[v as usize] = self.next;
        self.low[v as usize] = self.next;
        self.next += 1;
        self.open.push(v);
    }

    /// Completes the component of `v` when it is the first of it visited.
    fn leave(&mut self, v: u32) {
        if self.low[v as usize] != self.index[v as usize] {
            return;
        }
        while let Some(w) = self.open.pop() {
            self.scc[w as usize] = self.count;
            if w == v {
                break;
            }
        }
        self.count += 1;
    }
}
