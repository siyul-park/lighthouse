use std::{
    any::Any,
    fmt,
    path::Path,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    Durability, Error, Explanation, Input, Key, Query, Value,
    dirty::Dirty,
    explain, fan,
    frame::Frame,
    node::{Locked, NodeId},
    query::{Kind, decode, encode, hash},
    shared::{Shared, Versions, settle_clean, write_of},
    store::{Store, Write},
};

/// The size, in bytes of stored values, past which the least recently used
/// are dropped at [`Graph::commit`]: 256 MB.
pub const DEFAULT_LIMIT: u64 = 256 << 20;

/// Commits a row is kept without being verified. A row of a file that is gone
/// is read by no run, so this is how long it outlives the file; a row that is
/// read keeps itself alive. Sixty-four is a few days of commits at the pace a
/// developer commits (each run, watch cycle or fix round is one), long enough
/// that a branch switch does not lose the rows of the branch it left.
pub const DEFAULT_RETAIN: u64 = 64;

/// Verify mode samples one green node in this many when on by default.
const VERIFY_PERIOD: u64 = 64;

/// The versions given in [`Options`], as `Static` inputs by name: a query that
/// reads one is run again when it changes.
pub static VERSION: Input<String, String> = Input::new("version");

/// How the graph is opened.
#[derive(Clone)]
pub struct Options {
    /// The binary's, each plugin's, ...: set as the `VERSION` inputs. One that
    /// changed since the last open invalidates exactly what read it.
    pub versions: Versions,
    /// The most bytes of stored values kept at a [`Graph::commit`].
    pub limit: u64,
    /// Verify mode: run one green node in this many again and fail with
    /// [`Error::Impure`] if its value differs, `0` for none. On by default in
    /// debug builds.
    ///
    /// It catches a query that reads outside the graph (a file, the clock,
    /// the environment, a global) whenever that read changed since the node
    /// was computed and the node is sampled. It cannot catch an outside read
    /// that did not change, nor one that is missed on a node that is never
    /// sampled. That values are produced only through the `Graph` is held by the
    /// types: a query gets a `Graph` and nothing else of the graph.
    pub verify: u64,
    /// How many commits a row that nothing set, validated or ran is kept:
    /// past that a commit deletes it, so that the rows of files that are gone
    /// go too.
    pub retain: u64,
    /// What the queries of this graph read besides the graph, see
    /// [`Options::data`].
    pub data: Option<Arc<dyn Any + Send + Sync>>,
}

impl Options {
    /// Gives the queries `data` to read with [`Graph::data`]: the per-graph
    /// user data of Buck2's DICE and the fields of the user's database in
    /// Salsa. What a query reads there is not recorded, so it must be fixed
    /// for the life of the graph: the code a query calls through (analyzers,
    /// providers), never what changes from run to run, which is an input. A
    /// value that moves would reach a query without moving its hash, and
    /// what read it would stay green on a stale value.
    pub fn data(self, data: Arc<dyn Any + Send + Sync>) -> Self {
        Self {
            data: Some(data),
            ..self
        }
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("versions", &self.versions)
            .field("limit", &self.limit)
            .field("verify", &self.verify)
            .field("retain", &self.retain)
            .field("data", &self.data.is_some())
            .finish()
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            versions: Versions::new(),
            limit: DEFAULT_LIMIT,
            verify: if cfg!(debug_assertions) {
                VERIFY_PERIOD
            } else {
                0
            },
            retain: DEFAULT_RETAIN,
            data: None,
        }
    }
}

/// The handle that queries read through. The one from [`Graph::open`] is the
/// owner: it sets inputs and commits. The one a query is given records what
/// the query reads, and may be shared with threads that query spawns.
pub struct Graph {
    pub(crate) shared: Arc<Shared>,
    pub(crate) frame: Option<Arc<Frame>>,
}

impl Graph {
    /// Opens `graph.db` at `path`, creating it.
    pub fn open(path: &Path, options: Options) -> Result<Self, Error> {
        Self::with_store(Store::open(path)?, options)
    }

    /// A graph that is never written to disk (`--no-cache`).
    pub fn in_memory(options: Options) -> Result<Self, Error> {
        Self::with_store(Store::in_memory()?, options)
    }

    fn with_store(store: Store, options: Options) -> Result<Self, Error> {
        let meta = store.meta()?;
        let shared = Shared {
            revision: AtomicU64::new(meta.revision),
            changed: meta.changed.map(AtomicU64::new),
            marks: Mutex::new(decode(&meta.marks).unwrap_or_default()),
            kinds: RwLock::default(),
            slots: RwLock::default(),
            store,
            dirty: Dirty::default(),
            executed: Mutex::default(),
            waits: Mutex::default(),
            verify_period: AtomicU64::new(options.verify),
            verify_count: AtomicU64::new(0),
            limit: AtomicU64::new(options.limit),
            versions: Mutex::default(),
            retain: options.retain,
            data: options.data,
        };
        let mut graph = Self {
            shared: Arc::new(shared),
            frame: None,
        };
        for (name, version) in options.versions {
            graph.set_input(&VERSION, &name, version.clone(), Durability::Static)?;
            graph.shared.versions.locked().insert(name, version);
        }
        Ok(graph)
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
        let registered = self.shared.register(input)?;
        let persisted = registered.kind.persisted();
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        let blob = encode(&value)?;
        let new_hash = hash(&blob);
        let slot = self.shared.slot(&id);
        let mut node = slot.node.locked();
        self.shared.load(&mut node, &id)?;
        node.value = Some(Arc::new(value));
        node.persisted = persisted;
        node.blob = persisted.then(|| blob.into());
        let moved = !node.present || node.hash != new_hash || node.durability != durability;
        if moved {
            let reach = if node.present {
                node.durability.max(durability)
            } else {
                durability
            };
            let revision = self.shared.start_revision(reach);
            node.become_input(new_hash, durability, revision, persisted);
            self.shared.dirty.mark(&slot, &mut node);
        } else {
            if node.verified_at < self.shared.horizon(self.shared.retain / 2) {
                node.verified_at = self.shared.revision.load(Ordering::Acquire);
                self.shared.dirty.mark(&slot, &mut node);
            }
        }
        Ok(())
    }

