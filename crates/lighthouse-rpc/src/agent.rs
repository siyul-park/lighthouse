//! The agent of a plugin: `lighthouse/edit` runs in a process of its own, for
//! that one request, under the sandbox of a command agent. The long-lived
//! process of a plugin starts in the store and is not sandboxed; an agent that
//! edits files must never run in it, or the directory it is given would be
//! only a request that a write to the real tree could ignore.

use std::{
    collections::BTreeMap,
    io::Cursor,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_plugin::{EditRequest, Edited, Error};
use lighthouse_process::{Environment, Home, Job, Policy};
use lighthouse_protocol::{self as wire, Message, ToolEvent, read_message, write_message};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    events::{MAX_EVENTS, NOTES_BYTES, Roots, tail},
    plugin::folder,
};

/// The id the host gives each request of the conversation.
const INITIALIZE_ID: i64 = 1;
const EDIT_ID: i64 = 2;
const SHUTDOWN_ID: i64 = 3;

/// How a plugin's agent is run: a fresh process for each request.
#[derive(Debug, Clone)]
pub(crate) struct Agent {
    plugin: String,
    /// The executable and its arguments, as they are started.
    command: Vec<String>,
    /// The project's `[sandbox]`: hosts the agent may reach, places it may
    /// write besides the directory it is given.
    policy: Policy,
    /// The project the secrets to withhold are those of.
    root: PathBuf,
    timeout: Duration,
}

/// The result of `lighthouse/edit` as the host reads it: a plugin that sends
/// an event of a kind the host does not know, or a malformed one, is not
/// broken by it; the event is skipped and counted.
#[derive(Deserialize)]
struct Sent {
    #[serde(default)]
    said: Value,
    #[serde(default)]
    events: Value,
}

impl Agent {
    pub(crate) fn new(
        plugin: &str,
        command: Vec<String>,
        policy: &Policy,
        root: &Path,
        timeout: Duration,
    ) -> Self {
        Self {
            plugin: plugin.to_owned(),
            command,
            policy: policy.clone(),
            root: root.to_owned(),
            timeout,
        }
    }

    /// The same agent under another sandbox, for the project at `root`.
    pub(crate) fn confined(self, policy: &Policy, root: &Path) -> Self {
        Self {
            policy: policy.clone(),
            root: root.to_owned(),
            ..self
        }
    }

    /// Runs the agent in `request.dir`: a process started for this request
    /// that is told `initialize`, `lighthouse/edit` and `shutdown` in one go,
    /// sandboxed so that it may write the directory it is given and nothing
    /// else, with an environment that holds no secret of the user.
    pub(crate) fn edit(&self, request: &EditRequest<'_>) -> Result<Edited, Error> {
        let failed = |reason: String| Error::model(&self.plugin, reason);
        let home = self
            .policy
            .enabled
            .then(tempfile::tempdir)
            .transpose()
            .map_err(|e| failed(e.to_string()))?;
        let environment = Environment::new(
            &BTreeMap::new(),
            Vec::new(),
            home.as_ref().map_or(Home::User, |h| Home::Own(h.path())),
        )
        .map_err(failed)?;
        let mut profile = self.policy.profile(&self.root, request.dir);
        profile
            .writes
            .extend(home.iter().map(|h| h.path().to_owned()));
        profile.writes.push(request.dir.to_owned());
        let conversation = self.conversation(request).map_err(failed)?;
        let ran = Job {
            command: &self.command,
            dir: request.dir,
            stdin: &conversation,
            environment: &environment,
            timeout: self.timeout,
            policy: &self.policy,
            profile: &profile,
        }
        .run()
        .map_err(failed)?;
        let said_by = |reason: String| Error::model(&self.plugin, reason);
        ran.exit(&[0]).map_err(said_by)?;
        self.answer(&ran.stdout, ran.truncated, request.dir, &environment)
    }

