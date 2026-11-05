//! A toy model shaped like the engine's layers: files are inputs; `parse`
//! reads one file, `api` is what other files see of it, `uses` and `deps`
//! follow its imports, `facts` is the per-file layer and `report` the
//! whole-project aggregate.

use std::{collections::BTreeMap, path::Path};

use lighthouse_graph::{Durability, Error, Graph, Input, Options, Query, VERSION};
use serde::{Deserialize, Serialize};

pub static FILE: Input<String, Option<String>> = Input::new("file");
pub static FILES: Input<(), Vec<String>> = Input::new("files");
pub static CONFIG: Input<(), String> = Input::new("config");

pub static PARSE: Query<String, Parsed> = Query::new("parse", parse);
pub static API: Query<String, String> = Query::new("api", api);
pub static USES: Query<String, Vec<String>> = Query::new("uses", uses);
pub static DEPS: Query<String, Vec<(String, String)>> = Query::new("deps", deps);
pub static FACTS: Query<String, Facts> = Query::new("facts", facts);
pub static REPORT: Query<(), Vec<(String, Facts)>> = Query::new("report", report);
/// The report again, fanned out over the files: it must equal `REPORT`.
pub static PAR_REPORT: Query<(), Vec<(String, Facts)>> = Query::new("par-report", par_report);

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Parsed {
    pub api: String,
    pub body: String,
    pub uses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    pub body: String,
    pub deps: Vec<(String, String)>,
    pub config: String,
}

/// A file is `api=<text>;body=<text>;uses=<a,b>`; a removed file parses to
/// nothing.
fn parse(graph: &Graph, path: &String) -> Result<Parsed, Error> {
    let version = graph
        .input(&VERSION, &"parser".to_owned())?
        .unwrap_or_default();
    let Some(text) = graph
        .input(&FILE, path)?
        .and_then(|file| file.as_ref().clone())
    else {
        return Ok(Parsed::default());
    };
    let mut parsed = Parsed {
        body: format!("{version}"),
        ..Parsed::default()
    };
    for part in text.split(';') {
        match part.split_once('=') {
            Some(("api", api)) => parsed.api = api.to_owned(),
            Some(("body", body)) => parsed.body.push_str(body),
            Some(("uses", uses)) => {
                parsed.uses = uses
                    .split(',')
                    .filter(|u| !u.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            _ => {}
        }
    }
    Ok(parsed)
}

fn api(graph: &Graph, path: &String) -> Result<String, Error> {
    Ok(graph.query(&PARSE, path)?.api.clone())
}

fn uses(graph: &Graph, path: &String) -> Result<Vec<String>, Error> {
    Ok(graph.query(&PARSE, path)?.uses.clone())
}

fn deps(graph: &Graph, path: &String) -> Result<Vec<(String, String)>, Error> {
    let mut deps = Vec::new();
    for used in graph.query(&USES, path)?.iter() {
        deps.push((used.clone(), graph.query(&API, used)?.as_ref().clone()));
    }
    Ok(deps)
}

fn facts(graph: &Graph, path: &String) -> Result<Facts, Error> {
    let version = graph
        .input(&VERSION, &"facts".to_owned())?
        .unwrap_or_default();
    let body = format!("{version}:{}", graph.query(&PARSE, path)?.body);
    let deps = graph.query(&DEPS, path)?.as_ref().clone();
    Ok(Facts {
        body,
        deps,
        config: graph
            .input(&CONFIG, &())?
            .map(|c| c.as_ref().clone())
            .unwrap_or_default(),
    })
}

fn report(graph: &Graph, _: &()) -> Result<Vec<(String, Facts)>, Error> {
    let mut all = Vec::new();
    for path in graph.input(&FILES, &())?.unwrap_or_default().iter() {
        all.push((path.clone(), graph.query(&FACTS, path)?.as_ref().clone()));
    }
    Ok(all)
}

fn par_report(graph: &Graph, _: &()) -> Result<Vec<(String, Facts)>, Error> {
    let files = graph
        .input(&FILES, &())?
        .unwrap_or_default()
        .as_ref()
        .clone();
    graph
        .par_map(files, |graph, path| {
            Ok((path.clone(), graph.query(&FACTS, path)?.as_ref().clone()))
        })
        .into_iter()
        .collect()
}

/// The inputs of a toy project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub files: BTreeMap<String, String>,
    pub config: String,
    pub versions: BTreeMap<String, String>,
}

impl Project {
    pub fn new() -> Self {
        let versions = [("parser", "p1"), ("facts", "f1")]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .into();
        Self {
            files: BTreeMap::new(),
            config: "c1".into(),
            versions,
        }
    }
}

/// Opens a graph on disk, or in memory without a path, .
pub fn options(project: &Project) -> Options {
    Options {
        versions: project.versions.clone(),
        verify: 0,
        ..Options::default()
    }
}

/// Opens a graph with the project's versions and no verify mode.
pub fn open(path: Option<&Path>, project: &Project) -> Graph {
    open_with(path, options(project))
}

pub fn open_with(path: Option<&Path>, options: Options) -> Graph {
    match path {
        Some(path) => Graph::open(path, options),
        None => Graph::in_memory(options),
    }
    .expect("open")
}

/// Sets every input to the project's: `known` are the paths ever seen, so
/// that a removed file is set to nothing.
pub fn apply(graph: &mut Graph, project: &Project, known: &[String]) {
    for path in known {
        graph
            .set_input(
                &FILE,
                path,
                project.files.get(path).cloned(),
                Durability::Volatile,
            )
            .expect("set file");
    }
    let list: Vec<String> = project.files.keys().cloned().collect();
    graph
        .set_input(&FILES, &(), list, Durability::Volatile)
        .expect("set files");
    graph
        .set_input(&CONFIG, &(), project.config.clone(), Durability::Stable)
        .expect("set config");
    for (name, version) in &project.versions {
        graph
            .set_input(&VERSION, name, version.clone(), Durability::Static)
            .expect("set version");
    }
}

/// Pulls the layers from the bottom up: a stored node whose dependency is of
/// a kind this process has not used yet counts as moved, so each kind is
/// used before anything above it is validated, as the report would.
fn bottom_up(graph: &Graph) {
    let files = graph.input(&FILES, &()).expect("files").unwrap_or_default();
    for path in files.iter() {
        graph.query(&PARSE, path).expect("parse");
        for used in graph.query(&USES, path).expect("uses").iter() {
            graph.query(&API, used).expect("api");
        }
        graph.query(&DEPS, path).expect("deps");
        graph.query(&FACTS, path).expect("facts");
    }
}

pub fn pull_par(graph: &Graph) -> Vec<(String, Facts)> {
    bottom_up(graph);
    graph
        .query(&PAR_REPORT, &())
        .expect("par report")
        .as_ref()
        .clone()
}

pub fn pull(graph: &Graph) -> Vec<(String, Facts)> {
    bottom_up(graph);
    graph.query(&REPORT, &()).expect("report").as_ref().clone()
}

/// The report from a graph that has never run.
pub fn from_scratch(project: &Project, known: &[String]) -> Vec<(String, Facts)> {
    let mut graph = open(None, project);
    apply(&mut graph, project, known);
    pull(&graph)
}

/// The path inside `kind("path")`, or empty for `report(())`.
pub fn path_of(label: &str) -> &str {
    label.split('"').nth(1).unwrap_or("")
}

pub fn kind_of(label: &str) -> &str {
    label.split('(').next().unwrap_or(label)
}
