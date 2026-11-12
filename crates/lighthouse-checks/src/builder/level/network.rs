//! The minimum cut of one round of the solver: the sets of variables whose
//! raise by one changes the cost, as a network whose cut is that change.
//!
//! Only the variables the source reaches through positive capacities can be
//! raised, so the network is pruned to them and cut one weakly connected
//! component at a time.

use std::collections::VecDeque;

use petgraph::{Graph, algo::dinics, graph::NodeIndex};

use super::solve::Problem;

/// A capacity no finite cut reaches.
const INFINITE: u64 = u64::MAX / 4;

/// The network of raising some variables of a problem by one, from given
/// levels: a variable on the source side of a cut is raised. A cut costs the
/// change of the cost less `constant`.
pub(super) struct Network {
    vars: usize,
    /// `(from, to, capacity)` between variables.
    arcs: Vec<(usize, usize, u64)>,
    /// What the source gives each variable: the gain of raising it alone.
    gain: Vec<u64>,
    /// What each variable gives the sink: the cost of raising it alone, or
    /// infinite when it is frozen.
    cost: Vec<u64>,
}

impl Network {
    /// The network of `problem` at `levels`.
    pub(super) fn of(problem: &Problem, levels: &[i64]) -> Self {
        let vars = problem.slope.len();
        let mut network = Self {
            vars,
            arcs: Vec::with_capacity(problem.arcs.len() * 2),
            gain: vec![0; vars],
            cost: vec![0; vars],
        };
        let mut unary = problem.slope.clone();
        for &(a, b, term) in &problem.arcs {
            let (up, down) = term.moves(levels[a] - levels[b]);
            match down {
                // Raising a alone costs `up`, b alone `down`:
                // up [a up, b not] + down [b up, a not]
                //   = (up + down) [a up, b not] - down x_a + down x_b.
                Some(down) => {
                    network.arc(a, b, up + down);
                    unary[a] -= down;
                    unary[b] += down;
                }
                None => {
                    network.arcs.push((b, a, INFINITE));
                    network.arc(a, b, up);
                }
            }
        }
        for (v, &c) in unary.iter().enumerate() {
            if problem.frozen[v] {
                network.cost[v] = INFINITE;
            } else if c > 0 {
                network.cost[v] = c as u64;
            } else if c < 0 {
                network.gain[v] = (-c) as u64;
            }
        }
        network
    }

    /// The inclusion-minimal set whose raise lowers the cost most, or `None`
    /// when no raise lowers it.
    pub(super) fn raise(&self) -> Option<Vec<bool>> {
        let (groups, index) = self.components(&self.reached());
        let up: Vec<usize> = groups
            .iter()
            .enumerate()
            .flat_map(|(id, members)| self.cut(members, &index, id))
            .collect();
        if up.is_empty() {
            return None;
        }
        let mut raise = vec![false; self.vars];
        up.into_iter().for_each(|v| raise[v] = true);
        Some(raise)
    }

    /// The variables the source reaches through positive capacities.
    fn reached(&self) -> Vec<bool> {
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); self.vars];
        for &(a, b, _) in &self.arcs {
            out[a].push(b);
        }
        let seeds: Vec<usize> = (0..self.vars).filter(|&v| self.gain[v] > 0).collect();
        flood(&out, &seeds)
    }

    /// The weakly connected components of the reached variables, by the arcs
    /// between them: the members of each, and the component of each variable.
    fn components(&self, reached: &[bool]) -> (Vec<Vec<usize>>, Vec<Option<usize>>) {
        let mut parent: Vec<usize> = (0..self.vars).collect();
        for &(a, b, _) in &self.arcs {
            if reached[a] && reached[b] {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut index = vec![None; self.vars];
        let mut of_root: Vec<Option<usize>> = vec![None; self.vars];
        for v in (0..self.vars).filter(|&v| reached[v]) {
            let r = root(&mut parent, v);
            let id = *of_root[r].get_or_insert(groups.len());
            if id == groups.len() {
                groups.push(Vec::new());
            }
            groups[id].push(v);
            index[v] = Some(id);
        }
        (groups, index)
    }

    /// The source side nearest the source of the minimum cut of one
    /// component, empty when the cut does not lower the cost.
    fn cut(&self, members: &[usize], component: &[Option<usize>], id: usize) -> Vec<usize> {
        let local = |v: usize| {
            members
                .binary_search(&v)
                .expect("a member of its component")
        };
        let (source, sink) = (members.len(), members.len() + 1);
        let mut graph = Graph::<(), u64>::new();
        let nodes: Vec<NodeIndex> = (0..members.len() + 2).map(|_| graph.add_node(())).collect();
        let mut arcs: Vec<(usize, usize, u64)> = Vec::new();
        for (at, &v) in members.iter().enumerate() {
            if self.gain[v] > 0 {
                arcs.push((source, at, self.gain[v]));
            }
            if self.cost[v] > 0 {
                arcs.push((at, sink, self.cost[v]));
            }
        }
        for &(a, b, cap) in &self.arcs {
            if component[a] == Some(id) && component[b] == Some(id) {
                arcs.push((local(a), local(b), cap));
            }
        }
        arcs.sort_unstable_by_key(|&(a, b, _)| (a, b));
        arcs.dedup_by(|next, kept| {
            let same = (next.0, next.1) == (kept.0, kept.1);
            if same {
                kept.2 = kept.2.saturating_add(next.2).min(INFINITE);
            }
            same
        });
        for &(a, b, cap) in &arcs {
            graph.add_edge(nodes[a], nodes[b], cap);
        }
        let given: u64 = members.iter().map(|&v| self.gain[v]).sum();
        let (cut, flows) = dinics(&graph, nodes[source], nodes[sink]);
        if cut >= given {
            return Vec::new();
        }
        let mut residual: Vec<Vec<usize>> = vec![Vec::new(); members.len() + 2];
        for (at, &(a, b, cap)) in arcs.iter().enumerate() {
            if flows[at] < cap {
                residual[a].push(b);
            }
            if flows[at] > 0 {
                residual[b].push(a);
            }
        }
        let seen = flood(&residual, &[source]);
        (0..members.len())
            .filter(|&at| seen[at])
            .map(|at| members[at])
            .collect()
    }

    fn arc(&mut self, from: usize, to: usize, cap: i64) {
        if cap > 0 {
            self.arcs.push((from, to, cap as u64));
        }
    }
}

/// The representative of the set of `v`, compressing the path.
fn root(parent: &mut [usize], mut v: usize) -> usize {
    while parent[v] != v {
        parent[v] = parent[parent[v]];
        v = parent[v];
    }
    v
}

/// What the `seeds` reach along `next`, themselves included.
fn flood(next: &[Vec<usize>], seeds: &[usize]) -> Vec<bool> {
    let mut seen = vec![false; next.len()];
    let mut queue = VecDeque::new();
    for &v in seeds {
        seen[v] = true;
        queue.push_back(v);
    }
    while let Some(v) = queue.pop_front() {
        for &to in &next[v] {
            if !seen[to] {
                seen[to] = true;
                queue.push_back(to);
            }
        }
    }
    seen
}
