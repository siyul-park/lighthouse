use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    Durability, Error,
    dirty::Dirty,
    node::{Bytes, Dep, Hash, Locked, Node, NodeId, Slot, Trace},
    query::{Kind, decode, encode, hash},
    retained::{Changes, TraceRow},
    store::{Meta, Row, Store, Write},
};

/// How many nodes one task frees when the graph is dropped.
const FREED_TOGETHER: usize = 4096;

/// What every handle shares.
pub(crate) struct Shared {
    pub revision: AtomicU64,
    pub changed: [AtomicU64; Durability::COUNT],
    /// The revision at each of the last commits, oldest first: what a count of
    /// commits is worth in revisions.
    pub marks: Mutex<Vec<u64>>,
    pub kinds: RwLock<HashMap<&'static str, Registered>>,
    pub slots: RwLock<HashMap<NodeId, Arc<Slot>>>,
    pub store: Store,
    pub dirty: Dirty,
    pub executed: Mutex<Vec<NodeId>>,
    pub waits: Mutex<()>,
    pub verify_period: AtomicU64,
    pub verify_count: AtomicU64,
    pub versions: Mutex<Versions>,
    /// How many commits a row that nothing verified is kept.
    pub retain: u64,
    /// The kinds that live and die with another, see `Options::owned`.
    pub owned: Vec<(&'static str, &'static str)>,
    /// The graph's user data, fixed at open.
    pub data: Option<Arc<dyn Any + Send + Sync>>,
}

impl Drop for Shared {
    /// Frees the nodes on the threads of the pool: a graph holds what a run
    /// read, and one thread freeing all of it is what the process waits for
    /// at its exit.
    fn drop(&mut self) {
        let slots = std::mem::take(
            self.slots
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut all: Vec<Arc<Slot>> = slots.into_values().collect();
        rayon::scope(|scope| {
            while !all.is_empty() {
                let rest = all.split_off(all.len().saturating_sub(FREED_TOGETHER));
                scope.spawn(move |_| drop(rest));
            }
        });
    }
}

impl Shared {
    pub(crate) fn register(&self, kind: &'static dyn Kind) -> Result<Registered, Error> {
        let name = kind.name();
        if let Some(found) = self
            .kinds
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
        {
            return found.agree(kind);
        }
        let mut kinds = self
            .kinds
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        kinds
            .entry(name)
            .or_insert_with(|| Registered {
                kind,
                name: Arc::from(name),
            })
            .agree(kind)
    }

    pub(crate) fn registered(&self, name: &str) -> Option<&'static dyn Kind> {
        self.kinds
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .map(|found| found.kind)
    }

    pub(crate) fn slot(&self, id: &NodeId) -> Arc<Slot> {
        let slots = self
            .slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.get(id) {
            return slot.clone();
        }
        drop(slots);
        let mut slots = self
            .slots
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots
            .entry(id.clone())
            .or_insert_with(|| Arc::new(Slot::new(id.clone())))
            .clone()
    }

