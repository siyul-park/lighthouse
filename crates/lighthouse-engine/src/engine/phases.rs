//! The phases of a run between reading files and collecting findings: read the
//! files, have the providers index them, compute the facts and apply the rules.

use lighthouse_model::ucm::Model;
use std::{borrow::Cow, sync::OnceLock};

use lighthouse_model::ucm::Capability;
use lighthouse_plugin::RuleManifest;

use rayon::ThreadPool;

use super::{project::Units, regions::Views, *};
use crate::graph::Detected;
use lighthouse_plugin::ANALYSIS;
use lighthouse_spec::{Attributes, Content, Readable};

/// Why a file the languages claim is not an input: a notice for binary data a
/// fallback provider claims, otherwise a gap in the analysis.
type EmbeddedTargets = BTreeMap<usize, BTreeMap<PathBuf, EmbeddedFile>>;

enum Skipped {
    Notice(String),
    Incomplete(Incomplete),
}

/// Runs `op` on the worker threads the independent steps of a run share. A
/// worker gets the stack the main thread has, as the expressions of a check
/// recurse deeply. If such workers cannot start, workers with the default
/// stack do, and failing that `op` runs on the global pool.
pub(crate) fn in_pool<R: Send>(op: impl FnOnce() -> R + Send) -> R {
    const STACK: usize = 8 << 20;
    static POOL: OnceLock<Option<ThreadPool>> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        let named =
            || rayon::ThreadPoolBuilder::new().thread_name(|n| format!("lighthouse-worker-{n}"));
        named()
            .stack_size(STACK)
            .build()
            .or_else(|_| named().build())
            .ok()
    });
    match pool {
        Some(pool) => pool.install(op),
        None => op(),
    }
}

impl Engine {
    /// Makes an input of every file a listed language claims, with the hash
    /// the graph has of it, side by side. The text is read when first used.
    pub(super) fn read(
        &self,
        detected: &Detected,
        overlays: &Overlays,
        run: &mut Run,
    ) -> Vec<Input> {
        let listing = &detected.listing;
        for e in &listing.errors {
            run.incomplete.push(Incomplete {
                path: None,
                provider: None,
                reason: format!("walk: {e}"),
            });
        }
        let read: Vec<Result<Input, Skipped>> = in_pool(|| {
            listing
                .files
                .par_iter()
                .filter_map(|rel| {
                    let content = detected.contents.get(rel)?;
                    self.read_input(rel, content, &listing.attributes, overlays)
                })
                .collect()
        });
        let mut inputs = Vec::new();
        for read in read {
            match read {
                Ok(input) => inputs.push(input),
                Err(Skipped::Notice(notice)) => {
                    run.notices.insert(notice);
                }
                Err(Skipped::Incomplete(gap)) => run.incomplete.push(gap),
            }
        }
        inputs.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        inputs
    }

    /// Whether a listed language claims the file at `rel`.
    pub(super) fn claimed(&self, rel: &Path) -> bool {
        self.providers
            .iter()
            .any(|&i| self.languages[i].claims(rel))
    }

    /// The input of one file if a listed language claims it, from its overlay
    /// when it has one; a file that cannot be read is not an input.
    fn read_input(
        &self,
        rel: &Path,
        content: &Content,
        attributes: &Attributes,
        overlays: &Overlays,
    ) -> Option<Result<Input, Skipped>> {
        let &language = self
            .providers
            .iter()
            .find(|&&i| self.languages[i].claims(rel))?;
        let text = match (overlays.get(rel), &content.readable) {
            (Some(text), _) => Text::held(text.clone()),
            (None, Readable::Text) => Text::at(self.ws.root.join(rel)),
            (None, Readable::NotUtf8) => {
                return Some(Err(if self.provider(language).manifest().fallback {
                    Skipped::Notice(format!("{}: skipped, not valid UTF-8", rel.display()))
                } else {
                    Skipped::Incomplete(Incomplete {
                        path: Some(rel.to_owned()),
                        provider: None,
                        reason: "not valid UTF-8".to_owned(),
                    })
                }));
            }
            (None, Readable::Unreadable(why)) => {
                return Some(Err(Skipped::Incomplete(Incomplete {
                    path: Some(rel.to_owned()),
                    provider: None,
                    reason: format!("unreadable: {why}"),
                })));
            }
            (None, Readable::Missing) => {
                return Some(Err(Skipped::Incomplete(Incomplete {
                    path: Some(rel.to_owned()),
                    provider: None,
                    reason: "unreadable: the file is gone".to_owned(),
                })));
            }
        };
        let file = File {
            path: rel.to_owned(),
            lang: self.provider(language).manifest().id.clone(),
            hash: content.sha256.clone(),
            generated: self.generated.names(rel) || attributes.is_generated(rel),
            test: self.languages[language].tests.is_match(rel),
            facts: Default::default(),
        };
        Some(Ok(Input {
            file,
            text,
            language,
        }))
    }

