use std::{
    path::Path,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    Durability, Error, Explanation, Input, Key, Query, Revision, Value,
    dirty::Dirty,
    explain,
    frame::Frame,
    node::{Locked, NodeId},
    query::{Kind, encode, hash},
    shared::{Shared, Versions, drop_value, settle_clean, write_of},
    store::{Store, Write},
};

/// The size, in bytes of stored values, past which the least recently used
/// are dropped at [`Db::commit`]: 256 MB.
pub const DEFAULT_LIMIT: u64 = 256 << 20;

/// Verify mode samples one green node in this many when on by default.
const VERIFY_PERIOD: u64 = 64;

const VERSION: Input<String, String> = Input::new("version");

/// The handle that queries read through. The one from [`Db::open`] is the
/// owner: it sets inputs and commits. The one a query is given records what
/// the query reads, and may be shared with threads that query spawns.
pub struct Db {
    pub(crate) shared: Arc<Shared>,
    pub(crate) frame: Option<Arc<Frame>>,
}

impl Db {
    /// Opens `graph.db` at `path`, creating it. Each version (the binary,
    /// each plugin, ...) is a `Static` input: one that changed since the last
    /// open invalidates exactly the queries that read it.
    pub fn open(path: &Path, versions: &Versions) -> Result<Self, Error> {
        Self::with_store(Store::open(path)?, versions)
    }

    /// A graph that is never written to disk (`--no-cache`).
    pub fn in_memory(versions: &Versions) -> Result<Self, Error> {
        Self::with_store(Store::in_memory()?, versions)
    }

    fn with_store(store: Store, versions: &Versions) -> Result<Self, Error> {
        let meta = store.meta()?;
        let verify = if cfg!(debug_assertions) {
            VERIFY_PERIOD
        } else {
            0
        };
        let shared = Shared {
            revision: AtomicU64::new(meta.revision),
            changed: meta.changed.map(AtomicU64::new),
            kinds: RwLock::default(),
            slots: RwLock::default(),
            store,
            dirty: Dirty::default(),
            executed: Mutex::default(),
            waits: Mutex::default(),
            verify_period: AtomicU64::new(verify),
            verify_count: AtomicU64::new(0),
            limit: AtomicU64::new(DEFAULT_LIMIT),
            versions: Mutex::default(),
            states: Mutex::default(),
        };
        let mut db = Self {
            shared: Arc::new(shared),
            frame: None,
        };
        db.set_versions(versions)?;
        Ok(db)
    }

    /// Makes the kind known so that stored nodes of it can be validated and
    /// run. Pulling or setting it does this too; registering up front is
    /// what lets a node whose dependencies are not pulled yet, in a process
    /// that just opened the store, be validated without running.
    pub fn register_query<K: Key, V: Value>(&self, query: &'static Query<K, V>) {
        self.shared.register(query);
    }

    /// Makes an input kind known; see [`Db::register_query`].
    pub fn register_input<K: Key, V: Value>(&self, input: &'static Input<K, V>) {
        self.shared.register(input);
    }

    /// The revision the inputs are at.
    pub fn revision(&self) -> Revision {
        Revision(self.shared.revision.load(Ordering::Acquire))
    }

    /// Sets an input. A new revision starts only when the value's hash or the
    /// durability differs from the one stored: an equal value is a no-op.
    /// Needs no query to be running.
    pub fn set_input<K: Key, V: Value>(
        &mut self,
        input: &'static Input<K, V>,
        key: &K,
        value: V,
        durability: Durability,
    ) -> Result<(), Error> {
        let registered = self.shared.register(input);
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        let new_hash = hash(&encode(&value)?);
        let slot = self.shared.slot(&id);
        let mut node = slot.node.locked();
        self.shared.load(&mut node, &id)?;
        node.value = Some(Arc::new(value));
        let moved = !node.present || node.hash != new_hash || node.durability != durability;
        if moved {
            let reach = if node.present {
                node.durability.max(durability)
            } else {
                durability
            };
            let revision = self.shared.start_revision(reach);
            node.become_input(new_hash, durability, revision);
            self.shared.dirty.mark(&slot, &mut node);
        }
        Ok(())
    }

    /// The value of an input, recorded as read by the running query.
    pub fn input<K: Key, V: Value>(
        &self,
        input: &'static Input<K, V>,
        key: &K,
    ) -> Result<Arc<V>, Error> {
        self.read(input, key)
    }

    /// The value of a query for a key: from memory, from the store if it is
    /// valid, or by running it, and recorded as read by the running query.
    pub fn query<K: Key, V: Value>(
        &self,
        query: &'static Query<K, V>,
        key: &K,
    ) -> Result<Arc<V>, Error> {
        self.read(query, key)
    }

