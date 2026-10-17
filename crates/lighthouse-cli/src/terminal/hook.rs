//! `lighthouse hook claude-code <event>`: the adapter between Claude Code's
//! hook protocol and `check`. It reads the hook payload on stdin, picks the
//! report scope, runs the check and answers in the format Claude Code reads.
//! All judgment (what is a finding, what blocks) is Lighthouse's; the hook
//! only translates. It reports what the session introduced: PreToolUse records
//! a file's findings before the session's first edit of it (see `baseline`),
//! PostToolUse and Stop keep the findings that are new or updated against it.
//!
//! Contract used (Claude Code hooks reference): the payload carries `session_id`,
//! `cwd`, `hook_event_name`, and for PreToolUse and PostToolUse `tool_name` and
//! `tool_input` (`file_path` for Edit, Write and MultiEdit); for Stop
//! `stop_hook_active` says a stop hook already forced the agent to continue. Exit 0 with a JSON
//! object on stdout: `decision: "block"` with `reason` feeds the reason back to
//! the agent (after PostToolUse the edit already happened; at Stop it keeps the
//! agent going), `hookSpecificOutput.additionalContext` adds context without
//! blocking, `systemMessage` shows the user a note. Exit 2 is never used: a
//! failing hook must not block the agent. Only a payload that is not JSON
//! exits 1 (non-blocking), and so does a PreToolUse that cannot record its
//! baseline; for the other events a broken config, broken local decisions or a
//! plugin that cannot start are told to the agent as "nothing was checked".

use std::{
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use lighthouse_report::Format;
use lighthouse_session::{CheckRequest, Checked, ModelOptions, RunStatus, Session, baseline};
use serde_json::{Value, json};

use crate::terminal::Result;

/// Findings printed back to the agent by an edit and by the end of a turn.
const EDIT_LIMIT: usize = 5;
const STOP_LIMIT: usize = 10;
/// The tools of Claude Code that edit a file.
pub const EDIT_TOOLS: [&str; 3] = ["Edit", "Write", "MultiEdit"];
/// What the command of every hook starts with.
pub const COMMAND: &str = "lighthouse hook claude-code";
/// Said with a report that had no baseline to be measured against.
const NO_BASELINE: &str = "Lighthouse: no pre-edit baseline, so every finding in the file is shown, including ones that were there before this session; `lighthouse init --agent claude-code` adds the PreToolUse hook that records it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Event {
    /// Before Edit, Write or MultiEdit: record the file's findings as the
    /// session's baseline, once per file; prints nothing.
    PreToolUse,
    /// After Edit, Write or MultiEdit: report the findings the edit introduced.
    PostToolUse,
    /// When the agent wants to stop: report the findings the session introduced.
    Stop,
}

impl Event {
    /// The name Claude Code gives the event.
    pub fn name(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::Stop => "Stop",
        }
    }

    /// The command that runs the hook of the event: only the first, which
    /// records a baseline and prints nothing, has no incompleteness to allow.
    pub fn command(self) -> String {
        let event = self.to_possible_value().expect("every event has a name");
        let flag = if self == Self::PreToolUse {
            ""
        } else {
            " --allow-incomplete"
        };
        format!("{COMMAND} {}{flag}", event.get_name())
    }
}

/// Runs the hook; a failure is reported on stderr and exits 1 (non-blocking).
pub fn run(event: Event, allow_incomplete: bool) -> u8 {
    let mut payload = String::new();
    let answered = std::io::stdin()
        .read_to_string(&mut payload)
        .map_err(Into::into)
        .and_then(|_| reply(event, allow_incomplete, &payload));
    match answered {
        Ok(Some(reply)) => {
            let _ = writeln!(std::io::stdout(), "{reply}");
            0
        }
        Ok(None) => 0,
        Err(e) => {
            eprintln!("lighthouse hook: {e}");
            1
        }
    }
}

