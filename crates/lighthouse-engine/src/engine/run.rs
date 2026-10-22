//! One run of the engine: choose the rules, load the project, apply the rules,
//! suppress what the source waives and give every finding its identity.

use super::*;

impl Engine {
    pub(super) fn run(
        &self,
        scopes: Scopes,
        gaps: Vec<Incomplete>,
        only: &[String],
        overlays: &Overlays,
    ) -> Result<Run, Error> {
        let selected = self.select_rules(only);
        let ran: BTreeSet<String> = selected.iter().map(|r| r.manifest().id.clone()).collect();
        let mut run = Run {
            notices: self.notices.iter().cloned().collect(),
            incomplete: self.startup.iter().cloned().chain(gaps).collect(),
            ran: ran.iter().cloned().collect(),
            active: self.active.iter().cloned().collect(),
            ..Run::default()
        };
        let cache = self.open_cache().and_then(|opened| {
            opened
                .map_err(|e| {
                    run.notices
                        .insert(format!("the result cache is not used: {e}"));
                })
                .ok()
        });
        let models = RunModels::new(self, &scopes, cache.as_ref());
        let (inputs, project) = self.load(&selected, overlays, &mut run);
        let started = Instant::now();
        let run_cache = cache.as_ref().and_then(|cache| {
            self.run_cache(cache, &project, &selected, &models, &mut run.notices)
        });
        run.timings.hashing = started.elapsed();
        let memo = Memo::default();
        // The rules read what the run stands in place of files, as the
        // providers do.
        let workspace = Workspace {
            overlays: overlays.clone(),
            ..self.ws.clone()
        };
        let facts = self.analyze(
            &workspace,
            (&selected, &inputs, &project),
            &memo,
            &mut run.timings,
        )?;
        let scene = Scene {
            workspace: &workspace,
            rules: &selected,
            inputs: &inputs,
            project: &project,
            facts: &facts,
            memo: &memo,
            cache: run_cache.as_ref(),
            models: &models,
        };
        let found = self.collect(&scene, &mut run)?;
        run.questions = models.subjects();
        run.probabilities = models.probabilities();
        (run.model_calls, run.model_cost) = models.usage();
        run.notices.extend(models.finish());
        if let Some(cache) = &cache {
            if let Some(run_cache) = &run_cache {
                run_cache.store_rows();
            }
            match cache.finish() {
                Ok(stats) => {
                    run.timings.cache_hits = stats.hits;
                    run.timings.cache_misses = stats.misses;
                }
                Err(e) => {
                    run.notices
                        .insert(format!("the result cache was not saved: {e}"));
                }
            }
        }
        let started = Instant::now();
        let found = self.suppress(found, &scene, &ran, &scopes, &mut run)?;
        self.identify(found, &scene, &mut run)?;
        run.timings.fingerprints = started.elapsed();
        run.incomplete.sort();
        run.incomplete.dedup();
        run.reported = scopes.0.clone();
        run.project = project;
        Ok(run)
    }

    /// Rejects a rule that is unknown or not enabled.
    pub(super) fn validate_only(&self, only: &[String]) -> Result<(), Error> {
        for id in only {
            if self.registry.rule(id).is_none() {
                return Err(Error::UnknownRule(id.clone()));
            }
            if !self.active.contains(id) {
                return Err(Error::RuleNotEnabled(id.clone()));
            }
        }
        Ok(())
    }

    /// The rules a run applies: those named, else every enforced active one.
    fn select_rules(&self, only: &[String]) -> Vec<&dyn Rule> {
        self.active
            .iter()
            .filter(|id| only.is_empty() || only.contains(id))
            .filter_map(|id| self.registry.rule(id))
            .filter(|rule| {
                let id = &rule.manifest().id;
                !self.unenforced.contains(id) || only.contains(id)
            })
            .collect()
    }

    /// Reads the files, has the providers index them, and finds the project's
    /// own documents when a selected rule is about them.
    fn load(
        &self,
        selected: &[&dyn Rule],
        overlays: &Overlays,
        run: &mut Run,
    ) -> (Vec<Input>, Project) {
        let started = Instant::now();
        let inputs = self.read(overlays, run);
        run.timings.read = started.elapsed();
        let (inputs, project) = self.build_project(inputs, overlays, run);
        let documented = selected
            .iter()
            .any(|rule| self.documented.contains(&rule.manifest().id));
        if !documented {
            return (inputs, project);
        }
        let found = Documents::scan(&self.ws.root, overlays);
        run.notices.extend(found.notices);
        (inputs, project.with_documents(found.documents))
    }

