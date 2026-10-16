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
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use lighthouse_report::{Briefing, Detail, Format, render};
use lighthouse_resource::STATE_DIR;
use lighthouse_session::{CheckRequest, Checked, RunStatus, SKIPPED_DIRS, Session};
use serde_json::{Value, json};

use crate::cli::{Result, baseline};

/// Findings printed back to the agent by an edit and by the end of a turn.
const EDIT_LIMIT: usize = 5;
const STOP_LIMIT: usize = 10;
/// Bytes read to tell text from binary.
const SNIFF: usize = 8192;
const EDIT_TOOLS: [&str; 3] = ["Edit", "Write", "MultiEdit"];
/// Said with a report that had no baseline to be measured against.
const NO_BASELINE: &str = "Lighthouse: no pre-edit baseline, so every finding in the file is shown, including ones that were there before this session; `lighthouse init --agent claude-code` adds the PreToolUse hook that records it.";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Event {
    /// Before Edit, Write or MultiEdit: record the file's findings as the
    /// session's baseline, once per file; prints nothing.
    PreToolUse,
    /// After Edit, Write or MultiEdit: report the findings the edit introduced.
    PostToolUse,
    /// When the agent wants to stop: report the findings the session introduced.
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
        Err(e) => unchecked(event, active, &e.to_string()).map(Some).ok_or(e),
        reply => reply,
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
    let root = session.root.canonicalize()?;
    let id = baseline::session_id(payload);
    match event {
        Event::PreToolUse => {
            let (Some(id), Some(file)) = (id, edited_file(payload, &root)) else {
                return Ok(None);
            };
            if baseline::exists(&root, id, &file) {
                return Ok(None);
            }
            let found = if root.join(&file).exists() {
                lighthouse_session::check(session, &request(vec![root.join(&file)], false))?
                    .outcome
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
                lighthouse_session::check(session, &request(vec![root.join(&file)], true))?;
            let before = id.and_then(|id| baseline::load(&root, id, &file));
            if let Some(before) = &before {
                baseline::introduced(
                    &mut checked.outcome.diagnostics,
                    std::slice::from_ref(before),
                );
            }
            Ok(after_edit(&checked, allow_incomplete, before.is_none()))
        }
        Event::Stop => {
            let before = id
                .map(|id| baseline::session(&root, id))
                .unwrap_or_default();
            let files: Vec<PathBuf> = before
                .iter()
                .map(|b| root.join(b.path()))
                .filter(|f| f.exists())
                .collect();
            if files.is_empty() {
                return Ok(None);
            }
            let mut checked = lighthouse_session::check(session, &request(files, true))?;
            baseline::introduced(&mut checked.outcome.diagnostics, &before);
            let active = payload["stop_hook_active"].as_bool().unwrap_or(false);
            Ok(at_stop(&checked, allow_incomplete, active))
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
        Event::PostToolUse => Some(json!({
            "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": text }
        })),
        Event::Stop if stop_hook_active => Some(json!({ "systemMessage": text })),
        Event::Stop => Some(json!({ "decision": "block", "reason": text })),
    }
}

/// A check of `paths`; a baseline is not a run worth remembering (`store`).
fn request(paths: Vec<PathBuf>, store: bool) -> CheckRequest {
    CheckRequest {
        paths,
        changed: false,
        diff: None,
        rules: Vec::new(),
        store,
        cache: true,
        // An edit hook never spends on a model.
        models: lighthouse_session::ModelFlags {
            off: true,
            ..Default::default()
        },
    }
}

/// The project-relative path an editing tool touches, when it is worth
/// checking: it lies inside the project (`root`, canonical), is not in a
/// directory Lighthouse never reads, and is text when it exists. A file the
/// tool is about to create counts. Everything else is a cheap `None`, before
/// any plugin starts.
fn edited_file(payload: &Value, root: &Path) -> Option<PathBuf> {
    let tool = payload["tool_name"].as_str()?;
    if !EDIT_TOOLS.contains(&tool) {
        return None;
    }
    let absolute = resolved(Path::new(payload["tool_input"]["file_path"].as_str()?))?;
    let relative = absolute.strip_prefix(root).ok()?;
    let skipped = relative
        .components()
        .any(|c| c.as_os_str() == STATE_DIR || SKIPPED_DIRS.iter().any(|d| c.as_os_str() == *d));
    (!skipped && (!absolute.exists() || is_text(&absolute))).then(|| relative.to_owned())
}

/// `path` made canonical as far as it exists, so that a file about to be
/// created resolves like one that is there; `..` in the part that does not
/// exist is refused.
fn resolved(path: &Path) -> Option<PathBuf> {
    let mut missing = PathBuf::new();
    let mut here = path;
    loop {
        if let Ok(found) = here.canonicalize() {
            return Some(found.join(missing));
        }
        missing = Path::new(here.file_name()?).join(missing);
        here = here.parent()?;
    }
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
/// judgment); the remaining findings that ask for review, and incompleteness,
/// are context. Clean and complete is silent.
fn after_edit(checked: &Checked, allow_incomplete: bool, unbaselined: bool) -> Option<Value> {
    let note = unbaselined.then_some(NO_BASELINE);
    let summary = checked.summary();
    let incomplete = summary.status == RunStatus::Incomplete;
    if summary.errors + summary.warnings > 0 || (incomplete && !allow_incomplete) {
        return Some(json!({
            "decision": "block",
            "reason": format!(
                "Lighthouse found problems in the file you just edited. Fix them, then continue.\n{}{}",
                feedback(checked, EDIT_LIMIT),
                note.map_or_else(String::new, |n| format!("\n{n}")),
            ),
        }));
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
            feedback(checked, 0)
        ));
    }
    (!notes.is_empty()).then(|| {
        notes.extend(note.map(str::to_owned));
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
    (!message.is_empty()).then(|| json!({ "systemMessage": message }))
}

/// The agent-format text of the findings and gaps, at most `limit` findings.
fn feedback(checked: &Checked, limit: usize) -> String {
    let outcome = &checked.outcome;
    let fixes = checked.shown_fixes(Some(limit), Detail::default());
    render(
        Format::Agent,
        &outcome.diagnostics,
        &outcome.incomplete,
        &Briefing {
            fixes: Some(&fixes),
            // The agent has the tools of `lighthouse mcp`: hints name them.
            tool_form: true,
            ..checked.briefing(Some(limit))
        },
    )
}
