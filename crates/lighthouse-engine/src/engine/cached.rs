//! Reusing what a rule found: the key of a unit (a rule over one file, or over
//! the project), and the run of a rule that looks the unit up first.
//!
//! What is the same for every unit of a rule (the build, the rule, its
//! revision and reach) is absorbed once into a prefix; a unit adds its tag
//! (applicability and resolved options) and the digests its reach asks for.
//! A rule that reads the whole project and runs per file keeps one row per
//! project state, holding every file's findings, instead of one entry per file.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, PoisonError},
};

use lighthouse_cache::{Cache, Digest, Digests, Key, KeyBuilder, Stored};
use lighthouse_model::{Reach, TestScope, hash::Hasher};
use serde::{Deserialize, Serialize};

use super::*;

/// The cache a run reads and writes, with the digests its keys are built from.
pub(super) struct RunCache {
    cache: Cache,
    digests: Digests,
    documents: Digest,
    plans: HashMap<String, Plan>,
}

/// What is known of a rule before any unit runs.
struct Plan {
    prefix: KeyBuilder,
    reach: Reach,
    positions: bool,
    /// The row of a global rule that runs per file, read on first use.
    row: OnceLock<Row>,
}

/// The findings of a global rule over every file of one project state.
struct Row {
    key: Key,
    loaded: HashMap<PathBuf, Entry>,
    fresh: Mutex<Vec<Vec<u8>>>,
}

/// One file of a row: the tag says which options and applicability it was
/// judged with.
#[derive(Serialize, Deserialize)]
struct Entry {
    file: PathBuf,
    tag: String,
    found: Stored,
}

/// An entry as it is written, borrowing what the run found.
#[derive(Serialize)]
struct EntryRef<'a> {
    file: &'a Path,
    tag: &'a str,
    found: FoundRef<'a>,
}

#[derive(Serialize)]
struct FoundRef<'a> {
    diagnostics: &'a [Diagnostic],
    notices: &'a [String],
}

impl Engine {
    /// Runs `rule` for the subject `ctx` focuses (a file, else the project),
    /// or takes what an earlier run found for the same subject in the same
    /// state. What fails or cannot finish is never stored.
    pub(super) fn run_rule(
        &self,
        scene: &Scene,
        rule: &dyn Rule,
        ctx: &Ctx,
        options: &Options,
    ) -> Result<Vec<Diagnostic>, lighthouse_plugin::Error> {
        let plan = scene
            .cache
            .and_then(|run| Some((run, run.plans.get(&rule.manifest().id)?)));
        let Some((run, plan)) = plan else {
            return rule.check(ctx, options);
        };
        match ctx.file {
            Some((file, _)) if plan.reach == Reach::Global => {
                run.in_row(plan, file, rule, ctx, options)
            }
            _ => run.alone(plan, rule, ctx, options),
        }
    }

    /// Opens the cache for a run over `project` when the engine has one, and
    /// hashes the project for its keys; what goes wrong is a notice and the
    /// run goes on without.
    pub(super) fn open_cache(
        &self,
        project: &Project,
        selected: &[&dyn Rule],
        notices: &mut BTreeSet<String>,
    ) -> Option<RunCache> {
        let settings = self.cache.as_ref()?;
        let opened = Cache::open(&settings.dir, settings.limit).and_then(|cache| {
            Ok(RunCache {
                cache,
                digests: Digests::of(project)?,
                documents: lighthouse_cache::documents(project.documents())?,
                plans: self.plans(selected),
            })
        });
        match opened {
            Ok(run) => Some(run),
            Err(e) => {
                notices.insert(format!("the result cache is not used: {e}"));
                None
            }
        }
    }

    /// The prefix of the keys of each selected rule that may be cached.
    fn plans(&self, selected: &[&dyn Rule]) -> HashMap<String, Plan> {
        let base = self.cache_base();
        selected
            .iter()
            .filter_map(|rule| {
                let meta = rule.manifest();
                let caching = meta.caching.as_ref()?;
                let mut prefix = KeyBuilder::new();
                prefix
                    .part(base)
                    .part(&meta.id)
                    .part(&caching.revision)
                    .part([caching.reach as u8, u8::from(caching.positions)]);
                let plan = Plan {
                    prefix,
                    reach: caching.reach,
                    positions: caching.positions,
                    row: OnceLock::new(),
                };
                Some((meta.id.clone(), plan))
            })
            .collect()
    }

    /// What every key of this engine depends on besides its rule: the build,
    /// the plugins, and how the languages are configured.
    fn cache_base(&self) -> Digest {
        let mut hasher = Hasher::new();
        let mut part = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        part(lighthouse_cache::build_identity().as_bytes());
        for plugin in self.registry.manifests() {
            part(plugin.id.as_bytes());
            part(plugin.version.as_bytes());
        }
        // Maps of sorted keys: the same configuration is the same text.
        part(
            serde_json::to_string(&self.ws.languages)
                .unwrap_or_default()
                .as_bytes(),
        );
        part(
            serde_json::to_string(&self.ws.constructors)
                .unwrap_or_default()
                .as_bytes(),
        );
        hasher.finish_bytes()
    }

    /// Writes what the run stored and used, and says how it went.
    pub(super) fn close_cache(
        &self,
        run: &RunCache,
        timings: &mut Timings,
        notices: &mut BTreeSet<String>,
    ) {
        run.store_rows();
        match run.cache.finish() {
            Ok(stats) => {
                timings.cache_hits = stats.hits;
                timings.cache_misses = stats.misses;
            }
            Err(e) => {
                notices.insert(format!("the result cache was not saved: {e}"));
            }
        }
    }
}

