//! A project kept loaded between requests: its session, the engine over it
//! with the plugins started, and the open store. A host that serves request
//! after request holds one [`Warm`] and asks it, instead of loading the
//! project again each time.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use lighthouse_engine::{Engine, cache};
use lighthouse_protocol::hash::Hasher;
use lighthouse_rpc::{Started, discover, search_dirs};
use lighthouse_spec::{Config, local_files};
use lighthouse_store::Store;

use crate::{
    CheckRequest, Checked, Result, Session,
    check::{cache_limit, check, check_with},
    environment,
};

/// A loaded project and what a request leaves in it.
///
/// Before each request the files a session is made of are compared with those
/// it was loaded from (see [`watch`]); when they differ the project is loaded
/// again, and the engine, its plugins and the store are built anew. Otherwise
/// the engine serves the request with the graph it has: what changed on disk
/// is read through the inputs of the graph, as a command line run reads it.
#[derive(Default)]
pub struct Warm {
    loaded: Option<Loaded>,
}

struct Loaded {
    /// What the session was loaded from.
    stamp: Stamp,
    session: Session,
    /// Whether a `lighthouse.toml` was found, or named.
    configured: bool,
    engine: Option<Held>,
}

/// What a session was loaded from, and what is judged afresh at each request.
#[derive(PartialEq, Eq)]
struct Stamp {
    /// The files [`watch`] reads.
    watched: String,
    /// The files in the project that its programs name.
    programs: String,
    /// Whether the user trusted the project; the engine is built for one.
    trusted: bool,
    /// The size limit of the cache, which the engine is built with.
    limit: u64,
}

/// The engine of a session that keeps its cache, with what serves it.
struct Held {
    engine: Engine,
    /// What assembling the engine had to say, which every request repeats.
    notices: Vec<String>,
    /// The plugins, to start again one that crashed.
    started: Started,
    /// The store of findings, open since the first request that recorded.
    store: Option<Store>,
}

impl Warm {
    /// A host that has loaded nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The project that `dir` belongs to, as [`Session::load_in`] (when
    /// `required`) or [`Session::open`] reads it, loaded again only when what
    /// it is made of changed.
    pub fn session(
        &mut self,
        (dir, config): (&Path, Option<&Path>),
        required: bool,
    ) -> Result<Session> {
        let loaded = self.refresh(dir, config)?;
        loaded.demand(required)?;
        Ok(loaded.session.clone())
    }

    /// Checks the project that `dir` belongs to: `request` says what, once it
    /// is given the session. A request that keeps no cache is run as
    /// [`check`](crate::check) runs it.
    pub fn check(
        &mut self,
        (dir, config): (&Path, Option<&Path>),
        request: impl FnOnce(&Session) -> Result<CheckRequest>,
    ) -> Result<Checked> {
        let started = Instant::now();
        let loaded = self.refresh(dir, config)?;
        loaded.demand(true)?;
        let request = request(&loaded.session)?;
        if !request.cache {
            return check(&loaded.session, &request);
        }
        let held = held(&loaded.session, loaded.stamp.limit, &mut loaded.engine)?;
        held.started.revive();
        let checked = check_with(
            &loaded.session,
            (&mut held.engine, &held.notices),
            &mut held.store,
            &request,
            started,
        );
        // The session was loaded for the first request only.
        loaded.session.loading.clear();
        checked
    }

    /// The loaded project if what it is made of is as it was, else the project
    /// loaded again.
    fn refresh(&mut self, dir: &Path, config: Option<&Path>) -> Result<&mut Loaded> {
        let current = self
            .loaded
            .take()
            .filter(|loaded| loaded.is_current(dir, config));
        let loaded = match current {
            Some(loaded) => loaded,
            None => Loaded::load(dir, config)?,
        };
        Ok(self.loaded.insert(loaded))
    }
}

impl fmt::Debug for Warm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Warm")
            .field("loaded", &self.loaded.is_some())
            .finish()
    }
}

impl Loaded {
    fn load(dir: &Path, config: Option<&Path>) -> Result<Self> {
        let watched = watch(dir, config);
        let session = Session::open(dir, config, false)?;
        Ok(Self {
            stamp: Stamp::of(&session, watched),
            configured: config.is_some() || dir.ancestors().any(|a| Config::file_in(a).is_some()),
            session,
            engine: None,
        })
    }

