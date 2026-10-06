use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;
use lighthouse_config::{Config, GlobSet, glob_set};
use lighthouse_model::{Diagnostic, File, Fragment, Incomplete, Project, Severity};
use lighthouse_plugin::{Ctx, Facts, LanguageProvider, Registry, Rule, Scope, Source, Workspace};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] lighthouse_config::Error),
    #[error(transparent)]
    Plugin(#[from] lighthouse_plugin::Error),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    #[error("`{0}` belongs to a plugin not listed in `plugins`")]
    PluginNotListed(String),
    #[error("unknown rule `{0}`")]
    UnknownRule(String),
    #[error("rule `{0}` is not enabled by the configuration")]
    RuleNotEnabled(String),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error {
    |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// Process exit code of a run whose analysis was incomplete.
pub const EXIT_INCOMPLETE: u8 = 3;

#[derive(Debug, Default)]
pub struct Outcome {
    /// Sorted by file, line, column, rule; limited to the requested paths.
    pub diagnostics: Vec<Diagnostic>,
    /// Facts the user should know that leave the analysis complete: rules
    /// skipped for a missing provider capability, build-variant duplicates,
    /// ambiguous edge targets, provider messages.
    pub notices: BTreeSet<String>,
    /// Parts of the analysis scope that were not analyzed, sorted. Covers the
    /// whole project whatever paths were requested: paths only filter what is
    /// reported, not what is analyzed. Unreadable files, files a language
    /// provider could not parse or load, provider crashes and timeouts, and
    /// requested paths outside the root are incomplete. A non-UTF-8 file
    /// claimed only by a fallback provider (binary data) is a notice.
    pub incomplete: Vec<Incomplete>,
}

impl Outcome {
    /// Process exit code: 3 if the analysis is incomplete, unless
    /// `allow_incomplete`; else 1 if any error diagnostic, or any warning
    /// when `strict`; otherwise 0. `review` and `info` never fail a run. The
    /// CLI exits 2 on usage and runtime errors.
    pub fn exit_code(&self, strict: bool, allow_incomplete: bool) -> u8 {
        if !self.incomplete.is_empty() && !allow_incomplete {
            return EXIT_INCOMPLETE;
        }
        let fails = |s: Severity| s == Severity::Error || (strict && s == Severity::Warn);
        u8::from(self.diagnostics.iter().any(|d| fails(d.severity)))
    }
}

struct Language {
    files: GlobSet,
    tests: GlobSet,
}

struct Input {
    file: File,
    text: String,
    language: usize,
}

pub struct Engine {
    registry: Registry,
    config: Config,
    ws: Workspace,
    /// Providers from listed plugins; indexes into `languages`.
    providers: Vec<usize>,
    languages: Vec<Language>,
    /// Rules enabled by the configuration for at least one file.
    active: BTreeSet<String>,
    /// Gaps known before the run, such as a plugin that failed to start.
    startup: Vec<Incomplete>,
}

impl Engine {
    /// `root` is the directory of `lighthouse.toml`; globs match paths relative to it.
    pub fn new(registry: Registry, config: Config, root: &Path) -> Result<Self, Error> {
        for plugin in config.plugins() {
            if !registry.has_plugin(&plugin.id) {
                return Err(Error::UnknownPlugin(plugin.id.clone()));
            }
        }
        registry.validate()?;
        let listed = |id: &str| config.lists(lighthouse_plugin::plugin_of(id));
        let mut entries: Vec<_> = config
            .configured()
            .map(|(id, c)| (id.to_owned(), c.options.clone()))
            .collect();
        for preset in config.extends() {
            let preset = registry
                .preset(preset)
                .ok_or_else(|| lighthouse_config::Error::UnknownPreset(preset.clone()))?;
            if !listed(&preset.id) {
                return Err(Error::PluginNotListed(preset.id.clone()));
            }
            entries.extend(
                preset
                    .rules
                    .iter()
                    .map(|(id, c)| (id.clone(), c.options.clone())),
            );
        }
        for (id, options) in &entries {
            let rule = registry
                .rule(id)
                .ok_or_else(|| Error::UnknownRule(id.clone()))?;
            if !listed(id) {
                return Err(Error::PluginNotListed(id.clone()));
            }
            rule.validate(options)?;
        }

        let mut providers = Vec::new();
        let mut languages = Vec::new();
        for (plugin, provider) in registry.languages() {
            if config.lists(plugin) {
                providers.push(languages.len());
            }
            languages.push(Language {
                files: glob_set(provider.globs())?,
                tests: glob_set(provider.conventions().test_globs)?,
            });
        }
        let ws = Workspace {
            root: root.canonicalize().map_err(io_error(root))?,
            languages: config.languages().clone(),
        };
        let mut engine = Self {
            registry,
            config,
            ws,
            providers,
            languages,
            active: BTreeSet::new(),
            startup: Vec::new(),
        };
        engine.active = engine.active_rules()?;
        Ok(engine)
    }

    /// Adds gaps found while assembling the plugins; every run reports them.
    pub fn with_incomplete(mut self, gaps: Vec<Incomplete>) -> Self {
        self.startup.extend(gaps);
        self
    }

    /// Checks the whole project, then reports diagnostics under `paths`.
    /// `only` restricts which rules run; empty means all enabled rules.
    pub fn check(&self, paths: &[PathBuf], only: &[String]) -> Result<Outcome, Error> {
        for id in only {
            if self.registry.rule(id).is_none() {
                return Err(Error::UnknownRule(id.clone()));
            }
            if !self.active.contains(id) {
                return Err(Error::RuleNotEnabled(id.clone()));
            }
        }
        let selected: Vec<&dyn Rule> = self
            .active
            .iter()
            .filter(|id| only.is_empty() || only.contains(id))
            .filter_map(|id| self.registry.rule(id))
            .collect();

        let mut outcome = Outcome::default();
        let mut incomplete = self.startup.clone();
        let scopes = self.scopes(paths, &mut incomplete)?;
        let inputs = self.read(&mut outcome.notices, &mut incomplete);
        let (inputs, project) = self.index(inputs, &mut outcome.notices, &mut incomplete);
        let facts = self.analyze(&selected, &inputs, &project)?;
        let mut found = self.apply(&selected, &inputs, &project, &facts, &mut outcome.notices)?;

        found.retain(|d| scopes.iter().any(|s| d.file.starts_with(s)));
        found.sort_by(|a, b| {
            (&a.file, a.span.start, &a.rule_id).cmp(&(&b.file, b.span.start, &b.rule_id))
        });
        let mut seen: BTreeMap<_, usize> = BTreeMap::new();
        for d in &mut found {
            let n = seen
                .entry((d.rule_id.clone(), d.file.clone(), d.fingerprint.clone()))
                .or_default();
            d.fingerprint = d.fingerprint.occurrence(*n);
            *n += 1;
        }
        outcome.diagnostics = found;
        incomplete.sort();
        incomplete.dedup();
        outcome.incomplete = incomplete;
        Ok(outcome)
    }

    fn active_rules(&self) -> Result<BTreeSet<String>, Error> {
        let presets = |id: &str| self.registry.preset(id).map(|p| p.rules.clone());
        let mut active: BTreeSet<String> = self
            .config
            .resolve(Path::new(""), "", &presets)?
            .into_iter()
            .filter_map(|(id, c)| c.level.map(|_| id))
            .collect();
        let overridden = self.config.configured().filter(|(_, c)| c.level.is_some());
        active.extend(overridden.map(|(id, _)| id.to_owned()));
        Ok(active)
    }

    /// Report paths relative to the root; those outside it are incomplete.
    fn scopes(
        &self,
        paths: &[PathBuf],
        incomplete: &mut Vec<Incomplete>,
    ) -> Result<Vec<PathBuf>, Error> {
        if paths.is_empty() {
            return Ok(vec![PathBuf::new()]);
        }
        let mut scopes = Vec::new();
        for path in paths {
            let abs = path.canonicalize().map_err(io_error(path))?;
            match abs.strip_prefix(&self.ws.root) {
                Ok(rel) => scopes.push(rel.to_owned()),
                Err(_) => incomplete.push(Incomplete {
                    path: Some(path.clone()),
                    reason: format!("outside {}, not checked", self.ws.root.display()),
                }),
            }
        }
        Ok(scopes)
    }

    /// Reads every file under the root that a listed language claims.
    fn read(&self, notices: &mut BTreeSet<String>, incomplete: &mut Vec<Incomplete>) -> Vec<Input> {
        let mut inputs = Vec::new();
        for entry in WalkBuilder::new(&self.ws.root).require_git(false).build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    incomplete.push(Incomplete {
                        path: None,
                        reason: format!("walk: {e}"),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&self.ws.root) else {
                incomplete.push(Incomplete {
                    path: Some(entry.path().to_owned()),
                    reason: "outside the root, not checked".to_owned(),
                });
                continue;
            };
            let Some(&language) = self
                .providers
                .iter()
                .find(|&&i| self.languages[i].files.is_match(rel))
            else {
                continue;
            };
            let text = match fs::read_to_string(entry.path()) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    if self.provider(language).fallback() {
                        notices.insert(format!("{}: skipped, not valid UTF-8", rel.display()));
                    } else {
                        incomplete.push(Incomplete {
                            path: Some(rel.to_owned()),
                            reason: "not valid UTF-8".to_owned(),
                        });
                    }
                    continue;
                }
                Err(e) => {
                    incomplete.push(Incomplete {
                        path: Some(rel.to_owned()),
                        reason: format!("unreadable: {e}"),
                    });
                    continue;
                }
            };
            let file = File {
                path: rel.to_owned(),
                lang: self.provider(language).id().to_owned(),
                hash: Sha256::digest(text.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                generated: false,
                test: self.languages[language].tests.is_match(rel),
            };
            inputs.push(Input {
                file,
                text,
                language,
            });
        }
        inputs.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        inputs
    }

    fn provider(&self, language: usize) -> &dyn LanguageProvider {
        self.registry
            .languages()
            .nth(language)
            .map(|(_, l)| l)
            .expect("language index comes from the registry")
    }

    /// Indexes each provider's files in one batch. Files a provider did not
    /// index drop out; the reason is recorded as incomplete.
    fn index(
        &self,
        inputs: Vec<Input>,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> (Vec<Input>, Project) {
        let mut batches: BTreeMap<usize, Vec<&Input>> = BTreeMap::new();
        for input in &inputs {
            batches.entry(input.language).or_default().push(input);
        }
        let mut parts: Vec<Fragment> = Vec::new();
        for (language, batch) in batches {
            let provider = self.provider(language);
            let sources: Vec<Source> = batch
                .iter()
                .map(|i| Source {
                    file: &i.file,
                    text: &i.text,
                })
                .collect();
            match provider.index(&self.ws, &sources) {
                Ok(indexed) => {
                    parts.extend(indexed.fragments);
                    notices.extend(indexed.notices);
                    incomplete.extend(indexed.incomplete);
                }
                Err(e) => incomplete.push(Incomplete {
                    path: None,
                    reason: format!(
                        "language `{}` failed, {} file(s) not analyzed: {e}",
                        provider.id(),
                        batch.len()
                    ),
                }),
            }
        }
        let project = Project::merge(parts);
        notices.extend(project.notices().iter().cloned());
        let kept = inputs
            .into_iter()
            .filter(|i| project.file(&i.file.path).is_some())
            .collect();
        (kept, project)
    }

    fn analyze(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
    ) -> Result<Facts, Error> {
        let wanted = rules
            .iter()
            .flat_map(|r| r.meta().analyzers.iter().map(String::as_str));
        let mut facts = Facts::new();
        for analyzer in self.registry.order(wanted)? {
            let runs: Vec<(Option<&Input>, String)> = match analyzer.scope() {
                Scope::Project => vec![(None, String::new())],
                Scope::File => inputs
                    .iter()
                    .map(|i| (Some(i), i.file.path.to_string_lossy().into_owned()))
                    .collect(),
            };
            for (input, key) in runs {
                let ctx = Ctx {
                    ws: &self.ws,
                    project,
                    file: input.map(|i| (&i.file, i.text.as_str())),
                    facts: &facts,
                };
                let fact = analyzer.run(&ctx)?;
                facts.insert((analyzer.id().to_owned(), key), fact);
            }
        }
        Ok(facts)
    }

    fn apply(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
        facts: &Facts,
        notices: &mut BTreeSet<String>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let presets = |id: &str| self.registry.preset(id).map(|p| p.rules.clone());
        let mut found = Vec::new();
        let languages: BTreeSet<usize> = inputs.iter().map(|i| i.language).collect();

        for input in inputs {
            let resolved = self
                .config
                .resolve(&input.file.path, &input.file.lang, &presets)?;
            let provider = self.provider(input.language);
            for rule in rules.iter().filter(|r| r.meta().scope == Scope::File) {
                let meta = rule.meta();
                let Some(config) = resolved.get(&meta.id) else {
                    continue;
                };
                let Some(level) = config.level else { continue };
                if let Some(missing) = meta
                    .capabilities
                    .iter()
                    .find(|c| !provider.capabilities().contains(c))
                {
                    notices.insert(format!(
                        "{}: skipped for language `{}`, missing capability {missing}",
                        meta.id, input.file.lang
                    ));
                    continue;
                }
                let ctx = Ctx {
                    ws: &self.ws,
                    project,
                    file: Some((&input.file, input.text.as_str())),
                    facts,
                };
                for mut d in rule.check(&ctx, &config.options)? {
                    d.severity = level;
                    found.push(d);
                }
            }
        }

        let resolved = self.config.resolve(Path::new(""), "", &presets)?;
        for rule in rules.iter().filter(|r| r.meta().scope == Scope::Project) {
            let meta = rule.meta();
            let Some(config) = resolved.get(&meta.id) else {
                continue;
            };
            let Some(level) = config.level else { continue };
            let missing = meta.capabilities.iter().find(|c| {
                languages
                    .iter()
                    .any(|&l| !self.provider(l).capabilities().contains(c))
            });
            if let Some(missing) = missing {
                notices.insert(format!(
                    "{}: skipped, missing capability {missing}",
                    meta.id
                ));
                continue;
            }
            let ctx = Ctx {
                ws: &self.ws,
                project,
                file: None,
                facts,
            };
            for mut d in rule.check(&ctx, &config.options)? {
                d.severity = level;
                found.push(d);
            }
        }
        Ok(found)
    }
}
