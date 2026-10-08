//! `lighthouse hook claude-code <event>`: the adapter between Claude Code's
//! hook protocol and `check`. It reads the hook payload on stdin, picks the
//! report scope, runs the check and answers in the format Claude Code reads.
//! All judgment (what is a finding, what blocks) is Lighthouse's; the hook
//! only translates.
//!
//! Contract used (Claude Code hooks reference): the payload carries `cwd`,
//! `hook_event_name`, and for PostToolUse `tool_name` and `tool_input`
//! (`file_path` for Edit, Write and MultiEdit); for Stop `stop_hook_active`
//! says a stop hook already forced the agent to continue. Exit 0 with a JSON
//! object on stdout: `decision: "block"` with `reason` feeds the reason back to
//! the agent (after PostToolUse the edit already happened; at Stop it keeps the
//! agent going), `hookSpecificOutput.additionalContext` adds context without
//! blocking, `systemMessage` shows the user a note. Exit 2 is never used: a
//! failing hook must not block the agent. Only a payload that is not JSON
//! exits 1 (non-blocking); a broken config, broken local decisions or a plugin
//! that cannot start are told to the agent as "nothing was checked".

use std::{
    env,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use lighthouse_report::{Format, render_with};
use lighthouse_session::{CheckRequest, Checked, Session, Status};
use serde_json::{Value, json};

use crate::Result;

/// Findings printed back to the agent by an edit and by the end of a turn.
const EDIT_LIMIT: usize = 5;
const STOP_LIMIT: usize = 10;
/// Bytes read to tell text from binary.
const SNIFF: usize = 8192;
/// Directories whose files are never analyzed.
const SKIPPED_DIRS: [&str; 3] = [".git", ".lighthouse", "node_modules"];
const EDIT_TOOLS: [&str; 3] = ["Edit", "Write", "MultiEdit"];

#[derive(Clone, Copy, ValueEnum)]
pub enum Event {
    /// After Edit, Write or MultiEdit: report the edited file.
    PostToolUse,
    /// When the agent wants to stop: report the session's changed files.
    Stop,
}

/// Runs the hook; a failure is reported on stderr and exits 1 (non-blocking).
pub fn run(event: Event, allow_incomplete: bool) -> u8 {
    match answer(event, allow_incomplete) {
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

fn answer(event: Event, allow_incomplete: bool) -> Result<Option<Value>> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    reply(event, allow_incomplete, &text)
}

/// The JSON to print for `payload`, or `None` to stay silent. Only a payload
/// that is not JSON is an error (nothing can be said to the agent about it);
/// whatever else goes wrong is told to the agent, never swallowed.
fn reply(event: Event, allow_incomplete: bool, payload: &str) -> Result<Option<Value>> {
    let payload: Value =
        serde_json::from_str(payload).map_err(|e| format!("the hook payload is not JSON: {e}"))?;
    let active = payload["stop_hook_active"].as_bool().unwrap_or(false);
    match answer_event(event, allow_incomplete, &payload) {
        Ok(reply) => Ok(reply),
        Err(e) => Ok(Some(unchecked(event, active, &e.to_string()))),
    }
}

/// The project is the one `cwd` of the payload belongs to; a directory with
/// no `lighthouse.toml` is not a Lighthouse project and gets silence, but a
/// config or local decisions that are broken are an error.
fn answer_event(event: Event, allow_incomplete: bool, payload: &Value) -> Result<Option<Value>> {
    let here = match payload["cwd"].as_str() {
        Some(cwd) => PathBuf::from(cwd),
        None => env::current_dir()?,
    };
    let Some(session) = Session::find_in(&here)? else {
        return Ok(None);
    };
    match event {
        Event::PostToolUse => {
            let Some(file) = edited_file(payload, &session.root) else {
                return Ok(None);
            };
            let checked = lighthouse_session::check(session, &request(vec![file], false))?;
            Ok(after_edit(&checked, allow_incomplete))
        }
        Event::Stop => {
            let (changed, note) = match lighthouse_session::changed(&session.root) {
                Ok(_) => (true, None),
                Err(e) => (
                    false,
                    Some(format!(
                        "the changed files could not be listed ({e}), so the whole project was reported"
                    )),
                ),
            };
            let checked = lighthouse_session::check(session, &request(Vec::new(), changed))?;
            let active = payload["stop_hook_active"].as_bool().unwrap_or(false);
            Ok(at_stop(&checked, allow_incomplete, active, note))
        }
    }
}

/// What the agent hears when Lighthouse could not run at all: a broken config
/// or local rule, a plugin that cannot start. Nothing was checked, and that is
/// not a pass.
fn unchecked(event: Event, stop_hook_active: bool, error: &str) -> Value {
    let text = format!(
        "Lighthouse config broken or unusable, nothing was checked (this is not a pass): {error}"
    );
    match event {
        Event::PostToolUse => json!({
            "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": text }
        }),
        Event::Stop if stop_hook_active => json!({ "systemMessage": text }),
        Event::Stop => json!({ "decision": "block", "reason": text }),
    }
}

fn request(paths: Vec<PathBuf>, changed: bool) -> CheckRequest {
    CheckRequest {
        paths,
        changed,
        diff: None,
        rules: Vec::new(),
        store: true,
    }
}

/// The file an editing tool changed, when it is worth checking: it exists,
/// lies inside the project, is not in a directory Lighthouse never reads, and
/// is text. Everything else is a cheap `None`, before any plugin starts.
fn edited_file(payload: &Value, root: &Path) -> Option<PathBuf> {
    let tool = payload["tool_name"].as_str()?;
    if !EDIT_TOOLS.contains(&tool) {
        return None;
    }
    let path = Path::new(payload["tool_input"]["file_path"].as_str()?);
    let absolute = path.canonicalize().ok()?;
    let relative = absolute.strip_prefix(root.canonicalize().ok()?).ok()?;
    let skipped = relative
        .components()
        .any(|c| SKIPPED_DIRS.iter().any(|d| c.as_os_str() == *d));
    (!skipped && is_text(&absolute)).then_some(absolute)
}

fn is_text(path: &Path) -> bool {
    let mut head = Vec::with_capacity(SNIFF);
    let Ok(file) = File::open(path) else {
        return false;
    };
    file.take(SNIFF as u64)
        .read_to_end(&mut head)
        .is_ok_and(|_| !head.contains(&0))
}

/// An edit: error and warn findings go back to the agent as the reason of a
/// block so that it fixes them (or judges them, when their decision asks for a
/// verdict); the remaining findings that ask for a verdict, and incompleteness,
/// are context. Clean and complete is silent.
fn after_edit(checked: &Checked, allow_incomplete: bool) -> Option<Value> {
    let summary = checked.summary();
    let incomplete = summary.status == Status::Incomplete;
    if summary.errors + summary.warnings > 0 || (incomplete && !allow_incomplete) {
        return Some(json!({
            "decision": "block",
            "reason": format!(
                "Lighthouse found problems in the file you just edited. Fix them, then continue.\n{}",
                feedback(checked, EDIT_LIMIT)
            ),
        }));
    }
    let mut notes = Vec::new();
    if summary.reviews > 0 {
        notes.push(format!(
            "Lighthouse: {} finding(s) in the edited file ask for a verdict, not necessarily a fix. \
             List them with the MCP tool `review_tasks`, then fix the code or record a verdict with \
             `review_resolve` (a reason is required to reject).",
            summary.reviews
        ));
    }
    if incomplete {
        notes.push(format!(
            "Lighthouse: the analysis was INCOMPLETE, so this file is not known to be clean:\n{}",
            feedback(checked, 0)
        ));
    }
    (!notes.is_empty()).then(|| {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": notes.join("\n"),
            }
        })
    })
}