    pub(crate) fn slot_if_known(&self, id: &NodeId) -> Option<Arc<Slot>> {
        self.slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// `kind(key)` of a node, for messages.
    pub(crate) fn label(&self, id: &NodeId) -> String {
        match self.registered(&id.kind) {
            Some(kind) => kind.describe(&id.key),
            None => format!("{}(?)", id.kind),
        }
    }

    /// Reads the stored row into the node on first touch.
    pub(crate) fn load(&self, node: &mut Node, id: &NodeId) -> Result<(), Error> {
        if node.loaded {
            return Ok(());
        }
        if let Some(row) = self.store.load(&id.kind, &id.key)? {
            self.fill(node, row);
        }
        node.loaded = true;
        Ok(())
    }

    fn fill(&self, node: &mut Node, row: Row) {
        let Some(deps) = self.deps_of(&row.deps) else {
            return;
        };
        node.present = true;
        node.input = row.input;
        node.hash = row.hash;
        node.deps = deps;
        node.changed_at = row.changed_at;
        node.verified_at = row.verified_at;
        node.stored_verified_at = row.verified_at;
        node.durability = row.durability;
        node.used_at = row.used_at;
        node.synced_used_at = row.used_at;
        node.on_disk = row.has_value;
        node.traces_stored = row.has_traces;
    }

    /// The dependencies as stored, their kinds named as this process has them.
    fn deps_of(&self, bytes: &[u8]) -> Option<Arc<[Dep]>> {
        let deps = decode::<Vec<(String, Vec<u8>, Hash)>>(bytes).ok()?;
        let kinds = self
            .kinds
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(
            deps.into_iter()
                .map(|(kind, key, hash)| Dep {
                    id: NodeId {
                        kind: kinds
                            .get(kind.as_str())
                            .map_or_else(|| Arc::from(kind), |found| found.name.clone()),
                        key: key.into(),
                    },
                    hash,
                })
                .collect(),
        )
    }

    /// Reads the retained states the store has of the node, once, before they
    /// are used or changed.
    pub(crate) fn load_traces(&self, node: &mut Node, id: &NodeId) -> Result<(), Error> {
        if !node.traces_stored {
            return Ok(());
        }
        let rows = self.store.traces(&id.kind, &id.key)?;
        node.traces = rows
            .into_iter()
            .filter_map(|row| {
                Some(Trace {
                    hash: row.hash,
                    deps: self.deps_of(&row.deps)?,
                    durability: row.durability,
                    bytes: Bytes::Stored(row.row),
                })
            })
            .collect();
        node.traces_stored = false;
        Ok(())
    }

    /// The bytes of a retained state, if they are what its hash says.
    pub(crate) fn trace_bytes(&self, trace: &Trace) -> Result<Option<Arc<[u8]>>, Error> {
        let bytes = match &trace.bytes {
            Bytes::Memory(blob) => return Ok(Some(blob.clone())),
            Bytes::Disk | Bytes::Stored(_) => self.store.blob(&trace.hash)?,
        };
        Ok(bytes
            .filter(|bytes| hash(bytes) == trace.hash)
            .map(Into::into))
    }

    /// Makes the current state of a node a retained one, before it is
    /// replaced.
    pub(crate) fn retain(&self, node: &mut Node, id: &NodeId) -> Result<(), Error> {
        self.load_traces(node, id)?;
        node.retain();
        Ok(())
    }

    /// Starts a revision for an input of `reach` durability: nodes that read
    /// only inputs of a higher durability are not affected.
    pub(crate) fn start_revision(&self, reach: Durability) -> u64 {
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        for level in &self.changed[..=reach.index()] {
            level.store(revision, Ordering::Release);
        }
        revision
    }

    /// Whether verify mode runs this green node again.
    pub(crate) fn sampled(&self) -> bool {
        let period = self.verify_period.load(Ordering::Acquire);
        period > 0
            && self
                .verify_count
                .fetch_add(1, Ordering::Relaxed)
                .is_multiple_of(period)
    }

    /// The revision the last `commits` commits go back to, `0` when there
    /// were fewer.
    pub(crate) fn horizon(&self, commits: u64) -> u64 {
        let marks = self.marks.locked();
        marks
            .len()
            .checked_sub(commits as usize)
            .and_then(|at| marks.get(at))
            .copied()
            .unwrap_or(0)
    }

    /// The marks with this commit's, the oldest dropped past `retain`.
    pub(crate) fn marks_with(&self, revision: u64) -> Vec<u64> {
        let mut marks = self.marks.locked().clone();
        marks.push(revision);
        let kept = marks.len().saturating_sub(self.retain as usize);
        marks.drain(..kept);
        marks
    }

    /// Rows last verified before this revision are older than `retain` commits.
    pub(crate) fn oldest(&self, marks: &[u64]) -> u64 {
        match marks.len() as u64 >= self.retain {
            true => marks.first().copied().unwrap_or(0),
            false => 0,
        }
    }

    /// Gives a persisted input the value the store has of it, if that is the
    /// value its hash is of, and keeps its row from aging out while it is read.
    pub(crate) fn recall(
        &self,
        slot: &Arc<Slot>,
        node: &mut Node,
        kind: &dyn Kind,
        want_value: bool,
    ) -> Result<(), Error> {
        if want_value && node.value.is_none() && node.on_disk {
            let bytes = self.store.value(&slot.id.kind, &slot.id.key)?;
            if let Some(bytes) = bytes.filter(|bytes| hash(bytes) == node.hash)
                && let Some(value) = kind.decode(&bytes)
            {
                node.value = Some(value);
                node.blob = Some(bytes.into());
            }
        }
        node.persisted = true;
        if node.present && node.verified_at < self.horizon(self.retain / 2) {
            self.dirty.mark(slot, node);
        }
        Ok(())
    }

    pub(crate) fn meta(&self, marks: &[u64]) -> Result<Meta, Error> {
        let versions = encode(&*self.versions.locked())?;
        Ok(Meta {
            marks: encode(marks)?,
            revision: self.revision.load(Ordering::Acquire),
            changed: [0, 1, 2].map(|level| self.changed[level].load(Ordering::Acquire)),
            versions,
        })
    }

    pub(crate) fn forget_values(&self, pruned: &[(String, Vec<u8>)]) {
        for (kind, key) in pruned {
            let id = NodeId {
                kind: self.intern(kind),
                key: key.as_slice().into(),
            };
            let slot = self
                .slots
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&id)
                .cloned();
            if let Some(slot) = slot {
                drop_value(&mut slot.node.locked());
            }
        }
    }