    pub(super) fn provider(&self, language: usize) -> &dyn LanguageProvider {
        self.registry.languages()[language].provider.as_ref()
    }

    /// The provider of the language `id`, if one is registered.
    pub(crate) fn language(&self, id: &str) -> Option<&dyn LanguageProvider> {
        self.registry
            .languages()
            .iter()
            .map(|l| l.provider.as_ref())
            .find(|provider| provider.manifest().id == id)
    }

    /// Whether the provider of `language` declares `capability`.
    pub(crate) fn provides(&self, language: &str, capability: Capability) -> bool {
        self.language(language)
            .is_some_and(|provider| provider.manifest().capabilities.contains(&capability))
    }

    /// Has each provider index its files in one call, the providers side by
    /// side, and merges the fragments. The files that host source regions
    /// (an injection selects them) go first, so that a provider that analyzes
    /// regions is asked once, for its own files and for the regions together:
    /// what it loads for one it loads for both. Files a provider did not index
    /// drop out; the reason is recorded as incomplete.
    pub(super) fn build_project(
        &self,
        inputs: Vec<Input>,
        overlays: &Overlays,
        (key, run): (Option<&str>, &mut Run),
    ) -> Option<(Vec<Input>, Project)> {
        let ws = Workspace {
            overlays: overlays.clone(),
            ..self.ws.clone()
        };
        let hosts = self.hosts();
        let (host_calls, mut file_calls) = self.plan_units(&inputs, hosts.as_ref(), run);
        let units = Units::of(&host_calls, &file_calls);
        let mut sources = Vec::new();
        self.index_calls(&ws, host_calls, Some(&mut sources), run);
        let named = |name: &str| {
            let id = self.registry.language_named(name)?;
            self.providers
                .iter()
                .any(|&index| self.provider(index).manifest().id == id)
                .then(|| id.to_owned())
        };
        let regions = injections::select(self.config.injections(), sources, &inputs, &named, run);
        let target_files = self.route_embedded(regions, run);
        let views = self.view_regions(&target_files);
        for (target, files) in target_files {
            file_calls.entry(target).or_default().regions = files.into_values().collect();
        }
        self.index_calls(&ws, file_calls, None, run);
        // Nothing the providers had to be asked about moved: the run reports
        // what the last one did on this key.
        if key.is_some_and(|key| {
            run.timings.sent.iter().all(|(_, asked)| *asked == 0) && self.recall(key, run)
        }) {
            self.commit_index(run);
            return None;
        }
        let started = Instant::now();
        let project = self.assemble(&inputs, (&units, &views), run);
        run.timings.merge = started.elapsed();
        self.commit_index(run);
        run.notices.extend(project.notices().iter().cloned());
        let kept = inputs
            .into_iter()
            .filter(|i| project.file(&i.file.path).is_some())
            .collect();
        Some((kept, project))
    }

    /// The files injections select, `None` when none does.
    fn hosts(&self) -> Option<globset::GlobSet> {
        if self.ws.source_globs.is_empty() {
            return None;
        }
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in &self.ws.source_globs {
            builder.add(globset::Glob::new(pattern).expect("source globs are validated by Config"));
        }
        Some(
            builder
                .build()
                .expect("source globs are validated by Config"),
        )
    }

