//! Applying the rules as queries of the graph: what each rule is to judge in
//! each file, as the configuration sets it, and the pull of the findings of
//! every (rule, file) pair side by side.
//!
//! A rule whose findings are a node (`findings`) is pulled; one that is not
//! (a rule that reads the directives, one whose models are asked this run, one
//! about the project's own documents, a command that reads more than the files)
//! is judged here every run, through the same function.

use std::sync::Arc;

use lighthouse_graph::{Durability, Error as GraphError, Graph};
use lighthouse_model::ucm::Applicability;
use lighthouse_plugin::{
    BATCH, COMMAND, FINDINGS, Findings, Judged, Judging, Key, Runtime, TOOL, judge, run_batches,
    set_judged,
};
use lighthouse_protocol::hash::Hasher;

use super::{
    phases::{answered, in_pool},
    *,
};
use crate::tools;

/// What a phase of a run is to judge, and what was noted while choosing.
#[derive(Default)]
pub(super) struct Plan<'s> {
    files: Vec<FilePlan<'s>>,
    project: Vec<Task<'s>>,
    gathered: Gathered,
}

/// What is to be judged in one file.
pub(super) struct FilePlan<'s> {
    input: &'s Input,
    resolved: Arc<lighthouse_spec::Rules>,
    tasks: Vec<Task<'s>>,
}

/// One rule over one file (the project when there is none).
pub(super) struct Task<'r> {
    rule: &'r dyn Rule,
    level: Severity,
    judged: Judged,
    variant: String,
    /// The findings are a node of the graph.
    node: bool,
}

/// What is known of a run that every rule is judged with.
pub(super) struct Judgment<'a> {
    pub rules: &'a [&'a dyn Rule],
    pub inputs: &'a [&'a Input],
    pub project: &'a Stated,
    pub models: &'a dyn lighthouse_plugin::Models,
}

impl Plan<'_> {
    /// The findings nodes the plan is made of: the tasks of the rules that are
    /// not nodes read the files and the index, which the run's state holds.
    pub(super) fn nodes(&self) -> Vec<(String, String, String)> {
        let key = |task: &Task, file: &str| {
            (
                task.rule.manifest().id.clone(),
                file.to_owned(),
                task.variant.clone(),
            )
        };
        let mut keys = Vec::new();
        for file in &self.files {
            let name = file.input.file.path.to_string_lossy();
            keys.extend(file.tasks.iter().filter(|t| t.node).map(|t| key(t, &name)));
        }
        keys.extend(self.project.iter().filter(|t| t.node).map(|t| key(t, "")));
        keys
    }
}

