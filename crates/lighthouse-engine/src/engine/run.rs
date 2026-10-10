//! One run of the engine: choose the rules, load the project, apply the rules,
//! suppress what the source waives and give every finding its identity.

use super::*;

/// What a run gathers on its way to an [`Outcome`].
struct Progress {
    outcome: Outcome,
    timings: Timings,
    incomplete: Vec<Incomplete>,
    /// The paths whose findings the run reports.
    scopes: Vec<PathBuf>,
}

/// The files of a project and the model the providers made of them.
struct Loaded {
    inputs: Vec<Input>,
    project: Project,
}

impl Engine {
    pub(super) fn run(
        &self,
        reported: Reported,
        only: &[String],
        overlays: &Overlays,
    ) -> Result<Outcome, Error> {
        self.validate_only(only)?;
        let selected = self.select_rules(only);
        let mut progress = self.begin(reported)?;
        let loaded = self.load(&selected, overlays, &mut progress);
        let memo = Memo::default();
        let facts = self.analyze(
            &selected,
            &loaded.inputs,
            &loaded.project,
            &memo,
            &mut progress.timings,
        )?;
        let scene = Scene {
            rules: &selected,
            inputs: &loaded.inputs,
            project: &loaded.project,
            facts: &facts,
            memo: &memo,
        };
        let found = self.collect(&scene, &mut progress)?;
        let started = Instant::now();
        let found = self.suppress(found, &scene, &mut progress)?;
        self.identify(found, &scene, &mut progress)?;
        progress.timings.identity = started.elapsed();
        Ok(progress.finish(loaded.project, &self.active))
    }

