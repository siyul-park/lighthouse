//! The phases of a run between reading files and collecting findings: read the
//! files, have the providers index them, compute the facts and apply the rules.

use std::sync::OnceLock;

use lighthouse_model::ucm::Capability;

use lighthouse_protocol::hash;
use rayon::ThreadPool;

use super::*;

/// Why a file the languages claim is not an input: a notice for binary data a
/// fallback provider claims, otherwise a gap in the analysis.
enum Skipped {
    Notice(String),
    Incomplete(Incomplete),
}

/// Runs `op` on the worker threads the independent steps of a run share. A
/// worker gets the stack the main thread has, as the expressions of a check
/// recurse deeply. If such workers cannot start, workers with the default
/// stack do, and failing that `op` runs on the global pool.
pub(super) fn in_pool<R: Send>(op: impl FnOnce() -> R + Send) -> R {
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
    /// Reads every file under the root that a listed language claims, side by
    /// side.
    pub(super) fn read(&self, overlays: &Overlays, run: &mut Run) -> Vec<Input> {
        let walk = WalkBuilder::new(&self.ws.root)
            .require_git(false)
            .add_custom_ignore_filename(IGNORE_FILE)
            .build();
        let mut files = Vec::new();
        for entry in walk {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    run.incomplete.push(Incomplete {
                        path: None,
                        reason: format!("walk: {e}"),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            match entry.path().strip_prefix(&self.ws.root) {
                Ok(rel) => files.push(rel.to_owned()),
                Err(_) => run.incomplete.push(Incomplete {
                    path: Some(entry.path().to_owned()),
                    reason: "outside the root, not checked".to_owned(),
                }),
            }
        }
        let read: Vec<Result<Input, Skipped>> = in_pool(|| {
            files
                .par_iter()
                .filter_map(|rel| self.read_input(rel, overlays))
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

    /// Reads one file if a listed language claims it, from its overlay when
    /// it has one; a file that cannot be read is not an input.
    fn read_input(&self, rel: &Path, overlays: &Overlays) -> Option<Result<Input, Skipped>> {
        let &language = self
            .providers
            .iter()
            .find(|&&i| self.languages[i].files.is_match(rel))?;
        let text = match overlays.get(rel) {
            Some(text) => text.clone(),
            None => match fs::read_to_string(self.ws.root.join(rel)) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    return Some(Err(if self.provider(language).manifest().fallback {
                        Skipped::Notice(format!("{}: skipped, not valid UTF-8", rel.display()))
                    } else {
                        Skipped::Incomplete(Incomplete {
                            path: Some(rel.to_owned()),
                            reason: "not valid UTF-8".to_owned(),
                        })
                    }));
                }
                Err(e) => {
                    return Some(Err(Skipped::Incomplete(Incomplete {
                        path: Some(rel.to_owned()),
                        reason: format!("unreadable: {e}"),
                    })));
                }
            },
        };
        let file = File {
            path: rel.to_owned(),
            lang: self.provider(language).manifest().id.clone(),
            hash: hash::sha256(&text),
            generated: self.config.is_generated(rel) || self.attributes.is_generated(rel),
            test: self.languages[language].tests.is_match(rel),
        };
        Some(Ok(Input {
            file,
            text,
            language,
        }))
    }

    fn provider(&self, language: usize) -> &dyn LanguageProvider {
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

    /// Has each provider index its files in one batch, the providers side by
    /// side, and merges the fragments. Files a provider did not index drop
    /// out; the reason is recorded as incomplete.
    pub(super) fn build_project(
        &self,
        inputs: Vec<Input>,
        overlays: &Overlays,
        run: &mut Run,
    ) -> (Vec<Input>, Project) {
        let ws = Workspace {
            overlays: overlays.clone(),
            ..self.ws.clone()
        };
        let mut batches: BTreeMap<usize, Vec<&Input>> = BTreeMap::new();
        for input in &inputs {
            batches.entry(input.language).or_default().push(input);
        }
        let batches: Vec<(usize, Vec<&Input>)> = batches.into_iter().collect();
        let indexed: Vec<(Result<Indexed, lighthouse_plugin::Error>, Duration)> = in_pool(|| {
            batches
                .par_iter()
                .map(|(language, batch)| {
                    let sources: Vec<Source> = batch
                        .iter()
                        .map(|i| Source {
                            file: &i.file,
                            text: &i.text,
                        })
                        .collect();
                    let started = Instant::now();
                    let indexed = self.provider(*language).index(&ws, &sources);
                    (indexed, started.elapsed())
                })
                .collect()
        });
        let mut parts: Vec<Fragment> = Vec::new();
        for ((language, batch), (indexed, spent)) in batches.iter().zip(indexed) {
            let provider = self.provider(*language);
            run.timings
                .index
                .push((provider.manifest().id.clone(), spent));
            match indexed {
                Ok(indexed) => {
                    parts.extend(indexed.fragments);
                    run.notices.extend(indexed.notices);
                    run.incomplete.extend(indexed.incomplete);
                }
                Err(e) => run.incomplete.push(Incomplete {
                    path: None,
                    reason: format!(
                        "language `{}` had an execution error, {} file(s) not analyzed: {e}",
                        provider.manifest().id,
                        batch.len()
                    ),
                }),
            }
        }
        let started = Instant::now();
        let project = Project::merge(parts);
        run.timings.merge = started.elapsed();
        run.notices.extend(project.notices().iter().cloned());
        let kept = inputs
            .into_iter()
            .filter(|i| project.file(&i.file.path).is_some())
            .collect();
        (kept, project)
    }

    /// Computes the facts the rules read, analyzer by analyzer in dependency
    /// order. The files of a file-scope analyzer are analyzed side by side:
    /// an analyzer reads the facts of those it requires, never its own.
    pub(super) fn analyze(
        &self,
        ws: &Workspace,
        (rules, inputs, project): (&[&dyn Rule], &[Input], &Project),
        memo: &Memo,
        timings: &mut Timings,
    ) -> Result<Facts, Error> {
        let wanted = rules
            .iter()
            .flat_map(|r| r.manifest().analyzers.iter().map(String::as_str));
        let mut facts = Facts::new();
        for analyzer in self.registry.order(wanted)? {
            let started = Instant::now();
            let id = &analyzer.manifest().id;
            let run = |input: Option<&Input>, facts: &Facts| {
                analyzer.run(&Context {
                    workspace: ws,
                    project,
                    file: input.map(|i| (&i.file, i.text.as_str())),
                    facts,
                    trusted: self.trusted,
                    memo,
                    notices: &Notices::default(),
                    applies: Applicability::default(),
                    models: &lighthouse_plugin::NoModels,
                })
            };
            match analyzer.manifest().scope {
                RunScope::Project => {
                    let fact = run(None, &facts)?;
                    facts.insert(id, String::new(), fact);
                }
                RunScope::File => {
                    let found: Vec<_> = in_pool(|| {
                        inputs
                            .par_iter()
                            .map(|input| run(Some(input), &facts))
                            .collect()
                    });
                    for (input, fact) in inputs.iter().zip(found) {
                        facts.insert(id, input.file.path.to_string_lossy().into_owned(), fact?);
                    }
                }
            }
            timings.analyzers.push((id.clone(), started.elapsed()));
        }
        Ok(facts)
    }

    /// Runs the rules over the files and the project, in parallel; what they
    /// found comes back in a fixed order: the findings of each file in file
    /// order, rule by rule, then those of the project rules in rule order.
    pub(super) fn apply(&self, scene: &Scene) -> Result<Gathered, Error> {
        let (files, project) = in_pool(|| {
            rayon::join(
                || self.apply_file_rules(scene),
                || self.apply_project_rules(scene),
            )
        });
        let mut all = files?;
        all.merge(project?);
        Ok(all)
    }

    fn apply_file_rules(&self, scene: &Scene) -> Result<Gathered, Error> {
        let per_file: Vec<Result<Gathered, Error>> = scene
            .inputs
            .par_iter()
            .map(|input| self.apply_to_file(scene, input))
            .collect();
        let mut all = Gathered::default();
        for gathered in per_file {
            all.merge(gathered?);
        }
        Ok(all)
    }

    fn apply_to_file(&self, scene: &Scene, input: &Input) -> Result<Gathered, Error> {
        let mut gathered = Gathered::default();
        let resolved = self
            .config
            .resolve(&input.file.path, &input.file.lang, &self.projects)?;
        let provider = self.provider(input.language);
        // What the merged model says of the file: a provider may know it is
        // generated where the host, reading it, did not.
        let file = scene.project.file(&input.file.path).unwrap_or(&input.file);
        for rule in scene
            .rules
            .iter()
            .filter(|r| r.manifest().scope == RunScope::File)
        {
            let meta = rule.manifest();
            let Some(config) = resolved.get(&meta.id) else {
                continue;
            };
            let Some(level) = config.level else { continue };
            let applies = self.applies(meta.applicability, config);
            if applies.excludes(scene.project, file) {
                continue;
            }
            if let Some(missing) = meta
                .capabilities
                .iter()
                .find(|c| !provider.manifest().capabilities.contains(c))
            {
                gathered.notices.insert(format!(
                    "{}: skipped for language `{}`, missing capability {missing}",
                    meta.id, input.file.lang
                ));
                continue;
            }
            let at = Some((file, input.text.as_str()));
            let options = options_at(&self.inherited, &resolved, &meta.id, &config.options);
            self.run_one(scene, *rule, (&options, level, applies), at, &mut gathered)?;
        }
        // The models answer what the file's rules queued, all at once.
        for (rule, found) in scene.models.flush(&input.file.path.to_string_lossy()) {
            let Some(level) = resolved.get(&rule).and_then(|config| config.level) else {
                continue;
            };
            gathered
                .found
                .extend(found.into_iter().map(|d| answered(d, level)));
        }
        Ok(gathered)
    }

    fn apply_project_rules(&self, scene: &Scene) -> Result<Gathered, Error> {
        let languages: BTreeSet<usize> = scene.inputs.iter().map(|i| i.language).collect();
        let resolved = self.config.resolve(Path::new(""), "", &self.projects)?;
        let rules: Vec<&&dyn Rule> = scene
            .rules
            .iter()
            .filter(|r| r.manifest().scope == RunScope::Project)
            .collect();
        let per_rule: Vec<Result<Gathered, Error>> = rules
            .par_iter()
            .map(|rule| self.apply_project_rule(scene, **rule, &languages, &resolved))
            .collect();
        let mut all = Gathered::default();
        for gathered in per_rule {
            all.merge(gathered?);
        }
        Ok(all)
    }

    fn apply_project_rule(
        &self,
        scene: &Scene,
        rule: &dyn Rule,
        languages: &BTreeSet<usize>,
        resolved: &lighthouse_spec::Rules,
    ) -> Result<Gathered, Error> {
        let mut gathered = Gathered::default();
        let meta = rule.manifest();
        let Some(config) = resolved.get(&meta.id) else {
            return Ok(gathered);
        };
        let Some(level) = config.level else {
            return Ok(gathered);
        };
        let missing = meta.capabilities.iter().find(|c| {
            languages
                .iter()
                .any(|&l| !self.provider(l).manifest().capabilities.contains(c))
        });
        if let Some(missing) = missing {
            gathered.notices.insert(format!(
                "{}: skipped, missing capability {missing}",
                meta.id
            ));
            return Ok(gathered);
        }
        let applies = self.applies(meta.applicability, config);
        let options = options_at(&self.inherited, resolved, &meta.id, &config.options);
        self.run_one(scene, rule, (&options, level, applies), None, &mut gathered)?;
        Ok(gathered)
    }

    /// Runs `rule` for `file` and its text (the project when `None`) as the
    /// configuration sets it, and adds what it found, noticed and could not
    /// finish to `gathered`.
    fn run_one(
        &self,
        scene: &Scene,
        rule: &dyn Rule,
        (options, level, applies): (&Options, Severity, Applicability),
        file: Option<(&File, &str)>,
        gathered: &mut Gathered,
    ) -> Result<(), Error> {
        let meta = rule.manifest();
        let notices = Notices::default();
        let ctx = Context {
            workspace: scene.workspace,
            project: scene.project,
            file,
            facts: scene.facts,
            trusted: self.trusted,
            memo: scene.memo,
            notices: &notices,
            applies,
            models: scene.models,
        };
        let started = Instant::now();
        let checked = self.run_rule(scene, rule, &ctx, options);
        gathered.notices.extend(notices.take());
        *gathered.times.entry(meta.id.clone()).or_default() += started.elapsed();
        let at = file.map(|(file, _)| file.path.as_path());
        let found = unfinished(checked, &meta.id, at, &mut gathered.incomplete)?;
        gathered
            .found
            .extend(found.into_iter().map(|d| leveled(d, level)));
        Ok(())
    }

    /// What code a rule may report on at a file: the decision's scope, unless
    /// the project says otherwise for all its decisions (`generated.check`)
    /// or for this rule (`generated`).
    fn applies(&self, declared: Applicability, rule: &RuleConfig) -> Applicability {
        let project = self
            .config
            .generated_check()
            .map(|c| c == GeneratedCheck::Include);
        Applicability {
            generated: rule.generated.or(project).unwrap_or(declared.generated),
            ..declared
        }
    }
}

/// A finding at the level the configuration gives its rule; a subject nothing
/// decided is only `info`.
fn leveled(mut d: Diagnostic, level: Severity) -> Diagnostic {
    d.severity = if d.review.is_some() {
        Severity::Info
    } else {
        level
    };
    d
}

/// A finding that came from a model's answer: it is not worth an `error`.
fn answered(d: Diagnostic, level: Severity) -> Diagnostic {
    let level = match level {
        Severity::Error => Severity::Warn,
        other => other,
    };
    leveled(d, level)
}

/// What a rule found, or nothing and a gap when it could not finish: the gap
/// is recorded for the file (or the project), so the run is incomplete rather
/// than clean. Any other failure ends the run.
fn unfinished(
    checked: Result<Vec<Diagnostic>, lighthouse_plugin::Error>,
    rule: &str,
    file: Option<&Path>,
    incomplete: &mut Vec<Incomplete>,
) -> Result<Vec<Diagnostic>, Error> {
    match checked {
        Err(lighthouse_plugin::Error::Incomplete(reason)) => {
            incomplete.push(Incomplete {
                path: file.map(Path::to_owned),
                reason: format!("{rule}: {reason}"),
            });
            Ok(Vec::new())
        }
        other => Ok(other?),
    }
}