    /// The version set for `name` at open, as read by the running query.
    pub fn version(&self, name: &str) -> Result<Arc<String>, Error> {
        self.read(&VERSION, &name.to_owned())
    }

    /// The most bytes of stored values kept at the next [`Db::commit`].
    pub fn set_limit(&mut self, bytes: u64) {
        self.shared.limit.store(bytes, Ordering::Release);
    }

    /// Verify mode: run one green node in `period` again and fail with
    /// [`Error::Impure`] if its value differs, `0` for none. On by default in
    /// debug builds.
    ///
    /// It catches a query that reads outside the graph (a file, the clock,
    /// the environment, a global) whenever that read changed since the node
    /// was computed and the node is sampled. It cannot catch an outside read
    /// that did not change, nor one that is missed on a node that is never
    /// sampled. That values are produced only through the `Db` is held by the
    /// types: a query gets a `Db` and nothing else of the graph.
    pub fn set_verify(&mut self, period: u64) {
        self.shared.verify_period.store(period, Ordering::Release);
    }

    /// Drops the stored value of a node, memory and disk, keeping its hash,
    /// its dependencies and its revisions: dependents still validate, and
    /// pulling it runs it again.
    pub fn evict<K: Key, V: Value>(
        &mut self,
        query: &'static Query<K, V>,
        key: &K,
    ) -> Result<(), Error> {
        let registered = self.shared.register(query);
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        let slot = self.shared.slot(&id);
        let mut node = slot.node.locked();
        self.shared.load(&mut node, &id)?;
        if node.present && (node.on_disk || node.value.is_some()) {
            drop_value(&mut node);
            node.value_dirty = true;
            self.shared.dirty.mark(&slot, &mut node);
        }
        Ok(())
    }

    /// Why the query last ran for this key: the first dependency whose hash
    /// moved, walked down to an input. `None` when the node was not touched
    /// in this process. It backs `lighthouse cache explain`.
    pub fn explain<K: Key, V: Value>(
        &self,
        query: &'static Query<K, V>,
        key: &K,
    ) -> Result<Option<Explanation>, Error> {
        let registered = self.shared.register(query);
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        Ok(explain::walk(&self.shared, id))
    }

    /// The nodes that ran since the last call, as `kind(key)`, in order of
    /// completion.
    pub fn take_executed(&self) -> Vec<String> {
        let ids = std::mem::take(&mut *self.shared.executed.locked());
        ids.iter().map(|id| self.shared.label(id)).collect()
    }

    /// Writes what changed since the last commit in one transaction, and
    /// prunes values over the limit. Call it with no pull running.
    pub fn commit(&mut self) -> Result<(), Error> {
        let slots = self.shared.dirty.take();
        let writes: Vec<Write> = slots.iter().map(write_of).collect();
        let meta = self.shared.meta()?;
        let limit = self.shared.limit.load(Ordering::Acquire);
        let states: Vec<(String, Vec<u8>)> = self
            .shared
            .states
            .locked()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let pruned = match self.shared.store.commit(&writes, &states, &meta, limit) {
            Ok(pruned) => pruned,
            Err(err) => {
                self.shared.dirty.restore(slots);
                return Err(err);
            }
        };
        self.shared.states.locked().clear();
        for slot in &slots {
            settle_clean(slot);
        }
        self.shared.forget_values(&pruned);
        Ok(())
    }

    fn read<K: Key, V: Value, Q: Kind + 'static>(
        &self,
        kind: &'static Q,
        key: &K,
    ) -> Result<Arc<V>, Error> {
        let registered = self.shared.register(kind);
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        let pulled = self.pull(&id, kind, true)?;
        if let Some(frame) = &self.frame {
            frame.record(&id, pulled.hash, pulled.durability);
        }
        let value = pulled
            .value
            .ok_or_else(|| Error::MissingInput(self.shared.label(&id)))?;
        value
            .downcast::<V>()
            .map_err(|_| Error::Encode(format!("{} holds another type", id.kind)))
    }

    /// Sets one version after open: the same as having opened with it.
    pub fn set_version(&mut self, name: &str, version: &str) -> Result<(), Error> {
        self.set_input(
            &VERSION,
            &name.to_owned(),
            version.to_owned(),
            Durability::Static,
        )?;
        self.shared
            .versions
            .locked()
            .insert(name.to_owned(), version.to_owned());
        Ok(())
    }

    fn set_versions(&mut self, versions: &Versions) -> Result<(), Error> {
        for (name, version) in versions {
            self.set_version(name, version)?;
        }
        Ok(())
    }
}