/// The end of a turn. Remaining errors, and an incomplete analysis, are told
/// to the agent in one block; a stop that this hook already forced is allowed
/// so that the agent cannot be trapped, and the user gets a note instead.
/// `allow_incomplete` only changes the wording: the agent learns about the
/// gap either way, but is not asked to clear it.
fn at_stop(
    checked: &Checked,
    allow_incomplete: bool,
    stop_hook_active: bool,
    note: Option<String>,
) -> Option<Value> {
    let summary = checked.summary();
    let incomplete = summary.status == Status::Incomplete;
    let blocking = summary.errors > 0 || incomplete;
    if blocking && !stop_hook_active {
        let mut why = Vec::new();
        if summary.errors > 0 {
            why.push(format!(
                "{} error(s) remain in the files changed this session; fix them before stopping.",
                summary.errors
            ));
        }
        if incomplete {
            why.push(if allow_incomplete {
                "The analysis was INCOMPLETE, so the changed files are not known to be clean. Look at the gaps below and tell the user; this notice is shown once."
            } else {
                "The analysis was INCOMPLETE, so the changed files are not known to be clean. Resolve the gaps below."
            }.to_owned());
        }
        why.extend(note);
        return Some(json!({
            "decision": "block",
            "reason": format!("Lighthouse: {}\n{}", why.join(" "), feedback(checked, STOP_LIMIT)),
        }));
    }
    let mut parts = Vec::new();
    if summary.errors > 0 {
        parts.push(format!("{} error(s) still remain", summary.errors));
    }
    for (n, what) in [
        (summary.warnings, "warning(s)"),
        (summary.infos, "info finding(s)"),
        (summary.reviews, "finding(s) asking for a verdict"),
        (summary.incomplete, "incomplete gap(s)"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    let mut message = if parts.is_empty() {
        String::new()
    } else {
        format!("Lighthouse: {} in the changed files.", parts.join(", "))
    };
    if let Some(note) = note {
        message = format!("{message} Lighthouse: {note}.").trim().to_owned();
    }
    (!message.is_empty()).then(|| json!({ "systemMessage": message }))
}

/// The agent-format text of the findings and gaps, at most `limit` findings.
fn feedback(checked: &Checked, limit: usize) -> String {
    let outcome = &checked.outcome;
    render_with(
        Format::Agent,
        &outcome.diagnostics,
        &outcome.incomplete,
        &checked.briefing(Some(limit)),
    )
}
