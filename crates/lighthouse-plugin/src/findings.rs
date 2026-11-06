//! Findings as queries: `findings(rule, file, variant)` runs the rule over the
//! graph's view of the model for one file (the project when the file is empty)
//! and gives what it found. What the rule reads of the model, of the text of
//! its file, of the facts and of the commands it runs is recorded as the
//! dependencies of its query, so an edit that moves none of them runs it for no
//! file, and the trace is the key.
//!
//! What the rule was configured with (its options, the code it is to judge, and
//! what its way of judging depends on besides the code) is the input `judged`
//! under the `variant` that tells the configurations of a rule apart.

use std::{any::Any, path::Path, sync::Arc, time::Instant};

use lighthouse_graph::{Durability, Error, Graph, Input, Query, VERSION};
use lighthouse_model::{
    Diagnostic, Document, Options,
    ucm::{Applicability, HOST, Held, Host, Model, View},
};
use serde::{Deserialize, Serialize};

use crate::{
    Annotations, Batch, Context, Error as PluginError, Facts, Models, NoModels, Notices, Queries,
    Rule, Runtime, Workspace, analysis::required, queries::NoQueries,
};

/// The kind of the answer of a batch, as a rule pulls it.
pub const BATCHED: &str = "command";

/// The kind of the paths of the project's files, as a rule pulls it: what a
/// rule that reads whatever its program reads depends on.
pub const PATHS: &str = "paths";

/// How a rule judges: set by the host for each configuration of each rule.
/// `Stable`.
pub static JUDGED: Input<(String, String), Judged> = Input::new("judged");

/// The files of the batch a file is in, for the rules that run a program over
/// a batch: the files of its unit, or of its directory. `Volatile`.
pub static BATCH: Input<String, Vec<String>> = Input::new("batch");

/// What the program a rule runs is: the file the search path finds, its
/// version and what chooses the version or configures it, by program name.
/// `Stable`.
pub static TOOL: Input<String, String> = Input::new("tool");

/// What one run of the program of the rule over the files told of each
/// (`rule`, `variant`, `files`).
pub static COMMAND: Query<(String, String, Vec<String>), Batched> = Query::new("command", command);

/// What the rule `rule` found in the file `file` (the project when empty)
/// judged as `variant` says.
pub static FINDINGS: Query<(String, String, String), Findings> = Query::new("findings", findings);

/// How one rule judges one configuration of its code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judged {
    /// The options the configuration resolved, as JSON of sorted keys.
    pub options: String,
    pub applies: Applicability,
    /// What the way the rule judges depends on besides the code: the build,
    /// the plugins, the conventions of the languages and the rule's own
    /// revision. A change moves exactly the findings of the rule.
    pub environment: String,
}

impl Judged {
    /// The configuration `options` and `applies` under `environment`.
    pub fn of(options: &Options, applies: Applicability, environment: String) -> Self {
        Self {
            options: serde_json::to_string(options).unwrap_or_default(),
            applies,
            environment,
        }
    }

    /// The name that tells this configuration of a rule from the others: its
    /// options and the code it judges, not the environment.
    pub fn variant(&self) -> String {
        let mut hash = lighthouse_protocol::hash::Hasher::new();
        hash.update(self.options.as_bytes());
        hash.update(format!("{:?}", self.applies).as_bytes());
        hash.hex()
    }
}

/// What a rule found in one file, and what it said besides. The findings are
/// kept as JSON: a finding carries free-form evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Findings {
    pub found: String,
    pub notices: Vec<String>,
}

impl Findings {
    /// The findings the JSON holds.
    pub fn diagnostics(&self) -> Result<Vec<Diagnostic>, serde_json::Error> {
        serde_json::from_str(&self.found)
    }
}

/// What a run adds to the rules it judges when it does not take them from the
/// graph: the models they may ask, the directives they read and the texts the
/// run stands in place of files.
pub struct Judging<'a> {
    pub models: &'a dyn Models,
    pub annotations: &'a Annotations,
    pub workspace: Option<&'a Workspace>,
    pub documents: &'a [Document],
}

/// The files of the batch the program of a rule is run over, as it said the
/// findings in each (JSON), and what it said besides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batched {
    pub answered: Vec<(String, String)>,
    pub notices: Vec<String>,
}

/// What a rule pulls: the kinds of the plugin that owns its facts, and the
/// commands the host runs over batches.
struct Pulled<'g> {
    graph: &'g Graph,
    inner: Box<dyn Queries + 'g>,
}

