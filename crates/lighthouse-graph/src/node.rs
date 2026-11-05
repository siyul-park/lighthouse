use std::{
    any::Any,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
};

use crate::{Durability, frame::Frame};

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
}

/// A node that was read, with the hash seen.
#[derive(Debug, Clone)]
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
        }
    }
}

impl Node {
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