    /// Rejects a rule that is unknown or not enabled.
    fn validate_only(&self, only: &[String]) -> Result<(), Error> {
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

    /// The state of a run before anything is read: what assembling the engine
    /// noted and the gaps known up front.
    fn begin(&self, reported: Reported) -> Result<Progress, Error> {
        let mut outcome = Outcome::default();
        outcome.notices.extend(self.notices.iter().cloned());
        let mut incomplete = self.startup.clone();
        let scopes = match reported {
            Reported::Paths(paths) => self.scopes(paths, &mut incomplete)?,
            Reported::Files(files) => files.to_vec(),
        };
        Ok(Progress {
            outcome,
            timings: Timings::default(),
            incomplete,
            scopes,
        })
    }

    /// Reads the files, has the providers index them, and finds the project's
    /// own documents when a selected rule is about them.
    fn load(&self, selected: &[&dyn Rule], overlays: &Overlays, progress: &mut Progress) -> Loaded {
        let started = Instant::now();
        let inputs = self.read(
            overlays,
            &mut progress.outcome.notices,
            &mut progress.incomplete,
        );
        progress.timings.read = started.elapsed();
        let (inputs, project) = self.build_project(
            inputs,
            overlays,
            (&mut progress.outcome.notices, &mut progress.incomplete),
            &mut progress.timings,
        );
        let documented = selected
            .iter()
            .any(|rule| self.documented.contains(&rule.manifest().id));
        let project = if documented {
            let found = documents::find(&self.ws.root, overlays);
            progress.outcome.notices.extend(found.notices);
            project.with_documents(found.documents)
        } else {
            project
        };
        Loaded { inputs, project }
    }

    /// Applies the rules and keeps what they found, noticed and could not
    /// finish.
    fn collect(&self, scene: &Scene, progress: &mut Progress) -> Result<Vec<Diagnostic>, Error> {
        let started = Instant::now();
        let applied = self.apply(scene)?;
        progress.timings.rules_wall = started.elapsed();
        progress.timings.add_rules(applied.spent);
        progress.outcome.notices.extend(applied.notices);
        progress.incomplete.extend(applied.incomplete);
        Ok(applied.found)
    }

    /// Takes out what an `allow` annotation waives and what lies outside the
    /// reported paths; the waived findings go to the outcome.
    fn suppress(
        &self,
        found: Vec<Diagnostic>,
        scene: &Scene,
        progress: &mut Progress,
    ) -> Result<Vec<Diagnostic>, Error> {
        let ran = ran_rules(scene.rules);
        let level = |rule: &str, file: &Path, lang: &str| self.level_at(rule, file, lang);
        let manifests: BTreeMap<String, &RuleManifest> = scene
            .rules
            .iter()
            .map(|r| (r.manifest().id.clone(), r.manifest()))
            .collect();
        let texts: BTreeMap<&Path, &str> = scene
            .inputs
            .iter()
            .map(|i| (i.file.path.as_path(), i.text.as_str()))
            .collect();
        let text = |path: &Path| texts.get(path).copied();
        let gate = annotations::Gate {
            level: &level,
            active: &self.active,
            selected: &ran,
            manifests: &manifests,
            text: &text,
        };
        let applied = annotations::apply(found, scene.project, &gate)?;
        let scopes = &progress.scopes;
        let mut kept = applied.kept;
        kept.retain(|d| scopes.iter().any(|s| d.file.starts_with(s)));
        progress.outcome.suppressed = applied
            .allowed
            .into_iter()
            .filter(|a| scopes.iter().any(|s| a.diagnostic.file.starts_with(s)))
            .collect();
        Ok(kept)
    }

    /// Orders the findings, gives each its identity and the facts and options
    /// it was judged with, and puts them in the outcome.
    fn identify(
        &self,
        mut found: Vec<Diagnostic>,
        scene: &Scene,
        progress: &mut Progress,
    ) -> Result<(), Error> {
        found.sort_by(|a, b| {
            (&a.file, a.span.start, &a.rule_id).cmp(&(&b.file, b.span.start, &b.rule_id))
        });
        let ordinal = identity::assign(&mut found, scene.project);
        let subjects = Subjects::new(scene.project, scene.facts, scene.inputs);
        progress.outcome.facts = found
            .iter()
            .map(|d| {
                let mut subject = subjects.of(d);
                if ordinal.contains(&d.fingerprint) {
                    subject["ordinal"] = Value::Bool(true);
                }
                (d.fingerprint.clone(), subject)
            })
            .collect();
        progress.outcome.options = self.options_of(&found, scene.project)?;
        progress.outcome.rules = ran_rules(scene.rules).into_iter().collect();
        progress.outcome.diagnostics = found;
        Ok(())
    }

    /// The level the configuration gives a rule at a file; `None` when off.
    fn level_at(
        &self,
        rule: &str,
        file: &Path,
        lang: &str,
    ) -> Result<Option<Severity>, ProjectError> {
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
        let mut resolved: BTreeMap<(PathBuf, String), Rules> = BTreeMap::new();
        let mut options = BTreeMap::new();
        for d in found {
            let project_scope = self
                .registry
                .rule(&d.rule_id)
                .is_some_and(|r| r.manifest().scope == RunScope::Project);
            let (path, lang) = match project.file(&d.file) {
                Some(file) if !project_scope => (file.path.clone(), file.lang.clone()),
                _ => (PathBuf::new(), String::new()),
            };
            let rules = match resolved.entry((path, lang)) {
                std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::btree_map::Entry::Vacant(e) => {
                    let (path, lang) = e.key().clone();
                    let rules = self.config.resolve(&path, &lang, &self.projects)?;
                    e.insert(rules)
                }
            };
            let found = rules.get(&d.rule_id).map(|c| c.options.clone());
            options.insert(d.fingerprint.clone(), found.unwrap_or_default());
        }
        Ok(options)
    }

    /// Report paths relative to the root; those outside it are incomplete.
    pub(crate) fn scopes(
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
}

/// The ids of the rules a run applied.
fn ran_rules(selected: &[&dyn Rule]) -> BTreeSet<String> {
    selected.iter().map(|r| r.manifest().id.clone()).collect()
}

impl Progress {
    /// The outcome of the run over `project`.
    fn finish(self, project: Project, active: &BTreeSet<String>) -> Outcome {
        let Self {
            mut outcome,
            timings,
            mut incomplete,
            scopes,
        } = self;
        outcome.timings = timings;
        outcome.reported = scopes;
        outcome.project = project;
        outcome.configured = active.iter().cloned().collect();
        incomplete.sort();
        incomplete.dedup();
        outcome.incomplete = incomplete;
        outcome
    }
}
