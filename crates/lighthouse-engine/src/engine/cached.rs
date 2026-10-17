//! Reusing what a rule found: the key of a unit (a rule over one file, or over
//! the project), and the run of a rule that looks the unit up first.
//!
//! What is the same for every unit of a rule (the build, the rule, its
//! revision and reach) is absorbed once into a prefix; a unit adds its tag
//! (applicability and resolved options) and the digests its reach asks for.
//! A rule that reads the whole project and runs per file keeps one row per
//! project state, holding every file's findings, instead of one entry per file.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock, PoisonError},
};

use crate::cache::{Cache, Digest, Digests, KeyBuilder, Stored};
use lighthouse_model::{Reach, TestScope};
use lighthouse_plugin::Models;
use serde::{Deserialize, Serialize};

use super::*;

/// The cache a run reads and writes, with the digests its keys are built from.
pub(super) struct RunCache<'c> {
    cache: &'c Cache,
    digests: Digests,
    documents_digest: Digest,
    prefixes: HashMap<String, Prefix>,
}

/// What is known of a rule before any unit runs.
struct Prefix {
    key: KeyBuilder,
    reach: Reach,
    positions: bool,
    /// The row of a global rule that runs per file, read on first use.
    row: OnceLock<Row>,
}

/// The findings of a global rule over every file of one project state.
struct Row {
    key: Digest,
    loaded: HashMap<PathBuf, Entry<'static>>,
    /// The entries judged this run, as written, by file.
    fresh: Mutex<Vec<(PathBuf, Vec<u8>)>>,
}

/// One file of a row: the tag says which options and applicability it was
/// judged with.
#[derive(Serialize, Deserialize)]
struct Entry<'a> {
    file: Cow<'a, Path>,
    tag: String,
    found: Stored<'a>,
}

impl Engine {
    /// Runs `rule` for the subject `ctx` focuses (a file, else the project),
    /// or takes what an earlier run found for the same subject in the same
    /// state. What fails or cannot finish is never stored.
    pub(super) fn run_rule(
        &self,
        scene: &Scene,
        rule: &dyn Rule,
        ctx: &Context,
        options: &Options,
    ) -> Result<Vec<Diagnostic>, lighthouse_plugin::Error> {
        let prefix = scene
            .cache
            .and_then(|run| Some((run, run.prefixes.get(&rule.manifest().id)?)));
        let Some((run, prefix)) = prefix else {
            return rule.check(ctx, options);
        };
        match ctx.file {
            Some((file, _)) if prefix.reach == Reach::Global => {
                run.in_row(prefix, file, rule, ctx, options)
            }
            _ => run.alone(prefix, rule, ctx, options),
        }
    }

    /// The cache of a run over `project`, with the project hashed for its
    /// keys; what goes wrong is a notice and the run goes on without.
    pub(super) fn run_cache<'c>(
        &self,
        cache: &'c Cache,
        project: &Project,
        selected: &[&dyn Rule],
        models: &dyn Models,
        notices: &mut BTreeSet<String>,
    ) -> Option<RunCache<'c>> {
        let digests = Digests::new(project)
            .and_then(|digests| Ok((digests, crate::cache::documents(project.documents())?)));
        match digests {
            Ok((digests, documents)) => Some(RunCache {
                cache,
                digests,
                documents_digest: documents,
                prefixes: self.prefixes(selected, models),
            }),
            Err(e) => {
                notices.insert(format!("the result cache is not used: {e}"));
                None
            }
        }
    }

    /// The prefix of the keys of each selected rule that may be cached: not one
    /// whose `abstain` route the models are asked about this run.
    fn prefixes(&self, selected: &[&dyn Rule], models: &dyn Models) -> HashMap<String, Prefix> {
        let base = self.cache_base();
        selected
            .iter()
            .filter_map(|rule| {
                let meta = rule.manifest();
                if rule.reviews() && models.asks(&meta.id) {
                    return None;
                }
                let caching = meta.caching.as_ref()?;
                let mut key = base.clone();
                key.part(&meta.id)
                    .part(&caching.revision)
                    .part([caching.reach as u8, u8::from(caching.positions)]);
                let prefix = Prefix {
                    key,
                    reach: caching.reach,
                    positions: caching.positions,
                    row: OnceLock::new(),
                };
                Some((meta.id.clone(), prefix))
            })
            .collect()
    }

    /// What every key of this engine depends on besides its rule: the build,
    /// the plugins, and how the languages are configured.
    fn cache_base(&self) -> KeyBuilder {
        let mut key = KeyBuilder::new();
        key.part(crate::cache::build_identity());
        for plugin in self.registry.manifests() {
            key.part(&plugin.id).part(&plugin.version);
        }
        // Maps of sorted keys: the same configuration is the same text.
        let maps = [
            serde_json::to_string(&self.ws.languages),
            serde_json::to_string(&self.ws.constructors),
            serde_json::to_string(&self.ws.formatters),
        ];
        for map in maps {
            key.part(map.unwrap_or_default());
        }
        key
    }
}

