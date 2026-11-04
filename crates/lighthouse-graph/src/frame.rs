use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use crate::{
    Durability,
    node::{Dep, Hash, Locked, NodeId},
};

/// The most waits a cycle search follows: a bound against a wait chain that
/// is already broken.
const MAX_HOPS: usize = 100_000;

/// One execution in progress: its place in the chain of queries that called
/// it, what it has read, and what it waits for.
pub(crate) struct Frame {
    pub id: NodeId,
    parent: Option<Arc<Frame>>,
    reads: Mutex<Reads>,
    link: Mutex<Link>,
}

struct Reads {
    list: Vec<Dep>,
    seen: HashSet<NodeId>,
    durability: Durability,
}

#[derive(Default)]
struct Link {
    child: Option<Arc<Frame>>,
    waiting: Option<Arc<Frame>>,
}

impl Frame {
    pub(crate) fn new(id: NodeId, parent: Option<Arc<Frame>>) -> Arc<Self> {
        let reads = Reads {
            list: Vec::new(),
            seen: HashSet::new(),
            durability: Durability::Static,
        };
        Arc::new(Self {
            id,
            parent,
            reads: Mutex::new(reads),
            link: Mutex::default(),
        })
    }

    /// Records a read, once per node.
    pub(crate) fn record(&self, id: &NodeId, hash: Hash, durability: Durability) {
        let mut reads = self.reads.locked();
        reads.durability = reads.durability.min(durability);
        if reads.seen.insert(id.clone()) {
            reads.list.push(Dep {
                id: id.clone(),
                hash,
            });
        }
    }

    /// What was read, and the lowest durability among it.
    pub(crate) fn take_reads(&self) -> (Vec<Dep>, Durability) {
        let mut reads = self.reads.locked();
        (std::mem::take(&mut reads.list), reads.durability)
    }

    pub(crate) fn set_child(&self, child: Option<Arc<Frame>>) {
        self.link.locked().child = child;
    }

    pub(crate) fn set_waiting(&self, owner: Option<Arc<Frame>>) {
        self.link.locked().waiting = owner;
    }

    fn chain(self: &Arc<Self>) -> Vec<Arc<Frame>> {
        let mut chain = vec![self.clone()];
        while let Some(parent) = chain.last().and_then(|frame| frame.parent.clone()) {
            chain.push(parent);
        }
        chain.reverse();
        chain
    }

    fn leaf(self: &Arc<Self>) -> Arc<Frame> {
        let mut leaf = self.clone();
        loop {
            let child = leaf.link.locked().child.clone();
            match child {
                Some(child) => leaf = child,
                None => return leaf,
            }
        }
    }
}

/// The nodes of a cycle that waiting for `wanted`, claimed by `owner`, would
/// close, if it would: `owner` is a frame of the caller's own chain, or what
/// `owner` runs is blocked, hop by hop, on a frame of that chain. Call with
/// the wait lock held so that two threads cannot each miss the other's wait.
pub(crate) fn cycle(me: &Arc<Frame>, owner: &Arc<Frame>, wanted: &NodeId) -> Option<Vec<NodeId>> {
    let mine = me.chain();
    let mut tail = vec![wanted.clone()];
    let mut current = owner.clone();
    for _ in 0..MAX_HOPS {
        if let Some(at) = mine.iter().position(|frame| Arc::ptr_eq(frame, &current)) {
            let mut path: Vec<NodeId> = mine[at..].iter().map(|frame| frame.id.clone()).collect();
            path.append(&mut tail);
            return Some(path);
        }
        let leaf = current.leaf();
        let next = leaf.link.locked().waiting.clone()?;
        tail.push(leaf.id.clone());
        current = next;
    }
    None
}
