//! The phases of a run between reading files and collecting findings: read the
//! files, have the providers index them, compute the facts and apply the rules.

use super::*;

impl Engine {
    /// Reads every file under the root that a listed language claims.
    pub(super) fn read(
        &self,
        overlays: &Overlays,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> Vec<Input> {
        let mut inputs = Vec::new();
        let walk = WalkBuilder::new(&self.ws.root)
            .require_git(false)
            .add_custom_ignore_filename(IGNORE_FILE)
            .build();
        for entry in walk {
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
            if let Some(mut input) = self.read_input(entry.path(), rel, notices, incomplete) {
                if let Some(text) = overlays.get(rel) {
                    input.file.hash = hash::sha256(text);
                    input.text.clone_from(text);
                }
                inputs.push(input);
            }
        }
        inputs.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        inputs
    }

    /// Reads one file if a listed language claims it; a file that cannot be read
    /// becomes an incomplete entry (or a notice for binary data) and is dropped.
    fn read_input(
        &self,
        path: &Path,
        rel: &Path,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> Option<Input> {
        let &language = self
            .providers
            .iter()
            .find(|&&i| self.languages[i].files.is_match(rel))?;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                if self.provider(language).manifest().fallback {
                    notices.insert(format!("{}: skipped, not valid UTF-8", rel.display()));
                } else {
                    incomplete.push(Incomplete {
                        path: Some(rel.to_owned()),
                        reason: "not valid UTF-8".to_owned(),
                    });
                }
                return None;
            }
            Err(e) => {
                incomplete.push(Incomplete {
                    path: Some(rel.to_owned()),
                    reason: format!("unreadable: {e}"),
                });
                return None;
            }
        };
        let file = File {
            path: rel.to_owned(),
            lang: self.provider(language).manifest().id.clone(),
            hash: hash::sha256(&text),
            generated: self.config.is_generated(rel) || self.attributes.is_generated(rel),
            test: self.languages[language].tests.is_match(rel),
        };
        Some(Input {
            file,
            text,
            language,
        })
    }

    fn provider(&self, language: usize) -> &dyn LanguageProvider {
        self.registry
            .languages()
            .nth(language)
            .map(|(_, l)| l)
            .expect("language index comes from the registry")
    }

    /// Has each provider index its files in one batch, the providers side by
    /// side, and merges the fragments. Files a provider did not index drop
    /// out; the reason is recorded as incomplete.
    pub(super) fn build_project(
        &self,
        inputs: Vec<Input>,
        overlays: &Overlays,
        (notices, incomplete): (&mut BTreeSet<String>, &mut Vec<Incomplete>),
        timings: &mut Timings,
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
            timings.index.push((provider.manifest().id.clone(), spent));
            match indexed {
                Ok(indexed) => {
                    parts.extend(indexed.fragments);
                    notices.extend(indexed.notices);
                    incomplete.extend(indexed.incomplete);
                }
                Err(e) => incomplete.push(Incomplete {
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
        timings.merge = started.elapsed();
        notices.extend(project.notices().iter().cloned());
        let kept = inputs
            .into_iter()
            .filter(|i| project.file(&i.file.path).is_some())
            .collect();
        (kept, project)
    }

    pub(super) fn analyze(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
        memo: &Memo,
        timings: &mut Timings,
    ) -> Result<Facts, Error> {
        let wanted = rules
            .iter()
            .flat_map(|r| r.manifest().analyzers.iter().map(String::as_str));
        let mut facts = Facts::new();
        for analyzer in self.registry.order(wanted)? {
            let started = Instant::now();
            let runs: Vec<(Option<&Input>, String)> = match analyzer.manifest().scope {
                RunScope::Project => vec![(None, String::new())],
                RunScope::File => inputs
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
                    keys: &self.registry,
                    trusted: self.trusted,
                    memo,
                    notices: &Notices::default(),
                    applies: Applicability::default(),
                };
                let fact = analyzer.run(&ctx)?;
                facts.insert((analyzer.manifest().id.clone(), key), fact);
            }
            timings
                .analyzers
                .push((analyzer.manifest().id.clone(), started.elapsed()));
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
            let notices = Notices::default();
            let ctx = Ctx {
                ws: &self.ws,
                project: scene.project,
                file: Some((file, input.text.as_str())),
                facts: scene.facts,
                keys: &self.registry,
                trusted: self.trusted,
                memo: scene.memo,
                notices: &notices,
                applies,
            };
            let started = Instant::now();
            let checked = self.run_rule(scene, *rule, &ctx, &config.options);
            gathered.notices.extend(notices.take());
            *gathered.spent.entry(meta.id.clone()).or_default() += started.elapsed();
            let found = unfinished(
                checked,
                &meta.id,
                Some(&input.file.path),
                &mut gathered.incomplete,
            )?;
            gathered.found.extend(found.into_iter().map(|mut d| {
                d.severity = level;
                d
            }));
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
        resolved: &Rules,
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
        let notices = Notices::default();
        let ctx = Ctx {
            ws: &self.ws,
            project: scene.project,
            file: None,
            facts: scene.facts,
            keys: &self.registry,
            trusted: self.trusted,
            memo: scene.memo,
            notices: &notices,
            applies: self.applies(meta.applicability, config),
        };
        let started = Instant::now();
        let checked = self.run_rule(scene, rule, &ctx, &config.options);
        gathered.notices.extend(notices.take());
        *gathered.spent.entry(meta.id.clone()).or_default() += started.elapsed();
        let found = unfinished(checked, &meta.id, None, &mut gathered.incomplete)?;
        gathered.found.extend(found.into_iter().map(|mut d| {
            d.severity = level;
            d
        }));
        Ok(gathered)
    }
}

impl Engine {
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
