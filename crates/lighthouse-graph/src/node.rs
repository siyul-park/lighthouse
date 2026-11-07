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
    /// The stored traces `traces` no longer has, by row.
    pub traces_lost: Vec<i64>,
}

/// An earlier state of a node: its value's hash, what it read, and where the
/// bytes of the value are.
#[derive(Debug, Clone)]
pub(crate) struct Trace {
    pub hash: Hash,
    pub deps: Arc<[Dep]>,
    pub durability: Durability,
    pub bytes: Bytes,
}

/// Where the bytes of a retained value are: in memory only, in the store with
/// no row of the trace yet, or in the store with its row.
#[derive(Debug, Clone)]
pub(crate) enum Bytes {
    Memory(Arc<[u8]>),
    Disk,
    Stored(i64),
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
            traces_lost: Vec::new(),
        }
    }
}

impl Node {
    /// Keeps the current state as the newest trace, when its bytes are in the
    /// store or in memory, and drops the oldest past [`RETAINED`].
    pub(crate) fn retain(&mut self) {
        let bytes = match self.blob.clone() {
            _ if !self.present => return,
            _ if self.on_disk && !self.value_dirty => Bytes::Disk,
            Some(blob) => Bytes::Memory(blob),
            None => return,
        };
        self.forget_trace(self.hash, &self.deps.clone());
        let trace = Trace {
            hash: self.hash,
            deps: self.deps.clone(),
            durability: self.durability,
            bytes,
        };
        self.traces.insert(0, trace);
        for old in self.traces.split_off(RETAINED.min(self.traces.len())) {
            self.lose(&old);
        }
        self.traces_dirty = true;
    }

    /// Drops the traces of this value read from these dependencies: the node
    /// has that as its current state.
    pub(crate) fn forget_trace(&mut self, hash: Hash, deps: &[Dep]) {
        let (gone, kept) = std::mem::take(&mut self.traces)
            .into_iter()
            .partition(|trace| trace.hash == hash && *trace.deps == *deps);
        self.traces = kept;
        self.traces_dirty |= !gone.is_empty();
        gone.iter().for_each(|trace| self.lose(trace));
    }

    fn lose(&mut self, trace: &Trace) {
        if let Bytes::Stored(row) = trace.bytes {
            self.traces_lost.push(row);
        }
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
