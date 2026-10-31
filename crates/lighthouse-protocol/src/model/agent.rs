//! The agent method: `lighthouse/edit` runs an agent in a directory, and the
//! tool events are what it did there, in one vocabulary whatever ran it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Params of `lighthouse/edit`: the agent changes the files of `dir` as the
/// prompt says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// The agent asked: the part of the reference after the provider.
    pub model: String,
    /// What the agent is asked to do, as plain text.
    pub prompt: String,
    /// The directory it runs in and may change.
    pub dir: String,
}

/// Result of `lighthouse/edit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditResult {
    /// What the agent said about it.
    #[serde(default)]
    pub said: String,
    /// What the agent read, searched, listed, edited and ran, when the plugin
    /// can tell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ToolEvent>,
}

/// One thing an agent did, as its harness reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolEvent {
    pub tool: Tool,
    /// The file or directory it was about, relative to the directory the
    /// agent ran in, as the host normalizes it. A plugin may send it absolute
    /// or relative to that directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// It was about a path outside the directory the agent ran in. The host
    /// sets this and drops the path, so that no path of the machine reaches a
    /// report; a plugin leaves it out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
    /// What a `search` looked for, or the glob of a `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The command line of a `run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// What an agent did with a tool: the kinds of its work that a fix report
/// tells apart.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    /// Opened a file.
    Read,
    /// Searched file contents for a pattern.
    Search,
    /// Listed files or a directory.
    List,
    /// Wrote or changed a file.
    Edit,
    /// Ran a command.
    Run,
}

impl Tool {
    /// The tool a word names, as `read`, `search`, `list`, `edit` or `run`.
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "read" => Self::Read,
            "search" => Self::Search,
            "list" => Self::List,
            "edit" => Self::Edit,
            "run" => Self::Run,
            _ => return None,
        })
    }
}