impl Queries for Pulled<'_> {
    fn pull(&self, kind: &str, key: &str) -> Result<Arc<dyn Any + Send + Sync>, PluginError> {
        if kind == PATHS {
            return match self.graph.input(&lighthouse_model::ucm::PATHS, &()) {
                Ok(paths) => Ok(Arc::new(paths.map(|paths| paths.as_ref().clone()))),
                Err(e) => Err(PluginError::Failed(e.to_string())),
            };
        }
        if kind != BATCHED {
            return self.inner.pull(kind, key);
        }
        let key: (String, String, Vec<String>) =
            serde_json::from_str(key).map_err(|e| PluginError::Failed(e.to_string()))?;
        match self.graph.query(&COMMAND, &key) {
            Ok(answer) => Ok(answer),
            Err(Error::Incomplete(reason)) => Err(PluginError::Incomplete(reason)),
            Err(other) => Err(PluginError::Failed(other.to_string())),
        }
    }
}

/// Runs the rule `id` over `file` (the project when empty) as `judged` says,
/// through `graph`: the findings of the node, or of a rule that is not one.
///
/// # Errors
///
/// A rule that could not finish is [`Error::Incomplete`].
pub fn judge(
    graph: &Graph,
    id: &str,
    file: &str,
    judged: &Judged,
    judging: &Judging,
) -> Result<Findings, Error> {
    let runtime = Runtime::of(graph)?;
    let rule = rule_of(runtime, id)?;
    let options = options_of(judged, id)?;
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let text = match file {
        "" => None,
        file => (runtime.text)(graph, file)?.as_ref().clone(),
    };
    let found = match file {
        "" => None,
        file => Some(match view.file(Path::new(file)) {
            Some(found) => found.clone(),
            None => host_file(graph, file)?,
        }),
    };
    let facts = facts_of(graph, runtime, rule, file)?;
    let identity = tools(graph, rule)?;
    let batch = if rule.batches() && !file.is_empty() {
        let files = graph
            .input(&BATCH, &file.to_owned())?
            .map(|batch| batch.as_ref().clone())
            .unwrap_or_default();
        Batch {
            variant: judged.variant(),
            files,
        }
    } else {
        Batch::default()
    };
    let mut workspace = judging.workspace.unwrap_or(&runtime.workspace).clone();
    if !identity.is_empty()
        && !file.is_empty()
        && let Some(text) = (runtime.overlaid)(graph, file)?
    {
        workspace.overlays.insert(file.into(), text);
    }
    let queries = queries_of(graph, runtime);
    let notices = Notices::default();
    let ctx = Context {
        workspace: &workspace,
        model: &view,
        file: found
            .as_ref()
            .map(|found| (found, text.as_deref().unwrap_or_default())),
        facts: &facts,
        trusted: runtime.trusted,
        annotations: judging.annotations,
        notices: &notices,
        applies: judged.applies.clone(),
        batch: &batch,
        documents: judging.documents,
        models: judging.models,
        queries: &queries,
    };
    let started = Instant::now();
    let checked = rule.check(&ctx, &options);
    *runtime
        .times
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(id.to_owned())
        .or_default() += started.elapsed();
    let found = checked.map_err(|e| outcome(e, id))?;
    if let Some(error) = view.failure() {
        return Err(error);
    }
    Ok(Findings {
        found: serde_json::to_string(&found).map_err(|e| Error::Encode(e.to_string()))?,
        notices: notices.take(),
    })
}

/// Sets `judged` as a configuration of `rule` and returns its variant.
///
/// # Errors
///
/// The graph cannot encode it.
pub fn set_judged(graph: &mut Graph, rule: &str, judged: Judged) -> Result<String, Error> {
    let variant = judged.variant();
    graph.set_input(
        &JUDGED,
        &(rule.to_owned(), variant.clone()),
        judged,
        Durability::Stable,
    )?;
    Ok(variant)
}

fn findings(
    graph: &Graph,
    (rule, file, variant): &(String, String, String),
) -> Result<Findings, Error> {
    graph.input(&VERSION, &"binary".to_owned())?;
    let judged = configuration(graph, rule, variant)?;
    let judging = Judging {
        models: &NoModels,
        annotations: &Annotations::default(),
        workspace: None,
        documents: &[],
    };
    judge(graph, rule, file, &judged, &judging)
}

