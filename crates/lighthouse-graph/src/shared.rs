use std::{
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
    node::{Dep, Hash, Locked, Node, NodeId, Slot},
    query::{Kind, decode, encode},
    store::{Meta, Row, Store, Write},
};

/// What every handle shares.
pub(crate) struct Shared {
    pub revision: AtomicU64,
    pub changed: [AtomicU64; Durability::COUNT],
    pub kinds: RwLock<HashMap<&'static str, Registered>>,
    pub slots: RwLock<HashMap<NodeId, Arc<Slot>>>,
    pub store: Store,
    pub dirty: Dirty,
    pub executed: Mutex<Vec<NodeId>>,
    pub waits: Mutex<()>,
    pub verify_period: AtomicU64,
    pub verify_count: AtomicU64,
    pub limit: AtomicU64,
    pub versions: Mutex<Versions>,
}

pub(crate) struct Registered {
    pub kind: &'static dyn Kind,
    pub name: Arc<str>,
}

pub(crate) type Versions = BTreeMap<String, String>;

impl Shared {
    pub(crate) fn register(&self, kind: &'static dyn Kind) -> Registered {
        let name = kind.name();
        if let Some(found) = self
            .kinds
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
        {
            return Registered {
                kind: found.kind,
                name: found.name.clone(),
            };
        }
        let mut kinds = self
            .kinds
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = kinds.entry(name).or_insert_with(|| Registered {
            kind,
            name: Arc::from(name),
        });
        Registered {
            kind: found.kind,
            name: found.name.clone(),
        }
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
        let Ok(deps) = decode::<Vec<(String, Vec<u8>, Hash)>>(&row.deps) else {
            return;
        };
        let deps = deps
            .into_iter()
            .map(|(kind, key, hash)| Dep {
                id: NodeId {
                    kind: self.intern(&kind),
                    key: key.into(),
                },
                hash,
            })
            .collect();
        node.present = true;
        node.input = row.input;
        node.hash = row.hash;
        node.deps = deps;
        node.changed_at = row.changed_at;
        node.verified_at = row.verified_at;
        node.durability = row.durability;
        node.used_at = row.used_at;
        node.synced_used_at = row.used_at;
        node.on_disk = row.has_value;
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

    pub(crate) fn meta(&self) -> Result<Meta, Error> {
        let versions = encode(&*self.versions.locked())?;
        Ok(Meta {
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
}

pub(crate) fn write_of(slot: &Arc<Slot>) -> Write {
    let node = slot.node.locked();
    let deps: Vec<(&str, &[u8], Hash)> = node
        .deps
        .iter()
        .map(|dep| (&*dep.id.kind, &*dep.id.key, dep.hash))
        .collect();
    let row = Row {
        hash: node.hash,
        deps: encode(&deps).unwrap_or_default(),
        changed_at: node.changed_at,
        verified_at: node.verified_at,
        durability: node.durability,
        input: node.input,
        used_at: node.used_at,
        has_value: false,
    };
    let value = node
        .blob
        .as_ref()
        .filter(|_| !node.input)
        .map(|blob| blob.to_vec());
    Write {
        kind: slot.id.kind.to_string(),
        key: slot.id.key.to_vec(),
        row,
        with_value: node.value_dirty,
        value,
    }
}

pub(crate) fn settle_clean(slot: &Arc<Slot>) {
    let mut node = slot.node.locked();
    if node.value_dirty {
        node.on_disk = node.blob.is_some() && !node.input;
    }
    node.dirty = false;
    node.value_dirty = false;
    node.synced_used_at = node.used_at;
}