    /// The value of an input, or `None` when it is not set in this process.
    /// The read is recorded either way: a query that found it unset runs
    /// again when it is set, so an unset input counts as `Volatile`.
    pub fn input<K: Key, V: Value>(
        &self,
        input: &'static Input<K, V>,
        key: &K,
    ) -> Result<Option<Arc<V>>, Error> {
        let registered = self.shared.register(input)?;
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        let pulled = self.pull(&id, input, true)?;
        if let Some(frame) = &self.frame {
            let durability = if pulled.missing {
                Durability::Volatile
            } else {
                pulled.durability
            };
            frame.record(&id, pulled.hash, durability);
        }
        Ok(pulled.value.and_then(|value| value.downcast::<V>().ok()))
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

    /// The data the graph was opened with, if it is a `T`: what a query
    /// calls through that is fixed for the life of the graph, see
    /// [`Options::data`].
    pub fn data<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.shared.data.as_deref()?.downcast_ref()
    }

    /// Why the query last ran for this key: the first dependency whose hash
    /// moved, walked down to an input. `None` when the node was not touched
    /// in this process. It backs `lighthouse cache explain`.
    pub fn explain<K: Key, V: Value>(
        &self,
        query: &'static Query<K, V>,
        key: &K,
    ) -> Result<Option<Explanation>, Error> {
        let registered = self.shared.register(query)?;
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        Ok(explain::walk(&self.shared, id))
    }

    /// Whether the query being run is a node run again by verify mode, which
    /// a query that starts a program answers with [`Error::Unverifiable`].
    pub fn verifying(&self) -> bool {
        self.frame.as_ref().is_some_and(|frame| frame.verifying)
    }

    /// The nodes that ran since the last call, as `kind(key)`, in order of
    /// completion.
    pub fn take_executed(&self) -> Vec<String> {
        let ids = std::mem::take(&mut *self.shared.executed.locked());
        ids.iter().map(|id| self.shared.label(id)).collect()
    }

    /// Runs `run` on every item, on the calling thread and on the threads of
    /// the current `rayon` pool that are free, and returns the results in
    /// the order of the items. What `run` reads through the handle it is
    /// given is read by the query that called `par_map`.
    ///
    /// A claimed node that an item waits for is run by its owner, and a cycle
    /// that runs through items is an [`Error::Cycle`](crate::Error::Cycle)
    /// like any other. See the module docs for why this is the way to fan
    /// out. A panic in `run` is raised again here.
    pub fn par_map<I, R>(
        &self,
        items: Vec<I>,
        run: impl Fn(&Graph, &I) -> R + Send + Sync + 'static,
    ) -> Vec<R>
    where
        I: Send + Sync + 'static,
        R: Send + 'static,
    {
        fan::map(self, items, run)
    }

    /// Writes what changed since the last commit in one transaction, deletes
    /// the rows nothing verified for [`Options::retain`] commits and prunes
    /// values over the limit. Call it with no pull running.
    pub fn commit(&mut self) -> Result<(), Error> {
        let slots = self.shared.dirty.take();
        let revision = self.shared.revision.load(Ordering::Acquire);
        let mut writes: Vec<Write> = slots.iter().map(|slot| write_of(slot, revision)).collect();
        // In key order, so that rows go into the store's tree beside the last.
        writes.sort_unstable_by(|a, b| (&a.kind, &a.key).cmp(&(&b.kind, &b.key)));
        let marks = self.shared.marks_with(revision);
        let meta = self.shared.meta(&marks)?;
        let limit = self.shared.limit.load(Ordering::Acquire);
        let forget = self.shared.oldest(&marks);
        let pruned = match self.shared.store.commit(&writes, &meta, limit, forget) {
            Ok(pruned) => pruned,
            Err(err) => {
                self.shared.dirty.restore(slots);
                return Err(err);
            }
        };
        *self.shared.marks.locked() = marks;
        for slot in &slots {
            settle_clean(slot);
        }
        self.shared.forget_values(&pruned);
        Ok(())
    }

    pub(crate) fn hash_of<K: Key, V: Value>(
        &self,
        query: &'static Query<K, V>,
        key: &K,
    ) -> Result<[u8; 32], Error> {
        let registered = self.shared.register(query)?;
        let id = NodeId {
            kind: registered.name,
            key: encode(key)?.into(),
        };
        let pulled = self.pull(&id, query, false)?;
        if let Some(frame) = &self.frame {
            frame.record(&id, pulled.hash, pulled.durability);
        }
        Ok(pulled.hash)
    }

    fn read<K: Key, V: Value, Q: Kind + 'static>(
        &self,
        kind: &'static Q,
        key: &K,
    ) -> Result<Arc<V>, Error> {
        let registered = self.shared.register(kind)?;
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
            .ok_or_else(|| Error::Encode(format!("{} has no value", self.shared.label(&id))))?;
        value
            .downcast::<V>()
            .map_err(|_| Error::Encode(format!("{} holds another type", id.kind)))
    }
}