    /// What the host says to the process, in order: the handshake, the
    /// request, and the end of the conversation.
    fn conversation(&self, request: &EditRequest<'_>) -> Result<Vec<u8>, String> {
        let dir = std::path::absolute(request.dir)
            .map_err(|e| format!("cannot absolutize the directory: {e}"))?;
        let initialize = wire::InitializeParams {
            protocol_version: wire::VERSION.to_owned(),
            client_info: wire::Implementation {
                name: "lighthouse".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            workspace_folders: vec![folder(&dir)?],
        };
        let edit = wire::EditParams {
            model: request.model.to_owned(),
            prompt: request.prompt.to_owned(),
            dir: dir.display().to_string(),
        };
        let messages = [
            Message::request(INITIALIZE_ID, wire::INITIALIZE, initialize),
            Message::request(EDIT_ID, wire::EDIT, edit),
            Message::request(SHUTDOWN_ID, wire::SHUTDOWN, ()),
            Ok(Message::notification(wire::EXIT)),
        ];
        let mut frames = Vec::new();
        for message in messages {
            let message = message.map_err(|e| e.to_string())?;
            write_message(&mut frames, &message).map_err(|e| e.to_string())?;
        }
        Ok(frames)
    }

    /// The `Edited` of what the process answered.
    fn answer(
        &self,
        stdout: &[u8],
        cut: bool,
        dir: &Path,
        environment: &Environment,
    ) -> Result<Edited, Error> {
        let failed = |reason: String| Error::model(&self.plugin, reason);
        let mut reader = Cursor::new(stdout);
        let (mut initialized, mut edited) = (None, None);
        while let Ok(Some(message)) = read_message(&mut reader) {
            match message.id {
                Some(wire::Id::Number(INITIALIZE_ID)) => initialized = Some(message),
                Some(wire::Id::Number(EDIT_ID)) => edited = Some(message),
                _ => {}
            }
        }
        let shaken: wire::InitializeResult = initialized
            .ok_or_else(|| failed("it did not answer `initialize`".to_owned()))?
            .into_result()
            .map_err(|e| failed(format!("sent a malformed `initialize` result: {e}")))?
            .map_err(|e| failed(format!("rejected `initialize`: {}", e.message)))?;
        if shaken.protocol_version != wire::VERSION || shaken.server_info.name != self.plugin {
            return Err(failed(format!(
                "answered `initialize` as `{}` speaking protocol {}",
                shaken.server_info.name, shaken.protocol_version
            )));
        }
        let Some(edited) = edited else {
            let why = if cut {
                "its output passed the cap before it answered `lighthouse/edit`"
            } else {
                "it did not answer `lighthouse/edit`"
            };
            return Err(failed(why.to_owned()));
        };
        let sent: Sent = edited
            .into_result()
            .map_err(|e| failed(format!("sent a malformed `lighthouse/edit` result: {e}")))?
            .map_err(|e| {
                failed(format!(
                    "rejected `lighthouse/edit` ({}): {}",
                    e.code, e.message
                ))
            })?;
        Ok(read_sent(sent, cut, dir, environment))
    }
}

/// What a plugin said it did, read leniently: events that are not understood
/// are skipped and counted, paths are made relative to `dir`, the said text and
/// the events are bounded and scrubbed like those of a command agent.
fn read_sent(sent: Sent, cut: bool, dir: &Path, environment: &Environment) -> Edited {
    let roots = Roots::of(dir);
    let mut notes = Vec::new();
    let mut skipped = 0usize;
    let mut events: Vec<ToolEvent> = Vec::new();
    match sent.events {
        Value::Array(items) => {
            for item in items {
                match serde_json::from_value::<ToolEvent>(item) {
                    Ok(event) if events.len() < MAX_EVENTS => {
                        let placed = roots.place(scrubbed(event, environment));
                        events.extend(roots.ground(vec![placed]));
                    }
                    _ => skipped += 1,
                }
            }
        }
        Value::Null => {}
        _ => notes.push("its events were not a list".to_owned()),
    }
    if skipped > 0 {
        notes.push(format!(
            "{skipped} event(s) were skipped: of a kind the host does not know, malformed, or past the first {MAX_EVENTS}"
        ));
    }
    if cut {
        notes.push("its output passed the cap".to_owned());
    }
    let said = match sent.said {
        Value::String(text) => environment.scrub(text.trim()),
        _ => String::new(),
    };
    Edited {
        said: tail(&said, NOTES_BYTES).to_owned(),
        events,
        incomplete: (!notes.is_empty()).then(|| notes.join("; ")),
    }
}

/// `event` with the secrets of the environment hidden in what it carries.
fn scrubbed(event: ToolEvent, environment: &Environment) -> ToolEvent {
    let hide = |text: Option<String>| text.map(|t| environment.scrub(&t));
    ToolEvent {
        path: hide(event.path),
        pattern: hide(event.pattern),
        command: hide(event.command),
        ..event
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read_in(dir: &Path, sent: Value, cut: bool) -> Edited {
        let environment = Environment::new(&BTreeMap::new(), Vec::new(), Home::User).unwrap();
        let sent: Sent = serde_json::from_value(sent).unwrap();
        read_sent(sent, cut, dir, &environment)
    }

    fn read(sent: Value, cut: bool) -> Edited {
        read_in(Path::new("/work/scratch"), sent, cut)
    }

    #[test]
    fn read_sent_skips_an_event_of_an_unknown_tool_and_counts_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.go"), "x").unwrap();
        let file = dir.path().join("a.go");
        let edited = read_in(
            dir.path(),
            json!({"said": "ok", "events": [
                {"tool": "teleport", "path": "a"},
                {"tool": "read", "path": file},
                {"tool": "read", "path": 7},
                "nonsense",
            ]}),
            false,
        );

        assert_eq!(edited.events.len(), 1);
        assert_eq!(edited.events[0].path.as_deref(), Some("a.go"));
        let note = edited.incomplete.unwrap();
        assert!(note.starts_with("3 event(s) were skipped"), "{note}");
    }

    #[test]
    fn read_sent_notes_events_that_are_not_a_list() {
        let edited = read(json!({"events": {"tool": "read"}}), false);

        assert!(edited.events.is_empty());
        assert!(edited.incomplete.unwrap().contains("not a list"));
    }

    #[test]
    fn read_sent_keeps_the_end_of_what_was_said_to_the_cap() {
        let said = format!("{}{}", "e".repeat(5000), "fixed");
        let edited = read(json!({"said": said}), false);

        assert_eq!(edited.said.len(), NOTES_BYTES);
        assert!(edited.said.ends_with("fixed"));
        assert_eq!(edited.incomplete, None);
    }

    #[test]
    fn read_sent_says_when_the_output_was_cut() {
        let edited = read(json!({}), true);

        assert!(edited.incomplete.unwrap().contains("cap"));
    }

    #[test]
    fn read_sent_keeps_no_more_events_than_the_cap() {
        let events = vec![json!({"tool": "run", "command": "x"}); MAX_EVENTS + 3];

        let edited = read(json!({"events": events}), false);

        assert_eq!(edited.events.len(), MAX_EVENTS);
        assert!(edited.incomplete.unwrap().starts_with("3 event(s)"));
    }
}