impl RunCache {
    /// Runs a unit that has an entry of its own.
    fn alone(
        &self,
        plan: &Plan,
        rule: &dyn Rule,
        ctx: &Ctx,
        options: &Options,
    ) -> Result<Vec<Diagnostic>, lighthouse_plugin::Error> {
        let tag = tag(ctx.applies, options);
        let file = ctx.file.map(|(file, _)| file);
        let Some(key) = self.key(plan, &tag, file) else {
            return rule.check(ctx, options);
        };
        let stored = self.cache.get::<Stored>(&key);
        self.cache.count(stored.is_some());
        if let Some(stored) = stored {
            stored.notices.into_iter().for_each(|n| ctx.notices.push(n));
            return Ok(stored.diagnostics);
        }
        let found = rule.check(ctx, options)?;
        let notices = ctx.notices.take();
        self.cache.put(
            key,
            &FoundRef {
                diagnostics: &found,
                notices: &notices,
            },
        );
        notices.into_iter().for_each(|n| ctx.notices.push(n));
        Ok(found)
    }

    /// Runs a unit that is a file's part of a row.
    fn in_row(
        &self,
        plan: &Plan,
        file: &File,
        rule: &dyn Rule,
        ctx: &Ctx,
        options: &Options,
    ) -> Result<Vec<Diagnostic>, lighthouse_plugin::Error> {
        let tag = tag(ctx.applies, options);
        let row = self.row(plan);
        let loaded = row.loaded.get(&file.path).filter(|entry| entry.tag == tag);
        self.cache.count(loaded.is_some());
        if let Some(entry) = loaded {
            let found = &entry.found;
            found
                .notices
                .iter()
                .for_each(|n| ctx.notices.push(n.clone()));
            return Ok(found.diagnostics.clone());
        }
        let found = rule.check(ctx, options)?;
        let notices = ctx.notices.take();
        let entry = EntryRef {
            file: &file.path,
            tag: &tag,
            found: FoundRef {
                diagnostics: &found,
                notices: &notices,
            },
        };
        if let Ok(bytes) = serde_json::to_vec(&entry) {
            row.fresh
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(bytes);
        }
        notices.into_iter().for_each(|n| ctx.notices.push(n));
        Ok(found)
    }

    /// The key of a unit over `file` (the project for a project rule), by the
    /// reach of its rule; `None` when the file is not one the digests know.
    fn key(&self, plan: &Plan, tag: &str, file: Option<&File>) -> Option<Key> {
        let mut key = plan.prefix.clone();
        key.part(tag);
        match (file, plan.reach) {
            (Some(file), Reach::Local | Reach::Neighbors) => {
                let digests = self.digests.file(&file.path)?;
                key.part(file.path.as_os_str().as_encoded_bytes())
                    .part(digests.slice);
                if plan.reach == Reach::Neighbors {
                    key.part(if plan.positions {
                        digests.neighbors_at
                    } else {
                        digests.neighbors
                    });
                }
            }
            _ => {
                key.part(self.digests.project()).part(self.documents);
            }
        }
        Some(key.finish())
    }

    /// The row of a global rule that runs per file.
    fn row<'a>(&self, plan: &'a Plan) -> &'a Row {
        plan.row.get_or_init(|| {
            let mut key = plan.prefix.clone();
            key.part(self.digests.project()).part(self.documents);
            let key = key.finish();
            let loaded = self
                .cache
                .get::<Vec<Entry>>(&key)
                .unwrap_or_default()
                .into_iter()
                .map(|entry| (entry.file.clone(), entry))
                .collect();
            Row {
                key,
                loaded,
                fresh: Mutex::default(),
            }
        })
    }

    /// Hands the rows that gained files to the cache, with the files they
    /// already held that this run did not judge again.
    fn store_rows(&self) {
        for plan in self.plans.values() {
            let Some(row) = plan.row.get() else { continue };
            let fresh =
                std::mem::take(&mut *row.fresh.lock().unwrap_or_else(PoisonError::into_inner));
            if fresh.is_empty() {
                continue;
            }
            let judged: Vec<Entry> = fresh
                .iter()
                .filter_map(|bytes| serde_json::from_slice(bytes).ok())
                .collect();
            let mut all: Vec<Vec<u8>> = row
                .loaded
                .values()
                .filter(|entry| !judged.iter().any(|j| j.file == entry.file))
                .filter_map(|entry| serde_json::to_vec(entry).ok())
                .collect();
            all.extend(fresh);
            self.cache
                .put_bytes(row.key, [b"[", all.join(&b',').as_slice(), b"]"].concat());
        }
    }
}

/// What a unit was judged with besides the project: the applicability and the
/// options the configuration resolved. The options are a sorted map, so their
/// JSON is the same for the same options.
fn tag(applies: Applicability, options: &Options) -> String {
    let scope = match applies.tests {
        TestScope::Exclude => '0',
        TestScope::Include => '1',
        TestScope::Only => '2',
    };
    let mut tag = String::from(if applies.generated { '1' } else { '0' });
    tag.push(scope);
    if !options.is_empty() {
        tag.push_str(&serde_json::to_string(options).unwrap_or_default());
    }
    tag
}
