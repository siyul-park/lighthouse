//! Reusing what a rule found: the key of a unit (a rule over one file, or over
//! the project), and the run of a rule that looks the unit up first.

use lighthouse_cache::{Cache, Digest, Digests, Key, KeyBuilder, Stored};
use lighthouse_model::Reach;
use lighthouse_plugin::Caching;

use super::*;

/// The cache a run reads and writes, with the digests its keys are built from.
pub(super) struct RunCache {
    pub(super) cache: Cache,
    digests: Digests,
    documents: Digest,
    /// What every key depends on: the build, the plugins and the languages.
    base: Vec<u8>,
}

/// What a unit depends on besides the project, spelled out for the key.
struct Unit<'a> {
    rule: &'a str,
    caching: &'a Caching,
    options: String,
    applies: String,
}

impl RunCache {
    pub(super) fn new(cache: Cache, digests: Digests, documents: Digest, base: Vec<u8>) -> Self {
        Self {
            cache,
            digests,
            documents,
            base,
        }
    }

    /// The key of a unit over `file` (the whole project for a project rule);
    /// `None` when the file is not one the digests know.
    fn key(&self, unit: &Unit, file: Option<&File>) -> Option<Key> {
        let mut key = KeyBuilder::new();
        key.part(&self.base)
            .part(unit.rule)
            .part(&unit.caching.revision)
            .part(&unit.options)
            .part(&unit.applies);
        let reach = if file.is_some() {
            unit.caching.reach
        } else {
            Reach::Global
        };
        key.part(reach.as_str());
        if let Some(file) = file {
            key.part(file.path.as_os_str().as_encoded_bytes());
        }
        match (file, reach) {
            (Some(file), Reach::Local | Reach::Neighbors) => {
                let digests = self.digests.file(&file.path)?;
                key.part(digests.slice);
                if reach == Reach::Neighbors {
                    key.part(digests.neighbors);
                }
            }
            _ => {
                key.part(self.digests.project()).part(self.documents);
            }
        }
        Some(key.finish())
    }
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
        let Some((run, caching)) = scene.cache.zip(rule.caching()) else {
            return rule.check(ctx, options);
        };
        let unit = Unit {
            rule: &rule.manifest().id,
            caching: &caching,
            options: canonical(&Value::Object(options.clone())),
            applies: format!("{:?}", ctx.applies),
        };
        let Some(key) = run.key(&unit, ctx.file.map(|(file, _)| file)) else {
            return rule.check(ctx, options);
        };
        if let Some(entry) = run.cache.get(&key) {
            entry.notices.into_iter().for_each(|n| ctx.notices.push(n));
            return Ok(entry.diagnostics);
        }
        let found = rule.check(ctx, options)?;
        let notices = ctx.notices.take();
        notices.iter().for_each(|n| ctx.notices.push(n.clone()));
        run.cache.put(
            key,
            &Stored {
                diagnostics: found.clone(),
                notices,
            },
        );
        Ok(found)
    }

    /// Opens the cache for a run over `project` when the engine has one, and
    /// hashes the project for its keys; what goes wrong is a notice and the
    /// run goes on without.
    pub(super) fn open_cache(
        &self,
        project: &Project,
        notices: &mut BTreeSet<String>,
    ) -> Option<RunCache> {
        let settings = self.cache.as_ref()?;
        let opened = Cache::open(&settings.dir, settings.limit).and_then(|cache| {
            let digests = Digests::of(project)?;
            let documents = lighthouse_cache::documents(project.documents())?;
            Ok(RunCache::new(cache, digests, documents, self.cache_base()))
        });
        match opened {
            Ok(run) => Some(run),
            Err(e) => {
                notices.insert(format!("the result cache is not used: {e}"));
                None
            }
        }
    }

    /// What every key of this engine depends on besides its rule: the build,
    /// the plugins, and how the languages are configured.
    fn cache_base(&self) -> Vec<u8> {
        let plugins: Vec<String> = self
            .registry
            .manifests()
            .iter()
            .map(|m| format!("{}@{}", m.id, m.version))
            .collect();
        let languages = serde_json::to_value(&self.ws.languages).unwrap_or_default();
        let constructors = serde_json::to_value(&self.ws.constructors).unwrap_or_default();
        format!(
            "{}\n{}\n{}\n{}",
            lighthouse_cache::build_identity(),
            plugins.join(","),
            canonical(&languages),
            canonical(&constructors),
        )
        .into_bytes()
    }

    /// Writes what the run stored and used, and says how it went.
    pub(super) fn close_cache(
        &self,
        run: &RunCache,
        timings: &mut Timings,
        notices: &mut BTreeSet<String>,
    ) {
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

/// The JSON of `value` with the keys of every object in order.
pub(super) fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|k| (k.clone(), sorted(&map[k])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(value).to_string()
}