    fn route_embedded(&self, embedded: Vec<EmbeddedFile>, run: &mut Run) -> EmbeddedTargets {
        let providers: BTreeMap<_, _> = self
            .providers
            .iter()
            .map(|&index| (self.provider(index).manifest().id.clone(), index))
            .collect();
        let mut target_files: EmbeddedTargets = BTreeMap::new();
        for file in embedded {
            if file.regions.is_empty() {
                continue;
            }
            let path = file.file.path.clone();
            let mut ids = BTreeSet::new();
            let mut by_provider: BTreeMap<usize, Vec<lighthouse_plugin::EmbeddedRegion>> =
                BTreeMap::new();
            for source in file.regions {
                if source.id.is_empty()
                    || source.injection_id.is_empty()
                    || source.provider.is_empty()
                    || !ids.insert(source.id.clone())
                {
                    run.incomplete.push(Incomplete {
                        path: Some(path.clone()),
                        provider: None,
                        reason: "empty or duplicate embedded source identity/provider".to_owned(),
                    });
                    continue;
                }
                if let Err(reason) = source.validate(&file.text) {
                    run.incomplete.push(Incomplete {
                        path: Some(path.clone()),
                        provider: None,
                        reason: format!("embedded source {} is invalid: {reason}", source.id),
                    });
                    continue;
                }
                let Some(&index) = providers.get(&source.provider) else {
                    run.incomplete.push(Incomplete {
                        path: Some(path.clone()),
                        provider: None,
                        reason: format!(
                            "source {} selects provider {}, but that provider is not enabled",
                            source.id, source.provider
                        ),
                    });
                    continue;
                };
                by_provider.entry(index).or_default().push(source);
            }
            for (provider, sources) in by_provider {
                let entries = target_files.entry(provider).or_default();
                if entries.contains_key(&path) {
                    run.incomplete.push(Incomplete {
                        path: Some(path.clone()),
                        provider: None,
                        reason: "multiple embedded selections for one host file and provider"
                            .to_owned(),
                    });
                    continue;
                }
                entries.insert(
                    path.clone(),
                    EmbeddedFile {
                        file: file.file.clone(),
                        text: file.text.clone(),
                        regions: sources,
                    },
                );
            }
        }
        target_files
    }

    /// What the injections made of the host files: the languages their
    /// regions are in, and whether each is example code.
    fn view_regions(&self, target_files: &EmbeddedTargets) -> Views {
        let mut views = Views::default();
        for (target, files) in target_files {
            let language = self.provider(*target).manifest().id.clone();
            for (path, file) in files {
                views.add(path, &language, &file.regions);
            }
        }
        views
    }

    /// Computes the facts the rules read, analyzer by analyzer in dependency
    /// order, as the queries of `graph`: an analyzer is run again only for
    /// the files whose model, text or required facts changed. The files of a
    /// file-scope analyzer are pulled side by side.
    pub(super) fn analyze(
        &self,
        graph: &Graph,
        (rules, inputs): (&[&dyn Rule], &[Input]),
        timings: &mut Timings,
    ) -> Result<Facts, Error> {
        let wanted = rules
            .iter()
            .flat_map(|r| r.manifest().analyzers.iter().map(String::as_str));
        let pull = |id: &str, key: String| -> Result<Value, Error> {
            let json = graph.query(&ANALYSIS, &(id.to_owned(), key))?;
            serde_json::from_str(&json)
                .map_err(|e| Error::Graph(lighthouse_graph::Error::Encode(e.to_string())))
        };
        let mut facts = Facts::new();
        for analyzer in self.registry.order(wanted)? {
            let started = Instant::now();
            let id = &analyzer.manifest().id;
            match analyzer.manifest().scope {
                RunScope::Project => {
                    facts.insert(id, String::new(), pull(id, String::new())?);
                }
                RunScope::File => {
                    let keys: Vec<String> = inputs
                        .iter()
                        .map(|input| input.file.path.to_string_lossy().into_owned())
                        .collect();
                    let found: Vec<_> =
                        in_pool(|| keys.par_iter().map(|key| pull(id, key.clone())).collect());
                    for (key, fact) in keys.into_iter().zip(found) {
                        facts.insert(id, key, fact?);
                    }
                }
            }
            timings.analyzers.push((id.clone(), started.elapsed()));
        }
        Ok(facts)
    }

