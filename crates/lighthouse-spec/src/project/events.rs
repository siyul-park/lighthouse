//! How a `command` agent's stdout becomes tool events: a mapping in CEL over
//! the JSON lines a harness prints, written in the configuration that declares
//! the provider, so that no harness is known to the host.

use std::collections::BTreeMap;

use lighthouse_protocol::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::environment::compile;

/// The mapping of the lines of an agent's stdout to the events of a report.
/// Each expression sees `line`, the JSON value of one stdout line (a line
/// that is not JSON is skipped), and `tools`, the table below; the keys that
/// describe one event also see `item` when `each` is set. The functions are
/// those of the one CEL environment (its strings, lists, math and encoders
/// extensions, and the optional syntax `line.?name.orValue("")`, since the
/// lines of a harness are as free in shape as they come) and `commands(text)`,
/// which reads a shell command line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventsSpec {
    /// A bool: whether the line holds tool calls. A line it is false or
    /// fails for is skipped.
    pub select: String,
    /// A list: one event for each of its elements, each seen as `item` by the
    /// keys below, for a line that holds several calls (a shell command with
    /// a pipe, a change of several files). Without it a line is one event. A
    /// failure is no event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<String>,
    /// The tool of the call: `read`, `search`, `list`, `edit` or `run`,
    /// usually `tools[line.<name of the harness's tool>]`. A call whose tool
    /// is none of them, or that fails here (a harness tool the table lacks),
    /// is not an event.
    pub tool: String,
    /// The file or directory the call is about, or a list of them for one
    /// event each (a call without any is one event without a path); a path
    /// inside the directory the agent ran in is made relative to it, and one
    /// outside it is kept apart as `outside`. A failure is no path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What a `search` looks for, or the glob of a `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The command line of a `run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// A string: what the agent said in the line, for the notes of the fix
    /// (its reply of what is fixed and what is left). Read from every line,
    /// selected or not; the pieces are joined by line breaks, and the last
    /// few kilobytes are kept. Without it an agent whose output is mapped
    /// says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// The harness's tool names, each with the tool it is: a table to look up
    /// with `tools[name]` or `name in tools`, not one to search.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, String>,
}

impl EventsSpec {
    /// The expressions with the key each is written under.
    pub fn expressions(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("select", Some(self.select.as_str())),
            ("each", self.each.as_deref()),
            ("tool", Some(self.tool.as_str())),
            ("path", self.path.as_deref()),
            ("pattern", self.pattern.as_deref()),
            ("command", self.command.as_deref()),
            ("said", self.said.as_deref()),
        ]
        .into_iter()
        .filter_map(|(key, source)| Some((key, source?)))
    }

    /// Why the mapping cannot work: an expression that does not compile, or a
    /// harness tool mapped to something that is not a tool.
    pub fn problem(&self) -> Option<String> {
        for (key, source) in self.expressions() {
            if let Err(e) = compile(source) {
                return Some(format!("`events.{key}`: {e}"));
            }
        }
        self.tools.iter().find_map(|(name, tool)| {
            Tool::parse(tool).is_none().then(|| {
                format!(
                    "`events.tools.{name}` is `{tool}`, not a tool: read, search, list, edit or run"
                )
            })
        })
    }
}
