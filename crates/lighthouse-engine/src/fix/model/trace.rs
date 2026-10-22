//! What an agent did in a run, from the events its harness reported: the
//! files it opened that the brief did not give it, what it searched and
//! listed, and how many calls of each tool it made. A measurement for the
//! report; no decision of a fix depends on it.

use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
};

use lighthouse_protocol::{Tool, ToolEvent};

/// What stands in `reads` for every file outside the project the agent opened.
const OUTSIDE: &str = "(outside the project)";

/// The calls of each tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
mod tests {
    use super::*;

    fn event(tool: Tool, path: Option<&str>, pattern: Option<&str>) -> ToolEvent {
        ToolEvent {
            tool,
            outside: false,
            path: path.map(str::to_owned),
            pattern: pattern.map(str::to_owned),
            command: None,
        }
    }

    #[test]
    fn a_read_the_brief_did_not_give_is_listed_once_and_every_call_is_counted() {
        let briefed = BTreeSet::from(["src/lib.rs".to_owned()]);
        let events = [
            event(Tool::Read, Some("src/lib.rs"), None),
            event(Tool::Read, Some("src/other.rs"), None),
            event(Tool::Read, Some("src/other.rs"), None),
            event(Tool::Search, Some("."), Some("bad_one")),
            event(Tool::List, Some("src"), None),
            event(Tool::Edit, Some("src/lib.rs"), None),
        ];

        let trace = BatchTrace::of(PathBuf::from("src/lib.rs"), &briefed, &events, None);

        assert_eq!(trace.reads, ["src/other.rs"]);
        assert_eq!(trace.searches, [events[3].clone(), events[4].clone()]);
        assert_eq!(
            trace.counts,
            ToolCounts {
                read: 3,
                search: 1,
                list: 1,
                edit: 1,
                run: 0
            }
        );
    }

    #[test]
    fn of_lists_every_read_outside_the_project_as_one_entry() {
        let outside = ToolEvent {
            outside: true,
            ..event(Tool::Read, None, None)
        };
        let events = [
            outside.clone(),
            outside,
            event(Tool::Read, Some("a.go"), None),
        ];

        let trace = BatchTrace::of(PathBuf::from("a.go"), &BTreeSet::new(), &events, None);

        assert_eq!(trace.reads, [OUTSIDE, "a.go"]);
    }

    #[test]
    fn of_carries_why_the_events_are_not_everything() {
        let note = Some("2 event(s) past the first 10000 were not kept".to_owned());

        let trace = BatchTrace::of(PathBuf::from("a.go"), &BTreeSet::new(), &[], note.clone());

        assert_eq!(trace.incomplete, note);
    }

    #[test]
    fn of_lists_a_thousand_distinct_reads_without_comparing_each_to_all() {
        let events: Vec<ToolEvent> = (0..5000)
            .map(|n| event(Tool::Read, Some(&format!("f{}.go", n % 2500)), None))
            .collect();

        let trace = BatchTrace::of(PathBuf::from("a.go"), &BTreeSet::new(), &events, None);

        assert_eq!(trace.reads.len(), 2500);
        assert_eq!(trace.counts.read, 5000);
    }
}