/// The JSON to print for `payload`, or `None` to stay silent. Only a payload
/// that is not JSON is an error (nothing can be said to the agent about it);
/// whatever else goes wrong is told to the agent, never swallowed.
fn reply(event: Event, allow_incomplete: bool, payload: &str) -> Result<Option<Value>> {
    let payload: Value =
        serde_json::from_str(payload).map_err(|e| format!("the hook payload is not JSON: {e}"))?;
    let active = payload["stop_hook_active"].as_bool().unwrap_or(false);
    match answer_event(event, allow_incomplete, active, &payload) {
        Err(e) => unchecked(event, active, &e.to_string()).map(Some).ok_or(e),
        reply => reply,
    }
}

/// The project is the one `cwd` of the payload belongs to; a directory with
/// no `lighthouse.toml` is not a Lighthouse project and gets silence, but a
/// config or local decisions that are broken are an error.
fn answer_event(
    event: Event,
    allow_incomplete: bool,
    stop_hook_active: bool,
    payload: &Value,
) -> Result<Option<Value>> {
    let here = match payload["cwd"].as_str() {
        Some(cwd) => PathBuf::from(cwd),
        None => env::current_dir()?,
    };
    let Some(session) = Session::find_in(&here)? else {
        return Ok(None);
    };
    let root = session.root.canonicalize()?;
    let id = baseline::session_id(payload["session_id"].as_str());
    match event {
        Event::PreToolUse => {
            let (Some(id), Some(file)) = (id, edited_file(payload, &root)) else {
                return Ok(None);
            };
            if baseline::exists(&root, id, &file) {
                return Ok(None);
            }
            let found = if root.join(&file).exists() {
                lighthouse_session::check(&session, &request(vec![root.join(&file)], false))?
                    .run
                    .diagnostics
            } else {
                Vec::new()
            };
            baseline::record(&root, id, &baseline::Baseline::of(&file, &found))?;
            Ok(None)
        }
        Event::PostToolUse => {
            let Some(file) = edited_file(payload, &root).filter(|f| root.join(f).exists()) else {
                return Ok(None);
            };
            let mut checked =
                lighthouse_session::check(&session, &request(vec![root.join(&file)], true))?;
            let before = id.and_then(|id| baseline::load(&root, id, &file));
            if let Some(before) = &before {
                baseline::introduced(&mut checked.run.diagnostics, std::slice::from_ref(before));
            }
            Ok(after_edit(&checked, allow_incomplete, before.is_none()))
        }
        Event::Stop => {
            let before = id
                .map(|id| baseline::load_all(&root, id))
                .unwrap_or_default();
            let files: Vec<PathBuf> = before
                .iter()
                .map(|b| root.join(b.path()))
                .filter(|f| f.exists())
                .collect();
            if files.is_empty() {
                return Ok(None);
            }
            let mut checked = lighthouse_session::check(&session, &request(files, true))?;
            baseline::introduced(&mut checked.run.diagnostics, &before);
            Ok(at_stop(&checked, allow_incomplete, stop_hook_active))
        }
    }
}

/// What the agent hears when Lighthouse could not run at all: a broken config
/// or local rule, a plugin that cannot start. Nothing was checked, and that is
/// not a pass.
/// A PreToolUse tells the agent nothing: its failure is the hook's exit code.
fn unchecked(event: Event, stop_hook_active: bool, error: &str) -> Option<Value> {
    let text = format!(
        "Lighthouse config broken or unusable, nothing was checked (this is not a pass): {error}"
    );
    match event {
        Event::PreToolUse => None,
        Event::PostToolUse => Some(context(event, text)),
        Event::Stop if stop_hook_active => Some(notice(text)),
        Event::Stop => Some(block(text)),
    }
}

/// The answer that feeds `reason` back to the agent: after an edit it has
/// happened, at Stop it keeps the agent going.
fn block(reason: String) -> Value {
    json!({ "decision": "block", "reason": reason })
}

/// The answer that adds `text` to the agent's context without blocking it.
fn context(event: Event, text: String) -> Value {
    json!({ "hookSpecificOutput": { "hookEventName": event.name(), "additionalContext": text } })
}

/// The answer that shows the user `text`.
fn notice(text: String) -> Value {
    json!({ "systemMessage": text })
}

