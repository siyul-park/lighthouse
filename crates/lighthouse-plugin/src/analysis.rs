//! Analyzers as queries: `analysis(id, file)` runs the analyzer `id` over the
//! graph's view of the model for one file, and `analysis(id, "")` runs a
//! project-scope analyzer. What an analyzer reads of the model, of the text of
//! its file and of the facts it requires is recorded as the dependencies of
//! its query, so an edit that moves none of them runs it again for no file.
//!
//! The analyzers are the code a query calls through, so they are the graph's
//! [data](lighthouse_graph::Options::data), fixed for its life: every analyzer
//! of every plugin takes this path.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use lighthouse_graph::{Error, Graph, Query, VERSION};
use lighthouse_model::{
    RunScope,
    ucm::{File, Held, Model, PATHS, View},
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Analyzer, Facts, Registry, Workspace};

/// The fact of the analyzer `id` for a file, or for the project when the key
/// is empty, as JSON.
pub static ANALYSIS: Query<(String, String), String> = Query::new("analysis", analysis);

/// What an analyzer reads: the model through the graph's view, the file it is
/// run for with its text, and the facts of the analyzers it requires.
pub struct AnalyzerContext<'a> {
    pub model: &'a dyn Model,
    /// The focused file and its text; `None` for project scope.
    pub file: Option<(&'a File, &'a str)>,
    pub facts: &'a Facts,
}

impl AnalyzerContext<'_> {
    /// The fact of a required analyzer for the focused file, else its project
    /// fact.
    pub fn fact<T: DeserializeOwned>(&self, analyzer: &str) -> Result<T, crate::Error> {
        let file = self.file.map(|(f, _)| f.path.to_string_lossy());
        let value = file
            .and_then(|key| self.facts.get(analyzer, &key))
            .or_else(|| self.facts.get(analyzer, ""))
            .ok_or_else(|| crate::Error::MissingFact(analyzer.to_owned()))?;
        T::deserialize(value).map_err(|e| crate::Error::BadFact {
            analyzer: analyzer.to_owned(),
            message: e.to_string(),
        })
    }
}

/// What a graph runs its analyzers and rules with: the registry that holds
/// them, how the text of a file is read, and the workspace and trust the rules
/// run in. It is the data a graph that runs them is opened with.
pub struct Runtime {
    pub(crate) registry: Arc<Registry>,
    pub(crate) text: TextOf,
    pub(crate) overlaid: OverlaidOf,
    pub(crate) workspace: Workspace,
    pub(crate) trusted: bool,
    pub(crate) times: Mutex<BTreeMap<String, Duration>>,
    pub(crate) actions: Mutex<crate::findings::Actions>,
    pub(crate) statements: Mutex<crate::report::Statements>,
}

impl Runtime {
    /// The analyzers and rules of `registry`, whose files' text is read with
    /// `text`, run in `workspace` (without the texts a run stands in place of
    /// files: those are the graph's overlays).
    pub fn new(
        registry: Arc<Registry>,
        (text, overlaid): (TextOf, OverlaidOf),
        (workspace, trusted): (Workspace, bool),
    ) -> Self {
        Self {
            registry,
            text,
            overlaid,
            workspace,
            trusted,
            times: Mutex::default(),
            actions: Mutex::default(),
            statements: Mutex::default(),
        }
    }

    /// The runtime of `graph`.
    pub fn of(graph: &Graph) -> Result<&Self, Error> {
        graph
            .data::<Self>()
            .ok_or_else(|| Error::Encode("the graph has no registry".to_owned()))
    }

    /// The time each rule spent running since the last call.
    pub fn take_times(&self) -> BTreeMap<String, Duration> {
        std::mem::take(&mut *self.times.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn analyzer(&self, id: &str) -> Result<&dyn Analyzer, Error> {
        self.registry
            .analyzer(id)
            .ok_or_else(|| Error::Encode(format!("no analyzer `{id}` is registered")))
    }
}

/// The text a run stands in place of a file, read through the graph; `None`
/// for a file the run does not replace.
pub type OverlaidOf = fn(&Graph, &str) -> Result<Option<String>, Error>;

/// How the text of a file is read through the graph, recording the file's
/// content as a dependency; `None` for a file that is no text.
pub type TextOf = fn(&Graph, &str) -> Result<Arc<Option<String>>, Error>;

/// The facts of the analyzers `requires`, read as queries.
pub(crate) fn required(
    graph: &Graph,
    analyzers: &Runtime,
    requires: &[String],
    file: &str,
) -> Result<Facts, Error> {
    let mut facts = Facts::new();
    for id in requires {
        let scope = analyzers.analyzer(id)?.manifest().scope;
        let keys: Vec<String> = match (scope, file) {
            (RunScope::Project, _) => vec![String::new()],
            (_, file) if !file.is_empty() => vec![file.to_owned()],
            _ => graph
                .input(&PATHS, &())?
                .map(|paths| paths.as_ref().clone())
                .unwrap_or_default(),
        };
        for key in keys {
            let json = graph.query(&ANALYSIS, &(id.clone(), key.clone()))?;
            let value = serde_json::from_str(&json).map_err(|e| Error::Encode(e.to_string()))?;
            facts.insert(id, key, value);
        }
    }
    Ok(facts)
}

fn analysis(graph: &Graph, (id, file): &(String, String)) -> Result<String, Error> {
    let analyzers = Runtime::of(graph)?;
    graph.input(&VERSION, &"binary".to_owned())?;
    let analyzer = analyzers.analyzer(id)?;
    let facts = required(graph, analyzers, &analyzer.manifest().requires, file)?;
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let text = match file.as_str() {
        "" => Arc::new(None),
        file => (analyzers.text)(graph, file)?,
    };
    let focus = match view.file(Path::new(file)) {
        Some(file) => Some((file, text.as_deref().unwrap_or_default())),
        None if file.is_empty() => None,
        None => return Ok(Value::Null.to_string()),
    };
    let fact = analyzer
        .run(&AnalyzerContext {
            model: &view,
            file: focus,
            facts: &facts,
        })
        .map_err(|e| Error::Encode(format!("{id}: {e}")))?;
    if let Some(error) = view.failure() {
        return Err(error);
    }
    Ok(fact.to_string())
}
