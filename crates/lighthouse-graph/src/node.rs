use std::{
    any::Any,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
};

use crate::{Durability, frame::Frame};

/// How many earlier states a node keeps besides the current one.
pub const RETAINED: usize = 3;

/// A node and the signal that a claim on it was released.
pub(crate) struct Slot {
    pub id: NodeId,
    pub node: Mutex<Node>,
    pub ready: Condvar,
}

impl Slot {
    pub(crate) fn new(id: NodeId) -> Self {
        Self {
            id,
            node: Mutex::new(Node::new()),
            ready: Condvar::new(),
        }
    }
}

/// One stored result. Fields read from the disk are filled on first touch.
pub(crate) struct Node {
    pub loaded: bool,
    pub present: bool,
    pub input: bool,
    pub persisted: bool,
    pub hash: Hash,
    pub deps: Arc<[Dep]>,
    pub changed_at: u64,
    pub verified_at: u64,
    /// The `verified_at` the store has: a node found green is written again
    /// only when that is old, as nothing that read it can have moved.
    pub stored_verified_at: u64,
    pub durability: Durability,
    pub used_at: u64,
    pub synced_used_at: u64,
    pub value: Option<Arc<dyn Any + Send + Sync>>,
    pub blob: Option<Arc<[u8]>>,
    pub on_disk: bool,
    pub state: State,
    pub dirty: bool,
    pub value_dirty: bool,
    pub cause: Cause,
    /// Earlier states, newest first, none that is the current one.
    pub traces: Vec<Trace>,
    /// The store has traces of the node that are not in `traces` yet.
    pub traces_stored: bool,
    pub traces_dirty: bool,
}

/// An earlier state of a node: its value's hash and bytes, and what it read.
#[derive(Debug, Clone)]
pub(crate) struct Trace {
    pub hash: Hash,
    pub deps: Arc<[Dep]>,
    pub durability: Durability,
    pub blob: Arc<[u8]>,
}

/// A node that was read, with the hash seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dep {
    pub id: NodeId,
    pub hash: Hash,
}

/// Whether some thread is validating or running the node.
pub(crate) enum State {
    Idle,
    Running(Arc<Frame>),
}

/// Why the node was last run, or that it was not.
#[derive(Debug, Clone)]
pub(crate) enum Cause {
    Computed,
    Evicted,
    DepMoved(NodeId),
    UnknownDep(NodeId),
    InputChanged,
    Retained,
    Green,
}

/// The identity of a node: its kind and its encoded key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NodeId {
    pub kind: Arc<str>,
    pub key: Arc<[u8]>,
}

pub(crate) type Hash = [u8; 32];

impl Node {
    pub(crate) fn new() -> Self {
        Self {
            loaded: false,
            present: false,
            input: false,
            persisted: false,
            hash: [0; 32],
            deps: Arc::from([]),
            changed_at: 0,
            verified_at: 0,
            stored_verified_at: 0,
            durability: Durability::Static,
            used_at: 0,
            synced_used_at: 0,
            value: None,
            blob: None,
            on_disk: false,
            state: State::Idle,
            dirty: false,
            value_dirty: false,
            cause: Cause::Computed,
            traces: Vec::new(),
            traces_stored: false,
            traces_dirty: false,
        }
    }
}

impl Node {
    /// Keeps the current state as the newest trace, when it has bytes to give
    /// back, and drops the oldest past [`RETAINED`].
    pub(crate) fn retain(&mut self) {
        let Some(blob) = self.blob.clone().filter(|_| self.present) else {
            return;
        };
        self.forget_trace(self.hash, &self.deps.clone());
        self.traces.insert(
            0,
            Trace {
                hash: self.hash,
                deps: self.deps.clone(),
                durability: self.durability,
                blob,
            },
        );
        self.traces.truncate(RETAINED);
        self.traces_dirty = true;
    }

    /// Drops the traces of this value read from these dependencies: the node
    /// has that as its current state.
    pub(crate) fn forget_trace(&mut self, hash: Hash, deps: &[Dep]) {
        let before = self.traces.len();
        self.traces
            .retain(|trace| !(trace.hash == hash && *trace.deps == *deps));
        self.traces_dirty |= self.traces.len() != before;
    }

    /// The node as an input just set to a value of this hash.
    pub(crate) fn become_input(
        &mut self,
        hash: Hash,
        durability: Durability,
        revision: u64,
        persisted: bool,
    ) {
        self.present = true;
        self.input = true;
        self.persisted = persisted;
        self.hash = hash;
        self.deps = Arc::from([]);
        self.durability = durability;
        self.changed_at = revision;
        self.verified_at = revision;
        self.used_at = crate::shared::now();
        self.value_dirty = true;
        self.cause = Cause::InputChanged;
    }
}

/// Locks that a panicking query does not poison: the data is plain state.
pub(crate) trait Locked<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