fn command(
    graph: &Graph,
    (rule, variant, files): &(String, String, Vec<String>),
) -> Result<Batched, Error> {
    graph.input(&VERSION, &"binary".to_owned())?;
    let judged = configuration(graph, rule, variant)?;
    let runtime = Runtime::of(graph)?;
    let rule = rule_of(runtime, rule)?;
    let options = options_of(&judged, &rule.manifest().id)?;
    let identity = tools(graph, rule)?;
    let mut workspace = runtime.workspace.clone();
    let mut paths = Vec::new();
    for file in files {
        // The program reads the files from the disk: the text it is given in
        // place of one is a copy the rule makes.
        (runtime.text)(graph, file)?;
        if let Some(text) = (runtime.overlaid)(graph, file)? {
            workspace.overlays.insert(file.into(), text);
        }
        paths.push(std::path::PathBuf::from(file));
    }
    let held = Held::default();
    let view = View::new(graph, &held)?;
    paths.retain(|path| {
        view.file(path)
            .is_some_and(|file| !judged.applies.excludes(&view, file))
    });
    let notices = Notices::default();
    let queries = queries_of(graph, runtime);
    let ctx = Context {
        workspace: &workspace,
        model: &view,
        file: None,
        facts: &Facts::new(),
        trusted: runtime.trusted,
        annotations: &Annotations::default(),
        notices: &notices,
        applies: judged.applies.clone(),
        batch: &Batch::default(),
        documents: &[],
        models: &NoModels,
        queries: &queries,
    };
    let answered = rule
        .batch(&ctx, &options, &paths)
        .map_err(|e| outcome(e, &rule.manifest().id))?;
    if let Some(error) = view.failure() {
        return Err(error);
    }
    drop(identity);
    let answered = answered
        .into_iter()
        .map(|(path, found)| {
            let json = serde_json::to_string(&found).map_err(|e| Error::Encode(e.to_string()))?;
            Ok((path.to_string_lossy().into_owned(), json))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(Batched {
        answered,
        notices: notices.take(),
    })
}

fn configuration(graph: &Graph, rule: &str, variant: &str) -> Result<Arc<Judged>, Error> {
    graph
        .input(&JUDGED, &(rule.to_owned(), variant.to_owned()))?
        .ok_or_else(|| Error::Encode(format!("{rule}: no configuration `{variant}`")))
}

fn rule_of<'r>(runtime: &'r Runtime, id: &str) -> Result<&'r dyn Rule, Error> {
    runtime
        .registry
        .rule(id)
        .ok_or_else(|| Error::Encode(format!("no rule `{id}` is registered")))
}

fn options_of(judged: &Judged, id: &str) -> Result<Options, Error> {
    serde_json::from_str(&judged.options)
        .map_err(|e| Error::Encode(format!("{id}: the options cannot be read: {e}")))
}

/// The identity of the programs the rule runs, each read as an input.
fn tools(graph: &Graph, rule: &dyn Rule) -> Result<Vec<Arc<String>>, Error> {
    rule.manifest()
        .tools
        .iter()
        .map(|tool| Ok(graph.input(&TOOL, tool)?.unwrap_or_default()))
        .collect()
}

fn outcome(error: PluginError, id: &str) -> Error {
    match error {
        PluginError::Incomplete(reason) => Error::Incomplete(reason),
        other => Error::Encode(format!("{id}: {other}")),
    }
}

fn queries_of<'g>(graph: &'g Graph, runtime: &Runtime) -> Pulled<'g> {
    let inner: Box<dyn Queries + 'g> = match runtime.registry.pulling() {
        Some(pulling) => pulling.queries(graph),
        None => Box::new(NoQueries),
    };
    Pulled { graph, inner }
}

/// The file as the host knows it, for a file the merged model has not.
fn host_file(graph: &Graph, file: &str) -> Result<lighthouse_model::ucm::File, Error> {
    let host: Option<std::sync::Arc<Host>> = graph.input(&HOST, &file.to_owned())?;
    host.map(|host| host.file.clone())
        .ok_or_else(|| Error::Encode(format!("{file}: the file is not in the project")))
}

/// The facts of the analyzers the rule declares and those they require, read
/// as queries.
fn facts_of(graph: &Graph, runtime: &Runtime, rule: &dyn Rule, file: &str) -> Result<Facts, Error> {
    let mut wanted: Vec<String> = Vec::new();
    let mut todo: Vec<String> = rule.manifest().analyzers.clone();
    while let Some(id) = todo.pop() {
        if wanted.contains(&id) {
            continue;
        }
        todo.extend(runtime.analyzer(&id)?.manifest().requires.clone());
        wanted.push(id);
    }
    wanted.sort();
    required(graph, runtime, &wanted, file)
}
