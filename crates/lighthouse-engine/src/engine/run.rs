//! One run of the engine: choose the rules, load the project, apply the rules,
//! suppress what the source waives and give every finding its identity.

use super::judging::Judgment;
use super::*;
use lighthouse_model::ucm::Model;

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
        let (inputs, project) = self.load(&selected, overlays, &mut run)?;
        let annotations = Annotations::default();
        // The rules read what the run stands in place of files, as the
        // providers do.
        let workspace = Workspace {
            overlays: overlays.clone(),
            ..self.ws.clone()
        };
        let mut guard = self.lock_graph();
        let Some(graph) = guard.as_mut() else {
            return Err(Error::Graph(lighthouse_graph::Error::Encode(
                "the graph is not open".to_owned(),
            )));
        };
        if let Some(pulling) = self.registry.pulling() {
            pulling.inputs(graph, &selected, &workspace)?;
        }
        let facts = self.analyze(graph, (&selected, &inputs), &mut run.timings)?;
        let refs: Vec<&Input> = inputs.iter().collect();
        let judgment = Judgment {
            rules: &selected,
            inputs: &refs,
            project: &project,
            models: &models,
        };
        let plan = self.plan(&judgment, false)?;
        let started = Instant::now();
        self.judging_inputs(graph, &plan, &judgment)?;
        run.timings.commands = started.elapsed();
        {
            let scene = Scene {
                workspace: &workspace,
                rules: &selected,
                inputs: &inputs,
                project: &project,
                facts: &facts,
                annotations: &annotations,
                models: &models,
                graph,
            };
            let (found, unsettled) = self.collect(&scene, plan, &mut run)?;
            let started = Instant::now();
            let found = self.suppress(found, &scene, (&ran, &unsettled), &scopes, &mut run)?;
            self.identify(found, &scene, &mut run)?;
            run.timings.fingerprints = started.elapsed();
        }
        let started = Instant::now();
        let committed = graph.commit();
        run.timings.commit += started.elapsed();
        if let Err(e) = committed {
            run.notices.insert(format!("the graph was not saved: {e}"));
        }
        run.questions = models.subjects();
        run.probabilities = models.probabilities();
        run.usage = models.usage();
        run.notices.extend(models.finish());
        if let Some(cache) = &cache
            && let Err(e) = cache.finish()
        {
            run.notices
                .insert(format!("the result cache was not saved: {e}"));
        }
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
    ) -> Result<(Vec<Input>, Project), Error> {
        let started = Instant::now();
        let detected = self.detect(overlays, |rel| self.claimed(rel))?;
        run.notices.extend(detected.listing.notices.iter().cloned());
        run.notices.extend(detected.notices.iter().cloned());
        let listing = &detected.listing;
        let inputs = self.read(&detected, overlays, run);
        run.timings.read = started.elapsed();
        run.timings.files = detected.timing.clone();
        let (inputs, project) = self.build_project(inputs, overlays, run);
        let documented = selected
            .iter()
            .any(|rule| self.documented.contains(&rule.manifest().id));
        if !documented {
            return Ok((inputs, project));
        }
        let found = Documents::scan(&self.ws.root, &listing.files, overlays);
        run.notices.extend(found.notices);
        Ok((inputs, project.with_documents(found.documents)))
    }

    /// Applies the rules and keeps what they found, noticed and could not
    /// finish.
    fn collect(
        &self,
        scene: &Scene,
        plan: super::judging::Plan,
        run: &mut Run,
    ) -> Result<Collected, Error> {
        let started = Instant::now();
        let applied = self.judge_plan(scene, plan, &mut run.timings)?;
        run.timings.rules = started.elapsed();
        run.timings.add_rules(applied.times);
        run.notices.extend(applied.notices);
        run.incomplete.extend(applied.incomplete);
        let (reviews, found) = applied.found.into_iter().partition(|d| d.review.is_some());
        run.reviews = reviews;
        Ok((found, applied.unsettled))
    }

    /// Takes out what an `allow` annotation waives and what lies outside the
    /// reported paths; the waived findings go to the run.
    fn suppress(
        &self,
        found: Vec<Diagnostic>,
        scene: &Scene,
        (ran, unsettled): (&BTreeSet<String>, &Unsettled),
        scopes: &Scopes,
        run: &mut Run,
    ) -> Result<Vec<Diagnostic>, Error> {
        let texts: BTreeMap<&Path, &str> = scene
            .inputs
            .iter()
            .map(|i| (i.file.path.as_path(), i.text.as_str()))
            .collect();
        let text = |path: &Path| texts.get(path).copied();
        let gate = annotations::Gate {
            active: &self.active,
            selected: ran,
            unsettled,
            text: &text,
        };
        let applied = annotations::apply(found, scene.project, &gate);
        let mut kept = applied.kept;
        kept.extend(self.judge_annotations(scene, applied.annotations, run)?);
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
