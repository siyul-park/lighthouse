//! What a run reports as a query: `report(scope)` is the output of a run over
//! the reported paths, kept with what it was made from. Its first two
//! dependencies, the state of the run and the build, decide whether it is
//! current: when they have not moved the graph answers it without validating
//! the findings it was made of, which is a run of a project nothing has
//! touched.
//!
//! The host states the output (a run is more than the findings of the rules:
//! notices, suppressions, what the findings were judged with); the query
//! records what it rests on, the run's own state and the findings of the rules
//! it was made of, so that a state of the inputs that is not the one it was
//! made for runs the host again.

use std::{collections::HashMap, sync::PoisonError};

use lighthouse_graph::{Durability, Error, Graph, Input, Query, VERSION};
use serde::{Deserialize, Serialize};

use crate::{FINDINGS, Runtime};

/// What a run is besides the files and what the rules found: the configuration
/// it ran under, the build, the programs it runs and the state of the index.
/// `Volatile`: the files are in it.
pub static RUN: Input<(), String> = Input::new("run");

/// What a run over the reported paths of `scope` reports.
pub static REPORT: Query<String, Reported> = Query::new("report", report).decided_by(2);

/// The output of a run, as the host states it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    pub body: String,
}

/// The rule, the file (the project when empty) and the variant of a findings
/// node a report is made of.
pub type Read = (String, String, String);

/// What the host gives `report` to answer with when it has to run.
#[derive(Default)]
pub struct Statements {
    stated: HashMap<String, (Vec<Read>, String)>,
}

/// Sets what the run is, so that a report of another run is not this one's.
///
/// # Errors
///
/// The graph rejected the input.
pub fn set_run(graph: &mut Graph, state: String) -> Result<(), Error> {
    graph.set_input(&RUN, &(), state, Durability::Volatile)
}

/// The report of `scope` when the state of the run is the one it was made for.
///
/// # Errors
///
/// The store could not be read.
pub fn quiet_report(graph: &Graph, scope: &str) -> Result<Option<String>, Error> {
    let scope = scope.to_owned();
    if !graph.is_fresh(&REPORT, &scope)? {
        return Ok(None);
    }
    match graph.query(&REPORT, &scope) {
        Ok(reported) => Ok(Some(reported.body.clone())),
        Err(Error::Incomplete(_)) => Ok(None),
        Err(other) => Err(other),
    }
}

/// Keeps `body` as the report of `scope`, made of the findings `reads`.
///
/// # Errors
///
/// A node could not be read or stored.
pub fn state_report(
    graph: &Graph,
    scope: &str,
    (reads, body): (Vec<Read>, String),
) -> Result<(), Error> {
    let runtime = Runtime::of(graph)?;
    runtime
        .statements
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stated
        .insert(scope.to_owned(), (reads, body));
    let result = graph.query(&REPORT, &scope.to_owned()).map(|_| ());
    runtime
        .statements
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stated
        .remove(scope);
    result
}

fn report(graph: &Graph, scope: &String) -> Result<Reported, Error> {
    graph.input(&RUN, &())?;
    graph.input(&VERSION, &"binary".to_owned())?;
    let runtime = Runtime::of(graph)?;
    let stated = runtime
        .statements
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stated
        .get(scope)
        .cloned();
    let Some((reads, body)) = stated else {
        return Err(Error::Incomplete("the report is to be stated".to_owned()));
    };
    for read in &reads {
        graph.query(&FINDINGS, read)?;
    }
    Ok(Reported { body })
}
