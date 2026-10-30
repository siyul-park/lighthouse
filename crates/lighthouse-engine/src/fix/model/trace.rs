//! What an agent did in a run, from the events its harness reported: the
//! files it opened that the brief did not give it, what it searched and
//! listed, and how many calls of each tool it made. A measurement for the
//! report; no decision of a fix depends on it.

use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
};

use lighthouse_protocol::{Tool, ToolEvent};
use schemars::JsonSchema;
use serde::Serialize;

/// What stands in `reads` for every file outside the project the agent opened.
const OUTSIDE: &str = "(outside the project)";

/// The calls of each tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ToolCounts {
    pub read: usize,
    pub search: usize,
    pub list: usize,
    pub edit: usize,
    pub run: usize,
}

/// What the agent did for the findings of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTrace {
    /// The file the findings are in.
    pub file: PathBuf,
    /// Files it read that the brief did not include, in the order it opened
    /// them, each once.
    pub reads: Vec<String>,
    /// What it searched, and the listings it made, in order.
    pub searches: Vec<ToolEvent>,
    pub counts: ToolCounts,
    /// Why this is not all the agent did, when it is not: its output was cut,
    /// events were past the count kept or were not understood.
    pub incomplete: Option<String>,
}

impl BatchTrace {
    /// The trace of `events`, for a brief that included the files `briefed`;
    /// `incomplete` says why they are not everything, if they are not.
    pub(super) fn of(
        file: PathBuf,
        briefed: &BTreeSet<String>,
        events: &[ToolEvent],
        incomplete: Option<String>,
    ) -> Self {
        let mut counts = ToolCounts::default();
        let mut reads: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut searches = Vec::new();
        for event in events {
            match event.tool {
                Tool::Read => {
                    counts.read += 1;
                    let path = if event.outside {
                        Some(OUTSIDE)
                    } else {
                        event.path.as_deref()
                    };
                    if let Some(path) = path
                        && !briefed.contains(path)
                        && seen.insert(path)
                    {
                        reads.push(path.to_owned());
                    }
                }
                Tool::Search => {
                    counts.search += 1;
                    searches.push(event.clone());
                }
                Tool::List => {
                    counts.list += 1;
                    searches.push(event.clone());
                }
                Tool::Edit => counts.edit += 1,
                Tool::Run => counts.run += 1,
            }
        }
        Self {
            file,
            reads,
            searches,
            counts,
            incomplete,
        }
    }
}

#[cfg(test)]
mod tests;