    /// Queues a node found green for the store, when the mark it has there is
    /// old: nothing it read has moved after that mark, so it still says what a
    /// later run needs to know, and the row is written again only to keep it
    /// from aging out.
    pub(crate) fn mark_verified(&self, slot: &Arc<Slot>, node: &mut Node) {
        if node.stored_verified_at < self.horizon(self.retain / 2) {
            self.dirty.mark(slot, node);
        }
    }

    fn intern(&self, name: &str) -> Arc<str> {
        let kinds = self
            .kinds
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        kinds
            .get(name)
            .map_or_else(|| Arc::from(name), |found| found.name.clone())
    }
}

pub(crate) type Versions = BTreeMap<String, String>;

pub(crate) struct Registered {
    pub kind: &'static dyn Kind,
    pub name: Arc<str>,
}

impl Registered {
    /// This registration as the one of `kind`, which must stand for the same
    /// types.
    fn agree(&self, kind: &'static dyn Kind) -> Result<Self, Error> {
        let (had, now) = (self.kind.signature(), kind.signature());
        if !had.same(&now) {
            return Err(Error::Kind {
                name: self.name.to_string(),
                had: had.text().to_owned(),
                now: now.text().to_owned(),
            });
        }
        Ok(Self {
            kind: self.kind,
            name: self.name.clone(),
        })
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub(crate) fn drop_value(node: &mut Node) {
    node.value = None;
    node.blob = None;
    node.on_disk = false;
    node.traces.clear();
    node.traces_lost.clear();
    node.traces_stored = false;
    node.traces_dirty = false;
}

pub(crate) fn write_of(slot: &Arc<Slot>, revision: u64) -> Write {
    let node = slot.node.locked();
    let row = Row {
        hash: node.hash,
        deps: deps_blob(&node.deps),
        changed_at: node.changed_at,
        // An input is confirmed by being set: what is written is as of now.
        verified_at: if node.input {
            node.verified_at.max(revision)
        } else {
            node.verified_at
        },
        durability: node.durability,
        input: node.input,
        used_at: node.used_at,
        has_value: false,
        has_traces: false,
    };
    let value = node
        .blob
        .as_ref()
        .filter(|_| !node.input || node.persisted)
        .map(|blob| blob.to_vec());
    Write {
        kind: slot.id.kind.to_string(),
        key: slot.id.key.to_vec(),
        row,
        with_value: node.value_dirty,
        value,
        traces: node.traces_dirty.then(|| changes_of(&node)),
    }
}

pub(crate) fn settle_clean(slot: &Arc<Slot>) {
    let mut node = slot.node.locked();
    if node.value_dirty {
        node.on_disk = node.blob.is_some() && (!node.input || node.persisted);
    }
    node.dirty = false;
    node.value_dirty = false;
    if node.traces_dirty {
        // The store has them now; they are read again when needed.
        node.traces.clear();
        node.traces_lost.clear();
        node.traces_stored = true;
        node.traces_dirty = false;
    }
    node.stored_verified_at = node.verified_at;
    node.synced_used_at = node.used_at;
}

/// What a commit does to the stored states of the node: the new ones are
/// added, the lost ones go.
fn changes_of(node: &Node) -> Changes {
    let added = node.traces.iter().rev().filter_map(|trace| {
        let value = match &trace.bytes {
            Bytes::Memory(blob) => Some(blob.to_vec()),
            Bytes::Disk => None,
            Bytes::Stored(_) => return None,
        };
        Some(TraceRow {
            hash: trace.hash,
            deps: deps_blob(&trace.deps),
            durability: trace.durability,
            value,
        })
    });
    Changes {
        added: added.collect(),
        lost: node.traces_lost.clone(),
    }
}

fn deps_blob(deps: &[Dep]) -> Vec<u8> {
    let deps: Vec<(&str, &[u8], Hash)> = deps
        .iter()
        .map(|dep| (&*dep.id.kind, &*dep.id.key, dep.hash))
        .collect();
    encode(&deps).unwrap_or_default()
}
