use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use crate::{
    Durability,
    node::{Dep, Hash, Locked, NodeId},
};

/// The most frames a cycle search visits: a bound against a claim graph that
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

/// The claim graph around a frame: what it runs now, and whose claims its
/// threads wait for. Several threads may work for one frame (a fan-out), so
/// both are lists.
#[derive(Default)]
struct Link {
    children: Vec<Arc<Frame>>,
    waiting: Vec<Arc<Frame>>,
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

    pub(crate) fn add_child(&self, child: &Arc<Frame>) {
        self.link.locked().children.push(child.clone());
    }

    pub(crate) fn remove_child(&self, child: &Arc<Frame>) {
        remove(&mut self.link.locked().children, child);
    }

    pub(crate) fn add_waiting(&self, owner: &Arc<Frame>) {
        self.link.locked().waiting.push(owner.clone());
    }

    pub(crate) fn remove_waiting(&self, owner: &Arc<Frame>) {
        remove(&mut self.link.locked().waiting, owner);
    }

    /// What this frame depends on: the frames it runs and the owners of the
    /// claims its threads wait for.
    fn needs(&self) -> Vec<Arc<Frame>> {
        let link = self.link.locked();
        link.children.iter().chain(&link.waiting).cloned().collect()
    }

    fn chain(self: &Arc<Self>) -> Vec<Arc<Frame>> {
        let mut chain = vec![self.clone()];
        while let Some(parent) = chain.last().and_then(|frame| frame.parent.clone()) {
            chain.push(parent);
        }
        chain.reverse();
        chain
    }
}

fn remove(list: &mut Vec<Arc<Frame>>, frame: &Arc<Frame>) {
    if let Some(at) = list.iter().position(|found| Arc::ptr_eq(found, frame)) {
        list.swap_remove(at);
    }
}

/// The nodes of a cycle that waiting for a node claimed by `owner` would
/// close, if it would: some frame of the caller's own chain (the frames that
/// are waiting for the caller to finish) is reached from `owner` by following
/// what frames run and wait for, across threads and through fan-outs. Call
/// with the wait lock held so that two threads cannot each miss the other's
/// wait.
pub(crate) fn cycle(me: &Arc<Frame>, owner: &Arc<Frame>) -> Option<Vec<NodeId>> {
    let mine = me.chain();
    let at = |frame: &Arc<Frame>| mine.iter().position(|found| Arc::ptr_eq(found, frame));
    let mut came_from: HashMap<*const Frame, Arc<Frame>> = HashMap::new();
    let mut seen: HashSet<*const Frame> = HashSet::from([Arc::as_ptr(owner)]);
    let mut stack = vec![owner.clone()];
    while let Some(frame) = stack.pop() {
        if let Some(start) = at(&frame) {
            let mut route = vec![frame.id.clone()];
            let mut step = frame;
            while let Some(before) = came_from.get(&Arc::as_ptr(&step)) {
                route.push(before.id.clone());
                step = before.clone();
            }
            route.reverse();
            let mut path: Vec<NodeId> = mine[start..].iter().map(|f| f.id.clone()).collect();
            path.extend(route);
            return Some(path);
        }
        if seen.len() > MAX_HOPS {
            return None;
        }
        for next in frame.needs() {
            if seen.insert(Arc::as_ptr(&next)) {
                came_from.insert(Arc::as_ptr(&next), frame.clone());
                stack.push(next);
            }
        }
    }
    None
}
