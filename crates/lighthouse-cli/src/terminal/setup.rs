//! `lighthouse init --agent claude-code`: registers the MCP server, the
//! hooks and the skill in the project. Existing content is merged, never
//! overwritten, and a second run changes nothing.

use std::path::Path;

use lighthouse_resource::{read_optional, write_atomic};
use lighthouse_session::SKILL_MARKER;
use serde_json::{Value, json};

use crate::terminal::{
    Result, docs,
    hook::{COMMAND, EDIT_TOOLS, Event},
};

const MCP_FILE: &str = ".mcp.json";
const SETTINGS_FILE: &str = ".claude/settings.json";
const SKILL_FILE: &str = ".claude/skills/lighthouse/SKILL.md";
/// The permission rule that keeps an agent from trusting a project for the
/// user: trust is the user's decision, made at a terminal.
const DENY_TRUST: &str = "Bash(lighthouse trust:*)";
/// Seconds a hook may run: it analyzes the whole project, so allow a large
/// one time; the Stop hook checks every file the session edited.
const PRE_TIMEOUT: u64 = 120;
const POST_TIMEOUT: u64 = 120;
const STOP_TIMEOUT: u64 = 300;

/// Sets up Claude Code in the project at `root`, printing what changed.
pub fn claude_code(config: Option<&Path>, root: &Path) -> Result<()> {
    mcp(&root.join(MCP_FILE))?;
    hooks(&root.join(SETTINGS_FILE))?;
    skill(config, root)
}

fn mcp(path: &Path) -> Result<()> {
    let mut doc = read_json(path)?;
    let servers = object(&mut doc, "mcpServers", path)?;
    match servers.get("lighthouse") {
        Some(existing) if existing == &entry() => println!("{}: unchanged", path.display()),
        Some(_) => println!(
            "{}: kept your `lighthouse` server entry (it differs from `lighthouse mcp`)",
            path.display()
        ),
        None => {
            servers.insert("lighthouse".to_owned(), entry());
            write_json(path, &doc)?;
            println!("{}: added the `lighthouse` MCP server", path.display());
        }
    }
    Ok(())
}

fn entry() -> Value {
    json!({ "command": "lighthouse", "args": ["mcp"] })
}

fn hooks(path: &Path) -> Result<()> {
    let mut doc = read_json(path)?;
    let hooks = object(&mut doc, "hooks", path)?;
    let mut added = Vec::new();
    let edits = EDIT_TOOLS.join("|");
    for (event, matcher, timeout) in [
        (Event::PreToolUse, Some(&edits), PRE_TIMEOUT),
        (Event::PostToolUse, Some(&edits), POST_TIMEOUT),
        (Event::Stop, None, STOP_TIMEOUT),
    ] {
        let name = event.name();
        let groups = hooks
            .entry(name.to_owned())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("{}: `hooks.{name}` is not a list", path.display()))?;
        if groups.iter().any(ours) {
            continue;
        }
        let mut group = json!({
            "hooks": [{ "type": "command", "command": event.command(), "timeout": timeout }]
        });
        if let Some(matcher) = matcher {
            group["matcher"] = json!(matcher);
        }
        groups.push(group);
        added.push(name);
    }
    let denied = deny_trust(&mut doc, path)?;
    if added.is_empty() && !denied {
        println!("{}: unchanged", path.display());
        return Ok(());
    }
    write_json(path, &doc)?;
    if !added.is_empty() {
        println!("{}: added {} hook(s)", path.display(), added.join(" and "));
    }
    if denied {
        println!("{}: denied `{DENY_TRUST}` to the agent", path.display());
    }
    Ok(())
}

/// Adds the permission deny for `lighthouse trust`; false when it was there.
fn deny_trust(doc: &mut Value, path: &Path) -> Result<bool> {
    let permissions = object(doc, "permissions", path)?;
    let deny = permissions
        .entry("deny".to_owned())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("{}: `permissions.deny` is not a list", path.display()))?;
    if deny.iter().any(|rule| rule == DENY_TRUST) {
        return Ok(false);
    }
    deny.push(json!(DENY_TRUST));
    Ok(true)
}

/// Whether a matcher group already runs a Lighthouse hook.
fn ours(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|hooks| {
        hooks.iter().any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.trim_start().starts_with(COMMAND))
        })
    })
}

fn skill(config: Option<&Path>, root: &Path) -> Result<()> {
    let path = root.join(SKILL_FILE);
    let text = docs::skill_text(config)?;
    match read_optional(&path)? {
        Some(current) if current == text => println!("{}: unchanged", path.display()),
        Some(current) if !current.contains(SKILL_MARKER) => println!(
            "{}: kept your skill file (it is not generated)",
            path.display()
        ),
        Some(_) => {
            write_atomic(&path, &text)?;
            println!("{}: refreshed", path.display());
        }
        None => {
            write_atomic(&path, &text)?;
            println!("{}: wrote the Lighthouse skill", path.display());
        }
    }
    Ok(())
}

/// The JSON object in `path`, an empty one when the file does not exist.
fn read_json(path: &Path) -> Result<Value> {
    match read_optional(path)? {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} is not valid JSON ({e}); fix it or merge by hand",
                path.display()
            )
            .into()
        }),
        _ => Ok(json!({})),
    }
}

fn write_json(path: &Path, doc: &Value) -> Result<()> {
    write_atomic(path, &format!("{}\n", serde_json::to_string_pretty(doc)?))?;
    Ok(())
}

/// The object under `key` of the document, created when missing.
fn object<'a>(
    doc: &'a mut Value,
    key: &str,
    path: &Path,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    let root = doc
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;
    root.entry(key.to_owned())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{}: `{key}` is not an object", path.display()).into())
}
