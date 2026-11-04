//! A toy model shaped like the engine's layers: files are inputs; `parse`
//! reads one file, `api` is what other files see of it, `uses` and `deps`
//! follow its imports, `facts` is the per-file layer and `report` the
//! whole-project aggregate.

use std::{collections::BTreeMap, path::Path};

use lighthouse_graph::{Db, Durability, Error, Input, Query};
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
fn parse(db: &Db, path: &String) -> Result<Parsed, Error> {
    let version = db.version("parser")?;
    let Some(text) = db.input(&FILE, path)?.as_ref().clone() else {
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

fn api(db: &Db, path: &String) -> Result<String, Error> {
    Ok(db.query(&PARSE, path)?.api.clone())
}

fn uses(db: &Db, path: &String) -> Result<Vec<String>, Error> {
    Ok(db.query(&PARSE, path)?.uses.clone())
}

fn deps(db: &Db, path: &String) -> Result<Vec<(String, String)>, Error> {
    let mut deps = Vec::new();
    for used in db.query(&USES, path)?.iter() {
        deps.push((used.clone(), db.query(&API, used)?.as_ref().clone()));
    }
    Ok(deps)
}

fn facts(db: &Db, path: &String) -> Result<Facts, Error> {
    let version = db.version("facts")?;
    let body = format!("{version}:{}", db.query(&PARSE, path)?.body);
    let deps = db.query(&DEPS, path)?.as_ref().clone();
    Ok(Facts {
        body,
        deps,
        config: db.input(&CONFIG, &())?.as_ref().clone(),
    })
}

fn report(db: &Db, _: &()) -> Result<Vec<(String, Facts)>, Error> {
    let mut all = Vec::new();
    for path in db.input(&FILES, &())?.iter() {
        all.push((path.clone(), db.query(&FACTS, path)?.as_ref().clone()));
    }
    Ok(all)
}

fn par_report(db: &Db, _: &()) -> Result<Vec<(String, Facts)>, Error> {
    let files = db.input(&FILES, &())?.as_ref().clone();
    db.par_map(files, |db, path| {
        Ok((path.clone(), db.query(&FACTS, path)?.as_ref().clone()))
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

pub fn register(db: &Db) {
    db.register_input(&FILE);
    db.register_input(&FILES);
    db.register_input(&CONFIG);
    db.register_query(&PARSE);
    db.register_query(&API);
    db.register_query(&USES);
    db.register_query(&DEPS);
    db.register_query(&FACTS);
    db.register_query(&REPORT);
    db.register_query(&PAR_REPORT);
}

/// Opens a graph on disk, or in memory without a path, with all queries known.
pub fn open(path: Option<&Path>, project: &Project) -> Db {
    let db = match path {
        Some(path) => Db::open(path, &project.versions),
        None => Db::in_memory(&project.versions),
    }
    .expect("open");
    register(&db);
    db
}

/// Sets every input to the project's: `known` are the paths ever seen, so
/// that a removed file is set to nothing.
pub fn apply(db: &mut Db, project: &Project, known: &[String]) {
    for path in known {
        db.set_input(
            &FILE,
            path,
            project.files.get(path).cloned(),
            Durability::Volatile,
        )
        .expect("set file");
    }
    let list: Vec<String> = project.files.keys().cloned().collect();
    db.set_input(&FILES, &(), list, Durability::Volatile)
        .expect("set files");
    db.set_input(&CONFIG, &(), project.config.clone(), Durability::Stable)
        .expect("set config");
    for (name, version) in &project.versions {
        db.set_version(name, version).expect("set version");
    }
}

pub fn pull_par(db: &Db) -> Vec<(String, Facts)> {
    db.query(&PAR_REPORT, &())
        .expect("par report")
        .as_ref()
        .clone()
}

pub fn pull(db: &Db) -> Vec<(String, Facts)> {
    db.query(&REPORT, &()).expect("report").as_ref().clone()
}

/// The report from a graph that has never run.
pub fn from_scratch(project: &Project, known: &[String]) -> Vec<(String, Facts)> {
    let mut db = open(None, project);
    apply(&mut db, project, known);
    pull(&db)
}

/// The path inside `kind("path")`, or empty for `report(())`.
pub fn path_of(label: &str) -> &str {
    label.split('"').nth(1).unwrap_or("")
}

pub fn kind_of(label: &str) -> &str {
    label.split('(').next().unwrap_or(label)
}