    /// Applies the rules and keeps what they found, noticed and could not
    /// finish.
    fn collect(&self, scene: &Scene, run: &mut Run) -> Result<Vec<Diagnostic>, Error> {
        let started = Instant::now();
        let applied = self.apply(scene)?;
        run.timings.rules = started.elapsed();
        run.timings.add_rules(applied.times);
        run.notices.extend(applied.notices);
        run.incomplete.extend(applied.incomplete);
        let (reviews, found) = applied.found.into_iter().partition(|d| d.review.is_some());
        run.reviews = reviews;
        Ok(found)
    }

    /// Takes out what an `allow` annotation waives and what lies outside the
    /// reported paths; the waived findings go to the run.
    fn suppress(
        &self,
        found: Vec<Diagnostic>,
        scene: &Scene,
        ran: &BTreeSet<String>,
        scopes: &Scopes,
        run: &mut Run,
    ) -> Result<Vec<Diagnostic>, Error> {
        let level = |rule: &str, file: &Path, lang: &str| self.level_at(rule, file, lang);
        let texts: BTreeMap<&Path, &str> = scene
            .inputs
            .iter()
            .map(|i| (i.file.path.as_path(), i.text.as_str()))
            .collect();
        let text = |path: &Path| texts.get(path).copied();
        let gate = annotations::Gate {
            level: &level,
            active: &self.active,
            selected: ran,
            registry: &self.registry,
            text: &text,
        };
        let applied = annotations::apply(found, scene.project, &gate)?;
        let mut kept = applied.kept;
        kept.retain(|d| scopes.contains(&d.file));
        run.reviews.retain(|d| scopes.contains(&d.file));
        in_order(&mut run.reviews);
        run.suppressed = applied
            .allowed
            .into_iter()
            .filter(|a| scopes.contains(&a.diagnostic.file))
            .collect();
        Ok(kept)
    }

    /// Orders the findings, gives each its identity and the facts and options
    /// it was judged with, and puts them in the run.
    fn identify(
        &self,
        mut found: Vec<Diagnostic>,
        scene: &Scene,
        run: &mut Run,
    ) -> Result<(), Error> {
        in_order(&mut found);
        let ordinal = Diagnostic::disambiguate(&mut found, |d| scene.project.discriminator(d));
        let facts = FindingFacts::new(scene.project, scene.facts, scene.inputs);
        run.facts = found
            .iter()
            .map(|d| {
                let mut facts = facts.of(d);
                if ordinal.contains(&d.fingerprint) {
                    facts["ordinal"] = Value::Bool(true);
                }
                (d.fingerprint.clone(), facts)
            })
            .collect();
        run.options = self.options_of(&found, scene.project)?;
        run.diagnostics = found;
        Ok(())
    }

    /// The level the configuration gives a rule at a file; `None` when off.
    fn level_at(
        &self,
        rule: &str,
        file: &Path,
        lang: &str,
    ) -> Result<Option<Severity>, lighthouse_spec::Error> {
        let rules = self.config.resolve(file, lang, &self.projects)?;
        Ok(rules.get(rule).and_then(|config| config.level))
    }

    /// The options the configuration resolves for each finding's rule at its
    /// file (project-scope rules at the project).
    fn options_of(
        &self,
        found: &[Diagnostic],
        project: &Project,
    ) -> Result<BTreeMap<Fingerprint, Options>, Error> {
        let mut options = BTreeMap::new();
        for d in found {
            let project_scope = self
                .registry
                .rule(&d.rule_id)
                .is_some_and(|r| r.manifest().scope == RunScope::Project);
            let (path, lang) = match project.file(&d.file) {
                Some(file) if !project_scope => (file.path.as_path(), file.lang.as_str()),
                _ => (Path::new(""), ""),
            };
            let rules = self.config.resolve(path, lang, &self.projects)?;
            let found = rules
                .get(&d.rule_id)
                .map(|c| options_at(&self.inherited, &rules, &d.rule_id, &c.options).into_owned());
            options.insert(d.fingerprint.clone(), found.unwrap_or_default());
        }
        Ok(options)
    }
}

/// Sorts findings by file, position and rule.
fn in_order(found: &mut [Diagnostic]) {
    found.sort_by(|a, b| {
        (&a.file, a.span.start, &a.rule_id).cmp(&(&b.file, b.span.start, &b.rule_id))
    });
}