impl Engine {
    /// Chooses, for each file and the project, the rules to judge and how:
    /// the rules that read the directives (`after`), or those that do not.
    pub(super) fn plan<'s>(&self, run: &Judgment<'s>, after: bool) -> Result<Plan<'s>, Error> {
        let files: Vec<Result<(FilePlan<'s>, Gathered), Error>> = in_pool(|| {
            run.inputs
                .par_iter()
                .map(|input| self.plan_file(run, input, after))
                .collect()
        });
        let mut plan = Plan::default();
        for planned in files {
            let (file, gathered) = planned?;
            plan.gathered.merge(gathered);
            plan.files.push(file);
        }
        if !after {
            plan.project = self.plan_project(run, &mut plan.gathered)?;
        }
        Ok(plan)
    }

    fn plan_file<'s>(
        &self,
        run: &Judgment<'s>,
        input: &'s Input,
        after: bool,
    ) -> Result<(FilePlan<'s>, Gathered), Error> {
        let mut gathered = Gathered::default();
        let resolved = self
            .config
            .resolve(&input.file.path, &input.file.lang, &self.projects)?;
        // What the merged model says of the file: a provider may know it is
        // generated where the host, reading it, did not.
        let file = run.project.file(&input.file.path).unwrap_or(&input.file);
        let mut tasks = Vec::new();
        for rule in run
            .rules
            .iter()
            .filter(|r| r.manifest().scope == RunScope::File && r.after_suppression() == after)
        {
            let Some((options, level, applies)) =
                self.judged(run.project, *rule, &resolved, file, &mut gathered)
            else {
                continue;
            };
            tasks.push(self.task(run, *rule, (&options, level, applies)));
        }
        Ok((
            FilePlan {
                input,
                resolved,
                tasks,
            },
            gathered,
        ))
    }

    fn plan_project<'s>(
        &self,
        run: &Judgment<'s>,
        gathered: &mut Gathered,
    ) -> Result<Vec<Task<'s>>, Error> {
        let languages: BTreeSet<usize> = run.inputs.iter().map(|i| i.language).collect();
        let resolved = self.config.resolve(Path::new(""), "", &self.projects)?;
        let mut tasks = Vec::new();
        for rule in run
            .rules
            .iter()
            .filter(|r| r.manifest().scope == RunScope::Project)
        {
            let meta = rule.manifest();
            let Some(config) = resolved.get(&meta.id) else {
                continue;
            };
            let Some(level) = config.level else { continue };
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
                gathered.unsettled.insert((meta.id.clone(), None));
                continue;
            }
            let applies = self.applies(&meta.applicability, config);
            let options = options_at(&self.inherited, &resolved, &meta.id, &config.options);
            tasks.push(self.task(run, *rule, (&options, level, applies)));
        }
        Ok(tasks)
    }

    fn task<'s>(
        &self,
        run: &Judgment<'s>,
        rule: &'s dyn Rule,
        (options, level, applies): (&Options, Severity, Applicability),
    ) -> Task<'s> {
        let meta = rule.manifest();
        let judged = Judged::of(options, applies, self.environment(rule));
        let node = meta.revision.is_some()
            && !self.documented.contains(&meta.id)
            && !(rule.reviews() && run.models.asks(&meta.id));
        Task {
            rule,
            level,
            variant: judged.variant(),
            judged,
            node,
        }
    }

    /// What the way `rule` judges depends on besides the code: the plugins,
    /// how the languages are configured, and the rule's own revision. The
    /// build is the `binary` version, which every node reads.
    pub(super) fn environment(&self, rule: &dyn Rule) -> String {
        let mut hasher = Hasher::new();
        for plugin in self.registry.manifests() {
            hasher.update(&plugin.id);
            hasher.update(&plugin.version);
        }
        // Maps of sorted keys: the same configuration is the same text.
        let maps = [
            serde_json::to_string(&self.ws.languages),
            serde_json::to_string(&self.ws.constructors),
            serde_json::to_string(&self.ws.test_naming),
        ];
        for map in maps {
            hasher.update(map.unwrap_or_default());
        }
        hasher.update(rule.manifest().revision.as_deref().unwrap_or_default());
        hasher.hex()
    }

    /// Sets what the nodes of the plan read: how each rule is configured, the
    /// programs they run, and the files each program is run over together.
    pub(super) fn judging_inputs(
        &self,
        graph: &mut Graph,
        plan: &Plan,
        run: &Judgment,
    ) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        let mut batching = false;
        let mut programs = BTreeSet::new();
        let tasks = plan
            .files
            .iter()
            .flat_map(|file| &file.tasks)
            .chain(&plan.project)
            .filter(|task| task.node);
        for task in tasks {
            let id = &task.rule.manifest().id;
            if seen.insert((id.clone(), task.variant.clone())) {
                set_judged(graph, id, task.judged.clone())?;
            }
            batching |= task.rule.batches();
            programs.extend(task.rule.manifest().tools.iter().cloned());
        }
        COMMAND.register(graph)?;
        let tools: Vec<String> = programs.into_iter().collect();
        for (tool, identity) in tools::identities(&self.ws.root, run.project.files(), &tools) {
            graph.set_input(&TOOL, &tool, identity, Durability::Stable)?;
        }
        if batching {
            self.set_batches(graph, run)?;
            run_batches(graph, batches_of(graph, plan)?)?;
        }
        Ok(())
    }

    /// For each file, the files of its batch: those of its unit when its
    /// language has units, else those of its directory.
    fn set_batches(&self, graph: &mut Graph, run: &Judgment) -> Result<(), Error> {
        let mut by_directory: BTreeMap<(&str, &Path), Vec<String>> = BTreeMap::new();
        for input in run.inputs {
            let path = input.file.path.as_path();
            by_directory
                .entry((&input.file.lang, path.parent().unwrap_or(Path::new(""))))
                .or_default()
                .push(path.to_string_lossy().into_owned());
        }
        let languages: BTreeSet<&str> = run.inputs.iter().map(|i| i.file.lang.as_str()).collect();
        let mut units: BTreeMap<(&str, String), Vec<String>> = BTreeMap::new();
        for language in languages {
            for unit in graph
                .query(&crate::graph::UNITS, &language.to_owned())?
                .iter()
            {
                for file in &unit.files {
                    units.insert((language, file.clone()), unit.files.clone());
                }
            }
        }
        for input in run.inputs {
            let path = input.file.path.as_path();
            let name = path.to_string_lossy().into_owned();
            let batch = units
                .get(&(input.file.lang.as_str(), name.clone()))
                .or_else(|| {
                    by_directory.get(&(
                        input.file.lang.as_str(),
                        path.parent().unwrap_or(Path::new("")),
                    ))
                })
                .cloned()
                .unwrap_or_else(|| vec![name.clone()]);
            graph.set_input(&BATCH, &name, batch, Durability::Volatile)?;
        }
        Ok(())
    }

    /// Judges what the plan holds: the nodes are pulled side by side through
    /// the graph, the rest are judged here; what they found comes back in a
    /// fixed order: the findings of each file in file order, rule by rule,
    /// then those of the project rules in rule order.
    pub(super) fn judge_plan(
        &self,
        scene: &Scene,
        mut plan: Plan,
        timings: &mut Timings,
    ) -> Result<Gathered, Error> {
        let graph = scene.graph;
        let key = |task: &Task, file: &str| {
            (
                task.rule.manifest().id.clone(),
                file.to_owned(),
                task.variant.clone(),
            )
        };
        let mut keys = Vec::new();
        for file in &plan.files {
            let name = file.input.file.path.to_string_lossy();
            keys.extend(file.tasks.iter().filter(|t| t.node).map(|t| key(t, &name)));
        }
        keys.extend(plan.project.iter().filter(|t| t.node).map(|t| key(t, "")));
        let pulled = keys.len();
        let mut nodes =
            in_pool(|| graph.par_map(keys, |graph, key| graph.query(&FINDINGS, key))).into_iter();
        let direct: Vec<Result<Direct, Error>> = in_pool(|| {
            plan.files
                .par_iter()
                .map(|file| self.judge_directly(scene, file))
                .collect()
        });
        let mut all = std::mem::take(&mut plan.gathered);
        for (file, direct) in plan.files.iter().zip(direct) {
            let mut direct = direct?;
            let path = Some(file.input.file.path.as_path());
            for (at, task) in file.tasks.iter().enumerate() {
                let result = if task.node {
                    nodes.next().expect("a result for each node")
                } else {
                    direct.results.remove(&at).expect("a result for each task")
                };
                all.settle((&task.rule.manifest().id, task.level), path, result)?;
            }
            all.answer(direct.answered);
        }
        for task in &plan.project {
            let result = if task.node {
                nodes.next().expect("a result for each node")
            } else {
                self.judge_task(scene, task, "")
            };
            all.settle((&task.rule.manifest().id, task.level), None, result)?;
        }
        let executed = graph.take_executed();
        let run = executed
            .iter()
            .filter(|label| label.starts_with("findings("))
            .count();
        timings.findings_run += run;
        timings.findings_green += pulled.saturating_sub(run);
        all.times = Runtime::of(graph)?.take_times();
        Ok(all)
    }

    /// Judges the tasks of a file that are no node, and settles the models
    /// they queued.
    fn judge_directly(&self, scene: &Scene, file: &FilePlan) -> Result<Direct, Error> {
        let name = file.input.file.path.to_string_lossy().into_owned();
        let mut direct = Direct::default();
        for (at, task) in file.tasks.iter().enumerate().filter(|(_, t)| !t.node) {
            direct
                .results
                .insert(at, self.judge_task(scene, task, &name));
        }
        if direct.results.is_empty() {
            return Ok(direct);
        }
        // Settle the models queued by this phase with the same resolved levels.
        for (rule, found) in scene.models.flush(&name) {
            let Some(level) = file.resolved.get(&rule).and_then(|config| config.level) else {
                continue;
            };
            direct
                .answered
                .extend(found.into_iter().map(|d| answered(d, level)));
        }
        Ok(direct)
    }

    fn judge_task(
        &self,
        scene: &Scene,
        task: &Task,
        file: &str,
    ) -> Result<Arc<Findings>, GraphError> {
        let judging = Judging {
            models: scene.models,
            annotations: scene.annotations,
            workspace: Some(scene.workspace),
            documents: scene.stated.documents(),
        };
        judge(
            scene.graph,
            &task.rule.manifest().id,
            file,
            &task.judged,
            &judging,
        )
        .map(Arc::new)
    }

    /// Judges the rules that read the directives (`said`), which the engine
    /// knows once the other rules' findings are: the rules judge each file that
    /// has a directive. What they find is not itself suppressed.
    pub(super) fn judge_directives(
        &self,
        scene: &Scene,
        inputs: &[&Input],
        timings: &mut Timings,
    ) -> Result<Gathered, Error> {
        let run = Judgment {
            rules: scene.rules,
            inputs,
            project: scene.stated,
            models: scene.models,
        };
        let plan = self.plan(&run, true)?;
        self.judge_plan(scene, plan, timings)
    }
}

/// What the tasks of a file that are no node found.
#[derive(Default)]
struct Direct {
    results: BTreeMap<usize, Result<Arc<Findings>, GraphError>>,
    answered: Vec<Diagnostic>,
}

/// The batches the nodes of the plan read: of each rule that runs a program
/// over a batch, the files of the batch of each file it judges.
fn batches_of(graph: &Graph, plan: &Plan) -> Result<Vec<Key>, GraphError> {
    let mut batches = BTreeSet::new();
    for file in &plan.files {
        let name = file.input.file.path.to_string_lossy().into_owned();
        let Some(files) = graph.input(&BATCH, &name)? else {
            continue;
        };
        for task in file.tasks.iter().filter(|t| t.node && t.rule.batches()) {
            let id = task.rule.manifest().id.clone();
            batches.insert((id, task.variant.clone(), files.as_ref().clone()));
        }
    }
    Ok(batches.into_iter().collect())
}