impl RunCache<'_> {
    /// Hands the rows that gained files to the cache.
    pub(super) fn store_rows(&self) {
        for prefix in self.prefixes.values() {
            let Some(row) = prefix.row.get() else {
                continue;
            };
            let fresh =
                std::mem::take(&mut *row.fresh.lock().unwrap_or_else(PoisonError::into_inner));
            if fresh.is_empty() {
                continue;
            }
            // With the files it already held that this run did not judge again.
            let judged: HashSet<&Path> = fresh.iter().map(|(file, _)| file.as_path()).collect();
            let kept: Vec<Vec<u8>> = row
                .loaded
                .values()
                .filter(|entry| !judged.contains(&*entry.file))
                .filter_map(|entry| serde_json::to_vec(entry).ok())
                .collect();
            let entries: Vec<&[u8]> = kept
                .iter()
                .map(Vec::as_slice)
                .chain(fresh.iter().map(|(_, bytes)| bytes.as_slice()))
                .collect();
            self.cache.put_bytes(
                row.key,
                [b"[", entries.join(&b',').as_slice(), b"]"].concat(),
            );
        }
    }

    /// Runs a unit that has an entry of its own.
    fn alone(
        &self,
        prefix: &Prefix,
        rule: &dyn Rule,
        ctx: &Context,
        options: &Options,
    ) -> Result<Vec<Diagnostic>, lighthouse_plugin::Error> {
        let tag = tag(ctx.applies, options);
        let file = ctx.file.map(|(file, _)| file);
        let Some(key) = self.key(prefix, &tag, file) else {
            return rule.check(ctx, options);
        };
        let stored = self.cache.get::<Stored>(&key);
        self.cache.count(stored.is_some());
        if let Some(stored) = stored {
            stored
                .notices
                .into_owned()
                .into_iter()
                .for_each(|n| ctx.notices.push(n));
            return Ok(stored.diagnostics.into_owned());
        }
        let found = rule.check(ctx, options)?;
        let notices = ctx.notices.take();
        self.cache.put(
            key,
            &Stored {
                diagnostics: Cow::Borrowed(&found),
                notices: Cow::Borrowed(&notices),
            },
        );
        notices.into_iter().for_each(|n| ctx.notices.push(n));
        Ok(found)
    }

    /// Runs a unit that is a file's part of a row.
    fn in_row(
        &self,
        prefix: &Prefix,
        file: &File,
        rule: &dyn Rule,
        ctx: &Context,
        options: &Options,
    ) -> Result<Vec<Diagnostic>, lighthouse_plugin::Error> {
        let tag = tag(ctx.applies, options);
        let row = self.row(prefix);
        let loaded = row.loaded.get(&file.path).filter(|entry| entry.tag == tag);
        self.cache.count(loaded.is_some());
        if let Some(entry) = loaded {
            let found = &entry.found;
            found
                .notices
                .iter()
                .for_each(|n| ctx.notices.push(n.clone()));
            return Ok(found.diagnostics.to_vec());
        }
        let found = rule.check(ctx, options)?;
        let notices = ctx.notices.take();
        let entry = Entry {
            file: Cow::Borrowed(&file.path),
            tag: tag.clone(),
            found: Stored {
                diagnostics: Cow::Borrowed(&found),
                notices: Cow::Borrowed(&notices),
            },
        };
        if let Ok(bytes) = serde_json::to_vec(&entry) {
            row.fresh
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((file.path.clone(), bytes));
        }
        notices.into_iter().for_each(|n| ctx.notices.push(n));
        Ok(found)
    }

    /// The key of a unit over `file` (the project for a project rule), by the
    /// reach of its rule; `None` when the file is not one the digests know.
    fn key(&self, prefix: &Prefix, tag: &str, file: Option<&File>) -> Option<Digest> {
        let mut key = prefix.key.clone();
        key.part(tag);
        match (file, prefix.reach) {
            (Some(file), Reach::Local | Reach::Neighbors) => {
                let digests = self.digests.file(&file.path)?;
                key.part(file.path.as_os_str().as_encoded_bytes())
                    .part(digests.slice);
                if prefix.reach == Reach::Neighbors {
                    key.part(if prefix.positions {
                        digests.neighbors_at
                    } else {
                        digests.neighbors
                    });
                }
            }
            _ => {
                key.part(self.digests.project()).part(self.documents_digest);
            }
        }
        Some(key.finish())
    }

    /// The row of a global rule that runs per file.
    fn row<'a>(&self, prefix: &'a Prefix) -> &'a Row {
        prefix.row.get_or_init(|| {
            let mut key = prefix.key.clone();
            key.part(self.digests.project()).part(self.documents_digest);
            let key = key.finish();
            let loaded = self
                .cache
                .get::<Vec<Entry>>(&key)
                .unwrap_or_default()
                .into_iter()
                .map(|entry| (entry.file.to_path_buf(), entry))
                .collect();
            Row {
                key,
                loaded,
                fresh: Mutex::default(),
            }
        })
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
