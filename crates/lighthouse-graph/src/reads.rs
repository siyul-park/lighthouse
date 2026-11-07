use std::{collections::HashSet, sync::atomic::Ordering};

use crate::{
    Error, Graph, Input, Key, Query, Revision, Value,
    node::{Locked, NodeId},
    query::{decode, encode},
};

/// The nodes a node read, directly or through what it read.
#[derive(Debug, Clone, Default)]
pub struct Reads {
    nodes: Vec<NodeId>,
}

impl Reads {
    /// The keys of the nodes of the kind named `kind`.
    pub fn keys<K: Key>(&self, kind: &str) -> Vec<K> {
        self.nodes
            .iter()
            .filter(|id| &*id.kind == kind)
            .filter_map(|id| decode(&id.key).ok())
            .collect()
    }
}

/// The revision the graph is at.
pub fn revision(graph: &Graph) -> Revision {
    Revision(graph.shared.revision.load(Ordering::Acquire))
}

/// The keys of the inputs of a kind that were set or stored.
pub fn keys<K: Key, V: Value>(graph: &Graph, input: &'static Input<K, V>) -> Vec<K> {
    let slots = graph
        .shared
        .slots
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    slots
        .keys()
        .filter(|id| &*id.kind == input.name())
        .filter_map(|id| decode(&id.key).ok())
        .collect()
}

/// What the node of `query` for `key` read, as its last run recorded it;
/// `None` when the graph has no such node. With `moved` as `(since,
/// until)` only the nodes that last changed after `since` and by `until`
/// are followed: what the node owes its value at `until` to, as against
/// its value at `since`. The kinds named in `except` are not followed.
///
/// # Errors
///
/// The store could not be read.
pub fn reads<K: Key, V: Value>(
    graph: &Graph,
    query: &'static Query<K, V>,
    key: &K,
    (moved, except): (Option<(Revision, Revision)>, &[&str]),
) -> Result<Option<Reads>, Error> {
    let registered = graph.shared.register(query)?;
    let root = NodeId {
        kind: registered.name,
        key: encode(key)?.into(),
    };
    let mut seen = HashSet::from([root.clone()]);
    let mut found = Vec::new();
    let mut todo = vec![root.clone()];
    while let Some(id) = todo.pop() {
        let slot = graph.shared.slot(&id);
        let deps = {
            let mut node = slot.node.locked();
            graph.shared.load(&mut node, &id)?;
            if !node.present {
                if id == root {
                    return Ok(None);
                }
                continue;
            }
            node.deps.clone()
        };
        for dep in deps.iter() {
            if except.contains(&&*dep.id.kind) || !seen.insert(dep.id.clone()) {
                continue;
            }
            if let Some((since, until)) = moved {
                let Some(slot) = graph.shared.slot_if_known(&dep.id) else {
                    continue;
                };
                let at = Revision(slot.node.locked().changed_at);
                if at <= since || until < at {
                    continue;
                }
            }
            found.push(dep.id.clone());
            todo.push(dep.id.clone());
        }
    }
    Ok(Some(Reads { nodes: found }))
}