    /// Judges the directives (`said`), which the rules that read them
    /// (`after_suppression`) see once the findings of every other rule are
    /// known. What they find is not itself suppressed.
    pub(super) fn judge_annotations(
        &self,
        scene: &Scene,
        mut said: BTreeMap<PathBuf, Vec<Value>>,
        run: &mut Run,
    ) -> Result<Vec<Diagnostic>, Error> {
        if !scene.rules.iter().any(|r| r.after_suppression()) {
            return Ok(Vec::new());
        }
        let annotations = scene.annotations;
        let mut with_directives = Vec::new();
        for input in scene.inputs {
            let path = &input.file.path;
            let directives = said.remove(path).unwrap_or_default();
            if !directives.is_empty() {
                with_directives.push(input);
            }
            annotations.insert(path.clone(), directives);
        }
        // A rule that reads the directives judges them: a file that has none
        // has nothing for it to find.
        let all = self.judge_directives(scene, &with_directives, &mut run.timings)?;
        run.notices.extend(all.notices);
        run.incomplete.extend(all.incomplete);
        run.timings.add_rules(all.times);
        Ok(all.found)
    }

    /// How `rule` is to judge `file` as the configuration sets it: its
    /// options, level and applicability; `None` when it is off, leaves the
    /// file out or cannot read it (a provider without a capability it needs,
    /// which is noticed and left unsettled).
    pub(super) fn judged<'a>(
        &'a self,
        project: &Project,
        rule: &dyn Rule,
        resolved: &'a lighthouse_spec::Rules,
        file: &File,
        gathered: &mut Gathered,
    ) -> Option<(Cow<'a, Options>, Severity, Applicability)> {
        let meta = rule.manifest();
        let config = resolved.get(&meta.id)?;
        let level = config.level?;
        let applies = self.applies(&meta.applicability, config);
        if applies.excludes(project, file) {
            return None;
        }
        if let Some((language, missing)) = self.lacking(meta, &applies, file) {
            gathered.notices.insert(format!(
                "{}: skipped for language `{language}`, missing capability {missing}",
                meta.id
            ));
            gathered
                .unsettled
                .insert((meta.id.clone(), Some(file.path.clone())));
            return None;
        }
        let options = options_at(&self.inherited, resolved, &meta.id, &config.options);
        Some((options, level, applies))
    }

    /// The first language the rule looks at in `file` whose provider lacks a
    /// capability the rule requires, and the capability: the language of the
    /// file itself, when the rule covers it, and that of each of its regions
    /// the rule covers.
    fn lacking(
        &self,
        meta: &RuleManifest,
        applies: &Applicability,
        file: &File,
    ) -> Option<(String, Capability)> {
        let host = (applies.admits_language(&file.lang)
            || applies.admits_language(file.analysis_language()))
        .then_some(file.lang.as_str());
        let regions = file
            .regions()
            .filter(|(language, example)| {
                applies.admits_language(language) && applies.examples.admits(*example)
            })
            .map(|(language, _)| language);
        host.into_iter().chain(regions).find_map(|language| {
            meta.capabilities
                .iter()
                .find(|capability| !self.provides(language, **capability))
                .map(|capability| (language.to_owned(), *capability))
        })
    }

    /// What code a rule may report on at a file: the decision's scope, unless
    /// the project says otherwise for this rule (`generated`).
    pub(super) fn applies(&self, declared: &Applicability, rule: &RuleConfig) -> Applicability {
        Applicability {
            generated: rule.generated.unwrap_or(declared.generated),
            ..declared.clone()
        }
    }
}

/// A finding at the level the configuration gives its rule; a subject nothing
/// decided is only `info`.
pub(super) fn leveled(mut d: Diagnostic, level: Severity) -> Diagnostic {
    d.severity = if d.review.is_some() {
        Severity::Info
    } else {
        level
    };
    d
}

/// A finding that came from a model's answer: it is not worth an `error`.
pub(super) fn answered(d: Diagnostic, level: Severity) -> Diagnostic {
    let level = match level {
        Severity::Error => Severity::Warn,
        other => other,
    };
    leveled(d, level)
}