/// A check of `paths`; a baseline is not a run worth remembering (`store`).
fn request(paths: Vec<PathBuf>, store: bool) -> CheckRequest {
    CheckRequest {
        paths,
        store,
        cache: true,
        // An edit hook never spends on a model.
        models: ModelOptions {
            off: true,
            ..ModelOptions::default()
        },
        ..CheckRequest::default()
    }
}

/// The project-relative path an editing tool touches, when it is worth
/// checking (`root` is canonical).
fn edited_file(payload: &Value, root: &Path) -> Option<PathBuf> {
    let tool = payload["tool_name"].as_str()?;
    if !EDIT_TOOLS.contains(&tool) {
        return None;
    }
    baseline::editable(
        Path::new(payload["tool_input"]["file_path"].as_str()?),
        root,
    )
}

/// An edit: error and warn findings go back to the agent as the reason of a
/// block so that it fixes them (or judges them, when their decision asks for a
/// judgment); the remaining findings that ask for review, and incompleteness,
/// are context. Clean and complete is silent.
fn after_edit(checked: &Checked, allow_incomplete: bool, unbaselined: bool) -> Option<Value> {
    let note = unbaselined.then_some(NO_BASELINE);
    let summary = checked.summary();
    let incomplete = summary.status == RunStatus::Incomplete;
    if summary.errors + summary.warnings > 0 || (incomplete && !allow_incomplete) {
        return Some(block(format!(
            "Lighthouse found problems in the file you just edited. Fix them, then continue.\n{}{}",
            checked.render(Format::Agent, Some(EDIT_LIMIT)),
            note.map_or_else(String::new, |n| format!("\n{n}")),
        )));
    }
    let mut notes = Vec::new();
    if summary.reviews > 0 {
        notes.push(format!(
            "Lighthouse: {} finding(s) in the edited file ask for review, not necessarily a fix. \
             List them with the MCP tool `review_list`, then fix the code or record a judgment with \
             `review_resolve` (a reason is required to reject).",
            summary.reviews
        ));
    }
    if incomplete {
        notes.push(format!(
            "Lighthouse: the analysis was INCOMPLETE, so this file is not known to be clean:\n{}",
            checked.render(Format::Agent, Some(0))
        ));
    }
    (!notes.is_empty()).then(|| {
        notes.extend(note.map(str::to_owned));
        context(Event::PostToolUse, notes.join("\n"))
    })
}

/// The end of a turn. Remaining errors, and an incomplete analysis, are told
/// to the agent in one block; a stop that this hook already forced is allowed
/// so that the agent cannot be trapped, and the user gets a note instead.
/// `allow_incomplete` only changes the wording: the agent learns about the
/// gap either way, but is not asked to clear it.
fn at_stop(checked: &Checked, allow_incomplete: bool, stop_hook_active: bool) -> Option<Value> {
    let summary = checked.summary();
    let incomplete = summary.status == RunStatus::Incomplete;
    let blocking = summary.errors > 0 || incomplete;
    if blocking && !stop_hook_active {
        let mut why = Vec::new();
        if summary.errors > 0 {
            why.push(format!(
                "{} error(s) introduced this session remain; fix them before stopping.",
                summary.errors
            ));
        }
        if incomplete {
            why.push(if allow_incomplete {
                "The analysis was INCOMPLETE, so the files edited this session are not known to be clean. Look at the gaps below and tell the user; this notice is shown once."
            } else {
                "The analysis was INCOMPLETE, so the files edited this session are not known to be clean. Resolve the gaps below."
            }.to_owned());
        }
        return Some(block(format!(
            "Lighthouse: {}\n{}",
            why.join(" "),
            checked.render(Format::Agent, Some(STOP_LIMIT))
        )));
    }
    let mut parts = Vec::new();
    if summary.errors > 0 {
        parts.push(format!("{} error(s) still remain", summary.errors));
    }
    for (n, what) in [
        (summary.warnings, "warning(s)"),
        (summary.infos, "info finding(s)"),
        (summary.reviews, "finding(s) asking for review"),
        (summary.incomplete, "incomplete gap(s)"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    let message = if parts.is_empty() {
        String::new()
    } else {
        format!("Lighthouse: {} introduced this session.", parts.join(", "))
    };
    (!message.is_empty()).then(|| notice(message))
}
