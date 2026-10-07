//! `lighthouse init --agent claude-code`: registers the MCP server, the
//! hooks and the skill in the project. Existing content is merged, never
//! overwritten, and a second run changes nothing.

use std::{fs, io, path::Path};

use lighthouse_session::{SKILL_MARKER, Session, skill_for, write_atomic};
use serde_json::{Value, json};

use crate::Result;

const MCP_FILE: &str = ".mcp.json";
const SETTINGS_FILE: &str = ".claude/settings.json";
const SKILL_FILE: &str = ".claude/skills/lighthouse/SKILL.md";
const EDIT_TOOLS: &str = "Edit|Write|MultiEdit";
const POST_COMMAND: &str = "lighthouse hook claude-code post-tool-use --allow-incomplete";
const STOP_COMMAND: &str = "lighthouse hook claude-code stop --allow-incomplete";
/// Seconds a hook may run: it analyzes the whole project, so allow a large
/// one time; the Stop hook may also run a project-wide fallback.
const POST_TIMEOUT: u64 = 120;
const STOP_TIMEOUT: u64 = 300;
/// A hook command that starts with this is ours, whatever flags it carries.
const HOOK_PREFIX: &str = "lighthouse hook claude-code";

/// Sets up Claude Code in the project at `root`, printing what changed.
pub fn claude_code(root: &Path) -> Result<()> {
    mcp(&root.join(MCP_FILE))?;
    hooks(&root.join(SETTINGS_FILE))?;
    skill(root)
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
    for (event, matcher, command, timeout) in [
        ("PostToolUse", Some(EDIT_TOOLS), POST_COMMAND, POST_TIMEOUT),
        ("Stop", None, STOP_COMMAND, STOP_TIMEOUT),
    ] {
        let groups = hooks
            .entry(event.to_owned())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("{}: `hooks.{event}` is not a list", path.display()))?;
        if groups.iter().any(ours) {
            continue;
        }
        let mut group = json!({
            "hooks": [{ "type": "command", "command": command, "timeout": timeout }]
        });
        if let Some(matcher) = matcher {
            group["matcher"] = json!(matcher);
        }
        groups.push(group);
        added.push(event);
    }
    if added.is_empty() {
        println!("{}: unchanged", path.display());
        return Ok(());
    }
    write_json(path, &doc)?;
    println!("{}: added {} hook(s)", path.display(), added.join(" and "));
    Ok(())
}

/// Whether a matcher group already runs a Lighthouse hook.
fn ours(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|hooks| {
        hooks.iter().any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.trim_start().starts_with(HOOK_PREFIX))
        })
    })
}

fn skill(root: &Path) -> Result<()> {
    let path = root.join(SKILL_FILE);
    let text = skill_for(&Session::load_or_default(None)?)?;
    match fs::read_to_string(&path) {
        Ok(current) if current == text => println!("{}: unchanged", path.display()),
        Ok(current) if !current.contains(SKILL_MARKER) => println!(
            "{}: kept your skill file (it is not generated)",
            path.display()
        ),
        Ok(_) => {
            write_atomic(&path, &text)?;
            println!("{}: refreshed", path.display());
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            write_atomic(&path, &text)?;
            println!("{}: wrote the Lighthouse skill", path.display());
        }
        Err(e) => return Err(format!("cannot read {}: {e}", path.display()).into()),
    }
    Ok(())
}

/// The JSON object in `path`, an empty one when the file does not exist.
fn read_json(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} is not valid JSON ({e}); fix it or merge by hand",
                path.display()
            )
            .into()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("cannot read {}: {e}", path.display()).into()),
    }
}

fn write_json(path: &Path, doc: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
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