    /// Whether the project is as it was loaded.
    fn is_current(&self, dir: &Path, config: Option<&Path>) -> bool {
        let watched = watch(dir, config);
        self.stamp.watched == watched && self.stamp == Stamp::of(&self.session, watched)
    }

    /// Fails as loading a project that `required` to be configured does when
    /// there is no `lighthouse.toml`.
    fn demand(&self, required: bool) -> Result<()> {
        if required && !self.configured {
            return Err(format!(
                "no {} found (run `lighthouse init`)",
                lighthouse_spec::FILE_NAME
            )
            .into());
        }
        Ok(())
    }
}

impl Stamp {
    fn of(session: &Session, watched: String) -> Self {
        let mut programs = Hasher::new();
        for file in &session.basis.files {
            programs.update(file.as_os_str().as_encoded_bytes());
            programs.update(fs::read(file).unwrap_or_default());
        }
        Self {
            watched,
            programs: programs.hex(),
            trusted: session.trusted(),
            limit: cache_limit(),
        }
    }
}

/// The engine of `session` in `slot`, built by the first request that keeps a
/// cache. One that started with a plugin gone is built again by the next, as a
/// command line run would find that plugin.
fn held<'a>(session: &Session, limit: u64, slot: &'a mut Option<Held>) -> Result<&'a mut Held> {
    if slot
        .as_ref()
        .is_some_and(|held| !held.started.registered.incomplete.is_empty())
    {
        *slot = None;
    }
    if slot.is_none() {
        let dir = cache::dir(&session.root);
        let (engine, notices, started) = session.engine_kept(Some(&dir))?;
        *slot = Some(Held {
            engine: engine.with_cache(dir, limit),
            notices,
            started,
            store: None,
        });
    }
    slot.as_mut()
        .ok_or_else(|| "the engine was not built".into())
}

/// A digest of what loading the project of `dir` reads, the way it finds it:
/// the configuration, the personal files, the project's own decisions, and the
/// manifests, programs and catalogs of the plugins in the places plugins are
/// looked for. A file that cannot be read is part of it by the reason.
fn watch(dir: &Path, config: Option<&Path>) -> String {
    let mut hash = Hasher::new();
    let mut field = |name: &str, value: &[u8]| {
        hash.update(name);
        hash.update(value.len().to_le_bytes());
        hash.update(value);
    };
    field("dir", dir.as_os_str().as_encoded_bytes());
    let file = config
        .map(Path::to_owned)
        .or_else(|| dir.ancestors().find_map(Config::file_in));
    let root = match (config, &file) {
        (None, Some(file)) => file.parent().unwrap_or(dir),
        _ => dir,
    };
    field("root", root.as_os_str().as_encoded_bytes());
    for path in file
        .iter()
        .map(PathBuf::as_path)
        .chain(environment::user_file().as_deref())
        .chain([environment::local_file(root).as_path()])
    {
        field("file", path.as_os_str().as_encoded_bytes());
        field("text", &read(path));
    }
    match local_files(root) {
        Ok(Some(files)) => {
            for (name, text) in files {
                field("decision", name.as_bytes());
                field("text", text.as_bytes());
            }
        }
        Ok(None) => field("decisions", b"none"),
        Err(e) => field("decisions", e.to_string().as_bytes()),
    }
    match discover(&search_dirs(root)) {
        Ok(found) => {
            for plugin in found.found {
                field("plugin", plugin.dir.as_os_str().as_encoded_bytes());
                field("manifest", format!("{:?}", plugin.resource).as_bytes());
                field(
                    "program",
                    plugin.program_stamp().unwrap_or_default().as_bytes(),
                );
                if let Some(catalog) = plugin.catalog_dir() {
                    field("catalog", stamp_tree(&catalog).as_bytes());
                }
            }
            for broken in found.broken {
                field("broken", broken.to_string().as_bytes());
            }
        }
        Err(e) => field("plugins", e.to_string().as_bytes()),
    }
    hash.hex()
}

/// The content of a file, or why it has none.
fn read(path: &Path) -> Vec<u8> {
    match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => b"absent".to_vec(),
        Err(e) => e.to_string().into_bytes(),
    }
}

/// The names, sizes and modification times of the files under `dir`, in
/// order.
fn stamp_tree(dir: &Path) -> String {
    let mut out = String::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                pending.push(path);
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            out.push_str(&format!("{}:{}:{modified};", path.display(), meta.len()));
        }
    }
    out
}
